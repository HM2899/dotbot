//! Providers, switched for the whole team the way cc-switch does it, one in use per kind:
//! "text" is where the native pi-agent engine sends the team's requests, over Anthropic
//! Messages or OpenAI Chat Completions; "image" and "video" are the generation services
//! behind the Bots' generate_image and generate_video tools. A text provider lists its models
//! and a default; Bots and dotbot's background jobs each name one of them, or take the
//! default. Kept in `<home>/providers.json`, readable by the owner only; keys never go to the
//! page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The id the former built-in Claude Code card had; removed when a team is opened.
const LEGACY_OFFICIAL: &str = "official";
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
pub const KINDS: &[&str] = &["text", "image", "video"];
/// dotbot's own background jobs; each runs on a model of the provider in use, the default
/// one unless chosen. "planning" picks skills, past lessons and collaborators before a turn:
/// it is on the reply's critical path, so a fast model suits it.
pub const JOBS: &[&str] = &["approve", "review", "memory", "planning"];
/// How a video service takes a job and reports it done (see media_gen).
pub const VIDEO_FORMATS: &[&str] = &["zhipu", "ark", "siliconflow"];
/// Claude Code's model aliases. Settings from before the native engine may still hold them;
/// they name no model of any provider, so they mean "the default".
const LEGACY_ALIASES: &[&str] = &["default", "opus", "sonnet", "haiku", "opusplan"];

fn legacy_alias(model: &str) -> bool {
    LEGACY_ALIASES.contains(&model.trim())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// "text", "image" or "video".
    #[serde(default = "text")]
    pub kind: String,
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Native model transport: anthropic Messages or openai Chat Completions.
    #[serde(default = "anthropic")]
    pub api: String,
    /// Text on Anthropic Messages: "token" sends the key as `Authorization: Bearer`, "key" as
    /// `x-api-key`. OpenAI Chat Completions, image and video always use Bearer.
    #[serde(default = "token")]
    pub auth: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Text: the model for Bot chats. Image and video: the model that generates.
    #[serde(default)]
    pub default_model: String,
    /// Text: the models' context window in tokens (0: DEFAULT_CONTEXT_WINDOW). The engine
    /// summarizes older exchanges before a request would not fit.
    #[serde(default)]
    pub context_window: u64,
    /// Text: the reasoning effort Bots use unless their own settings name one. Empty sends no
    /// setting, so the model decides.
    #[serde(default)]
    pub default_effort: String,
    /// Video: how the service is called, one of VIDEO_FORMATS.
    #[serde(default)]
    pub format: String,
    /// Image and video: the size asked for when the Bot names none, such as 1024x1024.
    #[serde(default)]
    pub size: String,
}

fn token() -> String {
    "token".into()
}
fn anthropic() -> String { "anthropic".into() }

fn text() -> String {
    "text".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Saved {
    #[serde(default)]
    active: String,
    /// The image and video providers in use; empty when that kind is off.
    #[serde(default)]
    active_image: String,
    #[serde(default)]
    active_video: String,
    /// Job → model of the provider in use; empty or missing is its default model.
    #[serde(default)]
    jobs: BTreeMap<String, String>,
    #[serde(default)]
    providers: Vec<Provider>,
}

impl Saved {
    fn is_active(&self, p: &Provider) -> bool {
        match p.kind.as_str() {
            "image" => self.active_image == p.id,
            "video" => self.active_video == p.id,
            _ => self.active == p.id,
        }
    }

    fn jobs(&self) -> BTreeMap<String, String> {
        JOBS.iter().map(|j| (j.to_string(), self.jobs.get(*j).filter(|m| !legacy_alias(m)).cloned().unwrap_or_default())).collect()
    }
}

/// What the engine needs to reach a text provider: protocol, address, key and models,
/// as environment entries that take precedence over the service's own environment.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    pub env: Vec<(String, String)>,
}

impl Provider {
    /// Stands in when no text provider is in use: the engine refuses it with a clear error.
    fn none() -> Self {
        Provider { name: "未配置".into(), kind: text(), api: anthropic(), ..Default::default() }
    }

    /// A model this provider runs: one of its models, or any plain name when it lists none.
    /// Ids like "deepseek/deepseek-chat" from a DS Bot team only when listed.
    pub fn takes(&self, model: &str) -> bool {
        let model = model.trim();
        !model.is_empty() && !legacy_alias(model) && (self.models.iter().any(|m| m == model) || (self.models.is_empty() && !model.contains('/')))
    }

    /// The model a run asks for: the one named when this provider runs it, else the default
    /// model, else its first model (None: the provider has none set).
    pub fn chat_model(&self, wanted: Option<&str>) -> Option<String> {
        wanted.filter(|m| self.takes(m)).map(str::to_string).or_else(|| Some(self.default_model.clone()).filter(|m| !m.is_empty())).or_else(||self.models.first().cloned())
    }

    /// The reasoning effort for a run: the Bot's own when it has one, else this provider's default.
    pub fn effort_for(&self, bot: Option<&str>) -> Option<String> {
        bot.map(str::trim).filter(|e| !e.is_empty()).or(Some(self.default_effort.trim()).filter(|e| !e.is_empty())).map(str::to_string)
    }

    /// The model an image or video provider generates with.
    pub fn media_model(&self) -> String {
        Some(self.default_model.clone()).filter(|m| !m.is_empty()).or_else(|| self.models.first().cloned()).unwrap_or_default()
    }

    /// What the engine needs to reach this text provider, as environment-style entries.
    fn env(&self) -> Vec<(String, String)> {
        if self.kind != "text" {
            return Vec::new();
        }
        vec![
            ("DOTBOT_MODEL_API".into(), if self.api.is_empty() { anthropic() } else { self.api.clone() }),
            ("DOTBOT_MODEL_BASE_URL".into(), self.base_url.trim_end_matches('/').to_string()),
            ("DOTBOT_MODEL_KEY".into(), self.api_key.clone()),
            ("DOTBOT_MODEL_AUTH".into(), if self.api == "anthropic" && self.auth == "key" { "x-api-key".into() } else { "bearer".into() }),
            ("DOTBOT_MODEL_DEFAULT".into(), self.chat_model(None).unwrap_or_default()),
            ("DOTBOT_MODEL_CONTEXT".into(), if self.context_window == 0 { DEFAULT_CONTEXT_WINDOW } else { self.context_window }.to_string()),
        ]
    }

    /// What the page sees: everything but the key, which shows as its last four characters.
    pub fn view(&self, active: bool) -> Value {
        let mut v = serde_json::to_value(self).unwrap();
        v.as_object_mut().unwrap().remove("apiKey");
        let tail: String = self.api_key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        v["hasKey"] = json!(!self.api_key.is_empty());
        v["keyHint"] = json!(if self.api_key.chars().count() > 8 { format!("••••{tail}") } else if self.api_key.is_empty() { String::new() } else { "••••".into() });
        v["active"] = json!(active);
        v
    }

    fn check(mut self) -> Result<Self> {
        if self.kind.is_empty() {
            self.kind = text();
        }
        if !KINDS.contains(&self.kind.as_str()) {
            bail!("A provider is for text, images or videos.");
        }
        self.name = self.name.trim().to_string();
        if self.name.is_empty() || self.name.chars().count() > 40 {
            bail!("A provider needs a name of at most 40 characters.");
        }
        {
            self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
            if !self.base_url.starts_with("http://") && !self.base_url.starts_with("https://") {
                bail!("A provider needs its http(s) address.");
            }
        }
        if self.kind == "text" {
            if self.api.is_empty() { self.api = anthropic(); }
            if !["anthropic", "openai"].contains(&self.api.as_str()) { bail!("请选择 Anthropic Messages 或 OpenAI Chat Completions 协议。"); }
            if !["token", "key"].contains(&self.auth.as_str()) {
                self.auth = token();
            }
            if self.api == "openai" { self.auth = token(); }
            if self.context_window != 0 && !(8_000..=10_000_000).contains(&self.context_window) {
                bail!("上下文窗口应在 8000 到 10000000 tokens 之间，留空使用默认 128000。");
            }
            self.default_effort = crate::pi_agent::validate_effort(Some(&self.default_effort))?.unwrap_or_default();
            self.format.clear();
            self.size.clear();
        } else {
            self.default_effort.clear();
            self.auth = token();
            self.context_window = 0;
            self.size = self.size.trim().to_string();
            self.format = if self.kind == "video" && VIDEO_FORMATS.contains(&self.format.as_str()) { self.format } else if self.kind == "video" { "zhipu".into() } else { String::new() };
        }
        let mut seen = std::collections::HashSet::new();
        self.models = self.models.iter().map(|m| m.trim().to_string()).filter(|m| !m.is_empty() && seen.insert(m.clone())).collect();
        self.default_model = self.default_model.trim().to_string();
        if self.kind != "text" && self.media_model().is_empty() {
            bail!("Name the model that generates.");
        }
        Ok(self)
    }
}

pub struct Providers {
    path: PathBuf,
    saved: std::sync::Mutex<Saved>,
}

impl Providers {
    pub fn open(home: &Path) -> Self {
        let path = home.join("providers.json");
        let mut saved: Saved = crate::sqlite_storage::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // The former built-in Claude Code card goes. One the user gave a key to stays as an
        // ordinary Anthropic provider; Claude model names it was seeded with go either way.
        let mut migrated = false;
        if let Some(at) = saved.providers.iter().position(|p| p.id == LEGACY_OFFICIAL) {
            let mut legacy = saved.providers.remove(at);
            migrated = true;
            if !legacy.api_key.is_empty() {
                let id = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
                if saved.active == LEGACY_OFFICIAL { saved.active = id.clone(); }
                legacy.id = id;
                legacy.name = "Anthropic".into();
                legacy.preset = "anthropic".into();
                legacy.api = anthropic();
                if legacy.base_url.is_empty() { legacy.base_url = "https://api.anthropic.com".into(); }
                let seeded = ["opus", "sonnet", "haiku", "claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5"];
                legacy.models.retain(|m| !seeded.contains(&m.as_str()));
                if seeded.contains(&legacy.default_model.as_str()) { legacy.default_model.clear(); }
                saved.providers.insert(0, legacy);
            }
        }
        if saved.jobs.values().any(|m| legacy_alias(m)) {
            saved.jobs.retain(|_, m| !legacy_alias(m));
            migrated = true;
        }
        let has = |id: &str, kind: &str| saved.providers.iter().any(|p| p.id == id && p.kind == kind);
        if !has(&saved.active, "text") {
            // The only text provider with a key (or a keyless OpenAI-compatible one) goes into use.
            saved.active = saved.providers.iter().find(|p| p.kind == "text" && (!p.api_key.is_empty() || p.api == "openai")).map(|p| p.id.clone()).unwrap_or_default();
        }
        if !has(&saved.active_image, "image") {
            saved.active_image.clear();
        }
        if !has(&saved.active_video, "video") {
            saved.active_video.clear();
        }
        let providers = Providers { path, saved: std::sync::Mutex::new(saved) };
        if migrated {
            let saved = providers.saved.lock().unwrap();
            if let Err(error) = providers.save(&saved) { eprintln!("dotbot: provider migration not saved: {error}"); }
        }
        providers
    }

    fn save(&self, saved: &Saved) -> Result<()> {
        write_private(&self.path, &serde_json::to_string_pretty(saved)?)
    }

    /// The text provider in use; a stand-in with no address when none is set up yet.
    pub fn active(&self) -> Provider {
        let saved = self.saved.lock().unwrap();
        saved.providers.iter().find(|p| p.id == saved.active).cloned().unwrap_or_else(Provider::none)
    }

    /// The provider in use for a kind; None when images or videos are off.
    pub fn active_of(&self, kind: &str) -> Option<Provider> {
        if kind == "text" {
            return Some(self.active());
        }
        let saved = self.saved.lock().unwrap();
        saved.providers.iter().find(|p| p.kind == kind && saved.is_active(p)).cloned()
    }

    pub fn get(&self, id: &str) -> Option<Provider> {
        self.saved.lock().unwrap().providers.iter().find(|p| p.id == id).cloned()
    }

    /// The model for a run: a Bot's chat takes the Bot's model, a background job the model
    /// chosen for it, each only when the provider in use runs it; otherwise its default.
    pub fn model_for(&self, job: &str, bot: Option<&str>) -> Option<String> {
        let active = self.active();
        if job == "chat" || job == "group" {
            return active.chat_model(bot);
        }
        let chosen = self.saved.lock().unwrap().jobs().get(job).cloned().unwrap_or_default();
        active.chat_model(Some(&chosen))
    }

    /// Job → model; an empty model returns the job to the provider's default.
    pub fn set_jobs(&self, jobs: &Value) -> Result<()> {
        let mut saved = self.saved.lock().unwrap();
        for (job, model) in jobs.as_object().into_iter().flatten() {
            let model = model.as_str().ok_or_else(|| anyhow!("A job's model is a model name."))?.trim();
            if !JOBS.contains(&job.as_str()) {
                bail!("Unknown background job {job}.");
            }
            if model.is_empty() || legacy_alias(model) { saved.jobs.remove(job); } else { saved.jobs.insert(job.clone(), model.into()); }
        }
        self.save(&saved)
    }

    pub fn view(&self) -> Value {
        let saved = self.saved.lock().unwrap();
        json!({
            "active": saved.active, "activeImage": saved.active_image, "activeVideo": saved.active_video,
            "jobs": saved.jobs(), "formats": VIDEO_FORMATS,
            "providers": saved.providers.iter().map(|p| p.view(saved.is_active(p))).collect::<Vec<_>>(),
        })
    }

    /// Adds or changes a provider. An empty `apiKey` keeps the saved key; a provider's kind
    /// stays what it was made as.
    pub fn upsert(&self, mut provider: Provider) -> Result<Provider> {
        let mut saved = self.saved.lock().unwrap();
        let before = saved.providers.iter().find(|p| !provider.id.is_empty() && p.id == provider.id).cloned();
        if let Some(before) = &before {
            if provider.api_key.trim().is_empty() {
                provider.api_key = before.api_key.clone();
            }
            provider.kind = before.kind.clone();
        } else {
            provider.id = format!("p-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        }
        provider.api_key = provider.api_key.trim().to_string();
        let provider = provider.check()?;
        match saved.providers.iter_mut().find(|p| p.id == provider.id) {
            Some(slot) => *slot = provider.clone(),
            None => saved.providers.push(provider.clone()),
        }
        // The first image or video provider saved with a key goes into use: adding one and
        // then finding the Bots still without the tool is what people trip over.
        let slot = match provider.kind.as_str() {
            "image" => Some(&mut saved.active_image),
            "video" => Some(&mut saved.active_video),
            _ => None,
        };
        if let Some(slot) = slot
            && slot.is_empty()
            && !provider.api_key.is_empty()
        {
            *slot = provider.id.clone();
        }
        self.save(&saved)?;
        Ok(provider)
    }

    /// Sets only a text provider's default reasoning effort ("" clears it).
    pub fn set_effort(&self, id: &str, effort: &str) -> Result<()> {
        let mut saved = self.saved.lock().unwrap();
        let provider = saved.providers.iter_mut().find(|p| p.id == id).ok_or_else(|| anyhow!("No such provider"))?;
        if provider.kind != "text" { bail!("Only text providers have a reasoning effort."); }
        provider.default_effort = crate::pi_agent::validate_effort(Some(effort))?.unwrap_or_default();
        self.save(&saved)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut saved = self.saved.lock().unwrap();
        if let Some(p) = saved.providers.iter().find(|p| p.id == id)
            && saved.is_active(p)
        {
            if p.kind == "text" {
                bail!("Switch to another provider before removing this one.");
            }
            bail!("Stop using this provider before removing it.");
        }
        saved.providers.retain(|p| p.id != id);
        self.save(&saved)
    }

    pub fn use_provider(&self, id: &str) -> Result<Provider> {
        let mut saved = self.saved.lock().unwrap();
        let provider = saved.providers.iter().find(|p| p.id == id).cloned().ok_or_else(|| anyhow!("No such provider"))?;
        // Local OpenAI-compatible servers such as Ollama take no key.
        if provider.api_key.is_empty() && !(provider.kind == "text" && provider.api == "openai") {
            bail!("Add this provider's key before switching to it.");
        }
        match provider.kind.as_str() {
            "image" => saved.active_image = id.into(),
            "video" => saved.active_video = id.into(),
            _ => saved.active = id.into(),
        }
        self.save(&saved)?;
        Ok(provider)
    }

    /// Turns image or video generation off: the Bots lose that tool.
    pub fn stop(&self, kind: &str) -> Result<()> {
        let mut saved = self.saved.lock().unwrap();
        match kind {
            "image" => saved.active_image.clear(),
            "video" => saved.active_video.clear(),
            _ => bail!("A text provider is always in use; switch to another instead."),
        }
        self.save(&saved)
    }

    /// How the engine reaches a text provider (the active one when `id` is None).
    pub fn launch(&self, id: Option<&str>) -> Launch {
        let provider = id.and_then(|id| self.get(id)).filter(|p| p.kind == "text").unwrap_or_else(|| self.active());
        Launch { env: provider.env() }
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?)
}

/// The models a provider lists: Anthropic-style at /v1/models for text, OpenAI-style at
/// <address>/models for image and video services.
pub async fn fetch_models(base_url: &str, api_key: &str, kind: &str, api: &str) -> Result<Vec<String>> {
    let base = base_url.trim().trim_end_matches('/');
    let request = if kind == "text" && api != "openai" {
        let url = if base.ends_with("/v1") { format!("{base}/models") } else { format!("{base}/v1/models") };
        client()?.get(url).header("x-api-key", api_key).bearer_auth(api_key).header("anthropic-version", "2023-06-01")
    } else {
        client()?.get(format!("{base}/models")).bearer_auth(api_key)
    };
    let response = request.send().await?;
    let status = response.status();
    let body: Value = response.json().await.unwrap_or_default();
    if !status.is_success() {
        bail!("The provider answered {status}: {}", body["error"]["message"].as_str().or(body["message"].as_str()).unwrap_or("no model list"));
    }
    let models: Vec<String> = body["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    if models.is_empty() {
        bail!("The provider lists no models; type them in.");
    }
    Ok(models)
}

/// Whether an image or video service takes the key, without generating anything (each
/// generation costs money): "accepted", or "unchecked" when it has no model list to ask.
pub async fn check_key(base_url: &str, api_key: &str) -> Result<&'static str> {
    let response = client()?.get(format!("{}/models", base_url.trim().trim_end_matches('/'))).bearer_auth(api_key).send().await?;
    match response.status().as_u16() {
        200..=299 => Ok("accepted"),
        401 | 403 => bail!("The service refused the key ({}).", response.status()),
        404 | 405 => Ok("unchecked"),
        _ => bail!("The service answered {}.", response.status()),
    }
}

fn write_private(path: &Path, text: &str) -> Result<()> {
    if let Some(result) = crate::sqlite_storage::save(path, text.as_bytes()) { return result; }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deepseek() -> Provider {
        Provider {
            name: "DeepSeek".into(),
            base_url: "https://api.deepseek.com/anthropic/".into(),
            api_key: "sk-test-1234567890".into(),
            models: vec!["deepseek-chat".into(), "deepseek-reasoner".into()],
            default_model: "deepseek-chat".into(),
            ..Default::default()
        }
    }

    fn env(providers: &Providers) -> BTreeMap<String, String> {
        providers.launch(None).env.into_iter().collect()
    }

    #[test]
    fn a_new_team_has_no_text_provider_until_one_is_added() {
        let home = tempfile::tempdir().unwrap();
        let providers = Providers::open(home.path());
        assert!(providers.view()["providers"].as_array().unwrap().is_empty());
        assert_eq!(env(&providers)["DOTBOT_MODEL_BASE_URL"], "");
        let saved = providers.upsert(deepseek()).unwrap();
        assert_eq!((saved.kind.as_str(), saved.base_url.as_str()), ("text", "https://api.deepseek.com/anthropic"));
        providers.use_provider(&saved.id).unwrap();
        let reopened = Providers::open(home.path());
        let env = env(&reopened);
        assert_eq!(env["DOTBOT_MODEL_KEY"], "sk-test-1234567890");
        assert_eq!(env["DOTBOT_MODEL_AUTH"], "bearer");
        assert_eq!(env["DOTBOT_MODEL_DEFAULT"], "deepseek-chat");
        assert_eq!(env["DOTBOT_MODEL_CONTEXT"], "128000");
        assert!(providers.upsert(Provider { context_window: 100, ..deepseek() }).is_err());
        assert!(!env.keys().any(|k| k.starts_with("ANTHROPIC_")));
        let view = providers.view().to_string();
        assert!(!view.contains("sk-test-1234567890") && view.contains("••••7890") && !view.contains("tiers"));
        providers.upsert(Provider { id: saved.id.clone(), api_key: String::new(), ..deepseek() }).unwrap();
        assert_eq!(providers.get(&saved.id).unwrap().api_key, "sk-test-1234567890");
        assert!(providers.remove(&saved.id).is_err(), "the provider in use stays");
        let keyless = providers.upsert(Provider { name: "Ollama".into(), api: "openai".into(), auth: "key".into(), base_url: "http://127.0.0.1:11434/v1".into(), models: vec!["qwen3".into()], ..Default::default() }).unwrap();
        assert_eq!(keyless.auth, "token", "OpenAI-compatible services always use Bearer");
        providers.use_provider(&keyless.id).unwrap();
        providers.remove(&saved.id).unwrap();
    }

    #[test]
    fn a_provider_can_set_the_default_reasoning_effort_and_a_bot_overrides_it() {
        let home = tempfile::tempdir().unwrap();
        let providers = Providers::open(home.path());
        let saved = providers.upsert(Provider { default_effort: " high ".into(), ..deepseek() }).unwrap();
        assert_eq!(saved.default_effort, "high", "trimmed and validated like a Bot's own setting");
        assert!(providers.upsert(Provider { default_effort: "ultra".into(), ..deepseek() }).is_err());
        assert_eq!(saved.effort_for(None).as_deref(), Some("high"));
        assert_eq!(saved.effort_for(Some("")).as_deref(), Some("high"), "an empty Bot setting follows the provider");
        assert_eq!(saved.effort_for(Some("low")).as_deref(), Some("low"));
        assert_eq!(saved.effort_for(Some("none")).as_deref(), Some("none"), "a Bot can switch thinking off");
        providers.set_effort(&saved.id, "medium").unwrap();
        assert_eq!(Providers::open(home.path()).get(&saved.id).unwrap().default_effort, "medium");
        assert!(providers.set_effort(&saved.id, "loud").is_err());
        providers.set_effort(&saved.id, "").unwrap();
        assert_eq!(providers.get(&saved.id).unwrap().effort_for(None), None, "empty sends no setting");
        providers.use_provider(&saved.id).unwrap();
        providers.set_effort(&saved.id, "xhigh").unwrap();
        assert_eq!(providers.view()["providers"][0]["defaultEffort"], "xhigh");
        let media = providers.upsert(Provider { name: "Images".into(), kind: "image".into(), base_url: "https://img.example.com/v1".into(), api_key: "k".into(), default_model: "img".into(), default_effort: "high".into(), ..Default::default() }).unwrap();
        assert!(media.default_effort.is_empty(), "only text providers think");
        assert!(providers.set_effort(&media.id, "high").is_err());
    }

    #[test]
    fn the_claude_code_card_tiers_and_aliases_are_migrated_away() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("providers.json"), json!({
            "active": "official", "jobs": {"approve": "haiku", "review": "deepseek-reasoner"},
            "providers": [{"id": "official", "name": "Claude Code", "kind": "text", "models": ["opus", "sonnet", "haiku"], "tiers": {"opus": "x"}},
                          {"id": "p-1", "name": "DeepSeek", "kind": "text", "baseUrl": "https://api.deepseek.com/anthropic", "apiKey": "sk-1", "models": ["deepseek-chat", "deepseek-reasoner"], "defaultModel": "deepseek-chat", "tiers": {"haiku": "deepseek-chat"}}],
        }).to_string()).unwrap();
        let providers = Providers::open(home.path());
        let view = providers.view();
        assert_eq!(view["providers"].as_array().unwrap().len(), 1, "the keyless Claude Code card is gone");
        assert_eq!(view["active"], "p-1", "the provider with a key goes into use");
        assert_eq!(view["jobs"]["approve"], "");
        assert_eq!(view["jobs"]["review"], "deepseek-reasoner");
        let saved = std::fs::read_to_string(home.path().join("providers.json")).unwrap();
        assert!(!saved.contains("official") && !saved.contains("tiers") && !saved.contains("haiku"));
        assert_eq!(providers.model_for("approve", None).as_deref(), Some("deepseek-chat"));
        assert_eq!(providers.model_for("review", None).as_deref(), Some("deepseek-reasoner"));
        assert_eq!(providers.model_for("chat", Some("sonnet")).as_deref(), Some("deepseek-chat"), "Claude aliases mean the default");
        assert_eq!(providers.model_for("chat", Some("claude-opus-4")).as_deref(), Some("deepseek-chat"));
        assert_eq!(providers.model_for("chat", Some("deepseek-reasoner")).as_deref(), Some("deepseek-reasoner"));

        let keyed = tempfile::tempdir().unwrap();
        std::fs::write(keyed.path().join("providers.json"), json!({"active": "official", "providers": [{"id": "official", "name": "Anthropic", "kind": "text", "baseUrl": "https://api.anthropic.com", "apiKey": "sk-ant", "auth": "key", "models": ["claude-sonnet-4-6", "my-model"], "defaultModel": "claude-sonnet-4-6"}]}).to_string()).unwrap();
        let providers = Providers::open(keyed.path());
        let active = providers.active();
        assert!(active.id.starts_with("p-") && active.name == "Anthropic");
        assert_eq!(active.models, vec!["my-model".to_string()]);
        assert_eq!(active.default_model, "");
        assert_eq!(env(&providers)["DOTBOT_MODEL_AUTH"], "x-api-key");
    }

    #[test]
    fn jobs_take_a_model_of_the_provider_in_use() {
        let home = tempfile::tempdir().unwrap();
        let providers = Providers::open(home.path());
        let id = providers.upsert(deepseek()).unwrap().id;
        providers.use_provider(&id).unwrap();
        assert_eq!(providers.model_for("approve", None).as_deref(), Some("deepseek-chat"));
        providers.set_jobs(&json!({ "approve": "deepseek-reasoner" })).unwrap();
        assert_eq!(Providers::open(home.path()).model_for("approve", None).as_deref(), Some("deepseek-reasoner"));
        providers.set_jobs(&json!({ "approve": "" })).unwrap();
        assert_eq!(providers.model_for("approve", None).as_deref(), Some("deepseek-chat"));
        providers.set_jobs(&json!({ "planning": "not-listed" })).unwrap();
        assert_eq!(providers.model_for("planning", None).as_deref(), Some("deepseek-chat"), "a model the provider does not run falls back");
        assert!(providers.set_jobs(&json!({ "nonsense": "x" })).is_err());
    }

    #[test]
    fn image_and_video_providers_switch_on_their_own() {
        let home = tempfile::tempdir().unwrap();
        let providers = Providers::open(home.path());
        assert!(providers.active_of("image").is_none());
        let image = providers.upsert(Provider { kind: "image".into(), name: "Img".into(), base_url: "https://x.example/v1".into(), api_key: "k-12345678".into(), models: vec!["pic-1".into()], ..Default::default() }).unwrap();
        let video = providers.upsert(Provider { kind: "video".into(), name: "Vid".into(), base_url: "https://y.example/v1".into(), api_key: "k-12345678".into(), default_model: "clip-1".into(), format: "weird".into(), ..Default::default() }).unwrap();
        assert_eq!(video.format, "zhipu");
        assert!(providers.upsert(Provider { kind: "image".into(), name: "No model".into(), base_url: "https://x.example".into(), ..Default::default() }).is_err());
        // The first of each kind with a key is in use as soon as it is saved.
        assert_eq!(providers.active_of("image").unwrap().id, image.id);
        assert_eq!(providers.active_of("video").unwrap().id, video.id);
        assert_eq!(providers.active().id, "");
        assert_eq!(providers.active_of("image").unwrap().media_model(), "pic-1");
        assert!(providers.remove(&video.id).is_err());
        providers.stop("video").unwrap();
        assert!(Providers::open(home.path()).active_of("video").is_none());
        // A media provider is never a text endpoint: the engine stays on the active text provider.
        assert_eq!(providers.launch(Some(&image.id)).env, providers.launch(None).env);
    }
}
