use std::time::{Duration, Instant};

use crate::{
    artwork::Artwork,
    browser::BrowserState,
    playback::{PlaybackSnapshot, PlaybackStatus, TrackMetadata},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Authenticate,
    RetryConnection,
    TogglePlayback,
    PreviousTrack,
    NextTrack,
    Seek {
        direction: SeekDirection,
        amount: Duration,
    },
    AdjustVolume(f64),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekDirection {
    Backward,
    Forward,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    ConnectionPending,
    PlaybackUpdated {
        snapshot: PlaybackSnapshot,
        observed_at: Instant,
    },
    PlaybackDisconnected,
    PlaybackFailed(String),
    ArtworkLoaded {
        track_revision: u64,
        artwork: Artwork,
    },
    ArtworkFailed {
        track_revision: u64,
        message: String,
    },
    QuitRequested,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArtworkState {
    Unavailable,
    Loading,
    Ready(Artwork),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackState {
    status: PlaybackStatus,
    track: TrackMetadata,
    authoritative_position: Duration,
    observed_at: Instant,
    volume: f64,
}

impl PlaybackState {
    pub const fn status(&self) -> &PlaybackStatus {
        &self.status
    }

    pub const fn track(&self) -> &TrackMetadata {
        &self.track
    }

    pub const fn volume(&self) -> f64 {
        self.volume
    }

    pub fn position_at(&self, now: Instant) -> Duration {
        let elapsed = match self.status {
            PlaybackStatus::Playing => now.saturating_duration_since(self.observed_at),
            PlaybackStatus::Paused | PlaybackStatus::Stopped | PlaybackStatus::Unknown(_) => {
                Duration::ZERO
            }
        };
        let estimated = self.authoritative_position.saturating_add(elapsed);

        self.track
            .duration
            .map_or(estimated, |duration| estimated.min(duration))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    connection: ConnectionState,
    playback: Option<PlaybackState>,
    artwork: ArtworkState,
    browser: BrowserState,
    track_revision: u64,
    should_quit: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Connecting,
            playback: None,
            artwork: ArtworkState::Unavailable,
            browser: BrowserState::default(),
            track_revision: 0,
            should_quit: false,
        }
    }
}

impl AppState {
    pub const fn connection(&self) -> &ConnectionState {
        &self.connection
    }

    pub const fn playback(&self) -> Option<&PlaybackState> {
        self.playback.as_ref()
    }

    pub const fn artwork(&self) -> &ArtworkState {
        &self.artwork
    }

    pub const fn browser(&self) -> &BrowserState {
        &self.browser
    }

    pub const fn browser_mut(&mut self) -> &mut BrowserState {
        &mut self.browser
    }

    pub const fn track_revision(&self) -> u64 {
        self.track_revision
    }

    pub const fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn reduce(&mut self, event: AppEvent) {
        match event {
            AppEvent::ConnectionPending => {
                self.connection = ConnectionState::Connecting;
                self.playback = None;
                self.artwork = ArtworkState::Unavailable;
            }
            AppEvent::PlaybackUpdated {
                snapshot,
                observed_at,
            } => self.apply_snapshot(snapshot, observed_at),
            AppEvent::PlaybackDisconnected => {
                self.connection = ConnectionState::Disconnected;
                self.playback = None;
                self.artwork = ArtworkState::Unavailable;
            }
            AppEvent::PlaybackFailed(message) => {
                self.connection = ConnectionState::Error(message);
                self.playback = None;
                self.artwork = ArtworkState::Unavailable;
            }
            AppEvent::ArtworkLoaded {
                track_revision,
                artwork,
            } if track_revision == self.track_revision => {
                self.artwork = ArtworkState::Ready(artwork);
            }
            AppEvent::ArtworkFailed {
                track_revision,
                message,
            } if track_revision == self.track_revision => {
                self.artwork = ArtworkState::Failed(message);
            }
            AppEvent::ArtworkLoaded { .. } | AppEvent::ArtworkFailed { .. } => {}
            AppEvent::QuitRequested => self.should_quit = true,
        }
    }

    fn apply_snapshot(&mut self, snapshot: PlaybackSnapshot, observed_at: Instant) {
        let track_changed = self
            .playback
            .as_ref()
            .is_none_or(|playback| playback.track != snapshot.track);

        if track_changed {
            self.track_revision = self.track_revision.saturating_add(1);
            self.artwork = if snapshot.track.art_url.is_some() {
                ArtworkState::Loading
            } else {
                ArtworkState::Unavailable
            };
        }

        self.connection = ConnectionState::Connected;
        self.playback = Some(PlaybackState {
            status: snapshot.status,
            track: snapshot.track,
            authoritative_position: snapshot.position,
            observed_at,
            volume: snapshot.volume,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;

    fn snapshot(track_id: &str, status: PlaybackStatus, position: Duration) -> PlaybackSnapshot {
        PlaybackSnapshot {
            status,
            track: TrackMetadata {
                track_id: Some(track_id.to_owned()),
                title: Some(format!("Track {track_id}")),
                artists: vec!["Artist".to_owned()],
                album: Some("Album".to_owned()),
                duration: Some(Duration::from_secs(240)),
                art_url: Some(format!("https://example.com/{track_id}.jpg")),
            },
            position,
            volume: 0.75,
        }
    }

    #[test]
    fn playback_update_connects_and_populates_state() {
        let observed_at = Instant::now();
        let mut state = AppState::default();

        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(12)),
            observed_at,
        });

        assert_eq!(state.connection(), &ConnectionState::Connected);
        let playback = state.playback().expect("playback should be present");
        assert_eq!(playback.status(), &PlaybackStatus::Playing);
        assert_eq!(playback.track().track_id.as_deref(), Some("one"));
        assert_eq!(playback.volume(), 0.75);
        assert_eq!(state.track_revision(), 1);
    }

    #[test]
    fn track_change_replaces_metadata_and_advances_revision() {
        let observed_at = Instant::now();
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(30)),
            observed_at,
        });

        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("two", PlaybackStatus::Playing, Duration::from_secs(2)),
            observed_at: observed_at + Duration::from_secs(5),
        });

        let playback = state.playback().expect("playback should be present");
        assert_eq!(playback.track().track_id.as_deref(), Some("two"));
        assert_eq!(
            playback.position_at(observed_at + Duration::from_secs(5)),
            Duration::from_secs(2)
        );
        assert_eq!(state.track_revision(), 2);
        assert_eq!(state.artwork(), &ArtworkState::Loading);
    }

    #[test]
    fn stale_artwork_is_ignored_after_a_track_change() {
        let observed_at = Instant::now();
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::ZERO),
            observed_at,
        });
        let stale_revision = state.track_revision();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("two", PlaybackStatus::Playing, Duration::ZERO),
            observed_at,
        });

        state.reduce(AppEvent::ArtworkLoaded {
            track_revision: stale_revision,
            artwork: Artwork::new(
                "https://example.com/one.jpg".to_owned(),
                DynamicImage::new_rgb8(4, 4),
            ),
        });

        assert_eq!(state.artwork(), &ArtworkState::Loading);
    }

    #[test]
    fn current_artwork_is_published_to_the_view_state() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::ZERO),
            observed_at: Instant::now(),
        });
        let artwork = Artwork::new(
            "https://example.com/one.jpg".to_owned(),
            DynamicImage::new_rgb8(4, 4),
        );

        state.reduce(AppEvent::ArtworkLoaded {
            track_revision: state.track_revision(),
            artwork: artwork.clone(),
        });

        assert_eq!(state.artwork(), &ArtworkState::Ready(artwork));
    }

    #[test]
    fn same_track_update_does_not_advance_revision() {
        let observed_at = Instant::now();
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(30)),
            observed_at,
        });
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(45)),
            observed_at: observed_at + Duration::from_secs(15),
        });

        assert_eq!(state.track_revision(), 1);
    }

    #[test]
    fn playing_progress_is_interpolated_and_capped_at_duration() {
        let observed_at = Instant::now();
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(230)),
            observed_at,
        });
        let playback = state.playback().expect("playback should be present");

        assert_eq!(
            playback.position_at(observed_at + Duration::from_secs(5)),
            Duration::from_secs(235)
        );
        assert_eq!(
            playback.position_at(observed_at + Duration::from_secs(20)),
            Duration::from_secs(240)
        );
    }

    #[test]
    fn paused_progress_does_not_advance() {
        let observed_at = Instant::now();
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Paused, Duration::from_secs(90)),
            observed_at,
        });

        assert_eq!(
            state
                .playback()
                .expect("playback should be present")
                .position_at(observed_at + Duration::from_secs(30)),
            Duration::from_secs(90)
        );
    }

    #[test]
    fn disconnect_clears_stale_playback() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(30)),
            observed_at: Instant::now(),
        });

        state.reduce(AppEvent::PlaybackDisconnected);

        assert_eq!(state.connection(), &ConnectionState::Disconnected);
        assert!(state.playback().is_none());
    }

    #[test]
    fn snapshot_after_disconnect_reconnects() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackDisconnected);

        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(8)),
            observed_at: Instant::now(),
        });

        assert_eq!(state.connection(), &ConnectionState::Connected);
        assert!(state.playback().is_some());
    }

    #[test]
    fn playback_error_clears_stale_playback() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: snapshot("one", PlaybackStatus::Playing, Duration::from_secs(30)),
            observed_at: Instant::now(),
        });

        state.reduce(AppEvent::PlaybackFailed("permission denied".to_owned()));

        assert_eq!(
            state.connection(),
            &ConnectionState::Error("permission denied".to_owned())
        );
        assert!(state.playback().is_none());
    }

    #[test]
    fn quit_event_requests_shutdown() {
        let mut state = AppState::default();
        state.reduce(AppEvent::QuitRequested);
        assert!(state.should_quit());
    }
}
