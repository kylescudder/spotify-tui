use std::{fmt, io, sync::mpsc, thread, thread::JoinHandle};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use url::Url;

use crate::{
    catalog_auth::{CatalogAuthError, StoredToken, load_token, refresh_token, spotify_agent},
    config::SpotifyApiConfig,
};

const API_BASE_URL: &str = "https://api.spotify.com/v1/";
const SEARCH_LIMIT: &str = "8";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogItemKind {
    Artist,
    Album,
    Track,
    Playlist,
}

impl fmt::Display for CatalogItemKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogItem {
    kind: CatalogItemKind,
    id: String,
    uri: String,
    name: String,
    detail: String,
    image_url: Option<String>,
}

impl CatalogItem {
    pub(crate) fn new(
        kind: CatalogItemKind,
        id: impl Into<String>,
        uri: impl Into<String>,
        name: impl Into<String>,
        detail: impl Into<String>,
        image_url: Option<String>,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            uri: uri.into(),
            name: name.into(),
            detail: detail.into(),
            image_url,
        }
    }

    pub const fn kind(&self) -> CatalogItemKind {
        self.kind
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn image_url(&self) -> Option<&str> {
        self.image_url.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogPage {
    Search {
        query: String,
        items: Vec<CatalogItem>,
    },
    Artist {
        artist: CatalogItem,
        releases: Vec<CatalogItem>,
    },
    Album {
        album: CatalogItem,
        tracks: Vec<CatalogItem>,
    },
}

impl CatalogPage {
    pub fn title(&self) -> String {
        match self {
            Self::Search { query, .. } => format!("Search — {query}"),
            Self::Artist { artist, .. } => artist.name.clone(),
            Self::Album { album, .. } => album.name.clone(),
        }
    }

    pub fn subtitle(&self) -> String {
        match self {
            Self::Search { items, .. } => format!("{} results", items.len()),
            Self::Artist { releases, .. } => format!("Artist • {} releases", releases.len()),
            Self::Album { album, tracks } => {
                format!("{} • {} tracks", album.detail, tracks.len())
            }
        }
    }

    pub fn items(&self) -> &[CatalogItem] {
        match self {
            Self::Search { items, .. } => items,
            Self::Artist { releases, .. } => releases,
            Self::Album { tracks, .. } => tracks,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRequest {
    Search(String),
    Artist(String),
    Album(String),
}

pub trait CatalogSource {
    fn fetch(&mut self, request: &CatalogRequest) -> Result<CatalogPage, CatalogError>;
}

struct SpotifyCatalogSource {
    config: SpotifyApiConfig,
    agent: ureq::Agent,
    token: StoredToken,
}

impl SpotifyCatalogSource {
    fn new(config: SpotifyApiConfig) -> Result<Self, CatalogError> {
        let token = load_token(&config)?;
        Ok(Self {
            config,
            agent: spotify_agent(),
            token,
        })
    }

    fn search(&mut self, query: &str) -> Result<CatalogPage, CatalogError> {
        let mut url = api_url("search")?;
        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("type", "artist,album,track,playlist")
            .append_pair("limit", SEARCH_LIMIT);
        let response: SearchResponse = self.get_json(url)?;
        Ok(normalize_search(query, response))
    }

    fn artist(&mut self, id: &str) -> Result<CatalogPage, CatalogError> {
        let artist: ArtistWire = self.get_json(api_url(&format!("artists/{id}"))?)?;
        let mut albums_url = api_url(&format!("artists/{id}/albums"))?;
        albums_url
            .query_pairs_mut()
            .append_pair("include_groups", "album,single")
            .append_pair("limit", "10");
        let albums: Page<AlbumWire> = self.get_json(albums_url)?;
        let artist = artist.into_item().ok_or_else(|| {
            CatalogError::InvalidResponse("artist has no playable URI".to_owned())
        })?;
        let mut seen = std::collections::HashSet::new();
        let releases = albums
            .items
            .into_iter()
            .filter_map(AlbumWire::into_item)
            .filter(|album| seen.insert(album.id.clone()))
            .collect();
        Ok(CatalogPage::Artist { artist, releases })
    }

    fn album(&mut self, id: &str) -> Result<CatalogPage, CatalogError> {
        let album: AlbumWire = self.get_json(api_url(&format!("albums/{id}"))?)?;
        let mut tracks_url = api_url(&format!("albums/{id}/tracks"))?;
        tracks_url.query_pairs_mut().append_pair("limit", "50");
        let tracks: Page<TrackWire> = self.get_json(tracks_url)?;
        let album = album
            .into_item()
            .ok_or_else(|| CatalogError::InvalidResponse("album has no playable URI".to_owned()))?;
        let tracks = tracks
            .items
            .into_iter()
            .filter_map(|track| track.into_item(None))
            .collect();
        Ok(CatalogPage::Album { album, tracks })
    }

    fn get_json<T: DeserializeOwned>(&mut self, url: Url) -> Result<T, CatalogError> {
        if self.token.needs_refresh() {
            self.token = refresh_token(&self.config, &self.token, &self.agent)?;
        }

        match request_json(&self.agent, &url, self.token.access_token()) {
            Err(ureq::Error::StatusCode(401)) => {
                self.token = refresh_token(&self.config, &self.token, &self.agent)?;
                request_json(&self.agent, &url, self.token.access_token())
                    .map_err(CatalogError::from_http)
            }
            result => result.map_err(CatalogError::from_http),
        }
    }
}

impl CatalogSource for SpotifyCatalogSource {
    fn fetch(&mut self, request: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
        match request {
            CatalogRequest::Search(query) => self.search(query),
            CatalogRequest::Artist(id) => self.artist(id),
            CatalogRequest::Album(id) => self.album(id),
        }
    }
}

fn request_json<T: DeserializeOwned>(
    agent: &ureq::Agent,
    url: &Url,
    access_token: &str,
) -> Result<T, ureq::Error> {
    let authorization = format!("Bearer {access_token}");
    let mut response = agent
        .get(url.as_str())
        .header("Authorization", authorization)
        .call()?;
    response.body_mut().read_json()
}

fn api_url(path: &str) -> Result<Url, CatalogError> {
    Url::parse(API_BASE_URL)
        .expect("Spotify API base URL is valid")
        .join(path)
        .map_err(|error| CatalogError::InvalidRequest(error.to_string()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogEvent {
    Loaded { request_id: u64, page: CatalogPage },
    Failed { request_id: u64, message: String },
}

enum RuntimeRequest {
    Fetch {
        request_id: u64,
        request: CatalogRequest,
    },
    Shutdown,
}

pub struct CatalogRuntime {
    request_tx: mpsc::Sender<RuntimeRequest>,
    event_rx: mpsc::Receiver<CatalogEvent>,
    worker: Option<JoinHandle<()>>,
}

impl CatalogRuntime {
    pub fn start(config: SpotifyApiConfig) -> io::Result<Self> {
        Self::start_with_factory(move || SpotifyCatalogSource::new(config))
    }

    pub fn start_with_source<S>(source: S) -> io::Result<Self>
    where
        S: CatalogSource + Send + 'static,
    {
        Self::start_with_factory(move || Ok(source))
    }

    fn start_with_factory<S, F>(factory: F) -> io::Result<Self>
    where
        S: CatalogSource + Send + 'static,
        F: FnOnce() -> Result<S, CatalogError> + Send + 'static,
    {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("spotify-tui-catalog".to_owned())
            .spawn(move || run_worker(factory(), request_rx, &event_tx))?;
        Ok(Self {
            request_tx,
            event_rx,
            worker: Some(worker),
        })
    }

    pub fn fetch(
        &self,
        request_id: u64,
        request: CatalogRequest,
    ) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::Fetch {
                request_id,
                request,
            })
            .map_err(|_| CatalogRuntimeError)
    }

    pub fn try_event(&self) -> Option<CatalogEvent> {
        self.event_rx.try_recv().ok()
    }
}

fn run_worker<S: CatalogSource>(
    mut source: Result<S, CatalogError>,
    request_rx: mpsc::Receiver<RuntimeRequest>,
    event_tx: &mpsc::Sender<CatalogEvent>,
) {
    while let Ok(mut runtime_request) = request_rx.recv() {
        while let Ok(newer) = request_rx.try_recv() {
            runtime_request = newer;
        }
        let RuntimeRequest::Fetch {
            request_id,
            request,
        } = runtime_request
        else {
            return;
        };
        let result = match source.as_mut() {
            Ok(source) => source.fetch(&request),
            Err(error) => Err(CatalogError::Unavailable(error.to_string())),
        };
        let event = match result {
            Ok(page) => CatalogEvent::Loaded { request_id, page },
            Err(error) => CatalogEvent::Failed {
                request_id,
                message: error.to_string(),
            },
        };
        if event_tx.send(event).is_err() {
            return;
        }
    }
}

impl Drop for CatalogRuntime {
    fn drop(&mut self) {
        let _ = self.request_tx.send(RuntimeRequest::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error(transparent)]
    Authentication(#[from] CatalogAuthError),
    #[error("invalid Spotify catalogue request: {0}")]
    InvalidRequest(String),
    #[error("Spotify catalogue returned invalid data: {0}")]
    InvalidResponse(String),
    #[error("Spotify catalogue access is unavailable: {0}")]
    Unavailable(String),
    #[error("Spotify catalogue access was denied; check the app allowlist and authenticate again")]
    Forbidden,
    #[error("Spotify catalogue quota was exceeded; retry later")]
    QuotaExceeded,
    #[error("Spotify catalogue request failed: {0}")]
    Request(String),
}

impl CatalogError {
    fn from_http(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(403) => Self::Forbidden,
            ureq::Error::StatusCode(429) => Self::QuotaExceeded,
            error => Self::Request(error.to_string()),
        }
    }
}

#[derive(Debug, Error)]
#[error("the catalogue runtime has stopped")]
pub struct CatalogRuntimeError;

#[derive(Debug, Deserialize)]
struct SearchResponse {
    artists: Option<Page<ArtistWire>>,
    albums: Option<Page<AlbumWire>>,
    tracks: Option<Page<TrackWire>>,
    playlists: Option<Page<Option<PlaylistWire>>>,
}

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct Page<T> {
    #[serde(default)]
    items: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct ImageWire {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ArtistWire {
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    #[serde(default)]
    images: Vec<ImageWire>,
}

impl ArtistWire {
    fn into_item(self) -> Option<CatalogItem> {
        Some(CatalogItem::new(
            CatalogItemKind::Artist,
            self.id?,
            self.uri?,
            self.name?,
            "Artist",
            self.images.into_iter().next().map(|image| image.url),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct AlbumWire {
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    release_date: Option<String>,
    #[serde(default)]
    artists: Vec<NamedWire>,
    #[serde(default)]
    images: Vec<ImageWire>,
}

impl AlbumWire {
    fn into_item(self) -> Option<CatalogItem> {
        let mut details = Vec::new();
        let artists = names(&self.artists);
        if !artists.is_empty() {
            details.push(artists);
        }
        if let Some(year) = self.release_date.as_deref().and_then(|date| date.get(..4)) {
            details.push(year.to_owned());
        }
        Some(CatalogItem::new(
            CatalogItemKind::Album,
            self.id?,
            self.uri?,
            self.name?,
            details.join(" • "),
            self.images.into_iter().next().map(|image| image.url),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct TrackWire {
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    duration_ms: Option<u64>,
    #[serde(default)]
    artists: Vec<NamedWire>,
    album: Option<NamedWire>,
}

impl TrackWire {
    fn into_item(self, fallback_album: Option<&str>) -> Option<CatalogItem> {
        let mut details = Vec::new();
        let artists = names(&self.artists);
        if !artists.is_empty() {
            details.push(artists);
        }
        if let Some(album) = self
            .album
            .and_then(|album| album.name)
            .or_else(|| fallback_album.map(str::to_owned))
        {
            details.push(album);
        }
        if let Some(duration) = self.duration_ms {
            details.push(format_duration_ms(duration));
        }
        Some(CatalogItem::new(
            CatalogItemKind::Track,
            self.id?,
            self.uri?,
            self.name?,
            details.join(" • "),
            None,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct PlaylistWire {
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    #[serde(default)]
    images: Vec<ImageWire>,
}

impl PlaylistWire {
    fn into_item(self) -> Option<CatalogItem> {
        Some(CatalogItem::new(
            CatalogItemKind::Playlist,
            self.id?,
            self.uri?,
            self.name?,
            "Playlist",
            self.images.into_iter().next().map(|image| image.url),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct NamedWire {
    name: Option<String>,
}

fn normalize_search(query: &str, response: SearchResponse) -> CatalogPage {
    let mut items = Vec::new();
    items.extend(
        response
            .artists
            .into_iter()
            .flat_map(|page| page.items)
            .filter_map(ArtistWire::into_item),
    );
    items.extend(
        response
            .albums
            .into_iter()
            .flat_map(|page| page.items)
            .filter_map(AlbumWire::into_item),
    );
    items.extend(
        response
            .tracks
            .into_iter()
            .flat_map(|page| page.items)
            .filter_map(|track| track.into_item(None)),
    );
    items.extend(
        response
            .playlists
            .into_iter()
            .flat_map(|page| page.items)
            .flatten()
            .filter_map(PlaylistWire::into_item),
    );
    CatalogPage::Search {
        query: query.to_owned(),
        items,
    }
}

fn names(values: &[NamedWire]) -> String {
    values
        .iter()
        .filter_map(|value| value.name.as_deref())
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_duration_ms(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn search_response_is_normalized_for_navigation_and_playback() {
        let response: SearchResponse = serde_json::from_str(
            r#"{
                "artists": {"items": [{"id":"artist-1","uri":"spotify:artist:artist-1","name":"Enter Shikari","images":[]}]},
                "albums": {"items": [{"id":"album-1","uri":"spotify:album:album-1","name":"A Kiss for the Whole World","release_date":"2023-04-21","artists":[{"name":"Enter Shikari"}],"images":[]}]},
                "tracks": {"items": [{"id":"track-1","uri":"spotify:track:track-1","name":"Sorry You're Not a Winner","duration_ms":228000,"artists":[{"name":"Enter Shikari"}],"album":{"name":"Take to the Skies"}}]},
                "playlists": {"items": [null]}
            }"#,
        )
        .expect("fixture should parse");

        let page = normalize_search("enter shikari", response);
        assert_eq!(page.items().len(), 3);
        assert_eq!(page.items()[0].kind(), CatalogItemKind::Artist);
        assert_eq!(page.items()[1].kind(), CatalogItemKind::Album);
        assert_eq!(page.items()[2].uri(), "spotify:track:track-1");
        assert!(page.items()[2].detail().contains("3:48"));
    }

    struct FakeSource;

    impl CatalogSource for FakeSource {
        fn fetch(&mut self, request: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
            let CatalogRequest::Search(query) = request else {
                return Err(CatalogError::InvalidRequest(
                    "unexpected request".to_owned(),
                ));
            };
            Ok(CatalogPage::Search {
                query: query.clone(),
                items: Vec::new(),
            })
        }
    }

    #[test]
    fn runtime_fetches_catalogue_off_the_ui_thread() {
        let runtime = CatalogRuntime::start_with_source(FakeSource).expect("runtime should start");
        runtime
            .fetch(7, CatalogRequest::Search("shikari".to_owned()))
            .expect("request should be accepted");
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(event) = runtime.try_event() {
                assert!(matches!(
                    event,
                    CatalogEvent::Loaded {
                        request_id: 7,
                        page: CatalogPage::Search { query, .. }
                    } if query == "shikari"
                ));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for catalogue event"
            );
            thread::yield_now();
        }
    }
}
