use std::{
    fs,
    io::{self, ErrorKind},
    net::SocketAddr,
    path::PathBuf,
    time::Duration,
};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::timeout,
};

use crate::playback::{
    PlaybackCommand, PlaybackError, PlaybackSnapshot, PlaybackSource, PlaybackStatus, TrackMetadata,
};

pub const CONTROL_ADDRESS_ENV: &str = "SPOTIFY_TUI_CONTROL_ADDRESS";
pub const CONTROL_ADDRESS_FILE_ENV: &str = "SPOTIFY_TUI_CONTROL_ADDRESS_FILE";
pub const CONTROL_TOKEN_FILE_ENV: &str = "SPOTIFY_TUI_CONTROL_TOKEN_FILE";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const PROTOCOL_VERSION: u8 = 1;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct LocalControlPlaybackSource {
    endpoint: ControlEndpoint,
    token_file: PathBuf,
    poll_interval: Duration,
}

impl LocalControlPlaybackSource {
    pub async fn connect() -> Result<Self, PlaybackError> {
        let endpoint = match std::env::var(CONTROL_ADDRESS_ENV) {
            Ok(address) => ControlEndpoint::Fixed(address.parse().map_err(|error| {
                PlaybackError::Control(format!("invalid control address: {error}"))
            })?),
            Err(_) => {
                ControlEndpoint::Discovered(match std::env::var_os(CONTROL_ADDRESS_FILE_ENV) {
                    Some(path) => PathBuf::from(path),
                    None => default_data_file("spotifyd-control-address")?,
                })
            }
        };
        let token_file = match std::env::var_os(CONTROL_TOKEN_FILE_ENV) {
            Some(path) => PathBuf::from(path),
            None => default_data_file("spotifyd-control-token")?,
        };

        Ok(Self {
            endpoint,
            token_file,
            poll_interval: POLL_INTERVAL,
        })
    }

    #[cfg(test)]
    fn connect_to(address: SocketAddr, token_file: PathBuf) -> Self {
        Self {
            endpoint: ControlEndpoint::Fixed(address),
            token_file,
            poll_interval: Duration::from_millis(1),
        }
    }

    async fn request(&self, operation: ControlOperation) -> Result<ControlResponse, PlaybackError> {
        let token = fs::read_to_string(&self.token_file)
            .map_err(token_error)?
            .trim()
            .to_owned();
        if token.is_empty() {
            return Err(PlaybackError::Disconnected);
        }

        timeout(REQUEST_TIMEOUT, self.request_with_token(operation, token))
            .await
            .map_err(|_| PlaybackError::Disconnected)?
    }

    async fn request_with_token(
        &self,
        operation: ControlOperation,
        token: String,
    ) -> Result<ControlResponse, PlaybackError> {
        let mut stream = TcpStream::connect(self.endpoint.address()?)
            .await
            .map_err(network_error)?;
        let request = ControlRequest {
            version: PROTOCOL_VERSION,
            token,
            operation,
        };
        let mut request = serde_json::to_vec(&request)
            .map_err(|error| PlaybackError::Control(error.to_string()))?;
        request.push(b'\n');
        stream.write_all(&request).await.map_err(network_error)?;
        stream.shutdown().await.map_err(network_error)?;

        let mut response = String::new();
        let bytes_read = BufReader::new(stream)
            .take(MAX_RESPONSE_BYTES)
            .read_line(&mut response)
            .await
            .map_err(network_error)?;
        if bytes_read == 0 {
            return Err(PlaybackError::Disconnected);
        }

        let response: ControlResponse = serde_json::from_str(&response)
            .map_err(|error| PlaybackError::Control(format!("invalid response: {error}")))?;
        if response.version != PROTOCOL_VERSION {
            return Err(PlaybackError::Control(format!(
                "unsupported control protocol version {}",
                response.version
            )));
        }
        if response.ok {
            Ok(response)
        } else if response.error_code.as_deref() == Some("disconnected") {
            Err(PlaybackError::Disconnected)
        } else {
            Err(PlaybackError::Control(
                response
                    .error
                    .unwrap_or_else(|| "request was rejected".to_owned()),
            ))
        }
    }

    async fn request_snapshot(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        self.request(ControlOperation::Snapshot)
            .await?
            .snapshot
            .map(Into::into)
            .ok_or(PlaybackError::Disconnected)
    }
}

impl PlaybackSource for LocalControlPlaybackSource {
    async fn activate(&self) -> Result<(), PlaybackError> {
        self.request(ControlOperation::Activate).await.map(|_| ())
    }

    async fn snapshot(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        self.request_snapshot().await
    }

    async fn wait_for_change(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        tokio::time::sleep(self.poll_interval).await;
        self.request_snapshot().await
    }

    async fn execute(&self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        let operation = match command {
            PlaybackCommand::Toggle => ControlOperation::Toggle,
            PlaybackCommand::Previous => ControlOperation::Previous,
            PlaybackCommand::Next => ControlOperation::Next,
            PlaybackCommand::SeekBy(microseconds) => ControlOperation::SeekBy { microseconds },
            PlaybackCommand::SetVolume(volume) => ControlOperation::SetVolume {
                volume: volume.clamp(0.0, 1.0),
            },
            PlaybackCommand::OpenUri(uri) => ControlOperation::OpenUri { uri },
        };
        self.request(operation).await.map(|_| ())
    }
}

#[derive(Debug, Clone)]
enum ControlEndpoint {
    Fixed(SocketAddr),
    Discovered(PathBuf),
}

impl ControlEndpoint {
    fn address(&self) -> Result<SocketAddr, PlaybackError> {
        match self {
            Self::Fixed(address) => Ok(*address),
            Self::Discovered(path) => fs::read_to_string(path)
                .map_err(token_error)?
                .trim()
                .parse()
                .map_err(|_| PlaybackError::Disconnected),
        }
    }
}

fn default_data_file(name: &str) -> Result<PathBuf, PlaybackError> {
    BaseDirs::new()
        .map(|directories| directories.data_local_dir().join("spotify-tui").join(name))
        .ok_or_else(|| PlaybackError::Control("could not resolve the local data directory".into()))
}

fn token_error(error: io::Error) -> PlaybackError {
    if error.kind() == ErrorKind::NotFound {
        PlaybackError::Disconnected
    } else {
        PlaybackError::Control(format!("could not read control token: {error}"))
    }
}

fn network_error(error: io::Error) -> PlaybackError {
    match error.kind() {
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::BrokenPipe
        | ErrorKind::NotConnected
        | ErrorKind::TimedOut
        | ErrorKind::UnexpectedEof => PlaybackError::Disconnected,
        _ => PlaybackError::Control(error.to_string()),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ControlRequest {
    version: u8,
    token: String,
    #[serde(flatten)]
    operation: ControlOperation,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
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

#[derive(Debug, Serialize, Deserialize)]
struct ControlResponse {
    version: u8,
    ok: bool,
    #[serde(default)]
    snapshot: Option<WireSnapshot>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireSnapshot {
    status: String,
    #[serde(default)]
    track_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artists: Vec<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    duration_us: Option<u64>,
    #[serde(default)]
    art_url: Option<String>,
    position_us: u64,
    volume: f64,
}

impl From<WireSnapshot> for PlaybackSnapshot {
    fn from(snapshot: WireSnapshot) -> Self {
        let status = match snapshot.status.to_ascii_lowercase().as_str() {
            "playing" => PlaybackStatus::Playing,
            "paused" => PlaybackStatus::Paused,
            "stopped" => PlaybackStatus::Stopped,
            _ => PlaybackStatus::Unknown(snapshot.status),
        };
        Self {
            status,
            track: TrackMetadata {
                track_id: snapshot.track_id,
                title: snapshot.title,
                artists: snapshot.artists,
                album: snapshot.album,
                duration: snapshot.duration_us.map(Duration::from_micros),
                art_url: snapshot.art_url,
            },
            position: Duration::from_micros(snapshot.position_us),
            volume: snapshot.volume.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpListener,
        sync::Mutex,
    };

    use super::*;

    #[test]
    fn wire_snapshot_is_normalized() {
        let snapshot: PlaybackSnapshot = WireSnapshot {
            status: "PLAYING".to_owned(),
            track_id: Some("spotify:track:one".to_owned()),
            title: Some("One".to_owned()),
            artists: vec!["Artist".to_owned()],
            album: Some("Album".to_owned()),
            duration_us: Some(3_000_000),
            art_url: Some("https://example.com/cover.jpg".to_owned()),
            position_us: 1_000_000,
            volume: 1.5,
        }
        .into();

        assert_eq!(snapshot.status, PlaybackStatus::Playing);
        assert_eq!(snapshot.position, Duration::from_secs(1));
        assert_eq!(snapshot.track.duration, Some(Duration::from_secs(3)));
        assert_eq!(snapshot.volume, 1.0);
    }

    #[tokio::test]
    async fn adapter_authenticates_and_translates_commands() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("listener should have an address");
        let token_file =
            std::env::temp_dir().join(format!("spotify-tui-control-token-{}", std::process::id()));
        fs::write(&token_file, "secret\n").expect("test token should be written");
        let operations = Arc::new(Mutex::new(Vec::new()));
        let server_operations = operations.clone();

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.expect("request should connect");
                let (read, mut write) = stream.into_split();
                let mut request = String::new();
                BufReader::new(read)
                    .read_line(&mut request)
                    .await
                    .expect("request should be readable");
                let request: ControlRequest =
                    serde_json::from_str(&request).expect("request should be valid JSON");
                assert_eq!(request.version, PROTOCOL_VERSION);
                assert_eq!(request.token, "secret");
                server_operations.lock().await.push(request.operation);
                let response = ControlResponse {
                    version: PROTOCOL_VERSION,
                    ok: true,
                    snapshot: Some(WireSnapshot {
                        status: "paused".to_owned(),
                        track_id: None,
                        title: Some("Track".to_owned()),
                        artists: vec![],
                        album: None,
                        duration_us: None,
                        art_url: None,
                        position_us: 0,
                        volume: 0.5,
                    }),
                    error: None,
                    error_code: None,
                };
                let mut response =
                    serde_json::to_vec(&response).expect("response should serialize");
                response.push(b'\n');
                write
                    .write_all(&response)
                    .await
                    .expect("response should be writable");
            }
        });

        let source = LocalControlPlaybackSource::connect_to(address, token_file.clone());
        let snapshot = source.snapshot().await.expect("snapshot should load");
        assert_eq!(snapshot.track.title.as_deref(), Some("Track"));
        source
            .execute(PlaybackCommand::SeekBy(-5_000_000))
            .await
            .expect("command should succeed");
        server.await.expect("server should finish");

        assert_eq!(
            *operations.lock().await,
            vec![
                ControlOperation::Snapshot,
                ControlOperation::SeekBy {
                    microseconds: -5_000_000
                }
            ]
        );
        fs::remove_file(token_file).expect("test token should be removed");
    }
}
