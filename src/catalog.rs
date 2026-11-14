use crate::local_control::LocalControlPlaybackSource;
use crate::queue::{QueueEdit, QueueEntry, QueueSnapshot};
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
        following: bool,
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
            Self::Artist {
                artist, releases, ..
            } => releases
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackDestination {
    Album,
    Artist,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogRequest {
    Search(String),
    Track {
        id: String,
        destination: TrackDestination,
    },
    Artist(String),
    Album(String),
}

pub trait CatalogSource {
    fn fetch(&mut self, request: &CatalogRequest) -> Result<CatalogPage, CatalogError>;

    fn manage_queue(&mut self, _edit: Option<QueueEdit>) -> Result<QueueSnapshot, CatalogError> {
        Err(CatalogError::Unavailable(
            "Queue management is unavailable".into(),
        ))
    }

    fn play(&mut self, _playback: &CatalogPlayback) -> Result<(), CatalogError> {
        Err(CatalogError::Unavailable(
            "catalogue playback is unavailable".to_owned(),
        ))
    }

    fn queue(&mut self, _item: &CatalogItem) -> Result<usize, CatalogError> {
        Err(CatalogError::Unavailable(
            "queueing is unavailable".to_owned(),
        ))
    }

    fn follow_artist(&mut self, _id: &str, _following: bool) -> Result<(), CatalogError> {
        Err(CatalogError::Unavailable(
            "artist following is unavailable".to_owned(),
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

    fn track_destination(
        &mut self,
        id: &str,
        destination: TrackDestination,
    ) -> Result<CatalogPage, CatalogError> {
        let track: TrackWire = self.get_json(api_url(&format!("tracks/{id}"))?)?;
        let request = track.destination(destination)?;
        self.fetch(&request)
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
        let mut url = api_url("me/following/contains")?;
        url.query_pairs_mut()
            .append_pair("type", "artist")
            .append_pair("ids", id);
        let following: Vec<bool> = self.get_json(url)?;
        let following = following.first().copied().ok_or_else(|| {
            CatalogError::InvalidResponse("artist follow status is missing".to_owned())
        })?;
        Ok(CatalogPage::Artist {
            artist,
            releases,
            following,
        })
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

    fn web_queue(&mut self) -> Result<Vec<QueueEntry>, CatalogError> {
        #[derive(Deserialize)]
        struct Response {
            queue: Vec<Option<TrackWire>>,
        }
        let response: Response = self.get_json(api_url("me/player/queue")?)?;
        Ok(response
            .queue
            .into_iter()
            .flatten()
            .filter_map(|track| {
                Some(QueueEntry {
                    uri: track.uri?,
                    name: track.name?,
                    detail: names(&track.artists),
                })
            })
            .collect())
    }

    fn queue_track(&mut self, uri: &str) -> Result<(), CatalogError> {
        if self.token.needs_refresh() {
            self.refresh_or_authenticate()?;
        }
        let url = queue_url(uri, &spotifyd_device::device_id())?;
        match request_queue(&self.agent, &url, self.token.access_token()) {
            Err(ureq::Error::StatusCode(401)) => {
                self.refresh_or_authenticate()?;
                request_queue(&self.agent, &url, self.token.access_token())
                    .map_err(CatalogError::from_http)
            }
            result => result.map_err(CatalogError::from_http),
        }
    }

    fn album_queue_tracks(&mut self, id: &str) -> Result<Vec<String>, CatalogError> {
        album_queue_tracks(|offset| {
            let mut url = api_url(&format!("albums/{id}/tracks"))?;
            url.query_pairs_mut()
                .append_pair("limit", "50")
                .append_pair("offset", &offset.to_string());
            self.get_json(url)
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
            CatalogRequest::Track { id, destination } => self.track_destination(id, *destination),
            CatalogRequest::Artist(id) => self.artist(id),
            CatalogRequest::Album(id) => self.album(id),
        }
    }

    fn play(&mut self, playback: &CatalogPlayback) -> Result<(), CatalogError> {
        SpotifyCatalogSource::play(self, playback)
    }

    fn manage_queue(&mut self, edit: Option<QueueEdit>) -> Result<QueueSnapshot, CatalogError> {
        let editing = edit.is_some();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| CatalogError::Unavailable(error.to_string()))?;
        let local = runtime.block_on(async {
            LocalControlPlaybackSource::connect()
                .await?
                .queue(edit)
                .await
        });
        match local {
            Ok(mut snapshot) => {
                // Names are optional enrichment; an API outage must not turn an
                // acknowledged local edit into a reported failure.
                if let Ok(entries) = self.web_queue() {
                    for entry in &mut snapshot.entries {
                        if let Some(metadata) = entries.iter().find(|m| m.uri == entry.uri) {
                            entry.name.clone_from(&metadata.name);
                            entry.detail.clone_from(&metadata.detail);
                        }
                    }
                }
                Ok(snapshot)
            }
            Err(error) if editing => Err(CatalogError::Unavailable(format!("{error}. Press R to reload before trying another edit."))),
            Err(_) => Ok(QueueSnapshot {
                revision: String::new(), entries: self.web_queue()?, editable: false, context_count: 0,
                notice: Some("Read-only Spotify queue. Editing requires updated Spotifyd with local_control, active on this device. Upgrade/start Spotifyd, then press R.".into()),
            }),
        }
    }

    fn queue(&mut self, item: &CatalogItem) -> Result<usize, CatalogError> {
        let uris = match item.kind() {
            CatalogItemKind::Track => vec![item.uri().to_owned()],
            CatalogItemKind::Album => self.album_queue_tracks(item.id())?,
            _ => {
                return Err(CatalogError::InvalidRequest(
                    "Select a track or album.".to_owned(),
                ));
            }
        };
        queue_tracks(&uris, |uri| self.queue_track(uri))
    }

    fn follow_artist(&mut self, id: &str, following: bool) -> Result<(), CatalogError> {
        if self.token.needs_refresh() {
            self.refresh_or_authenticate()?;
        }
        let url = artist_follow_url(id)?;
        match request_artist_follow(&self.agent, &url, following, self.token.access_token()) {
            Err(ureq::Error::StatusCode(401)) => {
                self.refresh_or_authenticate()?;
                request_artist_follow(&self.agent, &url, following, self.token.access_token())
                    .map_err(CatalogError::from_http)
            }
            result => result.map_err(CatalogError::from_http),
        }
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

fn artist_follow_url(id: &str) -> Result<Url, CatalogError> {
    let mut url = api_url("me/following")?;
    url.query_pairs_mut()
        .append_pair("type", "artist")
        .append_pair("ids", id);
    Ok(url)
}

fn request_artist_follow(
    agent: &ureq::Agent,
    url: &Url,
    following: bool,
    access_token: &str,
) -> Result<(), ureq::Error> {
    let authorization = format!("Bearer {access_token}");
    if following {
        agent
            .put(url.as_str())
            .header("Authorization", authorization)
            .send_empty()?;
    } else {
        agent
            .delete(url.as_str())
            .header("Authorization", authorization)
            .call()?;
    }
    Ok(())
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

#[derive(Deserialize)]
struct QueueTracksPage {
    items: Vec<QueueTrack>,
    next: Option<String>,
}

#[derive(Deserialize)]
struct QueueTrack {
    uri: Option<String>,
}

fn album_queue_tracks(
    mut fetch: impl FnMut(usize) -> Result<QueueTracksPage, CatalogError>,
) -> Result<Vec<String>, CatalogError> {
    let mut offset = 0;
    let mut uris = Vec::new();
    loop {
        let page = fetch(offset)?;
        uris.extend(page.items.into_iter().filter_map(|track| track.uri));
        if page.next.is_none() {
            return Ok(uris);
        }
        offset += 50;
    }
}

fn queue_url(uri: &str, device_id: &str) -> Result<Url, CatalogError> {
    let mut url = api_url("me/player/queue")?;
    url.query_pairs_mut()
        .append_pair("uri", uri)
        .append_pair("device_id", device_id);
    Ok(url)
}

fn request_queue(agent: &ureq::Agent, url: &Url, access_token: &str) -> Result<(), ureq::Error> {
    agent
        .post(url.as_str())
        .header("Authorization", format!("Bearer {access_token}"))
        .send_empty()?;
    Ok(())
}

fn queue_tracks(
    uris: &[String],
    mut enqueue: impl FnMut(&str) -> Result<(), CatalogError>,
) -> Result<usize, CatalogError> {
    if uris.is_empty() {
        return Err(CatalogError::Unavailable(
            "No playable tracks to queue.".to_owned(),
        ));
    }
    for (index, uri) in uris.iter().enumerate() {
        if let Err(error) = enqueue(uri) {
            return Err(CatalogError::Unavailable(format!(
                "Added {index}/{} tracks; {error}. Start playback on Spotifyd if no device is active. Retrying adds duplicates of tracks already queued.",
                uris.len()
            )));
        }
    }
    Ok(uris.len())
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
    QueueLoaded {
        request_id: u64,
        result: Result<QueueSnapshot, String>,
    },
    QueueUpdated {
        name: String,
        result: Result<usize, String>,
    },
    ArtistFollowUpdated {
        id: String,
        result: Result<bool, String>,
    },
    Loaded {
        request_id: u64,
        page: CatalogPage,
    },
    Failed {
        request_id: u64,
        message: String,
    },
    PlaybackFailed {
        message: String,
    },
    PlaybackFallback {
        playback: CatalogPlayback,
    },
    HomeUpdated(HomeUpdate),
}

enum RuntimeRequest {
    ManageQueue {
        request_id: u64,
        edit: Option<QueueEdit>,
    },
    Queue(CatalogItem),
    FollowArtist {
        id: String,
        following: bool,
    },
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

    pub fn follow_artist(&self, id: String, following: bool) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::FollowArtist { id, following })
            .map_err(|_| CatalogRuntimeError)
    }

    pub fn try_event(&self) -> Option<CatalogEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn manage_queue(
        &self,
        request_id: u64,
        edit: Option<QueueEdit>,
    ) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::ManageQueue { request_id, edit })
            .map_err(|_| CatalogRuntimeError)
    }

    pub fn queue(&self, item: CatalogItem) -> Result<(), CatalogRuntimeError> {
        self.request_tx
            .send(RuntimeRequest::Queue(item))
            .map_err(|_| CatalogRuntimeError)
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
                RuntimeRequest::ManageQueue { request_id, edit } => {
                    manage_queue(request_id, edit, &mut source, &mut factory, event_tx)
                }
                RuntimeRequest::Queue(item) => {
                    queue_selection(item, &mut source, &mut factory, event_tx)
                }
                RuntimeRequest::FollowArtist { id, following } => {
                    follow_artist(id, following, &mut source, &mut factory, event_tx)
                }
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
            RuntimeRequest::ManageQueue { request_id, edit } => {
                if !manage_queue(request_id, edit, &mut source, &mut factory, event_tx) {
                    return;
                }
                continue;
            }
            RuntimeRequest::Queue(item) => {
                if !queue_selection(item, &mut source, &mut factory, event_tx) {
                    return;
                }
                continue;
            }
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
            RuntimeRequest::FollowArtist { id, following } => {
                if !follow_artist(id, following, &mut source, &mut factory, event_tx) {
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

fn follow_artist<S: CatalogSource, F: FnMut() -> Result<S, CatalogError>>(
    id: String,
    following: bool,
    source: &mut Option<S>,
    factory: &mut F,
    event_tx: &mpsc::Sender<CatalogEvent>,
) -> bool {
    let result = ensure_source(source, factory)
        .and_then(|source| source.follow_artist(&id, following))
        .map(|()| following)
        .map_err(|error| error.to_string());
    event_tx
        .send(CatalogEvent::ArtistFollowUpdated { id, result })
        .is_ok()
}

fn manage_queue<S: CatalogSource, F: FnMut() -> Result<S, CatalogError>>(
    request_id: u64,
    edit: Option<QueueEdit>,
    source: &mut Option<S>,
    factory: &mut F,
    event_tx: &mpsc::Sender<CatalogEvent>,
) -> bool {
    let result = ensure_source(source, factory)
        .and_then(|source| source.manage_queue(edit))
        .map_err(|error| error.to_string());
    event_tx
        .send(CatalogEvent::QueueLoaded { request_id, result })
        .is_ok()
}

fn queue_selection<S: CatalogSource, F: FnMut() -> Result<S, CatalogError>>(
    item: CatalogItem,
    source: &mut Option<S>,
    factory: &mut F,
    event_tx: &mpsc::Sender<CatalogEvent>,
) -> bool {
    let result = ensure_source(source, factory)
        .and_then(|source| source.queue(&item))
        .map_err(|error| error.to_string());
    event_tx
        .send(CatalogEvent::QueueUpdated {
            name: item.name().to_owned(),
            result,
        })
        .is_ok()
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
    fn destination(&self, destination: TrackDestination) -> Result<CatalogRequest, CatalogError> {
        let request = match destination {
            TrackDestination::Album => self
                .album
                .as_ref()
                .and_then(|album| album.id.as_ref())
                .map(|id| CatalogRequest::Album(id.clone())),
            TrackDestination::Artist => self
                .artists
                .first()
                .and_then(|artist| artist.id.as_ref())
                .map(|id| CatalogRequest::Artist(id.clone())),
        };
        request.ok_or_else(|| {
            CatalogError::InvalidResponse(
                "This track has no available album or artist page.".to_owned(),
            )
        })
    }

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
    id: Option<String>,
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
    id: Option<String>,
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
    fn album_queue_fetches_every_page_before_queueing_and_keeps_order() {
        let mut offsets = Vec::new();
        let uris = album_queue_tracks(|offset| {
            offsets.push(offset);
            Ok(if offset == 0 {
                serde_json::from_str(
                    r#"{"items":[{"uri":"spotify:track:first"},{"uri":null}],"next":"next-page"}"#,
                )
                .unwrap()
            } else {
                serde_json::from_str(r#"{"items":[{"uri":"spotify:track:last"}],"next":null}"#)
                    .unwrap()
            })
        })
        .unwrap();
        assert_eq!(offsets, vec![0, 50]);
        assert_eq!(uris, vec!["spotify:track:first", "spotify:track:last"]);
        assert!(
            album_queue_tracks(|offset| {
                if offset == 0 {
                    Ok(QueueTracksPage {
                        items: vec![],
                        next: Some("next".to_owned()),
                    })
                } else {
                    Err(CatalogError::QuotaExceeded)
                }
            })
            .is_err()
        );
    }

    #[test]
    fn queue_url_and_http_request_target_the_selected_track_on_spotifyd() {
        use std::io::{BufRead, BufReader, Write};
        let mut url = queue_url("spotify:track:chosen", "local-device").unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        url.set_scheme("http").unwrap();
        url.set_host(Some("127.0.0.1")).unwrap();
        url.set_port(Some(listener.local_addr().unwrap().port()))
            .unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                line.trim(),
                "POST /v1/me/player/queue?uri=spotify%3Atrack%3Achosen&device_id=local-device HTTP/1.1"
            );
            let mut authorized = false;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                authorized |= line
                    .to_ascii_lowercase()
                    .starts_with("authorization: bearer queue-token");
            }
            assert!(authorized);
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .unwrap();
        });
        request_queue(&ureq::Agent::new_with_defaults(), &url, "queue-token").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn album_queue_preserves_order_and_stops_at_first_failure() {
        let uris = vec!["first".to_owned(), "second".to_owned(), "third".to_owned()];
        let mut seen = Vec::new();
        assert_eq!(
            queue_tracks(&uris, |uri| {
                seen.push(uri.to_owned());
                Ok(())
            })
            .unwrap(),
            3
        );
        assert_eq!(seen, uris);
        seen.clear();
        let error = queue_tracks(&uris, |uri| {
            seen.push(uri.to_owned());
            if uri == "second" {
                Err(CatalogError::QuotaExceeded)
            } else {
                Ok(())
            }
        })
        .unwrap_err()
        .to_string();
        assert_eq!(seen, vec!["first", "second"]);
        assert!(error.contains("Added 1/3 tracks"));
        assert!(queue_tracks(&[], |_| panic!("empty album must not submit requests")).is_err());
    }

    #[test]
    fn queue_writes_survive_fetch_coalescing_and_report_errors() {
        struct QueueSource;
        impl CatalogSource for QueueSource {
            fn fetch(&mut self, _: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
                panic!("superseded fetch")
            }
            fn queue(&mut self, item: &CatalogItem) -> Result<usize, CatalogError> {
                if item.id() == "first" {
                    Ok(1)
                } else {
                    Err(CatalogError::QuotaExceeded)
                }
            }
        }
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        for id in ["first", "second"] {
            request_tx
                .send(RuntimeRequest::Queue(CatalogItem::new(
                    CatalogItemKind::Track,
                    id,
                    format!("spotify:track:{id}"),
                    id,
                    "",
                    None,
                )))
                .unwrap();
            request_tx
                .send(RuntimeRequest::Fetch {
                    request_id: 1,
                    request: CatalogRequest::Search("superseded".to_owned()),
                })
                .unwrap();
        }
        request_tx.send(RuntimeRequest::Shutdown).unwrap();
        run_worker(
            || Ok(QueueSource),
            HomeWorker::new(HomeConfig::default(), None),
            request_rx,
            &event_tx,
        );
        assert_eq!(
            event_rx.try_recv().unwrap(),
            CatalogEvent::QueueUpdated {
                name: "first".to_owned(),
                result: Ok(1)
            }
        );
        assert_eq!(
            event_rx.try_recv().unwrap(),
            CatalogEvent::QueueUpdated {
                name: "second".to_owned(),
                result: Err(CatalogError::QuotaExceeded.to_string())
            }
        );
        assert!(event_rx.try_recv().is_err());
    }

    #[test]
    fn current_track_pages_use_album_and_primary_artist_ids() {
        let track: TrackWire = serde_json::from_str(
            r#"{
            "album": {"id": "album-id"},
            "artists": [{"id": "primary"}, {"id": "guest"}]
        }"#,
        )
        .unwrap();
        assert_eq!(
            track.destination(TrackDestination::Album).unwrap(),
            CatalogRequest::Album("album-id".to_owned())
        );
        assert_eq!(
            track.destination(TrackDestination::Artist).unwrap(),
            CatalogRequest::Artist("primary".to_owned())
        );
        let missing: TrackWire = serde_json::from_str("{}").unwrap();
        assert!(missing.destination(TrackDestination::Album).is_err());
        assert!(missing.destination(TrackDestination::Artist).is_err());
    }

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
            following: false,
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

    #[test]
    fn artist_follow_http_uses_put_delete_and_accepts_empty_responses() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let url = artist_follow_url("artist-id").unwrap();
        let local_url = Url::parse(&format!(
            "http://{address}{}?{}",
            url.path(),
            url.query().unwrap()
        ))
        .unwrap();
        let server = thread::spawn(move || {
            for method in ["PUT", "DELETE"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(
                    line.trim(),
                    format!("{method} /v1/me/following?type=artist&ids=artist-id HTTP/1.1")
                );
                let mut authorized = false;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if line
                        .to_ascii_lowercase()
                        .starts_with("authorization: bearer test-token")
                    {
                        authorized = true;
                    }
                }
                assert!(authorized);
                stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .unwrap();
            }
        });
        let agent = ureq::Agent::new_with_defaults();
        request_artist_follow(&agent, &local_url, true, "test-token").unwrap();
        request_artist_follow(&agent, &local_url, false, "test-token").unwrap();
        server.join().unwrap();
    }

    #[test]
    fn queued_artist_follow_writes_survive_fetch_coalescing() {
        struct FollowSource;
        impl CatalogSource for FollowSource {
            fn fetch(&mut self, _: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
                panic!("superseded fetches must be discarded")
            }
            fn follow_artist(&mut self, id: &str, following: bool) -> Result<(), CatalogError> {
                assert_eq!(id, "artist");
                if following {
                    Ok(())
                } else {
                    Err(CatalogError::QuotaExceeded)
                }
            }
        }
        let (request_tx, request_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        for following in [true, false] {
            request_tx
                .send(RuntimeRequest::FollowArtist {
                    id: "artist".to_owned(),
                    following,
                })
                .unwrap();
            request_tx
                .send(RuntimeRequest::Fetch {
                    request_id: 1,
                    request: CatalogRequest::Search("superseded".to_owned()),
                })
                .unwrap();
        }
        request_tx.send(RuntimeRequest::Shutdown).unwrap();
        run_worker(
            || Ok(FollowSource),
            HomeWorker::new(HomeConfig::default(), None),
            request_rx,
            &event_tx,
        );
        assert_eq!(
            event_rx.try_recv().unwrap(),
            CatalogEvent::ArtistFollowUpdated {
                id: "artist".to_owned(),
                result: Ok(true)
            }
        );
        assert_eq!(
            event_rx.try_recv().unwrap(),
            CatalogEvent::ArtistFollowUpdated {
                id: "artist".to_owned(),
                result: Err(CatalogError::QuotaExceeded.to_string())
            }
        );
        assert!(event_rx.try_recv().is_err());
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
