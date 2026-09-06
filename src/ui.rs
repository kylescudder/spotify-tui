use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
    app::{AppState, ConnectionState},
    config::Theme,
};

pub fn render(frame: &mut Frame, state: &AppState, theme: &Theme) {
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

    frame.render_widget(block, area);

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
        ConnectionState::Connected => "q / Esc / Ctrl-C to quit",
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
    use ratatui::{Terminal, backend::TestBackend, style::Color};

    use super::*;
    use crate::config::Config;

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

        terminal
            .draw(|frame| render(frame, &AppState::default(), config.theme()))
            .expect("test backend is infallible");

        let buffer = terminal.backend().buffer();
        let top_left = buffer.cell((0, 0)).expect("cell should exist");
        let title = buffer.cell((2, 0)).expect("cell should exist");
        let body = buffer.cell((1, 1)).expect("cell should exist");
        assert_eq!(top_left.fg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(title.fg, Color::Rgb(0x40, 0x50, 0x60));
        assert_eq!(body.bg, Color::Rgb(0x01, 0x02, 0x03));
    }
}
