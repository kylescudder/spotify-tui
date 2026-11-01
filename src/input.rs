use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use std::time::Duration;

use crate::app::{Command, SeekDirection};
use crate::browser::{BrowserCommand, BrowserMode};

pub fn browser_command_for_key(key: KeyEvent, mode: BrowserMode) -> Option<BrowserCommand> {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(BrowserCommand::Quit);
    }

    match mode {
        BrowserMode::Closed => match key.code {
            KeyCode::Char('/') if key.modifiers.is_empty() => Some(BrowserCommand::OpenSearch),
            KeyCode::Char('H') | KeyCode::Esc => Some(BrowserCommand::OpenHome),
            _ => None,
        },
        BrowserMode::Home => match key.code {
            KeyCode::Char('q') => Some(BrowserCommand::Quit),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => Some(BrowserCommand::Back),
            KeyCode::Char('/') => Some(BrowserCommand::OpenSearch),
            KeyCode::Char('R') => Some(BrowserCommand::RefreshHome),
            KeyCode::Up | KeyCode::Char('k') => Some(BrowserCommand::Previous),
            KeyCode::Down | KeyCode::Char('j') => Some(BrowserCommand::Next),
            KeyCode::Enter => Some(BrowserCommand::Activate),
            _ => None,
        },
        BrowserMode::Editing => match key.code {
            KeyCode::Esc => Some(BrowserCommand::Close),
            KeyCode::Enter => Some(BrowserCommand::Submit),
            KeyCode::Backspace => Some(BrowserCommand::Backspace),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                Some(BrowserCommand::Insert(character))
            }
            _ => None,
        },
        BrowserMode::Page => match key.code {
            KeyCode::Char('q') => Some(BrowserCommand::Quit),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => Some(BrowserCommand::Back),
            KeyCode::Char('/') => Some(BrowserCommand::OpenSearch),
            KeyCode::Char('H') => Some(BrowserCommand::OpenHome),
            KeyCode::Up | KeyCode::Char('k') => Some(BrowserCommand::Previous),
            KeyCode::Down | KeyCode::Char('j') => Some(BrowserCommand::Next),
            KeyCode::Enter => Some(BrowserCommand::Activate),
            _ => None,
        },
        BrowserMode::Waiting => match key.code {
            KeyCode::Char('q') => Some(BrowserCommand::Quit),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => Some(BrowserCommand::Back),
            KeyCode::Char('/') => Some(BrowserCommand::OpenSearch),
            KeyCode::Char('H') => Some(BrowserCommand::OpenHome),
            _ => None,
        },
    }
}

pub fn command_for_key(key: KeyEvent) -> Option<Command> {
    if key.code == KeyCode::Char('q')
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        return Some(Command::Quit);
    }

    if !key.modifiers.is_empty() {
        return None;
    }

    match key.code {
        KeyCode::Char('a') => Some(Command::Authenticate),
        KeyCode::Char('r') => Some(Command::RetryConnection),
        KeyCode::Char(' ') => Some(Command::TogglePlayback),
        KeyCode::Char('p') => Some(Command::PreviousTrack),
        KeyCode::Char('n') => Some(Command::NextTrack),
        KeyCode::Left | KeyCode::Char('h') => Some(Command::Seek {
            direction: SeekDirection::Backward,
            amount: Duration::from_secs(5),
        }),
        KeyCode::Right | KeyCode::Char('l') => Some(Command::Seek {
            direction: SeekDirection::Forward,
            amount: Duration::from_secs(5),
        }),
        KeyCode::Down | KeyCode::Char('j') => Some(Command::AdjustVolume(-0.05)),
        KeyCode::Up | KeyCode::Char('k') => Some(Command::AdjustVolume(0.05)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_keys_map_to_quit_command() {
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(command_for_key(key), Some(Command::Quit));
        }
    }

    #[test]
    fn home_opens_from_now_playing_and_every_catalogue_page() {
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let shifted_h = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);

        assert_eq!(
            browser_command_for_key(escape, BrowserMode::Closed),
            Some(BrowserCommand::OpenHome)
        );
        for mode in [BrowserMode::Closed, BrowserMode::Page, BrowserMode::Waiting] {
            assert_eq!(
                browser_command_for_key(shifted_h, mode),
                Some(BrowserCommand::OpenHome),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn searching_can_still_type_a_capital_h() {
        assert_eq!(
            browser_command_for_key(
                KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT),
                BrowserMode::Editing
            ),
            Some(BrowserCommand::Insert('H'))
        );
    }

    #[test]
    fn home_navigates_like_a_catalogue_page_and_refreshes() {
        for (code, modifiers, expected) in [
            (KeyCode::Char('j'), KeyModifiers::NONE, BrowserCommand::Next),
            (KeyCode::Down, KeyModifiers::NONE, BrowserCommand::Next),
            (
                KeyCode::Char('k'),
                KeyModifiers::NONE,
                BrowserCommand::Previous,
            ),
            (KeyCode::Up, KeyModifiers::NONE, BrowserCommand::Previous),
            (KeyCode::Enter, KeyModifiers::NONE, BrowserCommand::Activate),
            (KeyCode::Esc, KeyModifiers::NONE, BrowserCommand::Back),
            (
                KeyCode::Char('/'),
                KeyModifiers::NONE,
                BrowserCommand::OpenSearch,
            ),
            (
                KeyCode::Char('R'),
                KeyModifiers::SHIFT,
                BrowserCommand::RefreshHome,
            ),
            (KeyCode::Char('q'), KeyModifiers::NONE, BrowserCommand::Quit),
        ] {
            assert_eq!(
                browser_command_for_key(KeyEvent::new(code, modifiers), BrowserMode::Home),
                Some(expected)
            );
        }
    }

    #[test]
    fn home_leaves_transport_keys_to_the_player_but_not_seek_or_volume() {
        for key in [KeyCode::Char(' '), KeyCode::Char('n'), KeyCode::Char('p')] {
            let key = KeyEvent::new(key, KeyModifiers::NONE);

            assert_eq!(browser_command_for_key(key, BrowserMode::Home), None);
            assert!(command_for_key(key).is_some_and(|command| command.applies_on_home()));
        }
        for key in [KeyCode::Char('l'), KeyCode::Right] {
            let key = KeyEvent::new(key, KeyModifiers::NONE);

            assert_eq!(browser_command_for_key(key, BrowserMode::Home), None);
            assert!(!command_for_key(key).is_some_and(|command| command.applies_on_home()));
        }
    }

    #[test]
    fn unrelated_keys_do_not_produce_a_command() {
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn onboarding_keys_map_to_authenticate_and_retry() {
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some(Command::Authenticate)
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
            Some(Command::RetryConnection)
        );
    }

    #[test]
    fn playback_keys_map_to_transport_commands() {
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE)),
            Some(Command::TogglePlayback)
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)),
            Some(Command::PreviousTrack)
        );
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE)),
            Some(Command::NextTrack)
        );
    }

    #[test]
    fn navigation_keys_map_to_seek_and_volume_commands() {
        for code in [KeyCode::Left, KeyCode::Char('h')] {
            assert_eq!(
                command_for_key(KeyEvent::new(code, KeyModifiers::NONE)),
                Some(Command::Seek {
                    direction: crate::app::SeekDirection::Backward,
                    amount: std::time::Duration::from_secs(5),
                })
            );
        }
        for code in [KeyCode::Right, KeyCode::Char('l')] {
            assert_eq!(
                command_for_key(KeyEvent::new(code, KeyModifiers::NONE)),
                Some(Command::Seek {
                    direction: crate::app::SeekDirection::Forward,
                    amount: std::time::Duration::from_secs(5),
                })
            );
        }
        for (code, amount) in [
            (KeyCode::Down, -0.05),
            (KeyCode::Char('j'), -0.05),
            (KeyCode::Up, 0.05),
            (KeyCode::Char('k'), 0.05),
        ] {
            assert_eq!(
                command_for_key(KeyEvent::new(code, KeyModifiers::NONE)),
                Some(Command::AdjustVolume(amount))
            );
        }
    }

    #[test]
    fn slash_opens_search_from_now_playing() {
        assert_eq!(
            browser_command_for_key(
                KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
                BrowserMode::Closed,
            ),
            Some(BrowserCommand::OpenSearch)
        );
    }

    #[test]
    fn search_editor_accepts_text_and_submission() {
        assert_eq!(
            browser_command_for_key(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                BrowserMode::Editing,
            ),
            Some(BrowserCommand::Insert('q'))
        );
        assert_eq!(
            browser_command_for_key(
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
                BrowserMode::Editing,
            ),
            Some(BrowserCommand::Backspace)
        );
        assert_eq!(
            browser_command_for_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                BrowserMode::Editing,
            ),
            Some(BrowserCommand::Submit)
        );
    }

    #[test]
    fn catalogue_pages_use_vim_navigation_and_enter() {
        for (code, command) in [
            (KeyCode::Char('j'), BrowserCommand::Next),
            (KeyCode::Char('k'), BrowserCommand::Previous),
            (KeyCode::Char('h'), BrowserCommand::Back),
            (KeyCode::Enter, BrowserCommand::Activate),
        ] {
            assert_eq!(
                browser_command_for_key(KeyEvent::new(code, KeyModifiers::NONE), BrowserMode::Page),
                Some(command)
            );
        }
    }
}
