use std::{
    cmp::Reverse,
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    catalog::{CatalogError, CatalogItem},
    catalog_auth::write_private_file,
    config::{HomeConfig, HomeSectionKind, TopArtistsRange},
};

const NEW_RELEASES_LIMIT: usize = 25;
const RELEASE_PROGRESS_INTERVAL: usize = 10;
const SECONDS_PER_DAY: u64 = 86_400;
const CACHE_TTL_SECS: u64 = 6 * 60 * 60;
const FAILED_LOAD_RETRY_SECS: u64 = 15 * 60;
const CACHE_VERSION: u32 = 1;

impl HomeSectionKind {
    pub const fn title(self) -> &'static str {
        match self {
            Self::RecentlyPlayed => "Jump back in",
            Self::NewReleases => "New from artists you follow",
            Self::Playlists => "Your playlists",
            Self::TopArtists => "Your top artists",
        }
    }

    pub const fn empty_message(self) -> &'static str {
        match self {
            Self::RecentlyPlayed => "Nothing played recently",
            Self::NewReleases => "No recent releases from artists you follow",
            Self::Playlists => "No playlists yet",
            Self::TopArtists => "Spotify has no top artists for you yet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSectionStatus {
    Loading,
    Ready,
    Notice(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeSection {
    kind: HomeSectionKind,
    items: Vec<CatalogItem>,
    status: HomeSectionStatus,
}

impl HomeSection {
    pub const fn kind(&self) -> HomeSectionKind {
        self.kind
    }

    pub fn items(&self) -> &[CatalogItem] {
        &self.items
    }

    pub const fn status(&self) -> &HomeSectionStatus {
        &self.status
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeUpdate {
    pub kind: HomeSectionKind,
    pub items: Vec<CatalogItem>,
    pub status: HomeSectionStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HomePage {
    sections: Vec<HomeSection>,
}

impl HomePage {
    pub fn new(kinds: &[HomeSectionKind]) -> Self {
        Self {
            sections: kinds
                .iter()
                .map(|kind| HomeSection {
                    kind: *kind,
                    items: Vec::new(),
                    status: HomeSectionStatus::Loading,
                })
                .collect(),
        }
    }

    pub fn sections(&self) -> &[HomeSection] {
        &self.sections
    }

    pub fn items(&self) -> impl Iterator<Item = &CatalogItem> {
        self.sections.iter().flat_map(|section| &section.items)
    }

    pub fn item_count(&self) -> usize {
        self.items().count()
    }

    pub fn item(&self, index: usize) -> Option<&CatalogItem> {
        self.items().nth(index)
    }

    pub fn position(&self, uri: &str) -> Option<usize> {
        self.items().position(|item| item.uri() == uri)
    }

    pub fn is_loading(&self) -> bool {
        self.sections
            .iter()
            .any(|section| section.status == HomeSectionStatus::Loading)
    }

    pub fn mark_loading(&mut self) {
        for section in &mut self.sections {
            section.status = HomeSectionStatus::Loading;
        }
    }

    pub fn apply(&mut self, update: HomeUpdate) {
        let Some(section) = self
            .sections
            .iter_mut()
            .find(|section| section.kind == update.kind)
        else {
            return;
        };
        // Partial or failed results never shrink what is already on screen.
        if update.status == HomeSectionStatus::Ready || update.items.len() >= section.items.len() {
            section.items = update.items;
        }
        section.status = update.status;
    }

    pub fn artwork_url(&self, selected: usize) -> Option<&str> {
        self.item(selected).and_then(CatalogItem::image_url)
    }

    pub fn prefetch_artwork_urls(&self, selected: usize) -> Vec<&str> {
        let items = self.items().collect::<Vec<_>>();
        items
            .iter()
            .skip(selected.saturating_add(1))
            .chain(items[..selected.min(items.len())].iter().rev())
            .filter_map(|item| item.image_url())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeRelease {
    released_on: i64,
    item: CatalogItem,
}

impl HomeRelease {
    pub(crate) const fn new(released_on: i64, item: CatalogItem) -> Self {
        Self { released_on, item }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeStep {
    RecentlyPlayed,
    Playlists,
    TopArtists(TopArtistsRange),
    FollowedArtists { after: Option<String> },
    ArtistReleases(String),
}

impl HomeStep {
    const fn kind(&self) -> HomeSectionKind {
        match self {
            Self::RecentlyPlayed => HomeSectionKind::RecentlyPlayed,
            Self::Playlists => HomeSectionKind::Playlists,
            Self::TopArtists(_) => HomeSectionKind::TopArtists,
            Self::FollowedArtists { .. } | Self::ArtistReleases(_) => HomeSectionKind::NewReleases,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeData {
    Items(Vec<CatalogItem>),
    FollowedArtists {
        artist_ids: Vec<String>,
        after: Option<String>,
    },
    Releases(Vec<HomeRelease>),
}

/// Walks one home load a request at a time, so the catalogue worker can serve
/// the user between steps and a partial result is never lost to a later failure.
#[derive(Debug)]
struct HomeLoader {
    steps: VecDeque<HomeStep>,
    today: i64,
    new_releases_days: u32,
    max_artists: usize,
    artists_found: usize,
    artists_checked: usize,
    releases: Vec<HomeRelease>,
    completed: Vec<CachedSection>,
    failed: bool,
}

impl HomeLoader {
    fn new(config: &HomeConfig, now: u64) -> Self {
        let mut steps = VecDeque::new();
        for kind in config.sections() {
            match kind {
                HomeSectionKind::RecentlyPlayed => steps.push_back(HomeStep::RecentlyPlayed),
                HomeSectionKind::Playlists => steps.push_back(HomeStep::Playlists),
                HomeSectionKind::TopArtists => {
                    steps.push_back(HomeStep::TopArtists(config.top_artists_range()));
                }
                HomeSectionKind::NewReleases => {}
            }
        }
        // The release scan is the slowest section, so it always runs last and
        // every step left after a release failure belongs to that same scan.
        if config.sections().contains(&HomeSectionKind::NewReleases) {
            steps.push_back(HomeStep::FollowedArtists { after: None });
        }
        Self {
            steps,
            today: i64::try_from(now / SECONDS_PER_DAY).unwrap_or(i64::MAX),
            new_releases_days: config.new_releases_days(),
            max_artists: config.new_releases_max_artists(),
            artists_found: 0,
            artists_checked: 0,
            releases: Vec::new(),
            completed: Vec::new(),
            failed: false,
        }
    }

    fn is_finished(&self) -> bool {
        self.steps.is_empty()
    }

    fn advance(&mut self, result: Result<HomeData, CatalogError>) -> Option<HomeUpdate> {
        let step = self.steps.pop_front()?;
        match result {
            Ok(HomeData::Items(items)) => Some(self.finish_section(step.kind(), items)),
            Ok(HomeData::FollowedArtists { artist_ids, after }) => {
                self.queue_artists(artist_ids, after)
            }
            Ok(HomeData::Releases(releases)) => self.record_releases(releases),
            Err(error) => Some(self.fail_section(step.kind(), &error.to_string())),
        }
    }

    fn abort(&mut self, message: &str) -> Vec<HomeUpdate> {
        let mut updates = Vec::new();
        while let Some(step) = self.steps.pop_front() {
            updates.push(self.fail_section(step.kind(), message));
        }
        updates
    }

    fn queue_artists(
        &mut self,
        mut artist_ids: Vec<String>,
        after: Option<String>,
    ) -> Option<HomeUpdate> {
        artist_ids.truncate(self.max_artists.saturating_sub(self.artists_found));
        self.artists_found += artist_ids.len();
        if self.artists_found < self.max_artists
            && let Some(after) = after
        {
            self.steps
                .push_front(HomeStep::FollowedArtists { after: Some(after) });
        }
        self.steps
            .extend(artist_ids.into_iter().map(HomeStep::ArtistReleases));
        self.steps
            .is_empty()
            .then(|| self.finish_section(HomeSectionKind::NewReleases, Vec::new()))
    }

    fn record_releases(&mut self, releases: Vec<HomeRelease>) -> Option<HomeUpdate> {
        self.releases.extend(releases);
        self.artists_checked += 1;
        let items = self.recent_releases();
        if self.steps.is_empty() {
            return Some(self.finish_section(HomeSectionKind::NewReleases, items));
        }
        self.artists_checked
            .is_multiple_of(RELEASE_PROGRESS_INTERVAL)
            .then_some(HomeUpdate {
                kind: HomeSectionKind::NewReleases,
                items,
                status: HomeSectionStatus::Loading,
            })
    }

    fn finish_section(&mut self, kind: HomeSectionKind, items: Vec<CatalogItem>) -> HomeUpdate {
        self.completed.push(CachedSection {
            kind,
            items: items.clone(),
        });
        HomeUpdate {
            kind,
            items,
            status: HomeSectionStatus::Ready,
        }
    }

    fn fail_section(&mut self, kind: HomeSectionKind, message: &str) -> HomeUpdate {
        self.failed = true;
        if kind != HomeSectionKind::NewReleases {
            return HomeUpdate {
                kind,
                items: Vec::new(),
                status: HomeSectionStatus::Notice(message.to_owned()),
            };
        }
        // Keep what the scan found and stop spending quota on the remaining artists.
        self.steps.clear();
        let notice = if self.artists_checked > 0 {
            format!(
                "Checked {} of {} artists: {message}",
                self.artists_checked, self.artists_found
            )
        } else {
            message.to_owned()
        };
        HomeUpdate {
            kind,
            items: self.recent_releases(),
            status: HomeSectionStatus::Notice(notice),
        }
    }

    fn recent_releases(&self) -> Vec<CatalogItem> {
        let mut recent = self
            .releases
            .iter()
            .filter(|release| {
                self.today.saturating_sub(release.released_on) <= i64::from(self.new_releases_days)
            })
            .collect::<Vec<_>>();
        recent.sort_by_key(|release| Reverse(release.released_on));
        let mut seen = HashSet::new();
        recent
            .into_iter()
            .filter(|release| seen.insert(release.item.id()))
            .take(NEW_RELEASES_LIMIT)
            .map(|release| release.item.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedSection {
    kind: HomeSectionKind,
    items: Vec<CatalogItem>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HomeCache {
    version: u32,
    fetched_at: u64,
    sections: Vec<CachedSection>,
}

/// The catalogue worker's view of home: what is cached, when it next needs
/// fetching, and the load in progress.
#[derive(Debug)]
pub(crate) struct HomeWorker {
    config: HomeConfig,
    cache_path: Option<PathBuf>,
    restored: bool,
    next_load_at: u64,
    loader: Option<HomeLoader>,
}

impl HomeWorker {
    pub(crate) const fn new(config: HomeConfig, cache_path: Option<PathBuf>) -> Self {
        Self {
            config,
            cache_path,
            restored: false,
            next_load_at: 0,
            loader: None,
        }
    }

    pub(crate) const fn is_loading(&self) -> bool {
        self.loader.is_some()
    }

    pub(crate) fn next_step(&self) -> Option<&HomeStep> {
        self.loader.as_ref()?.steps.front()
    }

    pub(crate) fn begin(&mut self, force: bool, now: u64) -> Vec<HomeUpdate> {
        let updates = if self.restored {
            Vec::new()
        } else {
            self.restored = true;
            self.restore(now)
        };
        if force || (self.loader.is_none() && now >= self.next_load_at) {
            let loader = HomeLoader::new(&self.config, now);
            self.loader = (!loader.is_finished()).then_some(loader);
        }
        updates
    }

    pub(crate) fn advance(
        &mut self,
        result: Result<HomeData, CatalogError>,
        now: u64,
    ) -> Option<HomeUpdate> {
        let update = self.loader.as_mut()?.advance(result);
        self.finish_if_done(now);
        update
    }

    pub(crate) fn abort(&mut self, message: &str, now: u64) -> Vec<HomeUpdate> {
        let updates = self
            .loader
            .as_mut()
            .map_or_else(Vec::new, |loader| loader.abort(message));
        self.finish_if_done(now);
        updates
    }

    fn finish_if_done(&mut self, now: u64) {
        let Some(loader) = self.loader.take_if(|loader| loader.is_finished()) else {
            return;
        };
        if loader.failed {
            self.next_load_at = now.saturating_add(FAILED_LOAD_RETRY_SECS);
            return;
        }
        self.next_load_at = now.saturating_add(CACHE_TTL_SECS);
        if let Some(path) = &self.cache_path {
            // The cache only saves requests, so a failed write just means fetching again.
            let _ = write_cache(path, now, loader.completed);
        }
    }

    fn restore(&mut self, now: u64) -> Vec<HomeUpdate> {
        let Some(cache) = self.cache_path.as_deref().and_then(read_cache) else {
            return Vec::new();
        };
        let complete = self
            .config
            .sections()
            .iter()
            .all(|kind| cache.sections.iter().any(|section| section.kind == *kind));
        let expires_at = cache.fetched_at.saturating_add(CACHE_TTL_SECS);
        let fresh = complete && now < expires_at;
        if fresh {
            self.next_load_at = expires_at;
        }
        cache
            .sections
            .into_iter()
            .filter(|section| self.config.sections().contains(&section.kind))
            .map(|section| HomeUpdate {
                kind: section.kind,
                items: section.items,
                status: if fresh {
                    HomeSectionStatus::Ready
                } else {
                    HomeSectionStatus::Loading
                },
            })
            .collect()
    }
}

fn read_cache(path: &Path) -> Option<HomeCache> {
    let bytes = fs::read(path).ok()?;
    let cache: HomeCache = serde_json::from_slice(&bytes).ok()?;
    (cache.version == CACHE_VERSION).then_some(cache)
}

fn write_cache(path: &Path, now: u64, sections: Vec<CachedSection>) -> std::io::Result<()> {
    let contents = serde_json::to_vec(&HomeCache {
        version: CACHE_VERSION,
        fetched_at: now,
        sections,
    })?;
    write_private_file(path, &contents)
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Days since the Unix epoch for a `YYYY-MM-DD` date. Spotify shortens the date
/// for year and month precision releases, which cannot be placed in a recent
/// window and so return `None`.
pub(crate) fn parse_release_day(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(year.div_euclid(400) * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use std::{env, sync::atomic::AtomicU64, sync::atomic::Ordering};

    use super::*;
    use crate::{catalog::CatalogItemKind, config::Config};

    static NEXT_CACHE_PATH: AtomicU64 = AtomicU64::new(0);

    const TODAY: &str = "2026-11-01";
    const NOW: u64 = 1_793_534_400;

    fn config(toml: &str) -> HomeConfig {
        Config::from_toml(&format!("version = 1\n\n[home]\n{toml}\n"))
            .expect("home configuration should resolve")
            .home()
            .clone()
    }

    fn item(kind: CatalogItemKind, id: &str) -> CatalogItem {
        CatalogItem::new(
            kind,
            id,
            format!("spotify:{}:{id}", kind.to_string().to_ascii_lowercase()),
            format!("Item {id}"),
            "detail",
            Some(format!("https://example.com/{id}.jpg")),
        )
    }

    fn album(id: &str) -> CatalogItem {
        item(CatalogItemKind::Album, id)
    }

    fn release(id: &str, date: &str) -> HomeRelease {
        HomeRelease::new(
            parse_release_day(date).expect("fixture date should parse"),
            album(id),
        )
    }

    fn items(update: &HomeUpdate) -> Vec<&str> {
        update.items.iter().map(CatalogItem::id).collect()
    }

    fn cache_path() -> PathBuf {
        let unique = NEXT_CACHE_PATH.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "spotify-tui-home-cache-{}-{unique}.json",
            std::process::id()
        ))
    }

    fn followed(ids: &[&str], after: Option<&str>) -> Result<HomeData, CatalogError> {
        Ok(HomeData::FollowedArtists {
            artist_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
            after: after.map(str::to_owned),
        })
    }

    #[test]
    fn release_days_count_from_the_unix_epoch_across_leap_years() {
        assert_eq!(parse_release_day("1970-01-01"), Some(0));
        assert_eq!(parse_release_day("1970-03-01"), Some(59));
        assert_eq!(parse_release_day("2024-03-01"), Some(19_783));
        assert_eq!(parse_release_day(TODAY), Some(20_758));
        assert_eq!(
            i64::try_from(NOW / SECONDS_PER_DAY).ok(),
            parse_release_day(TODAY)
        );
    }

    #[test]
    fn coarse_or_malformed_dates_cannot_be_placed_in_a_recent_window() {
        for date in [
            "2026",
            "2026-10",
            "2026-13-01",
            "2026-10-1x",
            "2026-10-01-02",
            "",
        ] {
            assert_eq!(parse_release_day(date), None, "{date}");
        }
    }

    #[test]
    fn sections_load_quick_steps_first_and_the_release_scan_last() {
        let mut loader = HomeLoader::new(&HomeConfig::default(), NOW);

        assert_eq!(
            Vec::from(std::mem::take(&mut loader.steps)),
            vec![
                HomeStep::RecentlyPlayed,
                HomeStep::Playlists,
                HomeStep::TopArtists(TopArtistsRange::MediumTerm),
                HomeStep::FollowedArtists { after: None },
            ]
        );
    }

    #[test]
    fn disabled_sections_spend_no_requests() {
        let loader = HomeLoader::new(&config(r#"sections = ["playlists"]"#), NOW);

        assert_eq!(loader.steps, VecDeque::from([HomeStep::Playlists]));
    }

    #[test]
    fn followed_artists_page_until_the_configured_cap_is_reached() {
        let mut loader = HomeLoader::new(
            &config(
                r#"
sections = ["new_releases"]
new_releases_max_artists = 3
"#,
            ),
            NOW,
        );

        assert_eq!(loader.advance(followed(&["a", "b"], Some("b"))), None);
        assert_eq!(
            loader.steps.front(),
            Some(&HomeStep::FollowedArtists {
                after: Some("b".to_owned())
            })
        );
        assert_eq!(loader.advance(followed(&["c", "d"], Some("d"))), None);

        assert_eq!(
            loader.steps,
            VecDeque::from([
                HomeStep::ArtistReleases("a".to_owned()),
                HomeStep::ArtistReleases("b".to_owned()),
                HomeStep::ArtistReleases("c".to_owned()),
            ])
        );
    }

    #[test]
    fn following_nobody_finishes_with_an_empty_ready_section() {
        let mut loader = HomeLoader::new(&config(r#"sections = ["new_releases"]"#), NOW);

        let update = loader
            .advance(followed(&[], None))
            .expect("the section should finish");

        assert_eq!(update.status, HomeSectionStatus::Ready);
        assert!(update.items.is_empty());
        assert!(loader.is_finished());
    }

    #[test]
    fn releases_merge_newest_first_within_the_window_without_duplicates() {
        let mut loader = HomeLoader::new(
            &config(
                r#"
sections = ["new_releases"]
new_releases_days = 30
"#,
            ),
            NOW,
        );
        loader.advance(followed(&["a", "b"], None));
        assert_eq!(
            loader.advance(Ok(HomeData::Releases(vec![
                release("edge", "2026-10-02"),
                release("expired", "2026-10-01"),
                release("collab", "2026-10-20"),
            ]))),
            None
        );

        let update = loader
            .advance(Ok(HomeData::Releases(vec![
                release("newest", "2026-10-30"),
                release("collab", "2026-10-20"),
            ])))
            .expect("the last artist should finish the section");

        assert_eq!(update.status, HomeSectionStatus::Ready);
        assert_eq!(items(&update), ["newest", "collab", "edge"]);
    }

    #[test]
    fn long_release_scans_publish_progress() {
        let mut loader = HomeLoader::new(&config(r#"sections = ["new_releases"]"#), NOW);
        let artists = (0..12).map(|index| index.to_string()).collect::<Vec<_>>();
        loader.advance(Ok(HomeData::FollowedArtists {
            artist_ids: artists,
            after: None,
        }));

        let updates = (0..12)
            .filter_map(|index| {
                loader.advance(Ok(HomeData::Releases(vec![release(
                    &format!("album-{index}"),
                    "2026-10-30",
                )])))
            })
            .collect::<Vec<_>>();

        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].status, HomeSectionStatus::Loading);
        assert_eq!(updates[0].items.len(), 10);
        assert_eq!(updates[1].status, HomeSectionStatus::Ready);
        assert_eq!(updates[1].items.len(), 12);
    }

    #[test]
    fn a_failed_section_does_not_stop_the_others() {
        let mut loader = HomeLoader::new(&HomeConfig::default(), NOW);

        let failed = loader
            .advance(Err(CatalogError::Forbidden))
            .expect("the failure should be reported");
        let next = loader
            .advance(Ok(HomeData::Items(vec![item(
                CatalogItemKind::Playlist,
                "playlist",
            )])))
            .expect("the next section should still load");

        assert_eq!(failed.kind, HomeSectionKind::RecentlyPlayed);
        assert!(matches!(failed.status, HomeSectionStatus::Notice(_)));
        assert_eq!(next.kind, HomeSectionKind::Playlists);
        assert_eq!(next.status, HomeSectionStatus::Ready);
    }

    #[test]
    fn a_quota_failure_keeps_partial_releases_and_stops_scanning() {
        let mut loader = HomeLoader::new(&config(r#"sections = ["new_releases"]"#), NOW);
        loader.advance(followed(&["a", "b", "c"], None));
        loader.advance(Ok(HomeData::Releases(vec![release("kept", "2026-10-30")])));

        let update = loader
            .advance(Err(CatalogError::QuotaExceeded))
            .expect("the failure should be reported");

        assert_eq!(items(&update), ["kept"]);
        assert!(matches!(
            update.status,
            HomeSectionStatus::Notice(notice)
                if notice.starts_with("Checked 1 of 3 artists:")
        ));
        assert!(loader.is_finished());
    }

    #[test]
    fn aborting_reports_every_unfinished_section_once() {
        let mut loader = HomeLoader::new(&HomeConfig::default(), NOW);

        let updates = loader.abort("sign-in failed");

        assert_eq!(
            updates.iter().map(|update| update.kind).collect::<Vec<_>>(),
            vec![
                HomeSectionKind::RecentlyPlayed,
                HomeSectionKind::Playlists,
                HomeSectionKind::TopArtists,
                HomeSectionKind::NewReleases,
            ]
        );
        assert!(loader.failed);
    }

    #[test]
    fn partial_results_never_shrink_a_section_but_a_finished_load_replaces_it() {
        let mut page = HomePage::new(&[HomeSectionKind::NewReleases]);
        let update = |ids: &[&str], status| HomeUpdate {
            kind: HomeSectionKind::NewReleases,
            items: ids.iter().map(|id| album(id)).collect(),
            status,
        };
        page.apply(update(&["a", "b"], HomeSectionStatus::Ready));

        page.apply(update(&["a"], HomeSectionStatus::Loading));
        assert_eq!(page.item_count(), 2);
        page.apply(update(&[], HomeSectionStatus::Notice("quota".to_owned())));
        assert_eq!(page.item_count(), 2);

        page.apply(update(&["c"], HomeSectionStatus::Ready));
        assert_eq!(page.item_count(), 1);
    }

    #[test]
    fn page_items_are_indexed_across_sections_for_selection_and_artwork() {
        let mut page =
            HomePage::new(&[HomeSectionKind::RecentlyPlayed, HomeSectionKind::Playlists]);
        page.apply(HomeUpdate {
            kind: HomeSectionKind::Playlists,
            items: vec![item(CatalogItemKind::Playlist, "second")],
            status: HomeSectionStatus::Ready,
        });
        page.apply(HomeUpdate {
            kind: HomeSectionKind::RecentlyPlayed,
            items: vec![item(CatalogItemKind::Track, "first")],
            status: HomeSectionStatus::Ready,
        });

        assert_eq!(page.item(1).map(CatalogItem::id), Some("second"));
        assert_eq!(page.position("spotify:playlist:second"), Some(1));
        assert_eq!(page.artwork_url(0), Some("https://example.com/first.jpg"));
        assert_eq!(
            page.prefetch_artwork_urls(0),
            vec!["https://example.com/second.jpg"]
        );
    }

    #[test]
    fn a_completed_load_is_cached_and_a_new_session_spends_no_requests() {
        let path = cache_path();
        let home = config(r#"sections = ["playlists"]"#);
        let mut first = HomeWorker::new(home.clone(), Some(path.clone()));
        assert!(first.begin(false, NOW).is_empty());
        assert_eq!(first.next_step(), Some(&HomeStep::Playlists));
        first.advance(
            Ok(HomeData::Items(vec![item(
                CatalogItemKind::Playlist,
                "cached",
            )])),
            NOW,
        );
        assert!(!first.is_loading());

        let mut second = HomeWorker::new(home, Some(path.clone()));
        let restored = second.begin(false, NOW + CACHE_TTL_SECS - 1);
        let _ = fs::remove_file(path);

        assert_eq!(restored.len(), 1);
        assert_eq!(items(&restored[0]), ["cached"]);
        assert_eq!(restored[0].status, HomeSectionStatus::Ready);
        assert!(!second.is_loading());
    }

    #[test]
    fn an_expired_cache_is_shown_while_it_refreshes() {
        let path = cache_path();
        let home = config(r#"sections = ["playlists"]"#);
        write_cache(
            &path,
            NOW,
            vec![CachedSection {
                kind: HomeSectionKind::Playlists,
                items: vec![album("stale")],
            }],
        )
        .expect("cache should be written");

        let mut worker = HomeWorker::new(home, Some(path.clone()));
        let restored = worker.begin(false, NOW + CACHE_TTL_SECS);
        let _ = fs::remove_file(path);

        assert_eq!(items(&restored[0]), ["stale"]);
        assert_eq!(restored[0].status, HomeSectionStatus::Loading);
        assert!(worker.is_loading());
    }

    #[test]
    fn a_cache_missing_a_newly_enabled_section_still_refreshes() {
        let path = cache_path();
        write_cache(
            &path,
            NOW,
            vec![CachedSection {
                kind: HomeSectionKind::Playlists,
                items: Vec::new(),
            }],
        )
        .expect("cache should be written");

        let mut worker = HomeWorker::new(
            config(r#"sections = ["playlists", "top_artists"]"#),
            Some(path.clone()),
        );
        worker.begin(false, NOW);
        let _ = fs::remove_file(path);

        assert!(worker.is_loading());
    }

    #[test]
    fn corrupt_or_outdated_caches_are_ignored() {
        let path = cache_path();
        fs::write(&path, "not-json").expect("cache should be written");
        assert!(read_cache(&path).is_none());

        fs::write(
            &path,
            format!(
                r#"{{"version":{},"fetched_at":0,"sections":[]}}"#,
                CACHE_VERSION + 1
            ),
        )
        .expect("cache should be written");
        assert!(read_cache(&path).is_none());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_failed_load_is_not_cached_and_backs_off_before_retrying() {
        let path = cache_path();
        let mut worker = HomeWorker::new(config(r#"sections = ["playlists"]"#), Some(path.clone()));
        worker.begin(false, NOW);
        worker.advance(Err(CatalogError::QuotaExceeded), NOW);

        assert!(!path.exists());
        worker.begin(false, NOW + FAILED_LOAD_RETRY_SECS - 1);
        assert!(!worker.is_loading());
        worker.begin(false, NOW + FAILED_LOAD_RETRY_SECS);
        assert!(worker.is_loading());
    }

    #[test]
    fn a_forced_refresh_ignores_a_fresh_cache() {
        let mut worker = HomeWorker::new(config(r#"sections = ["playlists"]"#), None);
        worker.begin(false, NOW);
        worker.advance(Ok(HomeData::Items(Vec::new())), NOW);

        worker.begin(false, NOW + 1);
        assert!(!worker.is_loading());
        worker.begin(true, NOW + 1);
        assert!(worker.is_loading());
    }

    #[test]
    fn a_forced_refresh_restarts_a_load_already_running() {
        let mut worker = HomeWorker::new(HomeConfig::default(), None);
        worker.begin(false, NOW);
        worker.advance(Ok(HomeData::Items(Vec::new())), NOW);
        assert_eq!(worker.next_step(), Some(&HomeStep::Playlists));

        worker.begin(true, NOW);

        assert_eq!(worker.next_step(), Some(&HomeStep::RecentlyPlayed));
    }

    #[test]
    fn opening_home_with_no_sections_starts_nothing() {
        let mut worker = HomeWorker::new(config("sections = []"), None);

        worker.begin(false, NOW);

        assert!(!worker.is_loading());
    }
}
