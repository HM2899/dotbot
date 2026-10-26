//! Cancellable, atomic reservations for a shared computer. Dropping a waiter or
//! guard releases its place/resources; unrelated paths can proceed together.
use std::{collections::{BTreeSet, VecDeque}, sync::{Arc, Mutex}};
use anyhow::{Result, ensure};
use tokio::sync::Notify;

#[derive(Default)]
struct State { next: u64, held: Vec<(u64, String, Vec<String>)>, waiting: VecDeque<(u64, String, Vec<String>)> }
#[derive(Default)]
pub struct Resources { state: Mutex<State>, changed: Notify }
pub struct Guard { pool: Arc<Resources>, id: u64 }
impl Drop for Guard {
    fn drop(&mut self) {
        let mut state = self.pool.state.lock().unwrap();
        state.held.retain(|(id, _, _)| *id != self.id);
        state.waiting.retain(|(id, _, _)| *id != self.id);
        drop(state); self.pool.changed.notify_waiters();
    }
}

fn overlap(a: &str, b: &str) -> bool {
    a == b || a == "/" && b.starts_with('/') || b == "/" && a.starts_with('/') || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}
fn conflicts(a: &[String], b: &[String]) -> bool { a.iter().any(|x| b.iter().any(|y| overlap(x,y))) }

pub fn normalize(value: &str) -> Result<String> {
    let value = value.trim();
    if value == "packages" { return Ok("@packages".into()); }
    ensure!(value.starts_with('/') && value.len() <= 4096 && !value.contains(char::is_control), "共享资源须为绝对文件路径或 packages。");
    let mut parts = vec![];
    for part in value.split('/') {
        match part { "" | "." => {}, ".." => { ensure!(!parts.is_empty(), "共享资源路径越界。"); parts.pop(); }, _ => parts.push(part) }
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Conservative literal-path detection is supplemented by explicit resources;
/// dynamic shell paths cannot be inferred reliably from arbitrary shell syntax.
pub fn command_resources(command: &str, explicit: &[String]) -> Result<Vec<String>> {
    ensure!(explicit.len() <= 32, "一次最多协调 32 个资源。");
    let mut result = BTreeSet::new();
    for value in explicit { result.insert(normalize(value)?); }
    static PACKAGES: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if PACKAGES.get_or_init(|| regex::Regex::new(r"(^|[\s;&|/])(apt-get|apt|dpkg|pip3?|npm|pnpm|yarn|snap|flatpak)([\s;&|]|$)").unwrap()).is_match(command) { result.insert("@packages".into()); }
    static PATHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    for capture in PATHS.get_or_init(|| regex::Regex::new(r#"(?:^|[\s='\"])(/config(?:/[^\s'\";&|<>]*)?)"#).unwrap()).captures_iter(command) {
        result.insert(normalize(&capture[1])?);
    }
    // Relative/dynamic paths in an undeclared shell command may touch any part
    // of the working home. Be conservative; explicit paths permit independent work.
    if result.is_empty() { result.insert("/config".into()); }
    Ok(result.into_iter().collect())
}

impl Resources {
    pub fn waiting(&self) -> usize { self.state.lock().unwrap().waiting.len() }
    pub async fn acquire(self: &Arc<Self>, owner: &str, resources: Vec<String>) -> Guard {
        let id = {
            let mut state = self.state.lock().unwrap(); state.next += 1;
            let id = state.next; state.waiting.push_back((id, owner.into(), resources.clone())); id
        };
        let guard = Guard { pool: self.clone(), id };
        loop {
            let changed = self.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
            {
                let mut state = self.state.lock().unwrap();
                let held = state.held.iter().any(|(_, who, keys)| who != owner && conflicts(keys,&resources));
                let earlier = state.waiting.iter().take_while(|(n,_,_)| *n != id).any(|(_,who,keys)| who != owner && conflicts(keys,&resources));
                // Reentrant reservations may finish existing work even if a
                // competing waiter arrived between two steps of that work.
                let reentrant = state.held.iter().any(|(_, who, keys)| who == owner && resources.iter().all(|r| keys.iter().any(|k| overlap(k,r))));
                if !held && (!earlier || reentrant) {
                    state.waiting.retain(|(n,_,_)| *n != id); state.held.push((id, owner.into(), resources)); return guard;
                }
            }
            changed.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test] async fn reservations_serialize_overlap_without_blocking_other_projects_and_cancel_cleanly() {
        let pool = Arc::new(Resources::default());
        let first = pool.acquire("a", vec!["/config/project".into()]).await;
        let same = pool.acquire("a", vec!["/config/project/a.md".into()]).await;
        let other = pool.acquire("b", vec!["/config/project-b".into()]).await;
        let blocked = pool.acquire("b", vec!["/config/project/a.md".into()]);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), blocked).await.is_err());
        assert!(pool.state.lock().unwrap().waiting.is_empty());
        drop(first); drop(same); drop(other);
        let _next = pool.acquire("b", vec!["/config/project".into()]).await;
    }
    #[test] fn detects_packages_and_paths_and_keeps_names_canonical() {
        assert_eq!(normalize("/config/a/../b/").unwrap(), "/config/b");
        assert!(normalize("../../x").is_err());
        assert!(command_resources("apt-get install app", &[]).unwrap().contains(&"@packages".into()));
        assert!(command_resources("cat '/config/a.txt'", &[]).unwrap().contains(&"/config/a.txt".into()));
    }
}
