use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::Command;

pub fn command_for_key(key: KeyEvent) -> Option<Command> {
    if key.code == KeyCode::Char('a') && key.modifiers.is_empty() {
        Some(Command::Authenticate)
    } else if key.code == KeyCode::Char('r') && key.modifiers.is_empty() {
        Some(Command::RetryConnection)
    } else if matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        Some(Command::Quit)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_keys_map_to_quit_command() {
        for key in [
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            assert_eq!(command_for_key(key), Some(Command::Quit));
        }
    }

    #[test]
    fn unrelated_keys_do_not_produce_a_command() {
        assert_eq!(
            command_for_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
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
}
