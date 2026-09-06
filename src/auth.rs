use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    process::{Command, ExitStatus, Stdio},
};

use thiserror::Error;

pub const SPOTIFYD_PROGRAM_ENV: &str = "SPOTIFY_TUI_SPOTIFYD";
pub const SPOTIFYD_CONFIG_ENV: &str = "SPOTIFY_TUI_SPOTIFYD_CONFIG";
pub const SPOTIFYD_SERVICE_ENV: &str = "SPOTIFY_TUI_SPOTIFYD_SERVICE";
pub const SYSTEMCTL_PROGRAM_ENV: &str = "SPOTIFY_TUI_SYSTEMCTL";

const DEFAULT_SPOTIFYD_PROGRAM: &str = "spotifyd";
const DEFAULT_SPOTIFYD_SERVICE: &str = "spotifyd.service";
const DEFAULT_SYSTEMCTL_PROGRAM: &str = "systemctl";

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
    let restart_warning = restart_user_service().err().map(|error| error.to_string());

    Ok(AuthOutcome { restart_warning })
}

fn restart_user_service() -> Result<(), ServiceRestartError> {
    let systemctl = optional_environment_value(SYSTEMCTL_PROGRAM_ENV)
        .map_err(ServiceRestartError::Configuration)?
        .unwrap_or_else(|| OsString::from(DEFAULT_SYSTEMCTL_PROGRAM));
    let service = optional_environment_value(SPOTIFYD_SERVICE_ENV)
        .map_err(ServiceRestartError::Configuration)?
        .unwrap_or_else(|| OsString::from(DEFAULT_SPOTIFYD_SERVICE));
    let output = Command::new(&systemctl)
        .args([OsStr::new("--user"), OsStr::new("restart"), &service])
        .stdin(Stdio::null())
        .output()
        .map_err(|source| ServiceRestartError::Launch {
            program: systemctl.to_string_lossy().into_owned(),
            source,
        })?;

    if output.status.success() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(ServiceRestartError::Unsuccessful {
        service: service.to_string_lossy().into_owned(),
        status: output.status,
        detail,
    })
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

#[derive(Debug, Error)]
enum ServiceRestartError {
    #[error(transparent)]
    Configuration(#[from] AuthError),
    #[error("could not start {program} to restart spotifyd: {source}")]
    Launch {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("could not restart {service} ({status}){detail_suffix}", detail_suffix = detail_suffix(.detail))]
    Unsuccessful {
        service: String,
        status: ExitStatus,
        detail: String,
    },
}

fn detail_suffix(detail: &str) -> String {
    if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    }
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

    #[test]
    fn restart_error_includes_stderr_when_available() {
        assert_eq!(detail_suffix("unit was not found"), ": unit was not found");
        assert_eq!(detail_suffix(""), "");
    }
}
