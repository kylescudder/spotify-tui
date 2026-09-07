use std::ffi::OsString;

use thiserror::Error;

pub const HELP: &str = "\
Spotify TUI — local-first terminal Spotify interface

Usage:
  spotify-tui                 Start the terminal interface
  spotify-tui auth [-- ARGS]  Authenticate spotifyd, forwarding optional ARGS
  spotify-tui catalog-auth    Authenticate Spotify catalogue access
  spotify-tui --help          Show this help
  spotify-tui --version       Show the installed version
";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchMode {
    Tui,
    Authenticate { spotifyd_arguments: Vec<OsString> },
    CatalogAuthenticate,
    Help,
    Version,
}

pub fn parse_args(arguments: impl IntoIterator<Item = OsString>) -> Result<LaunchMode, CliError> {
    let mut arguments = arguments.into_iter();
    let Some(command) = arguments.next() else {
        return Ok(LaunchMode::Tui);
    };

    if command == "--help" || command == "-h" {
        return Ok(LaunchMode::Help);
    }

    if command == "--version" || command == "-V" {
        return Ok(LaunchMode::Version);
    }

    if command == "auth" || command == "authenticate" {
        let mut spotifyd_arguments: Vec<_> = arguments.collect();
        if spotifyd_arguments
            .first()
            .is_some_and(|value| value == "--")
        {
            spotifyd_arguments.remove(0);
        }
        return Ok(LaunchMode::Authenticate { spotifyd_arguments });
    }

    if command == "catalog-auth" || command == "api-auth" {
        if arguments.next().is_some() {
            return Err(CliError::UnexpectedArguments(command));
        }
        return Ok(LaunchMode::CatalogAuthenticate);
    }

    Err(CliError::UnknownCommand(command))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    #[error("unknown command '{}'; run spotify-tui --help", .0.to_string_lossy())]
    UnknownCommand(OsString),
    #[error("command '{}' does not accept arguments", .0.to_string_lossy())]
    UnexpectedArguments(OsString),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_starts_the_tui() {
        assert_eq!(parse_args([]).unwrap(), LaunchMode::Tui);
    }

    #[test]
    fn auth_arguments_are_forwarded_without_separator() {
        assert_eq!(
            parse_args([
                OsString::from("auth"),
                OsString::from("--"),
                OsString::from("--oauth-port"),
                OsString::from("9876"),
            ])
            .unwrap(),
            LaunchMode::Authenticate {
                spotifyd_arguments: vec![OsString::from("--oauth-port"), OsString::from("9876")]
            }
        );
    }

    #[test]
    fn authenticate_alias_is_supported() {
        assert_eq!(
            parse_args([OsString::from("authenticate")]).unwrap(),
            LaunchMode::Authenticate {
                spotifyd_arguments: Vec::new()
            }
        );
    }

    #[test]
    fn version_flags_show_the_package_version() {
        assert_eq!(
            parse_args([OsString::from("--version")]).unwrap(),
            LaunchMode::Version
        );
        assert_eq!(
            parse_args([OsString::from("-V")]).unwrap(),
            LaunchMode::Version
        );
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn catalogue_authentication_has_a_dedicated_command() {
        assert_eq!(
            parse_args([OsString::from("catalog-auth")]).unwrap(),
            LaunchMode::CatalogAuthenticate
        );
        assert_eq!(
            parse_args([OsString::from("api-auth")]).unwrap(),
            LaunchMode::CatalogAuthenticate
        );
        assert_eq!(
            parse_args([OsString::from("catalog-auth"), OsString::from("unexpected"),]),
            Err(CliError::UnexpectedArguments(OsString::from(
                "catalog-auth"
            )))
        );
    }

    #[test]
    fn unknown_command_is_rejected() {
        assert_eq!(
            parse_args([OsString::from("login")]),
            Err(CliError::UnknownCommand(OsString::from("login")))
        );
    }
}
