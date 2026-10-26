//! Named conversation folders. Membership never changes a Bot's execution settings.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use crate::{store::State, team::Team};

pub fn groups(state: &State) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in state.prefs.get("sidebarGroups").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
        .chain(state.bots.values().filter_map(|b| b.extra.get("sidebarGroup").and_then(Value::as_str)))
        .chain(state.rooms.values().filter_map(|r| r.extra.get("sidebarGroup").and_then(Value::as_str))) {
        if !name.is_empty() && !names.iter().any(|n| n == name) { names.push(name.into()); }
    }
    names
}

fn valid_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        bail!("A sidebar group name must contain 1 to 32 characters.");
    }
    Ok(name)
}

impl Team {
    pub async fn sidebar_group(&self, action: &str, patch: &Value) -> Result<()> {
        let name = valid_name(patch["name"].as_str().unwrap_or_default())?.to_string();
        self.change(|state, _| {
            let mut names = groups(state);
            let exists = names.contains(&name);
            let members = patch.get("conversations").map(|v| -> Result<Vec<String>> {
                let values = v.as_array().ok_or_else(|| anyhow::anyhow!("Choose valid conversations."))?;
                values.iter().map(|v| {
                    let key = v.as_str().ok_or_else(|| anyhow::anyhow!("Unknown conversation"))?;
                    if !state.bots.values().any(|b| key == format!("bot:{}", b.id)) && !state.rooms.values().any(|r| key == format!("room:{}", r.id)) { bail!("Unknown conversation"); }
                    Ok(key.to_string())
                }).collect()
            }).transpose()?;
            if action == "create" {
                if exists { bail!("A sidebar group with this name already exists."); }
                names.push(name.clone());
            } else {
                if !exists { bail!("Unknown sidebar group."); }
                let renamed = if action == "delete" { "" } else { valid_name(patch["newName"].as_str().unwrap_or(&name))? };
                if renamed != name && names.iter().any(|n| n == renamed) { bail!("A sidebar group with this name already exists."); }
                for extra in state.bots.values_mut().map(|b| &mut b.extra).chain(state.rooms.values_mut().map(|r| &mut r.extra)) {
                    if extra.get("sidebarGroup").and_then(Value::as_str) == Some(&name) {
                        if renamed.is_empty() { extra.remove("sidebarGroup"); }
                        else { extra.insert("sidebarGroup".into(), json!(renamed)); }
                    }
                }
                let mut collapsed: Vec<String> = state.prefs.get("sidebarCollapsed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).filter(|n| n != &name).collect();
                let was_collapsed = state.prefs.get("sidebarCollapsed").and_then(Value::as_array).is_some_and(|v| v.contains(&json!(name)));
                if !renamed.is_empty() && patch["collapsed"].as_bool().unwrap_or(was_collapsed) { collapsed.push(renamed.into()); }
                state.prefs.insert("sidebarCollapsed".into(), json!(collapsed));
                names = names.into_iter().filter_map(|n| if n == name { if renamed.is_empty() { None } else { Some(renamed.into()) } } else { Some(n) }).collect();
            }
            if action != "delete" && let Some(members) = members {
                let target = if action == "create" { name.as_str() } else { patch["newName"].as_str().unwrap_or(&name).trim() };
                for (key, extra) in state.bots.values_mut().map(|b| (format!("bot:{}", b.id), &mut b.extra)).chain(state.rooms.values_mut().map(|r| (format!("room:{}", r.id), &mut r.extra))) {
                    if members.contains(&key) { extra.insert("sidebarGroup".into(), json!(target)); }
                    else if extra.get("sidebarGroup").and_then(Value::as_str) == Some(target) { extra.remove("sidebarGroup"); }
                }
            }
            state.prefs.insert("sidebarGroups".into(), json!(names));
            Ok(())
        }).await
    }

    pub async fn sidebar_move(&self, conversation: &str, group: &str) -> Result<()> {
        self.change(|state, _| {
            let group = group.trim();
            if !group.is_empty() && !groups(state).iter().any(|n| n == group) { bail!("Unknown sidebar group."); }
            let extra = if let Some(id) = conversation.strip_prefix("bot:") {
                state.bots.get_mut(id).map(|b| &mut b.extra)
            } else if let Some(id) = conversation.strip_prefix("room:") {
                state.rooms.get_mut(id).map(|r| &mut r.extra)
            } else { None }.ok_or_else(|| anyhow::anyhow!("Unknown conversation"))?;
            if group.is_empty() { extra.remove("sidebarGroup"); }
            else { extra.insert("sidebarGroup".into(), json!(group)); }
            Ok(())
        }).await
    }
}
