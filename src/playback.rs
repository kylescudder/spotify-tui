use std::{fmt, future::Future, time::Duration};

#[cfg(target_os = "linux")]
use std::collections::HashMap;
use thiserror::Error;
#[cfg(target_os = "linux")]
use zbus::{
    Connection,
    names::BusName,
    zvariant::{OwnedObjectPath, OwnedValue},
};

#[cfg(target_os = "linux")]
const SPOTIFYD_BUS_NAME: &str = "org.mpris.MediaPlayer2.spotifyd";

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

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlaybackError {
    #[error("spotifyd is disconnected")]
    Disconnected,
    #[error("could not connect to the session bus: {0}")]
    SessionBus(String),
    #[error("MPRIS request failed: {0}")]
    Mpris(String),
    #[error("the local playback adapter is not implemented for {0}")]
    UnsupportedPlatform(&'static str),
}

pub trait PlaybackSource {
    fn snapshot(&self) -> impl Future<Output = Result<PlaybackSnapshot, PlaybackError>> + Send;
}

#[cfg(target_os = "linux")]
pub struct MprisPlaybackSource {
    connection: Connection,
}

#[cfg(not(target_os = "linux"))]
pub struct MprisPlaybackSource;

#[cfg(target_os = "linux")]
impl MprisPlaybackSource {
    pub async fn connect() -> Result<Self, PlaybackError> {
        let connection = Connection::session()
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;

        Ok(Self { connection })
    }

    async fn spotifyd_is_connected(&self) -> Result<bool, PlaybackError> {
        let dbus = zbus::fdo::DBusProxy::new(&self.connection)
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))?;
        let name = BusName::try_from(SPOTIFYD_BUS_NAME)
            .map_err(|error| PlaybackError::Mpris(error.to_string()))?;

        dbus.name_has_owner(name)
            .await
            .map_err(|error| PlaybackError::SessionBus(error.to_string()))
    }
}

#[cfg(not(target_os = "linux"))]
impl MprisPlaybackSource {
    pub async fn connect() -> Result<Self, PlaybackError> {
        Err(PlaybackError::UnsupportedPlatform(std::env::consts::OS))
    }
}

#[cfg(target_os = "linux")]
impl PlaybackSource for MprisPlaybackSource {
    async fn snapshot(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        if !self.spotifyd_is_connected().await? {
            return Err(PlaybackError::Disconnected);
        }

        let player = MprisPlayerProxy::new(&self.connection)
            .await
            .map_err(|error| PlaybackError::Mpris(error.to_string()))?;

        let (status, metadata, position, volume) = tokio::try_join!(
            player.playback_status(),
            player.metadata(),
            player.position(),
            player.volume(),
        )
        .map_err(|error| PlaybackError::Mpris(error.to_string()))?;

        Ok(PlaybackSnapshot {
            status: status.into(),
            track: normalize_metadata(metadata),
            position: microseconds_to_duration(position),
            volume,
        })
    }
}

#[cfg(not(target_os = "linux"))]
impl PlaybackSource for MprisPlaybackSource {
    async fn snapshot(&self) -> Result<PlaybackSnapshot, PlaybackError> {
        Err(PlaybackError::UnsupportedPlatform(std::env::consts::OS))
    }
}

#[cfg(target_os = "linux")]
#[zbus::proxy(
    default_service = "org.mpris.MediaPlayer2.spotifyd",
    default_path = "/org/mpris/MediaPlayer2",
    interface = "org.mpris.MediaPlayer2.Player"
)]
trait MprisPlayer {
    #[zbus(property)]
    fn playback_status(&self) -> zbus::Result<String>;

    #[zbus(property)]
    fn metadata(&self) -> zbus::Result<HashMap<String, OwnedValue>>;

    #[zbus(property)]
    fn position(&self) -> zbus::Result<i64>;

    #[zbus(property)]
    fn volume(&self) -> zbus::Result<f64>;
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
}
