use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
};

use crate::{
    app::{AppState, ArtworkState, ConnectionState},
    artwork::ArtworkRenderer,
    config::Theme,
};

pub fn render(
    frame: &mut Frame,
    state: &AppState,
    theme: &Theme,
    artwork_renderer: &mut ArtworkRenderer,
) {
    let area = frame.area();
    let block = Block::new()
        .borders(Borders::ALL)
        .style(
            Style::default()
                .bg(theme.background())
                .fg(theme.foreground()),
        )
        .border_style(Style::default().fg(theme.border()))
        .title(
            Line::from(" Spotify TUI ").style(
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ),
        );
    let inner = block.inner(area);

    frame.render_widget(block, area);

    if let Some(playback) = state.playback()
        && matches!(state.connection(), ConnectionState::Connected)
    {
        render_playback(frame, inner, state, playback, theme, artwork_renderer);
    } else {
        render_connection_state(frame, inner, state, theme);
    }
}

fn render_connection_state(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    theme: &Theme,
) {
    let content_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .margin(1)
        .split(area)[1];

    let text = Text::from(vec![
        Line::from(status_text(state)).style(
            Style::default()
                .fg(status_color(state, theme))
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(help_text(state)).style(Style::default().fg(theme.muted())),
    ]);

    frame.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        content_area,
    );
}

fn render_playback(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &AppState,
    playback: &crate::app::PlaybackState,
    theme: &Theme,
    artwork_renderer: &mut ArtworkRenderer,
) {
    let compact = area.height < 16 || area.width < 60;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if compact {
            [
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(1),
            ]
        } else {
            [
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(0),
                Constraint::Length(1),
            ]
        })
        .margin(1)
        .split(area);

    if compact {
        render_metadata(frame, layout[0], playback, theme, true);
    } else {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(38),
                Constraint::Length(2),
                Constraint::Fill(1),
            ])
            .split(layout[0]);
        render_artwork(frame, columns[0], state.artwork(), theme, artwork_renderer);
        let metadata_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(5),
                Constraint::Fill(1),
            ])
            .split(columns[2])[1];
        render_metadata(frame, metadata_area, playback, theme, false);
    }

    let volume = if playback.volume().is_finite() {
        playback.volume().clamp(0.0, 1.0)
    } else {
        0.0
    };
    let status = format!(
        "{}  •  Volume {:.0}%",
        playback.status().to_string().to_ascii_uppercase(),
        volume * 100.0
    );
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::default().fg(status_color_for_playback(playback, theme)))
            .alignment(Alignment::Center),
        layout[1],
    );

    let position = playback.position_at(std::time::Instant::now());
    let duration = playback.track().duration;
    let ratio = duration.map_or(0.0, |duration| {
        if duration.is_zero() {
            0.0
        } else {
            position.as_secs_f64() / duration.as_secs_f64()
        }
    });
    let label = duration.map_or_else(
        || format_duration(position),
        |duration| {
            format!(
                "{} / {}",
                format_duration(position),
                format_duration(duration)
            )
        },
    );
    let gauge = Gauge::default()
        .gauge_style(
            Style::default()
                .fg(theme.accent())
                .bg(theme.border())
                .add_modifier(Modifier::BOLD),
        )
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label);
    frame.render_widget(
        if compact {
            gauge
        } else {
            gauge.block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border())),
            )
        },
        layout[2],
    );

    let help = if area.width >= 76 {
        "Space play/pause · p/n track · h/l or ←/→ seek · j/k or ↓/↑ volume · q quit"
    } else if area.width >= 48 {
        "Space play/pause · p/n track · h/l seek · j/k volume · q quit"
    } else {
        "Space play · h/l seek · j/k vol · q"
    };
    frame.render_widget(
        Paragraph::new(help)
            .style(Style::default().fg(theme.muted()))
            .alignment(Alignment::Center),
        layout[4],
    );
}

fn render_metadata(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    playback: &crate::app::PlaybackState,
    theme: &Theme,
    compact: bool,
) {
    let title = playback.track().title.as_deref().unwrap_or("Unknown track");
    let artists = if playback.track().artists.is_empty() {
        "Unknown artist".to_owned()
    } else {
        playback.track().artists.join(", ")
    };
    let album = playback.track().album.as_deref().unwrap_or("Unknown album");
    let mut metadata = vec![
        Line::from(title).style(
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(artists).style(Style::default().fg(theme.foreground())),
    ];
    if !compact {
        metadata.push(Line::from(album).style(Style::default().fg(theme.muted())));
    }
    frame.render_widget(
        Paragraph::new(metadata)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_artwork(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    artwork: &ArtworkState,
    theme: &Theme,
    renderer: &mut ArtworkRenderer,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border()))
        .title(Line::from(" Artwork ").style(Style::default().fg(theme.muted())));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label = match artwork {
        ArtworkState::Loading => "Loading artwork…",
        ArtworkState::Ready(_) => "Rendering artwork…",
        ArtworkState::Unavailable => "No artwork",
        ArtworkState::Failed(_) => "Artwork unavailable",
    };
    let label_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .split(inner)[1];
    frame.render_widget(
        Paragraph::new(label)
            .style(Style::default().fg(theme.muted()))
            .alignment(Alignment::Center),
        label_area,
    );

    if matches!(artwork, ArtworkState::Ready(_)) {
        renderer.render(frame, inner);
    }
}

fn status_color_for_playback(playback: &crate::app::PlaybackState, theme: &Theme) -> Color {
    match playback.status() {
        crate::playback::PlaybackStatus::Playing => theme.accent(),
        crate::playback::PlaybackStatus::Paused
        | crate::playback::PlaybackStatus::Stopped
        | crate::playback::PlaybackStatus::Unknown(_) => theme.muted(),
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3_600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn status_color(state: &AppState, theme: &Theme) -> Color {
    match state.connection() {
        ConnectionState::Connecting => theme.warning(),
        ConnectionState::Connected => theme.accent(),
        ConnectionState::Disconnected => theme.muted(),
        ConnectionState::Error(_) => theme.error(),
    }
}

fn help_text(state: &AppState) -> &'static str {
    match state.connection() {
        ConnectionState::Connected => "Space play/pause / q quit",
        ConnectionState::Connecting | ConnectionState::Disconnected | ConnectionState::Error(_) => {
            "a auth / r retry / q quit"
        }
    }
}

fn status_text(state: &AppState) -> String {
    match state.connection() {
        ConnectionState::Connecting => "Connecting to spotifyd…".to_owned(),
        ConnectionState::Disconnected => "spotifyd is disconnected".to_owned(),
        ConnectionState::Error(message) => format!("Playback error: {message}"),
        ConnectionState::Connected => state.playback().map_or_else(
            || "Connected to spotifyd".to_owned(),
            |playback| {
                playback
                    .track()
                    .title
                    .clone()
                    .unwrap_or_else(|| "Unknown track".to_owned())
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::{Terminal, backend::TestBackend, style::Color};

    use super::*;
    use crate::{
        app::AppEvent,
        config::Config,
        playback::{PlaybackSnapshot, PlaybackStatus, TrackMetadata},
    };

    fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = buffer.area;
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .filter_map(|x| buffer.cell((x, y)))
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn resolved_theme_is_applied_to_the_rendered_cells() {
        let config = Config::from_toml(
            r##"
version = 1
theme = "test"

[themes.test]
border = "#102030"
accent = "#405060"
background = "#010203"
"##,
        )
        .expect("test theme should resolve");
        let backend = TestBackend::new(40, 12);
        let mut terminal = Terminal::new(backend).expect("test backend is infallible");
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(config.theme()).expect("renderer should start");

        terminal
            .draw(|frame| {
                render(
                    frame,
                    &AppState::default(),
                    config.theme(),
                    &mut artwork_renderer,
                );
            })
            .expect("test backend is infallible");

        let buffer = terminal.backend().buffer();
        let top_left = buffer.cell((0, 0)).expect("cell should exist");
        let title = buffer.cell((2, 0)).expect("cell should exist");
        let body = buffer.cell((1, 1)).expect("cell should exist");
        assert_eq!(top_left.fg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(title.fg, Color::Rgb(0x40, 0x50, 0x60));
        assert_eq!(body.bg, Color::Rgb(0x01, 0x02, 0x03));
    }

    #[test]
    fn connected_view_renders_live_metadata_progress_volume_and_controls() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: PlaybackSnapshot {
                status: PlaybackStatus::Paused,
                track: TrackMetadata {
                    track_id: Some("spotify:track:one".to_owned()),
                    title: Some("Live track".to_owned()),
                    artists: vec!["Artist".to_owned()],
                    album: Some("Album".to_owned()),
                    duration: Some(Duration::from_secs(180)),
                    art_url: None,
                },
                position: Duration::from_secs(12),
                volume: 0.75,
            },
            observed_at: Instant::now(),
        });
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        for expected in [
            "Live track",
            "Artist",
            "Album",
            "0:12 / 3:00",
            "75%",
            "play/pause",
            "No artwork",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }

    #[test]
    fn artwork_loading_state_is_visible_in_the_normal_layout() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: PlaybackSnapshot {
                status: PlaybackStatus::Playing,
                track: TrackMetadata {
                    track_id: Some("spotify:track:one".to_owned()),
                    title: Some("Live track".to_owned()),
                    artists: vec!["Artist".to_owned()],
                    album: Some("Album".to_owned()),
                    duration: Some(Duration::from_secs(180)),
                    art_url: Some("https://example.com/art.jpg".to_owned()),
                },
                position: Duration::ZERO,
                volume: 0.75,
            },
            observed_at: Instant::now(),
        });
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        assert!(rendered_text(&terminal).contains("Loading artwork…"));
    }

    #[test]
    fn narrow_layout_prioritizes_playback_over_the_artwork_panel() {
        let mut state = AppState::default();
        state.reduce(AppEvent::PlaybackUpdated {
            snapshot: PlaybackSnapshot {
                status: PlaybackStatus::Paused,
                track: TrackMetadata {
                    track_id: Some("spotify:track:one".to_owned()),
                    title: Some("Live track".to_owned()),
                    artists: vec!["Artist".to_owned()],
                    album: Some("Album".to_owned()),
                    duration: Some(Duration::from_secs(180)),
                    art_url: Some("https://example.com/art.jpg".to_owned()),
                },
                position: Duration::from_secs(12),
                volume: 0.75,
            },
            observed_at: Instant::now(),
        });
        let mut terminal =
            Terminal::new(TestBackend::new(50, 12)).expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        assert!(text.contains("Live track"));
        assert!(!text.contains("Artwork"));
    }
}
