//! Each Bot's own browser: one long-lived Playwright MCP server per Bot, on a local port,
//! with the Bot's profile folder so cookies last. the native engine reaches it over HTTP from
//! the Bot's turns, and the web page drives the same browser through dotbot, which
//! speaks MCP to it too. So the user sees and steers what the Bot sees.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct BrowserConfig {
    /// The command that starts Playwright MCP; dotbot adds the port, host and profile.
    pub command: Vec<String>,
    pub headless: bool,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig { command: vec!["npx".into(), "-y".into(), "@playwright/mcp@0.0.83".into()], headless: true }
    }
}

struct Running {
    child: Option<Child>,
    pid: u32,
    port: u16,
    session: Option<String>,
}

pub struct Browsers {
    config: BrowserConfig,
    profiles: PathBuf,
    running: Mutex<HashMap<String, Running>>,
    /// The Bots whose running browser has the shared logins already.
    shared: Mutex<std::collections::HashSet<String>>,
    cookie_write: Mutex<()>,
    client: reqwest::Client,
    next_id: AtomicU64,
    requests: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    starting: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    control: Mutex<HashMap<String, std::time::Instant>>,
}

/// Stops a browser server's process group: TERM first, so Playwright closes Chrome and the
/// profile is left unlocked, then KILL whatever is left after a few seconds.
async fn stop_group(pgid: u32) {
    let group = format!("-{pgid}");
    let alive = || async { Command::new("kill").args(["-0", "--", &group]).stderr(Stdio::null()).status().await.is_ok_and(|s| s.success()) };
    let _ = Command::new("kill").args(["-TERM", "--", &group]).stderr(Stdio::null()).status().await;
    for _ in 0..25 {
        if !alive().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let _ = Command::new("kill").args(["-KILL", "--", &group]).stderr(Stdio::null()).status().await;
}

// Linux process evidence is used only for this exact persistent profile. Never
// adopt an interactive browser or a server still owned by another live dotbot.
struct ProcessInfo { pid: u32, parent: u32, group: u32, args: Vec<String> }
impl ProcessInfo {
    fn profile(&self, profile: &std::path::Path) -> bool {
        let profile = profile.to_string_lossy();
        self.args.windows(2).any(|a| a[0] == "--user-data-dir" && a[1] == profile)
            || self.args.iter().any(|a| a == &format!("--user-data-dir={profile}"))
    }
    fn mcp_port(&self) -> Option<u16> {
        if !self.args.iter().any(|a| a.contains("playwright") && (a.contains("mcp") || a.contains("cli.js")))
            || !self.args.iter().any(|a| a == "--shared-browser-context") { return None; }
        self.args.windows(2).find(|a| a[0] == "--port").and_then(|a| a[1].parse().ok())
    }
}
fn process_info(pid: u32) -> Option<ProcessInfo> {
    let stat = crate::sqlite_storage::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    if fields[0] == "Z" { return None; }
    let args = crate::sqlite_storage::read(format!("/proc/{pid}/cmdline")).ok()?.split(|b| *b == 0).filter(|b| !b.is_empty()).map(|b| String::from_utf8_lossy(b).into_owned()).collect();
    Some(ProcessInfo { pid, parent: fields[1].parse().ok()?, group: fields[2].parse().ok()?, args })
}
fn orphan_owner(profile: &std::path::Path) -> Option<(u32, u16)> {
    let lock = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let mut pid: u32 = lock.to_str()?.rsplit_once('-')?.1.parse().ok()?;
    let browser = process_info(pid)?;
    if !browser.profile(profile) || !browser.args.iter().any(|a| a.starts_with("--headless")) { return None; }
    let mut server = None;
    for _ in 0..16 {
        let process = process_info(pid)?;
        if process.profile(profile) && let Some(port) = process.mcp_port() { server = Some((process.pid, port)); }
        if process.parent == 1 { return server.filter(|(root, _)| *root == process.pid && process.group == process.pid); }
        pid = process.parent;
    }
    None
}
fn descendant_groups(root: u32) -> Vec<u32> {
    let processes: Vec<_> = std::fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str()?.parse().ok().and_then(process_info)).collect();
    let mut owned = std::collections::HashSet::from([root]);
    loop {
        let before = owned.len();
        for p in &processes { if owned.contains(&p.parent) { owned.insert(p.pid); } }
        if owned.len() == before { break; }
    }
    processes.iter().filter(|p| p.pid != root && owned.contains(&p.pid) && p.group == p.pid).map(|p| p.group).collect()
}

fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// The JSON-RPC message in a response that may be plain JSON or Server-Sent Events.
fn message_of(body: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return Ok(value);
    }
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:")
            && let Ok(value) = serde_json::from_str::<Value>(data.trim())
                && value.get("id").is_some() {
                    return Ok(value);
                }
    }
    bail!("no JSON-RPC answer in: {}", body.chars().take(200).collect::<String>())
}

impl Browsers {
    pub fn new(config: BrowserConfig, profiles: PathBuf) -> Self {
        Browsers { config, profiles, running: Mutex::new(HashMap::new()), shared: Mutex::new(Default::default()), cookie_write: Mutex::new(()), client: reqwest::Client::builder().timeout(Duration::from_secs(120)).build().unwrap(), next_id: AtomicU64::new(1), requests: Mutex::new(HashMap::new()), starting: Mutex::new(HashMap::new()), control: Mutex::new(HashMap::new()) }
    }

    pub fn url_of(port: u16) -> String {
        // Playwright MCP accepts only requests addressed to "localhost".
        format!("http://localhost:{port}/mcp")
    }

    /// The Bot's browser server, started on first use; its MCP URL.
    pub async fn ensure(&self, bot_id: &str) -> Result<String> {
        let startup = self.starting.lock().await.entry(bot_id.into()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        let _startup = startup.lock().await;
        let mut running = self.running.lock().await;
        if let Some(entry) = running.get_mut(bot_id) {
            let alive = match &mut entry.child {
                Some(child) => child.try_wait()?.is_none(),
                None => process_info(entry.pid).is_some_and(|p| p.profile(&self.profiles.join(crate::memory::safe_id(bot_id)))),
            };
            if alive { return Ok(Self::url_of(entry.port)); }
            running.remove(bot_id);
        }
        let port = free_port()?;
        let profile = self.profiles.join(crate::memory::safe_id(bot_id));
        std::fs::create_dir_all(&profile)?;
        // Reattach to the orphan MCP that actually owns Chromium's profile lock. Old
        // dotbot releases lost their PID file on restart; never kill that browser or
        // switch to an isolated context (which would discard its saved logins).
        if let Some((pid, port)) = orphan_owner(&profile)
            && tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok()
        {
            running.insert(bot_id.into(), Running { child: None, pid, port, session: None });
            self.shared.lock().await.remove(bot_id);
            return Ok(Self::url_of(port));
        }

        let (program, rest) = self.config.command.split_first().context("no browser command")?;
        let mut command = Command::new(program);
        // One context for the Bot's turns and the page, so the user sees what the Bot sees;
        // vision adds clicks by position, which the page's live view uses.
        command.args(rest).args(["--port", &port.to_string(), "--host", "127.0.0.1", "--shared-browser-context", "--caps", "vision", "--user-data-dir"]).arg(&profile);
        // Screenshots and traces it writes stay in dotbot's home, not where dotbot runs.
        command.arg("--output-dir").arg(self.profiles.join(format!("{}-output", crate::memory::safe_id(bot_id))));
        command.arg("--blocked-origins").arg(blocked_origins().join(";"));
        if self.config.headless {
            command.arg("--headless");
        }
        // Its own process group: npx starts node, which starts Chrome, and killing npx alone
        // leaves them holding the profile, so the next start fails with "Browser is already in use".
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).process_group(0);
        let pid_file = self.profiles.join(format!("{}.pid", crate::memory::safe_id(bot_id)));
        let child = command.spawn().with_context(|| format!("starting {program}"))?;
        if let Some(pid) = child.id() {
            let _ = std::fs::write(&pid_file, pid.to_string());
        }
        let pid = child.id().context("browser process exited before startup")?;
        running.insert(bot_id.to_string(), Running { child: Some(child), pid, port, session: None });
        drop(running);
        self.shared.lock().await.remove(bot_id);
        // npx may need a while the first time; the port answers once the server is up.
        for _ in 0..240 {
            let mut running = self.running.lock().await;
            if let Some(entry) = running.get_mut(bot_id)
                && let Some(child) = &mut entry.child
                && let Some(status) = child.try_wait()?
            {
                let entry = running.remove(bot_id).unwrap();
                drop(running);
                self.stop_entry(bot_id, entry).await;
                bail!("Browser process exited during startup ({status}). Check the browser command and installation.");
            }
            drop(running);
            if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return Ok(Self::url_of(port));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let entry = self.running.lock().await.remove(bot_id);
        if let Some(entry) = entry { self.stop_entry(bot_id, entry).await; }
        bail!("the browser for this Bot did not start within a minute")
    }

    pub async fn stop(&self, bot_id: &str) {
        let entry = self.running.lock().await.remove(bot_id);
        if let Some(entry) = entry { self.stop_entry(bot_id, entry).await; }
    }

    pub async fn stop_all(&self) {
        let entries: Vec<(String, Running)> = self.running.lock().await.drain().collect();
        futures::future::join_all(entries.into_iter().map(|(id, entry)| async move { self.stop_entry(&id, entry).await })).await;
    }

    async fn stop_entry(&self, bot_id: &str, mut entry: Running) {
        // Chromium may create its own process group. Ask MCP to close its context
        // first, then stop only process groups descended from our MCP server.
        let profile = self.profiles.join(crate::memory::safe_id(bot_id));
        if entry.child.is_none() && !process_info(entry.pid).is_some_and(|p| p.profile(&profile) && p.mcp_port().is_some()) { return; }
        let groups = descendant_groups(entry.pid);
        let _ = tokio::time::timeout(Duration::from_secs(4), self.post(&Self::url_of(entry.port), entry.session.as_deref(), &json!({"jsonrpc":"2.0","id":self.next_id.fetch_add(1, Ordering::Relaxed),"method":"tools/call","params":{"name":"browser_close","arguments":{}}}))).await;
        stop_group(entry.pid).await;
        for group in groups { stop_group(group).await; }
        if let Some(child) = &mut entry.child { let _ = child.kill().await; let _ = child.wait().await; }
        let _ = std::fs::remove_file(self.profiles.join(format!("{}.pid", crate::memory::safe_id(bot_id))));
    }

    async fn post(&self, url: &str, session: Option<&str>, body: &Value) -> Result<(Option<String>, String)> {
        let mut request = self.client.post(url).header("accept", "application/json, text/event-stream").json(body);
        if let Some(id) = session {
            request = request.header("mcp-session-id", id);
        }
        let response = request.send().await?;
        let session = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
        let status = response.status();
        let text = response.text().await?;
        if !status.is_success() {
            bail!("browser server answered {status}: {}", text.chars().take(200).collect::<String>());
        }
        Ok((session, text))
    }

    async fn session(&self, bot_id: &str) -> Result<(String, Option<String>)> {
        let url = self.ensure(bot_id).await?;
        if let Some(session) = self.running.lock().await.get(bot_id).and_then(|e| e.session.clone()) {
            return Ok((url, Some(session)));
        }
        let init = json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "dotbot", "version": env!("CARGO_PKG_VERSION") } } });
        let (session, response) = self.post(&url, None, &init).await?;
        let initialized = message_of(&response)?;
        if initialized.get("error").is_some() || !initialized["result"].is_object() { bail!("Browser initialization failed. Check the browser command and installation."); }
        let _ = self.post(&url, session.as_deref(), &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        if let Some(entry) = self.running.lock().await.get_mut(bot_id) {
            entry.session = session.clone();
        }
        Ok((url, session))
    }

    /// Calls one browser tool for the user, as the Bot's browser; the result's content blocks.
    pub async fn call(&self, bot_id: &str, tool: &str, args: Value) -> Result<Value> {
        self.request(bot_id, "tools/call", json!({ "name": tool, "arguments": args })).await
    }

    pub async fn request(&self, bot_id: &str, method: &str, params: Value) -> Result<Value> {
        let lock = self.requests.lock().await.entry(bot_id.into()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        let _request = lock.lock().await;
        let (url, session) = self.session(bot_id).await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let (_, text) = self.post(&url, session.as_deref(), &body).await?;
        let message = message_of(&text)?;
        if let Some(error) = message.get("error") {
            return Err(anyhow!("{}", error["message"].as_str().unwrap_or("browser error")));
        }
        let result = message["result"].clone();
        if result["isError"].as_bool() == Some(true) && Self::code_text(&result).contains("Browser is already in use") {
            bail!("The browser profile is in use. Return control or close the other dotbot instance, then retry. Saved logins are preserved.");
        }
        Ok(result)
    }

    pub async fn set_control(&self, bot: &str, user: bool) {
        let mut control = self.control.lock().await;
        if user { control.insert(bot.into(), std::time::Instant::now() + Duration::from_secs(60)); } else { control.remove(bot); }
    }

    pub async fn user_has_control(&self, bot: &str) -> bool {
        self.control.lock().await.get(bot).is_some_and(|until| *until > std::time::Instant::now())
    }
}

pub fn observation(tool: &str) -> bool {
    matches!(tool, "browser_snapshot" | "browser_take_screenshot" | "browser_console_messages" | "browser_network_requests" | "browser_find")
}

pub fn readonly_tool(tool: &str) -> bool { observation(tool) || matches!(tool, "browser_navigate" | "browser_navigate_back" | "browser_wait_for") }

/// Whether a page is a human check (CAPTCHA, Cloudflare and the like), from its title,
/// address or text.
pub fn looks_like_challenge(text: &str) -> bool {
    const MARKS: &[&str] = &[
        "just a moment", "verify you are human", "verifying you are human", "are you a robot", "i'm not a robot", "i am not a robot",
        "captcha", "challenges.cloudflare.com", "cf-turnstile", "attention required", "security check", "unusual traffic",
        "人机验证", "安全验证", "请完成验证", "拖动滑块", "滑块验证", "访问验证", "验证码",
    ];
    let lower = text.to_lowercase();
    MARKS.iter().any(|m| lower.contains(m))
}

/// The addresses a Bot's browser may not open: this computer (its services, dotbot
/// included) and the private networks around it. Each host in both forms Playwright takes,
/// with any port and without one.
pub fn blocked_origins() -> Vec<String> {
    let mut hosts: Vec<String> = ["localhost", "*.localhost", "127.0.0.1", "0.0.0.0", "10.*", "192.168.*", "169.254.*"].iter().map(|h| h.to_string()).collect();
    hosts.extend((16..32).map(|n| format!("172.{n}.*")));
    if let Ok(name) = crate::sqlite_storage::read_to_string("/etc/hostname") {
        hosts.extend(Some(name.trim().to_string()).filter(|n| !n.is_empty()));
    }
    // This computer's own address on its network (no packet is sent).
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0")
        && socket.connect("8.8.8.8:53").is_ok()
        && let Ok(addr) = socket.local_addr()
    {
        hosts.push(addr.ip().to_string());
    }
    hosts.dedup();
    hosts.iter().flat_map(|h| [format!("http://{h}:*"), format!("https://{h}:*"), h.clone()]).collect()
}

pub fn is_web(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// "- 0: (current) [Title](url)" in a tab list.
fn current_tab(text: &str) -> (Option<String>, Option<String>) {
    let Some(line) = text.lines().find(|l| l.contains("(current)")) else { return (None, None) };
    let Some(open) = line.find("(current) [") else { return (None, None) };
    let rest = &line[open + 11..];
    let Some(close) = rest.rfind("](") else { return (None, None) };
    let title = rest[..close].to_string();
    let url = rest[close + 2..].trim_end_matches(')').to_string();
    (Some(title), Some(url))
}

fn cookies_path(profiles: &std::path::Path) -> PathBuf {
    profiles.join("shared-cookies.json")
}

/// The JSON a run_code result printed, between its first `[` and last `]`.
fn json_list(text: &str) -> Option<Value> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    let raw = &text[start..=end];
    serde_json::from_str::<Value>(raw).ok().or_else(|| serde_json::from_str::<String>(&format!("\"{}\"", raw)).ok().and_then(|s| serde_json::from_str(&s).ok()))
}

impl Browsers {
    fn code_text(result: &Value) -> String {
        result["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n")
    }

    fn last_url_file(&self, bot_id: &str) -> PathBuf {
        self.profiles.join(format!("{}.url", crate::memory::safe_id(bot_id)))
    }

    /// The last web address this Bot's browser showed, kept across restarts: a fresh browser
    /// opens on a blank tab, which the profile does not bring back.
    pub fn last_url(&self, bot_id: &str) -> Option<String> {
        crate::sqlite_storage::read_to_string(self.last_url_file(bot_id)).ok().map(|u| u.trim().to_string()).filter(|u| is_web(u))
    }

    pub fn remember_url(&self, bot_id: &str, url: &str) {
        if is_web(url) && self.last_url(bot_id).as_deref() != Some(url) {
            let _ = std::fs::write(self.last_url_file(bot_id), url);
        }
    }

    /// The title and address of the tab the browser shows.
    pub async fn current_page(&self, bot_id: &str) -> (Option<String>, Option<String>) {
        let Ok(result) = self.call(bot_id, "browser_tabs", json!({ "action": "list" })).await else { return (None, None) };
        current_tab(&Self::code_text(&result))
    }

    /// Saves the cookies of this Bot's browser as the team's shared logins.
    pub async fn share_from(&self, bot_id: &str) -> Result<usize> {
        let result = self.call(bot_id, "browser_run_code_unsafe", json!({ "code": "async (page) => JSON.stringify((await page.context().storageState()).cookies)" })).await?;
        if result["isError"].as_bool() == Some(true) { bail!("The browser could not read login state. Open a page and sign in before sharing."); }
        let text = Self::code_text(&result);
        let cookies = json_list(&text).filter(|v| v.is_array()).ok_or_else(|| anyhow!("The browser could not read login state. Open a page and sign in before sharing."))?;
        let count = cookies.as_array().map_or(0, |c| c.len());
        if count == 0 { bail!("There are no saved logins to share. Sign in to a site in this browser first."); }
        std::fs::create_dir_all(&self.profiles)?;
        let path = cookies_path(&self.profiles);
        let _writing = self.cookie_write.lock().await;
        crate::durable::save(&path, &cookies)?;
        self.shared.lock().await.insert(bot_id.to_string());
        Ok(count)
    }

    /// Gives a running browser the shared logins, once per start.
    pub async fn apply_shared(&self, bot_id: &str) {
        let Ok(cookies) = crate::sqlite_storage::read_to_string(cookies_path(&self.profiles)) else { return };
        if !self.running.lock().await.contains_key(bot_id) || self.shared.lock().await.contains(bot_id) {
            return;
        }
        let code = format!("async (page) => {{ await page.context().addCookies({cookies}); return 'ok' }}");
        if self.call(bot_id, "browser_run_code_unsafe", json!({ "code": code })).await.is_ok_and(|r| r["isError"].as_bool() != Some(true)) {
            self.shared.lock().await.insert(bot_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn human_checks_are_recognised() {
        assert!(looks_like_challenge("- Page Title: Just a moment..."));
        assert!(looks_like_challenge("请完成安全验证"));
        assert!(!looks_like_challenge("Example Domain"));
    }

    use super::*;

    #[test]
    fn profile_recovery_requires_exact_profile_and_shared_mcp_process() {
        let mut p = ProcessInfo { pid: 1, parent: 1, group: 1, args: vec!["node".into(), "/tools/@playwright/mcp/cli.js".into(), "--user-data-dir".into(), "/tmp/team/bot-1".into(), "--port".into(), "8123".into(), "--shared-browser-context".into()] };
        assert!(p.profile(std::path::Path::new("/tmp/team/bot-1")));
        assert!(!p.profile(std::path::Path::new("/tmp/team/bot")));
        assert_eq!(p.mcp_port(), Some(8123));
        p.args.retain(|a| a != "--shared-browser-context");
        assert_eq!(p.mcp_port(), None);
        p.args = vec!["chrome".into(), "--user-data-dir=/tmp/team/bot-1".into(), "--port".into(), "8123".into(), "--shared-browser-context".into()];
        assert!(p.profile(std::path::Path::new("/tmp/team/bot-1")));
        assert_eq!(p.mcp_port(), None);
    }

    #[test]
    fn answers_come_as_json_or_as_events() {
        assert_eq!(message_of(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#).unwrap()["id"], 1);
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"content\":[]}}\n\n";
        assert_eq!(message_of(sse).unwrap()["id"], 7);
        assert!(message_of("nothing").is_err());
        assert_eq!(current_tab("### Result\n- 0: (current) [dotbot](http://127.0.0.1:1/#a)\n- 1: [x](y)"), (Some("dotbot".into()), Some("http://127.0.0.1:1/#a".into())));
        assert_eq!(json_list("### Result\n\"[{\\\"name\\\":\\\"a\\\"}]\"").unwrap()[0]["name"], "a");
        assert_eq!(json_list("[{\"name\":\"b\"}]").unwrap()[0]["name"], "b");
    }
}
