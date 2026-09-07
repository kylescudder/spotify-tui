use std::{fmt, future::Future, time::Duration};

#[cfg(target_os = "linux")]
use futures_util::StreamExt;
#[cfg(target_os = "linux")]
use std::collections::HashMap;
use thiserror::Error;
#[cfg(target_os = "linux")]
use zbus::{
    Connection,
    names::OwnedBusName,
    zvariant::{OwnedObjectPath, OwnedValue},
};

#[cfg(target_os = "linux")]
const SPOTIFYD_BUS_NAME: &str = "org.mpris.MediaPlayer2.spotifyd";
#[cfg(target_os = "linux")]
const SPOTIFYD_CONTROL_BUS_NAME: &str = "rs.spotifyd";

#[cfg(target_os = "linux")]
fn is_spotifyd_bus_name(name: &str) -> bool {
    is_spotifyd_instance_name(name, SPOTIFYD_BUS_NAME)
}

#[cfg(target_os = "linux")]
fn is_spotifyd_control_bus_name(name: &str) -> bool {
    is_spotifyd_instance_name(name, SPOTIFYD_CONTROL_BUS_NAME)
}

#[cfg(target_os = "linux")]
fn is_spotifyd_instance_name(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|suffix| suffix.strip_prefix(".instance"))
            .is_some_and(|instance| {
                !instance.is_empty() && instance.chars().all(|character| character.is_ascii_digit())
            })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    Stopped,
    Unknown(String),
}

impl From<String> for PlaybackStatus {
    fn from(status: String) -> Self {
        match status.as_str() {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            "Stopped" => Self::Stopped,
            _ => Self::Unknown(status),
        }
    }
}

impl fmt::Display for PlaybackStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Playing => formatter.write_str("playing"),
            Self::Paused => formatter.write_str("paused"),
            Self::Stopped => formatter.write_str("stopped"),
            Self::Unknown(status) => write!(formatter, "unknown ({status})"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    pub track_id: Option<String>,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub art_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub status: PlaybackStatus,
    pub track: TrackMetadata,
    pub position: Duration,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackCommand {
    Toggle,
    Previous,
    Next,
    SeekBy(i64),
    SetVolume(f64),
    OpenUri(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlaybackError {
    #[error("spotifyd is disconnected")]
    Disconnected,
    #[error("could not connect to the session bus: {0}")]
    SessionBus(String),
    #[error("MPRIS request failed: {0}")]
    Mpris(String),
    #[error("Spotifyd control request failed: {0}")]
    Control(String),
    #[error("the local playback adapter is not implemented for {0}")]
    UnsupportedPlatform(&'static str),
}

pub trait PlaybackSource {
    fn activate(&self) -> impl Future<Output = Result<(), PlaybackError>> + Send;

    fn snapshot(&self) -> impl Future<Output = Result<PlaybackSnapshot, PlaybackError>> + Send;

    fn wait_for_change(
        &self,
    ) -> impl Future<Output = Result<PlaybackSnapshot, PlaybackError>> + Send;

    fn execute(
        &self,
        command: PlaybackCommand,
    ) -> impl Future<Output = Result<(), PlaybackError>> + Send;
}

#[cfg(target_os = "linux")]
pub struct MprisPlaybackSource {
    connection: Connection,
}

#[cfg(target_os = "linux")]
pub type PlatformPlaybackSource = MprisPlaybackSource;

#[cfg(not(target_os = "linux"))]
pub type PlatformPlaybackSource = crate::local_control::LocalControlPlaybackSource;

#[cfg(target_os = "linux")]
impl MprisPlaybackSource {
    pub async fn connect() -> Result<Self, PlaybackError> {
        let connection = Connection::session()
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        Ok(Self { connection })
    }

    #[cfg(test)]
    pub(crate) async fn connect_to_address(address: String) -> Result<Self, PlaybackError> {
        let connection = zbus::connection::Builder::address(address.as_str())
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?
            .build()
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        Ok(Self { connection })
    }

    async fn spotifyd_bus_name(&self) -> Result<Option<OwnedBusName>, PlaybackError> {
        self.spotifyd_named_bus(is_spotifyd_bus_name).await
    }

    async fn spotifyd_control_bus_name(&self) -> Result<Option<OwnedBusName>, PlaybackError> {
        self.spotifyd_named_bus(is_spotifyd_control_bus_name).await
    }

    async fn spotifyd_named_bus(
        &self,
        matches: fn(&str) -> bool,
    ) -> Result<Option<OwnedBusName>, PlaybackError> {
        let dbus = zbus::fdo::DBusProxy::new(&self.connection)
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        let names = dbus
            .list_names()
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        Ok(names
            .into_iter()
            .filter(|name| matches(name.as_str()))
            .min_by(|left, right| left.as_str().cmp(right.as_str())))
    }

    async fn bus_name_is_owned(&self, expected: &str) -> Result<bool, PlaybackError> {
        let dbus = zbus::fdo::DBusProxy::new(&self.connection)
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;
        let names = dbus
            .list_names()
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        Ok(names.iter().any(|name| name.as_str() == expected))
    }

    async fn player(&self) -> Result<MprisPlayerProxy<'_>, PlaybackError> {
        let service = self
            .spotifyd_bus_name()
            .await?
            .ok_or(PlaybackError::Disconnected)?;

        MprisPlayerProxy::builder(&self.connection)
            .destination(service)
            .map_err(mpris_error)?
            .build()
            .await
            .map_err(mpris_error)
    }
}

#[cfg(target_os = "linux")]
impl PlaybackSource for MprisPlaybackSource {
    async fn activate(&self) -> Result<(), PlaybackError> {
        let service = self
            .spotifyd_control_bus_name()
            .await?
            .ok_or(PlaybackError::Disconnected)?;
        let controls = SpotifydControlsProxy::builder(&self.connection)
            .destination(service)
            .map_err(control_error)?
            .build()
            .await
            .map_err(control_error)?;

        controls.transfer_playback().await.map_err(control_error)
    }

    async fn snapshot(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        let player = self.player().await?;

        let (status, metadata, position, volume) = tokio::join!(
            player.playback_status(),
            player.metadata(),
            player.position(),
            player.volume(),
        );
        let status = status.map_err(mpris_error)?;
        let metadata = metadata.map_err(mpris_error)?;
        // Spotifyd can expose a usable player before its session has a current
        // track position. Keep the valid state and begin at zero until the next
        // property or Seeked signal supplies an authoritative position.
        let position = position.unwrap_or_default();
        let volume = volume.map_err(mpris_error)?;

        Ok(PlaybackSnapshot {
            status: status.into(),
            track: normalize_metadata(metadata),
            position: microseconds_to_duration(position),
            volume,
        })
    }

    async fn wait_for_change(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        let player = self.player().await?;
        let service = player.inner().destination().to_owned();
        let properties = zbus::fdo::PropertiesProxy::builder(&self.connection)
            .destination(service.clone())
            .map_err(mpris_error)?
            .path("/org/mpris/MediaPlayer2")
            .map_err(mpris_error)?
            .build()
            .await
            .map_err(mpris_error)?;
        let mut property_changes = properties
            .receive_properties_changed()
            .await
            .map_err(mpris_error)?;
        let mut seeked = player.receive_seeked().await.map_err(mpris_error)?;
        let mut owner_changes = player
            .inner()
            .receive_owner_changed()
            .await
            .map_err(mpris_error)?;

        // The player can exit between discovery and installing these streams. Rechecking after
        // every subscription is live closes that gap; a later exit is then delivered normally.
        if !self.bus_name_is_owned(service.as_str()).await? {
            return Err(PlaybackError::Disconnected);
        }

        tokio::select! {
            change = property_changes.next() => {
                change.ok_or(PlaybackError::Disconnected)?;
            }
            change = seeked.next() => {
                change.ok_or(PlaybackError::Disconnected)?;
            }
            owner = owner_changes.next() => {
                match owner {
                    Some(Some(_)) => {}
                    Some(None) | None => return Err(PlaybackError::Disconnected),
                }
            }
        }

        self.snapshot().await
    }

    async fn execute(&self, command: PlaybackCommand) -> Result<(), PlaybackError> {
        let player = self.player().await?;

        match command {
            PlaybackCommand::Toggle => player.play_pause().await.map_err(mpris_error),
            PlaybackCommand::Previous => player.previous().await.map_err(mpris_error),
            PlaybackCommand::Next => player.next().await.map_err(mpris_error),
            PlaybackCommand::SeekBy(offset) => player.seek(offset).await.map_err(mpris_error),
            PlaybackCommand::SetVolume(volume) => player
                .set_volume(volume.clamp(0.0, 1.0))
                .await
                .map_err(mpris_error),
            PlaybackCommand::OpenUri(uri) => player.open_uri(&uri).await.map_err(mpris_error),
        }
    }
}

#[cfg(target_os = "linux")]
#[zbus::proxy(
    default_service = "org.mpris.MediaPlayer2.spotifyd",
    default_path = "/org/mpris/MediaPlayer2",
    interface = "org.mpris.MediaPlayer2.Player"
)]
trait MprisPlayer {
    fn play_pause(&self) -> zbus::Result<()>;

    fn previous(&self) -> zbus::Result<()>;

    fn next(&self) -> zbus::Result<()>;

    fn seek(&self, offset: i64) -> zbus::Result<()>;

    fn open_uri(&self, uri: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn seeked(&self, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;

    #[zbus(property)]
    fn volume(&self) -> zbus::Result<f64>;

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::Result<()>;
}

#[cfg(target_os = "linux")]
#[zbus::proxy(
    default_service = "rs.spotifyd",
    default_path = "/rs/spotifyd/Controls",
    interface = "rs.spotifyd.Controls"
)]
trait SpotifydControls {
    fn transfer_playback(&self) -> zbus::Result<()>;
}

#[cfg(target_os = "linux")]
fn mpris_error(error: impl fmt::Display) -> PlaybackError {
    PlaybackError::Mpris(error.to_string())
}

#[cfg(target_os = "linux")]
fn control_error(error: impl fmt::Display) -> PlaybackError {
    PlaybackError::Control(error.to_string())
}

#[cfg(target_os = "linux")]
fn normalize_metadata(mut metadata: HashMap<String, OwnedValue>) -> TrackMetadata {
    TrackMetadata {
        track_id: take_value::<OwnedObjectPath>(&mut metadata, "mpris:trackid")
            .map(|path| path.to_string()),
        title: take_value(&mut metadata, "xesam:title"),
        artists: take_value(&mut metadata, "xesam:artist").unwrap_or_default(),
        album: take_value(&mut metadata, "xesam:album"),
        duration: take_value::<i64>(&mut metadata, "mpris:length").map(microseconds_to_duration),
        art_url: take_value(&mut metadata, "mpris:artUrl"),
    }
}

#[cfg(target_os = "linux")]
fn take_value<T>(metadata: &mut HashMap<String, OwnedValue>, key: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    metadata.remove(key)?.try_into().ok()
}

#[cfg(any(target_os = "linux", test))]
fn microseconds_to_duration(microseconds: i64) -> Duration {
    Duration::from_micros(u64::try_from(microseconds).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_status_is_normalized() {
        assert_eq!(
            PlaybackStatus::from("Playing".to_owned()),
            PlaybackStatus::Playing
        );
        assert_eq!(
            PlaybackStatus::from("Paused".to_owned()),
            PlaybackStatus::Paused
        );
        assert_eq!(
            PlaybackStatus::from("Stopped".to_owned()),
            PlaybackStatus::Stopped
        );
        assert_eq!(
            PlaybackStatus::from("Buffering".to_owned()),
            PlaybackStatus::Unknown("Buffering".to_owned())
        );
    }

    #[test]
    fn negative_mpris_positions_are_clamped_to_zero() {
        assert_eq!(microseconds_to_duration(-1), Duration::ZERO);
        assert_eq!(
            microseconds_to_duration(1_500_000),
            Duration::from_millis(1_500)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recognizes_spotifyd_unique_mpris_bus_name() {
        assert!(is_spotifyd_bus_name(
            "org.mpris.MediaPlayer2.spotifyd.instance215550"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recognizes_legacy_spotifyd_mpris_bus_name() {
        assert!(is_spotifyd_bus_name("org.mpris.MediaPlayer2.spotifyd"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recognizes_spotifyd_unique_control_bus_name() {
        assert!(is_spotifyd_control_bus_name("rs.spotifyd.instance215550"));
        assert!(!is_spotifyd_control_bus_name("rs.spotifyd.instanceother"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_unrelated_mpris_bus_names() {
        assert!(!is_spotifyd_bus_name("org.mpris.MediaPlayer2.spotify"));
        assert!(!is_spotifyd_bus_name(
            "org.mpris.MediaPlayer2.spotifyd.instance"
        ));
        assert!(!is_spotifyd_bus_name(
            "org.mpris.MediaPlayer2.spotifyd.instanceother"
        ));
    }
}
