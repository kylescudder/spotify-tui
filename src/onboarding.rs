use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::SPOTIFY_API_REDIRECT_URI;

const MAX_CLIENT_ID_LENGTH: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStage {
    Configure,
    Authorize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingAction {
    None,
    SaveClientId(String),
    Authenticate,
    EditClientId,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnboardingState {
    stage: OnboardingStage,
    client_id: String,
    redirect_uri: String,
    config_path: PathBuf,
    error: Option<String>,
}

impl OnboardingState {
    pub fn configure(config_path: PathBuf) -> Self {
        Self::edit_client_id(
            config_path,
            String::new(),
            SPOTIFY_API_REDIRECT_URI.to_owned(),
        )
    }

    pub fn edit_client_id(config_path: PathBuf, client_id: String, redirect_uri: String) -> Self {
        Self {
            stage: OnboardingStage::Configure,
            client_id,
            redirect_uri,
            config_path,
            error: None,
        }
    }

    pub fn authorize(
        config_path: PathBuf,
        client_id: String,
        redirect_uri: String,
        error: Option<String>,
    ) -> Self {
        Self {
            stage: OnboardingStage::Authorize,
            client_id,
            redirect_uri,
            config_path,
            error,
        }
    }

    pub const fn stage(&self) -> OnboardingStage {
        self.stage
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> OnboardingAction {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return OnboardingAction::Quit;
        }

        match self.stage {
            OnboardingStage::Configure => self.handle_configure_key(key),
            OnboardingStage::Authorize => self.handle_authorize_key(key),
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.stage != OnboardingStage::Configure {
            return;
        }

        for character in text
            .trim()
            .chars()
            .filter(|character| !character.is_control())
        {
            if self.client_id.len() >= MAX_CLIENT_ID_LENGTH {
                break;
            }
            self.client_id.push(character);
        }
        self.error = None;
    }

    fn handle_configure_key(&mut self, key: KeyEvent) -> OnboardingAction {
        match key.code {
            KeyCode::Esc => OnboardingAction::Quit,
            KeyCode::Enter => {
                let client_id = self.client_id.trim();
                if client_id.is_empty() {
                    self.error = Some("A Spotify client ID is required to continue.".to_owned());
                    OnboardingAction::None
                } else {
                    OnboardingAction::SaveClientId(client_id.to_owned())
                }
            }
            KeyCode::Backspace => {
                self.client_id.pop();
                self.error = None;
                OnboardingAction::None
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                self.client_id.clear();
                self.error = None;
                OnboardingAction::None
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.client_id.len() < MAX_CLIENT_ID_LENGTH =>
            {
                self.client_id.push(character);
                self.error = None;
                OnboardingAction::None
            }
            _ => OnboardingAction::None,
        }
    }

    fn handle_authorize_key(&self, key: KeyEvent) -> OnboardingAction {
        match key.code {
            KeyCode::Enter => OnboardingAction::Authenticate,
            KeyCode::Char('e') => OnboardingAction::EditClientId,
            KeyCode::Esc | KeyCode::Char('q') => OnboardingAction::Quit,
            _ => OnboardingAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn configuration_cannot_continue_without_a_client_id() {
        let mut state = OnboardingState::configure(PathBuf::from("config.toml"));

        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            OnboardingAction::None
        );
        assert_eq!(
            state.error(),
            Some("A Spotify client ID is required to continue.")
        );
    }

    #[test]
    fn configuration_collects_typing_and_paste_before_saving() {
        let mut state = OnboardingState::configure(PathBuf::from("config.toml"));
        state.handle_key(key(KeyCode::Char('a')));
        state.paste("bc123\n");

        assert_eq!(state.client_id(), "abc123");
        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            OnboardingAction::SaveClientId("abc123".to_owned())
        );
    }

    #[test]
    fn authorization_requires_an_explicit_confirmation() {
        let mut state = OnboardingState::authorize(
            PathBuf::from("config.toml"),
            "client-id".to_owned(),
            SPOTIFY_API_REDIRECT_URI.to_owned(),
            None,
        );

        assert_eq!(
            state.handle_key(key(KeyCode::Enter)),
            OnboardingAction::Authenticate
        );
        assert_eq!(state.handle_key(key(KeyCode::Esc)), OnboardingAction::Quit);
        assert_eq!(
            state.handle_key(key(KeyCode::Char('e'))),
            OnboardingAction::EditClientId
        );
    }

    #[test]
    fn correcting_a_client_id_can_clear_the_existing_value() {
        let mut state = OnboardingState::edit_client_id(
            PathBuf::from("config.toml"),
            "mistyped".to_owned(),
            "http://127.0.0.1:9876/return".to_owned(),
        );

        state.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        state.paste("corrected");

        assert_eq!(state.client_id(), "corrected");
        assert_eq!(state.redirect_uri(), "http://127.0.0.1:9876/return");
    }
}
