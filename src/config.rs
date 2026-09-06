use std::{
    collections::BTreeMap,
    env, fs, io,
    path::{Path, PathBuf},
};

use ratatui::style::Color;
use serde::Deserialize;
use thiserror::Error;

pub const CONFIG_VERSION: u32 = 1;
pub const CONFIG_PATH_ENV: &str = "SPOTIFY_TUI_CONFIG";
pub const BUILT_IN_THEME_NAMES: [&str; 3] = ["spotify", "midnight", "high-contrast"];

const DEFAULT_THEME_NAME: &str = "spotify";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    theme_name: String,
    theme: Theme,
    source_path: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme_name: DEFAULT_THEME_NAME.to_owned(),
            theme: Theme::spotify(),
            source_path: None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        if let Some(path) = env::var_os(CONFIG_PATH_ENV) {
            if path.is_empty() {
                return Err(ConfigError::EmptyPathOverride);
            }
            return Self::load_from(path);
        }

        let Some(path) = default_config_path() else {
            return Ok(Self::default());
        };

        Self::load_optional_path(&path)
    }

    pub fn load_from(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        Self::parse(&contents, Some(path.to_owned()))
    }

    pub fn from_toml(contents: &str) -> Result<Self, ConfigError> {
        Self::parse(contents, None)
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    fn load_optional_path(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents, Some(path.to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            }),
        }
    }

    fn parse(contents: &str, source_path: Option<PathBuf>) -> Result<Self, ConfigError> {
        let document: ConfigDocument =
            toml::from_str(contents).map_err(|source| ConfigError::Toml {
                location: source_path.as_deref().map_or_else(
                    || "configuration".to_owned(),
                    |path| path.display().to_string(),
                ),
                source,
            })?;

        document.resolve(source_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    background: Color,
    foreground: Color,
    muted: Color,
    border: Color,
    accent: Color,
    warning: Color,
    error: Color,
}

impl Theme {
    pub const fn background(&self) -> Color {
        self.background
    }

    pub const fn foreground(&self) -> Color {
        self.foreground
    }

    pub const fn muted(&self) -> Color {
        self.muted
    }

    pub const fn border(&self) -> Color {
        self.border
    }

    pub const fn accent(&self) -> Color {
        self.accent
    }

    pub const fn warning(&self) -> Color {
        self.warning
    }

    pub const fn error(&self) -> Color {
        self.error
    }

    fn built_in(name: &str) -> Option<Self> {
        match name {
            "spotify" => Some(Self::spotify()),
            "midnight" => Some(Self::midnight()),
            "high-contrast" => Some(Self::high_contrast()),
            _ => None,
        }
    }

    const fn spotify() -> Self {
        Self {
            background: Color::Rgb(13, 17, 23),
            foreground: Color::Rgb(230, 237, 243),
            muted: Color::Rgb(105, 115, 134),
            border: Color::Rgb(48, 54, 61),
            accent: Color::Rgb(30, 215, 96),
            warning: Color::Rgb(246, 196, 83),
            error: Color::Rgb(255, 107, 107),
        }
    }

    const fn midnight() -> Self {
        Self {
            background: Color::Rgb(11, 16, 32),
            foreground: Color::Rgb(219, 234, 254),
            muted: Color::Rgb(100, 116, 139),
            border: Color::Rgb(39, 52, 73),
            accent: Color::Rgb(125, 211, 252),
            warning: Color::Rgb(251, 191, 36),
            error: Color::Rgb(251, 113, 133),
        }
    }

    const fn high_contrast() -> Self {
        Self {
            background: Color::Black,
            foreground: Color::White,
            muted: Color::Gray,
            border: Color::White,
            accent: Color::LightGreen,
            warning: Color::LightYellow,
            error: Color::LightRed,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("{CONFIG_PATH_ENV} is set but empty")]
    EmptyPathOverride,
    #[error("could not read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse {location}: {source}")]
    Toml {
        location: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("unsupported config version {found}; this build supports version {CONFIG_VERSION}")]
    UnsupportedVersion { found: u32 },
    #[error(
        "unknown colour scheme '{0}'; use one of spotify, midnight, high-contrast, or a name from [themes]"
    )]
    UnknownTheme(String),
    #[error("custom colour scheme '{0}' conflicts with a built-in scheme")]
    BuiltInThemeRedefined(String),
    #[error("custom colour scheme '{theme}' has unknown built-in base '{base}'")]
    UnknownBase { theme: String, base: String },
    #[error(
        "invalid colour '{value}' for themes.{theme}.{field}; use #RRGGBB or a named terminal colour"
    )]
    InvalidColor {
        theme: String,
        field: &'static str,
        value: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigDocument {
    version: u32,
    #[serde(default = "default_theme_name")]
    theme: String,
    #[serde(default)]
    themes: BTreeMap<String, ThemeDefinition>,
}

impl ConfigDocument {
    fn resolve(self, source_path: Option<PathBuf>) -> Result<Config, ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: self.version,
            });
        }

        let mut custom_themes = BTreeMap::new();
        for (name, definition) in self.themes {
            if Theme::built_in(&name).is_some() {
                return Err(ConfigError::BuiltInThemeRedefined(name));
            }

            let base_name = definition.base.as_deref().unwrap_or(DEFAULT_THEME_NAME);
            let base = Theme::built_in(base_name).ok_or_else(|| ConfigError::UnknownBase {
                theme: name.clone(),
                base: base_name.to_owned(),
            })?;
            custom_themes.insert(name.clone(), definition.apply(&name, base)?);
        }

        let theme = Theme::built_in(&self.theme)
            .or_else(|| custom_themes.get(&self.theme).cloned())
            .ok_or_else(|| ConfigError::UnknownTheme(self.theme.clone()))?;

        Ok(Config {
            theme_name: self.theme,
            theme,
            source_path,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDefinition {
    base: Option<String>,
    background: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    border: Option<String>,
    accent: Option<String>,
    warning: Option<String>,
    error: Option<String>,
}

impl ThemeDefinition {
    fn apply(self, name: &str, base: Theme) -> Result<Theme, ConfigError> {
        Ok(Theme {
            background: resolve_color(self.background, name, "background", base.background)?,
            foreground: resolve_color(self.foreground, name, "foreground", base.foreground)?,
            muted: resolve_color(self.muted, name, "muted", base.muted)?,
            border: resolve_color(self.border, name, "border", base.border)?,
            accent: resolve_color(self.accent, name, "accent", base.accent)?,
            warning: resolve_color(self.warning, name, "warning", base.warning)?,
            error: resolve_color(self.error, name, "error", base.error)?,
        })
    }
}

fn default_theme_name() -> String {
    DEFAULT_THEME_NAME.to_owned()
}

fn default_config_path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|root| root.join("spotify-tui").join("config.toml"))
}

fn resolve_color(
    value: Option<String>,
    theme: &str,
    field: &'static str,
    fallback: Color,
) -> Result<Color, ConfigError> {
    let Some(value) = value else {
        return Ok(fallback);
    };

    parse_color(&value).ok_or_else(|| ConfigError::InvalidColor {
        theme: theme.to_owned(),
        field,
        value,
    })
}

fn parse_color(value: &str) -> Option<Color> {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    if let Some(hex) = normalized.strip_prefix('#')
        && hex.len() == 6
    {
        let rgb = u32::from_str_radix(hex, 16).ok()?;
        return Some(Color::Rgb(
            ((rgb >> 16) & 0xff) as u8,
            ((rgb >> 8) & 0xff) as u8,
            (rgb & 0xff) as u8,
        ));
    }

    match normalized.as_str() {
        "default" | "reset" => Some(Color::Reset),
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "gray" | "grey" => Some(Color::Gray),
        "dark-gray" | "dark-grey" => Some(Color::DarkGray),
        "light-red" => Some(Color::LightRed),
        "light-green" => Some(Color::LightGreen),
        "light-yellow" => Some(Color::LightYellow),
        "light-blue" => Some(Color::LightBlue),
        "light-magenta" => Some(Color::LightMagenta),
        "light-cyan" => Some(Color::LightCyan),
        "white" => Some(Color::White),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn defaults_to_spotify_theme() {
        let config = Config::default();
        assert_eq!(config.theme_name(), "spotify");
        assert_eq!(config.theme(), &Theme::spotify());
        assert!(config.source_path().is_none());
    }

    #[test]
    fn missing_optional_config_uses_defaults() {
        let unique = NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "spotify-tui-missing-config-{}-{unique}.toml",
            std::process::id()
        ));

        let config = Config::load_optional_path(&path).expect("missing config should be allowed");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn selects_a_built_in_theme() {
        let config = Config::from_toml(
            r#"
version = 1
theme = "midnight"
"#,
        )
        .expect("built-in theme should resolve");

        assert_eq!(config.theme_name(), "midnight");
        assert_eq!(config.theme(), &Theme::midnight());
    }

    #[test]
    fn custom_theme_inherits_and_overrides_colours() {
        let config = Config::from_toml(
            r##"
version = 1
theme = "ocean"

[themes.ocean]
base = "midnight"
accent = "#12AbCd"
warning = "light-yellow"
"##,
        )
        .expect("custom theme should resolve");

        assert_eq!(config.theme_name(), "ocean");
        assert_eq!(config.theme().background(), Theme::midnight().background());
        assert_eq!(config.theme().accent(), Color::Rgb(0x12, 0xab, 0xcd));
        assert_eq!(config.theme().warning(), Color::LightYellow);
    }

    #[test]
    fn rejects_unsupported_config_versions() {
        let error = Config::from_toml("version = 2").expect_err("version should be rejected");
        assert!(matches!(
            error,
            ConfigError::UnsupportedVersion { found: 2 }
        ));
    }

    #[test]
    fn rejects_unknown_selected_theme() {
        let error = Config::from_toml(
            r#"
version = 1
theme = "missing"
"#,
        )
        .expect_err("unknown theme should be rejected");

        assert!(matches!(error, ConfigError::UnknownTheme(name) if name == "missing"));
    }

    #[test]
    fn invalid_colour_names_report_the_theme_and_field() {
        let error = Config::from_toml(
            r#"
version = 1
theme = "broken"

[themes.broken]
accent = "ultraviolet"
"#,
        )
        .expect_err("invalid colour should be rejected");

        assert!(matches!(
            error,
            ConfigError::InvalidColor {
                theme,
                field: "accent",
                value,
            } if theme == "broken" && value == "ultraviolet"
        ));
    }

    #[test]
    fn custom_theme_base_must_be_built_in() {
        let error = Config::from_toml(
            r#"
version = 1
theme = "ocean"

[themes.ocean]
base = "another-custom-theme"
"#,
        )
        .expect_err("unknown base should be rejected");

        assert!(matches!(
            error,
            ConfigError::UnknownBase { theme, base }
                if theme == "ocean" && base == "another-custom-theme"
        ));
    }

    #[test]
    fn malformed_non_ascii_hex_colour_is_rejected() {
        assert_eq!(parse_color("#aébcd"), None);
    }

    #[test]
    fn bundled_catppuccin_mocha_theme_stays_valid() {
        let config = Config::from_toml(include_str!("../themes/catppuccin-mocha.toml"))
            .expect("bundled Catppuccin theme should resolve");

        assert_eq!(config.theme_name(), "catppuccin-mocha");
        assert_eq!(config.theme().background(), Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(config.theme().foreground(), Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(config.theme().muted(), Color::Rgb(0x7f, 0x84, 0x9c));
        assert_eq!(config.theme().border(), Color::Rgb(0x45, 0x47, 0x5a));
        assert_eq!(config.theme().accent(), Color::Rgb(0xcb, 0xa6, 0xf7));
        assert_eq!(config.theme().warning(), Color::Rgb(0xf9, 0xe2, 0xaf));
        assert_eq!(config.theme().error(), Color::Rgb(0xf3, 0x8b, 0xa8));
    }
}
