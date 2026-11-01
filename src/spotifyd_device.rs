use std::{env, fs, path::PathBuf};

use directories::ProjectDirs;
use gethostname::gethostname;
use serde::Deserialize;
use sha1::{Digest, Sha1};

use crate::spotifyd_lifecycle::SPOTIFYD_CONFIG_ENV;

const CONFIG_FILE_NAME: &str = "spotifyd.conf";

#[derive(Debug, Default, Deserialize)]
struct SpotifydConfig {
    global: Option<SpotifydSection>,
    spotifyd: Option<SpotifydSection>,
}

#[derive(Debug, Deserialize)]
struct SpotifydSection {
    device_name: Option<String>,
}

// Spotifyd exposes neither its device name nor its ID locally, so both are
// worked out the way the pinned Spotifyd 0.4.2 does.
pub fn device_id() -> String {
    let config = config_path().and_then(|path| fs::read_to_string(path).ok());
    let hostname = gethostname();
    id_for_name(&device_name(config.as_deref(), &hostname.to_string_lossy()))
}

fn config_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os(SPOTIFYD_CONFIG_ENV) {
        return Some(path.into());
    }
    let directories = ProjectDirs::from("", "", "spotifyd")?;
    [
        directories.config_dir().join(CONFIG_FILE_NAME),
        PathBuf::from("/etc").join(CONFIG_FILE_NAME),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn device_name(config: Option<&str>, hostname: &str) -> String {
    let config: SpotifydConfig = config
        .and_then(|config| toml::from_str(config).ok())
        .unwrap_or_default();
    [config.spotifyd, config.global]
        .into_iter()
        .flatten()
        .find_map(|section| section.device_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Spotifyd@{hostname}"))
}

fn id_for_name(name: &str) -> String {
    Sha1::digest(name.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_ids_are_the_hex_sha1_of_the_name() {
        assert_eq!(
            id_for_name("Archie"),
            "5c4431b9465afb337d1c4be5c5d7f9047746d3f9"
        );
    }

    #[test]
    fn device_names_come_from_the_global_section() {
        assert_eq!(
            device_name(
                Some("[global]\ndevice_name = \"Archie\"\nbitrate = 320\n"),
                "stevie"
            ),
            "Archie"
        );
    }

    #[test]
    fn the_spotifyd_section_overrides_the_global_section() {
        assert_eq!(
            device_name(
                Some("[global]\ndevice_name = \"One\"\n[spotifyd]\ndevice_name = \"Two\"\n"),
                "stevie"
            ),
            "Two"
        );
    }

    #[test]
    fn a_missing_or_blank_name_uses_the_spotifyd_default() {
        for config in [
            None,
            Some("[global]\nbitrate = 320\n"),
            Some("[global]\ndevice_name = \"  \"\n"),
            Some("not toml ["),
        ] {
            assert_eq!(device_name(config, "stevie"), "Spotifyd@stevie");
        }
    }
}
