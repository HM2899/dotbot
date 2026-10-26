//! Keys the user typed into a key card: `<home>/secrets.json`, readable by the owner
//! only. Bots never see a value: their turns get it as $DOTBOT_SECRET_<NAME>, team
//! connections take it as ${NAME}, and any value that shows up in what a Bot wrote or a
//! tool returned is replaced before it is saved or shown.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

pub struct Secrets {
    path: PathBuf,
    values: std::sync::Mutex<BTreeMap<String, String>>,
}

pub fn clean_name(raw: &str) -> Result<String> {
    let name: String = raw.trim().to_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let name = name.trim_matches('_').to_string();
    if name.is_empty() || name.len() > 48 || name.starts_with(|c: char| c.is_ascii_digit()) {
        bail!("A key's name is letters, digits and _, at most 48, not starting with a digit.");
    }
    Ok(name)
}

impl Secrets {
    pub fn open(home: &Path) -> Self {
        let path = home.join("secrets.json");
        let values = crate::sqlite_storage::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Secrets { path, values: std::sync::Mutex::new(values) }
    }

    fn save(&self, values: &BTreeMap<String, String>) -> Result<()> {
        if let Some(result) = crate::sqlite_storage::save(&self.path, &serde_json::to_vec(values)?) { return result; }
        let temp = self.path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_string_pretty(values)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&temp, &self.path)?;
        Ok(())
    }

    pub fn names(&self) -> Vec<String> {
        self.values.lock().unwrap().keys().cloned().collect()
    }

    pub fn set(&self, name: &str, value: &str) -> Result<String> {
        let name = clean_name(name)?;
        if value.trim().is_empty() {
            bail!("Type the key.");
        }
        let mut values = self.values.lock().unwrap();
        values.insert(name.clone(), value.trim().to_string());
        self.save(&values)?;
        Ok(name)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let mut values = self.values.lock().unwrap();
        values.remove(name);
        self.save(&values)
    }

    pub fn env(&self) -> Vec<(String, String)> {
        // Existing scripts and resumed conversations may still use the old prefix.
        self.values.lock().unwrap().iter().flat_map(|(k, v)| [
            (format!("DOTBOT_SECRET_{k}"), v.clone()),
            (format!("TEAMBOT_SECRET_{k}"), v.clone()),
        ]).collect()
    }

    pub fn get(&self, name: &str) -> Option<String> {
        self.values.lock().unwrap().get(name).cloned()
    }

    /// The text with every saved value (of four characters or more) replaced.
    pub fn redact(&self, text: &str) -> String {
        let values = self.values.lock().unwrap();
        let mut pairs: Vec<(&String, &String)> = values.iter().filter(|(_, v)| v.chars().count() >= 4).collect();
        pairs.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        let mut out = text.to_string();
        for (name, value) in pairs {
            if out.contains(value.as_str()) {
                out = out.replace(value.as_str(), &format!("[key {name}]"));
            }
        }
        out
    }

    pub fn redact_value(&self, value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::String(text) => Value::String(self.redact(&text)),
            Value::Array(values) => Value::Array(values.into_iter().map(|v| self.redact_value(v)).collect()),
            Value::Object(values) => Value::Object(values.into_iter().map(|(key, v)| (key, self.redact_value(v))).collect()),
            other => other,
        }
    }

    /// Retain suffixes that could be the start of a secret until the next delta.
    /// Redacting each delta independently would leak values split across chunks.
    pub fn drain_stream(&self, pending: &mut String, flush: bool) -> String {
        *pending = self.redact(pending);
        let values = self.values.lock().unwrap();
        let mut keep = 0;
        for value in values.values().filter(|v| v.chars().count() >= 4) {
            let earliest = pending.len().saturating_sub(value.len().saturating_sub(1));
            for (start, _) in pending.char_indices().filter(|(start, _)| *start >= earliest) {
                if value.starts_with(&pending[start..]) { keep = keep.max(pending.len() - start); break; }
            }
        }
        if flush {
            if pending[pending.len() - keep..].chars().count() >= 4 {
                pending.truncate(pending.len() - keep); pending.push_str("[partial key]");
            }
            return std::mem::take(pending);
        }
        let rest = pending.split_off(pending.len() - keep);
        std::mem::replace(pending, rest)
    }

    /// `${NAME}` in a connection's settings, filled from the saved keys.
    pub fn fill(&self, text: &str) -> String {
        let values = self.values.lock().unwrap();
        let mut out = text.to_string();
        for (name, value) in values.iter() {
            out = out.replace(&format!("${{{name}}}"), value);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_stay_out_of_text_and_fill_settings() {
        let home = tempfile::tempdir().unwrap();
        let secrets = Secrets::open(home.path());
        assert_eq!(secrets.set("github token", "ghp_abcdef123456").unwrap(), "GITHUB_TOKEN");
        assert_eq!(secrets.redact("token=ghp_abcdef123456!"), "token=[key GITHUB_TOKEN]!");
        assert_eq!(secrets.fill("Bearer ${GITHUB_TOKEN}"), "Bearer ghp_abcdef123456");
        assert_eq!(secrets.env(), vec![
            ("DOTBOT_SECRET_GITHUB_TOKEN".to_string(), "ghp_abcdef123456".to_string()),
            ("TEAMBOT_SECRET_GITHUB_TOKEN".to_string(), "ghp_abcdef123456".to_string()),
        ]);
        assert_eq!(Secrets::open(home.path()).names(), vec!["GITHUB_TOKEN"]);
        assert!(clean_name("1abc").is_err());
    }

    #[test]
    fn secrets_are_redacted_across_stream_boundaries_and_in_json_strings() {
        let home = tempfile::tempdir().unwrap(); let secrets = Secrets::open(home.path());
        secrets.set("TOKEN", "秘密-abc\"def").unwrap();
        let mut pending = String::new(); let mut shown = String::new();
        for chunk in ["prefix 秘", "密-ab", "c\"", "def suffix"] {
            pending.push_str(chunk); shown.push_str(&secrets.drain_stream(&mut pending, false));
        }
        shown.push_str(&secrets.drain_stream(&mut pending, true));
        assert_eq!(shown, "prefix [key TOKEN] suffix");
        assert_eq!(secrets.redact_value(serde_json::json!({"input": "秘密-abc\"def"}))["input"], "[key TOKEN]");
    }
}
