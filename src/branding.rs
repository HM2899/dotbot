//! Public names can evolve without moving existing teams or their workspaces.
use std::path::{Path, PathBuf};

/// Prefer the new variable; older deployment environments remain valid.
pub fn env(name: impl AsRef<str>) -> Result<String, std::env::VarError> {
    let name = name.as_ref();
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => match name.strip_prefix("DOTBOT_") {
            Some(suffix) => std::env::var(format!("TEAMBOT_{suffix}")),
            None => Err(std::env::VarError::NotPresent),
        },
        result => result,
    }
}

pub fn default_directory(user_home: &Path, current: &str, legacy: &str) -> PathBuf {
    let current = user_home.join(current);
    let legacy = user_home.join(legacy);
    if !current.exists() && legacy.exists() { legacy } else { current }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_installs_and_existing_teams_resolve_without_moving_data() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(default_directory(home.path(), ".dotbot", ".teambot"), home.path().join(".dotbot"));
        std::fs::create_dir(home.path().join(".teambot")).unwrap();
        std::fs::write(home.path().join(".teambot/state.json"), "existing team").unwrap();
        assert_eq!(default_directory(home.path(), ".dotbot", ".teambot"), home.path().join(".teambot"));
        std::fs::create_dir(home.path().join(".dotbot")).unwrap();
        assert_eq!(default_directory(home.path(), ".dotbot", ".teambot"), home.path().join(".dotbot"));
        assert_eq!(std::fs::read_to_string(home.path().join(".teambot/state.json")).unwrap(), "existing team");
    }
}
