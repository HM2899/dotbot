//! One persistent team computer, with an independent X11/DBus/streaming seat per Bot.
//! Software and /config are shared; screens are not filesystem security boundaries.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use tokio::process::Command;
use tokio::sync::Mutex;
use std::sync::Arc;

/// The desktops' own docker network, on its own bridge, so the user can shut it off from
/// this computer with one firewall rule (see the README).
pub const NETWORK: &str = "teambot-desktops";
const BRIDGE: &str = "br-teambot";
pub const DEFAULT_IMAGE: &str = "lscr.io/linuxserver/webtop:ubuntu-xfce";
/// Screenshots are at most this wide, so the model sees them unscaled and its clicks land.
const SHOT_WIDTH: u32 = 1280;
const SCREEN_SCRIPT: &str = include_str!("../deploy/desktop/screen.sh");
const SESSION_SCRIPT: &str = include_str!("../deploy/desktop/session.sh");
const LAYOUT_SCRIPT: &str = include_str!("../deploy/desktop/layout.py");
const BROWSER_SCRIPT: &str = include_str!("../deploy/desktop/browser.sh");
const CANCEL_COMMAND: &str = include_str!("../deploy/desktop/cancel.py");
/// The user keeps control of a desktop while the page renews it; a closed tab gives it back.
const CONTROL_LEASE: Duration = Duration::from_secs(60);
/// Runs as root when the container starts: the tools the Bot drives the desktop with.
const INIT_SCRIPT: &str = "#!/bin/sh
command -v xdotool >/dev/null && command -v import >/dev/null && exit 0
apt-get update -qq && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends xdotool imagemagick x11-utils >/dev/null
";

pub struct Desktops {
    image: String,
    dir: PathBuf,
    /// Screen pixels per screenshot pixel, from each Bot's last screenshot.
    scale: Mutex<HashMap<String, f64>>,
    /// Stable seats survive service/container restarts; IDs are never recycled.
    screens: Mutex<Option<HashMap<String, u32>>>,
    /// The desktops that answered as ready since they last started.
    ready: Mutex<HashSet<String>>,
    /// Until when the user has a desktop's controls; the Bot's desktop steps wait meanwhile.
    control: Mutex<HashMap<String, Instant>>,
    starting: Mutex<()>,
    stopping: AtomicBool,
    prepared: AtomicBool,
    retired_sync: AtomicBool,
    /// The Bot whose screen shows the team's one browser right now.
    browser_holder: Mutex<Option<String>>,
    layout_ready: Mutex<HashSet<String>>,
    pub resources: Arc<crate::desktop_resources::Resources>,
    reservations: Mutex<HashMap<String, Vec<crate::desktop_resources::Guard>>>,
    commands: Arc<std::sync::Mutex<HashSet<String>>>,
    /// Last desktop step or user control per Bot; service start counts as activity.
    activity: Mutex<HashMap<String, Instant>>,
}

pub struct CommandRecord { commands: Arc<std::sync::Mutex<HashSet<String>>>, id: String }
impl Drop for CommandRecord {
    fn drop(&mut self) { self.commands.lock().unwrap().remove(&self.id); }
}

/// What keeping a desktop up found.
#[derive(Debug, PartialEq)]
pub enum Kept {
    Up,
    Restarted,
    Down(String),
}

pub struct Shot {
    pub mime: String,
    pub base64: String,
}

/// A web address for the browser: HTTP(S) only, without credentials; empty when none.
pub fn browser_url(url: &str) -> Result<String> {
    let raw = url.trim();
    if raw.is_empty() { return Ok(String::new()); }
    if raw.len() > 8192 { bail!("The web address is too long."); }
    let normalized = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let parsed = reqwest::Url::parse(&normalized).map_err(|_| anyhow!("Enter a valid HTTP or HTTPS web address."))?;
    if !["http", "https"].contains(&parsed.scheme()) || parsed.host_str().is_none() || !parsed.username().is_empty() || parsed.password().is_some() {
        bail!("Enter a valid HTTP or HTTPS web address.");
    }
    Ok(parsed.to_string())
}

/// Fixed desktop launchers for the user's toolbar and the model's browser tool.
/// URL data is parsed before it reaches a shell, and is always a quoted argument.
fn app_command(app: &str, url: &str) -> Result<String> {
    match app {
        "browser" => {
            let url = browser_url(url)?;
            let argument = if url.is_empty() { String::new() } else { format!(" {}", shell_quote(&url)) };
            Ok(format!("exec /bin/bash /opt/teambot-desktop/browser.sh{argument}"))
        }
        "files" if url.trim().is_empty() => Ok("thunar /config".into()),
        "terminal" if url.trim().is_empty() => Ok("xfce4-terminal --disable-server".into()),
        _ => bail!("Choose the desktop browser, files or terminal."),
    }
}

async fn docker(args: &[&str]) -> Result<std::process::Output> {
    Command::new("docker").args(args).stdin(Stdio::null()).output().await.context("running docker")
}

impl Desktops {
    pub fn new(image: String, dir: PathBuf) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        let saved: HashMap<String, i64> = crate::sqlite_storage::read(dir.join("runtime/control.json")).ok().and_then(|v| serde_json::from_slice(&v).ok()).unwrap_or_default();
        let control = saved.into_iter().filter_map(|(bot, until)| {let remaining = until.saturating_sub(now).clamp(0, CONTROL_LEASE.as_millis() as i64); (remaining > 0).then(|| (bot, Instant::now() + Duration::from_millis(remaining as u64)))}).collect();
        Desktops { image, dir, scale: Mutex::new(HashMap::new()), screens: Mutex::new(None), ready: Mutex::new(HashSet::new()), control: Mutex::new(control), starting: Mutex::new(()), stopping: AtomicBool::new(false), prepared: AtomicBool::new(false), retired_sync: AtomicBool::new(false), browser_holder: Mutex::new(None), layout_ready: Mutex::new(HashSet::new()), resources: Arc::default(), reservations: Mutex::new(HashMap::new()), commands: Arc::default(), activity: Mutex::new(HashMap::new()) }
    }

    /// Whether docker answers at all.
    pub async fn available() -> bool {
        docker(&["version", "--format", "{{.Server.Version}}"]).await.is_ok_and(|o| o.status.success())
    }

    /// The host folder that is the desktop user's home (/config in the container).
    pub fn home_of(&self, _bot_id: &str) -> PathBuf {
        self.dir.join("shared")
    }

    /// Maps a path inside the desktop to the same file on the host, when it is under the home.
    /// The desktop is root's to change, so a link there to a file elsewhere on this computer
    /// is not followed: the real path must stay inside the home.
    pub fn host_path(&self, bot_id: &str, inside: &str) -> Option<PathBuf> {
        let rest = if inside == "/config" { "" } else { inside.strip_prefix("/config/")? };
        let home = self.home_of(bot_id).canonicalize().ok()?;
        let path = home.join(rest).canonicalize().ok()?;
        path.starts_with(&home).then_some(path)
    }

    /// A folder inside the desktop's home, on this computer, made when missing; None when
    /// it would lead out of the home.
    pub fn host_dir(&self, bot_id: &str, inside: &str) -> Option<PathBuf> {
        let rest = inside.strip_prefix("/config/")?;
        let parts: Vec<_> = std::path::Path::new(rest).components().collect();
        if parts.iter().any(|p| !matches!(p, std::path::Component::Normal(_))) { return None; }
        std::fs::create_dir_all(self.home_of(bot_id)).ok()?;
        let home = self.home_of(bot_id).canonicalize().ok()?;
        let mut path = home.clone();
        for part in parts {
            path.push(part.as_os_str());
            if !path.exists() { std::fs::create_dir(&path).ok()?; }
            path = path.canonicalize().ok()?;
            if !path.starts_with(&home) || !path.is_dir() { return None; }
        }
        Some(path)
    }

    pub fn container_name(&self) -> String {
        format!("teambot-computer-{}", crate::memory::safe_id(&format!("home:{}", self.dir.display())))
    }

    pub async fn screen(&self, bot_id: &str) -> Result<u32> {
        let mut guard = self.screens.lock().await;
        if guard.is_none() {
            let path = self.dir.join("screens.json");
            let seats: HashMap<String, u32> = if path.exists() { serde_json::from_slice(&crate::sqlite_storage::read(path)?)? } else { HashMap::new() };
            let unique: HashSet<_> = seats.values().collect();
            if unique.len() != seats.len() || seats.values().any(|n| !(10..=60000).contains(n)) { bail!("桌面屏幕配置无效"); }
            *guard = Some(seats);
        }
        let seats = guard.as_mut().unwrap();
        if let Some(n) = seats.get(bot_id) { return Ok(*n); }
        let n = seats.values().max().copied().unwrap_or(9) + 1;
        if n > 60000 { bail!("桌面屏幕数量已达上限"); }
        let mut next = seats.clone();
        next.insert(bot_id.into(), n);
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("screens.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&next)?)?;
        std::fs::rename(tmp, self.dir.join("screens.json"))?;
        *seats = next;
        Ok(n)
    }

    fn seat_env(n: u32) -> Vec<String> {
        vec!["HOME=/config".into(), format!("DISPLAY=:{n}"), format!("TEAMBOT_SCREEN_RUNTIME=/run/teambot/s{n}"),
            format!("XDG_CONFIG_HOME=/config/.teambot/s{n}/config"), format!("XDG_CACHE_HOME=/config/.teambot/s{n}/cache"),
            "XDG_DATA_HOME=/config/.local/share".into(), format!("XDG_RUNTIME_DIR=/run/teambot/s{n}/xdg"),
            "TEAMBOT_BROWSER_HOME=/config/.teambot/browser".into()]
    }

    pub async fn socket(&self, bot_id: &str) -> Option<PathBuf> {
        if !self.ready.lock().await.contains(bot_id) { return None; }
        let n = self.screen(bot_id).await.ok()?;
        Some(self.dir.join(format!("runtime/s{n}/stream.sock")))
    }

    /// Start the shared computer, then only this Bot's screen. Never replace foreign containers.
    pub async fn ensure(&self, bot_id: &str) -> Result<String> {
        let _guard = self.starting.lock().await;
        if self.stopping.load(Ordering::Relaxed) { bail!("服务正在关闭"); }
        if !self.prepared.load(Ordering::Relaxed) {
            let scripts = self.dir.join("scripts");
            std::fs::create_dir_all(&scripts)?;
            std::fs::write(scripts.join("screen.sh"), SCREEN_SCRIPT)?;
            std::fs::write(scripts.join("session.sh"), SESSION_SCRIPT)?;
            std::fs::write(scripts.join("layout.py"), LAYOUT_SCRIPT)?;
            std::fs::write(scripts.join("browser.sh"), BROWSER_SCRIPT)?;
            // The browser is shared directly now; nothing is synchronized between profiles.
            let _ = std::fs::remove_dir_all(scripts.join("login-sync"));
            self.prepared.store(true, Ordering::Relaxed);
        }
        if let Some(socket) = self.socket(bot_id).await
            && tokio::net::UnixStream::connect(socket).await.is_ok() {
            self.retire_login_sync().await;
            self.ensure_layout(bot_id).await?;
            return Ok(format!("/desktop/{bot_id}/"));
        }
        let name = self.container_name();
        let state = docker(&["inspect", "-f", "{{.State.Running}}", &name]).await?;
        if !state.status.success() {
            self.ready.lock().await.clear();
            self.create(&name).await?;
        } else {
            let owned = docker(&["inspect", "-f", "{{index .Config.Labels \"teambot.home\"}}", &name]).await?;
            if String::from_utf8_lossy(&owned.stdout).trim() != self.home_label().trim_start_matches("teambot.home=") {
                bail!("共享电脑名称被其他实例占用，未修改现有容器");
            }
            if String::from_utf8_lossy(&state.stdout).trim() != "true" {
                self.ready.lock().await.clear();
                if !self.remount_legacy(&name).await? {
                    let out = docker(&["start", &name]).await?;
                    if !out.status.success() { bail!("共享电脑启动失败：{}", String::from_utf8_lossy(&out.stderr)); }
                }
            }
        }
        let n = self.screen(bot_id).await?;
        let runtime = self.dir.join(format!("runtime/s{n}"));
        std::fs::create_dir_all(&runtime)?;
        let env = Self::seat_env(n);
        let mut args = vec!["exec", "-d", "-u", "abc"];
        for item in &env { args.extend(["-e", item]); }
        args.extend([&name, "setsid", "bash", "/opt/teambot-desktop/screen.sh"]);
        for attempt in 0..360 {
            if self.stopping.load(Ordering::Relaxed) { bail!("服务正在关闭"); }
            if tokio::net::UnixStream::connect(runtime.join("stream.sock")).await.is_ok() {
                self.ready.lock().await.insert(bot_id.into());
                self.retire_login_sync().await;
                self.ensure_layout(bot_id).await?;
                return Ok(format!("/desktop/{bot_id}/"));
            }
            // webtop initially remaps abc's UID and installs tools. Detached exec can
            // report success even if the script exits before that initialization finishes.
            if attempt % 4 == 0 {
                let check = format!("command -v xdotool >/dev/null && command -v import >/dev/null && test -w /run/teambot/s{n}");
                if docker(&["exec", "-u", "abc", &name, "sh", "-c", &check]).await.is_ok_and(|o| o.status.success()) {
                    let out = docker(&args).await?;
                    if !out.status.success() { bail!("桌面启动失败：{}", String::from_utf8_lossy(&out.stderr)); }
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        bail!("桌面未能在三分钟内启动，请检查 runtime/s{n} 下的日志")
    }

    async fn ensure_layout(&self, bot_id: &str) -> Result<()> {
        let mut ready = self.layout_ready.lock().await;
        if ready.contains(bot_id) { return Ok(()); }
        let n = self.screen(bot_id).await?;
        let env = Self::seat_env(n);
        let name = self.container_name();
        let mut args = vec!["exec", "-u", "abc"];
        for item in &env { args.extend(["-e", item]); }
        args.extend([&name, "bash", "-c", "export DBUS_SESSION_BUS_ADDRESS=$(cat \"$TEAMBOT_SCREEN_RUNTIME/bus\"); exec /usr/bin/python3 /opt/teambot-desktop/layout.py"]);
        if docker(&args).await?.status.success() { ready.insert(bot_id.to_string()); }
        Ok(())
    }

    /// Earlier versions ran a daemon copying logins between per-screen browsers.
    /// One shared profile replaces it; stop a copy left running in the computer.
    async fn retire_login_sync(&self) {
        if self.retired_sync.swap(true, Ordering::Relaxed) { return; }
        let _ = docker(&["exec", "-u", "abc", &self.container_name(), "pkill", "-f", "/opt/teambot-desktop/login-sync/sync.py"]).await;
    }

    /// Which Bot's screen has the browser; None when no Bot holds it.
    pub async fn browser_holder(&self) -> Option<String> { self.browser_holder.lock().await.clone() }
    pub async fn set_browser_holder(&self, bot_id: Option<&str>) { *self.browser_holder.lock().await = bot_id.map(String::from); }

    pub async fn retain_resources(&self, owner: &str, guard: crate::desktop_resources::Guard) {
        self.reservations.lock().await.entry(owner.into()).or_default().push(guard);
    }
    pub async fn release_resources(&self, owner: &str) {
        self.reservations.lock().await.remove(owner);
    }

    pub fn track_command(&self, id: &str) -> CommandRecord {
        self.commands.lock().unwrap().insert(id.into());
        CommandRecord { commands: self.commands.clone(), id: id.into() }
    }

    pub async fn finish_commands(&self) {
        // A stopped model may exit before its HTTP tool/supervisor notices the
        // cancellation. Do this before exec replaces the runtime during upgrade.
        let ids: Vec<_> = self.commands.lock().unwrap().iter().cloned().collect();
        for id in ids {
            if self.cancel_command(&id).await.is_err() {
                eprintln!("dotbot: 升级前仍在等待受管远程命令结束，尚未释放共享资源。");
                while self.commands.lock().unwrap().contains(&id) { tokio::time::sleep(Duration::from_millis(100)).await; }
            }
        }
    }

    pub fn canonical_resources(&self, keys: Vec<String>) -> Vec<String> {
        keys.into_iter().map(|key| {
            let Some(rest) = key.strip_prefix("/config/") else { return key };
            let home = self.home_of("");
            let mut path = home.join(rest); let mut suffix = vec![];
            while !path.exists() { if let Some(name) = path.file_name() { suffix.push(name.to_os_string()); } else { return key }; if !path.pop() { return key; } }
            let Ok(mut path) = path.canonicalize() else { return key };
            for part in suffix.into_iter().rev() { path.push(part); }
            let Ok(home) = home.canonicalize() else { return key };
            path.strip_prefix(home).map(|p| format!("/config/{}", p.display())).unwrap_or(key)
        }).collect()
    }

    /// Preserve existing applications in a snapshot; old containers and homes are untouched.
    async fn migration_image(&self) -> Result<String> {
        let manifest = self.dir.join("migration.json");
        if manifest.exists() {
            let value: serde_json::Value = serde_json::from_slice(&crate::sqlite_storage::read(manifest)?)?;
            let image = value["image"].as_str().context("迁移镜像记录无效")?;
            if image.ends_with(":migration") {
                let inspected = docker(&["image", "inspect", image, "--format", "{{.Architecture}}"]).await?;
                let architecture = String::from_utf8_lossy(&inspected.stdout).trim().to_string();
                let native = match std::env::consts::ARCH { "aarch64" => "arm64", "x86_64" => "amd64", arch => arch };
                if !inspected.status.success() || architecture != native {
                    eprintln!("dotbot: 旧电脑的软件快照在本机不存在或架构不匹配，将使用配置的桌面镜像。共享文件保留；旧容器里额外安装的软件需要在新机器重新安装。");
                    return Ok(self.image.clone());
                }
            }
            return Ok(image.into());
        }
        let label = format!("label={}", self.home_label());
        let out = docker(&["ps", "-aq", "--filter", "label=teambot.desktop=1", "--filter", &label]).await?;
        if !out.status.success() { bail!("无法检查旧电脑"); }
        let mut sources = Vec::new();
        for id in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            let seen = docker(&["inspect", "--size", id]).await?;
            let items: Vec<serde_json::Value> = serde_json::from_slice(&seen.stdout)?;
            let item = &items[0];
            if item["Config"]["Image"].as_str() == Some(&self.image) && item["Name"].as_str().is_some_and(|n| n.starts_with("/teambot-desktop-")) {
                sources.push((item["SizeRw"].as_u64().unwrap_or(0), id.to_string(), item.clone()));
            }
        }
        sources.sort_by(|a,b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut image = self.image.clone();
        let mut source = None;
        if let Some((_, id, info)) = sources.first() {
            let busy = docker(&["exec", id, "sh", "-c", "pgrep -x 'apt|apt-get|dpkg' >/dev/null"]).await?;
            if busy.status.success() { bail!("旧电脑正在安装软件，安装完成后会重试共享电脑迁移"); }
            image = format!("{}:migration", self.container_name());
            let out = docker(&["commit", "--pause=false", id, &image]).await?;
            if !out.status.success() { bail!("保留旧软件失败：{}", String::from_utf8_lossy(&out.stderr)); }
            if let Some(home) = info["Mounts"].as_array().and_then(|m| m.iter().find(|m| m["Destination"] == "/config")).and_then(|m| m["Source"].as_str()) {
                // No-clobber copy keeps files already received into the new shared inbox.
                let out = Command::new("cp").args(["-a", "-n", &format!("{home}/."), &self.home_of("").display().to_string()]).output().await?;
                if !out.status.success() { bail!("复制旧电脑文件失败：{}", String::from_utf8_lossy(&out.stderr)); }
            }
            source = Some(id);
        }
        let record = serde_json::json!({"image":image,"source":source,"legacy":"旧容器保留；旧 Bot 目录不再挂载，文件保留在主机"});
        let tmp = self.dir.join("migration.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&record)?)?;
        std::fs::rename(tmp, manifest)?;
        Ok(image)
    }

    /// A stopped computer from an older version still mounts old per-Bot homes under
    /// /config/legacy; file dialogs list each as a drive. Snapshot its software and recreate
    /// it without them. Their files stay on the host. Running computers are never replaced.
    async fn remount_legacy(&self, name: &str) -> Result<bool> {
        let out = docker(&["inspect", "-f", "{{json .Mounts}}", name]).await?;
        if !out.status.success() { return Ok(false); }
        let mounts: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        if !mounts.iter().any(|m| m["Destination"].as_str().is_some_and(|d| d.starts_with("/config/legacy/"))) { return Ok(false); }
        let image = format!("{name}:migration");
        let out = docker(&["commit", name, &image]).await?;
        if !out.status.success() { bail!("保留共享电脑软件失败：{}", String::from_utf8_lossy(&out.stderr)); }
        let manifest = self.dir.join("migration.json");
        let mut record: serde_json::Value = std::fs::read(&manifest).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| serde_json::json!({}));
        record["image"] = image.into();
        record["legacy"] = "旧 Bot 目录不再挂载，文件保留在主机".into();
        let tmp = self.dir.join("migration.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&record)?)?;
        std::fs::rename(tmp, manifest)?;
        let out = docker(&["rm", name]).await?;
        if !out.status.success() { bail!("更新共享电脑挂载失败：{}", String::from_utf8_lossy(&out.stderr)); }
        self.create(name).await?;
        Ok(true)
    }

    async fn create(&self, name: &str) -> Result<()> {
        let home = self.home_of("");
        let init = self.dir.join("init");
        let scripts = self.dir.join("scripts");
        for dir in [&home, &init, &scripts, &self.dir.join("runtime")] { std::fs::create_dir_all(dir)?; }
        let script = init.join("50-teambot-tools.sh");
        std::fs::write(&script, INIT_SCRIPT)?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
        std::fs::write(scripts.join("screen.sh"), SCREEN_SCRIPT)?;
        std::fs::write(scripts.join("session.sh"), SESSION_SCRIPT)?;
        std::fs::write(scripts.join("layout.py"), LAYOUT_SCRIPT)?;
        let image = self.migration_image().await?;
        let uid = String::from_utf8_lossy(&Command::new("id").arg("-u").output().await?.stdout).trim().to_string();
        let gid = String::from_utf8_lossy(&Command::new("id").arg("-g").output().await?.stdout).trim().to_string();
        if !docker(&["network", "inspect", NETWORK]).await.is_ok_and(|o| o.status.success()) {
            let bridge = format!("com.docker.network.bridge.name={BRIDGE}");
            let out = docker(&["network", "create", "--driver", "bridge", "-o", &bridge, NETWORK]).await?;
            if !out.status.success() && !String::from_utf8_lossy(&out.stderr).contains("already exists") { bail!("无法创建电脑网络"); }
        }
        let mut args = vec!["run".to_string(), "-d".into(), "--name".into(), name.into(), "--label".into(), "teambot.desktop=1".into(), "--label".into(), self.home_label(),
            "--label".into(), "teambot.shared=1".into(), "--shm-size".into(), "1g".into(), "--security-opt".into(), "seccomp=unconfined".into(), "--network".into(), NETWORK.into(),
            "-e".into(), format!("PUID={uid}"), "-e".into(), format!("PGID={gid}"), "-e".into(), "TITLE=dotbot".into()];
        for (host, dest) in [(home, "/config"), (init, "/custom-cont-init.d:ro"), (scripts, "/opt/teambot-desktop:ro"), (self.dir.join("runtime"), "/run/teambot")] {
            args.extend(["-v".into(), format!("{}:{dest}", host.display())]);
        }
        // Old per-Bot homes are no longer mounted. Remove only the empty mount points they left.
        let legacy = self.home_of("").join("legacy");
        if let Ok(stubs) = std::fs::read_dir(&legacy) {
            for stub in stubs.flatten() { let _ = std::fs::remove_dir(stub.path()); }
            let _ = std::fs::remove_dir(&legacy);
        }
        args.push(image);
        let refs: Vec<_> = args.iter().map(String::as_str).collect();
        let out = docker(&refs).await?;
        if !out.status.success() { bail!("共享电脑启动失败：{}", String::from_utf8_lossy(&out.stderr)); }
        Ok(())
    }

    /// Runs a shell command in the desktop as its user, on its screen; (output, success).
    pub async fn run(&self, bot_id: &str, command: &str, timeout_secs: u64) -> Result<(String, bool)> {
        self.run_with_env(bot_id, command, timeout_secs, &[]).await
    }

    /// Values stay in the Docker client's environment, never command arguments.
    /// A timeout inside the container also stops the remote process tree.
    pub async fn run_with_env(&self, bot_id: &str, command: &str, timeout_secs: u64, env: &[(String, String)]) -> Result<(String, bool)> {
        self.ensure(bot_id).await?;
        let name = self.container_name();
        let mut child = Command::new("docker");
        child.args(["exec", "-u", "abc", "-w", "/config"]);
        for item in Self::seat_env(self.screen(bot_id).await?) { child.arg("-e").arg(item); }
        for (key, value) in env { child.arg("-e").arg(key).env(key, value); }
        let script = format!("if [ -n \"${{TEAMBOT_COMMAND_ID:-}}\" ] && [ -e \"/run/teambot/cancelled/$TEAMBOT_COMMAND_ID\" ]; then exit 125; fi; export DBUS_SESSION_BUS_ADDRESS=$(cat \"$TEAMBOT_SCREEN_RUNTIME/bus\"); {command}");
        child.args([&name, "setsid", "--wait", "timeout", "--signal=TERM", "--kill-after=5", &format!("{timeout_secs}s"), "bash", "-lc", &script]).stdin(Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(timeout_secs + 10), child.output()).await.map_err(|_| anyhow!("the command exceeded its remote timeout of {timeout_secs} s"))??;
        let mut text = String::from_utf8_lossy(&out.stdout).to_string();
        let err = String::from_utf8_lossy(&out.stderr);
        if !err.trim().is_empty() {
            text.push_str(&format!("\n[stderr]\n{err}"));
        }
        Ok((text, out.status.success()))
    }

    pub async fn cancel_command(&self, id: &str) -> Result<()> {
        // Match our unguessable environment marker and only a timeout process
        // that leads its own group. Detached applications/user windows survive.
        // Container root is needed for a command's elevated child processes,
        // never for host administration or an unrelated application group.
        let name = self.container_name();
        let discovered = docker(&["exec", "-u", "abc", &name, "python3", "-c", CANCEL_COMMAND, id, "--discover"]).await?;
        if !discovered.status.success() { bail!("未能确认本次命令的进程身份。"); }
        let groups = String::from_utf8(discovered.stdout)?;
        if groups.len() > 4096 { bail!("命令进程组信息无效。"); }
        let stopped = docker(&["exec", &name, "python3", "-c", CANCEL_COMMAND, id, groups.trim()]).await?;
        if !stopped.status.success() { bail!("未能确认本次命令已停止。"); }
        Ok(())
    }

    /// Starts a program on the desktop and leaves it running.
    pub async fn launch(&self, bot_id: &str, command: &str) -> Result<()> {
        let log = format!("/tmp/teambot-launch-{}.log", uuid::Uuid::new_v4());
        let detached = format!("umask 077; nohup setsid sh -c {} >{} 2>&1 & teambot_app_pid=$!; mkdir -p \"$TEAMBOT_SCREEN_RUNTIME/apps\"; printf '%s\\n' \"$teambot_app_pid\" >\"$TEAMBOT_SCREEN_RUNTIME/apps/$teambot_app_pid\"; sleep 1; if kill -0 \"$teambot_app_pid\" 2>/dev/null; then rm -f {}; else wait \"$teambot_app_pid\"; teambot_app_status=$?; head -c 1200 {}; rm -f {}; exit \"$teambot_app_status\"; fi", shell_quote(command), shell_quote(&log), shell_quote(&log), shell_quote(&log), shell_quote(&log));
        let (output, ok) = self.run(bot_id, &detached, 20).await?;
        if !ok { bail!("Desktop launch failed: {}", output.trim()); }
        Ok(())
    }

    pub async fn open_app(&self, bot_id: &str, app: &str, url: &str) -> Result<()> {
        if app == "browser" { self.set_browser_holder(Some(bot_id)).await; }
        let command = app_command(app, url)?;
        self.launch(bot_id, &command).await
    }

    /// The screen now, as a JPEG at most SHOT_WIDTH wide.
    pub async fn screenshot(&self, bot_id: &str) -> Result<Shot> {
        let script = format!("import -window root -resize '{SHOT_WIDTH}x>' -quality 80 jpg:- | base64 -w0; echo; xdotool getdisplaygeometry");
        let (out, ok) = self.run(bot_id, &script, 30).await?;
        if !ok {
            bail!("no screenshot: {}", out.trim());
        }
        let mut lines = out.lines();
        let base64 = lines.next().unwrap_or_default().trim().to_string();
        let width: f64 = lines.next().and_then(|g| g.split_whitespace().next()).and_then(|w| w.parse().ok()).unwrap_or(SHOT_WIDTH as f64);
        let scale = if width > SHOT_WIDTH as f64 { width / SHOT_WIDTH as f64 } else { 1.0 };
        self.scale.lock().await.insert(bot_id.to_string(), scale);
        Ok(Shot { mime: "image/jpeg".into(), base64 })
    }

    /// Screenshot pixels to screen pixels.
    async fn to_screen(&self, bot_id: &str, x: f64, y: f64) -> (i64, i64) {
        let scale = self.scale.lock().await.get(bot_id).copied().unwrap_or(1.0);
        ((x * scale).round() as i64, (y * scale).round() as i64)
    }

    pub async fn click(&self, bot_id: &str, x: f64, y: f64, button: &str, count: u32) -> Result<()> {
        let (x, y) = self.to_screen(bot_id, x, y).await;
        let button = match button {
            "right" => 3,
            "middle" => 2,
            _ => 1,
        };
        self.xdotool(bot_id, &format!("xdotool mousemove {x} {y} click --repeat {} --delay 80 {button}", count.clamp(1, 3))).await
    }

    pub async fn drag(&self, bot_id: &str, from: (f64, f64), to: (f64, f64)) -> Result<()> {
        let (x1, y1) = self.to_screen(bot_id, from.0, from.1).await;
        let (x2, y2) = self.to_screen(bot_id, to.0, to.1).await;
        self.xdotool(bot_id, &format!("xdotool mousemove {x1} {y1} mousedown 1 sleep 0.2 mousemove {x2} {y2} sleep 0.2 mouseup 1")).await
    }

    pub async fn type_text(&self, bot_id: &str, text: &str) -> Result<()> {
        // xdotool types a character outside the keymap by remapping a spare keycode; at 15 ms
        // the next remap can land before the app reads the last one, and a character repeats.
        let delay = if text.is_ascii() { 15 } else { 40 };
        self.xdotool(bot_id, &format!("xdotool type --delay {delay} -- {}", shell_quote(text))).await
    }

    /// Keys like "Return", "ctrl+l", "alt+F4"; several separated by spaces.
    pub async fn key(&self, bot_id: &str, keys: &str) -> Result<()> {
        let keys: Vec<String> = keys.split_whitespace().map(shell_quote).collect();
        if keys.is_empty() {
            bail!("no key given");
        }
        self.xdotool(bot_id, &format!("xdotool key --delay 60 {}", keys.join(" "))).await
    }

    pub async fn scroll(&self, bot_id: &str, x: Option<(f64, f64)>, dy: i64) -> Result<()> {
        let at = match x {
            Some((x, y)) => {
                let (x, y) = self.to_screen(bot_id, x, y).await;
                format!("mousemove {x} {y} ")
            }
            None => String::new(),
        };
        let button = if dy < 0 { 4 } else { 5 };
        let clicks = (dy.unsigned_abs() / 100).clamp(1, 30);
        self.xdotool(bot_id, &format!("xdotool {at}click --repeat {clicks} --delay 30 {button}")).await
    }

    async fn xdotool(&self, bot_id: &str, command: &str) -> Result<()> {
        let (out, ok) = self.run(bot_id, command, 60).await?;
        if !ok {
            bail!("{}", out.trim());
        }
        Ok(())
    }

    /// Copies a file out of the desktop to the host, for paths outside its home.
    pub async fn fetch(&self, bot_id: &str, inside: &str, to: &std::path::Path) -> Result<()> {
        self.ensure(bot_id).await?;
        let source = format!("{}:{inside}", self.container_name());
        let out = docker(&["cp", "-L", &source, &to.display().to_string()]).await?;
        if !out.status.success() {
            bail!("no file at {inside} on the desktop");
        }
        Ok(())
    }

    async fn running(&self, bot_id: &str) -> bool {
        let Some(socket) = self.socket(bot_id).await else { return false };
        tokio::net::UnixStream::connect(socket).await.is_ok()
    }

    pub async fn is_ready(&self, bot_id: &str) -> bool {
        self.ready.lock().await.contains(bot_id)
    }

    /// Starts the desktop when it is not running, or restarts it when it stopped.
    pub async fn keep_up(&self, bot_id: &str) -> Kept {
        let was_ready = self.is_ready(bot_id).await;
        if was_ready && self.running(bot_id).await {
            return Kept::Up;
        }
        self.ready.lock().await.remove(bot_id);
        match self.ensure(bot_id).await {
            Ok(_) if was_ready => Kept::Restarted,
            Ok(_) => Kept::Up,
            Err(error) => Kept::Down(format!("{error:#}")),
        }
    }

    /// Gives the user the desktop's controls (renewed by the page), or gives them back.
    pub async fn set_control(&self, bot_id: &str, user: bool) {
        self.touch(bot_id).await;
        let mut control = self.control.lock().await;
        if user {
            control.insert(bot_id.to_string(), Instant::now() + CONTROL_LEASE);
        } else {
            control.remove(bot_id);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let saved: HashMap<_,_> = control.iter().filter_map(|(bot, until)| until.checked_duration_since(Instant::now()).map(|left| (bot.clone(), now + left.as_millis() as i64))).collect();
        if crate::durable::save(&self.dir.join("runtime/control.json"), &saved).is_err() { eprintln!("dotbot: 桌面接管状态暂未保存，当前接管仍有效。"); }
    }

    pub async fn touch(&self, bot_id: &str) {
        self.activity.lock().await.insert(bot_id.to_string(), Instant::now());
    }

    pub async fn idle_for(&self, bot_id: &str) -> Duration {
        self.activity.lock().await.entry(bot_id.to_string()).or_insert_with(Instant::now).elapsed()
    }

    /// Closes this screen's apps, and the team's browser if it is on this screen. TERM lets
    /// Chromium shut down normally and flush its profile; other screens are untouched.
    pub async fn close_apps(&self, bot_id: &str) -> Result<u32> {
        let script = r#"n=0
# The team's one browser, when it is the one on this screen.
for pid in $(pgrep -u "$(id -u)" -f -- "--user-data-dir=$TEAMBOT_BROWSER_HOME"); do
    tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | grep -q '^--type=' && continue
    tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -Fxq "DISPLAY=$DISPLAY" || continue
    kill -TERM "$pid" 2>/dev/null && n=$((n+1))
done
for record in "$TEAMBOT_SCREEN_RUNTIME"/apps/*; do
    [ -f "$record" ] || continue
    pid=$(cat "$record")
    case "$pid" in ''|*[!0-9]*) rm -f "$record"; continue;; esac
    if tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -Fxq "TEAMBOT_SCREEN_RUNTIME=$TEAMBOT_SCREEN_RUNTIME"; then
        kill -TERM -- -"$pid" 2>/dev/null && n=$((n+1))
    fi
    rm -f "$record"
done
echo "$n""#;
        let (out, ok) = self.run(bot_id, script, 30).await?;
        if !ok { bail!("关闭桌面窗口失败：{}", out.trim()); }
        if self.browser_holder().await.as_deref() == Some(bot_id) { self.set_browser_holder(None).await; }
        Ok(out.trim().parse().unwrap_or(0))
    }

    pub async fn user_has_control(&self, bot_id: &str) -> bool {
        self.control.lock().await.get(bot_id).is_some_and(|until| *until > Instant::now())
    }

    /// Stops every running teambot desktop; their homes stay for next time.
    /// Labels a desktop with the teambot home it belongs to, so stopping one teambot leaves
    /// the desktops of another (a second home on this computer) running.
    fn home_label(&self) -> String {
        format!("teambot.home={}", crate::memory::safe_id(&format!("home:{}", self.dir.display())))
    }

    pub async fn stop_all(&self) {
        self.stopping.store(true, Ordering::Relaxed);
        let _guard = self.starting.lock().await;
        self.ready.lock().await.clear();
        let name = self.container_name();
        let owned = docker(&["inspect", "-f", "{{index .Config.Labels \"teambot.home\"}}", &name]).await;
        if owned.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == self.home_label().trim_start_matches("teambot.home=")) {
            let _ = docker(&["stop", "-t", "10", &name]).await;
        }
    }

    pub fn prepare_restart(&self) { self.stopping.store(true, Ordering::Relaxed); }

    /// Delete only the Bot's screen. Shared files, applications and other screens survive.
    pub async fn remove(&self, bot_id: &str) {
        let _guard = self.starting.lock().await;
        self.ready.lock().await.remove(bot_id);
        self.control.lock().await.remove(bot_id);
        self.scale.lock().await.remove(bot_id);
        if let Ok(n) = self.screen(bot_id).await {
            let script = format!("flock -n /run/teambot/s{n}/lock true && exit 0; p=$(cat /run/teambot/s{n}/pid 2>/dev/null); case \"$p\" in ''|*[!0-9]*) exit 0;; esac; kill -TERM -- -\"$p\" 2>/dev/null || true");
            let _ = docker(&["exec", "-u", "abc", &self.container_name(), "bash", "-c", &script]).await;
        }
        if self.browser_holder().await.as_deref() == Some(bot_id) { self.set_browser_holder(None).await; }
    }

}

pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_shortcuts_validate_urls() {
        assert_eq!(browser_url("").unwrap(), "");
        assert_eq!(browser_url("example.com").unwrap(), "https://example.com/");
        for bad in ["file:///etc/passwd", "javascript://example.com/", "https://name:pass@example.com/", "https://", "data:text/html,test"] {
            assert!(browser_url(bad).is_err(), "{bad}");
        }
        assert!(browser_url("https://example.com/?q=$(touch%20/tmp/injected)'&x=1").unwrap().starts_with("https://example.com/"));
        assert!(app_command("browser", "example.com").unwrap().contains("'https://example.com/'"));
        assert!(app_command("files", "https://example.com").is_err());
        assert!(app_command("arbitrary-command", "").is_err());
    }

    #[test]
    fn the_browser_profile_is_shared_by_every_screen() {
        for n in [10, 11] { assert!(Desktops::seat_env(n).contains(&"TEAMBOT_BROWSER_HOME=/config/.teambot/browser".to_string())); }
    }

    #[test]
    fn quoting_survives_quotes_and_names_are_docker_safe() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        let d = Desktops::new("unused".into(), PathBuf::from("/tmp/team"));
        assert!(d.container_name().chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[tokio::test]
    async fn screen_assignments_survive_restart_and_reject_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let d = Desktops::new("unused".into(), dir.path().into());
        let a = d.screen("bot-a").await.unwrap();
        let b = d.screen("bot-b").await.unwrap();
        assert_ne!(a, b);
        let restarted = Desktops::new("unused".into(), dir.path().into());
        assert_eq!(restarted.screen("bot-b").await.unwrap(), b);
        assert_eq!(restarted.screen("bot-a").await.unwrap(), a);
        assert_ne!(restarted.screen("bot-c").await.unwrap(), a);
        std::fs::write(dir.path().join("screens.json"), r#"{"bot-a":10,"bot-b":10}"#).unwrap();
        let corrupt = Desktops::new("unused".into(), dir.path().into());
        assert!(corrupt.screen("bot-a").await.is_err());
    }

    #[test]
    fn desktop_paths_map_to_the_host_home() {
        let dir = tempfile::tempdir().unwrap();
        let d = Desktops::new(DEFAULT_IMAGE.into(), dir.path().to_path_buf());
        std::fs::create_dir_all(d.home_of("bot-1")).unwrap();
        let home = d.home_of("bot-1").canonicalize().unwrap();
        assert_eq!(home, d.home_of("bot-2").canonicalize().unwrap());
        std::fs::create_dir_all(home.join("Desktop")).unwrap();
        std::fs::write(home.join("Desktop/a.mp4"), "x").unwrap();
        assert_eq!(d.host_path("bot-1", "/config/Desktop/a.mp4"), Some(home.join("Desktop/a.mp4")));
        assert_eq!(d.host_path("bot-1", "/etc/passwd"), None);
        // A link the desktop makes to a file outside its home is not followed.
        std::fs::write(dir.path().join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), home.join("link")).unwrap();
        assert_eq!(d.host_path("bot-1", "/config/link"), None);
        std::os::unix::fs::symlink(dir.path(), home.join("out")).unwrap();
        assert_eq!(d.host_dir("bot-1", "/config/out/x"), None);
        assert!(!dir.path().join("x").exists());
        assert_eq!(d.host_dir("bot-1", "/config/../../outside"), None);
        assert_eq!(d.host_path("bot-1", "/config-other/Desktop/a.mp4"), None);
        assert!(d.host_dir("bot-1", "/config/generated").is_some());
    }
}
