use crate::{
    catalog::{CatalogEvent, CatalogItemKind, CatalogPage, CatalogPlayback, CatalogRequest},
    config::HomeSectionKind,
    home::{HomePage, HomeUpdate},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    Closed,
    Home,
    Editing,
    Page,
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCommand {
    OpenSearch,
    OpenHome,
    RefreshHome,
    Insert(char),
    Paste(String),
    Backspace,
    Submit,
    Previous,
    Next,
    Activate,
    Back,
    Close,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserEffect {
    None,
    Fetch {
        request_id: u64,
        request: CatalogRequest,
    },
    LoadHome {
        force: bool,
    },
    Play(String),
    PlayTrack(CatalogPlayback),
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserView {
    Closed,
    Home,
    Editing { query: String },
    Loading { label: String },
    Page { page: CatalogPage, selected: usize },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserState {
    view: BrowserView,
    history: Vec<(CatalogPage, usize)>,
    next_request_id: u64,
    pending_request_id: Option<u64>,
    pending_pushes_history: bool,
    fallback: Option<Box<BrowserView>>,
    home: HomePage,
    home_selected: usize,
    home_is_root: bool,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            view: BrowserView::Closed,
            history: Vec::new(),
            next_request_id: 0,
            pending_request_id: None,
            pending_pushes_history: false,
            fallback: None,
            home: HomePage::default(),
            home_selected: 0,
            home_is_root: false,
        }
    }
}

impl BrowserState {
    pub fn set_home_sections(&mut self, sections: &[HomeSectionKind]) {
        self.home = HomePage::new(sections);
        self.home_selected = 0;
    }

    pub const fn view(&self) -> &BrowserView {
        &self.view
    }

    pub const fn home(&self) -> &HomePage {
        &self.home
    }

    pub const fn home_selected(&self) -> usize {
        self.home_selected
    }

    pub const fn mode(&self) -> BrowserMode {
        match self.view {
            BrowserView::Closed => BrowserMode::Closed,
            BrowserView::Home => BrowserMode::Home,
            BrowserView::Editing { .. } => BrowserMode::Editing,
            BrowserView::Page { .. } => BrowserMode::Page,
            BrowserView::Loading { .. } | BrowserView::Error { .. } => BrowserMode::Waiting,
        }
    }

    pub fn artwork_url(&self) -> Option<&str> {
        match &self.view {
            BrowserView::Page { page, selected } => page.artwork_url(*selected),
            BrowserView::Home => self.home.artwork_url(self.home_selected),
            BrowserView::Closed
            | BrowserView::Editing { .. }
            | BrowserView::Loading { .. }
            | BrowserView::Error { .. } => None,
        }
    }

    pub fn prefetch_artwork_urls(&self) -> Vec<&str> {
        match &self.view {
            BrowserView::Page { page, selected } => page.prefetch_artwork_urls(*selected),
            BrowserView::Home => self.home.prefetch_artwork_urls(self.home_selected),
            BrowserView::Closed
            | BrowserView::Editing { .. }
            | BrowserView::Loading { .. }
            | BrowserView::Error { .. } => Vec::new(),
        }
    }

    pub fn apply(&mut self, command: BrowserCommand) -> BrowserEffect {
        match command {
            BrowserCommand::OpenSearch => {
                self.cancel_pending();
                self.history.clear();
                self.view = BrowserView::Editing {
                    query: String::new(),
                };
                BrowserEffect::None
            }
            BrowserCommand::OpenHome => {
                self.cancel_pending();
                self.history.clear();
                self.home_is_root = true;
                self.view = BrowserView::Home;
                BrowserEffect::LoadHome { force: false }
            }
            BrowserCommand::RefreshHome => {
                if !matches!(self.view, BrowserView::Home) || self.home.is_loading() {
                    return BrowserEffect::None;
                }
                self.home.mark_loading();
                BrowserEffect::LoadHome { force: true }
            }
            BrowserCommand::Insert(character) => {
                if let BrowserView::Editing { query } = &mut self.view {
                    query.push(character);
                }
                BrowserEffect::None
            }
            BrowserCommand::Paste(text) => {
                if let BrowserView::Editing { query } = &mut self.view {
                    query.extend(
                        text.trim()
                            .chars()
                            .filter(|character| !character.is_control()),
                    );
                }
                BrowserEffect::None
            }
            BrowserCommand::Backspace => {
                if let BrowserView::Editing { query } = &mut self.view {
                    query.pop();
                }
                BrowserEffect::None
            }
            BrowserCommand::Submit => self.submit_search(),
            BrowserCommand::Previous => {
                self.move_selection(-1);
                BrowserEffect::None
            }
            BrowserCommand::Next => {
                self.move_selection(1);
                BrowserEffect::None
            }
            BrowserCommand::Activate => self.activate_selection(),
            BrowserCommand::Back => {
                self.back();
                BrowserEffect::None
            }
            BrowserCommand::Close => {
                self.leave();
                BrowserEffect::None
            }
            BrowserCommand::Quit => BrowserEffect::Quit,
        }
    }

    pub fn resolve(&mut self, event: CatalogEvent) {
        match event {
            CatalogEvent::Loaded { request_id, page }
                if self.pending_request_id == Some(request_id) =>
            {
                self.pending_request_id = None;
                if self.pending_pushes_history
                    && let Some(BrowserView::Page { page, selected }) =
                        self.fallback.take().map(|view| *view)
                {
                    self.history.push((page, selected));
                } else {
                    self.fallback = None;
                    self.history.clear();
                }
                self.view = BrowserView::Page { page, selected: 0 };
                self.pending_pushes_history = false;
            }
            CatalogEvent::Failed {
                request_id,
                message,
            } if self.pending_request_id == Some(request_id) => {
                self.pending_request_id = None;
                self.view = BrowserView::Error { message };
                self.pending_pushes_history = false;
            }
            CatalogEvent::HomeUpdated(update) => self.apply_home_update(update),
            CatalogEvent::Loaded { .. }
            | CatalogEvent::Failed { .. }
            | CatalogEvent::PlaybackFailed { .. }
            | CatalogEvent::PlaybackFallback { .. } => {}
        }
    }

    // Sections fill in while the user browses, so the cursor follows its item
    // rather than its index.
    fn apply_home_update(&mut self, update: HomeUpdate) {
        let selected_uri = self
            .home
            .item(self.home_selected)
            .map(|item| item.uri().to_owned());
        self.home.apply(update);
        self.home_selected = selected_uri
            .and_then(|uri| self.home.position(&uri))
            .unwrap_or(self.home_selected)
            .min(self.home.item_count().saturating_sub(1));
    }

    fn submit_search(&mut self) -> BrowserEffect {
        let BrowserView::Editing { query } = &self.view else {
            return BrowserEffect::None;
        };
        let query = query.trim();
        if query.is_empty() {
            return BrowserEffect::None;
        }
        self.begin_request(
            CatalogRequest::Search(query.to_owned()),
            format!("Searching for {query}… Spotify sign-in opens in your browser if needed."),
            false,
        )
    }

    fn move_selection(&mut self, delta: isize) {
        if matches!(self.view, BrowserView::Home) {
            self.home_selected = self
                .home_selected
                .saturating_add_signed(delta)
                .min(self.home.item_count().saturating_sub(1));
            return;
        }
        let BrowserView::Page { page, selected } = &mut self.view else {
            return;
        };
        let item_count = page.items().len();
        if item_count == 0 {
            *selected = 0;
            return;
        }
        *selected = selected
            .saturating_add_signed(delta)
            .min(item_count.saturating_sub(1));
    }

    fn activate_selection(&mut self) -> BrowserEffect {
        let item = match &self.view {
            BrowserView::Page { page, selected } => page.items().get(*selected).cloned(),
            BrowserView::Home => self.home.item(self.home_selected).cloned(),
            BrowserView::Closed
            | BrowserView::Editing { .. }
            | BrowserView::Loading { .. }
            | BrowserView::Error { .. } => None,
        };
        let Some(item) = item else {
            return BrowserEffect::None;
        };
        match item.kind() {
            CatalogItemKind::Artist => self.begin_request(
                CatalogRequest::Artist(item.id().to_owned()),
                format!("Loading {}…", item.name()),
                true,
            ),
            CatalogItemKind::Album => self.begin_request(
                CatalogRequest::Album(item.id().to_owned()),
                format!("Loading {}…", item.name()),
                true,
            ),
            CatalogItemKind::Track => {
                let playback = item.playback();
                self.close();
                BrowserEffect::PlayTrack(playback)
            }
            CatalogItemKind::Playlist => {
                let uri = item.uri().to_owned();
                self.close();
                BrowserEffect::Play(uri)
            }
        }
    }

    fn begin_request(
        &mut self,
        request: CatalogRequest,
        label: String,
        pushes_history: bool,
    ) -> BrowserEffect {
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request_id = self.next_request_id;
        let previous = std::mem::replace(&mut self.view, BrowserView::Loading { label });
        self.fallback = Some(Box::new(previous));
        self.pending_request_id = Some(request_id);
        self.pending_pushes_history = pushes_history;
        BrowserEffect::Fetch {
            request_id,
            request,
        }
    }

    fn back(&mut self) {
        match self.view {
            BrowserView::Loading { .. } | BrowserView::Error { .. } => {
                if let Some(view) = self.fallback.take() {
                    self.view = *view;
                } else {
                    self.leave();
                }
                self.pending_request_id = None;
                self.pending_pushes_history = false;
            }
            BrowserView::Page { .. } => {
                if let Some((page, selected)) = self.history.pop() {
                    self.view = BrowserView::Page { page, selected };
                } else {
                    self.leave();
                }
            }
            BrowserView::Home | BrowserView::Closed => self.close(),
            BrowserView::Editing { .. } => self.leave(),
        }
    }

    fn leave(&mut self) {
        if self.home_is_root {
            self.cancel_pending();
            self.history.clear();
            self.view = BrowserView::Home;
        } else {
            self.close();
        }
    }

    fn close(&mut self) {
        self.cancel_pending();
        self.history.clear();
        self.home_is_root = false;
        self.view = BrowserView::Closed;
    }

    fn cancel_pending(&mut self) {
        self.pending_request_id = None;
        self.pending_pushes_history = false;
        self.fallback = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        catalog::{CatalogItem, CatalogItemKind},
        home::HomeSectionStatus,
    };

    fn item(kind: CatalogItemKind, id: &str) -> CatalogItem {
        CatalogItem::new(
            kind,
            id,
            format!("spotify:{}:{id}", kind.to_string().to_ascii_lowercase()),
            format!("Item {id}"),
            kind.to_string(),
            None,
        )
    }

    fn item_with_artwork(kind: CatalogItemKind, id: &str) -> CatalogItem {
        CatalogItem::new(
            kind,
            id,
            format!("spotify:{}:{id}", kind.to_string().to_ascii_lowercase()),
            format!("Item {id}"),
            kind.to_string(),
            Some(format!("https://example.com/{id}.jpg")),
        )
    }

    fn home_update(kind: HomeSectionKind, items: Vec<CatalogItem>) -> CatalogEvent {
        CatalogEvent::HomeUpdated(HomeUpdate {
            kind,
            items,
            status: HomeSectionStatus::Ready,
        })
    }

    fn home_browser() -> BrowserState {
        let mut browser = BrowserState::default();
        browser.set_home_sections(&[
            HomeSectionKind::RecentlyPlayed,
            HomeSectionKind::NewReleases,
        ]);
        browser.resolve(home_update(
            HomeSectionKind::RecentlyPlayed,
            vec![item(CatalogItemKind::Track, "recent")],
        ));
        browser.resolve(home_update(
            HomeSectionKind::NewReleases,
            vec![item_with_artwork(CatalogItemKind::Album, "release")],
        ));
        browser.apply(BrowserCommand::OpenHome);
        browser
    }

    fn search_page(items: Vec<CatalogItem>) -> CatalogPage {
        CatalogPage::Search {
            query: "shikari".to_owned(),
            items,
        }
    }

    #[test]
    fn typing_and_submitting_produces_one_search_request() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        for character in "enter shikari".chars() {
            browser.apply(BrowserCommand::Insert(character));
        }

        assert_eq!(
            browser.apply(BrowserCommand::Submit),
            BrowserEffect::Fetch {
                request_id: 1,
                request: CatalogRequest::Search("enter shikari".to_owned()),
            }
        );
        assert!(matches!(browser.view(), BrowserView::Loading { .. }));
    }

    #[test]
    fn bracketed_paste_populates_the_search_editor() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Paste("enter shikari\n".to_owned()));

        assert_eq!(
            browser.view(),
            &BrowserView::Editing {
                query: "enter shikari".to_owned(),
            }
        );
    }

    #[test]
    fn selecting_an_artist_fetches_its_page_and_back_restores_results() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(vec![item(CatalogItemKind::Artist, "artist")]),
        });

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::Fetch {
                request_id: 2,
                request: CatalogRequest::Artist("artist".to_owned()),
            }
        );
        browser.resolve(CatalogEvent::Loaded {
            request_id: 2,
            page: CatalogPage::Artist {
                artist: item(CatalogItemKind::Artist, "artist"),
                releases: vec![item(CatalogItemKind::Album, "album")],
            },
        });
        browser.apply(BrowserCommand::Back);
        assert!(matches!(
            browser.view(),
            BrowserView::Page {
                page: CatalogPage::Search { .. },
                ..
            }
        ));
    }

    #[test]
    fn artist_page_artwork_tracks_the_selected_release() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(vec![item_with_artwork(CatalogItemKind::Artist, "artist")]),
        });
        browser.apply(BrowserCommand::Activate);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 2,
            page: CatalogPage::Artist {
                artist: item_with_artwork(CatalogItemKind::Artist, "artist"),
                releases: vec![
                    item_with_artwork(CatalogItemKind::Album, "first"),
                    item_with_artwork(CatalogItemKind::Album, "second"),
                ],
            },
        });
        assert_eq!(browser.artwork_url(), Some("https://example.com/first.jpg"));

        browser.apply(BrowserCommand::Next);

        assert_eq!(
            browser.artwork_url(),
            Some("https://example.com/second.jpg")
        );
        assert_eq!(
            browser.prefetch_artwork_urls(),
            vec!["https://example.com/first.jpg"]
        );
    }

    #[test]
    fn selecting_a_track_closes_the_browser_and_returns_its_uri() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(vec![item(CatalogItemKind::Track, "track")]),
        });

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::PlayTrack(item(CatalogItemKind::Track, "track").playback())
        );
        assert_eq!(browser.mode(), BrowserMode::Closed);
    }

    #[test]
    fn selecting_the_second_album_track_plays_the_second_track_uri() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: CatalogPage::Album {
                album: item(CatalogItemKind::Album, "album"),
                tracks: vec![
                    item(CatalogItemKind::Track, "first"),
                    item(CatalogItemKind::Track, "second"),
                    item(CatalogItemKind::Track, "third"),
                ],
            },
        });
        browser.apply(BrowserCommand::Next);

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::PlayTrack(item(CatalogItemKind::Track, "second").playback())
        );
    }

    #[test]
    fn artist_album_track_flow_ends_in_local_playback() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(vec![item(CatalogItemKind::Artist, "artist")]),
        });

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::Fetch {
                request_id: 2,
                request: CatalogRequest::Artist("artist".to_owned()),
            }
        );
        browser.resolve(CatalogEvent::Loaded {
            request_id: 2,
            page: CatalogPage::Artist {
                artist: item(CatalogItemKind::Artist, "artist"),
                releases: vec![item(CatalogItemKind::Album, "album")],
            },
        });
        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::Fetch {
                request_id: 3,
                request: CatalogRequest::Album("album".to_owned()),
            }
        );
        browser.resolve(CatalogEvent::Loaded {
            request_id: 3,
            page: CatalogPage::Album {
                album: item(CatalogItemKind::Album, "album"),
                tracks: vec![item(CatalogItemKind::Track, "track")],
            },
        });

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::PlayTrack(item(CatalogItemKind::Track, "track").playback())
        );
        assert_eq!(browser.mode(), BrowserMode::Closed);
    }

    #[test]
    fn opening_home_requests_a_load_that_may_use_the_cache() {
        let mut browser = BrowserState::default();

        assert_eq!(
            browser.apply(BrowserCommand::OpenHome),
            BrowserEffect::LoadHome { force: false }
        );
        assert_eq!(browser.mode(), BrowserMode::Home);
    }

    #[test]
    fn home_navigates_across_sections_and_previews_the_selected_artwork() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);
        browser.apply(BrowserCommand::Next);

        assert_eq!(browser.home_selected(), 1);
        assert_eq!(
            browser.artwork_url(),
            Some("https://example.com/release.jpg")
        );
    }

    #[test]
    fn a_home_release_opens_its_album_and_back_returns_to_home() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::Fetch {
                request_id: 1,
                request: CatalogRequest::Album("release".to_owned()),
            }
        );
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: CatalogPage::Album {
                album: item(CatalogItemKind::Album, "release"),
                tracks: vec![item(CatalogItemKind::Track, "track")],
            },
        });
        browser.apply(BrowserCommand::Back);

        assert_eq!(browser.view(), &BrowserView::Home);
        assert_eq!(browser.home_selected(), 1);
    }

    #[test]
    fn a_failed_home_item_returns_to_home() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);
        browser.apply(BrowserCommand::Activate);
        browser.resolve(CatalogEvent::Failed {
            request_id: 1,
            message: "quota".to_owned(),
        });

        browser.apply(BrowserCommand::Back);

        assert_eq!(browser.view(), &BrowserView::Home);
    }

    #[test]
    fn playing_from_home_closes_to_now_playing() {
        let mut browser = home_browser();

        assert_eq!(
            browser.apply(BrowserCommand::Activate),
            BrowserEffect::PlayTrack(item(CatalogItemKind::Track, "recent").playback())
        );
        assert_eq!(browser.mode(), BrowserMode::Closed);
    }

    #[test]
    fn back_from_home_reaches_now_playing_and_search_returns_where_it_began() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Close);
        assert_eq!(browser.view(), &BrowserView::Home);

        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(Vec::new()),
        });
        browser.apply(BrowserCommand::Back);
        assert_eq!(browser.view(), &BrowserView::Home);

        browser.apply(BrowserCommand::Back);
        assert_eq!(browser.mode(), BrowserMode::Closed);

        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Close);
        assert_eq!(browser.mode(), BrowserMode::Closed);
    }

    #[test]
    fn home_can_be_reached_from_deep_in_the_catalogue() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);
        browser.apply(BrowserCommand::Activate);

        assert_eq!(
            browser.apply(BrowserCommand::OpenHome),
            BrowserEffect::LoadHome { force: false }
        );
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(Vec::new()),
        });

        assert_eq!(browser.view(), &BrowserView::Home);
    }

    #[test]
    fn the_cursor_stays_on_its_item_as_sections_fill_in() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);

        browser.resolve(home_update(
            HomeSectionKind::RecentlyPlayed,
            vec![
                item(CatalogItemKind::Track, "new-first"),
                item(CatalogItemKind::Track, "recent"),
            ],
        ));

        assert_eq!(browser.home_selected(), 2);
        assert_eq!(browser.home().item(2).map(CatalogItem::id), Some("release"));
    }

    #[test]
    fn the_cursor_stays_in_range_when_its_item_disappears() {
        let mut browser = home_browser();
        browser.apply(BrowserCommand::Next);

        browser.resolve(home_update(HomeSectionKind::NewReleases, Vec::new()));

        assert_eq!(browser.home_selected(), 0);
    }

    #[test]
    fn refreshing_marks_sections_and_forces_one_load() {
        let mut browser = home_browser();

        assert_eq!(
            browser.apply(BrowserCommand::RefreshHome),
            BrowserEffect::LoadHome { force: true }
        );
        assert!(browser.home().is_loading());
        assert_eq!(browser.home().item_count(), 2);
        assert_eq!(
            browser.apply(BrowserCommand::RefreshHome),
            BrowserEffect::None
        );
    }

    #[test]
    fn stale_catalogue_results_are_ignored_after_closing() {
        let mut browser = BrowserState::default();
        browser.apply(BrowserCommand::OpenSearch);
        browser.apply(BrowserCommand::Insert('x'));
        browser.apply(BrowserCommand::Submit);
        browser.apply(BrowserCommand::Close);
        browser.resolve(CatalogEvent::Loaded {
            request_id: 1,
            page: search_page(Vec::new()),
        });

        assert_eq!(browser.mode(), BrowserMode::Closed);
    }
}
