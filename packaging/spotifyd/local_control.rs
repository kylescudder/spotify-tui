use std::{
    env, fs,
    fs::OpenOptions,
    future::Future,
    io::{self, ErrorKind, Write},
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use color_eyre::eyre::{self, Context as _};
use librespot_connect::{LoadContextOptions, LoadRequest, LoadRequestOptions, Spirc};
use librespot_core::{Session, SpotifyUri};
use librespot_metadata::audio::{AudioItem, UniqueFields};
use librespot_playback::player::PlayerEvent;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
};

const CONTROL_ADDRESS_ENV: &str = "SPOTIFY_TUI_CONTROL_ADDRESS";
const CONTROL_ADDRESS_FILE_ENV: &str = "SPOTIFY_TUI_CONTROL_ADDRESS_FILE";
const CONTROL_TOKEN_FILE_ENV: &str = "SPOTIFY_TUI_CONTROL_TOKEN_FILE";
const MAX_REQUEST_BYTES: u64 = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const PROTOCOL_VERSION: u8 = 1;

enum ControlMessage {
    SetSession(Arc<Spirc>, Session),
    DropSession,
    Shutdown,
}

pub(crate) struct LocalControlServer {
    server: Pin<Box<dyn Future<Output = eyre::Result<()>>>>,
    control_tx: UnboundedSender<ControlMessage>,
}

impl LocalControlServer {
    pub(crate) fn new(event_rx: UnboundedReceiver<PlayerEvent>) -> Self {
        let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            server: Box::pin(create_server(event_rx, control_rx)),
            control_tx,
        }
    }

    pub(crate) fn set_session(&self, spirc: Arc<Spirc>, session: Session) -> eyre::Result<()> {
        self.control_tx
            .send(ControlMessage::SetSession(spirc, session))
            .map_err(|_| eyre::eyre!("local control channel closed unexpectedly"))
    }

    pub(crate) fn drop_session(&self) -> eyre::Result<()> {
        self.control_tx
            .send(ControlMessage::DropSession)
            .map_err(|_| eyre::eyre!("local control channel closed unexpectedly"))
    }

    pub(crate) fn shutdown(&self) -> bool {
        self.control_tx.send(ControlMessage::Shutdown).is_ok()
    }
}

impl Future for LocalControlServer {
    type Output = eyre::Result<()>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.server.as_mut().poll(context)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
}

impl PlaybackStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Debug)]
struct Position {
    value: Duration,
    updated_at: Instant,
}

impl Position {
    fn new(milliseconds: u32) -> Self {
        Self {
            value: Duration::from_millis(u64::from(milliseconds)),
            updated_at: Instant::now(),
        }
    }

    fn current(&self, status: PlaybackStatus) -> Duration {
        if status == PlaybackStatus::Playing {
            self.value.saturating_add(self.updated_at.elapsed())
        } else {
            self.value
        }
    }
}

#[derive(Debug)]
struct CurrentState {
    status: PlaybackStatus,
    position: Option<Position>,
    audio_item: Option<Box<AudioItem>>,
    volume: u16,
    active_connection: Option<String>,
    shuffle: bool,
    repeat_context: bool,
    repeat_track: bool,
    play_request_id: Option<u64>,
}

impl Default for CurrentState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            position: None,
            audio_item: None,
            volume: u16::MAX,
            active_connection: None,
            shuffle: false,
            repeat_context: false,
            repeat_track: false,
            play_request_id: None,
        }
    }
}

impl CurrentState {
    fn handle_event(&mut self, event: PlayerEvent) {
        if Option::zip(self.play_request_id, event.get_play_request_id())
            .is_some_and(|(current, incoming)| current != incoming)
        {
            debug!("discarding local control event due to play_request_id mismatch");
            return;
        }

        match event {
            PlayerEvent::VolumeChanged { volume } => self.volume = volume,
            PlayerEvent::Stopped { .. } => {
                self.status = PlaybackStatus::Stopped;
                self.position = None;
                self.audio_item = None;
            }
            PlayerEvent::Playing { position_ms, .. } => {
                self.status = PlaybackStatus::Playing;
                self.position = Some(Position::new(position_ms));
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.status = PlaybackStatus::Paused;
                self.position = Some(Position::new(position_ms));
            }
            PlayerEvent::TrackChanged { audio_item } => self.audio_item = Some(audio_item),
            PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. }
            | PlayerEvent::Seeked { position_ms, .. } => {
                self.position = Some(Position::new(position_ms));
            }
            PlayerEvent::ShuffleChanged { shuffle } => self.shuffle = shuffle,
            PlayerEvent::RepeatChanged { context, track } => {
                self.repeat_context = context;
                self.repeat_track = track;
            }
            PlayerEvent::PlayRequestIdChanged { play_request_id } => {
                self.play_request_id = Some(play_request_id);
            }
            PlayerEvent::SessionConnected { connection_id, .. } => {
                self.active_connection = Some(connection_id);
            }
            PlayerEvent::SessionDisconnected { connection_id, .. } => {
                if self.active_connection.as_deref() == Some(connection_id.as_str()) {
                    self.active_connection = None;
                }
            }
            PlayerEvent::Preloading { .. }
            | PlayerEvent::Loading { .. }
            | PlayerEvent::TimeToPreloadNextTrack { .. }
            | PlayerEvent::EndOfTrack { .. }
            | PlayerEvent::Unavailable { .. }
            | PlayerEvent::AutoPlayChanged { .. }
            | PlayerEvent::FilterExplicitContentChanged { .. }
            | PlayerEvent::SessionClientChanged { .. } => {}
        }
    }

    fn snapshot(&self) -> WireSnapshot {
        let mut snapshot = WireSnapshot {
            status: self.status.as_str().to_owned(),
            track_id: None,
            title: None,
            artists: Vec::new(),
            album: None,
            duration_us: None,
            art_url: None,
            position_us: self
                .position
                .as_ref()
                .map_or(0, |position| duration_micros(position.current(self.status))),
            volume: f64::from(self.volume) / f64::from(u16::MAX),
        };

        let Some(item) = self.audio_item.as_deref() else {
            return snapshot;
        };
        snapshot.track_id = item.track_id.to_uri().ok();
        snapshot.title = Some(item.name.clone());
        snapshot.duration_us = Some(u64::from(item.duration_ms) * 1_000);
        snapshot.art_url = item
            .covers
            .iter()
            .max_by_key(|image| image.width)
            .map(|image| image.url.clone());
        match &item.unique_fields {
            UniqueFields::Local { artists, album, .. } => {
                snapshot.artists = artists.iter().cloned().collect();
                snapshot.album.clone_from(album);
            }
            UniqueFields::Track { artists, album, .. } => {
                snapshot.artists = artists.iter().map(|artist| artist.name.clone()).collect();
                snapshot.album = Some(album.clone());
            }
            UniqueFields::Episode { show_name, .. } => {
                snapshot.artists = vec![show_name.clone()];
            }
        }
        snapshot
    }
}

async fn create_server(
    mut event_rx: UnboundedReceiver<PlayerEvent>,
    mut control_rx: UnboundedReceiver<ControlMessage>,
) -> eyre::Result<()> {
    let configured_address = configured_address()?;
    let listener = TcpListener::bind(
        configured_address.unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0))),
    )
    .await
    .wrap_err("could not bind local control server")?;
    let address = listener
        .local_addr()
        .wrap_err("could not read local control address")?;
    if configured_address.is_none() {
        write_control_address(address)?;
    }
    let token = load_or_create_token()?;
    info!("Local playback control listening on {address}");

    let mut state = CurrentState::default();
    let mut spirc = None;
    let mut session = None;
    loop {
        tokio::select! {
            connection = listener.accept() => {
                let (stream, peer) = connection.wrap_err("local control listener failed")?;
                if peer.ip().is_loopback()
                    && let Err(error) = handle_connection(
                        stream,
                        &token,
                        &state,
                        spirc.clone(),
                        session.clone(),
                    ).await
                {
                    debug!("local control request failed: {error}");
                }
            }
            event = event_rx.recv() => {
                state.handle_event(event.ok_or_else(|| {
                    eyre::eyre!("local control event channel closed unexpectedly")
                })?);
            }
            control = control_rx.recv() => {
                match control.ok_or_else(|| {
                    eyre::eyre!("local control channel closed unexpectedly")
                })? {
                    ControlMessage::SetSession(new_spirc, new_session) => {
                        spirc = Some(new_spirc);
                        session = Some(new_session);
                    }
                    ControlMessage::DropSession => {
                        spirc = None;
                        session = None;
                        state.active_connection = None;
                    }
                    ControlMessage::Shutdown => break,
                }
            }
        }
    }
    Ok(())
}

async fn handle_connection(
    stream: TcpStream,
    expected_token: &str,
    state: &CurrentState,
    spirc: Option<Arc<Spirc>>,
    session: Option<Session>,
) -> eyre::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut request = String::new();
    let bytes_read = tokio::time::timeout(
        REQUEST_TIMEOUT,
        BufReader::new(read)
            .take(MAX_REQUEST_BYTES)
            .read_line(&mut request),
    )
    .await
    .wrap_err("local control request timed out")?
    .wrap_err("could not read local control request")?;
    if bytes_read == 0 {
        return Ok(());
    }
    let request: ControlRequest =
        serde_json::from_str(&request).wrap_err("local control request was not valid JSON")?;
    let response = if request.version != PROTOCOL_VERSION {
        ControlResponse::error("unsupported protocol version")
    } else if !constant_time_eq(request.token.as_bytes(), expected_token.as_bytes()) {
        ControlResponse::error("authentication failed")
    } else {
        execute(request.operation, state, spirc, session).await
    };
    let mut response = serde_json::to_vec(&response)?;
    response.push(b'\n');
    tokio::time::timeout(REQUEST_TIMEOUT, write.write_all(&response))
        .await
        .wrap_err("local control response timed out")?
        .wrap_err("could not write local control response")
}

async fn execute(
    operation: ControlOperation,
    state: &CurrentState,
    spirc: Option<Arc<Spirc>>,
    session: Option<Session>,
) -> ControlResponse {
    if matches!(operation, ControlOperation::Snapshot) {
        return if state.active_connection.is_some() {
            ControlResponse::snapshot(state.snapshot())
        } else {
            ControlResponse::disconnected()
        };
    }

    let Some(spirc) = spirc else {
        return ControlResponse::disconnected();
    };
    let result = match operation {
        ControlOperation::Snapshot => unreachable!(),
        ControlOperation::Activate => spirc.activate().map_err(|error| error.to_string()),
        ControlOperation::Toggle => spirc.play_pause().map_err(|error| error.to_string()),
        ControlOperation::Previous => spirc.prev().map_err(|error| error.to_string()),
        ControlOperation::Next => spirc.next().map_err(|error| error.to_string()),
        ControlOperation::SeekBy { microseconds } => seek_by(&spirc, state, microseconds),
        ControlOperation::SetVolume { volume } => spirc
            .set_volume((volume.clamp(0.0, 1.0) * f64::from(u16::MAX)) as u16)
            .map_err(|error| error.to_string()),
        ControlOperation::OpenUri { uri } => match session {
            Some(session) => open_uri(&spirc, session, state, &uri).await,
            None => Err("spotify session is unavailable".to_owned()),
        },
    };
    match result {
        Ok(()) => ControlResponse::ok(),
        Err(error) => ControlResponse::error(error),
    }
}

fn seek_by(spirc: &Spirc, state: &CurrentState, microseconds: i64) -> Result<(), String> {
    let current = state
        .position
        .as_ref()
        .map(|position| position.current(state.status))
        .ok_or_else(|| "cannot seek while playback is stopped".to_owned())?;
    let target = if microseconds.is_negative() {
        current.saturating_sub(Duration::from_micros(microseconds.unsigned_abs()))
    } else {
        current.saturating_add(Duration::from_micros(microseconds as u64))
    };
    let milliseconds = u32::try_from(target.as_millis())
        .map_err(|error| format!("seek position is out of bounds: {error}"))?;
    spirc
        .set_position_ms(milliseconds)
        .map_err(|error| error.to_string())
}

async fn open_uri(
    spirc: &Spirc,
    session: Session,
    state: &CurrentState,
    value: &str,
) -> Result<(), String> {
    use librespot_metadata::{Metadata as _, Track};

    let uri = SpotifyUri::from_uri(value).map_err(|error| error.to_string())?;
    let (playing_track, context_uri) = match uri {
        SpotifyUri::Track { .. } => {
            let track = Track::get(&session, &uri)
                .await
                .map_err(|error| error.to_string())?;
            let context = track.album.id.to_uri().map_err(|error| error.to_string())?;
            let track_uri = uri.to_uri().map_err(|error| error.to_string())?;
            (librespot_connect::PlayingTrack::Uri(track_uri), context)
        }
        SpotifyUri::Album { .. }
        | SpotifyUri::Artist { .. }
        | SpotifyUri::Playlist { .. }
        | SpotifyUri::Episode { .. }
        | SpotifyUri::Show { .. } => (
            librespot_connect::PlayingTrack::Index(0),
            uri.to_uri().map_err(|error| error.to_string())?,
        ),
        SpotifyUri::Local { .. } | SpotifyUri::Unknown { .. } => {
            return Err("this type of Spotify URI is not supported".to_owned());
        }
    };

    spirc
        .load(LoadRequest::from_context_uri(
            context_uri,
            LoadRequestOptions {
                start_playing: true,
                seek_to: 0,
                context_options: Some(LoadContextOptions::Options(librespot_connect::Options {
                    shuffle: state.shuffle,
                    repeat: state.repeat_context,
                    repeat_track: state.repeat_track,
                })),
                playing_track: Some(playing_track),
            },
        ))
        .map_err(|error| error.to_string())
}

fn configured_address() -> eyre::Result<Option<SocketAddr>> {
    let Ok(value) = env::var(CONTROL_ADDRESS_ENV) else {
        return Ok(None);
    };
    let address: SocketAddr = value
        .parse()
        .wrap_err_with(|| format!("invalid {CONTROL_ADDRESS_ENV} value"))?;
    if !matches!(address.ip(), IpAddr::V4(ip) if ip.is_loopback())
        && !matches!(address.ip(), IpAddr::V6(ip) if ip.is_loopback())
    {
        return Err(eyre::eyre!("local control address must use a loopback IP"));
    }
    Ok(Some(address))
}

fn data_file(environment: &str, name: &str) -> eyre::Result<PathBuf> {
    if let Some(path) = env::var_os(environment) {
        return Ok(path.into());
    }
    directories::BaseDirs::new()
        .map(|directories| directories.data_local_dir().join("spotify-tui").join(name))
        .ok_or_else(|| eyre::eyre!("could not resolve the local data directory"))
}

fn load_or_create_token() -> eyre::Result<String> {
    let path = data_file(CONTROL_TOKEN_FILE_ENV, "spotifyd-control-token")?;
    if let Ok(token) = fs::read_to_string(&path)
        && !token.trim().is_empty()
    {
        return Ok(token.trim().to_owned());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err("could not create local control data directory")?;
    }
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| eyre::eyre!(error.to_string()))?;
    let token = hex::encode(bytes);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            restrict_permissions(&file)?;
            writeln!(file, "{token}").wrap_err("could not write local control token")?;
            Ok(token)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let existing = fs::read_to_string(&path)
                .wrap_err("could not read local control token")?
                .trim()
                .to_owned();
            if !existing.is_empty() {
                return Ok(existing);
            }
            let mut file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&path)
                .wrap_err("could not repair local control token")?;
            restrict_permissions(&file)?;
            writeln!(file, "{token}").wrap_err("could not write local control token")?;
            Ok(token)
        }
        Err(error) => Err(error).wrap_err("could not create local control token"),
    }
}

fn write_control_address(address: SocketAddr) -> eyre::Result<()> {
    let path = data_file(CONTROL_ADDRESS_FILE_ENV, "spotifyd-control-address")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err("could not create local control data directory")?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .wrap_err("could not create local control address file")?;
    restrict_permissions(&file)?;
    writeln!(file, "{address}").wrap_err("could not write local control address")
}

#[cfg(unix)]
fn restrict_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[derive(Debug, Deserialize)]
struct ControlRequest {
    version: u8,
    token: String,
    #[serde(flatten)]
    operation: ControlOperation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ControlOperation {
    Snapshot,
    Activate,
    Toggle,
    Previous,
    Next,
    SeekBy { microseconds: i64 },
    SetVolume { volume: f64 },
    OpenUri { uri: String },
}

#[derive(Debug, Serialize)]
struct ControlResponse {
    version: u8,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<WireSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<&'static str>,
}

impl ControlResponse {
    fn ok() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            ok: true,
            snapshot: None,
            error: None,
            error_code: None,
        }
    }

    fn snapshot(snapshot: WireSnapshot) -> Self {
        Self {
            snapshot: Some(snapshot),
            ..Self::ok()
        }
    }

    fn error(error: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            ok: false,
            snapshot: None,
            error: Some(error.into()),
            error_code: None,
        }
    }

    fn disconnected() -> Self {
        Self {
            error_code: Some("disconnected"),
            ..Self::error("spotifyd is disconnected")
        }
    }
}

#[derive(Debug, Serialize)]
struct WireSnapshot {
    status: String,
    track_id: Option<String>,
    title: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    duration_us: Option<u64>,
    art_url: Option<String>,
    position_us: u64,
    volume: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_comparison_requires_equal_bytes_and_length() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"diff"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn state_tracks_active_connection_playback_and_volume() {
        let mut state = CurrentState::default();
        state.handle_event(PlayerEvent::SessionConnected {
            connection_id: "current".to_owned(),
            user_name: "listener".to_owned(),
        });
        state.handle_event(PlayerEvent::Playing {
            play_request_id: 7,
            track_id: SpotifyUri::from_uri("spotify:track:4cOdK2wGLETKBW3PvgPWqT")
                .expect("test URI should be valid"),
            position_ms: 1_500,
        });
        state.handle_event(PlayerEvent::VolumeChanged {
            volume: u16::MAX / 2,
        });

        let snapshot = state.snapshot();
        assert_eq!(state.active_connection.as_deref(), Some("current"));
        assert_eq!(snapshot.status, "playing");
        assert!(snapshot.position_us >= 1_500_000);
        assert!((snapshot.volume - 0.5).abs() < 0.001);

        state.handle_event(PlayerEvent::SessionDisconnected {
            connection_id: "stale".to_owned(),
            user_name: "listener".to_owned(),
        });
        assert_eq!(state.active_connection.as_deref(), Some("current"));
        state.handle_event(PlayerEvent::SessionDisconnected {
            connection_id: "current".to_owned(),
            user_name: "listener".to_owned(),
        });
        assert!(state.active_connection.is_none());
    }
}
