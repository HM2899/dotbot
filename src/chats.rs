//! What each conversation shows: `<home>/chats/<conversation>.jsonl`, one item per line.
//! A tool call is written when it starts and again with its result; the later line wins.
//! Screenshots are files in `<home>/media`, served at /media.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;

use crate::memory::safe_id;

pub struct Chats {
    dir: PathBuf,
    media: PathBuf,
    writing: tokio::sync::Mutex<()>,
}

impl Chats {
    pub fn new(home: &Path) -> Self {
        Chats { dir: home.join("chats"), media: home.join("media"), writing: tokio::sync::Mutex::new(()) }
    }

    /// Migrate the retained group history before State::append starts trimming it.
    pub fn seed_rooms(&self, state: &crate::store::State) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir)?;
        for room in state.rooms.values() {
            let path = self.path(&format!("room:{}", room.id));
            if let Some(existing) = crate::sqlite_storage::read_chat(self.dir.parent().unwrap(), path.file_stem().unwrap().to_str().unwrap()) {
                let ids: std::collections::HashSet<String> = existing?.iter().filter_map(|v| v["id"].as_str().map(String::from)).collect();
                for line in &room.log { let item = crate::team::log_item(line); if !ids.contains(item["id"].as_str().unwrap()) { crate::sqlite_storage::append_chat(self.dir.parent().unwrap(), path.file_stem().unwrap().to_str().unwrap(), &item).unwrap()?; } }
                continue;
            }
            let existing = crate::sqlite_storage::read_to_string(&path).unwrap_or_default();
            let ids: std::collections::HashSet<String> = existing.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| v["id"].as_str().map(String::from)).collect();
            let mut options = std::fs::OpenOptions::new();
            options.append(true).create(true);
            #[cfg(unix)]
            { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
            let mut file = options.open(path)?;
            for line in &room.log {
                let item = crate::team::log_item(line);
                if !ids.contains(item["id"].as_str().unwrap()) { writeln!(file, "{item}")?; }
            }
        }
        Ok(())
    }

    fn path(&self, conversation: &str) -> PathBuf {
        self.dir.join(format!("{}.jsonl", safe_id(&conversation.replace(':', "_"))))
    }

    pub async fn append(&self, conversation: &str, item: &Value) -> Result<()> {
        let _guard = self.writing.lock().await;
        let path = self.path(conversation);
        if let Some(result) = crate::sqlite_storage::append_chat(self.dir.parent().unwrap(), path.file_stem().unwrap().to_str().unwrap(), item) { return result; }
        tokio::fs::create_dir_all(&self.dir).await?;
        let mut file = tokio::fs::OpenOptions::new().create(true).append(true).open(self.path(conversation)).await?;
        file.write_all(format!("{item}\n").as_bytes()).await?;
        // tokio writes in the background: without a flush, dropping the file can lose the line.
        file.flush().await?;
        Ok(())
    }

    /// Deletes every item of one conversation. Attachments in media stay on disk.
    pub async fn clear(&self, conversation: &str) -> Result<()> {
        let _guard = self.writing.lock().await;
        let path = self.path(conversation);
        if let Some(result) = crate::sqlite_storage::clear_chat(self.dir.parent().unwrap(), path.file_stem().unwrap().to_str().unwrap()) { return result; }
        match tokio::fs::remove_file(path).await {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// The items, oldest first; an item written again replaces its earlier line.
    pub async fn read(&self, conversation: &str) -> Vec<Value> {
        let _guard = self.writing.lock().await;
        let path = self.path(conversation);
        if let Some(result) = crate::sqlite_storage::read_chat(self.dir.parent().unwrap(), path.file_stem().unwrap().to_str().unwrap()) {
            return result.unwrap_or_else(|error| { eprintln!("dotbot: SQLite 聊天读取失败：{error}"); Vec::new() });
        }
        let Ok(text) = tokio::fs::read_to_string(path).await else { return Vec::new() };
        let mut items: Vec<Value> = Vec::new();
        let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for line in text.lines() {
            let Ok(item) = serde_json::from_str::<Value>(line) else { continue };
            let id = item["id"].as_str().unwrap_or_default().to_string();
            match at.get(&id) {
                Some(index) if !id.is_empty() => items[*index] = item,
                _ => {
                    at.insert(id, items.len());
                    items.push(item);
                }
            }
        }
        items
    }

    /// The newest `limit` items before `before` (an index), and where the next page starts.
    #[cfg(test)]
    pub async fn page(&self, conversation: &str, before: Option<usize>, limit: usize) -> (Vec<Value>, Option<usize>) {
        let items: Vec<Value> = self.read(conversation).await.into_iter().filter(|item| item["kind"] != "tool" && item["kind"] != "upload").collect();
        let end = before.unwrap_or(items.len()).min(items.len());
        let start = end.saturating_sub(limit);
        (items[start..end].to_vec(), (start > 0).then_some(start))
    }

    /// Keeps an image and returns the path the page loads it from.
    pub async fn keep_image(&self, mime: &str, base64: &str) -> Result<String> {
        use base64_decode::decode;
        let bytes = decode(base64)?;
        let ext = match mime {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        tokio::fs::create_dir_all(&self.media).await?;
        let name = format!("{}.{ext}", uuid::Uuid::new_v4());
        tokio::fs::write(self.media.join(&name), bytes).await?;
        Ok(format!("media/{name}"))
    }

    /// Copies a file a Bot sends into the media folder; the path the page loads it from.
    pub async fn keep_file(&self, source: &Path) -> Result<String> {
        let name = source.file_name().map(|n| original_name(&n.to_string_lossy())).filter(|n| !n.is_empty()).unwrap_or_else(|| "file".into());
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || ".-_".contains(c) { c } else { '_' }).collect();
        tokio::fs::create_dir_all(&self.media).await?;
        let kept = format!("{}-{safe}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        tokio::fs::copy(source, self.media.join(&kept)).await?;
        Ok(format!("media/{kept}"))
    }

    pub fn media_dir(&self) -> &Path {
        &self.media
    }
}

/// The user's file name inside a stored or forwarded copy. Each hop (media, desktop
/// inbox, send_file) adds a random hex prefix, and earlier uploads repeated the
/// extension, so a forwarded `a.jpg` could come back as `…-…-a.jpg.jpg.jpg`.
pub fn original_name(name: &str) -> String {
    let mut rest = name;
    while let Some((head, tail)) = rest.split_once('-')
        && !tail.is_empty()
        && head.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && (head.len() == 32 || head.len() == 12 && head.bytes().any(|b| b.is_ascii_alphabetic())) {
        rest = tail;
    }
    let same = |a: &str, b: &str| { let norm = |e: &str| { let e = e.to_ascii_lowercase(); if e == "jpeg" { "jpg".to_string() } else { e } }; norm(a) == norm(b) };
    let mut name = rest.to_string();
    while let Some((stem, ext)) = name.rsplit_once('.') && !ext.is_empty()
        && let Some((head, previous)) = stem.rsplit_once('.') && !head.is_empty() && same(previous, ext) {
        name.truncate(stem.len());
    }
    name
}

pub fn item(kind: &str, fields: Value) -> Value {
    let mut map = Map::new();
    map.insert("id".into(), json!(uuid::Uuid::new_v4().to_string()));
    map.insert("time".into(), json!(crate::store::now_ms()));
    map.insert("kind".into(), json!(kind));
    if let Value::Object(more) = fields {
        map.extend(more);
    }
    Value::Object(map)
}

/// Base64 without another dependency.
mod base64_decode {
    use anyhow::{bail, Result};

    pub fn decode(input: &str) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(input.len() * 3 / 4);
        let mut buffer = 0u32;
        let mut bits = 0;
        for byte in input.bytes() {
            let value = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' | b'-' => 62,
                b'/' | b'_' => 63,
                b'=' | b'\n' | b'\r' | b' ' => continue,
                _ => bail!("not base64"),
            };
            buffer = (buffer << 6) | value as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
                buffer &= (1 << bits) - 1;
            }
        }
        Ok(out)
    }

    #[test]
    fn decodes() {
        assert_eq!(decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode("aGk").unwrap(), b"hi");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_rewritten_item_replaces_its_line_and_pages_go_back() {
        let home = tempfile::tempdir().unwrap();
        let chats = Chats::new(home.path());
        let mut tool = item("tool", json!({ "name": "Read", "done": false }));
        chats.append("bot:b1", &item("user", json!({ "text": "hi" }))).await.unwrap();
        chats.append("bot:b1", &tool).await.unwrap();
        tool["done"] = json!(true);
        chats.append("bot:b1", &tool).await.unwrap();
        chats.append("bot:b1", &item("bot", json!({ "text": "done" }))).await.unwrap();
        let items = chats.read("bot:b1").await;
        assert_eq!(items.len(), 3);
        assert_eq!(items[1]["done"], true);
        let (page, more) = chats.page("bot:b1", None, 2).await;
        assert_eq!(page.len(), 2);
        assert_eq!(more, None);
        let (older, none) = chats.page("bot:b1", None, 2).await;
        assert_eq!(older[0]["text"], "hi");
        assert_eq!(none, None);
    }

    #[test]
    fn forwarded_copies_keep_the_original_file_name() {
        assert_eq!(original_name("0f3a9c2e7b1d4e5f8a6b9c0d1e2f3a4b-photo.jpg.jpg"), "photo.jpg");
        assert_eq!(original_name("1b2c3d4e5f6a-0f3a9c2e7b1d4e5f8a6b9c0d1e2f3a4b-photo.jpg.jpg.jpg"), "photo.jpg");
        assert_eq!(original_name("scan.JPEG.jpg"), "scan.JPEG");
        assert_eq!(original_name("archive.tar.gz"), "archive.tar.gz");
        assert_eq!(original_name("202510251230-shot.png"), "202510251230-shot.png");
        assert_eq!(original_name(".jpg.jpg"), ".jpg.jpg");
    }
}
