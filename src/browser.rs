use crate::catalog::{CatalogEvent, CatalogItemKind, CatalogPage, CatalogRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserMode {
    Closed,
    Editing,
    Page,
    Waiting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCommand {
    OpenSearch,
    Insert(char),
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
    Play(String),
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserView {
    Closed,
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
        }
    }
}

impl BrowserState {
    pub const fn view(&self) -> &BrowserView {
        &self.view
    }

    pub const fn mode(&self) -> BrowserMode {
        match self.view {
            BrowserView::Closed => BrowserMode::Closed,
            BrowserView::Editing { .. } => BrowserMode::Editing,
            BrowserView::Page { .. } => BrowserMode::Page,
            BrowserView::Loading { .. } | BrowserView::Error { .. } => BrowserMode::Waiting,
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
            BrowserCommand::Insert(character) => {
                if let BrowserView::Editing { query } = &mut self.view {
                    query.push(character);
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
                self.close();
                BrowserEffect::None
            }
            BrowserCommand::Quit => BrowserEffect::Quit,
        }
    }

    pub fn resolve(&mut self, event: CatalogEvent) {
        let request_id = match &event {
            CatalogEvent::Loaded { request_id, .. } | CatalogEvent::Failed { request_id, .. } => {
                *request_id
            }
        };
        if self.pending_request_id != Some(request_id) {
            return;
        }
        self.pending_request_id = None;
        match event {
            CatalogEvent::Loaded { page, .. } => {
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
            }
            CatalogEvent::Failed { message, .. } => {
                self.view = BrowserView::Error { message };
            }
        }
        self.pending_pushes_history = false;
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
        let BrowserView::Page { page, selected } = &self.view else {
            return BrowserEffect::None;
        };
        let Some(item) = page.items().get(*selected).cloned() else {
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
            CatalogItemKind::Track | CatalogItemKind::Playlist => {
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
                    self.close();
                }
                self.pending_request_id = None;
                self.pending_pushes_history = false;
            }
            BrowserView::Page { .. } => {
                if let Some((page, selected)) = self.history.pop() {
                    self.view = BrowserView::Page { page, selected };
                } else {
                    self.close();
                }
            }
            BrowserView::Editing { .. } | BrowserView::Closed => self.close(),
        }
    }

    fn close(&mut self) {
        self.cancel_pending();
        self.history.clear();
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
    use crate::catalog::{CatalogItem, CatalogItemKind};

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
            BrowserEffect::Play("spotify:track:track".to_owned())
        );
        assert_eq!(browser.mode(), BrowserMode::Closed);
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
            BrowserEffect::Play("spotify:track:track".to_owned())
        );
        assert_eq!(browser.mode(), BrowserMode::Closed);
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
