use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

use crate::config::SpotifyApiConfig;

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);
const AUTHORIZATION_SCOPES: &str = "user-modify-playback-state";
const AUTHORIZATION_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredToken {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    #[serde(default)]
    authorization_version: u32,
}

impl StoredToken {
    pub(crate) fn access_token(&self) -> &str {
        &self.access_token
    }

    pub(crate) fn needs_refresh(&self) -> bool {
        unix_timestamp().saturating_add(EXPIRY_MARGIN.as_secs()) >= self.expires_at
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthorizationRequest {
    url: Url,
    verifier: String,
    state: String,
}

pub fn authenticate(config: &SpotifyApiConfig) -> Result<PathBuf, CatalogAuthError> {
    authenticate_with(config, None, |url| {
        println!("Browse to: {url}");
        if let Err(error) = launch_browser(url.as_str()) {
            eprintln!("warning: could not open a browser automatically: {error}");
        }
        Ok(())
    })
}

pub(crate) fn authenticate_from_tui(
    config: &SpotifyApiConfig,
    cancelled: &AtomicBool,
) -> Result<PathBuf, CatalogAuthError> {
    authenticate_with(config, Some(cancelled), |url| {
        launch_browser(url.as_str()).map_err(|source| CatalogAuthError::OpenBrowser {
            url: url.to_string(),
            source,
        })
    })
}

fn authenticate_with(
    config: &SpotifyApiConfig,
    cancelled: Option<&AtomicBool>,
    present: impl FnOnce(&Url) -> Result<(), CatalogAuthError>,
) -> Result<PathBuf, CatalogAuthError> {
    let redirect = Url::parse(config.redirect_uri())
        .map_err(|_| CatalogAuthError::InvalidRedirectUri(config.redirect_uri().to_owned()))?;
    let listener = bind_callback(&redirect)?;
    let request = authorization_request(config, &redirect)?;

    present(&request.url)?;

    let code = wait_for_callback(&listener, &redirect, &request.state, cancelled)?;
    let agent = spotify_agent();
    let token = exchange_code(config, &code, &request.verifier, &agent)?;
    let path = token_cache_path(config)?;
    save_token(&path, &token)?;
    Ok(path)
}

pub(crate) fn spotify_agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(HTTP_TIMEOUT))
        .user_agent(concat!("spotify-tui/", env!("CARGO_PKG_VERSION")))
        .build();
    config.into()
}

pub(crate) fn load_token(config: &SpotifyApiConfig) -> Result<StoredToken, CatalogAuthError> {
    let path = token_cache_path(config)?;
    let bytes = fs::read(&path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            CatalogAuthError::NotAuthenticated(path.clone())
        } else {
            CatalogAuthError::ReadToken {
                path: path.clone(),
                source,
            }
        }
    })?;
    let token: StoredToken = serde_json::from_slice(&bytes)
        .map_err(|source| CatalogAuthError::ParseToken { path, source })?;
    if token.authorization_version < AUTHORIZATION_VERSION {
        return Err(CatalogAuthError::AuthorizationExpired);
    }
    Ok(token)
}

pub(crate) fn refresh_token(
    config: &SpotifyApiConfig,
    current: &StoredToken,
    agent: &ureq::Agent,
) -> Result<StoredToken, CatalogAuthError> {
    let mut response = agent
        .post(TOKEN_URL)
        .send_form([
            ("grant_type", "refresh_token"),
            ("refresh_token", current.refresh_token.as_str()),
            ("client_id", config.client_id()),
        ])
        .map_err(|error| match error {
            ureq::Error::StatusCode(400 | 401) => CatalogAuthError::AuthorizationExpired,
            error => CatalogAuthError::TokenRequest(error.to_string()),
        })?;
    let response: TokenResponse = response
        .body_mut()
        .read_json()
        .map_err(|error| CatalogAuthError::TokenResponse(error.to_string()))?;
    let token = StoredToken {
        access_token: response.access_token,
        refresh_token: response
            .refresh_token
            .unwrap_or_else(|| current.refresh_token.clone()),
        expires_at: unix_timestamp().saturating_add(response.expires_in),
        authorization_version: current.authorization_version,
    };
    save_token(&token_cache_path(config)?, &token)?;
    Ok(token)
}

fn authorization_request(
    config: &SpotifyApiConfig,
    redirect: &Url,
) -> Result<AuthorizationRequest, CatalogAuthError> {
    let verifier = random_urlsafe(32)?;
    let state = random_urlsafe(24)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(AUTHORIZE_URL).expect("Spotify authorization URL is valid");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", config.client_id())
        .append_pair("redirect_uri", redirect.as_str())
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &challenge)
        .append_pair("scope", AUTHORIZATION_SCOPES)
        .append_pair("state", &state);
    Ok(AuthorizationRequest {
        url,
        verifier,
        state,
    })
}

fn random_urlsafe(byte_count: usize) -> Result<String, CatalogAuthError> {
    let mut bytes = vec![0; byte_count];
    getrandom::fill(&mut bytes).map_err(|error| CatalogAuthError::Random(error.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn bind_callback(redirect: &Url) -> Result<TcpListener, CatalogAuthError> {
    let ip = match redirect.host() {
        Some(url::Host::Ipv4(address)) => IpAddr::V4(address),
        Some(url::Host::Ipv6(address)) => IpAddr::V6(address),
        Some(url::Host::Domain(_)) | None => {
            return Err(CatalogAuthError::InvalidRedirectUri(redirect.to_string()));
        }
    };
    let port = redirect
        .port()
        .ok_or_else(|| CatalogAuthError::InvalidRedirectUri(redirect.to_string()))?;
    let address = SocketAddr::new(ip, port);
    let listener =
        TcpListener::bind(address).map_err(|source| CatalogAuthError::Bind { address, source })?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn wait_for_callback(
    listener: &TcpListener,
    redirect: &Url,
    expected_state: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<String, CatalogAuthError> {
    let deadline = Instant::now() + AUTH_TIMEOUT;
    loop {
        if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
            return Err(CatalogAuthError::AuthenticationCancelled);
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream.set_read_timeout(Some(Duration::from_secs(2)))?;
                let request_target = read_request_target(&mut stream)?;
                match callback_code(&request_target, redirect.path(), expected_state) {
                    Ok(Some(code)) => {
                        respond(
                            &mut stream,
                            200,
                            "Spotify TUI authentication complete. You can close this tab.",
                        )?;
                        return Ok(code);
                    }
                    Ok(None) => respond(&mut stream, 404, "Not found")?,
                    Err(error) => {
                        let _ = respond(
                            &mut stream,
                            400,
                            "Spotify TUI authentication failed. Return to the terminal.",
                        );
                        return Err(error);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(CatalogAuthError::CallbackTimeout);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(source) => return Err(CatalogAuthError::Callback(source)),
        }
    }
}

fn read_request_target(stream: &mut TcpStream) -> Result<String, CatalogAuthError> {
    let mut first_line = String::new();
    BufReader::new(stream)
        .take(8 * 1024)
        .read_line(&mut first_line)
        .map_err(CatalogAuthError::Callback)?;
    let mut fields = first_line.split_whitespace();
    if fields.next() != Some("GET") {
        return Err(CatalogAuthError::InvalidCallback);
    }
    fields
        .next()
        .map(str::to_owned)
        .ok_or(CatalogAuthError::InvalidCallback)
}

fn callback_code(
    request_target: &str,
    expected_path: &str,
    expected_state: &str,
) -> Result<Option<String>, CatalogAuthError> {
    let callback = Url::parse(&format!("http://127.0.0.1{request_target}"))
        .map_err(|_| CatalogAuthError::InvalidCallback)?;
    if callback.path() != expected_path {
        return Ok(None);
    }
    let parameters = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if parameters.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err(CatalogAuthError::StateMismatch);
    }
    if let Some(error) = parameters.get("error") {
        return Err(CatalogAuthError::AuthorizationDenied(error.to_string()));
    }
    parameters
        .get("code")
        .map(|code| Some(code.to_string()))
        .ok_or(CatalogAuthError::InvalidCallback)
}

fn respond(stream: &mut TcpStream, status: u16, message: &str) -> io::Result<()> {
    let reason = if status == 200 { "OK" } else { "Bad Request" };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{message}",
        message.len()
    )
}

fn exchange_code(
    config: &SpotifyApiConfig,
    code: &str,
    verifier: &str,
    agent: &ureq::Agent,
) -> Result<StoredToken, CatalogAuthError> {
    let mut response = agent
        .post(TOKEN_URL)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", config.redirect_uri()),
            ("client_id", config.client_id()),
            ("code_verifier", verifier),
        ])
        .map_err(|error| CatalogAuthError::TokenRequest(error.to_string()))?;
    let response: TokenResponse = response
        .body_mut()
        .read_json()
        .map_err(|error| CatalogAuthError::TokenResponse(error.to_string()))?;
    let refresh_token = response
        .refresh_token
        .ok_or(CatalogAuthError::MissingRefreshToken)?;
    Ok(StoredToken {
        access_token: response.access_token,
        refresh_token,
        expires_at: unix_timestamp().saturating_add(response.expires_in),
        authorization_version: AUTHORIZATION_VERSION,
    })
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

fn token_cache_path(config: &SpotifyApiConfig) -> Result<PathBuf, CatalogAuthError> {
    if let Some(path) = config.token_cache() {
        return Ok(path.to_owned());
    }
    if let Some(root) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root)
            .join("spotify-tui")
            .join("spotify-api-token.json"));
    }
    if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("spotify-tui")
            .join("spotify-api-token.json"));
    }
    if let Some(root) = env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(root)
            .join("spotify-tui")
            .join("spotify-api-token.json"));
    }
    Err(CatalogAuthError::NoStateDirectory)
}

fn save_token(path: &Path, token: &StoredToken) -> Result<(), CatalogAuthError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| CatalogAuthError::WriteToken {
            path: path.to_owned(),
            source,
        })?;
    }
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|source| CatalogAuthError::WriteToken {
            path: path.to_owned(),
            source,
        })?;
    serde_json::to_writer(&file, token).map_err(|source| CatalogAuthError::SerializeToken {
        path: path.to_owned(),
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
            CatalogAuthError::WriteToken {
                path: path.to_owned(),
                source,
            }
        })?;
    }
    Ok(())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(target_os = "linux")]
fn launch_browser(url: &str) -> io::Result<()> {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn launch_browser(url: &str) -> io::Result<()> {
    Command::new("open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "windows")]
fn launch_browser(url: &str) -> io::Result<()> {
    Command::new("cmd")
        .args(["/C", "start", "", url])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn launch_browser(_url: &str) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "automatic browser launch is unsupported",
    ))
}

#[derive(Debug, Error)]
pub enum CatalogAuthError {
    #[error("invalid Spotify API redirect URI '{0}'")]
    InvalidRedirectUri(String),
    #[error("could not generate secure OAuth state: {0}")]
    Random(String),
    #[error("could not open Spotify authorization automatically; open {url}: {source}")]
    OpenBrowser {
        url: String,
        #[source]
        source: io::Error,
    },
    #[error("could not listen for Spotify authentication on {address}: {source}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },
    #[error("Spotify authentication timed out after five minutes")]
    CallbackTimeout,
    #[error("Spotify catalogue authentication was cancelled")]
    AuthenticationCancelled,
    #[error("could not receive Spotify authentication callback: {0}")]
    Callback(#[from] io::Error),
    #[error("Spotify returned an invalid authentication callback")]
    InvalidCallback,
    #[error("Spotify authentication state did not match the request")]
    StateMismatch,
    #[error("Spotify authentication was denied: {0}")]
    AuthorizationDenied(String),
    #[error("Spotify catalogue authorization has expired")]
    AuthorizationExpired,
    #[error("Spotify token request failed: {0}")]
    TokenRequest(String),
    #[error("Spotify returned an invalid token response: {0}")]
    TokenResponse(String),
    #[error("Spotify did not return a refresh token")]
    MissingRefreshToken,
    #[error("could not determine a directory for the Spotify token cache")]
    NoStateDirectory,
    #[error("Spotify catalogue sign-in is required (expected token at {})", .0.display())]
    NotAuthenticated(PathBuf),
    #[error("could not read Spotify token cache {}: {source}", path.display())]
    ReadToken {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse Spotify token cache {}: {source}", path.display())]
    ParseToken {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("could not write Spotify token cache {}: {source}", path.display())]
    WriteToken {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not encode Spotify token cache {}: {source}", path.display())]
    SerializeToken {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl CatalogAuthError {
    pub(crate) const fn requires_authentication(&self) -> bool {
        matches!(
            self,
            Self::AuthorizationExpired | Self::NotAuthenticated(_) | Self::ParseToken { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn api_config() -> SpotifyApiConfig {
        Config::from_toml(
            r#"
version = 1

[spotify_api]
client_id = "client-id"
redirect_uri = "http://127.0.0.1:8989/callback"
"#,
        )
        .expect("configuration should resolve")
        .spotify_api()
        .expect("Spotify API should be configured")
        .clone()
    }

    #[test]
    fn authorization_uses_pkce_without_a_client_secret() {
        let config = api_config();
        let redirect = Url::parse(config.redirect_uri()).expect("redirect should parse");
        let request = authorization_request(&config, &redirect).expect("request should build");
        let parameters = request
            .url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(
            parameters.get("client_id").map(|value| value.as_ref()),
            Some("client-id")
        );
        assert_eq!(
            parameters
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert!(parameters.contains_key("code_challenge"));
        assert_eq!(
            parameters.get("scope").map(|value| value.as_ref()),
            Some("user-modify-playback-state")
        );
        assert!(!parameters.contains_key("client_secret"));
        assert!(request.verifier.len() >= 43);
    }

    #[test]
    fn callback_requires_matching_path_and_state() {
        assert_eq!(
            callback_code(
                "/callback?code=approved&state=expected",
                "/callback",
                "expected"
            )
            .expect("callback should parse"),
            Some("approved".to_owned())
        );
        assert_eq!(
            callback_code("/favicon.ico", "/callback", "expected")
                .expect("unrelated path should be ignored"),
            None
        );
        assert!(matches!(
            callback_code(
                "/callback?code=approved&state=wrong",
                "/callback",
                "expected"
            ),
            Err(CatalogAuthError::StateMismatch)
        ));
    }

    #[test]
    fn callback_wait_stops_when_the_tui_closes() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        listener
            .set_nonblocking(true)
            .expect("test listener should be nonblocking");
        let cancelled = AtomicBool::new(true);
        let redirect =
            Url::parse("http://127.0.0.1:8989/callback").expect("test redirect should parse");

        assert!(matches!(
            wait_for_callback(&listener, &redirect, "state", Some(&cancelled)),
            Err(CatalogAuthError::AuthenticationCancelled)
        ));
    }

    #[test]
    fn missing_expired_and_corrupt_tokens_trigger_interactive_authentication() {
        assert!(
            CatalogAuthError::NotAuthenticated(PathBuf::from("token.json"))
                .requires_authentication()
        );
        assert!(CatalogAuthError::AuthorizationExpired.requires_authentication());
        assert!(
            CatalogAuthError::ParseToken {
                path: PathBuf::from("token.json"),
                source: serde_json::from_str::<StoredToken>("not-json")
                    .expect_err("fixture should be invalid"),
            }
            .requires_authentication()
        );
        assert!(
            !CatalogAuthError::AuthorizationDenied("access_denied".to_owned())
                .requires_authentication()
        );
    }

    #[cfg(unix)]
    #[test]
    fn token_cache_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = env::temp_dir().join(format!(
            "spotify-tui-token-permissions-{}.json",
            std::process::id()
        ));
        let token = StoredToken {
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            expires_at: 1,
            authorization_version: AUTHORIZATION_VERSION,
        };

        save_token(&path, &token).expect("token cache should be written");
        let mode = fs::metadata(&path)
            .expect("token cache should exist")
            .permissions()
            .mode()
            & 0o777;
        let _ = fs::remove_file(&path);

        assert_eq!(mode, 0o600);
    }
}
