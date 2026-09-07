use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::{AppState, ArtworkState, ConnectionState},
    artwork::ArtworkRenderer,
    browser::BrowserView,
    catalog::CatalogItemKind,
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

    if !matches!(state.browser().view(), BrowserView::Closed) {
        render_browser(frame, inner, state.browser().view(), theme);
        return;
    }

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
    let (status_area, progress_area, help_area) = if compact {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(1),
            ])
            .margin(1)
            .split(area);
        render_metadata(frame, layout[1], playback, theme, true);
        (layout[2], layout[3], layout[5])
    } else {
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(0),
                Constraint::Length(1),
            ])
            .margin(1)
            .split(area);
        let stage = now_playing_stage(layout[0]);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(stage.artwork_width),
                Constraint::Length(2),
                Constraint::Fill(1),
            ])
            .split(stage.area);
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
        (layout[1], layout[2], layout[4])
    };

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
        status_area,
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
        progress_area,
    );

    let help = if area.width >= 90 {
        "/ search · Space play/pause · p/n track · h/l seek · j/k volume · q quit"
    } else if area.width >= 64 {
        "/ search · Space play/pause · p/n track · h/l seek · q quit"
    } else if area.width >= 48 {
        "/ search · Space play · p/n track · q quit"
    } else {
        "/ search · Space play · q"
    };
    frame.render_widget(
        Paragraph::new(help)
            .style(Style::default().fg(theme.muted()))
            .alignment(Alignment::Center),
        help_area,
    );
}

fn render_browser(frame: &mut Frame, area: Rect, view: &BrowserView, theme: &Theme) {
    match view {
        BrowserView::Closed => {}
        BrowserView::Editing { query } => render_search_editor(frame, area, query, theme),
        BrowserView::Loading { label } => {
            render_browser_message(
                frame,
                area,
                label,
                theme.warning(),
                "Esc back · q quit",
                theme,
            );
        }
        BrowserView::Error { message } => {
            render_browser_message(
                frame,
                area,
                message,
                theme.error(),
                "Esc back · / new search · q quit",
                theme,
            );
        }
        BrowserView::Page { page, selected } => {
            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Fill(1),
                    Constraint::Length(1),
                ])
                .margin(1)
                .split(area);
            let heading = Text::from(vec![
                Line::from(page.title()).style(
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                ),
                Line::from(page.subtitle()).style(Style::default().fg(theme.muted())),
            ]);
            frame.render_widget(Paragraph::new(heading).wrap(Wrap { trim: true }), layout[0]);

            if page.items().is_empty() {
                frame.render_widget(
                    Paragraph::new("No results")
                        .style(Style::default().fg(theme.muted()))
                        .alignment(Alignment::Center),
                    layout[1],
                );
            } else {
                let items = page.items().iter().map(|item| {
                    let tag = match item.kind() {
                        CatalogItemKind::Artist => "[Artist]",
                        CatalogItemKind::Album => "[Album]",
                        CatalogItemKind::Track => "[Track]",
                        CatalogItemKind::Playlist => "[Playlist]",
                    };
                    let mut spans = vec![
                        Span::styled(format!("{tag:<11}"), Style::default().fg(theme.muted())),
                        Span::styled(
                            item.name().to_owned(),
                            Style::default().fg(theme.foreground()),
                        ),
                    ];
                    if !item.detail().is_empty() {
                        spans.push(Span::styled(
                            format!(" — {}", item.detail()),
                            Style::default().fg(theme.muted()),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                });
                let list = List::new(items).highlight_symbol("› ").highlight_style(
                    Style::default()
                        .fg(theme.accent())
                        .bg(theme.border())
                        .add_modifier(Modifier::BOLD),
                );
                let mut list_state = ListState::default().with_selected(Some(*selected));
                frame.render_stateful_widget(list, layout[1], &mut list_state);
            }

            let help = if area.width >= 70 {
                "j/k or ↑/↓ navigate · Enter open/play · / search · Esc back · q quit"
            } else {
                "j/k navigate · Enter open · / search · Esc back"
            };
            frame.render_widget(
                Paragraph::new(help)
                    .style(Style::default().fg(theme.muted()))
                    .alignment(Alignment::Center),
                layout[2],
            );
        }
    }
}

fn render_search_editor(frame: &mut Frame, area: Rect, query: &str, theme: &Theme) {
    let centered = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(5),
            Constraint::Fill(1),
        ])
        .margin(1)
        .split(area)[1];
    let input = Paragraph::new(format!("> {query}█"))
        .style(Style::default().fg(theme.foreground()))
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.accent()))
                .title(" Search Spotify "),
        );
    frame.render_widget(input, centered);
    let help_area = Rect::new(centered.x, centered.y.saturating_add(3), centered.width, 1);
    frame.render_widget(
        Paragraph::new("Enter search · Esc close")
            .style(Style::default().fg(theme.muted()))
            .alignment(Alignment::Center),
        help_area,
    );
}

fn render_browser_message(
    frame: &mut Frame,
    area: Rect,
    message: &str,
    color: Color,
    help: &str,
    theme: &Theme,
) {
    let centered = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(3),
            Constraint::Fill(1),
        ])
        .margin(1)
        .split(area)[1];
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(message).style(Style::default().fg(color).add_modifier(Modifier::BOLD)),
            Line::from(help).style(Style::default().fg(theme.muted())),
        ]))
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        centered,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NowPlayingStage {
    area: Rect,
    artwork_width: u16,
}

fn now_playing_stage(area: Rect) -> NowPlayingStage {
    const MAX_STAGE_WIDTH: u16 = 112;
    const MAX_ARTWORK_HEIGHT: u16 = 24;
    const MIN_METADATA_WIDTH: u16 = 24;
    const COLUMN_GAP: u16 = 2;

    let stage_width = area.width.min(MAX_STAGE_WIDTH);
    let available_artwork_width = stage_width
        .saturating_sub(COLUMN_GAP)
        .saturating_sub(MIN_METADATA_WIDTH);
    let artwork_height = area
        .height
        .min(MAX_ARTWORK_HEIGHT)
        .min(available_artwork_width / 2);
    let artwork_width = artwork_height.saturating_mul(2);

    NowPlayingStage {
        area: Rect::new(
            area.x + area.width.saturating_sub(stage_width) / 2,
            area.y + area.height.saturating_sub(artwork_height) / 2,
            stage_width,
            artwork_height,
        ),
        artwork_width,
    }
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
    match artwork {
        ArtworkState::Ready(_) => renderer.render(frame, area),
        ArtworkState::Loading | ArtworkState::Unavailable | ArtworkState::Failed(_) => {
            let label = match artwork {
                ArtworkState::Loading => "Loading artwork…",
                ArtworkState::Unavailable => "No artwork",
                ArtworkState::Failed(_) => "Artwork unavailable",
                ArtworkState::Ready(_) => unreachable!("handled above"),
            };
            let label_area = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(1),
                    Constraint::Fill(1),
                ])
                .split(area)[1];
            frame.render_widget(
                Paragraph::new(label)
                    .style(Style::default().fg(theme.muted()))
                    .alignment(Alignment::Center),
                label_area,
            );
        }
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

    use image::DynamicImage;
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    use super::*;
    use crate::{
        app::{AppEvent, ArtworkState},
        artwork::Artwork,
        browser::BrowserCommand,
        catalog::{CatalogEvent, CatalogItem, CatalogItemKind, CatalogPage},
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

    #[test]
    fn tall_compact_layout_centers_the_playback_details() {
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
        const TERMINAL_HEIGHT: u16 = 34;
        let mut terminal = Terminal::new(TestBackend::new(58, TERMINAL_HEIGHT))
            .expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        let lines = text.lines().collect::<Vec<_>>();
        let title_row = lines
            .iter()
            .position(|line| line.contains("Live track"))
            .expect("track title should be visible");
        let progress_row = lines
            .iter()
            .position(|line| line.contains("0:12 / 3:00"))
            .expect("progress should be visible");
        let group_center = (title_row + progress_row) / 2;
        let viewport_center = usize::from(TERMINAL_HEIGHT / 2);

        assert!(
            group_center.abs_diff(viewport_center) <= 2,
            "compact playback group is centered at row {group_center}, expected row {viewport_center}"
        );
    }

    #[test]
    fn tall_layout_keeps_artwork_in_a_bounded_centered_stage() {
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
            Terminal::new(TestBackend::new(160, 70)).expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        let lines = text.lines().collect::<Vec<_>>();
        let loading_row = lines
            .iter()
            .position(|line| line.contains("Loading artwork…"))
            .expect("loading state should be visible");
        let viewport_center = 35;

        assert!(
            loading_row.abs_diff(viewport_center) <= 5,
            "artwork placeholder is centered at row {loading_row}, expected near row {viewport_center}"
        );
    }

    #[test]
    fn ready_artwork_does_not_leave_a_rendering_placeholder() {
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
        state.reduce(AppEvent::ArtworkLoaded {
            track_revision: state.track_revision(),
            artwork: Artwork::new(
                "https://example.com/art.jpg".to_owned(),
                DynamicImage::new_rgb8(8, 8),
            ),
        });
        assert!(matches!(state.artwork(), ArtworkState::Ready(_)));
        let mut terminal =
            Terminal::new(TestBackend::new(100, 30)).expect("test backend is infallible");
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        assert!(!rendered_text(&terminal).contains("Rendering artwork…"));
    }

    #[test]
    fn artwork_is_rendered_without_a_decorative_frame_or_title() {
        let artwork = ArtworkState::Ready(Artwork::new(
            "https://example.com/art.jpg".to_owned(),
            DynamicImage::new_rgb8(8, 8),
        ));
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");
        let mut terminal =
            Terminal::new(TestBackend::new(30, 15)).expect("test backend is infallible");

        terminal
            .draw(|frame| {
                render_artwork(
                    frame,
                    Rect::new(2, 2, 20, 10),
                    &artwork,
                    theme.theme(),
                    &mut artwork_renderer,
                );
            })
            .expect("test backend is infallible");

        let buffer = terminal.backend().buffer();
        assert_ne!(
            buffer
                .cell((2, 2))
                .expect("artwork corner should exist")
                .symbol(),
            "┌"
        );
        assert!(!rendered_text(&terminal).contains("Artwork"));
    }

    #[test]
    fn search_editor_replaces_now_playing_and_shows_the_query() {
        let mut state = AppState::default();
        state.browser_mut().apply(BrowserCommand::OpenSearch);
        for character in "enter shikari".chars() {
            state.browser_mut().apply(BrowserCommand::Insert(character));
        }
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");
        let mut terminal =
            Terminal::new(TestBackend::new(80, 24)).expect("test backend is infallible");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        assert!(text.contains("Search Spotify"));
        assert!(text.contains("> enter shikari█"));
        assert!(text.contains("Enter search"));
    }

    #[test]
    fn catalogue_page_renders_typed_navigable_results() {
        let mut state = AppState::default();
        state.browser_mut().apply(BrowserCommand::OpenSearch);
        state.browser_mut().apply(BrowserCommand::Insert('x'));
        state.browser_mut().apply(BrowserCommand::Submit);
        state.browser_mut().resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: CatalogPage::Search {
                query: "enter shikari".to_owned(),
                items: vec![
                    CatalogItem::new(
                        CatalogItemKind::Artist,
                        "artist",
                        "spotify:artist:artist",
                        "Enter Shikari",
                        "Artist",
                        None,
                    ),
                    CatalogItem::new(
                        CatalogItemKind::Track,
                        "track",
                        "spotify:track:track",
                        "Sorry You're Not a Winner",
                        "Enter Shikari • 3:48",
                        None,
                    ),
                ],
            },
        });
        let theme = Config::default();
        let mut artwork_renderer =
            ArtworkRenderer::halfblocks(theme.theme()).expect("renderer should start");
        let mut terminal =
            Terminal::new(TestBackend::new(100, 28)).expect("test backend is infallible");

        terminal
            .draw(|frame| render(frame, &state, theme.theme(), &mut artwork_renderer))
            .expect("test backend is infallible");

        let text = rendered_text(&terminal);
        for expected in [
            "Search — enter shikari",
            "[Artist]",
            "Enter Shikari",
            "[Track]",
            "Sorry You're Not a Winner",
            "Enter open/play",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }
}
