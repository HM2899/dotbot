//! The user's verdict on a Bot's reply: right, wrong, or taken back. It is saved on the
//! reply and written into that Bot's memory, so the Bot learns what to repeat and avoid.
//! A retracted reply also leaves the Bot's context: its saved session, the group log and
//! every chat reader skip it until the user restores it.

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::memory::{self, Actor, Scope};
use crate::team::{clip, Team};

/// What stands in the Bot's session for a reply the user took back.
const RETRACTED: &str = "[这条回复已被用户撤回：不要沿用、引用或重复其中的内容。]";

fn lesson(rating: &str, reply: &str, request: &str, reason: &str) -> String {
    let quote = clip(&memory::squash(reply), 80);
    let asked = if request.is_empty() { String::new() } else { format!("用户问「{}」时，", clip(&memory::squash(request), 40)) };
    match rating {
        "good" => format!("{asked}认可这样的回复，继续保持：「{quote}」"),
        _ if !reason.is_empty() => format!("{asked}否定了这样的回复（{}），不要重复：「{quote}」", clip(&memory::squash(reason), 60)),
        _ => format!("{asked}否定了这样的回复，不要重复：「{quote}」"),
    }
}

/// Whether a chat item is a reply the user took back; readers that feed a model skip these.
pub fn retracted(item: &Value) -> bool {
    item["retracted"] == true
}

impl Team {
    /// `rating` is "good", "bad" or "" (clear); `retract` takes the reply back and counts as
    /// bad; `reason` says what was wrong with a bad reply.
    pub async fn rate_reply(&self, conversation: &str, id: &str, rating: &str, retract: bool, reason: &str) -> Result<Value> {
        self.check_conversation(conversation).await?;
        if !matches!(rating, "good" | "bad" | "") { bail!("Unknown rating"); }
        let items = self.chats.read(conversation).await;
        let at = items.iter().position(|i| i["id"] == id).ok_or_else(|| anyhow!("Unknown message"))?;
        let mut item = items[at].clone();
        if item["kind"] != "bot" { bail!("Only a Bot's reply can be rated"); }
        let bot = item["bot"].as_str().unwrap_or_default().to_string();
        let reply = item["text"].as_str().unwrap_or_default().to_string();
        let request = items[..at].iter().rev().find(|i| i["kind"] == "user").and_then(|i| i["text"].as_str()).unwrap_or_default().to_string();
        let previous = item["feedback"].as_str().unwrap_or_default().to_string();
        let previous_reason = item["feedbackReason"].as_str().unwrap_or_default().to_string();
        let rating = if retract { "bad" } else { rating };
        let reason = if rating == "bad" { reason.trim() } else { "" };
        let was_retracted = retracted(&item);
        let execution = item["execution"].as_str().map(str::to_string).unwrap_or_else(|| if conversation.starts_with("room:") { format!("{conversation}:{bot}") } else { conversation.to_string() });
        if retract != was_retracted {
            if self.runs.lock().await.get(&execution).is_some_and(|r| r.running) { bail!("Bot 还在回复，等它说完再撤回或恢复。"); }
            self.scrub_session(&execution, &reply, retract).await;
            self.mark_room_line(conversation, id, retract).await;
        }
        let scope = Scope::Bot(bot.clone());
        let known = self.state().await.bots.get(&bot).is_some();
        if known && (previous != rating || previous_reason != reason) {
            if !previous.is_empty() {
                // Lessons written before the request and reason were part of them are removed too.
                for old in [lesson(&previous, &reply, &request, &previous_reason), lesson(&previous, &reply, "", ""), legacy_lesson(&previous, &reply)] {
                    let _ = self.memory.remove(&scope, None, &old, &Actor::user(), "memory").await;
                }
            }
            if !rating.is_empty() {
                let meta = vec![("source".into(), "feedback".into()), ("added".into(), memory::today())];
                let _ = self.memory.add(&scope, None, &lesson(rating, &reply, &request, reason), &meta, &Actor::user(), "memory").await;
            }
            self.publish("memory", json!({ "scope": scope.key() }));
        }
        item["feedback"] = json!(rating);
        item["feedbackReason"] = json!(reason);
        item["retracted"] = json!(retract);
        self.record(conversation, item.clone()).await;
        Ok(item)
    }

    /// Takes a reply out of (or puts it back into) the Bot's saved model session, so the
    /// next turn neither sees nor builds on what the user retracted.
    async fn scrub_session(&self, execution: &str, reply: &str, retract: bool) {
        let Some(session) = self.team.lock().await.1.sessions.get(execution).cloned() else { return };
        let path = self.cfg.home.join("pi-sessions").join(format!("{session}.json"));
        let Ok(bytes) = std::fs::read(&path) else { return };
        let Ok(mut messages) = serde_json::from_slice::<Vec<Value>>(&bytes) else { return };
        let (from, to) = if retract { (reply.trim().to_string(), RETRACTED.to_string()) } else { (RETRACTED.to_string(), reply.to_string()) };
        // The newest assistant text that matches; the chat copy is redacted, the session's is not.
        let found = messages.iter_mut().rev().filter(|m| m["role"] == "assistant").flat_map(|m| m["content"].as_array_mut().into_iter().flatten()).find(|block| {
            block["type"] == "text" && block["text"].as_str().is_some_and(|text| if retract { self.secrets.redact(text).trim() == from } else { text == from })
        });
        let Some(block) = found else { return };
        block["text"] = json!(to);
        if let Err(error) = crate::durable::save(&path, &messages) { eprintln!("dotbot: retracting a reply from its session: {error}"); }
    }

    /// Group members read each other through the room log: a retracted reply drops out of it.
    async fn mark_room_line(&self, conversation: &str, id: &str, retract: bool) {
        let (Some(room), Some(seq)) = (conversation.strip_prefix("room:").filter(|r| !r.contains(':')), id.strip_prefix("log-").and_then(|s| s.parse::<u64>().ok())) else { return };
        let _ = self.change(|state, _| {
            if let Some(line) = state.rooms.get_mut(room).and_then(|r| r.log.iter_mut().find(|l| l.seq == seq)) {
                if retract { line.extra.insert("retracted".into(), json!(true)); } else { line.extra.remove("retracted"); }
            }
            Ok(())
        }).await;
    }
}

/// The lesson text before it named the request and the reason.
fn legacy_lesson(rating: &str, reply: &str) -> String {
    let quote = clip(&memory::squash(reply), 80);
    match rating {
        "good" => format!("用户认可这类回复，继续保持：「{quote}」"),
        _ => format!("用户否定这类回复，不要重复：「{quote}」"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lessons_name_the_request_and_the_reason() {
        assert_eq!(lesson("good", "好的", "", ""), "认可这样的回复，继续保持：「好的」");
        assert_eq!(lesson("bad", "长篇大论", "今天天气", "太啰嗦"), "用户问「今天天气」时，否定了这样的回复（太啰嗦），不要重复：「长篇大论」");
        assert!(retracted(&json!({ "retracted": true })));
        assert!(!retracted(&json!({ "retracted": false })));
    }
}
