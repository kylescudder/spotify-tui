//! Local edits use the same queue replacement path as Spotify Connect clients.
use crate::{
    core::{Error, dealer::protocol::SetQueueCommand},
    protocol::player::PlayerState,
    state::provider::IsProvider,
};
use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

pub(crate) fn snapshot(player: &PlayerState) -> Value {
    let mut hash = DefaultHasher::new();
    player.context_uri.hash(&mut hash);
    player.track.uri.hash(&mut hash);
    player.track.uid.hash(&mut hash);
    for track in &player.next_tracks {
        (&track.uri, &track.uid, &track.provider).hash(&mut hash);
    }
    json!({
        "revision": hash.finish().to_string(),
        "editable": true,
        "context_count": player.next_tracks.iter().filter(|t| !t.is_queue()).count(),
        "entries": player.next_tracks.iter().filter(|t| t.is_queue()).map(|track| json!({
            "uri": track.uri,
            "name": track.metadata.get("title").unwrap_or(&track.uri),
            "detail": track.metadata.get("artist_name").cloned().unwrap_or_default(),
        })).collect::<Vec<_>>()
    })
}

pub(crate) fn edit(player: &PlayerState, edit: &Value) -> Result<SetQueueCommand, Error> {
    if edit.get("revision").and_then(Value::as_str) != snapshot(player)["revision"].as_str() {
        return Err(Error::aborted(
            "Queue changed. Press R to refresh before editing.",
        ));
    }
    let mut next_tracks = player.next_tracks.clone();
    let queued: Vec<_> = next_tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_queue())
        .map(|(index, _)| index)
        .collect();
    let action = edit
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if action == "clear" {
        next_tracks.retain(|t| !t.is_queue());
    } else {
        let index = edit
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| Error::invalid_argument("Missing queue index"))?;
        let actual = *queued
            .get(index)
            .ok_or_else(|| Error::invalid_argument("Queue index out of range"))?;
        match action {
            "remove" => {
                next_tracks.remove(actual);
            }
            "move_up" if index > 0 => next_tracks.swap(actual, queued[index - 1]),
            "move_down" if index + 1 < queued.len() => next_tracks.swap(actual, queued[index + 1]),
            _ => {
                return Err(Error::invalid_argument(
                    "Invalid queue action or move boundary",
                ));
            }
        }
    }
    Ok(SetQueueCommand {
        next_tracks,
        prev_tracks: player.prev_tracks.clone(),
        queue_revision: player.queue_revision.clone(),
        logging_params: serde_json::from_value(json!({})).map_err(Error::invalid_argument)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::player::ProvidedTrack;
    fn player() -> PlayerState {
        PlayerState {
            next_tracks: [
                ("first", "queue"),
                ("second", "queue"),
                ("automatic", "context"),
            ]
            .into_iter()
            .map(|(uri, provider)| ProvidedTrack {
                uri: uri.into(),
                uid: uri.into(),
                provider: provider.into(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn moving_duplicates_and_clearing_preserve_context_and_playback_history() {
        let mut player = player();
        player.next_tracks[1].uri = "first".into();
        player.prev_tracks = vec![player.next_tracks[2].clone()];
        let revision = snapshot(&player)["revision"].clone();
        let down = edit(
            &player,
            &json!({"revision":revision,"action":"move_down","index":0}),
        )
        .unwrap();
        assert_eq!(down.next_tracks[0].uid, "second");
        assert_eq!(down.prev_tracks, player.prev_tracks);
        assert_eq!(down.next_tracks[2], player.next_tracks[2]);
        let clear = edit(&player, &json!({"revision":revision,"action":"clear"})).unwrap();
        assert_eq!(clear.next_tracks.len(), 1);
        assert_eq!(clear.next_tracks[0].uri, "automatic");
        player.next_tracks.swap(0, 1);
        assert!(
            edit(
                &player,
                &json!({"revision":revision,"action":"remove","index":0})
            )
            .is_err()
        );
        let revision = snapshot(&player)["revision"].clone();
        for (action, index) in [("move_up", 0), ("move_down", 1), ("remove", 99)] {
            assert!(
                edit(
                    &player,
                    &json!({"revision":revision,"action":action,"index":index})
                )
                .is_err()
            );
        }
    }

    #[test]
    fn local_queue_edits_preserve_automatic_tracks_and_reject_stale_snapshots() {
        let player = player();
        let revision = snapshot(&player)["revision"].clone();
        let updated = edit(
            &player,
            &json!({"revision":revision,"action":"remove","index":0}),
        )
        .unwrap();
        assert_eq!(
            updated
                .next_tracks
                .iter()
                .map(|t| t.uri.as_str())
                .collect::<Vec<_>>(),
            ["second", "automatic"]
        );
        assert_eq!(player.next_tracks[0].uri, "first");
        assert!(
            edit(
                &player,
                &json!({"revision":"stale","action":"remove","index":0})
            )
            .is_err()
        );
    }
}
