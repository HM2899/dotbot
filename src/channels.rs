//! Channels: chats on messaging platforms (Telegram, Feishu/Lark, DingTalk, QQ, WeChat)
//! that reach a Bot's own chat or a group, in `<home>/channels.json` (owner only).
//!
//! dotbot listens on 127.0.0.1 only, so every platform is reached the way that needs no
//! public address: Telegram by long polling, Feishu over its WebSocket long connection,
//! DingTalk in Stream mode, QQ over the bot gateway, WeChat over the iLink bot API after
//! a QR sign-in. A message from someone not on a channel's allow list is not delivered;
//! the sender shows on the page with an Allow button.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

use crate::chats::item;
use crate::team::{mention_hint, Conv, Inbound, Origin, Reply, Sink, Team};
use crate::channel_media::{self, IncomingFile, Outgoing};

pub const KINDS: &[&str] = &["telegram", "feishu", "dingtalk", "qq", "wechat"];
/// Settings the page never gets back; it sees MASK in their place.
const SECRET_FIELDS: &[&str] = &["token", "appSecret", "clientSecret"];
/// Settings dotbot keeps for itself.
const HIDDEN_FIELDS: &[&str] = &["cursor", "baseUrl", "cdnBase"];
const MASK: &str = "••••••";
const PENDING_MAX: usize = 20;
const WECHAT_BASE: &str = "https://ilinkai.weixin.qq.com";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Channel {
    #[serde(default)]
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub name: String,
    /// `bot:<id>` or `room:<id>`.
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Sender ids that may write; "*" lets anyone.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// How a platform is called in what the Bots and the user read.
pub fn label(kind: &str) -> &'static str {
    match kind {
        "telegram" => "Telegram",
        "feishu" => "飞书",
        "dingtalk" => "钉钉",
        "qq" => "QQ",
        "wechat" => "微信",
        _ => "?",
    }
}

/// The settings a kind cannot run without.
fn required(kind: &str) -> &'static [&'static str] {
    match kind {
        "telegram" => &["token"],
        "feishu" => &["appId", "appSecret"],
        "dingtalk" => &["clientId", "clientSecret"],
        "qq" => &["appId", "appSecret"],
        _ => &[],
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "off", "connecting", "online", "login" (WeChat waits for a QR scan) or "error".
    pub state: String,
    pub detail: String,
    /// The WeChat sign-in QR code, as SVG.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qr: Option<String>,
    /// Senders not on the allow list who wrote: {id, name}.
    pub pending: Vec<Value>,
    pub last_received: Option<String>,
    pub last_sent: Option<String>,
    pub last_error: Option<String>,
}

pub struct Channels {
    path: PathBuf,
    pub list: std::sync::Mutex<Vec<Channel>>,
    status: std::sync::Mutex<HashMap<String, Status>>,
    tasks: std::sync::Mutex<HashMap<String, tokio::task::AbortHandle>>,
    logins: std::sync::Mutex<HashMap<String, tokio::task::AbortHandle>>,
    seen: std::sync::Mutex<HashMap<String, VecDeque<String>>>,
    /// Last received / last sent per channel, kept across restarts in `channel-activity.json`.
    activity_path: PathBuf,
}

impl Channels {
    pub fn open(home: &Path) -> Self {
        let path = home.join("channels.json");
        let list = crate::sqlite_storage::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let activity_path = home.join("channel-activity.json");
        let activity: HashMap<String, (Option<String>, Option<String>)> = crate::sqlite_storage::read_to_string(&activity_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let status = activity.into_iter().map(|(id, (last_received, last_sent))| (id, Status { state: "off".into(), last_received, last_sent, ..Default::default() })).collect();
        Channels { path, list: std::sync::Mutex::new(list), status: std::sync::Mutex::new(status), tasks: Default::default(), logins: Default::default(), seen: Default::default(), activity_path }
    }

    fn save(&self, list: &[Channel]) -> Result<()> {
        if let Some(result) = crate::sqlite_storage::save(&self.path, &serde_json::to_vec(list)?) { return result; }
        let temp = self.path.with_extension("json.tmp");
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
        let mut file = options.open(&temp)?;
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; file.set_permissions(std::fs::Permissions::from_mode(0o600))?; }
        file.write_all(serde_json::to_string_pretty(list)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &self.path)?;
        Ok(())
    }

    pub fn stop_all(&self) {
        for (_, task) in self.tasks.lock().unwrap().drain() { task.abort(); }
        for (_, task) in self.logins.lock().unwrap().drain() { task.abort(); }
    }

    pub fn get(&self, id: &str) -> Option<Channel> {
        self.list.lock().unwrap().iter().find(|c| c.id == id).cloned()
    }

    /// Saves a channel from the page: a new one gets an id, and a secret left as MASK
    /// keeps the value saved before.
    pub fn upsert(&self, mut channel: Channel) -> Result<Channel> {
        if !KINDS.contains(&channel.kind.as_str()) {
            bail!("A channel is one of: {}.", KINDS.join(", "));
        }
        channel.name = channel.name.trim().to_string();
        if channel.name.is_empty() {
            channel.name = label(&channel.kind).to_string();
        }
        channel.allow = channel.allow.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
        let mut unique = std::collections::HashSet::new();
        channel.allow.retain(|a| unique.insert(a.clone()));
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let before = next.iter().find(|c| c.id == channel.id && !c.id.is_empty()).cloned();
        if let Some(before) = &before {
            channel.kind = before.kind.clone();
            for field in SECRET_FIELDS.iter().chain(HIDDEN_FIELDS) {
                let incoming = channel.settings.get(*field).map(|v| v.as_str());
                if (incoming.is_none() || incoming == Some(MASK) || incoming == Some("")) && let Some(old) = before.settings.get(*field) {
                    channel.settings.insert(field.to_string(), old.clone());
                }
            }
        } else {
            channel.id = format!("ch-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        }
        channel.settings.retain(|_, v| {
            *v = v.trim().to_string();
            !v.is_empty() && v != MASK
        });
        for field in required(&channel.kind) {
            if !channel.settings.contains_key(*field) {
                bail!("{} needs {field}.", label(&channel.kind));
            }
        }
        match next.iter_mut().find(|c| c.id == channel.id) {
            Some(slot) => *slot = channel.clone(),
            None => next.push(channel.clone()),
        }
        self.save(&next)?;
        *list = next;
        Ok(channel)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let next: Vec<_> = list.iter().filter(|c| c.id != id).cloned().collect();
        self.save(&next)?;
        *list = next;
        self.status.lock().unwrap().remove(id);
        self.seen.lock().unwrap().remove(id);
        Ok(())
    }

    /// Changes one channel in place and saves it.
    pub fn update(&self, id: &str, f: impl FnOnce(&mut Channel)) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let channel = next.iter_mut().find(|c| c.id == id).ok_or_else(|| anyhow!("Unknown channel"))?;
        f(channel);
        self.save(&next)?;
        *list = next;
        Ok(())
    }

    fn set_status(&self, id: &str, f: impl FnOnce(&mut Status)) {
        f(self.status.lock().unwrap().entry(id.to_string()).or_default());
    }
    pub(crate) fn delivery_error(&self,id:&str,error:String){self.set_status(id,|s|s.last_error=Some(error));}
    pub(crate) fn delivery_sent(&self,id:&str){self.set_status(id,|s|{s.last_sent=Some(chrono::Utc::now().to_rfc3339());s.last_error=None;});self.save_activity();}
    pub(crate) fn received(&self,id:&str){self.set_status(id,|s|s.last_received=Some(chrono::Utc::now().to_rfc3339()));self.save_activity();}
    /// Best effort: losing a timestamp must never block a message.
    fn save_activity(&self) {
        let ids: std::collections::HashSet<String> = self.list.lock().unwrap().iter().map(|c| c.id.clone()).collect();
        let activity: BTreeMap<String, (Option<String>, Option<String>)> = self.status.lock().unwrap().iter()
            .filter(|(id, s)| ids.contains(*id) && (s.last_received.is_some() || s.last_sent.is_some()))
            .map(|(id, s)| (id.clone(), (s.last_received.clone(), s.last_sent.clone()))).collect();
        if let Err(error) = crate::durable::save(&self.activity_path, &activity) { eprintln!("dotbot: saving channel activity: {error}"); }
    }

    /// What the page shows: secrets masked, with how each channel is doing.
    pub fn view(&self) -> Vec<Value> {
        let list = self.list.lock().unwrap().clone();
        let status = self.status.lock().unwrap();
        list.iter()
            .map(|c| {
                let settings: BTreeMap<&String, &str> = c
                    .settings
                    .iter()
                    .filter(|(k, _)| !HIDDEN_FIELDS.contains(&k.as_str()))
                    .map(|(k, v)| (k, if SECRET_FIELDS.contains(&k.as_str()) && !v.starts_with("${") { MASK } else { v.as_str() }))
                    .collect();
                let s = status.get(&c.id).cloned().unwrap_or_else(|| Status { state: "off".into(), ..Default::default() });
                let signed_in = c.kind == "wechat" && c.settings.get("token").is_some_and(|t| !t.is_empty());
                json!({ "id": c.id, "kind": c.kind, "name": c.name, "target": c.target, "settings": settings, "allow": c.allow, "enabled": c.enabled, "signedIn": signed_in, "status": s })
            })
            .collect()
    }
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(60)).redirect(reqwest::redirect::Policy::none()).build()?)
}

pub(crate) fn redact_channel(channel: &Channel, text: &str, team: &Team) -> String {
    let mut text = team.secrets.redact(text);
    let mut values: Vec<_> = SECRET_FIELDS.iter().filter_map(|key| channel.settings.get(*key)).map(|value| team.secrets.fill(value)).filter(|value| !value.is_empty()).collect();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    for value in values {
        text = text.replace(&value, "[channel key]");
        text = text.replace(urlencoding::encode(&value).as_ref(), "[channel key]");
    }
    text
}

pub(crate) fn channel_error(channel: &Channel, error: &anyhow::Error, team: &Team) -> String {
    // reqwest errors can contain short-lived access tokens and signed CDN URLs,
    // which are not among the saved secrets and must never be echoed to a chat.
    let detail = if let Some(http) = error.downcast_ref::<reqwest::Error>() {
        if http.is_timeout() { "平台请求超时，请稍后重试。".into() }
        else if let Some(status) = http.status() { format!("平台请求失败（HTTP {status}）。") }
        else { "平台网络请求失败，请检查连接后重试。".into() }
    } else { format!("{error:#}") };
    redact_channel(channel, &detail, team)
}

/// Group messages must address this bot, or reply to it; other bots' commands stay theirs.
fn telegram_text(message: &Value, username: &str, bot_id: i64) -> Option<String> {
    if message["from"]["is_bot"].as_bool() == Some(true) || message["from"]["id"].as_i64().is_none() { return None; }
    let text = message["text"].as_str().or(message["caption"].as_str()).unwrap_or_default();
    let first = text.split_whitespace().next().unwrap_or_default();
    if first.starts_with('/') && let Some((_, addressed)) = first.split_once('@') && !addressed.eq_ignore_ascii_case(username) { return None; }
    let mention = regex::Regex::new(&format!(r"(?i)@{}\b", regex::escape(username))).ok()?;
    let group = message["chat"]["type"].as_str() != Some("private");
    let reply = bot_id != 0 && message["reply_to_message"]["from"]["id"].as_i64() == Some(bot_id);
    if group && !mention.is_match(text) && !reply { return None; }
    Some(mention.replace_all(text, "").trim().to_string())
}

/// One message from a platform, as every platform's driver hands it over.
pub struct Incoming {
    pub sender_id: String,
    pub sender: String,
    pub text: String,
    pub group: bool,
    pub files: Vec<IncomingFile>,
}

/// The text cut into pieces of at most `max` characters, at line ends where it can.
pub fn pieces(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > max && !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
        if line.chars().count() > max {
            let chars: Vec<char> = line.chars().collect();
            for chunk in chars.chunks(max) {
                out.push(chunk.iter().collect());
            }
        } else {
            current.push_str(line);
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out.into_iter().map(|p| p.trim_end().to_string()).filter(|p| !p.is_empty()).collect()
}

fn ordered(team: &Arc<Team>, channel_id: &str, recipient: &str, max: usize, data: Value) -> Sink {
    let channel = team.channels.get(channel_id).expect("driver channel exists");
    team.platform_sink(crate::outbox::Route { diagnostic_progress:false, trace:None, channel: channel_id.into(), recipient: recipient.into(), target: channel.target, kind: channel.kind, max, data, require_allowed: channel.allow.iter().any(|a| a == "*" || a == recipient) })
}

/// An access token kept until shortly before it runs out.
#[derive(Clone, Default)]
struct Token(Arc<tokio::sync::Mutex<Option<(String, Instant)>>>);

impl Token {
    async fn get<F, Fut>(&self, fetch: F) -> Result<String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(String, u64)>>,
    {
        let mut guard = self.0.lock().await;
        if let Some((token, until)) = &*guard
            && Instant::now() < *until
        {
            return Ok(token.clone());
        }
        let (token, secs) = fetch().await?;
        *guard = Some((token.clone(), Instant::now() + Duration::from_secs(secs.saturating_sub((secs / 10).clamp(1, 300)))));
        Ok(token)
    }
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn short(id: &str) -> String {
    let tail: String = id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    format!("…{tail}")
}

impl Team {
    pub(crate) async fn send_delivery(&self, route:&crate::outbox::Route, outgoing:Outgoing, delivery_id:&str, sequence:u32, first:bool)->Result<()> {
        let channel=self.channels.get(&route.channel).ok_or_else(||anyhow!("Unknown channel"))?;
        let client=http_client()?;
        let data=&route.data;
        match channel.kind.as_str(){
            "telegram"=>{
                let base=Some(self.setting(&channel,"apiBase")).filter(|s|!s.is_empty()).unwrap_or_else(||"https://api.telegram.org".into());
                let api=format!("{}/bot{}",base.trim_end_matches('/'),self.setting(&channel,"token"));
                let outgoing=match outgoing{Outgoing::Text(text)=>Outgoing::Text(redact_channel(&channel,&text,self)),other=>other};
                channel_media::telegram_send(client,api,data["chat"].clone(),first.then(||data["replyTo"].as_i64()).flatten(),outgoing).await
            }
            "feishu"=>{
                let domain=if self.setting(&channel,"domain")=="lark"{"https://open.larksuite.com"}else{"https://open.feishu.cn"};
                let access=feishu_access(&client,&Token::default(),domain,&self.setting(&channel,"appId"),&self.setting(&channel,"appSecret")).await?;
                channel_media::feishu_send(&client,domain,&access,data["messageId"].as_str().unwrap_or_default(),outgoing).await
            }
            "dingtalk"=>{
                match outgoing{
                    Outgoing::Text(text)=>{
                        let url=channel_media::checked_url(data["webhook"].as_str().unwrap_or_default(),"https://api.dingtalk.com",&["dingtalk.com"])?;
                        let answer:Value=client.post(url).json(&json!({"msgtype":"markdown","markdown":{"title":text.chars().take(20).collect::<String>(),"text":text}})).send().await?.error_for_status()?.json().await?;
                        if answer["errcode"].as_i64().unwrap_or(0)!=0{bail!("钉钉拒绝发送，请检查会话 webhook 是否过期。");}Ok(())
                    }
                    file@Outgoing::File(_)=>{
                        let access=dingtalk_access(&client,&Token::default(),&self.setting(&channel,"clientId"),&self.setting(&channel,"clientSecret")).await?;
                        channel_media::dingtalk_send(&client,&access,"https://oapi.dingtalk.com","https://api.dingtalk.com",data["robot"].as_str().unwrap_or_default(),data["recipient"].as_str().unwrap_or_default(),data["group"].as_bool().unwrap_or(false),file).await
                    }
                }
            }
            "qq"=>{
                let api=if self.setting(&channel,"sandbox")=="1"{"https://sandbox.api.sgroup.qq.com"}else{"https://api.sgroup.qq.com"};
                let answer:Value=client.post("https://bots.qq.com/app/getAppAccessToken").json(&json!({"appId":self.setting(&channel,"appId"),"clientSecret":self.setting(&channel,"appSecret")})).send().await?.error_for_status()?.json().await?;
                let access=answer["access_token"].as_str().ok_or_else(||anyhow!("QQ authorization failed"))?;
                channel_media::qq_send(&client,api,access,data["path"].as_str().unwrap_or_default(),data["messageId"].as_str().unwrap_or_default(),sequence,data["v2"].as_bool().unwrap_or(false),outgoing).await
            }
            "wechat"=>{
                let base=Some(self.setting(&channel,"baseUrl")).filter(|s|!s.is_empty()).unwrap_or_else(||WECHAT_BASE.into());
                let cdn=Some(self.setting(&channel,"cdnBase")).filter(|s|!s.is_empty()).unwrap_or_else(||"https://novac2c.cdn.weixin.qq.com/c2c".into());
                let token=self.setting(&channel,"token");
                let item=channel_media::wechat_item(&client,&base,&token,&cdn,data["to"].as_str().unwrap_or_default(),outgoing).await?;
                let body=json!({"msg":{"from_user_id":"","to_user_id":data["to"],"client_id":delivery_id,"message_type":2,"message_state":2,"item_list":[item],"context_token":data["context"]},"base_info":{"channel_version":env!("CARGO_PKG_VERSION")}});
                let answer=wechat_post(&client,&base,&token,"sendmessage",body,60).await?;
                if answer["errcode"].as_i64().or(answer["ret"].as_i64()).unwrap_or(0)!=0{bail!("微信拒绝发送，请检查登录及回复时限。");}Ok(())
            }
            _=>bail!("Unknown messaging platform"),
        }
    }

    pub fn start_channels(self: &Arc<Self>) {
        let ids: Vec<String> = self.channels.list.lock().unwrap().iter().map(|c| c.id.clone()).collect();
        for id in ids {
            self.restart_channel(&id);
        }
    }

    fn channel_event(&self, id: &str, event: &str) -> bool {
        if event.is_empty() { return true; }
        let mut seen = self.channels.seen.lock().unwrap();
        let events = seen.entry(id.to_string()).or_default();
        if events.iter().any(|old| old == event) { return false; }
        events.push_back(event.to_string());
        if events.len() > 512 { events.pop_front(); }
        true
    }

    pub async fn channel_reconnect(self: &Arc<Self>, id: &str) -> Result<()> {
        let channel = self.channels.get(id).ok_or_else(|| anyhow!("Unknown channel"))?;
        if !channel.enabled { bail!("Resume this platform before reconnecting."); }
        self.restart_channel(id);
        self.team_changed().await;
        Ok(())
    }

    /// Stops a channel's connection and starts it again with its saved settings.
    pub fn restart_channel(self: &Arc<Self>, id: &str) {
        if let Some(task) = self.channels.logins.lock().unwrap().remove(id) { task.abort(); }
        if let Some(task) = self.channels.tasks.lock().unwrap().remove(id) {
            task.abort();
        }
        let Some(channel) = self.channels.get(id) else { return };
        self.channels.set_status(id, |s| { s.qr = None; s.state = "connecting".into(); s.detail.clear(); });
        if !channel.enabled {
            self.channels.set_status(id, |s| {
                s.state = "off".into();
                s.detail.clear();
                s.qr = None;
            });
            return;
        }
        let team = self.clone();
        let task = tokio::spawn(async move { team.keep_channel(channel).await });
        self.channels.tasks.lock().unwrap().insert(id.to_string(), task.abort_handle());
    }

    pub(crate) async fn channel_status(&self, id: &str, state: &str, detail: impl Into<String>) {
        let Some(channel) = self.channels.get(id) else { return };
        if !channel.enabled && state != "off" { return; }
        let detail = redact_channel(&channel, &detail.into(), self);
        self.channels.set_status(id, |s| {
            s.state = state.into();
            s.detail = detail;
            if state != "login" {
                s.qr = None;
            }
        });
        self.team_changed().await;
    }

    /// Runs a channel's connection, connecting again after it breaks, waiting longer each
    /// time it breaks soon.
    async fn keep_channel(self: Arc<Self>, channel: Channel) {
        let (initial_wait,max_wait) = if channel.kind == "wechat" { (1,15) } else { (5,300) };
        let mut wait = initial_wait;
        loop {
            self.channel_status(&channel.id, "connecting", "").await;
            let started = Instant::now();
            let result = match channel.kind.as_str() {
                "telegram" => self.run_telegram(&channel).await,
                "feishu" => self.run_feishu(&channel).await,
                "dingtalk" => self.run_dingtalk(&channel).await,
                "qq" => self.run_qq(&channel).await,
                "wechat" => self.run_wechat(&channel).await,
                _ => Err(anyhow!("Unknown kind")),
            };
            if started.elapsed() > Duration::from_secs(120) {
                wait = initial_wait;
            }
            let detail = match result {
                Ok(()) => "The connection closed; connecting again.".to_string(),
                Err(error) => format!("{error:#}"),
            };
            self.channel_status(&channel.id, "error", redact_channel(&channel, &format!("{detail} (again in {wait}s)"), &self)).await;
            tokio::time::sleep(Duration::from_secs(wait)).await;
            wait = (wait * 2).min(max_wait);
        }
    }

    fn setting(&self, channel: &Channel, key: &str) -> String {
        channel.settings.get(key).map(|v| self.secrets.fill(v)).unwrap_or_default()
    }

    /// A message from a platform: checked against the allow list, then into the Bot's
    /// chat or the group, with the answer going back through `reply`.
    pub(crate) async fn channel_message(self: &Arc<Self>, channel_id: &str, message: Incoming, reply: Sink) {
        let Some(channel) = self.channels.get(channel_id) else { return };
        let text = message.text.trim().to_string();
        if self.shutting_down.load(std::sync::atomic::Ordering::Relaxed) || !channel.enabled || (text.is_empty() && message.files.is_empty()) || message.sender_id.trim().is_empty() {
            return;
        }
        if !channel.allow.iter().any(|a| a == "*" || *a == message.sender_id) {
            let mut new = false;
            self.channels.set_status(channel_id, |s| {
                if !s.pending.iter().any(|p| p["id"] == message.sender_id) {
                    new = true;
                    s.pending.push(json!({ "id": message.sender_id, "name": message.sender }));
                    if s.pending.len() > PENDING_MAX {
                        s.pending.remove(0);
                    }
                }
            });
            if new {
                self.team_changed().await;
                reply.send(format!("这个 dotbot 还没有允许你使用。请它的主人在 dotbot 的「设置 → 社交平台」允许你（ID：{}）。", message.sender_id).into());
            }
            return;
        }
        let (team, id, sender, sink) = (self.clone(), channel_id.to_string(), message.sender_id.clone(), reply);
        if sink.route.as_ref().is_some_and(|r|self.recovery.has_route(r)){return;}
        let trace=self.channel_diagnostics.received(channel_id);
        self.channels.received(channel_id);
        let sink=if let Some(mut route)=sink.route.clone(){route.trace=Some(trace.clone());self.platform_sink(route)}else{sink};
        let reply = Sink::new(sink.route.clone(), move |text| {
            if team.channels.get(&id).is_some_and(|c| c.enabled && c.allow.iter().any(|allowed| allowed == "*" || *allowed == sender)) { sink.send(text); }
        });
        let files: Result<Vec<Value>> = async {
            if message.files.len() > crate::attachments::FILE_COUNT { bail!("每条消息最多 8 个附件。"); }
            let mut files = Vec::new();
            let mut total = 0u64;
            for file in message.files {
                if file.size.is_some_and(|n| n > crate::attachments::FILE_MAX as u64) { bail!("附件 {} 超过 25 MB。", file.name); }
                if total + file.size.unwrap_or(0) > crate::attachments::TOTAL_MAX { bail!("每条消息附件总大小不能超过 50 MB。"); }
                let bytes = file.get.await?;
                total += bytes.len() as u64;
                if total > crate::attachments::TOTAL_MAX { bail!("每条消息附件总大小不能超过 50 MB。"); }
                files.push(self.stage_attachment(&channel.target, &file.name, &bytes).await?);
            }
            let refs = files.iter().filter_map(|f| f["src"].as_str().map(String::from)).collect::<Vec<_>>();
            self.attachment_records(&channel.target, &refs).await
        }.await;
        let files = match files {
            Ok(files) => files,
            Err(error) => {
                let detail = format!("附件接收失败：{}", channel_error(&channel, &error, self));
                self.channels.set_status(channel_id, |s| s.last_error = Some(detail.clone()));
                self.channel_diagnostics.mark(&trace,"failed",Some(detail.clone()));
                reply.send(Reply::failed(detail, false)); self.team_changed().await; return;
            }
        };
        let text = if text.is_empty() { "(attachments)".into() } else { text };
        if !self.channels.get(channel_id).is_some_and(|c| c.enabled && c.target == channel.target && c.allow.iter().any(|a| a == "*" || *a == message.sender_id)) { return; }
        self.channels.received(channel_id);
        self.channel_diagnostics.mark(&trace,"queued",None);
        self.team_changed().await;
        let via = format!("{} · {}", label(&channel.kind), message.sender);
        let command = text.split_whitespace().next().unwrap_or_default().split('@').next().unwrap_or_default().to_lowercase();
        if let Some(room) = channel.target.strip_prefix("room:") {
            if command == "/stop" {
                let keys: Vec<String> = self.runs.lock().await.keys().filter(|k| k.starts_with(&format!("room:{room}:"))).cloned().collect();
                for key in keys {
                    self.stop(&key).await;
                }
                reply.send("已停止。".into());
                return;
            }
            if let Err(error) = self.post_as_files(room, &message.sender, Some(&via), &text, Some(reply.clone()), None, files).await {
                reply.send(Reply::failed(format!("没能发到群聊：{error}"), false));
            }
            return;
        }
        let Some(Conv::Bot(bot)) = Conv::parse(&channel.target) else {
            reply.send("这个通道没有连到 Bot 或群聊。".into());
            return;
        };
        let state = self.state().await;
        if state.bots.get(&bot).is_none() {
            reply.send("这个通道连着的 Bot 已经不在了。".into());
            return;
        }
        match command.as_str() {
            "/stop" => {
                self.stop(&channel.target).await;
                reply.send("已停止。".into());
                return;
            }
            "/new" | "/reset" => {
                self.reset(&channel.target).await;
                reply.send("新的对话开始了，记忆会保留。".into());
                return;
            }
            _ => {}
        }
        let (context, images) = match self.attachment_context(&channel.target, &files).await {
            Ok(context) => context,
            Err(error) => { reply.send(Reply::failed(redact_channel(&channel, &format!("附件读取失败：{error}"), self), false)); return; }
        };
        self.record(&channel.target, item("user", json!({ "text": text, "via": via, "files": files }))).await;
        let hint = mention_hint(&state, &bot, &text);
        let place = if message.group { "，在群里" } else { "" };
        let mut inbound = Inbound::new(format!("[{via}{place}，通过消息平台发来；你的回复会发回给对方]\n{text}{hint}{context}"), Origin::User);
        inbound.images = images;
        inbound.sinks.push(reply);
        self.deliver(&channel.target, inbound).await;
    }

    /// Lets a sender write to a channel, or takes the permission back.
    pub async fn channel_allow(&self, id: &str, sender: &str, allow: bool) -> Result<()> {
        let sender = sender.trim();
        if sender.is_empty() {
            bail!("Which sender?");
        }
        self.channels.update(id, |c| {
            c.allow.retain(|a| a != sender);
            if allow {
                c.allow.push(sender.to_string());
            }
        })?;
        self.channels.set_status(id, |s| s.pending.retain(|p| p["id"] != sender));
        self.team_changed().await;
        Ok(())
    }

    pub async fn channel_save(self: &Arc<Self>, params: &Value) -> Result<Value> {
        let channel: Channel = serde_json::from_value(params.clone()).context("Not a channel")?;
        let state = self.state().await;
        let fits = match Conv::parse(&channel.target) {
            Some(Conv::Bot(bot)) => state.bots.get(&bot).is_some(),
            _ => channel.target.strip_prefix("room:").is_some_and(|r| state.rooms.get(r).is_some()),
        };
        if !fits {
            bail!("Choose the Bot or group chat the messages go to.");
        }
        let saved = self.channels.upsert(channel)?;
        self.restart_channel(&saved.id);
        self.team_changed().await;
        Ok(json!(saved.id))
    }

    pub async fn channel_delete(&self, id: &str) -> Result<()> {
        self.channels.remove(id)?;
        if let Some(task) = self.channels.logins.lock().unwrap().remove(id) { task.abort(); }
        if let Some(task) = self.channels.tasks.lock().unwrap().remove(id) {
            task.abort();
        }
        self.team_changed().await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Telegram: the Bot API by long polling. In a group, only messages that call the bot
    // by @name or answer it.

    async fn run_telegram(self: &Arc<Self>, channel: &Channel) -> Result<()> {
        let base = Some(self.setting(channel, "apiBase")).filter(|b| !b.is_empty()).unwrap_or_else(|| "https://api.telegram.org".into());
        let api = format!("{}/bot{}", base.trim_end_matches('/'), self.setting(channel, "token"));
        let client = http_client()?;
        let me: Value = client.get(format!("{api}/getMe")).timeout(Duration::from_secs(30)).send().await.context("Telegram does not answer")?.json().await?;
        if me["ok"] != json!(true) {
            bail!("Telegram: {}", me["description"].as_str().unwrap_or("the token is not right"));
        }
        let username = me["result"]["username"].as_str().unwrap_or_default().to_string();
        let bot_id = me["result"]["id"].as_i64().unwrap_or_default();
        self.channel_status(&channel.id, "online", format!("@{username}")).await;
        let mut offset = self.setting(channel, "cursor").parse::<i64>().unwrap_or(0);
        loop {
            let answer: Value = client
                .post(format!("{api}/getUpdates"))
                .json(&json!({ "offset": offset, "timeout": 30, "allowed_updates": ["message"] }))
                .timeout(Duration::from_secs(45))
                .send()
                .await?
                .json()
                .await?;
            if answer["ok"] != json!(true) {
                bail!("Telegram: {}", answer["description"].as_str().unwrap_or("getUpdates failed"));
            }
            let previous = offset;
            for update in answer["result"].as_array().cloned().unwrap_or_default() {
                let Some(update_id) = update["update_id"].as_i64() else { continue };
                if update_id < previous || !self.channel_event(&channel.id, &format!("tg:{update_id}")) { continue; }
                offset = offset.max(update_id + 1);
                let m = &update["message"];
                let Some(text) = telegram_text(m, &username, bot_id) else { continue };
                let group = m["chat"]["type"].as_str() != Some("private");
                let from = &m["from"];
                let sender = from["username"].as_str().map(|u| format!("@{u}")).or_else(|| from["first_name"].as_str().map(String::from)).unwrap_or_else(|| "?".into());
                let chat = m["chat"]["id"].clone();
                let files = channel_media::telegram_files(&client, &api, &base, m);
                let sender_id = from["id"].to_string();
                let sink = ordered(self, &channel.id, &sender_id, 4000, json!({"chat":chat,"replyTo":m["message_id"]}));
                let incoming = Incoming { sender_id, sender, text, group, files };
                self.channel_message(&channel.id, incoming, sink).await;
            }
            if offset != previous {
                self.channels.update(&channel.id, |c| { c.settings.insert("cursor".into(), offset.to_string()); })?;
            }
        }
    }

    // -----------------------------------------------------------------------
    // DingTalk: Stream mode, a WebSocket that carries the robot's messages as JSON; each
    // answer goes to the message's session webhook.

    async fn run_dingtalk(self: &Arc<Self>, channel: &Channel) -> Result<()> {
        let client = http_client()?;
        let token = Token::default();
        let client_id = self.setting(channel, "clientId");
        let client_secret = self.setting(channel, "clientSecret");
        let open: Value = client
            .post("https://api.dingtalk.com/v1.0/gateway/connections/open")
            .json(&json!({
                "clientId": self.setting(channel, "clientId"), "clientSecret": self.setting(channel, "clientSecret"),
                "subscriptions": [{ "type": "CALLBACK", "topic": "/v1.0/im/bot/messages/get" }], "ua": "dotbot",
            }))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("DingTalk does not answer")?
            .json()
            .await?;
        let (Some(endpoint), Some(ticket)) = (open["endpoint"].as_str(), open["ticket"].as_str()) else {
            bail!("DingTalk: {}", open["message"].as_str().unwrap_or("check the Client ID and Client Secret"));
        };
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("{endpoint}?ticket={}", urlencoding::encode(ticket))).await.context("DingTalk stream")?;
        self.channel_status(&channel.id, "online", "Stream").await;
        while let Some(frame) = ws.next().await {
            let text = match frame? {
                Message::Text(text) => text.to_string(),
                Message::Close(_) => return Ok(()),
                _ => continue,
            };
            let Ok(message) = serde_json::from_str::<Value>(&text) else { continue };
            let headers = &message["headers"];
            let topic = headers["topic"].as_str().unwrap_or_default();
            match message["type"].as_str() {
                Some("SYSTEM") if topic == "ping" => {
                    let pong = json!({ "code": 200, "headers": headers, "message": "OK", "data": message["data"] });
                    ws.send(Message::Text(pong.to_string().into())).await?;
                }
                Some("SYSTEM") if topic == "disconnect" => return Ok(()),
                Some("CALLBACK") => {
                    let ack = json!({ "code": 200, "headers": { "contentType": "application/json", "messageId": headers["messageId"] }, "message": "OK", "data": "{\"response\":null}" });
                    ws.send(Message::Text(ack.to_string().into())).await?;
                    let Ok(data) = serde_json::from_str::<Value>(message["data"].as_str().unwrap_or("{}")) else { continue };
                    if !self.channel_event(&channel.id, data["msgId"].as_str().unwrap_or_default()) { continue; }
                    let text = data["text"]["content"].as_str().map(String::from).unwrap_or_else(|| data["content"]["richText"].as_array().into_iter().flatten().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""));
                    let Some(webhook) = data["sessionWebhook"].as_str().map(String::from) else { continue };
                    let sender_id = data["senderStaffId"].as_str().or(data["senderId"].as_str()).unwrap_or_default().to_string();
                    let group = data["conversationType"].as_str() == Some("2");
                    let robot = data["robotCode"].as_str().unwrap_or(&client_id).to_string();
                    let recipient = if group { data["conversationId"].as_str().unwrap_or_default().to_string() } else { sender_id.clone() };
                    let mut files = Vec::new();
                    for code in media_keys(&data["content"], "downloadCode") {
                        let (client, token, client_id, client_secret, robot) = (client.clone(), token.clone(), client_id.clone(), client_secret.clone(), robot.clone());
                        let fallback = if data["msgtype"] == "picture" || data["msgtype"] == "richText" { "image.jpg" } else { "file.bin" };
                        let name = data["content"]["fileName"].as_str().unwrap_or(fallback).to_string();
                        files.push(IncomingFile { name, size: data["content"]["fileSize"].as_u64(), get: Box::pin(async move {
                            let access = dingtalk_access(&client, &token, &client_id, &client_secret).await?;
                            let answer: Value = client.post("https://api.dingtalk.com/v1.0/robot/messageFiles/download").header("x-acs-dingtalk-access-token", &access).json(&json!({ "downloadCode": code, "robotCode": robot })).send().await?.error_for_status()?.json().await?;
                            let raw = answer["downloadUrl"].as_str().ok_or_else(|| anyhow!("DingTalk returned no attachment address: {}", answer["message"]))?;
                            let url = channel_media::checked_url(raw, "https://api.dingtalk.com", &["dingtalk.com", "alicdn.com", "aliyuncs.com"])?;
                            channel_media::follow_download(&client, client.get(url).send().await?, "https://api.dingtalk.com", &["dingtalk.com", "alicdn.com", "aliyuncs.com"]).await
                        }) });
                    }
                    let sink = ordered(self, &channel.id, &sender_id, 4000, json!({"webhook":webhook,"robot":robot,"recipient":recipient,"group":group,"message":data["msgId"]}));
                    let incoming = Incoming { sender: data["senderNick"].as_str().map(String::from).unwrap_or_else(|| short(&sender_id)), sender_id, text: text.trim().to_string(), group, files };
                    self.channel_message(&channel.id, incoming, sink).await;
                }
                _ => {}
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Feishu / Lark: the long connection, a WebSocket of protobuf frames with JSON events
    // inside; answers go through the open API as replies to the message.

    async fn run_feishu(self: &Arc<Self>, channel: &Channel) -> Result<()> {
        let domain = if self.setting(channel, "domain") == "lark" { "https://open.larksuite.com" } else { "https://open.feishu.cn" };
        let (app_id, app_secret) = (self.setting(channel, "appId"), self.setting(channel, "appSecret"));
        let client = http_client()?;
        let endpoint: Value = client
            .post(format!("{domain}/callback/ws/endpoint"))
            .header("locale", "zh")
            .json(&json!({ "AppID": app_id, "AppSecret": app_secret }))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("Feishu does not answer")?
            .json()
            .await?;
        if endpoint["code"].as_i64() != Some(0) {
            bail!("Feishu: {}", endpoint["msg"].as_str().unwrap_or("check the App ID and App Secret"));
        }
        let url = endpoint["data"]["URL"].as_str().ok_or_else(|| anyhow!("Feishu gave no address"))?.to_string();
        let ping_every = endpoint["data"]["ClientConfig"]["PingInterval"].as_u64().unwrap_or(120).clamp(10, 600);
        let service: i32 = url.split(['?', '&']).find_map(|p| p.strip_prefix("service_id=")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await.context("Feishu long connection")?;
        self.channel_status(&channel.id, "online", if domain.contains("lark") { "Lark" } else { "飞书" }).await;
        let token = Token::default();
        let mut parts: HashMap<String, Vec<Option<Vec<u8>>>> = HashMap::new();
        let mut ping = tokio::time::interval(Duration::from_secs(ping_every));
        let mentions = regex::Regex::new(r"@_user_\d+").unwrap();
        loop {
            let frame = tokio::select! {
                frame = ws.next() => frame,
                _ = ping.tick() => {
                    let frame = Frame { service, method: 0, headers: vec![("type".into(), "ping".into())], ..Default::default() };
                    ws.send(Message::Binary(frame.encode().into())).await?;
                    continue;
                }
            };
            let Some(frame) = frame else { return Ok(()) };
            let bytes = match frame? {
                Message::Binary(bytes) => bytes.to_vec(),
                Message::Close(_) => return Ok(()),
                _ => continue,
            };
            let Ok(mut frame) = Frame::decode(&bytes) else { continue };
            if frame.method != 1 || frame.header("type") != Some("event") {
                continue;
            }
            let id = frame.header("message_id").unwrap_or_default().to_string();
            let sum: usize = frame.header("sum").and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 64);
            let seq: usize = frame.header("seq").and_then(|v| v.parse().ok()).unwrap_or(0);
            let payload = if sum > 1 {
                let slots = parts.entry(id.clone()).or_insert_with(|| vec![None; sum]);
                if let Some(slot) = slots.get_mut(seq) {
                    *slot = Some(std::mem::take(&mut frame.payload));
                }
                if slots.iter().any(Option::is_none) {
                    continue;
                }
                parts.remove(&id).unwrap().into_iter().flatten().flatten().collect()
            } else {
                std::mem::take(&mut frame.payload)
            };
            frame.headers.push(("biz_rt".into(), "0".into()));
            frame.payload = br#"{"code":200}"#.to_vec();
            ws.send(Message::Binary(frame.encode().into())).await?;
            let Ok(event) = serde_json::from_slice::<Value>(&payload) else { continue };
            if event["header"]["event_type"].as_str() != Some("im.message.receive_v1") {
                continue;
            }
            // Feishu sends an event again when the answer was late; once is enough.
            let event_id = event["header"]["event_id"].as_str().unwrap_or_default();
            if !self.channel_event(&channel.id, event_id) { continue; }
            let message = &event["event"]["message"];
            let content: Value = serde_json::from_str(message["content"].as_str().unwrap_or("{}")).unwrap_or_default();
            let kind = message["message_type"].as_str().unwrap_or_default();
            let text = match kind {
                "text" => content["text"].as_str().unwrap_or_default().to_string(),
                "post" => post_text(&content),
                _ => String::new(),
            };
            let text = mentions.replace_all(&text, "").trim().to_string();
            let message_id = message["message_id"].as_str().unwrap_or_default().to_string();
            let sender_id = event["event"]["sender"]["sender_id"]["open_id"].as_str().unwrap_or_default().to_string();
            let mut files = Vec::new();
            let mut keys = Vec::new();
            if kind == "image" {
                if let Some(key) = content["image_key"].as_str() { keys.push((key.to_string(), "image.jpg".to_string(), "image")); }
            } else if matches!(kind, "file" | "audio" | "media" | "video") {
                if let Some(key) = content["file_key"].as_str() { keys.push((key.to_string(), content["file_name"].as_str().unwrap_or(match kind { "audio" => "audio.ogg", "media" | "video" => "video.mp4", _ => "file.bin" }).to_string(), "file")); }
            } else if kind == "post" {
                for key in media_keys(&content, "image_key") { keys.push((key, "image.jpg".into(), "image")); }
            }
            for (key, name, resource_type) in keys {
                let (client, token, app_id, app_secret, message_id) = (client.clone(), token.clone(), app_id.clone(), app_secret.clone(), message_id.clone());
                files.push(IncomingFile { name, size: None, get: Box::pin(async move {
                    let access = feishu_access(&client, &token, domain, &app_id, &app_secret).await?;
                    let response = client.get(format!("{domain}/open-apis/im/v1/messages/{}/resources/{}", urlencoding::encode(&message_id), urlencoding::encode(&key))).query(&[("type", resource_type)]).bearer_auth(access).send().await?;
                    channel_media::follow_download(&client, response, domain, &["feishu.cn", "feishucdn.com", "larksuite.com", "larksuitecdn.com", "byteimg.com"]).await
                }) });
            }
            if files.is_empty() && text.is_empty() && kind != "text" {
                files.push(IncomingFile { name: "unsupported-message".into(), size: None, get: Box::pin(async { bail!("飞书暂不支持此消息类型，请改为图片或文件附件发送。"); }) });
            }
            let sink = ordered(self, &channel.id, &sender_id, 4000, json!({"messageId":message_id}));
            let incoming = Incoming { sender: short(&sender_id), sender_id, text, group: message["chat_type"].as_str() == Some("group"), files };
            self.channel_message(&channel.id, incoming, sink).await;
        }
    }

    // -----------------------------------------------------------------------
    // QQ: the official bot gateway (a WebSocket like Discord's), for direct messages,
    // group @-messages and channel @-messages; answers are passive replies to the message.

    async fn run_qq(self: &Arc<Self>, channel: &Channel) -> Result<()> {
        let api = if self.setting(channel, "sandbox").is_empty() { "https://api.sgroup.qq.com" } else { "https://sandbox.api.sgroup.qq.com" };
        let (app_id, app_secret) = (self.setting(channel, "appId"), self.setting(channel, "appSecret"));
        let client = http_client()?;
        let token = Token::default();
        let fetch = {
            let (client, app_id, app_secret) = (client.clone(), app_id.clone(), app_secret.clone());
            move || {
                let (client, app_id, app_secret) = (client.clone(), app_id.clone(), app_secret.clone());
                async move {
                    let answer: Value = client.post("https://bots.qq.com/app/getAppAccessToken").json(&json!({ "appId": app_id, "clientSecret": app_secret })).send().await?.json().await?;
                    let token = answer["access_token"].as_str().ok_or_else(|| anyhow!("QQ: {}", answer["message"].as_str().unwrap_or("check the AppID and AppSecret")))?.to_string();
                    Ok((token, number(&answer["expires_in"]).unwrap_or(7200)))
                }
            }
        };
        let access = token.get(fetch.clone()).await.context("QQ sign-in")?;
        let gateway: Value = client.get(format!("{api}/gateway")).header("Authorization", format!("QQBot {access}")).send().await?.json().await?;
        let url = gateway["url"].as_str().ok_or_else(|| anyhow!("QQ: {}", gateway["message"].as_str().unwrap_or("no gateway")))?;
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.context("QQ gateway")?;
        let mut seq = Value::Null;
        let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_secs(40), Duration::from_secs(40));
        let intents: u64 = self.setting(channel, "intents").parse().unwrap_or((1 << 25) | (1 << 30) | (1 << 12));
        let mentions = regex::Regex::new(r"<@!?\d+>").unwrap();
        loop {
            let frame = tokio::select! {
                frame = ws.next() => frame,
                _ = beat.tick() => {
                    ws.send(Message::Text(json!({ "op": 1, "d": seq }).to_string().into())).await?;
                    continue;
                }
            };
            let Some(frame) = frame else { return Ok(()) };
            let text = match frame? {
                Message::Text(text) => text.to_string(),
                Message::Close(close) => bail!("QQ closed the gateway: {}", close.map(|c| format!("{} {}", u16::from(c.code), c.reason)).unwrap_or_default()),
                _ => continue,
            };
            let Ok(payload) = serde_json::from_str::<Value>(&text) else { continue };
            if !payload["s"].is_null() {
                seq = payload["s"].clone();
            }
            match payload["op"].as_u64() {
                Some(10) => {
                    let every = payload["d"]["heartbeat_interval"].as_u64().unwrap_or(40_000).max(5_000);
                    beat = tokio::time::interval_at(tokio::time::Instant::now() + Duration::from_millis(every), Duration::from_millis(every));
                    let access = token.get(fetch.clone()).await?;
                    let identify = json!({ "op": 2, "d": { "token": format!("QQBot {access}"), "intents": intents, "shard": [0, 1], "properties": {} } });
                    ws.send(Message::Text(identify.to_string().into())).await?;
                }
                Some(7) => return Ok(()),
                Some(9) => bail!("QQ did not accept the session (are the message events enabled for this bot?)"),
                Some(0) => {
                    let d = &payload["d"];
                    let kind = payload["t"].as_str().unwrap_or_default();
                    if kind == "READY" {
                        self.channel_status(&channel.id, "online", d["user"]["username"].as_str().unwrap_or("QQ")).await;
                        continue;
                    }
                    let (path, sender_id, sender, group, v2) = match kind {
                        "C2C_MESSAGE_CREATE" => {
                            let id = d["author"]["user_openid"].as_str().unwrap_or_default();
                            (format!("/v2/users/{id}/messages"), id.to_string(), short(id), false, true)
                        }
                        "GROUP_AT_MESSAGE_CREATE" => {
                            let id = d["author"]["member_openid"].as_str().unwrap_or_default();
                            (format!("/v2/groups/{}/messages", d["group_openid"].as_str().unwrap_or_default()), id.to_string(), short(id), true, true)
                        }
                        "AT_MESSAGE_CREATE" => (format!("/channels/{}/messages", d["channel_id"].as_str().unwrap_or_default()), d["author"]["id"].as_str().unwrap_or_default().to_string(), d["author"]["username"].as_str().unwrap_or("?").to_string(), true, false),
                        "DIRECT_MESSAGE_CREATE" => (format!("/dms/{}/messages", d["guild_id"].as_str().unwrap_or_default()), d["author"]["id"].as_str().unwrap_or_default().to_string(), d["author"]["username"].as_str().unwrap_or("?").to_string(), false, false),
                        _ => continue,
                    };
                    let text = mentions.replace_all(d["content"].as_str().unwrap_or_default(), "").trim().to_string();
                    let msg_id = d["id"].as_str().unwrap_or_default().to_string();
                    if !self.channel_event(&channel.id, &msg_id) { continue; }
                    let mut files = Vec::new();
                    for attachment in d["attachments"].as_array().into_iter().flatten() {
                        let Some(raw) = attachment["url"].as_str() else { continue };
                        let url = if raw.starts_with("//") { format!("https:{raw}") } else { raw.to_string() };
                        let name = attachment["filename"].as_str().or(attachment["file_name"].as_str()).map(String::from).unwrap_or_else(|| {
                            let ext = attachment["content_type"].as_str().and_then(mime_guess::get_mime_extensions_str).and_then(|exts| exts.first()).copied().unwrap_or("bin");
                            format!("attachment.{ext}")
                        });
                        files.push(channel_media::url_file(client.clone(), url, api.into(), name, attachment["size"].as_u64(), &["qq.com", "qpic.cn", "gtimg.cn", "gtimg.com", "qlogo.cn"]));
                    }
                    let sink = ordered(self, &channel.id, &sender_id, 2000, json!({"path":path,"messageId":msg_id,"v2":v2}));
                    self.channel_message(&channel.id, Incoming { sender_id, sender, text, group, files }, sink).await;
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // WeChat: the iLink bot API. Sign in by scanning a QR code with WeChat; then messages
    // come by long polling, and answers carry the message's context token.

    async fn run_wechat(self: &Arc<Self>, channel: &Channel) -> Result<()> {
        let token = self.setting(channel, "token");
        if token.is_empty() {
            self.channel_status(&channel.id, "login", "Sign in with WeChat: press Sign in and scan the code.").await;
            return std::future::pending().await;
        }
        let base = Some(self.setting(channel, "baseUrl")).filter(|b| !b.is_empty()).unwrap_or_else(|| WECHAT_BASE.into());
        let base = base.trim_end_matches('/').to_string();
        let client = http_client()?;
        let mut cursor = self.channels.get(&channel.id).map(|current|self.setting(&current,"cursor")).unwrap_or_default();
        let mut poll_ms = 35_000;
        self.channel_status(&channel.id, "online", "微信").await;
        loop {
            // A quiet long poll may hit its read deadline without being disconnected.
            // Retry it immediately with the same cursor, rather than entering the
            // connection retry loop (which used to grow to five minutes).
            let answer = match wechat_post_timeout(&client, &base, &token, "getupdates", json!({ "get_updates_buf": cursor, "base_info": { "channel_version": env!("CARGO_PKG_VERSION") } }), Duration::from_millis(poll_ms + 5_000)).await {
                Ok(answer) => answer,
                Err(error) if error.downcast_ref::<reqwest::Error>().is_some_and(|e|e.is_timeout()&&!e.is_connect()) => continue,
                Err(error) => return Err(error),
            };
            if let Some(timeout) = number(&answer["longpolling_timeout_ms"]).filter(|n|*n>0) { poll_ms = timeout.clamp(1_000,120_000); }
            let code = answer["errcode"].as_i64().or(answer["ret"].as_i64()).unwrap_or(0);
            if code == -14 {
                self.channels.update(&channel.id, |c| {
                    c.settings.remove("token");
                    c.settings.remove("cursor");
                })?;
                self.channel_status(&channel.id, "login", "The WeChat sign-in ran out: sign in again.").await;
                return std::future::pending().await;
            }
            if code != 0 {
                bail!("WeChat: {} {}", code, answer["errmsg"].as_str().unwrap_or_default());
            }
            let next_cursor = answer["get_updates_buf"].as_str().filter(|n| !n.is_empty() && *n != cursor).map(String::from);
            for m in answer["msgs"].as_array().cloned().unwrap_or_default() {
                if m["message_type"].as_i64() != Some(1) {
                    continue;
                }
                let event_id = m["message_id"].as_str().map(String::from).or_else(|| m["message_id"].as_i64().map(|n| n.to_string())).or_else(|| m["client_id"].as_str().map(String::from)).unwrap_or_default();
                if !self.channel_event(&channel.id, &event_id) { continue; }
                let items = m["item_list"].as_array().cloned().unwrap_or_default();
                let text: Vec<String> = items.iter().filter_map(|i| match i["type"].as_i64() {
                    Some(1) => i["text_item"]["text"].as_str().map(String::from),
                    Some(3) => i["voice_item"]["text"].as_str().map(String::from),
                    _ => None,
                }).collect();
                let cdn = Some(self.setting(channel, "cdnBase")).filter(|s| !s.is_empty()).unwrap_or_else(|| "https://novac2c.cdn.weixin.qq.com/c2c".into());
                let files = channel_media::wechat_files(&client, &cdn, &items);
                let from = m["from_user_id"].as_str().unwrap_or_default().to_string();
                let context = m["context_token"].clone();
                let sink = ordered(self, &channel.id, &from, 2000, json!({"to":from,"context":context,"messageId":event_id}));
                let incoming = Incoming { sender: short(&from), sender_id: from, text: text.join("\n"), group: false, files };
                self.channel_message(&channel.id, incoming, sink).await;
            }
            // Acknowledge this batch only after its messages have been handled and
            // accepted execution inputs have been staged for recovery.
            if let Some(next) = next_cursor {
                self.channels.update(&channel.id, |c| { c.settings.insert("cursor".into(), next.clone()); })?;
                cursor = next;
            }
        }
    }

    /// Starts a WeChat sign-in: the QR code to scan, as SVG. Once it is scanned and
    /// confirmed, the token is saved, the person who scanned is allowed, and the channel
    /// connects.
    pub async fn channel_login(self: &Arc<Self>, id: &str) -> Result<Value> {
        let channel = self.channels.get(id).ok_or_else(|| anyhow!("Unknown channel"))?;
        if channel.kind != "wechat" {
            bail!("Only WeChat signs in with a QR code.");
        }
        if !channel.enabled { bail!("Resume this platform before signing in."); }
        if let Some(task) = self.channels.logins.lock().unwrap().remove(id) { task.abort(); }
        let base = Some(self.setting(&channel, "baseUrl")).filter(|b| !b.is_empty()).unwrap_or_else(|| WECHAT_BASE.into());
        let base = base.trim_end_matches('/').to_string();
        let client = http_client()?;
        let answer: Value = client.get(format!("{base}/ilink/bot/get_bot_qrcode?bot_type=3")).timeout(Duration::from_secs(30)).send().await.context("WeChat does not answer")?.json().await?;
        let code = answer["qrcode"].as_str().ok_or_else(|| anyhow!("WeChat gave no QR code"))?.to_string();
        let content = answer["qrcode_img_content"].as_str().unwrap_or(&code).to_string();
        let svg = qrcode::QrCode::new(content.as_bytes())?.render::<qrcode::render::svg::Color>().min_dimensions(220, 220).build();
        if !self.channels.get(id).is_some_and(|c| c.enabled) { bail!("This platform is paused or removed."); }
        if let Some(task) = self.channels.tasks.lock().unwrap().remove(id) { task.abort(); }
        self.channels.set_status(id, |s| {
            s.state = "login".into();
            s.detail = "Scan with WeChat, then confirm on the phone.".into();
            s.qr = Some(svg.clone());
        });
        self.team_changed().await;
        let team = self.clone();
        let id = id.to_string();
        let shown = svg.clone();
        let login_id = id.clone();
        let login = tokio::spawn(async move {
            let deadline = Instant::now() + Duration::from_secs(8 * 60);
            while Instant::now() < deadline {
                if team.channels.status.lock().unwrap().get(&id).and_then(|s| s.qr.clone()).as_deref() != Some(shown.as_str()) {
                    return; // a newer sign-in took over
                }
                let polled = async {
                    let answer: Value = client
                        .get(format!("{base}/ilink/bot/get_qrcode_status?qrcode={}", urlencoding::encode(&code)))
                        .header("iLink-App-ClientVersion", "1")
                        .timeout(Duration::from_secs(40))
                        .send()
                        .await?
                        .json()
                        .await?;
                    anyhow::Ok(answer)
                }
                .await;
                let Ok(answer) = polled else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                };
                match answer["status"].as_str() {
                    Some("scaned") => team.channels.set_status(&id, |s| s.detail = "Scanned: confirm on the phone.".into()),
                    Some("expired") => {
                        team.channel_status(&id, "error", "The QR code ran out: press Sign in again.").await;
                        return;
                    }
                    Some("confirmed") => {
                        let token = answer["bot_token"].as_str().unwrap_or_default().to_string();
                        let user = answer["ilink_user_id"].as_str().unwrap_or_default().to_string();
                        let new_base = answer["baseurl"].as_str().unwrap_or_default().to_string();
                        if token.is_empty() { team.channel_status(&id, "error", "WeChat gave no sign-in token: sign in again.").await; return; }
                        if !team.channels.get(&id).is_some_and(|c| c.enabled) { return; }
                        let saved = team.channels.update(&id, |c| {
                            c.settings.insert("token".into(), token);
                            c.settings.remove("cursor");
                            if !new_base.is_empty() {
                                c.settings.insert("baseUrl".into(), new_base);
                            }
                            if !user.is_empty() && !c.allow.contains(&user) {
                                c.allow.push(user);
                            }
                        });
                        if let Err(error) = saved {
                            team.channel_status(&id, "error", format!("{error:#}")).await;
                            return;
                        }
                        team.channels.logins.lock().unwrap().remove(&id);
                        team.restart_channel(&id);
                        team.team_changed().await;
                        return;
                    }
                    _ => {}
                }
                team.team_changed().await;
            }
            team.channel_status(&id, "error", "Nobody scanned the QR code: press Sign in again.").await;
        });
        self.channels.logins.lock().unwrap().insert(login_id, login.abort_handle());
        Ok(json!({ "qr": svg }))
    }
}

pub(crate) async fn wechat_post(client: &reqwest::Client, base: &str, token: &str, path: &str, body: Value, secs: u64) -> Result<Value> {
    wechat_post_timeout(client,base,token,path,body,Duration::from_secs(secs)).await
}

async fn wechat_post_timeout(client: &reqwest::Client, base: &str, token: &str, path: &str, body: Value, timeout: Duration) -> Result<Value> {
    use base64::Engine;
    let uin = u32::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..4].try_into().unwrap());
    let response = client
        .post(format!("{base}/ilink/bot/{path}"))
        .header("AuthorizationType", "ilink_bot_token")
        .header("X-WECHAT-UIN", base64::engine::general_purpose::STANDARD.encode(uin.to_string()))
        .bearer_auth(token)
        .json(&body)
        .timeout(timeout)
        .send()
        .await
        .context("WeChat does not answer")?
        .error_for_status()?;
    let text = response.text().await?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(&text)?)
}

/// The words of a Feishu rich-text ("post") message.
fn post_text(content: &Value) -> String {
    let body = if content["content"].is_array() { content } else { content.as_object().and_then(|m| m.values().find(|v| v["content"].is_array())).unwrap_or(content) };
    let mut lines = Vec::new();
    if let Some(title) = body["title"].as_str().filter(|t| !t.is_empty()) {
        lines.push(title.to_string());
    }
    for paragraph in body["content"].as_array().into_iter().flatten() {
        let line: String = paragraph.as_array().into_iter().flatten().filter_map(|el| el["text"].as_str()).collect();
        lines.push(line);
    }
    lines.join("\n")
}

async fn dingtalk_access(client: &reqwest::Client, token: &Token, client_id: &str, client_secret: &str) -> Result<String> {
    token.get(|| async {
        let answer: Value = client.post("https://api.dingtalk.com/v1.0/oauth2/accessToken").json(&json!({ "appKey": client_id, "appSecret": client_secret })).send().await?.error_for_status()?.json().await?;
        let access = answer["accessToken"].as_str().ok_or_else(|| anyhow!("DingTalk: {}", answer["message"]))?.to_string();
        Ok((access, number(&answer["expireIn"]).unwrap_or(7200)))
    }).await
}

async fn feishu_access(client: &reqwest::Client, token: &Token, domain: &str, app_id: &str, app_secret: &str) -> Result<String> {
    token.get(|| async {
        let answer: Value = client.post(format!("{domain}/open-apis/auth/v3/tenant_access_token/internal")).json(&json!({ "app_id": app_id, "app_secret": app_secret })).send().await?.error_for_status()?.json().await?;
        let token = answer["tenant_access_token"].as_str().ok_or_else(|| anyhow!("Feishu: {}", answer["msg"]))?.to_string();
        Ok((token, number(&answer["expire"]).unwrap_or(7200)))
    }).await
}

fn media_keys(value: &Value, key: &str) -> Vec<String> {
    match value {
        Value::Object(map) => map.iter().flat_map(|(k, v)| if k == key { v.as_str().map(String::from).into_iter().collect() } else { media_keys(v, key) }).collect(),
        Value::Array(items) => items.iter().flat_map(|v| media_keys(v, key)).collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// The Feishu long connection's frame, protobuf by hand:
// SeqID=1, LogID=2, service=3, method=4 (0 control, 1 data), headers=5 {key=1, value=2},
// payload_encoding=6, payload_type=7, payload=8, LogIDNew=9.

#[derive(Debug, Clone, Default, PartialEq)]
struct Frame {
    seq_id: u64,
    log_id: u64,
    service: i32,
    method: i32,
    headers: Vec<(String, String)>,
    payload_encoding: String,
    payload_type: String,
    payload: Vec<u8>,
    log_id_new: String,
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_varint(out, (field << 3) | 2);
    put_varint(out, bytes.len() as u64);
    out.extend_from_slice(bytes);
}

fn get_varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at).ok_or_else(|| anyhow!("frame cut short"))?;
        *at += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
    }
    bail!("varint too long")
}

/// Each field of a message: (number, varint value or bytes).
fn fields(bytes: &[u8]) -> Result<Vec<(u64, u64, &[u8])>> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < bytes.len() {
        let key = get_varint(bytes, &mut at)?;
        match key & 7 {
            0 => out.push((key >> 3, get_varint(bytes, &mut at)?, &[][..])),
            2 => {
                let len = get_varint(bytes, &mut at)? as usize;
                let end = at.checked_add(len).filter(|e| *e <= bytes.len()).ok_or_else(|| anyhow!("frame cut short"))?;
                out.push((key >> 3, 0, &bytes[at..end]));
                at = end;
            }
            1 => at += 8,
            5 => at += 4,
            wire => bail!("wire type {wire}"),
        }
    }
    Ok(out)
}

impl Frame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (field, value) in [(1, self.seq_id), (2, self.log_id), (3, self.service as u64), (4, self.method as u64)] {
            put_varint(&mut out, field << 3);
            put_varint(&mut out, value);
        }
        for (key, value) in &self.headers {
            let mut header = Vec::new();
            put_bytes(&mut header, 1, key.as_bytes());
            put_bytes(&mut header, 2, value.as_bytes());
            put_bytes(&mut out, 5, &header);
        }
        for (field, text) in [(6, &self.payload_encoding), (7, &self.payload_type)] {
            if !text.is_empty() {
                put_bytes(&mut out, field, text.as_bytes());
            }
        }
        if !self.payload.is_empty() {
            put_bytes(&mut out, 8, &self.payload);
        }
        if !self.log_id_new.is_empty() {
            put_bytes(&mut out, 9, self.log_id_new.as_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Frame> {
        let mut frame = Frame::default();
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        for (field, number, data) in fields(bytes)? {
            match field {
                1 => frame.seq_id = number,
                2 => frame.log_id = number,
                3 => frame.service = number as i32,
                4 => frame.method = number as i32,
                5 => {
                    let (mut key, mut value) = (String::new(), String::new());
                    for (f, _, d) in fields(data)? {
                        match f {
                            1 => key = text(d),
                            2 => value = text(d),
                            _ => {}
                        }
                    }
                    frame.headers.push((key, value));
                }
                6 => frame.payload_encoding = text(data),
                7 => frame.payload_type = text(data),
                8 => frame.payload = data.to_vec(),
                9 => frame.log_id_new = text(data),
                _ => {}
            }
        }
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_groups_only_reach_the_addressed_bot() {
        let mut message = json!({ "from": { "id": 42 }, "chat": { "type": "group" }, "text": "/stop@OtherBot" });
        assert!(telegram_text(&message, "MyBot", 900).is_none());
        for text in ["/stop", "hi @MyBotExtra", "ordinary group talk"] {
            message["text"] = json!(text);
            assert!(telegram_text(&message, "MyBot", 900).is_none());
        }
        message["text"] = json!("/stop@MYBOT");
        assert_eq!(telegram_text(&message, "MyBot", 900).as_deref(), Some("/stop"));
        message["text"] = json!("reply"); message["reply_to_message"] = json!({ "from": { "id": 900 } });
        assert_eq!(telegram_text(&message, "MyBot", 900).as_deref(), Some("reply"));
        message["from"]["is_bot"] = json!(true);
        assert!(telegram_text(&message, "MyBot", 900).is_none());
    }

    #[test]
    fn failed_channel_writes_leave_memory_and_disk_unchanged() {
        let home = tempfile::tempdir().unwrap(); let channels = Channels::open(home.path());
        let original = channels.upsert(Channel { kind: "wechat".into(), name: "Before".into(), enabled: true, ..Default::default() }).unwrap();
        std::fs::create_dir(home.path().join("channels.json.tmp")).unwrap();
        let mut changed = original.clone(); changed.name = "After".into();
        assert!(channels.upsert(changed).is_err());
        assert!(channels.update(&original.id, |c| c.enabled = false).is_err());
        assert!(channels.remove(&original.id).is_err());
        assert_eq!(channels.get(&original.id).unwrap().name, "Before");
        assert!(channels.get(&original.id).unwrap().enabled);
        assert_eq!(Channels::open(home.path()).get(&original.id).unwrap().name, "Before");
    }

    #[test]
    fn wechat_internal_login_settings_survive_an_edit() {
        let home = tempfile::tempdir().unwrap(); let channels = Channels::open(home.path());
        let saved = channels.upsert(Channel { kind: "wechat".into(), settings: [("token".into(), "private".into()), ("baseUrl".into(), "https://region.example".into()), ("cursor".into(), "next".into())].into(), allow: vec!["42".into(), "7".into(), "42".into()], ..Default::default() }).unwrap();
        let again = channels.upsert(Channel { id: saved.id, kind: "wechat".into(), ..Default::default() }).unwrap();
        assert_eq!(again.settings["baseUrl"], "https://region.example");
        assert_eq!(again.settings["token"], "private");
        assert_eq!(again.settings["cursor"], "next");
        assert_eq!(saved.allow, vec!["42", "7"]);
        assert!(channels.view()[0]["settings"].get("baseUrl").is_none());
    }

    #[test]
    fn a_feishu_frame_round_trips() {
        let frame = Frame { seq_id: 3, log_id: 300, service: 12345, method: 1, headers: vec![("type".into(), "event".into()), ("sum".into(), "1".into())], payload: br#"{"a":1}"#.to_vec(), ..Default::default() };
        let decoded = Frame::decode(&frame.encode()).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(decoded.header("type"), Some("event"));
        assert!(Frame::decode(&[0x0a, 0x05, 0x01]).is_err());
    }

    #[test]
    fn long_replies_are_cut_at_line_ends() {
        assert_eq!(pieces("one\ntwo\nthree", 8), vec!["one\ntwo", "three"]);
        assert_eq!(pieces("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(pieces("  \n", 10), Vec::<String>::new());
        assert_eq!(pieces("你好世界", 2), vec!["你好", "世界"]);
    }

    #[test]
    fn secrets_stay_saved_and_out_of_the_view() {
        let home = tempfile::tempdir().unwrap();
        let channels = Channels::open(home.path());
        let saved = channels.upsert(Channel { kind: "telegram".into(), target: "bot:a".into(), settings: [("token".to_string(), "123:abc".to_string())].into(), ..Default::default() }).unwrap();
        assert!(saved.id.starts_with("ch-"));
        assert_eq!(saved.name, "Telegram");
        let view = channels.view();
        assert_eq!(view[0]["settings"]["token"], MASK);
        assert_eq!(view[0]["status"]["state"], "off");
        let again = channels.upsert(Channel { id: saved.id.clone(), kind: "telegram".into(), name: "Mine".into(), target: "bot:a".into(), settings: [("token".to_string(), MASK.to_string())].into(), allow: vec![" 42 ".into()], ..Default::default() }).unwrap();
        assert_eq!(again.settings["token"], "123:abc");
        assert_eq!(again.allow, vec!["42"]);
        assert_eq!(Channels::open(home.path()).list.lock().unwrap().len(), 1);
        assert!(channels.upsert(Channel { kind: "qq".into(), target: "bot:a".into(), ..Default::default() }).is_err());
        assert!(channels.upsert(Channel { kind: "slack".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn feishu_rich_text_reads_as_lines() {
        let content = json!({ "zh_cn": { "title": "T", "content": [[{ "tag": "text", "text": "a" }, { "tag": "at", "user_id": "x" }, { "tag": "text", "text": "b" }]] } });
        assert_eq!(post_text(&content), "T\nab");
    }
}
