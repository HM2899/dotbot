//! Event setup evidence distinguishes a local signed test, provider registration,
//! URL verification and an actual provider delivery.
use crate::{
    durable,
    routines::{Routine, When},
    store::now_ms,
    team::Team,
};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct Events {
    path: PathBuf,
    states: Mutex<BTreeMap<String, Value>>,
    origin: Mutex<Option<String>>,
}
impl Events {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("event-status.json");
        let states = match crate::sqlite_storage::read(&path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        let origin = match crate::sqlite_storage::read(home.join("event-settings.json")) {
            Ok(b) => serde_json::from_slice::<Value>(&b)?["publicUrl"]
                .as_str()
                .map(String::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path,
            states: Mutex::new(states),
            origin: Mutex::new(origin),
        })
    }
    pub fn state(&self, id: &str) -> Value {
        self.states
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| json!({}))
    }
    pub fn update(&self, id: &str, patch: Value) -> Result<()> {
        let mut states = self.states.lock().unwrap();
        let mut next = states.clone();
        let slot = next.entry(id.into()).or_insert_with(|| json!({}));
        for (k, v) in patch
            .as_object()
            .ok_or_else(|| anyhow!("Invalid event status"))?
        {
            slot[k] = v.clone();
        }
        durable::save(&self.path, &next)?;
        *states = next;
        Ok(())
    }
    pub fn clear(&self, id: &str) -> Result<()> {
        let mut states = self.states.lock().unwrap();
        let mut next = states.clone();
        next.remove(id);
        durable::save(&self.path, &next)?;
        *states = next;
        Ok(())
    }
    pub fn set_origin(&self, raw: &str) -> Result<()> {
        let value = if raw.trim().is_empty() {
            None
        } else {
            let url = reqwest::Url::parse(raw.trim())?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                bail!(
                    "Use an HTTPS origin such as https://team.example.com, without a path, query or credentials."
                );
            }
            Some(url.origin().ascii_serialization())
        };
        let mut origin = self.origin.lock().unwrap();
        durable::save(
            &self.path.with_file_name("event-settings.json"),
            &json!({"publicUrl":value}),
        )?;
        *origin = value;
        Ok(())
    }
}
impl Team {
    pub fn public_origin(&self) -> String {
        self.events
            .origin
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.cfg.public_url.clone())
            .unwrap_or_else(|| self.cfg.url.clone())
    }
    pub fn event_configuration(&self, routine: &Routine) -> Value {
        json!({"when":routine.when,"webhook":format!("{}/hook/{}/{}",self.public_origin(),routine.id,routine.key)})
    }
    async fn event_routine(&self, id: &str) -> Result<Routine> {
        self.routines
            .list
            .lock()
            .await
            .iter()
            .find(|r| r.id == id && r.when.triggered())
            .cloned()
            .ok_or_else(|| anyhow!("Choose a webhook or platform event routine"))
    }
    pub async fn event_setup(&self, id: &str) -> Result<Value> {
        let routine = self.event_routine(id).await?;
        let source = match &routine.when {
            When::Event { source, .. } => source.as_str(),
            _ => "webhook",
        };
        let hook = format!(
            "{}/hook/{}/{}",
            self.public_origin(),
            routine.id,
            routine.key
        );
        let public = reqwest::Url::parse(&hook).is_ok_and(|u| {
            u.scheme() == "https"
                && !u.host_str().is_some_and(|h| {
                    h == "localhost" || h.parse::<std::net::IpAddr>().is_ok_and(|i| i.is_loopback())
                })
        });
        let guide = match source {
            "github" => vec![
                "连接 GitHub 账号并授予仓库 webhook 管理权限。",
                "选择仓库后点击“注册 GitHub 订阅”；已有同地址订阅会更新配置。",
                "点击“请求平台测试”让 GitHub 发出真实 ping；核对下方平台到达时间。",
            ],
            "slack" => vec![
                "在 Slack App 的 Basic Information 取得 Signing Secret，保存到下方密钥配置。",
                "在 Event Subscriptions 打开 Enable Events，把此回调地址填入 Request URL 并完成验证。",
                "订阅实际需要的 Bot Events（如 message.channels / message.im），添加相应 history 权限并重新安装 App。",
                "从目标频道发送一条真实消息；下方出现平台事件到达时间才代表已打通。Slack OAuth 或 MCP 连接本身不会创建事件订阅。",
            ],
            "teams" => vec![
                "在 Teams Workflows 创建消息触发器，并用 HTTP 步骤 POST 到此私有回调地址。",
                "若配置签名密钥，发送 X-Teambot-Signature-256: sha256=<原始请求体的 HMAC-SHA256>。",
                "从 Teams 触发实际工作流并核对到达时间；此模式不会自动创建 Microsoft Graph 订阅。",
            ],
            _ => vec![
                "从外部应用 POST 到此私有 URL；需要公网 HTTPS 及反向代理。",
                "使用 Idempotency-Key 请求头去重，先运行本地链路测试，再从外部发送真实事件。",
            ],
        };
        let mut status = self.events.state(id);
        let fingerprint = self.event_configuration(&routine);
        if status["configuration"] != fingerprint {
            status = json!({});
        }
        Ok(
            json!({"source":source,"webhook":hook,"public":public,"publicUrl":self.public_origin(),"guide":guide,"status":status,"configuration":fingerprint,"enabled":routine.enabled}),
        )
    }
    pub async fn event_local_test(self: &Arc<Self>, id: &str, payload: &Value) -> Result<Value> {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        let routine = self.event_routine(id).await?;
        if !routine.enabled {
            bail!("Resume the routine before testing its trigger.");
        }
        let body = serde_json::to_string(payload)?;
        if body.len() > 256 * 1024 {
            bail!("Event payload is at most 256 KiB");
        }
        let key = uuid::Uuid::new_v4().simple().to_string();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut request = client
            .post(format!(
                "{}/hook/{}/{}",
                self.cfg.url, routine.id, routine.key
            ))
            .header("content-type", "application/json")
            .header("x-teambot-local-test", &self.cfg.internal_token)
            .header("idempotency-key", format!("test:{key}"));
        if let When::Event {
            source,
            event,
            signing_secret,
            ..
        } = &routine.when
        {
            let secret = self.secrets.get(signing_secret);
            let timestamp = chrono::Utc::now().timestamp().to_string();
            let signed = if source == "slack" {
                format!("v0:{timestamp}:{body}")
            } else {
                body.clone()
            };
            if source != "teams" || secret.is_some() {
                let mut mac = Hmac::<Sha256>::new_from_slice(
                    secret
                        .as_deref()
                        .ok_or_else(|| anyhow!("Save the signing key before testing"))?
                        .as_bytes(),
                )?;
                mac.update(signed.as_bytes());
                let signature = hex::encode(mac.finalize().into_bytes());
                request = match source.as_str() {
                    "github" => {
                        request.header("x-hub-signature-256", format!("sha256={signature}"))
                    }
                    "slack" => request
                        .header("x-slack-signature", format!("v0={signature}"))
                        .header("x-slack-request-timestamp", timestamp),
                    _ => request.header("x-teambot-signature-256", format!("sha256={signature}")),
                };
            }
            if source == "github" {
                request = request
                    .header(
                        "x-github-event",
                        if event.is_empty() { "push" } else { event },
                    )
                    .header("x-github-delivery", format!("local:{key}"));
            }
        }
        // The diagnostic endpoint traverses the real signature/filter handler but
        // never dispatches model work or impersonates a provider delivery.
        let response = request
            .body(body)
            .send()
            .await
            .map_err(|_| anyhow!("Cannot reach this service's webhook handler"))?;
        if !response.status().is_success() {
            bail!("Webhook test failed; check the routine, key and payload format.");
        }
        let result: Value = response.json().await?;
        self.publish("routines", json!({}));
        Ok(result)
    }
    pub async fn event_register(
        &self,
        id: &str,
        account_id: &str,
        repository: &str,
    ) -> Result<Value> {
        let routine = self.event_routine(id).await?;
        let When::Event {
            source,
            event,
            signing_secret,
            ..
        } = &routine.when
        else {
            bail!("Only GitHub event routines can register repository hooks.");
        };
        if source != "github" {
            bail!("Use the provider setup guide to configure this event source.");
        }
        let parts = repository.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts.iter().any(|s| {
                s.is_empty()
                    || s.len() > 100
                    || !s
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                    || *s == "."
                    || *s == ".."
            })
        {
            bail!("Repository must be owner/name.");
        }
        let account = self.accounts.get(account_id)?;
        if account.provider != "github" {
            bail!("Choose an authorized GitHub account.");
        }
        let setup = self.event_setup(id).await?;
        // Loopback is supported for isolated protocol fixtures. Real GitHub needs public HTTPS.
        if setup["public"] != true && !account.endpoints.api.starts_with("http://127.0.0.1:") {
            bail!("Set a reachable HTTPS public URL before registering a provider subscription.");
        }
        let secret = self
            .secrets
            .get(signing_secret)
            .ok_or_else(|| anyhow!("Save the webhook signing key first"))?;
        let path = format!("/repos/{repository}/hooks");
        let existing = self
            .accounts
            .request(account_id, "GET", &format!("{path}?per_page=100"), None)
            .await?;
        let hook = existing
            .as_array()
            .and_then(|a| a.iter().find(|h| h["config"]["url"] == setup["webhook"]))
            .and_then(|h| h["id"].as_u64());
        let body = json!({"name":"web","active":routine.enabled,"events":if event.is_empty(){vec!["push"]}else{vec![event.as_str()]},"config":{"url":setup["webhook"],"content_type":"json","secret":secret,"insecure_ssl":"0"}});
        let result = if let Some(hook) = hook {
            self.accounts
                .request(account_id, "PATCH", &format!("{path}/{hook}"), Some(&body))
                .await?
        } else {
            self.accounts
                .request(account_id, "POST", &path, Some(&body))
                .await?
        };
        let hook = result["id"]
            .as_u64()
            .ok_or_else(|| anyhow!("GitHub did not return a webhook ID"))?;
        if self.events.state(id)["configuration"] != self.event_configuration(&routine) {
            self.events.clear(id)?;
        }
        self.events.update(id,json!({"configuration":self.event_configuration(&routine),"registered":now_ms(),"account":account_id,"repository":repository,"hook":hook,"providerActive":routine.enabled,"error":null}))?;
        self.publish("routines", json!({}));
        self.event_setup(id).await
    }
    pub async fn event_provider_test(&self, id: &str) -> Result<Value> {
        let setup = self.event_setup(id).await?;
        let status = &setup["status"];
        let (Some(account), Some(repository), Some(hook)) = (
            status["account"].as_str(),
            status["repository"].as_str(),
            status["hook"].as_u64(),
        ) else {
            bail!(
                "Register a GitHub subscription first; other providers require a real test from their console."
            );
        };
        self.accounts
            .request(
                account,
                "POST",
                &format!("/repos/{repository}/hooks/{hook}/tests"),
                Some(&json!({})),
            )
            .await?;
        self.events
            .update(id, json!({"providerTestRequested":now_ms()}))?;
        self.publish("routines", json!({}));
        Ok(json!({"requested":true,"received":false}))
    }
    pub async fn event_subscription(&self, id: &str, remove: bool) -> Result<Value> {
        let setup = self.event_setup(id).await?;
        let status = &setup["status"];
        let (Some(account), Some(repository), Some(hook)) = (
            status["account"].as_str(),
            status["repository"].as_str(),
            status["hook"].as_u64(),
        ) else {
            bail!("Register a GitHub subscription first.");
        };
        let result = self
            .accounts
            .request(
                account,
                if remove { "DELETE" } else { "GET" },
                &format!("/repos/{repository}/hooks/{hook}"),
                None,
            )
            .await;
        match result {
            Ok(value) => {
                if remove {
                    self.events.clear(id)?;
                } else {
                    self.events.update(id,json!({"subscriptionChecked":now_ms(),"providerActive":value["active"],"providerUrlMatches":value["config"]["url"]==setup["webhook"],"error":null}))?;
                }
                self.publish("routines", json!({}));
                self.event_setup(id).await
            }
            Err(error) => {
                self.events.update(
                    id,
                    json!({"subscriptionChecked":now_ms(),"error":error.to_string()}),
                )?;
                self.publish("routines", json!({}));
                Err(error)
            }
        }
    }
}
