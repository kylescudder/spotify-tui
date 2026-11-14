use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use spotify_tui::{
    browser::{BrowserCommand, BrowserEffect, BrowserMode, BrowserState},
    catalog::{
        CatalogError, CatalogEvent, CatalogPage, CatalogRequest, CatalogRuntime, CatalogSource,
    },
    input::browser_command_for_key,
    queue::{QueueAction, QueueEdit, QueueEntry, QueueSnapshot},
};
use std::time::{Duration, Instant};

// This boundary substitutes the external player. Browser state, input mapping,
// worker dispatch and acknowledgement handling all run unchanged.
struct Player {
    step: usize,
}
impl CatalogSource for Player {
    fn fetch(&mut self, _: &CatalogRequest) -> Result<CatalogPage, CatalogError> {
        unreachable!()
    }
    fn manage_queue(&mut self, edit: Option<QueueEdit>) -> Result<QueueSnapshot, CatalogError> {
        let (expected, entries, revision) = match self.step {
            0 => (None, vec!["First", "Second", "Third"], "v1"),
            1 => (
                Some(QueueEdit {
                    revision: "v1".into(),
                    action: QueueAction::MoveDown { index: 0 },
                }),
                vec!["Second", "First", "Third"],
                "v2",
            ),
            2 => (
                Some(QueueEdit {
                    revision: "v2".into(),
                    action: QueueAction::Remove { index: 1 },
                }),
                vec!["Second", "Third"],
                "v3",
            ),
            3 => (
                Some(QueueEdit {
                    revision: "v3".into(),
                    action: QueueAction::Clear,
                }),
                vec![],
                "v4",
            ),
            _ => panic!("unexpected player request"),
        };
        assert_eq!(edit, expected);
        self.step += 1;
        Ok(QueueSnapshot {
            revision: revision.into(),
            editable: true,
            context_count: 5,
            notice: None,
            entries: entries
                .into_iter()
                .map(|name| QueueEntry {
                    uri: format!("spotify:track:{name}"),
                    name: name.into(),
                    detail: "Artist".into(),
                })
                .collect(),
        })
    }
}
fn key(browser: &mut BrowserState, code: char) -> BrowserEffect {
    let command = browser_command_for_key(
        KeyEvent::new(KeyCode::Char(code), KeyModifiers::NONE),
        browser.mode(),
    )
    .unwrap();
    browser.apply(command)
}
fn deliver(browser: &mut BrowserState, runtime: &CatalogRuntime, effect: BrowserEffect) {
    let BrowserEffect::ManageQueue { request_id, edit } = effect else {
        panic!("expected queue request")
    };
    runtime.manage_queue(request_id, edit).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(event) = runtime.try_event() {
            assert!(matches!(event, CatalogEvent::QueueLoaded { .. }));
            browser.resolve(event);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "player acknowledgement timed out"
        );
        std::thread::yield_now();
    }
}
#[test]
fn user_can_view_reorder_remove_and_confirm_clearing_added_tracks() {
    let runtime = CatalogRuntime::start_with_source(Player { step: 0 }).unwrap();
    let mut browser = BrowserState::default();
    let open = key(&mut browser, 'v');
    deliver(&mut browser, &runtime, open);
    assert_eq!(browser.mode(), BrowserMode::Queue);
    let move_down = key(&mut browser, 'J');
    assert_eq!(
        browser.queue().snapshot.as_ref().unwrap().entries[0].name,
        "First"
    );
    deliver(&mut browser, &runtime, move_down);
    assert_eq!(browser.queue().selected, 1);
    assert_eq!(
        browser.queue().snapshot.as_ref().unwrap().entries[1].name,
        "First"
    );
    let remove = key(&mut browser, 'd');
    deliver(&mut browser, &runtime, remove);
    assert_eq!(
        browser
            .queue()
            .snapshot
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["Second", "Third"]
    );
    assert_eq!(key(&mut browser, 'c'), BrowserEffect::None);
    assert!(browser.queue().confirm_clear);
    assert_eq!(key(&mut browser, 'n'), BrowserEffect::None);
    assert!(!browser.queue().confirm_clear);
    key(&mut browser, 'c');
    let clear = key(&mut browser, 'y');
    deliver(&mut browser, &runtime, clear);
    assert!(
        browser
            .queue()
            .snapshot
            .as_ref()
            .unwrap()
            .entries
            .is_empty()
    );
    assert_eq!(browser.queue().snapshot.as_ref().unwrap().context_count, 5);
    browser.apply(BrowserCommand::Back);
    assert_eq!(browser.mode(), BrowserMode::Closed);
}
