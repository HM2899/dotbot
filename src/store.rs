//! The team: `<home>/state.json` in DS Bot's shape (Bots, groups, Main Bots, the group
//! logs), so a DS Bot team folder opens as it is. Fields this program does not know are
//! kept as they were. What only dotbot needs (Claude sessions, what each member has
//! read in a group) lives beside it in `teambot.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const ROOM_LOG_MAX: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Bot {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub instructions: String,
    /// What it is for, in a line or two: other Bots read it to decide whom to hand work to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Pinned Bots head the list, as the user's main contacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    /// "readonly", "ask" (the user approves each risky step), "auto" (the default: the model
    /// approves them and leaves only the irreversible ones to the user) or "edits".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<Value>,
    /// Preferred workflows; learned task-specific workflows live in skill-profiles.json.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub who: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default)]
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub admin: Option<String>,
    /// "auto": the coordinator assigns bounded work automatically;
    /// "admin": the admin answers the user unless others are called with @Name;
    /// "all": every member answers, in order.
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub notice: String,
    #[serde(default)]
    pub log: Vec<LogLine>,
    #[serde(default)]
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_dir: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_mode() -> String {
    "auto".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub main_bot_ids: Vec<String>,
    #[serde(default)]
    pub bots: indexmap_like::Ordered<Bot>,
    #[serde(default)]
    pub rooms: indexmap_like::Ordered<Room>,
    #[serde(default)]
    pub prefs: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn one() -> u32 {
    1
}

/// Maps that keep the order they were written in, as JS objects do, so the roster
/// lists Bots in the order they were made.
pub mod indexmap_like {
    use serde::de::DeserializeOwned;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::{Map, Value};

    #[derive(Debug, Clone)]
    pub struct Ordered<T>(pub Vec<(String, T)>);

    impl<T> Default for Ordered<T> {
        fn default() -> Self {
            Ordered(Vec::new())
        }
    }

    impl<T> Ordered<T> {
        pub fn get(&self, id: &str) -> Option<&T> {
            self.0.iter().find(|(k, _)| k == id).map(|(_, v)| v)
        }
        pub fn get_mut(&mut self, id: &str) -> Option<&mut T> {
            self.0.iter_mut().find(|(k, _)| k == id).map(|(_, v)| v)
        }
        pub fn insert(&mut self, id: String, value: T) {
            match self.get_mut(&id) {
                Some(slot) => *slot = value,
                None => self.0.push((id, value)),
            }
        }
        pub fn remove(&mut self, id: &str) -> Option<T> {
            let at = self.0.iter().position(|(k, _)| k == id)?;
            Some(self.0.remove(at).1)
        }
        pub fn values(&self) -> impl Iterator<Item = &T> {
            self.0.iter().map(|(_, v)| v)
        }
        pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
            self.0.iter_mut().map(|(_, v)| v)
        }
        pub fn len(&self) -> usize {
            self.0.len()
        }
    }

    impl<T: Serialize> Serialize for Ordered<T> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeMap;
            let mut map = serializer.serialize_map(Some(self.0.len()))?;
            for (k, v) in &self.0 {
                map.serialize_entry(k, v)?;
            }
            map.end()
        }
    }

    impl<'de, T: DeserializeOwned> Deserialize<'de> for Ordered<T> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            // serde_json's preserve_order keeps the file's order in the Map.
            let map = Map::<String, Value>::deserialize(deserializer)?;
            let mut out = Vec::new();
            for (k, v) in map {
                out.push((k, serde_json::from_value(v).map_err(serde::de::Error::custom)?));
            }
            Ok(Ordered(out))
        }
    }
}

/// What only dotbot keeps: per conversation its Claude session and what a member
/// has read of its group.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Local {
    #[serde(default)]
    pub sessions: BTreeMap<String, String>,
    #[serde(default)]
    pub seen: BTreeMap<String, u64>,
    /// Per conversation, how many of its items the memory review has read.
    #[serde(default)]
    pub reviewed: BTreeMap<String, u64>,
    #[serde(default)]
    pub team_workspace: Option<String>,
    /// The provider each Claude session ran on: a session does not move to another one.
    #[serde(default)]
    pub session_providers: BTreeMap<String, String>,
    /// Model/provider/workspace configuration used to create each CLI session.
    #[serde(default)]
    pub session_configs: BTreeMap<String, String>,
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl State {
    /// Upgrade the former default while retaining an explicit choice on later opens.
    pub fn enable_auto_coordination(&mut self) -> bool {
        let mut changed = false;
        for room in self.rooms.values_mut() {
            if !room.extra.contains_key("coordinationVersion") {
                if room.mode == "admin" { room.mode = "auto".into(); }
                room.extra.insert("coordinationVersion".into(), serde_json::json!(1));
                changed = true;
            }
        }
        changed
    }

    /// Retire legacy group model overrides and sessions that could retain them.
    /// Bot models, group history and memory are untouched.
    pub fn clear_room_model_overrides(&mut self, local: &mut Local) -> bool {
        let mut changed = false;
        for room in self.rooms.values_mut() {
            if let Some(model) = room.extra.remove("model") {
                changed = true;
                if model.as_str().is_some_and(|m| !m.trim().is_empty()) {
                    let prefix = format!("room:{}:", room.id);
                    local.sessions.retain(|key, _| !key.starts_with(&prefix));
                    local.session_providers.retain(|key, _| !key.starts_with(&prefix));
                }
            }
        }
        changed
    }

    pub fn is_main(&self, id: &str) -> bool {
        self.main_bot_ids.iter().any(|m| m == id)
    }
    /// A Bot by id, or by name, case and spaces aside.
    pub fn find_bot(&self, key: &str) -> Option<&Bot> {
        let key = key.trim().trim_start_matches('@');
        self.bots.get(key).or_else(|| self.bots.values().find(|b| b.name.trim().eq_ignore_ascii_case(key)))
    }
    pub fn find_room(&self, key: &str) -> Option<&Room> {
        let key = key.trim();
        self.rooms.get(key).or_else(|| self.rooms.values().find(|r| r.name.trim().eq_ignore_ascii_case(key)))
    }
    pub fn writes_team(&self, id: &str) -> bool {
        self.is_main(id) || self.rooms.values().any(|r| r.admin.as_deref() == Some(id))
    }
    pub fn append(room: &mut Room, mut line: LogLine) -> LogLine {
        room.seq += 1;
        line.seq = room.seq;
        line.time.get_or_insert_with(now_ms);
        room.log.push(line.clone());
        if room.log.len() > ROOM_LOG_MAX {
            let cut = room.log.len() - ROOM_LOG_MAX;
            room.log.drain(..cut);
        }
        line
    }
}

pub struct Store {
    pub home: PathBuf,
}

impl Store {
    pub fn new(home: &Path) -> Self {
        Store { home: home.to_path_buf() }
    }
    fn state_path(&self) -> PathBuf {
        self.home.join("state.json")
    }
    fn local_path(&self) -> PathBuf {
        self.home.join("teambot.json")
    }

    pub fn load(&self) -> Result<State> {
        match crate::sqlite_storage::read_to_string(self.state_path()) {
            Ok(text) => serde_json::from_str(&text).with_context(|| format!("reading {}", self.state_path().display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State { version: 1, ..Default::default() }),
            Err(error) => Err(error.into()),
        }
    }
    pub fn load_local(&self) -> Result<Local> {
        match crate::sqlite_storage::read_to_string(self.local_path()) {
            Ok(text) => Ok(serde_json::from_str(&text)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Local::default()),
            Err(error) => Err(error.into()),
        }
    }

    fn write(path: &Path, text: &str) -> Result<()> {
        if let Some(result) = crate::sqlite_storage::save(path, text.as_bytes()) { return result; }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = PathBuf::from(format!("{}.{}.tmp", path.display(), std::process::id()));
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// Saves with the next revision.
    pub fn save(&self, state: &mut State) -> Result<()> {
        state.revision += 1;
        Self::write(&self.state_path(), &serde_json::to_string_pretty(state)?)
    }
    pub fn save_local(&self, local: &Local) -> Result<()> {
        Self::write(&self.local_path(), &serde_json::to_string_pretty(local)?)
    }
    pub fn save_team(&self, state: &mut State, local: &Local) -> Result<()> {
        let mut next = state.clone();
        next.revision += 1;
        if let Some(result) = crate::sqlite_storage::save_team(&self.home, &next, local) {
            result?;
            state.revision = next.revision;
            return Ok(());
        }
        self.save(state)?;
        self.save_local(local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DS_BOT_STATE: &str = r##"{
  "version": 1,
  "revision": 41,
  "mainBotIds": ["m1"],
  "bots": {
    "m1": { "id": "m1", "sessionId": "m1c", "segments": [{ "sessionId": "m1", "endedAt": 50 }], "name": "Chief", "role": "Chief of Staff", "instructions": "", "createdAt": 1, "createdBy": null, "capabilities": [], "pinned": true },
    "b2": { "id": "b2", "name": "Writer", "role": "Writer", "color": "#3b82f6", "model": "deepseek/deepseek-v4-pro", "workDir": "/tmp/w", "instructions": "Write well.", "createdAt": 2, "createdBy": "m1", "capabilities": ["coding"] }
  },
  "rooms": {
    "r1": { "id": "r1", "name": "Launch", "members": ["m1", "b2"], "admin": "m1", "mode": "admin", "notice": "", "createdAt": 3, "createdBy": null, "seq": 2,
      "log": [{ "who": "User", "text": "hi", "seq": 1 }, { "who": "Chief", "botId": "m1", "text": "hello", "seq": 2 }],
      "sessions": { "m1": "s-m1-r1" }, "segments": {} }
  },
  "exchanges": [{ "id": "e1", "from": "m1", "to": "b2", "text": "go" }],
  "exchangeTurns": {},
  "questions": {},
  "prefs": { "theme": "mono" }
}"##;

    #[test]
    fn a_ds_bot_team_reads_and_writes_back_with_nothing_lost() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("state.json"), DS_BOT_STATE).unwrap();
        let store = Store::new(home.path());
        let mut state = store.load().unwrap();
        assert_eq!(state.bots.values().map(|b| b.name.as_str()).collect::<Vec<_>>(), vec!["Chief", "Writer"]);
        assert!(state.is_main("m1"));
        assert_eq!(state.find_bot("@writer").unwrap().id, "b2");
        assert_eq!(state.rooms.get("r1").unwrap().log.len(), 2);
        store.save(&mut state).unwrap();
        let before: Value = serde_json::from_str(DS_BOT_STATE).unwrap();
        let mut after: Value = serde_json::from_str(&std::fs::read_to_string(home.path().join("state.json")).unwrap()).unwrap();
        assert_eq!(after["revision"], 42);
        after["revision"] = before["revision"].clone();
        assert_eq!(after, before);
    }

    #[test]
    fn legacy_group_models_are_removed_without_changing_bots_or_history() {
        let mut data: Value = serde_json::from_str(DS_BOT_STATE).unwrap();
        data["rooms"]["r1"]["model"] = serde_json::json!("old-unsupported-model");
        let mut state: State = serde_json::from_value(data.clone()).unwrap();
        let mut local = Local::default();
        for key in ["room:r1:m1", "room:r1:b2", "room:r10:m1", "bot:m1"] {
            local.sessions.insert(key.into(), format!("session-{key}"));
            local.session_providers.insert(key.into(), "official".into());
        }
        local.seen.insert("room:r1:b2".into(), 2);
        assert!(state.clear_room_model_overrides(&mut local));
        let after = serde_json::to_value(&state).unwrap();
        assert!(after["rooms"]["r1"].get("model").is_none());
        assert_eq!(after["bots"], data["bots"]);
        assert_eq!(after["rooms"]["r1"]["log"], data["rooms"]["r1"]["log"]);
        assert_eq!(after["rooms"]["r1"]["segments"], data["rooms"]["r1"]["segments"]);
        assert_eq!(local.sessions.keys().cloned().collect::<Vec<_>>(), vec!["bot:m1", "room:r10:m1"]);
        assert_eq!(local.session_providers.keys().cloned().collect::<Vec<_>>(), vec!["bot:m1", "room:r10:m1"]);
        assert_eq!(local.seen["room:r1:b2"], 2);
        assert!(!state.clear_room_model_overrides(&mut local));
    }

    #[test]
    fn empty_legacy_group_models_keep_existing_sessions() {
        for model in [Value::Null, serde_json::json!(""), serde_json::json!("  ")] {
            let mut data: Value = serde_json::from_str(DS_BOT_STATE).unwrap();
            data["rooms"]["r1"]["model"] = model;
            let mut state: State = serde_json::from_value(data).unwrap();
            let mut local = Local::default();
            local.sessions.insert("room:r1:m1".into(), "session".into());
            assert!(state.clear_room_model_overrides(&mut local));
            assert_eq!(local.sessions["room:r1:m1"], "session");
            assert!(serde_json::to_value(&state).unwrap()["rooms"]["r1"].get("model").is_none());
        }
    }

    #[test]
    fn the_group_log_keeps_its_latest_lines() {
        let mut room = Room { id: "r".into(), name: "R".into(), ..Default::default() };
        for i in 0..(ROOM_LOG_MAX + 5) {
            State::append(&mut room, LogLine { who: "User".into(), text: i.to_string(), ..Default::default() });
        }
        assert_eq!(room.log.len(), ROOM_LOG_MAX);
        assert_eq!(room.log.last().unwrap().seq, (ROOM_LOG_MAX + 5) as u64);
    }

    #[test]
    fn coordination_migration_preserves_all_and_later_explicit_admin_choices() {
        let mut state = State::default();
        state.rooms.insert("default".into(), Room { mode: "admin".into(), ..Default::default() });
        state.rooms.insert("all".into(), Room { mode: "all".into(), ..Default::default() });
        assert!(state.enable_auto_coordination());
        assert_eq!(state.rooms.get("default").unwrap().mode, "auto");
        assert_eq!(state.rooms.get("all").unwrap().mode, "all");
        state.rooms.get_mut("default").unwrap().mode = "admin".into();
        assert!(!state.enable_auto_coordination());
        assert_eq!(state.rooms.get("default").unwrap().mode, "admin");
    }
}
