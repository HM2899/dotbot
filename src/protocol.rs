//! The window ↔ server protocol, as DS Bot's protocol v2: POST /api/bot `{method,
//! params}` answers `{ok, value}` or `{ok: false, error: {code, message}}`; the feed
//! /api/bot/feed pushes `hello`, `team.patch`, `work`, `chat.item` and `chat.stream`.

use serde_json::{json, Map, Value};

pub const PROTOCOL: u32 = 2;
const KEYED: &[&str] = &["bots", "rooms"];

/// The ops that turn one team view into the next: keyed lists entry by entry (when
/// the order holds and new entries come last), any other key whole.
pub fn diff_view(before: &Value, after: &Value) -> Vec<Value> {
    let empty = Map::new();
    let b = before.as_object().unwrap_or(&empty);
    let a = after.as_object().unwrap_or(&empty);
    let mut ops = Vec::new();
    let mut keys: Vec<&String> = b.keys().collect();
    for key in a.keys() {
        if !b.contains_key(key) {
            keys.push(key);
        }
    }
    for key in keys {
        let Some(next) = a.get(key) else {
            ops.push(json!({ "op": "remove", "path": [key] }));
            continue;
        };
        let prev = b.get(key);
        if let (true, Some(Value::Array(old)), Value::Array(new)) = (KEYED.contains(&key.as_str()), prev, next) {
            let id = |v: &Value| v["id"].as_str().unwrap_or_default().to_string();
            let old_ids: Vec<String> = old.iter().map(id).collect();
            let new_ids: Vec<String> = new.iter().map(id).collect();
            let kept: Vec<String> = old_ids.iter().filter(|i| new_ids.contains(i)).cloned().collect();
            let added: Vec<String> = new_ids.iter().filter(|i| !old_ids.contains(i)).cloned().collect();
            if new_ids != [kept.clone(), added].concat() {
                ops.push(json!({ "op": "set", "path": [key], "value": next }));
                continue;
            }
            for gone in old_ids.iter().filter(|i| !new_ids.contains(i)) {
                ops.push(json!({ "op": "remove", "path": [key, gone] }));
            }
            for entry in new {
                let i = id(entry);
                if old.iter().find(|o| id(o) == i) != Some(entry) {
                    ops.push(json!({ "op": "set", "path": [key, i], "value": entry }));
                }
            }
        } else if prev != Some(next) {
            ops.push(json!({ "op": "set", "path": [key], "value": next }));
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_changed_is_sent() {
        let before = json!({ "revision": 1, "bots": [{"id":"a","name":"A"},{"id":"b","name":"B"}], "rooms": [] });
        let after = json!({ "revision": 2, "bots": [{"id":"a","name":"A"},{"id":"b","name":"Bee"},{"id":"c","name":"C"}], "rooms": [] });
        assert_eq!(diff_view(&before, &after), vec![
            json!({"op":"set","path":["revision"],"value":2}),
            json!({"op":"set","path":["bots","b"],"value":{"id":"b","name":"Bee"}}),
            json!({"op":"set","path":["bots","c"],"value":{"id":"c","name":"C"}}),
        ]);
        let reordered = json!({ "revision": 1, "bots": [{"id":"b","name":"B"},{"id":"a","name":"A"}], "rooms": [] });
        assert_eq!(diff_view(&before, &reordered)[0]["path"], json!(["bots"]));
        assert!(diff_view(&before, &before).is_empty());
    }
}
