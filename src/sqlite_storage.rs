//! SQLite metadata and chat persistence. Files and externally consumed artifacts stay on disk.
use std::{path::{Path, PathBuf}, time::Duration};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

pub const FILE: &str = "dotbot.sqlite3";
fn connect(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "synchronous", "FULL")?;
    Ok(db)
}

pub fn initialize(home: &Path) -> Result<()> {
    std::fs::create_dir_all(home)?;
    let path = home.join(FILE);
    use std::os::unix::fs::OpenOptionsExt;
    let _file = std::fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(&path)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    let mut db = connect(&path).context("无法打开 SQLite 数据库")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(version <= 1, "SQLite 数据库版本较新，请使用匹配的 dotbot 版本");
    db.pragma_update(None, "journal_mode", "WAL")?;
    let tx = db.transaction()?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS documents (path TEXT PRIMARY KEY, payload TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS chat_items (ordinal INTEGER PRIMARY KEY, conversation TEXT NOT NULL, item_id TEXT NOT NULL, payload TEXT NOT NULL, UNIQUE(conversation,item_id));
        CREATE INDEX IF NOT EXISTS chat_order ON chat_items(conversation,ordinal);
        CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
    let migrated = tx.query_row("SELECT value FROM metadata WHERE key='migrated'", [], |r| r.get::<_, String>(0)).optional()?.is_some();
    if !migrated {
        for entry in std::fs::read_dir(home)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if entry.file_type()?.is_file() && managed(&entry.path()) {
                let text = std::fs::read_to_string(entry.path())?;
                serde_json::from_str::<Value>(&text).with_context(|| format!("迁移 {name} 失败：JSON 无效，原文件保留"))?;
                tx.execute("INSERT INTO documents(path,payload) VALUES (?1,?2)", params![name, text])?;
            }
        }
        let chats = home.join("chats");
        if chats.is_dir() { for entry in std::fs::read_dir(chats)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|s| s != "jsonl") { continue; }
            let key = entry.path().file_stem().unwrap().to_string_lossy().to_string();
            let text = std::fs::read_to_string(entry.path())?;
            for (index,line) in text.lines().enumerate().filter(|(_,l)| !l.trim().is_empty()) {
                let item: Value = serde_json::from_str(line).with_context(|| format!("迁移聊天 {key} 第 {} 行失败，原文件保留", index+1))?;
                put_chat(&tx, &key, &item)?;
            }
        }}
        tx.execute("INSERT INTO metadata(key,value) VALUES ('migrated','1')", [])?;
    }
    tx.pragma_update(None, "user_version", 1)?;
    tx.commit()?;
    let check: String = db.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    ensure!(check == "ok", "SQLite 完整性检查失败，请恢复备份");
    Ok(())
}

// Only metadata directly in the team home; CLI settings must remain real files.
fn managed(path: &Path) -> bool {
    path.extension().is_some_and(|s| s == "json")
        && path.file_name().is_some_and(|s| s != "provider-settings.json")
}
fn database(path: &Path) -> Option<PathBuf> {
    let db = path.parent()?.join(FILE);
    (managed(path) && db.is_file()).then_some(db)
}
pub fn read(path: impl AsRef<Path>) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    let Some(db) = database(path) else { return std::fs::read(path); };
    let result = (|| -> Result<Option<String>> {
        Ok(connect(&db)?.query_row("SELECT payload FROM documents WHERE path=?1", [path.file_name().unwrap().to_string_lossy().as_ref()], |r| r.get(0)).optional()?)
    })().map_err(std::io::Error::other)?;
    result.map(String::into_bytes).ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
}
pub fn read_to_string(path: impl AsRef<Path>) -> std::io::Result<String> {
    String::from_utf8(read(path)?).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData,e))
}
pub fn save(path: &Path, bytes: &[u8]) -> Option<Result<()>> {
    let db = database(path)?;
    Some((|| {
        let text = std::str::from_utf8(bytes)?;
        serde_json::from_str::<Value>(text)?;
        connect(&db)?.execute("INSERT INTO documents(path,payload) VALUES (?1,?2) ON CONFLICT(path) DO UPDATE SET payload=excluded.payload", params![path.file_name().unwrap().to_string_lossy(), text])?;
        Ok(())
    })())
}

pub fn save_team(home: &Path, state: &impl serde::Serialize, local: &impl serde::Serialize) -> Option<Result<()>> {
    let path = home.join(FILE);
    path.is_file().then(|| {
        let mut db = connect(&path)?;
        let tx = db.transaction()?;
        for (name, value) in [("state.json", serde_json::to_string(state)?), ("teambot.json", serde_json::to_string(local)?)] {
            tx.execute("INSERT INTO documents(path,payload) VALUES (?1,?2) ON CONFLICT(path) DO UPDATE SET payload=excluded.payload", params![name,value])?;
        }
        tx.commit()?;
        Ok(())
    })
}
fn put_chat(db: &Connection, conversation: &str, item: &Value) -> Result<()> {
    let id = item["id"].as_str().filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    db.execute("INSERT INTO chat_items(conversation,item_id,payload) VALUES (?1,?2,?3) ON CONFLICT(conversation,item_id) DO UPDATE SET payload=excluded.payload", params![conversation, id, item.to_string()])?;
    Ok(())
}
pub fn append_chat(home: &Path, key: &str, item: &Value) -> Option<Result<()>> {
    let path = home.join(FILE);
    path.is_file().then(|| put_chat(&connect(&path)?, key, item))
}
pub fn clear_chat(home: &Path, key: &str) -> Option<Result<()>> {
    let path = home.join(FILE);
    path.is_file().then(|| { connect(&path)?.execute("DELETE FROM chat_items WHERE conversation=?1", [key])?; Ok(()) })
}
pub fn read_chat(home: &Path, key: &str) -> Option<Result<Vec<Value>>> {
    let path = home.join(FILE);
    path.is_file().then(|| {
        let db = connect(&path)?;
        let mut statement = db.prepare("SELECT payload FROM chat_items WHERE conversation=?1 ORDER BY ordinal")?;
        let rows = statement.query_map([key], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_is_atomic_and_original_files_are_untouched() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("state.json"), r#"{"revision":7}"#).unwrap();
        std::fs::write(home.path().join("channels.json"), "invalid").unwrap();
        assert!(initialize(home.path()).is_err());
        std::fs::write(home.path().join("channels.json"), "[]").unwrap();
        initialize(home.path()).unwrap();
        save(&home.path().join("state.json"), br#"{"revision":8}"#).unwrap().unwrap();
        assert_eq!(read_to_string(home.path().join("state.json")).unwrap(), r#"{"revision":8}"#);
        assert_eq!(std::fs::read_to_string(home.path().join("state.json")).unwrap(), r#"{"revision":7}"#);
        initialize(home.path()).unwrap();
        assert_eq!(read_to_string(home.path().join("state.json")).unwrap(), r#"{"revision":8}"#);
    }
    #[test]
    fn chat_migration_keeps_order_and_latest_item_and_is_not_repeated() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("chats")).unwrap();
        std::fs::write(home.path().join("chats/bot_a.jsonl"), "{\"id\":\"a\",\"text\":\"first\"}\n{\"id\":\"b\"}\n{\"id\":\"a\",\"text\":\"updated\"}\n").unwrap();
        initialize(home.path()).unwrap();
        let items = read_chat(home.path(), "bot_a").unwrap().unwrap();
        assert_eq!(items.len(), 2); assert_eq!(items[0]["text"], "updated");
        append_chat(home.path(), "bot_a", &serde_json::json!({"id":"c"})).unwrap().unwrap();
        initialize(home.path()).unwrap();
        let items = read_chat(home.path(), "bot_a").unwrap().unwrap();
        assert_eq!(items.len(), 3); assert_eq!(items[1]["id"], "b");
    }
    #[test]
    fn simultaneous_writers_commit_private_metadata_without_losing_updates() {
        let home = tempfile::tempdir().unwrap(); initialize(home.path()).unwrap();
        std::thread::scope(|scope| { for i in 0..16 { let home = home.path(); scope.spawn(move || { save(&home.join(format!("test-{i}.json")), br#"{"saved":true}"#).unwrap().unwrap(); }); } });
        for i in 0..16 { assert!(read_to_string(home.path().join(format!("test-{i}.json"))).unwrap().contains("true")); }
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(home.path().join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(read(home.path().join("missing.json")).unwrap_err().kind() == std::io::ErrorKind::NotFound);
    }
}
