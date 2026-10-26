//! Durable reading positions and complete conversation history, including group logs.
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use crate::team::Team;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Reading {
    pub through: Option<String>,
    pub anchor: Option<String>,
    pub offset: f64,
    pub version: u64,
}

pub struct History {
    path: PathBuf,
    reading: Mutex<BTreeMap<String, Reading>>,
}

impl History {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("reading.json");
        let reading = match crate::sqlite_storage::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, reading: Mutex::new(reading) })
    }

    async fn save(&self, conversation: &str, update: impl FnOnce(&mut Reading) -> Result<()>) -> Result<()> {
        use std::io::Write;
        let mut current = self.reading.lock().await;
        let mut next = current.clone();
        update(next.entry(conversation.into()).or_default())?;
        next.get_mut(conversation).unwrap().version += 1;
        if let Some(result) = crate::sqlite_storage::save(&self.path, &serde_json::to_vec(&next)?) {
            result?; *current = next; return Ok(());
        }
        let temp = self.path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).truncate(true).create(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp)?;
        file.write_all(&serde_json::to_vec(&next)?)?;
        file.sync_all()?;
        std::fs::rename(temp, &self.path)?;
        *current = next;
        Ok(())
    }
}

impl History {
    /// After a conversation's history is cleared there is nothing left to have read.
    pub async fn forget(&self, conversation: &str) -> Result<()> {
        self.save(conversation, |reading| { reading.through = None; reading.anchor = None; reading.offset = 0.0; Ok(()) }).await
    }
}

fn visible(item: &Value) -> bool {
    matches!(item["kind"].as_str(), Some("user" | "bot" | "incoming" | "file" | "notice"))
}

/// Tool traces stay private, except a message to another Bot: the page shows it as a
/// "Message to ▢ Name" handoff with the recipient, the text sent and its status only.
pub fn public_tool(item: &Value) -> Option<Value> {
    if item["kind"] != "tool" || !item["name"].as_str().is_some_and(|n| n.ends_with("__message_bot")) { return None; }
    let input = &item["input"];
    let mut public = json!({ "id": item["id"], "time": item["time"], "kind": "tool", "bot": item["bot"], "name": item["name"],
        "input": { "bot": input["bot"], "text": input["text"], "reply_to": input["reply_to"] }, "target": item["target"], "done": item["done"], "error": item["error"] });
    if item["error"] == true { public["result"] = item["result"].clone(); }
    Some(public)
}

fn unread_item(item: &Value) -> bool {
    matches!(item["kind"].as_str(), Some("bot" | "incoming" | "file")) || (item["kind"] == "notice" && item["error"] == true)
}

/// The task id in a message_bot receipt: "Sent to kk (task task-…)".
fn receipt_task(result: &str) -> Option<&str> {
    let at = result.find("(task ")? + 6;
    result[at..].split(')').next().filter(|id| !id.is_empty())
}

impl Team {
    /// A handoff keeps its recipient's id, so it still shows that Bot after a rename:
    /// from the delegated task when the receipt names one, else by the name sent.
    pub(crate) fn handoff_target(&self, item: &mut Value, state: &crate::store::State) {
        if item["kind"] != "tool" || item["target"].is_string() || !item["name"].as_str().is_some_and(|n| n.ends_with("__message_bot")) { return; }
        let by_task = item["result"].as_str().and_then(receipt_task).and_then(|id| self.tasks.get(id)).map(|t| t.target);
        let name = item["input"]["bot"].as_str().unwrap_or_default().trim().to_lowercase();
        let by_name = || state.bots.values().find(|b| b.name.to_lowercase() == name || b.id == name).map(|b| b.id.clone());
        if let Some(id) = by_task.or_else(by_name) { item["target"] = json!(id); }
    }

    pub async fn history_items(&self, conversation: &str) -> Vec<Value> {
        let mut items = self.chats.read(conversation).await;
        if let Some(room) = conversation.strip_prefix("room:").filter(|r| !r.contains(':')) {
            let state = self.state().await;
            if let Some(room) = state.rooms.get(room) {
                // Old releases kept only group text in state.json and member file logs
                // separately. Seeded text is durable; merge legacy member files as well.
                items.extend(room.log.iter().map(crate::team::log_item));
                for bot in &room.members {
                    items.extend(self.chats.read(&format!("room:{}:{bot}", room.id)).await.into_iter().filter(|i| i["kind"] == "file"));
                }
            }
        }
        let mut seen = HashSet::new();
        let state = self.state().await;
        let mut items: Vec<Value> = items.into_iter().filter_map(|mut i| if visible(&i) { Some(i) } else { self.handoff_target(&mut i, &state); public_tool(&i) }).collect();
        items.retain(|i| seen.insert(i["id"].as_str().unwrap_or_default().to_string()));
        for item in &mut items { crate::media_gen::public_file(item); }
        items.sort_by_key(|i| i["time"].as_i64().unwrap_or(0));
        items
    }

    pub async fn history_meta(&self, conversation: &str) -> Value {
        let reading = self.history.reading.lock().await.get(conversation).cloned().unwrap_or_default();
        let items = self.history_items(conversation).await;
        let start = reading.through.as_ref().and_then(|id| items.iter().position(|i| i["id"] == *id)).map_or(0, |at| at + 1);
        let unread = items[start..].iter().filter(|i| unread_item(i)).count();
        let last = items.iter().rev().find(|i| matches!(i["kind"].as_str(), Some("user" | "bot" | "file")));
        json!({ "conversation": conversation, "unread": unread, "reading": reading, "total": items.len(), "last": items.last().map(|i| &i["id"]), "preview": last.map(|i| json!({ "text": (if i["kind"] == "file" { i["name"].as_str() } else { i["text"].as_str() }).unwrap_or_default().chars().take(140).collect::<String>(), "at": i["time"] })) })
    }

    pub async fn history_page(&self, conversation: &str, before: Option<usize>, after: Option<usize>, anchor: Option<&str>) -> Value {
        let items = self.history_items(conversation).await;
        let (start, end) = if let Some(at) = anchor.and_then(|id| items.iter().position(|i| i["id"] == id)) {
            (at.saturating_sub(80), (at + 120).min(items.len()))
        } else if let Some(start) = after {
            let start = start.min(items.len());
            (start, (start + 200).min(items.len()))
        } else {
            let end = before.unwrap_or(items.len()).min(items.len());
            (end.saturating_sub(200), end)
        };
        json!({ "items": items[start..end], "more": (start > 0).then_some(start), "newer": (end < items.len()).then_some(end), "total": items.len() })
    }

    pub async fn history_search(&self, conversation: &str, query: &str, offset: usize) -> Value {
        let query = query.trim().to_lowercase();
        if query.is_empty() { return json!({ "total": 0, "matches": [] }); }
        let matches: Vec<_> = self.history_items(conversation).await.into_iter().filter(|i| {
            let fields = format!("{} {} {}", i["text"].as_str().unwrap_or_default(), i["name"].as_str().unwrap_or_default(), i["files"].as_array().into_iter().flatten().filter_map(|f| f["name"].as_str()).collect::<Vec<_>>().join(" "));
            fields.to_lowercase().contains(&query)
        }).collect();
        json!({ "total": matches.len(), "matches": matches.iter().skip(offset).take(1).map(|i| json!({ "id": i["id"], "time": i["time"] })).collect::<Vec<_>>() })
    }

    pub async fn history_read(&self, conversation: &str, through: Option<&str>, anchor: Option<&str>, offset: f64) -> Result<Value> {
        self.check_conversation(conversation).await?;
        let items = self.history_items(conversation).await;
        let position = |id: &str| items.iter().position(|i| i["id"] == id);
        self.history.save(conversation, |reading| {
            if let Some(id) = through {
                let at = position(id).ok_or_else(|| anyhow!("Unknown message"))?;
                let old = reading.through.as_deref().and_then(position);
                if old.is_none_or(|old| at > old) { reading.through = Some(id.into()); }
            }
            if let Some(id) = anchor {
                if position(id).is_none() { return Err(anyhow!("Unknown reading position")); }
                reading.anchor = Some(id.into());
                reading.offset = if offset.is_finite() { offset.clamp(-100_000.0, 100_000.0) } else { 0.0 };
            }
            Ok(())
        }).await?;
        let meta = self.history_meta(conversation).await;
        self.publish("chat.meta", meta.clone());
        Ok(meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_messages_to_bots_leave_tool_traces_and_only_their_public_fields() {
        let sent = json!({ "id": "t1", "time": 1, "kind": "tool", "bot": "b1", "name": "mcp__teambot__message_bot", "input": { "bot": "kk", "text": "查一下", "context": "private notes", "files": ["media/x.pdf"] }, "done": true, "error": false, "result": "receipt r1" });
        assert_eq!(receipt_task("Sent to kk (task task-a92f). Its reply returns automatically."), Some("task-a92f"));
        assert_eq!(receipt_task("Created kk"), None);
        let public = public_tool(&sent).unwrap();
        assert_eq!(public["input"]["bot"], "kk");
        assert_eq!(public["input"]["text"], "查一下");
        assert!(public["input"].get("context").is_none() && public["input"].get("files").is_none() && public.get("result").is_none());
        assert!(public_tool(&json!({ "kind": "tool", "name": "mcp__teambot__desktop_run", "input": { "command": "ls" } })).is_none());
        assert!(public_tool(&json!({ "kind": "bot", "name": "mcp__teambot__message_bot" })).is_none());
    }
    #[tokio::test]
    async fn reading_positions_survive_reopen_and_failed_writes_do_not_change_memory() {
        let home = tempfile::tempdir().unwrap();
        let history = History::open(home.path()).unwrap();
        history.save("bot:a", |r| { r.through = Some("message-20".into()); r.anchor = Some("message-10".into()); r.offset = -4.0; Ok(()) }).await.unwrap();
        drop(history);
        let history = History::open(home.path()).unwrap();
        assert_eq!(history.reading.lock().await["bot:a"].through.as_deref(), Some("message-20"));
        assert_eq!(history.reading.lock().await["bot:a"].offset, -4.0);
        std::fs::create_dir(home.path().join("reading.json.tmp")).unwrap();
        assert!(history.save("bot:a", |r| { r.through = Some("message-99".into()); Ok(()) }).await.is_err());
        assert_eq!(history.reading.lock().await["bot:a"].through.as_deref(), Some("message-20"));
    }
}
