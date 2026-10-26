//! Opt-in terminal access to the machine running dotbot. A conversation reserves
//! the shared local environment until its turn ends or explicitly releases it.

use std::collections::{HashSet,HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::broadcast;

const OUTPUT_LIMIT: usize = 1_000_000;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    paused: bool,
}

pub struct LocalComputer {
    path: PathBuf,
    settings: Mutex<Settings>,
    owner: Mutex<Option<String>>,
    scopes: Mutex<HashMap<PathBuf,String>>,
    scope_operations: Mutex<HashMap<PathBuf,std::sync::Arc<tokio::sync::Mutex<()>>>>,
    stopped: Mutex<HashSet<String>>,
    pub(crate) operation: tokio::sync::Mutex<()>,
    cancel: broadcast::Sender<String>,
}

fn executable(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|dir| {
        std::fs::metadata(dir.join(name)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }))
}

async fn read_limited(mut stream: impl AsyncRead + Unpin, limit: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = [0; 8192];
    while let Ok(n) = stream.read(&mut buffer).await {
        if n == 0 { break; }
        out.extend_from_slice(&buffer[..n.min(limit.saturating_sub(out.len()))]);
    }
    out
}

fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // Negative PID addresses only the new process group we created.
        unsafe extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
        unsafe { kill(-(pid as i32), 9); }
    }
}

impl LocalComputer {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("local-computer.json");
        let settings = match crate::sqlite_storage::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading local-computer.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Settings::default(),
            Err(e) => return Err(e.into()),
        };
        let (cancel, _) = broadcast::channel(128);
        Ok(Self { path, settings: Mutex::new(settings), owner: Mutex::new(None), scopes:Mutex::new(HashMap::new()),scope_operations:Mutex::new(HashMap::new()),stopped: Mutex::new(HashSet::new()), operation: tokio::sync::Mutex::new(()), cancel })
    }

    pub fn enabled(&self) -> bool { self.settings.lock().unwrap().enabled }

    fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<()> {
        let mut current = self.settings.lock().unwrap();
        let mut settings = current.clone();
        change(&mut settings);
        crate::durable::save(&self.path, &settings)?;
        let stopping = !settings.enabled || settings.paused;
        *current = settings;
        if stopping {
            *self.owner.lock().unwrap() = None;
            let _ = self.cancel.send("*".into());
        }
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        self.update(|s| { s.enabled = enabled; if !enabled { s.paused = false; } })
    }

    pub fn pause(&self, paused: bool) -> Result<()> {
        self.update(|s| s.paused = paused)
    }

    pub fn status(&self) -> Value {
        let settings = self.settings.lock().unwrap().clone();
        json!({ "enabled": settings.enabled, "paused": settings.paused, "platform": std::env::consts::OS,
            "shell": "bash", "commands": executable("bash"), "owner": *self.owner.lock().unwrap(),"isolatedOwners":self.scopes.lock().unwrap().iter().map(|(path,key)|json!({"workspace":path,"conversation":key})).collect::<Vec<_>>() })
    }

    pub fn check(&self, key: &str, mutate: bool) -> Result<()> {
        let settings = self.settings.lock().unwrap();
        ensure!(settings.enabled, "Local terminal is off. Enable it in Settings before using local_ tools.");
        if mutate { ensure!(!settings.paused, "Local terminal commands are paused by the user."); }
        ensure!(!self.stopped.lock().unwrap().contains(key), "This conversation was stopped; local terminal work is cancelled.");
        Ok(())
    }

    pub fn begin_turn(&self, key: &str) { self.stopped.lock().unwrap().remove(key); }

    pub fn stop(&self, key: &str) {
        self.stopped.lock().unwrap().insert(key.into());
        let _ = self.cancel.send(key.into());
        self.release(key);
    }

    pub fn shutdown(&self) { let _ = self.cancel.send("*".into()); *self.owner.lock().unwrap() = None;self.scopes.lock().unwrap().clear(); }

    pub fn claim(&self, key: &str) -> Result<()> {
        let settings = self.settings.lock().unwrap();
        ensure!(settings.enabled, "Local terminal is off. Enable it in Settings before using local_ tools.");
        ensure!(!settings.paused, "Local terminal commands are paused by the user.");
        let stopped = self.stopped.lock().unwrap();
        ensure!(!stopped.contains(key), "This conversation was stopped; local terminal work is cancelled.");
        let mut owner = self.owner.lock().unwrap();
        if let Some(other) = owner.as_ref() { ensure!(other == key, "Local terminal is reserved by {other}. Coordinate with that Bot or finish independent work; do not make conflicting changes to shared local files."); }
        ensure!(self.scopes.lock().unwrap().values().all(|other|other==key),"其他任务正在操作独立目录，请等待它们释放后再修改共享目录。");
        *owner = Some(key.into());
        Ok(())
    }

    pub fn release(&self, key: &str) {
        let mut owner = self.owner.lock().unwrap();
        if owner.as_deref() == Some(key) { *owner = None; }
        let mut scopes=self.scopes.lock().unwrap();
        scopes.retain(|_,other|other!=key);
        self.scope_operations.lock().unwrap().retain(|path,operation|scopes.contains_key(path)||std::sync::Arc::strong_count(operation)>1);
    }

    pub fn claim_scope(&self,key:&str,path:&Path)->Result<std::sync::Arc<tokio::sync::Mutex<()>>> {
        self.check(key,true)?;let path=path.canonicalize()?;
        let owner=self.owner.lock().unwrap();ensure!(owner.as_ref().is_none_or(|other|other==key),"共享终端正由其他任务使用。");
        let mut scopes=self.scopes.lock().unwrap();ensure!(scopes.iter().all(|(p,other)|other==key||(!p.starts_with(&path)&&!path.starts_with(p))),"当前目录正由其他任务使用，请等待或选择独立目录。");
        scopes.insert(path.clone(),key.into());
        Ok(self.scope_operations.lock().unwrap().entry(path).or_insert_with(||std::sync::Arc::new(tokio::sync::Mutex::new(()))).clone())
    }

    /// Bounded output and a private process group: disabling, stopping and timeouts
    /// terminate only the processes started for this invocation, never user apps.
    async fn execute(&self, key: &str, mut command: Command, timeout: u64) -> Result<(Vec<u8>, Vec<u8>, bool)> {
        let mut cancel = self.cancel.subscribe();
        self.check(key, true)?;
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).process_group(0);
        let mut child = command.spawn().context("starting a local terminal command")?;
        let pid = child.id();
        let mut stdout = tokio::spawn(read_limited(child.stdout.take().unwrap(), OUTPUT_LIMIT));
        let mut stderr = tokio::spawn(read_limited(child.stderr.take().unwrap(), OUTPUT_LIMIT));
        let cancelled = async {
            loop {
                match cancel.recv().await {
                    Ok(who) if who == "*" || who == key => break,
                    Err(_) => break,
                    _ => {}
                }
            }
        };
        let result = tokio::select! {
            result = child.wait() => result.map_err(anyhow::Error::from),
            _ = tokio::time::sleep(Duration::from_secs(timeout)) => Err(anyhow!("Local command exceeded {timeout} seconds.")),
            _ = cancelled => Err(anyhow!("Local command was cancelled or access was disabled.")),
        };
        kill_group(pid);
        let _ = child.wait().await;
        let out = tokio::time::timeout(Duration::from_secs(2), &mut stdout).await.ok().and_then(Result::ok).unwrap_or_default();
        let err = tokio::time::timeout(Duration::from_secs(2), &mut stderr).await.ok().and_then(Result::ok).unwrap_or_default();
        stdout.abort(); stderr.abort();
        Ok((out, err, result?.success()))
    }

    pub async fn run(&self, key: &str, command: &str, cwd: &Path, timeout: u64, env: &[(String, String)]) -> Result<(String, bool)> {
        ensure!(!command.trim().is_empty(), "command is needed");
        ensure!(cwd.is_absolute() && cwd.is_dir(), "cwd must be an existing absolute directory");
        let mut process = Command::new("bash");
        process.args(["--noprofile", "--norc", "-c", command]).current_dir(cwd);
        for (name, value) in env { process.env(name, value); }
        let (out, err, ok) = self.execute(key, process, timeout.clamp(1, 1800)).await?;
        let mut text = String::from_utf8_lossy(&out).to_string();
        if !err.is_empty() { text.push_str(&format!("\n[stderr]\n{}", String::from_utf8_lossy(&err))); }
        if out.len() == OUTPUT_LIMIT || err.len() == OUTPUT_LIMIT { text.push_str("\n[output truncated]"); }
        Ok((text, ok))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn independent_scopes_overlap_and_cancellation_does_not_cross_tasks() {
        let home=tempfile::tempdir().unwrap();let computer=LocalComputer::open(home.path()).unwrap();computer.set_enabled(true).unwrap();
        let a=home.path().join("a");let b=home.path().join("b");std::fs::create_dir(&a).unwrap();std::fs::create_dir(&b).unwrap();
        let lock_a=computer.claim_scope("a",&a).unwrap();let lock_b=computer.claim_scope("b",&b).unwrap();
        assert!(computer.claim_scope("other",&a).is_err());assert!(computer.claim("shared").is_err());
        let run_a=async {let _guard=lock_a.lock().await;computer.run("a","touch started; sleep 5; touch forbidden",&a,10,&[]).await};
        let run_b=async {let _guard=lock_b.lock().await;computer.run("b","touch started; sleep 0.4; printf completed",&b,10,&[]).await};
        let stop=async {
            tokio::time::timeout(Duration::from_secs(3),async {while !a.join("started").exists()||!b.join("started").exists(){tokio::time::sleep(Duration::from_millis(10)).await;}}).await.unwrap();
            computer.stop("a");
        };
        let (result_a,result_b,_)=tokio::join!(run_a,run_b,stop);
        assert!(result_a.is_err());assert_eq!(result_b.unwrap(),("completed".into(),true));assert!(!a.join("forbidden").exists());
        drop(lock_a);drop(lock_b);computer.release("a");computer.release("b");
        assert!(computer.scope_operations.lock().unwrap().is_empty());computer.claim("shared").unwrap();
    }

    #[tokio::test]
    async fn opt_in_persists_and_stopping_cancels_only_owned_commands() {
        let home = tempfile::tempdir().unwrap();
        let computer = LocalComputer::open(home.path()).unwrap();
        assert!(!computer.enabled());
        assert!(computer.run("bot:a", "printf forbidden", home.path(), 5, &[]).await.is_err());
        computer.set_enabled(true).unwrap();
        assert!(LocalComputer::open(home.path()).unwrap().enabled());
        let (text, ok) = computer.run("bot:a", "printf '%s' \"$DOTBOT_SECRET_TEST\"", home.path(), 5, &[("DOTBOT_SECRET_TEST".into(), "a quoted ' value".into())]).await.unwrap();
        assert!(ok); assert_eq!(text, "a quoted ' value");
        let slow = computer.run("bot:a", "sleep 30", home.path(), 35, &[]);
        let stop = async { tokio::time::sleep(Duration::from_millis(100)).await; computer.stop("bot:a"); };
        let (result, _) = tokio::join!(slow, stop);
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        computer.begin_turn("bot:a");
        computer.claim("bot:a").unwrap();
        assert!(computer.claim("bot:b").is_err());
        computer.release("bot:b");
        assert_eq!(computer.status()["owner"], "bot:a");
        computer.release("bot:a");
        computer.claim("bot:b").unwrap();
        computer.pause(true).unwrap();
        assert!(computer.claim("bot:b").is_err());
        assert!(computer.check("bot:b", false).is_ok());
        assert!(LocalComputer::open(home.path()).unwrap().status()["paused"].as_bool().unwrap());
        computer.set_enabled(false).unwrap();
        assert!(!LocalComputer::open(home.path()).unwrap().enabled());
    }

    #[tokio::test]
    async fn commands_use_explicit_directories_and_bound_failures_and_output() {
        let home = tempfile::tempdir().unwrap();
        let computer = LocalComputer::open(home.path()).unwrap(); computer.set_enabled(true).unwrap();
        assert!(computer.run("bot:a", "pwd", Path::new("relative"), 5, &[]).await.is_err());
        let (text, ok) = computer.run("bot:a", "pwd; printf fail >&2; exit 7", home.path(), 5, &[]).await.unwrap();
        assert!(!ok); assert!(text.starts_with(home.path().to_str().unwrap())); assert!(text.contains("[stderr]\nfail"));
        let (text, ok) = computer.run("bot:a", "head -c 1100000 /dev/zero", home.path(), 5, &[]).await.unwrap();
        assert!(ok); assert!(text.ends_with("[output truncated]")); assert!(text.len() < 1_001_000);
        assert!(computer.run("bot:a", "sleep 30", home.path(), 1, &[]).await.unwrap_err().to_string().contains("exceeded"));
    }
}
