//! `dotbot import <folder>`: copies a DS Bot team (its state.json and memory folder,
//! e.g. ~/.dsh/bot or ~/.ds-bot/bot) into dotbot's home. The source is only read.
//! DS Bot's own chats live in DSH Sessions, which dotbot cannot read; group chat logs
//! are in state.json and come along.

use std::path::Path;

use anyhow::{bail, Context, Result};

fn copy_tree(from: &Path, to: &Path) -> Result<u64> {
    let mut count = 0;
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            count += copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
            count += 1;
        }
    }
    Ok(count)
}

pub fn import(source: &Path, home: &Path) -> Result<String> {
    let state = source.join("state.json");
    if !state.is_file() {
        bail!("{} has no state.json: point at the DS Bot team folder (the `bot` folder inside DSH_HOME).", source.display());
    }
    let text = crate::sqlite_storage::read_to_string(&state).context("reading state.json")?;
    let parsed: crate::store::State = serde_json::from_str(&text).context("state.json is not a DS Bot team")?;
    let existing = crate::store::Store::new(home).load()?;
    if existing.bots.len() > 0 {
        bail!("{} already has a team. Import into an empty home (--home).", home.display());
    }
    std::fs::create_dir_all(home)?;
    crate::durable::save(&home.join("state.json"), &parsed)?;
    let memory = source.join("memory");
    let files = if memory.is_dir() { copy_tree(&memory, &home.join("memory"))? } else { 0 };
    // Frozen sections belong to DS Bot's Sessions; dotbot freezes its own.
    let _ = std::fs::remove_dir_all(home.join("memory").join("snapshots"));
    Ok(format!("Imported {} Bots, {} group chats and {files} memory files from {}.", parsed.bots.len(), parsed.rooms.len(), source.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ds_bot_team_and_its_memory_come_across_unchanged() {
        let source = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("state.json"), r#"{"version":1,"revision":5,"mainBotIds":["m1"],"bots":{"m1":{"id":"m1","name":"Chief","role":"Chief","instructions":"","createdAt":1}},"rooms":{}}"#).unwrap();
        std::fs::create_dir_all(source.path().join("memory/bots/m1")).unwrap();
        std::fs::write(source.path().join("memory/bots/m1/MEMORY.md"), "# Memory\n\n- Likes tea [added: 2026-10-01]\n\n## Index\n").unwrap();
        std::fs::create_dir_all(source.path().join("memory/snapshots")).unwrap();
        std::fs::write(source.path().join("memory/snapshots/m1.md"), "old").unwrap();
        let message = import(source.path(), &home.path().join("team")).unwrap();
        assert!(message.contains("1 Bots"), "{message}");
        assert_eq!(std::fs::read_to_string(home.path().join("team/memory/bots/m1/MEMORY.md")).unwrap(), "# Memory\n\n- Likes tea [added: 2026-10-01]\n\n## Index\n");
        assert!(!home.path().join("team/memory/snapshots").exists());
        assert!(source.path().join("memory/snapshots/m1.md").exists());
        assert!(import(source.path(), &home.path().join("team")).is_err());
    }
}
