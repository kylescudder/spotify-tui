use std::{
    collections::HashSet,
    fmt, io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    thread::JoinHandle,
};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::{
    catalog_auth::{
        CatalogAuthError, StoredToken, authenticate_from_tui, home_cache_path, load_token,
        refresh_token, spotify_agent,
    },
    config::{HomeConfig, SpotifyApiConfig, TopArtistsRange},
    home::{HomeData, HomeRelease, HomeStep, HomeUpdate, HomeWorker, parse_release_day, unix_now},
    spotifyd_device,
};

const API_BASE_URL: &str = "https://api.spotify.com/v1/";
const SEARCH_LIMIT: &str = "8";
const RECENTLY_PLAYED_FETCH_LIMIT: &str = "50";
const RECENTLY_PLAYED_SECTION_LIMIT: usize = 10;
const PLAYLISTS_LIMIT: &str = "20";
const TOP_ARTISTS_LIMIT: &str = "10";
const FOLLOWED_ARTISTS_PAGE_LIMIT: &str = "50";
const RELEASES_PER_GROUP: &str = "3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogItem {
    kind: CatalogItemKind,
    id: String,
    uri: String,
    name: String,
    detail: String,
    image_url: Option<String>,
    playback_context_uri: Option<String>,
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
            playback_context_uri: None,
        }
    }

    fn with_playback_context(mut self, context_uri: impl Into<String>) -> Self {
        self.playback_context_uri = Some(context_uri.into());
        self
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

    pub fn playback(&self) -> CatalogPlayback {
        let context_uri = match self.kind {
            CatalogItemKind::Playlist => Some(self.uri.clone()),
            _ => self.playback_context_uri.clone(),
        };
        CatalogPlayback {
            uri: self.uri.clone(),
            context_uri,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPlayback {
    uri: String,
    context_uri: Option<String>,
}

impl CatalogPlayback {
    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn context_uri(&self) -> Option<&str> {
        self.context_uri.as_deref()
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

    pub fn artwork_url(&self, selected: usize) -> Option<&str> {
        match self {
            Self::Search { items, .. } => items.get(selected).and_then(CatalogItem::image_url),
            Self::Artist { artist, releases } => releases
                .get(selected)
                .and_then(CatalogItem::image_url)
                .or_else(|| artist.image_url()),
            Self::Album { album, .. } => album.image_url(),
        }
    }

    pub fn prefetch_artwork_urls(&self, selected: usize) -> Vec<&str> {
        let items = match self {
            Self::Search { items, .. } => items,
            Self::Artist { releases, .. } => releases,
            Self::Album { .. } => return Vec::new(),
        };
        items
            .iter()
            .skip(selected.saturating_add(1))
            .chain(items[..selected.min(items.len())].iter().rev())
            .filter_map(CatalogItem::image_url)
            .collect()
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

    fn play(&mut self, _playback: &CatalogPlayback) -> Result<(), CatalogError> {
        Err(CatalogError::Unavailable(
            "catalogue playback is unavailable".to_owned(),
        ))
    }

    fn home_step(&mut self, _step: &HomeStep) -> Result<HomeData, CatalogError> {
        Err(CatalogError::Unavailable(
            "the catalogue home is unavailable".to_owned(),
        ))
    }
}

struct SpotifyCatalogSource {
    config: SpotifyApiConfig,
    agent: ureq::Agent,
    token: StoredToken,
    cancelled: Arc<AtomicBool>,
}

impl SpotifyCatalogSource {
    fn new_interactive(
        config: SpotifyApiConfig,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, CatalogError> {
        let token = match load_token(&config) {
            Ok(token) => token,
            Err(error) if error.requires_authentication() => {
                authenticate_from_tui(&config, &cancelled)?;
                load_token(&config)?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            config,
            agent: spotify_agent(),
            token,
            cancelled,
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
        let mut seen = HashSet::new();
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
        let album_context = (album.name().to_owned(), album.uri().to_owned());
        let tracks = tracks
            .items
            .into_iter()
            .filter_map(|track| track.into_item(Some(&album_context)))
            .collect();
        Ok(CatalogPage::Album { album, tracks })
    }

    fn recently_played(&mut self) -> Result<HomeData, CatalogError> {
        let mut url = api_url("me/player/recently-played")?;
        url.query_pairs_mut()
            .append_pair("limit", RECENTLY_PLAYED_FETCH_LIMIT);
        let response: Page<PlayHistoryWire> = self.get_json(url)?;
        Ok(HomeData::Items(normalize_recently_played(response)))
    }

    fn playlists(&mut self) -> Result<HomeData, CatalogError> {
        let mut url = api_url("me/playlists")?;
        url.query_pairs_mut().append_pair("limit", PLAYLISTS_LIMIT);
        let response: Page<Option<PlaylistWire>> = self.get_json(url)?;
        Ok(HomeData::Items(
            response
                .items
                .into_iter()
                .flatten()
                .filter_map(PlaylistWire::into_item)
                .collect(),
        ))
    }

    fn top_artists(&mut self, range: TopArtistsRange) -> Result<HomeData, CatalogError> {
        let mut url = api_url("me/top/artists")?;
        url.query_pairs_mut()
            .append_pair("limit", TOP_ARTISTS_LIMIT)
            .append_pair("time_range", range.as_query());
        let response: Page<ArtistWire> = self.get_json(url)?;
        Ok(HomeData::Items(
            response
                .items
                .into_iter()
                .filter_map(ArtistWire::into_item)
                .collect(),
        ))
    }

    fn followed_artists(&mut self, after: Option<&str>) -> Result<HomeData, CatalogError> {
        let mut url = api_url("me/following")?;
        url.query_pairs_mut()
            .append_pair("type", "artist")
            .append_pair("limit", FOLLOWED_ARTISTS_PAGE_LIMIT);
        if let Some(after) = after {
            url.query_pairs_mut().append_pair("after", after);
        }
        let response: FollowedArtistsResponse = self.get_json(url)?;
        Ok(normalize_followed_artists(response))
    }

    // Spotify groups a combined album,single response by type, so a small limit
    // would hide an artist's new single behind their older albums.
    fn artist_releases(&mut self, id: &str) -> Result<HomeData, CatalogError> {
        let mut releases = Vec::new();
        for group in ["album", "single"] {
            let mut url = api_url(&format!("artists/{id}/albums"))?;
            url.query_pairs_mut()
                .append_pair("include_groups", group)
                .append_pair("limit", RELEASES_PER_GROUP);
            let albums: Page<AlbumWire> = self.get_json(url)?;
            releases.extend(albums.items.into_iter().filter_map(AlbumWire::into_release));
        }
        Ok(HomeData::Releases(releases))
    }

    fn play(&mut self, playback: &CatalogPlayback) -> Result<(), CatalogError> {
        if self.token.needs_refresh() {
            self.refresh_or_authenticate()?;
        }

        play_on_spotifyd(&spotifyd_device::device_id(), |device_id| {
            self.request_play(playback, device_id)
        })
    }

    fn request_play(
        &mut self,
        playback: &CatalogPlayback,
        device_id: Option<&str>,
    ) -> Result<(), CatalogError> {
        match request_playback(&self.agent, playback, device_id, self.token.access_token()) {
            Err(ureq::Error::StatusCode(401)) => {
                self.refresh_or_authenticate()?;
                request_playback(&self.agent, playback, device_id, self.token.access_token())
                    .map_err(CatalogError::from_playback_http)?;
            }
            Err(ureq::Error::StatusCode(403)) => {
                self.authenticate()?;
                request_playback(&self.agent, playback, device_id, self.token.access_token())
                    .map_err(CatalogError::from_playback_http)?;
            }
            result => {
                result.map_err(CatalogError::from_playback_http)?;
            }
        }
        Ok(())
    }

    fn get_json<T: DeserializeOwned>(&mut self, url: Url) -> Result<T, CatalogError> {
        if self.token.needs_refresh() {
            self.refresh_or_authenticate()?;
        }

        match request_json(&self.agent, &url, self.token.access_token()) {
            Err(ureq::Error::StatusCode(401)) => {
                self.refresh_or_authenticate()?;
                request_json(&self.agent, &url, self.token.access_token())
                    .map_err(CatalogError::from_http)
            }
            result => result.map_err(CatalogError::from_http),
        }
    }

    fn refresh_or_authenticate(&mut self) -> Result<(), CatalogError> {
        self.token = match refresh_token(&self.config, &self.token, &self.agent) {
            Ok(token) => token,
            Err(error) if error.requires_authentication() => {
                authenticate_from_tui(&self.config, &self.cancelled)?;
                load_token(&self.config)?
            }
            Err(error) => return Err(error.into()),
        };
        Ok(())
    }

    fn authenticate(&mut self) -> Result<(), CatalogError> {
        authenticate_from_tui(&self.config, &self.cancelled)?;
        self.token = load_token(&self.config)?;
        Ok(())
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

    fn play(&mut self, playback: &CatalogPlayback) -> Result<(), CatalogError> {
        SpotifyCatalogSource::play(self, playback)
    }

    fn home_step(&mut self, step: &HomeStep) -> Result<HomeData, CatalogError> {
        match step {
            HomeStep::RecentlyPlayed => self.recently_played(),
            HomeStep::Playlists => self.playlists(),
            HomeStep::TopArtists(range) => self.top_artists(*range),
            HomeStep::FollowedArtists { after } => self.followed_artists(after.as_deref()),
            HomeStep::ArtistReleases(id) => self.artist_releases(id),
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

// Without a device ID Spotify only reaches a device it already considers active.
// Local activation does not make Spotifyd one, but naming it works even while
// idle. A missing device falls back to whichever one is active, because the ID
// is derived locally and may not match a Spotifyd configured elsewhere.
fn play_on_spotifyd(
    device_id: &str,
    mut request: impl FnMut(Option<&str>) -> Result<(), CatalogError>,
) -> Result<(), CatalogError> {
    match request(Some(device_id)) {
        Err(CatalogError::PlaybackUnavailable) => request(None),
        result => result,
    }
}

fn request_playback(
    agent: &ureq::Agent,
    playback: &CatalogPlayback,
    device_id: Option<&str>,
    access_token: &str,
) -> Result<(), ureq::Error> {
    let authorization = format!("Bearer {access_token}");
    let body = playback_body(playback);
    let mut request = agent
        .put("https://api.spotify.com/v1/me/player/play")
        .header("Authorization", authorization);
    if let Some(device_id) = device_id {
        request = request.query("device_id", device_id);
    }
    request.send_json(&body)?;
    Ok(())
}

fn playback_body(playback: &CatalogPlayback) -> serde_json::Value {
    match playback.context_uri() {
        None => serde_json::json!({ "uris": [playback.uri()] }),
        Some(context_uri) if context_uri == playback.uri() => {
            serde_json::json!({ "context_uri": context_uri, "position_ms": 0 })
        }
        Some(context_uri) => serde_json::json!({
            "context_uri": context_uri,
            "offset": { "uri": playback.uri() },
            "position_ms": 0
        }),
    }
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
    PlaybackFailed { message: String },
    PlaybackFallback { playback: CatalogPlayback },
    HomeUpdated(HomeUpdate),
}

enum RuntimeRequest {
    Fetch {
        request_id: u64,
        request: CatalogRequest,
    },
    Play(CatalogPlayback),
    LoadHome {
        force: bool,
    },
    Shutdown,
}

pub struct CatalogRuntime {
    request_tx: mpsc::Sender<RuntimeRequest>,
    event_rx: mpsc::Receiver<CatalogEvent>,
    worker: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

impl CatalogRuntime {
    pub fn start(config: SpotifyApiConfig, home: HomeConfig) -> io::Result<Self> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let home = HomeWorker::new(home, home_cache_path(&config).ok());
        Self::start_with_factory_and_cancel(
            move || {
                SpotifyCatalogSource::new_interactive(config.clone(), Arc::clone(&worker_cancelled))
            },
            cancelled,
            home,
        )
    }

    pub fn start_with_source<S>(source: S) -> io::Result<Self>
    where
        S: CatalogSource + Send + 'static,
    {
        let mut source = Some(source);
        Self::start_with_factory(move || {
            source
                .take()
                .ok_or_else(|| CatalogError::Unavailable("source cannot be restarted".to_owned()))
        })
    }

    fn start_with_factory<S, F>(factory: F) -> io::Result<Self>
    where
        S: CatalogSource + Send + 'static,
        F: FnMut() -> Result<S, CatalogError> + Send + 'static,
    {
        Self::start_with_factory_and_cancel(
            factory,
            Arc::new(AtomicBool::new(false)),
            HomeWorker::new(HomeConfig::default(), None),
        )
    }

    fn start_with_factory_and_cancel<S, F>(
        factory: F,
        cancelled: Arc<AtomicBool>,
        home: HomeWorker,
    ) -> io::Result<Self>
    where
        S: CatalogSource + Send + 'static,
        F: FnMut() -> Result<S, CatalogError> + Send + 'static,
    {
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("spotify-tui-catalog".to_owned())
            .spawn(move || run_worker(factory, home, request_rx, &event_tx))?;
        Ok(Self {
            request_tx,
            event_rx,
            worker: Some(worker),
            cancelled,
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

    pub fn play(&self, playback: CatalogPlayback) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::Play(playback))
            .map_err(|_| CatalogRuntimeError)
    }

    pub fn load_home(&self, force: bool) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::LoadHome { force })
            .map_err(|_| CatalogRuntimeError)
    }
}

fn run_worker<S: CatalogSource, F: FnMut() -> Result<S, CatalogError>>(
    mut factory: F,
    mut home: HomeWorker,
    request_rx: mpsc::Receiver<RuntimeRequest>,
    event_tx: &mpsc::Sender<CatalogEvent>,
) {
    let mut source: Option<S> = None;
    loop {
        // Home loads one step at a time so user requests are never kept waiting
        // for a long release scan. They share this worker, rather than having
        // their own, because a second token owner could invalidate this one.
        let received = if home.is_loading() {
            request_rx.try_recv()
        } else {
            request_rx.recv().map_err(mpsc::TryRecvError::from)
        };
        let mut runtime_request = match received {
            Ok(request) => request,
            Err(mpsc::TryRecvError::Empty) => {
                if !advance_home(&mut home, &mut source, &mut factory, event_tx) {
                    return;
                }
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
        };
        // A newer request only ever replaces a fetch nobody is waiting for. The
        // worker is busy throughout a release scan, so plays and home loads queue
        // up behind it and must survive whatever the user does next.
        while let Ok(newer) = request_rx.try_recv() {
            let keep_running = match std::mem::replace(&mut runtime_request, newer) {
                RuntimeRequest::Fetch { .. } => true,
                RuntimeRequest::Play(playback) => {
                    play_selection(playback, &mut source, &mut factory, event_tx)
                }
                RuntimeRequest::LoadHome { force } => begin_home(&mut home, force, event_tx),
                RuntimeRequest::Shutdown => false,
            };
            if !keep_running {
                return;
            }
        }
        let (request_id, request) = match runtime_request {
            RuntimeRequest::Fetch {
                request_id,
                request,
            } => (request_id, request),
            RuntimeRequest::Play(playback) => {
                if !play_selection(playback, &mut source, &mut factory, event_tx) {
                    return;
                }
                continue;
            }
            RuntimeRequest::LoadHome { force } => {
                if !begin_home(&mut home, force, event_tx) {
                    return;
                }
                continue;
            }
            RuntimeRequest::Shutdown => return,
        };
        let result =
            ensure_source(&mut source, &mut factory).and_then(|source| source.fetch(&request));
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

fn ensure_source<'a, S, F>(
    source: &'a mut Option<S>,
    factory: &mut F,
) -> Result<&'a mut S, CatalogError>
where
    F: FnMut() -> Result<S, CatalogError>,
{
    if source.is_none() {
        *source = Some(factory()?);
    }
    Ok(source.as_mut().expect("the source was just created"))
}

fn play_selection<S, F>(
    playback: CatalogPlayback,
    source: &mut Option<S>,
    factory: &mut F,
    event_tx: &mpsc::Sender<CatalogEvent>,
) -> bool
where
    S: CatalogSource,
    F: FnMut() -> Result<S, CatalogError>,
{
    // A cached home can offer playback before any request has created the source.
    let result = ensure_source(source, factory).and_then(|source| source.play(&playback));
    let event = match result {
        Ok(()) => None,
        Err(CatalogError::PlaybackUnavailable) => Some(CatalogEvent::PlaybackFallback { playback }),
        Err(error) => Some(CatalogEvent::PlaybackFailed {
            message: error.to_string(),
        }),
    };
    event.is_none_or(|event| event_tx.send(event).is_ok())
}

fn begin_home(home: &mut HomeWorker, force: bool, event_tx: &mpsc::Sender<CatalogEvent>) -> bool {
    send_home_updates(home.begin(force, unix_now()), event_tx)
}

fn advance_home<S, F>(
    home: &mut HomeWorker,
    source: &mut Option<S>,
    factory: &mut F,
    event_tx: &mpsc::Sender<CatalogEvent>,
) -> bool
where
    S: CatalogSource,
    F: FnMut() -> Result<S, CatalogError>,
{
    let Some(step) = home.next_step().cloned() else {
        return true;
    };
    let updates = match ensure_source(source, factory) {
        Ok(source) => home
            .advance(source.home_step(&step), unix_now())
            .into_iter()
            .collect(),
        Err(error) => home.abort(&error.to_string(), unix_now()),
    };
    send_home_updates(updates, event_tx)
}

fn send_home_updates(updates: Vec<HomeUpdate>, event_tx: &mpsc::Sender<CatalogEvent>) -> bool {
    updates
        .into_iter()
        .all(|update| event_tx.send(CatalogEvent::HomeUpdated(update)).is_ok())
}

impl Drop for CatalogRuntime {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
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
    #[error("Spotify Web API could not find an active playback device")]
    PlaybackUnavailable,
}

impl CatalogError {
    fn from_http(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(403) => Self::Forbidden,
            ureq::Error::StatusCode(429) => Self::QuotaExceeded,
            error => Self::Request(error.to_string()),
        }
    }

    fn from_playback_http(error: ureq::Error) -> Self {
        match error {
            ureq::Error::StatusCode(404) => Self::PlaybackUnavailable,
            error => Self::from_http(error),
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
        let year = self
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned);
        self.into_dated_item(year)
    }

    fn into_release(self) -> Option<HomeRelease> {
        let date = self.release_date.clone()?;
        let released_on = parse_release_day(&date)?;
        Some(HomeRelease::new(
            released_on,
            self.into_dated_item(Some(date))?,
        ))
    }

    fn into_dated_item(self, date: Option<String>) -> Option<CatalogItem> {
        let mut details = Vec::new();
        let artists = names(&self.artists);
        if !artists.is_empty() {
            details.push(artists);
        }
        details.extend(date);
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
    album: Option<TrackAlbumWire>,
}

impl TrackWire {
    fn into_item(self, fallback_album: Option<&(String, String)>) -> Option<CatalogItem> {
        let mut details = Vec::new();
        let artists = names(&self.artists);
        if !artists.is_empty() {
            details.push(artists);
        }
        let (album_name, context_uri, image_url) = match self.album {
            Some(album) => (
                album.name,
                album.uri,
                album.images.into_iter().next().map(|image| image.url),
            ),
            None => (
                fallback_album.map(|(name, _)| name.clone()),
                fallback_album.map(|(_, uri)| uri.clone()),
                None,
            ),
        };
        if let Some(album_name) = album_name {
            details.push(album_name);
        }
        if let Some(duration) = self.duration_ms {
            details.push(format_duration_ms(duration));
        }
        let item = CatalogItem::new(
            CatalogItemKind::Track,
            self.id?,
            self.uri?,
            self.name?,
            details.join(" • "),
            image_url,
        );
        Some(match context_uri {
            Some(uri) => item.with_playback_context(uri),
            None => item,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TrackAlbumWire {
    name: Option<String>,
    uri: Option<String>,
    #[serde(default)]
    images: Vec<ImageWire>,
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

#[derive(Debug, Deserialize)]
struct PlayHistoryWire {
    track: Option<TrackWire>,
}

#[derive(Debug, Deserialize)]
struct FollowedArtistsResponse {
    artists: FollowedArtistsPage,
}

#[derive(Debug, Deserialize)]
struct FollowedArtistsPage {
    #[serde(default)]
    items: Vec<ArtistWire>,
    next: Option<String>,
    cursors: Option<CursorsWire>,
}

#[derive(Debug, Deserialize)]
struct CursorsWire {
    after: Option<String>,
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

fn normalize_recently_played(response: Page<PlayHistoryWire>) -> Vec<CatalogItem> {
    let mut seen = HashSet::new();
    response
        .items
        .into_iter()
        .filter_map(|entry| entry.track?.into_item(None))
        .filter(|track| seen.insert(track.id.clone()))
        .take(RECENTLY_PLAYED_SECTION_LIMIT)
        .collect()
}

fn normalize_followed_artists(response: FollowedArtistsResponse) -> HomeData {
    let FollowedArtistsPage {
        items,
        next,
        cursors,
    } = response.artists;
    HomeData::FollowedArtists {
        artist_ids: items.into_iter().filter_map(|artist| artist.id).collect(),
        after: next.and(cursors).and_then(|cursors| cursors.after),
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
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn search_response_is_normalized_for_navigation_and_playback() {
        let response: SearchResponse = serde_json::from_str(
            r#"{
                "artists": {"items": [{"id":"artist-1","uri":"spotify:artist:artist-1","name":"Enter Shikari","images":[]}]},
                "albums": {"items": [{"id":"album-1","uri":"spotify:album:album-1","name":"A Kiss for the Whole World","release_date":"2023-04-21","artists":[{"name":"Enter Shikari"}],"images":[]}]},
                "tracks": {"items": [{"id":"track-1","uri":"spotify:track:track-1","name":"Sorry You're Not a Winner","duration_ms":228000,"artists":[{"name":"Enter Shikari"}],"album":{"name":"Take to the Skies","uri":"spotify:album:album-2","images":[{"url":"https://example.com/take-to-the-skies.jpg"}]}}]},
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
        assert_eq!(
            page.items()[2].image_url(),
            Some("https://example.com/take-to-the-skies.jpg")
        );
        assert_eq!(
            page.items()[2].playback().context_uri(),
            Some("spotify:album:album-2")
        );
    }

    #[test]
    fn album_track_playback_uses_the_exact_track_as_a_zero_free_uri_offset() {
        let playback = CatalogItem::new(
            CatalogItemKind::Track,
            "track-2",
            "spotify:track:track-2",
            "Second track",
            "Artist • Album • 3:20",
            None,
        )
        .with_playback_context("spotify:album:album-1")
        .playback();

        assert_eq!(
            playback_body(&playback),
            serde_json::json!({
                "context_uri": "spotify:album:album-1",
                "offset": { "uri": "spotify:track:track-2" },
                "position_ms": 0
            })
        );
    }

    #[test]
    fn artist_page_uses_the_selected_release_as_its_primary_artwork() {
        let artist = CatalogItem::new(
            CatalogItemKind::Artist,
            "artist",
            "spotify:artist:artist",
            "Architects",
            "Artist",
            Some("https://example.com/architects.jpg".to_owned()),
        );
        let first_album = CatalogItem::new(
            CatalogItemKind::Album,
            "first",
            "spotify:album:first",
            "First album",
            "Architects • 2025",
            Some("https://example.com/first.jpg".to_owned()),
        );
        let selected_album = CatalogItem::new(
            CatalogItemKind::Album,
            "selected",
            "spotify:album:selected",
            "Selected album",
            "Architects • 2022",
            Some("https://example.com/selected.jpg".to_owned()),
        );
        let page = CatalogPage::Artist {
            artist,
            releases: vec![first_album, selected_album],
        };

        assert_eq!(
            page.artwork_url(1),
            Some("https://example.com/selected.jpg")
        );
        assert_eq!(
            page.prefetch_artwork_urls(1),
            vec!["https://example.com/first.jpg"]
        );
    }

    #[test]
    fn recently_played_keeps_album_context_and_each_track_once() {
        let response: Page<PlayHistoryWire> = serde_json::from_str(
            r#"{
                "items": [
                    {"track": {"id":"track-1","uri":"spotify:track:track-1","name":"Solar","duration_ms":200000,"artists":[{"name":"Architects"}],"album":{"name":"The Sky","uri":"spotify:album:sky","images":[{"url":"https://example.com/sky.jpg"}]}},"played_at":"2026-10-31T10:00:00Z"},
                    {"track": {"id":"track-1","uri":"spotify:track:track-1","name":"Solar","duration_ms":200000,"artists":[{"name":"Architects"}],"album":{"name":"The Sky","uri":"spotify:album:sky","images":[]}},"played_at":"2026-10-31T09:00:00Z"},
                    {"track": null, "played_at":"2026-10-31T08:00:00Z"}
                ]
            }"#,
        )
        .expect("fixture should parse");

        let items = normalize_recently_played(response);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind(), CatalogItemKind::Track);
        assert_eq!(items[0].image_url(), Some("https://example.com/sky.jpg"));
        assert_eq!(items[0].playback().context_uri(), Some("spotify:album:sky"));
    }

    #[test]
    fn followed_artists_expose_a_cursor_only_while_pages_remain() {
        let parse = |json: &str| {
            normalize_followed_artists(serde_json::from_str(json).expect("fixture should parse"))
        };

        assert_eq!(
            parse(
                r#"{"artists":{"items":[{"id":"a","uri":"spotify:artist:a","name":"A"}],"next":"https://api.spotify.com/v1/me/following?after=a","cursors":{"after":"a"}}}"#
            ),
            HomeData::FollowedArtists {
                artist_ids: vec!["a".to_owned()],
                after: Some("a".to_owned()),
            }
        );
        assert_eq!(
            parse(
                r#"{"artists":{"items":[{"id":"b","uri":"spotify:artist:b","name":"B"}],"next":null,"cursors":{"after":"b"}}}"#
            ),
            HomeData::FollowedArtists {
                artist_ids: vec!["b".to_owned()],
                after: None,
            }
        );
    }

    #[test]
    fn releases_show_their_full_date_and_need_day_precision() {
        let album = |date: &str| -> AlbumWire {
            serde_json::from_str(&format!(
                r#"{{"id":"album","uri":"spotify:album:album","name":"New","release_date":"{date}","artists":[{{"name":"Architects"}}],"images":[]}}"#
            ))
            .expect("fixture should parse")
        };

        assert_eq!(
            album("2026-10-30").into_release(),
            Some(HomeRelease::new(
                20_756,
                CatalogItem::new(
                    CatalogItemKind::Album,
                    "album",
                    "spotify:album:album",
                    "New",
                    "Architects • 2026-10-30",
                    None,
                )
            ))
        );
        assert_eq!(album("2026-10").into_release(), None);
        assert_eq!(album("2026").into_release(), None);
        assert_eq!(
            album("2026-10-30")
                .into_item()
                .expect("album should normalize")
                .detail(),
            "Architects • 2026"
        );
    }

    /// Answers each home step from the test thread, so a test can hold a load
    /// open while it sends the worker user requests.
    struct SteppedHomeSource {
        steps: mpsc::Sender<HomeStep>,
        answers: mpsc::Receiver<Result<HomeData, CatalogError>>,
    }

    impl CatalogSource for SteppedHomeSource {
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

        fn home_step(&mut self, step: &HomeStep) -> Result<HomeData, CatalogError> {
            self.steps
                .send(step.clone())
                .map_err(|error| CatalogError::Unavailable(error.to_string()))?;
            self.answers
                .recv()
                .map_err(|error| CatalogError::Unavailable(error.to_string()))?
        }
    }

    struct PlayingHomeSource {
        home: SteppedHomeSource,
        plays: mpsc::Sender<CatalogPlayback>,
    }

    impl CatalogSource for PlayingHomeSource {
        fn fetch(&mut self, request: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
            self.home.fetch(request)
        }

        fn play(&mut self, playback: &CatalogPlayback) -> Result<(), CatalogError> {
            self.plays
                .send(playback.clone())
                .map_err(|error| CatalogError::Unavailable(error.to_string()))
        }

        fn home_step(&mut self, step: &HomeStep) -> Result<HomeData, CatalogError> {
            self.home.home_step(step)
        }
    }

    fn stepped_runtime() -> (
        CatalogRuntime,
        mpsc::Receiver<HomeStep>,
        mpsc::Sender<Result<HomeData, CatalogError>>,
    ) {
        let (step_tx, step_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let runtime = CatalogRuntime::start_with_source(SteppedHomeSource {
            steps: step_tx,
            answers: answer_rx,
        })
        .expect("runtime should start");
        (runtime, step_rx, answer_tx)
    }

    #[test]
    fn user_requests_run_between_home_steps() {
        let (runtime, steps, answers) = stepped_runtime();
        runtime.load_home(false).expect("load should be accepted");
        assert_eq!(
            steps.recv_timeout(Duration::from_secs(1)),
            Ok(HomeStep::RecentlyPlayed)
        );

        runtime
            .fetch(1, CatalogRequest::Search("shikari".to_owned()))
            .expect("search should be accepted");
        answers
            .send(Ok(HomeData::Items(Vec::new())))
            .expect("worker should be waiting");

        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::HomeUpdated(_)
        ));
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Loaded { request_id: 1, .. }
        ));
        assert_eq!(
            steps.recv_timeout(Duration::from_secs(1)),
            Ok(HomeStep::Playlists)
        );
        answers
            .send(Err(CatalogError::QuotaExceeded))
            .expect("worker should be waiting");
    }

    #[test]
    fn a_home_load_is_not_dropped_by_a_request_queued_behind_it() {
        let (runtime, steps, answers) = stepped_runtime();
        runtime
            .fetch(1, CatalogRequest::Search("first".to_owned()))
            .expect("first search should be accepted");
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Loaded { request_id: 1, .. }
        ));

        runtime.load_home(false).expect("load should be accepted");
        runtime
            .fetch(2, CatalogRequest::Search("second".to_owned()))
            .expect("second search should be accepted");

        assert_eq!(
            steps.recv_timeout(Duration::from_secs(1)),
            Ok(HomeStep::RecentlyPlayed)
        );
        answers
            .send(Err(CatalogError::QuotaExceeded))
            .expect("worker should be waiting");
    }

    #[test]
    fn a_cached_home_can_play_before_any_request_created_the_source() {
        let (play_tx, play_rx) = mpsc::channel();
        let runtime = CatalogRuntime::start_with_source(RecordingSource { plays: play_tx })
            .expect("runtime should start");
        let playback = CatalogItem::new(
            CatalogItemKind::Track,
            "track",
            "spotify:track:track",
            "Track",
            "Artist • Album",
            None,
        )
        .playback();

        runtime
            .play(playback.clone())
            .expect("playback request should be accepted");

        assert_eq!(play_rx.recv_timeout(Duration::from_secs(1)), Ok(playback));
    }

    #[test]
    fn a_play_queued_behind_a_home_step_survives_the_next_request() {
        let (step_tx, step_rx) = mpsc::channel();
        let (answer_tx, answer_rx) = mpsc::channel();
        let (play_tx, play_rx) = mpsc::channel();
        let runtime = CatalogRuntime::start_with_source(PlayingHomeSource {
            home: SteppedHomeSource {
                steps: step_tx,
                answers: answer_rx,
            },
            plays: play_tx,
        })
        .expect("runtime should start");
        runtime.load_home(false).expect("load should be accepted");
        step_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the worker should be inside a home step");
        let playback = CatalogItem::new(
            CatalogItemKind::Track,
            "track",
            "spotify:track:track",
            "Track",
            "Artist • Album",
            None,
        )
        .playback();

        runtime
            .play(playback.clone())
            .expect("playback request should be accepted");
        runtime
            .fetch(1, CatalogRequest::Search("next".to_owned()))
            .expect("search should be accepted");
        answer_tx
            .send(Ok(HomeData::Items(Vec::new())))
            .expect("worker should be waiting");

        // Release the worker before asserting, because dropping the runtime joins it.
        let played = play_rx.recv_timeout(Duration::from_secs(1));
        drop(answer_tx);
        assert_eq!(played, Ok(playback));
    }

    #[test]
    fn a_source_that_cannot_start_fails_every_home_section_once() {
        let runtime = CatalogRuntime::start_with_factory(|| -> Result<FakeSource, CatalogError> {
            Err(CatalogError::Unavailable("sign-in required".to_owned()))
        })
        .expect("runtime should start");

        runtime.load_home(false).expect("load should be accepted");

        for _ in 0..HomeConfig::default().sections().len() {
            assert!(matches!(
                receive_catalog_event(&runtime),
                CatalogEvent::HomeUpdated(HomeUpdate {
                    status: crate::home::HomeSectionStatus::Notice(_),
                    ..
                })
            ));
        }
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

    struct RecordingSource {
        plays: mpsc::Sender<CatalogPlayback>,
    }

    impl CatalogSource for RecordingSource {
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

        fn play(&mut self, playback: &CatalogPlayback) -> Result<(), CatalogError> {
            self.plays
                .send(playback.clone())
                .map_err(|error| CatalogError::Unavailable(error.to_string()))
        }
    }

    struct MissingActiveDeviceSource;

    impl CatalogSource for MissingActiveDeviceSource {
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

        fn play(&mut self, _playback: &CatalogPlayback) -> Result<(), CatalogError> {
            Err(CatalogError::PlaybackUnavailable)
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

    #[test]
    fn runtime_forwards_one_exact_catalogue_playback_target() {
        let (play_tx, play_rx) = mpsc::channel();
        let runtime = CatalogRuntime::start_with_source(RecordingSource { plays: play_tx })
            .expect("runtime should start");
        runtime
            .fetch(1, CatalogRequest::Search("shikari".to_owned()))
            .expect("setup request should be accepted");
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Loaded { request_id: 1, .. }
        ));
        let playback = CatalogItem::new(
            CatalogItemKind::Track,
            "second",
            "spotify:track:second",
            "Second",
            "Artist • Album",
            None,
        )
        .with_playback_context("spotify:album:album")
        .playback();

        runtime
            .play(playback.clone())
            .expect("playback request should be accepted");

        assert_eq!(play_rx.recv_timeout(Duration::from_secs(1)), Ok(playback));
    }

    #[test]
    fn selections_target_spotifyd_directly() {
        let mut requests = Vec::new();
        let result = play_on_spotifyd("spotifyd", |device_id| {
            requests.push(device_id.map(str::to_owned));
            Ok(())
        });

        assert!(result.is_ok());
        assert_eq!(requests, vec![Some("spotifyd".to_owned())]);
    }

    #[test]
    fn a_missing_spotifyd_device_falls_back_to_the_active_device() {
        let mut requests = Vec::new();
        let result = play_on_spotifyd("spotifyd", |device_id| {
            requests.push(device_id.map(str::to_owned));
            if device_id.is_some() {
                Err(CatalogError::PlaybackUnavailable)
            } else {
                Ok(())
            }
        });

        assert!(result.is_ok());
        assert_eq!(requests, vec![Some("spotifyd".to_owned()), None]);
    }

    #[test]
    fn other_playback_failures_are_not_retried_on_another_device() {
        let mut requests = 0;
        let result = play_on_spotifyd("spotifyd", |_| {
            requests += 1;
            Err(CatalogError::QuotaExceeded)
        });

        assert!(matches!(result, Err(CatalogError::QuotaExceeded)));
        assert_eq!(requests, 1);
    }

    #[test]
    fn a_playlist_plays_as_its_own_context() {
        let playlist = CatalogItem::new(
            CatalogItemKind::Playlist,
            "mix",
            "spotify:playlist:mix",
            "Mix",
            "",
            None,
        );

        assert_eq!(
            playback_body(&playlist.playback()),
            serde_json::json!({ "context_uri": "spotify:playlist:mix", "position_ms": 0 })
        );
    }

    #[test]
    fn unavailable_web_api_device_falls_back_to_local_spotifyd_playback() {
        assert!(matches!(
            CatalogError::from_playback_http(ureq::Error::StatusCode(404)),
            CatalogError::PlaybackUnavailable
        ));

        let runtime = CatalogRuntime::start_with_source(MissingActiveDeviceSource)
            .expect("runtime should start");
        runtime
            .fetch(1, CatalogRequest::Search("shikari".to_owned()))
            .expect("setup request should be accepted");
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Loaded { request_id: 1, .. }
        ));
        let playback = CatalogItem::new(
            CatalogItemKind::Track,
            "track",
            "spotify:track:track",
            "Track",
            "Artist • Album",
            None,
        )
        .with_playback_context("spotify:album:album")
        .playback();

        runtime
            .play(playback.clone())
            .expect("playback request should be accepted");

        assert_eq!(
            receive_catalog_event(&runtime),
            CatalogEvent::PlaybackFallback { playback }
        );
    }

    #[test]
    fn catalogue_permission_and_quota_failures_remain_actionable() {
        assert!(matches!(
            CatalogError::from_http(ureq::Error::StatusCode(403)),
            CatalogError::Forbidden
        ));
        assert!(matches!(
            CatalogError::from_http(ureq::Error::StatusCode(429)),
            CatalogError::QuotaExceeded
        ));
        assert_eq!(
            CatalogError::Forbidden.to_string(),
            "Spotify catalogue access was denied; check the app allowlist and authenticate again"
        );
        assert_eq!(
            CatalogError::QuotaExceeded.to_string(),
            "Spotify catalogue quota was exceeded; retry later"
        );
    }

    #[test]
    fn runtime_lazily_retries_source_setup_without_restarting_the_tui() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&attempts);
        let runtime = CatalogRuntime::start_with_factory(move || {
            if observed_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(CatalogError::Unavailable("sign-in required".to_owned()))
            } else {
                Ok(FakeSource)
            }
        })
        .expect("runtime should start");

        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        runtime
            .fetch(1, CatalogRequest::Search("first".to_owned()))
            .expect("first request should be accepted");
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Failed { request_id: 1, .. }
        ));
        runtime
            .fetch(2, CatalogRequest::Search("second".to_owned()))
            .expect("retry should be accepted");
        assert!(matches!(
            receive_catalog_event(&runtime),
            CatalogEvent::Loaded {
                request_id: 2,
                page: CatalogPage::Search { query, .. },
            } if query == "second"
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    fn receive_catalog_event(runtime: &CatalogRuntime) -> CatalogEvent {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(event) = runtime.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "timed out waiting for event");
            thread::yield_now();
        }
    }
}
