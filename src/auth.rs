use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    process::{Command, ExitStatus, Stdio},
};

use thiserror::Error;

use crate::spotifyd_lifecycle::SpotifydLifecycle;

pub use crate::spotifyd_lifecycle::{SPOTIFYD_CONFIG_ENV, SPOTIFYD_PROGRAM_ENV};

const DEFAULT_SPOTIFYD_PROGRAM: &str = "spotifyd";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthRequest {
    program: OsString,
    arguments: Vec<OsString>,
}

impl AuthRequest {
    pub fn from_environment(extra_arguments: &[OsString]) -> Result<Self, AuthError> {
        let program = optional_environment_value(SPOTIFYD_PROGRAM_ENV)?
            .unwrap_or_else(|| OsString::from(DEFAULT_SPOTIFYD_PROGRAM));
        let config_path = optional_environment_value(SPOTIFYD_CONFIG_ENV)?;

        Ok(Self::new(program, config_path, extra_arguments))
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    fn new(program: OsString, config_path: Option<OsString>, extra_arguments: &[OsString]) -> Self {
        let mut arguments = Vec::with_capacity(extra_arguments.len() + 3);
        if let Some(config_path) = config_path {
            arguments.push(OsString::from("--config-path"));
            arguments.push(config_path);
        }
        arguments.push(OsString::from("authenticate"));
        arguments.extend_from_slice(extra_arguments);

        Self { program, arguments }
    }

    fn run(&self) -> Result<(), AuthError> {
        let status = Command::new(&self.program)
            .args(&self.arguments)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|source| AuthError::Launch {
                program: self.program.to_string_lossy().into_owned(),
                source,
            })?;

        if status.success() {
            Ok(())
        } else {
            Err(AuthError::Unsuccessful {
                program: self.program.to_string_lossy().into_owned(),
                status,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthOutcome {
    restart_warning: Option<String>,
}

impl AuthOutcome {
    pub fn restart_warning(&self) -> Option<&str> {
        self.restart_warning.as_deref()
    }
}

pub fn authenticate(extra_arguments: &[OsString]) -> Result<AuthOutcome, AuthError> {
    AuthRequest::from_environment(extra_arguments)?.run()?;
    let restart_warning = SpotifydLifecycle::from_environment()
        .and_then(|lifecycle| lifecycle.restart())
        .err()
        .map(|error| error.to_string());

    Ok(AuthOutcome { restart_warning })
}

fn optional_environment_value(name: &'static str) -> Result<Option<OsString>, AuthError> {
    let Some(value) = env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Err(AuthError::EmptyEnvironmentValue(name));
    }

    Ok(Some(value))
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("{0} is set but empty")]
    EmptyEnvironmentValue(&'static str),
    #[error("could not start {program}: {source}")]
    Launch {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("{program} authenticate exited with {status}")]
    Unsuccessful { program: String, status: ExitStatus },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_request_uses_spotifyd_authenticate_subcommand() {
        let request = AuthRequest::new(OsString::from("spotifyd"), None, &[]);
        assert_eq!(request.program(), OsStr::new("spotifyd"));
        assert_eq!(request.arguments(), [OsString::from("authenticate")]);
    }

    #[test]
    fn config_path_precedes_subcommand_and_extra_arguments_follow_it() {
        let request = AuthRequest::new(
            OsString::from("/nix/store/spotifyd/bin/spotifyd"),
            Some(OsString::from("/nix/store/spotifyd.conf")),
            &[OsString::from("--oauth-port"), OsString::from("9876")],
        );

        assert_eq!(
            request.arguments(),
            [
                OsString::from("--config-path"),
                OsString::from("/nix/store/spotifyd.conf"),
                OsString::from("authenticate"),
                OsString::from("--oauth-port"),
                OsString::from("9876"),
            ]
        );
    }
}
