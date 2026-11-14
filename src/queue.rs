//! Queue snapshots and edits shared with the authenticated Spotifyd control protocol.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEntry {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSnapshot {
    pub revision: String,
    pub entries: Vec<QueueEntry>,
    pub editable: bool,
    #[serde(default)]
    pub context_count: usize,
    #[serde(default)]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueEdit {
    pub revision: String,
    #[serde(flatten)]
    pub action: QueueAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QueueAction {
    Remove { index: usize },
    MoveUp { index: usize },
    MoveDown { index: usize },
    Clear,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueState {
    pub snapshot: Option<QueueSnapshot>,
    pub selected: usize,
    pub pending: Option<u64>,
    pub error: Option<String>,
    pub confirm_clear: bool,
}

impl QueueState {
    pub fn edit(&self, action: QueueAction) -> Option<QueueEdit> {
        let snapshot = self.snapshot.as_ref()?;
        if self.pending.is_some()
            || self.error.is_some()
            || !snapshot.editable
            || snapshot.entries.is_empty()
        {
            return None;
        }
        let count = snapshot.entries.len();
        match action {
            QueueAction::Remove { index } if index >= count => return None,
            QueueAction::MoveUp { index } if index == 0 || index >= count => return None,
            QueueAction::MoveDown { index } if index.saturating_add(1) >= count => return None,
            _ => {}
        }
        Some(QueueEdit {
            revision: snapshot.revision.clone(),
            action,
        })
    }

    pub fn resolve(&mut self, request_id: u64, result: Result<QueueSnapshot, String>) {
        if self.pending != Some(request_id) {
            return;
        }
        self.pending = None;
        self.confirm_clear = false;
        match result {
            Ok(snapshot) => {
                self.selected = self.selected.min(snapshot.entries.len().saturating_sub(1));
                self.snapshot = Some(snapshot);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}
