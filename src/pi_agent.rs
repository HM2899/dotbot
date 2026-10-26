//! dotbot adapter for the Rust port of pi-agent-core; no JavaScript subprocess and no
//! Claude Code. Speaks Anthropic Messages or OpenAI Chat Completions to the provider in
//! use, runs the team tools through `/internal/tool`, and is itself the MCP client for
//! the standalone browser and the user's connections.
use crate::engine::{Event, Turn, TurnSpec};
use crate::pi_context::{self, Window};
use anyhow::{Context, Result, bail, ensure};
use futures::future::BoxFuture;
use pi_agent_core::{
    AgentContext, AgentControl, AgentEvent, Config, EventSink, Runtime, Tool, ToolCall, ToolResult,
    TurnDecision,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

const TEAM_PREFIX: &str = "mcp__teambot__";
/// Loads a skill's full workflow by name, as Claude Code's Skill tool did for plugin skills.
const SKILL_TOOL: &str = "Skill";
const SKILL_INDEX_MAX: usize = 200;
/// Tool output above this many characters is cut before it enters the session.
const TOOL_TEXT_MAX: usize = 100_000;

pub fn validate_effort(value: Option<&str>) -> Result<Option<String>> {
    let value = value.unwrap_or_default().trim();
    ensure!(["", "none", "minimal", "low", "medium", "high", "xhigh", "max"].contains(&value), "不支持的思考强度：{value}");
    Ok((!value.is_empty()).then(|| value.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Api {
    Anthropic,
    OpenAi,
}

struct Model {
    client: reqwest::Client,
    url: String,
    key: String,
    bearer: bool,
    model: String,
    api: Api,
    max_tokens: u64,
    effort: Option<String>,
    /// The provider's context window in tokens.
    context: usize,
    /// Anthropic prompt caching (cache_control); turned off for a compatible service that
    /// refuses it, which is then sent the plain request form.
    caching: AtomicBool,
    /// Test-only request headers (DOTBOT_TEST_MODEL_HEADERS=1) that let a fake model
    /// see what a CLI's arguments used to show; never set in normal operation.
    trace: Vec<(String, String)>,
}

impl Model {
    /// The provider comes only from the team's settings (`Provider::env` entries); nothing
    /// is read from the service's environment except the optional DOTBOT_MAX_TOKENS.
    fn new(env: &[(String, String)], model: Option<&str>) -> Result<Self> {
        let vars: BTreeMap<_, _> = env.iter().cloned().collect();
        let get = |key: &str| vars.get(key).cloned().unwrap_or_default();
        let api = if get("DOTBOT_MODEL_API") == "openai" {
            Api::OpenAi
        } else {
            Api::Anthropic
        };
        let base = get("DOTBOT_MODEL_BASE_URL");
        let base = base.trim_end_matches('/');
        ensure!(
            !base.is_empty(),
            "还没有可用的文本模型供应商：请在「设置 → 模型与供应商 → 文本」添加并启用一个"
        );
        let parsed = reqwest::Url::parse(base).context("模型地址无效")?;
        ensure!(
            ["http", "https"].contains(&parsed.scheme())
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none(),
            "模型地址不能包含凭据、查询参数或片段"
        );
        let key = get("DOTBOT_MODEL_KEY");
        // Local OpenAI-compatible servers (Ollama, vLLM, LM Studio) take no key.
        ensure!(
            !key.is_empty() || api == Api::OpenAi,
            "模型服务需要 API 密钥：在「设置 → 模型与供应商」中填写"
        );
        let resolved = model
            .filter(|m| !m.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| get("DOTBOT_MODEL_DEFAULT"));
        ensure!(
            !resolved.is_empty(),
            "需要模型 ID：在「设置 → 模型与供应商」中为当前供应商设置默认模型"
        );
        let context: usize = get("DOTBOT_MODEL_CONTEXT").parse().unwrap_or(128_000).max(8_000);
        // An answer never takes more than a quarter of a small window.
        let max_tokens = std::env::var("DOTBOT_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(16_384u64)
            .min(context as u64 / 4);
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(600))
                .build()?,
            url: match api {
                Api::Anthropic if base.ends_with("/v1") => format!("{base}/messages"),
                Api::Anthropic => format!("{base}/v1/messages"),
                Api::OpenAi => format!("{base}/chat/completions"),
            },
            bearer: get("DOTBOT_MODEL_AUTH") != "x-api-key" || api == Api::OpenAi,
            key,
            model: resolved,
            api,
            max_tokens,
            effort: None,
            context,
            caching: AtomicBool::new(api == Api::Anthropic),
            trace: vec![],
        })
    }

    fn post(&self) -> reqwest::RequestBuilder {
        let mut request = self.client.post(&self.url);
        for (name, value) in &self.trace {
            request = request.header(name, value);
        }
        let request = if self.api == Api::Anthropic {
            request.header("anthropic-version", "2023-06-01")
        } else {
            request
        };
        if self.key.is_empty() {
            request
        } else if self.bearer {
            request.bearer_auth(&self.key)
        } else {
            request.header("x-api-key", &self.key)
        }
    }

    /// Only apply explicit Bot settings to its tool loop; summaries and approval jobs
    /// keep their own defaults and may force tool_choice, which conflicts with thinking.
    fn apply_effort(&self, body: &mut Value) -> Result<()> {
        let Some(effort) = self.effort.as_deref() else { return Ok(()); };
        match self.api {
            Api::OpenAi => {
                body["reasoning_effort"] = json!(effort);
                let limit = body.as_object_mut().unwrap().remove("max_tokens");
                if let Some(limit) = limit { body["max_completion_tokens"] = limit; }
            }
            Api::Anthropic if effort == "none" => {
                body["thinking"] = json!({"type":"disabled"});
            }
            Api::Anthropic => {
                let requested = match effort {
                    "minimal" => 1024,
                    "low" => 2048,
                    "medium" => 4096,
                    "high" => 8192,
                    "xhigh" => 12288,
                    _ => 16384,
                };
                ensure!(self.max_tokens > 1024, "输出上限过小，无法启用思考；请提高 DOTBOT_MAX_TOKENS");
                body["thinking"] = json!({"type":"enabled","budget_tokens":requested.min(self.max_tokens - 1)});
            }
        }
        Ok(())
    }

    /// Sends one request; a refusal is reported in the form the turn's retry and model
    /// checks recognize, with the provider's own message but never the key.
    async fn send(&self, body: &Value) -> Result<reqwest::Response> {
        // One quiet retry for a broken connection or a 5xx gateway reply, for every request kind.
        match self.send_once(body).await {
            Err(error) if transient_request_error(&format!("{error:#}")) => {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                self.send_once(body).await
            }
            result => result,
        }
    }

    async fn send_once(&self, body: &Value) -> Result<reqwest::Response> {
        let response = self
            .post()
            .json(body)
            .send()
            .await
            .context("模型请求失败：connection reset")?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await.unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).unwrap_or_default();
        let mut message = parsed["error"]["message"]
            .as_str()
            .or(parsed["message"].as_str())
            .or(parsed["error"].as_str())
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect::<String>();
        if !self.key.is_empty() {
            message = message.replace(&self.key, "••••");
        }
        bail!("API Error: {}: {message} (model {})", status.as_u16(), self.model)
    }

    /// Sends a request built with or without prompt caching. A compatible service that
    /// refuses the cached form (HTTP 400, not for length) gets the plain form from then on.
    async fn send_built(&self, build: impl Fn(bool) -> Result<Value>) -> Result<reqwest::Response> {
        let caching = self.caching.load(Ordering::Relaxed);
        match self.send(&build(caching)?).await {
            Err(error) if caching && refuses_cached_form(&error.to_string()) => {
                self.caching.store(false, Ordering::Relaxed);
                self.send(&build(false)?).await
            }
            result => result,
        }
    }

    /// The Anthropic request: the stable system prompt, then this turn's context. With
    /// caching, breakpoints sit after the stable system prompt, the tools and the newest
    /// message, so each request re-reads the conversation so far from the cache.
    fn anthropic_body(&self, system: &str, turn: &str, messages: &[Value], tools: &[Tool], caching: bool) -> Result<Value> {
        let cache = json!({"type":"ephemeral"});
        let mut messages = to_anthropic_messages(messages)?;
        let mut body = json!({"model":self.model,"max_tokens":self.max_tokens,"stream":true});
        if caching {
            let mut blocks = vec![json!({"type":"text","text":system,"cache_control":cache})];
            if !turn.is_empty() {
                blocks.push(json!({"type":"text","text":turn}));
            }
            body["system"] = json!(blocks);
            if let Some(block) = messages.last_mut().and_then(|m| m["content"].as_array_mut()).and_then(|b| b.last_mut()).filter(|b| b["type"] != "thinking") {
                block["cache_control"] = cache.clone();
            }
        } else {
            body["system"] = json!(if turn.is_empty() { system.to_string() } else { format!("{system}\n\n{turn}") });
        }
        body["messages"] = json!(messages);
        if !tools.is_empty() {
            let mut tools: Vec<Value> = tools.iter().map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.parameters})).collect();
            if caching {
                tools.last_mut().unwrap()["cache_control"] = cache;
            }
            body["tools"] = json!(tools);
        }
        self.apply_effort(&mut body)?;
        Ok(body)
    }

    /// A plain text answer with no tools, for summaries.
    async fn complete(&self, system: &str, prompt: &str, max_tokens: u64) -> Result<String> {
        let max_tokens = max_tokens.min(self.max_tokens.max(1024));
        let body = match self.api {
            Api::Anthropic => json!({"model":self.model,"max_tokens":max_tokens,"system":system,"messages":[{"role":"user","content":prompt}]}),
            Api::OpenAi => json!({"model":self.model,"max_tokens":max_tokens,"messages":[{"role":"system","content":system},{"role":"user","content":prompt}]}),
        };
        let value: Value = self.send(&body).await?.json().await.context("模型返回了无效 JSON")?;
        let message = match self.api {
            Api::Anthropic => from_anthropic_message(value)?,
            Api::OpenAi => from_openai_response(&value)?,
        };
        Ok(message_text(&message))
    }

    /// One structured answer: the model is made to call the single `structured_answer` tool.
    async fn structured(&self, system: &str, prompt: &str, schema: &Value) -> Result<Value> {
        let body = match self.api {
            Api::Anthropic => json!({
                "model":self.model,"max_tokens":self.max_tokens.min(8192),"system":system,
                "messages":[{"role":"user","content":prompt}],
                "tools":[{"name":"structured_answer","description":"Return the requested structured answer.","input_schema":schema}],
                "tool_choice":{"type":"tool","name":"structured_answer"},
            }),
            Api::OpenAi => json!({
                "model":self.model,"max_tokens":self.max_tokens.min(8192),
                "messages":[{"role":"system","content":system},{"role":"user","content":prompt}],
                "tools":[{"type":"function","function":{"name":"structured_answer","description":"Return the requested structured answer.","parameters":schema}}],
                "tool_choice":{"type":"function","function":{"name":"structured_answer"}},
            }),
        };
        let value: Value = self
            .send(&body)
            .await?
            .json()
            .await
            .context("模型返回了无效 JSON")?;
        let message = match self.api {
            Api::Anthropic => from_anthropic_message(value)?,
            Api::OpenAi => from_openai_response(&value)?,
        };
        for block in message["content"].as_array().into_iter().flatten() {
            if block["type"] == "toolCall" && block["name"] == "structured_answer" {
                ensure!(block["arguments"].is_object(), "结构化回答必须为对象");
                return Ok(block["arguments"].clone());
            }
            // Some compatible services ignore tool_choice and answer in text.
            if block["type"] == "text"
                && let Some(text) = block["text"].as_str()
                && let Some(found) = json_object_in(text)
            {
                return Ok(found);
            }
        }
        bail!("模型没有返回 structured_answer")
    }

    /// One streamed answer. `turn` is this turn's context (lessons, prepared skills,
    /// delegation data), sent after the stable system prompt so the prefix stays cacheable.
    async fn stream(&self, context: &AgentContext, turn: &str, emit: EventSink) -> Result<Value> {
        let mut response = match self.api {
            Api::Anthropic => {
                self.send_built(|caching| self.anthropic_body(&context.system_prompt, turn, &context.messages, &context.tools, caching))
                    .await?
            }
            Api::OpenAi => {
                // OpenAI-compatible services cache by prefix: stable system first, turn context after.
                let system = if turn.is_empty() { context.system_prompt.clone() } else { format!("{}\n\n{turn}", context.system_prompt) };
                let mut body = json!({"model":self.model,"max_tokens":self.max_tokens,"messages":to_openai_messages(&system,&context.messages)?,"stream":true,"stream_options":{"include_usage":true}});
                if !context.tools.is_empty() {
                    body["tools"] = json!(context.tools.iter().map(|t| json!({"type":"function","function":{"name":t.name,"description":t.description,"parameters":t.parameters}})).collect::<Vec<_>>());
                }
                self.apply_effort(&mut body)?;
                self.send(&body).await?
            }
        };
        emit(AgentEvent::MessageStart {
            message: json!({"role":"assistant","content":[]}),
        });
        if response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"))
        {
            // Some compatible gateways return one JSON response even for stream=true.
            let value: Value = response.json().await.context("无效模型 JSON")?;
            return match self.api {
                Api::Anthropic => from_anthropic_message(value),
                Api::OpenAi => from_openai_response(&value),
            };
        }
        let mut anthropic = AnthropicStream::default();
        let mut openai = OpenAiStream::default();
        while let Some(bytes) = response
            .chunk()
            .await
            .context("模型流中断：stream terminated")?
        {
            let events = match self.api {
                Api::Anthropic => anthropic.push(&bytes)?,
                Api::OpenAi => openai.sse.push(&bytes)?,
            };
            for event in events {
                let delta = match self.api {
                    Api::Anthropic => anthropic.apply(event)?,
                    Api::OpenAi => openai.apply(event)?,
                };
                if let Some(delta) = delta {
                    emit(AgentEvent::MessageUpdate {
                        message: json!({"role":"assistant","content":[]}),
                        assistant_message_event: json!({"type":"text_delta","delta":delta}),
                    });
                }
            }
            if self.api == Api::Anthropic && anthropic.done {
                return from_anthropic_message(anthropic.message);
            }
            if self.api == Api::OpenAi && openai.done {
                return openai.finish();
            }
        }
        // OpenAI-compatible servers may close without [DONE] once a finish reason came.
        if self.api == Api::OpenAi && openai.finish_reason.is_some() {
            return openai.finish();
        }
        bail!("模型流未正常结束：stream terminated；未自动重试")
    }
}

/// A 400 about the request's shape (block-form system, cache_control, unknown fields), as a
/// compatible service without prompt caching answers it; other refusals are not retried.
fn refuses_cached_form(error: &str) -> bool {
    let lower = error.to_lowercase();
    lower.starts_with("api error: 400")
        && !pi_context::overflow(&lower)
        && ["cache_control", "system", "extra inputs", "extra_forbidden", "unknown field", "unknown variant", "unexpected", "invalid type", "not permitted"].iter().any(|k| lower.contains(k))
}

/// The first JSON object in a text answer, fenced or bare.
fn json_object_in(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str::<Value>(text.get(start..=end)?)
        .ok()
        .filter(Value::is_object)
}

fn to_anthropic_content(block: &Value) -> Value {
    match block["type"].as_str() {
        Some("toolCall") => {
            json!({"type":"tool_use","id":block["id"],"name":block["name"],"input":block["arguments"]})
        }
        Some("image" | "document") if block.get("source").is_none() => {
            json!({"type":block["type"],"source":{"type":"base64","media_type":block["mimeType"],"data":block["data"]}})
        }
        _ => block.clone(),
    }
}

fn to_anthropic_messages(messages: &[Value]) -> Result<Vec<Value>> {
    let mut wire: Vec<Value> = vec![];
    for message in messages {
        if message["role"] == "toolResult" {
            let content: Vec<Value> = message["content"]
                .as_array()
                .context("无效工具内容")?
                .iter()
                .map(to_anthropic_content)
                .collect();
            let block = json!({"type":"tool_result","tool_use_id":message["toolCallId"],"content":content,"is_error":message["isError"]});
            if let Some(last) = wire.last_mut().filter(|m| {
                m["role"] == "user"
                    && m["content"]
                        .as_array()
                        .is_some_and(|b| b.iter().all(|b| b["type"] == "tool_result"))
            }) {
                last["content"].as_array_mut().unwrap().push(block);
            } else {
                wire.push(json!({"role":"user","content":[block]}));
            }
        } else {
            let (role, content): (&str, Vec<Value>) = match message["role"].as_str() {
                Some(pi_context::SUMMARY_ROLE) => ("user", vec![json!({"type":"text","text":pi_context::summary_text(message)})]),
                Some(role @ ("user" | "assistant")) => (role, message["content"].as_array().context("无效消息内容")?.iter().map(to_anthropic_content).collect()),
                _ => bail!("尚未配置自定义消息到模型消息的转换"),
            };
            // Consecutive user messages (a summary, then the next request) go as one.
            match wire.last_mut().filter(|m| role == "user" && m["role"] == "user") {
                Some(last) => last["content"].as_array_mut().unwrap().extend(content),
                None => wire.push(json!({"role":role,"content":content})),
            }
        }
    }
    Ok(wire)
}

fn from_anthropic_message(message: Value) -> Result<Value> {
    let content: Vec<Value> = message["content"].as_array().context("模型响应缺少 content")?.iter().map(|block| {
        if block["type"] == "tool_use" {json!({"type":"toolCall","id":block["id"],"name":block["name"],"arguments":block["input"]})} else {block.clone()}
    }).collect();
    let stop = match message["stop_reason"].as_str() {
        Some("end_turn" | "stop_sequence") => "stop",
        Some("tool_use") => "toolUse",
        Some("max_tokens") => "length",
        None => "streaming",
        _ => "error",
    };
    Ok(json!({"role":"assistant","content":content,"stopReason":stop,"usage":message["usage"]}))
}

/// Failures worth one more identical request: the connection, the stream or the gateway
/// broke, not the request itself (bad key, unknown model, quota, safety refusal).
fn transient_request_error(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["connection reset", "stream terminated", "peer closed", "operation timed out", "api error: 500", "api error: 502", "api error: 503", "api error: 504", "api error: 529", "overloaded"].iter().any(|s| lower.contains(s))
}

/// Server-sent events split at byte boundaries; yields each complete `data:` payload as JSON.
#[derive(Default)]
struct Sse {
    buffer: Vec<u8>,
    data: Vec<String>,
    high_surrogate: Option<u16>,
}
impl Sse {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        self.buffer.extend_from_slice(bytes);
        ensure!(self.buffer.len() <= 8 * 1024 * 1024, "模型流帧过大");
        let mut events = vec![];
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = std::str::from_utf8(&line)?.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = self.data.join("\n");
                    self.data.clear();
                    // OpenAI-style streams end with a literal [DONE] sentinel.
                    if data != "[DONE]" {
                        let event = match serde_json::from_str(&data) {
                            Ok(event) => event,
                            Err(_) => serde_json::from_str(&self.repair_surrogates(&data))
                                .context("模型 SSE 数据无效")?,
                        };
                        events.push(event);
                    } else {
                        events.push(json!({"type":"[DONE]"}));
                    }
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).into());
            }
        }
        Ok(events)
    }

    /// Some gateways split an emoji's UTF-16 surrogate pair across two events. Holds a high
    /// surrogate that ends a string until the next event's low surrogate, and turns any
    /// surrogate that still can't be paired into U+FFFD.
    fn repair_surrogates(&mut self, data: &str) -> String {
        fn escape(s: &str, i: usize) -> Option<u16> {
            let hex = s.get(i..i + 6)?.strip_prefix("\\u")?;
            u16::from_str_radix(hex, 16).ok()
        }
        let mut out = String::with_capacity(data.len());
        let mut i = 0;
        while i < data.len() {
            let Some(code) = escape(data, i) else {
                let c = data[i..].chars().next().unwrap();
                // Copy other escapes whole so `\\u` is never read as a unicode escape.
                let len = if c == '\\' { 1 + data[i + 1..].chars().next().map_or(0, char::len_utf8) } else { c.len_utf8() };
                out.push_str(&data[i..i + len]);
                i += len;
                continue;
            };
            let next = escape(data, i + 6);
            match code {
                0xD800..=0xDBFF if next.is_some_and(|n| (0xDC00..=0xDFFF).contains(&n)) => {
                    out.push_str(&data[i..i + 12]);
                    i += 12;
                    continue;
                }
                0xD800..=0xDBFF if data[i + 6..].starts_with('"') => self.high_surrogate = Some(code),
                0xD800..=0xDBFF => out.push_str("\\ufffd"),
                0xDC00..=0xDFFF => match self.high_surrogate.take() {
                    Some(high) => out.push_str(&format!("\\u{high:04x}\\u{code:04x}")),
                    None => out.push_str("\\ufffd"),
                },
                _ => out.push_str(&data[i..i + 6]),
            }
            i += 6;
        }
        out
    }
}

#[derive(Default)]
struct AnthropicStream {
    sse: Sse,
    message: Value,
    arguments: BTreeMap<usize, String>,
    done: bool,
}
impl AnthropicStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>> {
        self.sse.push(bytes)
    }
    fn apply(&mut self, event: Value) -> Result<Option<String>> {
        match event["type"].as_str() {
            Some("message_start") => {
                self.message = event["message"].clone();
                ensure!(self.message["content"].is_array(), "无效 message_start");
            }
            Some("content_block_start") => {
                let index = event["index"].as_u64().context("缺少块索引")? as usize;
                let blocks = self.message["content"]
                    .as_array_mut()
                    .context("缺少 message_start")?;
                ensure!(index == blocks.len(), "模型块顺序无效");
                blocks.push(event["content_block"].clone());
            }
            Some("content_block_delta") => {
                let index = event["index"].as_u64().context("缺少块索引")? as usize;
                let block = self.message["content"]
                    .get_mut(index)
                    .context("无效块索引")?;
                let (field, delta) = match event["delta"]["type"].as_str() {
                    Some("text_delta") => (
                        "text",
                        event["delta"]["text"].as_str().context("无效文本增量")?,
                    ),
                    Some("thinking_delta") => (
                        "thinking",
                        event["delta"]["thinking"]
                            .as_str()
                            .context("无效思考增量")?,
                    ),
                    Some("signature_delta") => (
                        "signature",
                        event["delta"]["signature"]
                            .as_str()
                            .context("无效签名增量")?,
                    ),
                    Some("input_json_delta") => {
                        self.arguments.entry(index).or_default().push_str(
                            event["delta"]["partial_json"]
                                .as_str()
                                .context("无效参数增量")?,
                        );
                        return Ok(None);
                    }
                    _ => return Ok(None),
                };
                let value = format!("{}{delta}", block[field].as_str().unwrap_or_default());
                block[field] = json!(value);
                if field == "text" {
                    return Ok(Some(delta.to_string()));
                }
            }
            Some("content_block_stop") => {
                let index = event["index"].as_u64().context("缺少块索引")? as usize;
                if let Some(arguments) = self.arguments.remove(&index) {
                    self.message["content"][index]["input"] = serde_json::from_str(&arguments)
                        .context("工具参数 JSON 不完整；未执行工具")?;
                }
            }
            Some("message_delta") => {
                self.message["stop_reason"] = event["delta"]["stop_reason"].clone();
                if let Some(usage) = event["usage"].as_object() {
                    if !self.message["usage"].is_object() {
                        self.message["usage"] = json!({});
                    }
                    self.message["usage"]
                        .as_object_mut()
                        .unwrap()
                        .extend(usage.clone());
                }
            }
            Some("message_stop") => {
                ensure!(self.arguments.is_empty(), "工具参数流未完成");
                self.done = true;
            }
            Some("error") => bail!(
                "API Error: stream error {}: {}",
                event["error"]["type"].as_str().unwrap_or("unknown"),
                event["error"]["message"].as_str().unwrap_or("模型服务报告流错误")
            ),
            _ => {}
        }
        Ok(None)
    }
}


fn data_url(block: &Value) -> String {
    format!(
        "data:{};base64,{}",
        block["mimeType"].as_str().unwrap_or("application/octet-stream"),
        block["data"].as_str().unwrap_or_default()
    )
}

/// pi messages → OpenAI Chat Completions. Tool results cannot carry pictures there, so
/// any images they return follow as one user message after that batch of results.
fn to_openai_messages(system: &str, messages: &[Value]) -> Result<Vec<Value>> {
    let mut wire = vec![json!({"role":"system","content":system})];
    let mut pending_images: Vec<Value> = vec![];
    let flush = |wire: &mut Vec<Value>, pending: &mut Vec<Value>| {
        if !pending.is_empty() {
            let mut parts = vec![json!({"type":"text","text":"工具返回的图片："})];
            parts.append(pending);
            wire.push(json!({"role":"user","content":parts}));
        }
    };
    for message in messages {
        let empty = vec![];
        let blocks = match message["role"].as_str() {
            Some(pi_context::SUMMARY_ROLE) => &empty,
            _ => message["content"].as_array().context("无效消息内容")?,
        };
        match message["role"].as_str() {
            Some("toolResult") => {
                let text = blocks
                    .iter()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                for block in blocks.iter().filter(|b| b["type"] == "image") {
                    pending_images.push(json!({"type":"image_url","image_url":{"url":data_url(block)}}));
                }
                let text = if message["isError"] == true && !text.starts_with("Error") {
                    format!("Error: {text}")
                } else {
                    text
                };
                wire.push(json!({"role":"tool","tool_call_id":message["toolCallId"],"content":text}));
            }
            Some("user") => {
                flush(&mut wire, &mut pending_images);
                let parts: Vec<Value> = blocks
                    .iter()
                    .map(|b| match b["type"].as_str() {
                        Some("image") => json!({"type":"image_url","image_url":{"url":data_url(b)}}),
                        Some("document") => json!({"type":"file","file":{"filename":"document.pdf","file_data":data_url(b)}}),
                        _ => json!({"type":"text","text":b["text"].as_str().unwrap_or_default()}),
                    })
                    .collect();
                // Plain text as a string: the form every compatible server accepts.
                let content = if parts.iter().all(|p| p["type"] == "text") {
                    json!(parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"))
                } else {
                    json!(parts)
                };
                wire.push(json!({"role":"user","content":content}));
            }
            Some("assistant") => {
                flush(&mut wire, &mut pending_images);
                let text = blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let reasoning = blocks
                    .iter()
                    .filter(|b| b["type"] == "thinking")
                    .filter_map(|b| b["thinking"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let calls: Vec<Value> = blocks
                    .iter()
                    .filter(|b| b["type"] == "toolCall")
                    .map(|b| json!({"id":b["id"],"type":"function","function":{"name":b["name"],"arguments":b["arguments"].to_string()}}))
                    .collect();
                let mut out = json!({"role":"assistant","content":if text.is_empty() && !calls.is_empty() {Value::Null} else {json!(text)}});
                if !calls.is_empty() {
                    out["tool_calls"] = json!(calls);
                }
                // Reasoning models (DeepSeek and others) expect their reasoning back within a tool loop.
                if !reasoning.is_empty() {
                    out["reasoning_content"] = json!(reasoning);
                }
                wire.push(out);
            }
            Some(pi_context::SUMMARY_ROLE) => {
                flush(&mut wire, &mut pending_images);
                wire.push(json!({"role":"user","content":pi_context::summary_text(message)}));
            }
            _ => bail!("尚未配置自定义消息到模型消息的转换"),
        }
    }
    flush(&mut wire, &mut pending_images);
    Ok(wire)
}

/// A complete OpenAI-style assistant turn in pi's message form.
fn openai_message(
    text: &str,
    reasoning: &str,
    calls: &[(String, String, String)],
    finish: Option<&str>,
    usage: &Value,
) -> Result<Value> {
    let mut content = vec![];
    if !reasoning.is_empty() {
        content.push(json!({"type":"thinking","thinking":reasoning}));
    }
    if !text.is_empty() {
        content.push(json!({"type":"text","text":text}));
    }
    for (id, name, arguments) in calls {
        let arguments: Value = if arguments.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str(arguments).context("工具参数 JSON 不完整；未执行工具")?
        };
        content.push(json!({"type":"toolCall","id":id,"name":name,"arguments":arguments}));
    }
    let stop = match finish {
        _ if !calls.is_empty() && finish != Some("length") => "toolUse",
        Some("stop") | None => "stop",
        Some("tool_calls" | "function_call") => "toolUse",
        Some("length") => "length",
        Some(_) => "error",
    };
    let mut message = json!({"role":"assistant","content":content,"stopReason":stop,"usage":usage});
    if stop == "error" {
        message["errorMessage"] = json!(format!("模型停止：{}", finish.unwrap_or_default()));
    }
    Ok(message)
}

fn from_openai_response(value: &Value) -> Result<Value> {
    let choice = &value["choices"][0];
    ensure!(choice.is_object(), "模型响应缺少 choices");
    let message = &choice["message"];
    let calls: Vec<(String, String, String)> = message["tool_calls"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            (
                c["id"].as_str().unwrap_or_default().to_string(),
                c["function"]["name"].as_str().unwrap_or_default().to_string(),
                c["function"]["arguments"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    openai_message(
        message["content"].as_str().unwrap_or_default(),
        message["reasoning_content"]
            .as_str()
            .or(message["reasoning"].as_str())
            .unwrap_or_default(),
        &calls,
        choice["finish_reason"].as_str(),
        &value["usage"],
    )
}

#[derive(Default)]
struct OpenAiStream {
    sse: Sse,
    text: String,
    reasoning: String,
    calls: BTreeMap<u64, (String, String, String)>,
    finish_reason: Option<String>,
    usage: Value,
    done: bool,
}
impl OpenAiStream {
    fn apply(&mut self, event: Value) -> Result<Option<String>> {
        if event["type"] == "[DONE]" {
            self.done = true;
            return Ok(None);
        }
        if event["error"].is_object() {
            bail!(
                "API Error: stream error: {}",
                event["error"]["message"].as_str().unwrap_or("模型服务报告流错误")
            );
        }
        if event["usage"].is_object() {
            self.usage = event["usage"].clone();
        }
        let Some(choice) = event["choices"].get(0) else {
            return Ok(None);
        };
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.into());
        }
        let delta = &choice["delta"];
        if let Some(reasoning) = delta["reasoning_content"]
            .as_str()
            .or(delta["reasoning"].as_str())
        {
            self.reasoning.push_str(reasoning);
        }
        for (position, call) in delta["tool_calls"].as_array().into_iter().flatten().enumerate() {
            let index = call["index"].as_u64().unwrap_or(position as u64);
            let entry = self.calls.entry(index).or_default();
            if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                entry.0 = id.into();
            }
            if let Some(name) = call["function"]["name"].as_str() {
                entry.1.push_str(name);
            }
            if let Some(arguments) = call["function"]["arguments"].as_str() {
                entry.2.push_str(arguments);
            }
        }
        match delta["content"].as_str().filter(|t| !t.is_empty()) {
            Some(text) => {
                self.text.push_str(text);
                Ok(Some(text.into()))
            }
            None => Ok(None),
        }
    }
    fn finish(self) -> Result<Value> {
        let calls: Vec<_> = self
            .calls
            .into_values()
            .enumerate()
            .map(|(n, (id, name, arguments))| {
                let id = if id.is_empty() { format!("call_{n}") } else { id };
                (id, name, arguments)
            })
            .collect();
        openai_message(
            &self.text,
            &self.reasoning,
            &calls,
            self.finish_reason.as_deref(),
            &self.usage,
        )
    }
}

pub async fn ask(
    env: &[(String, String)],
    model: Option<&str>,
    system: &str,
    prompt: &str,
    schema: &Value,
) -> Result<Value> {
    Model::new(env, model)?
        .structured(system, prompt, schema)
        .await
}

pub fn start(spec: TurnSpec) -> Result<Turn> {
    let mut model = Model::new(&spec.env, spec.model.as_deref())?;
    model.effort = validate_effort(spec.effort.as_deref())?;
    if std::env::var("DOTBOT_TEST_MODEL_HEADERS").as_deref() == Ok("1") {
        let set: Vec<&str> = spec.env.iter().filter(|(k, v)| !v.is_empty() && !k.starts_with("DOTBOT_MODEL_")).map(|(k, _)| k.as_str()).collect();
        model.trace = vec![
            ("x-dotbot-conversation".into(), spec.mcp["mcpServers"]["teambot"]["env"]["DOTBOT_CONVERSATION"].as_str().unwrap_or_default().into()),
            ("x-dotbot-mcp".into(), urlencoding::encode(&spec.mcp.to_string()).into_owned()),
            ("x-dotbot-cwd".into(), urlencoding::encode(&spec.cwd.display().to_string()).into_owned()),
            ("x-dotbot-plugin-dir".into(), spec.plugin_dir.as_ref().map(|d| urlencoding::encode(&d.display().to_string()).into_owned()).unwrap_or_default()),
            ("x-dotbot-env".into(), set.join(",")),
        ];
    }
    let env = spec.mcp["mcpServers"]["teambot"]["env"].clone();
    ensure!(env.is_object(), "缺少团队工具配置");
    let dir = spec.session_dir.clone();
    let id = match spec.session.as_deref() {
        // Sessions from the former Claude Code engine or lost files start over: the turn
        // sees "No conversation found" and rebuilds its context on a fresh session.
        Some(id)
            if !(id.starts_with("pi-") && uuid::Uuid::parse_str(&id[3..]).is_ok())
                || !dir.join(format!("{id}.json")).exists() =>
        {
            bail!("No conversation found with session ID {id}")
        }
        Some(id) => id.to_string(),
        None => format!("pi-{}", uuid::Uuid::new_v4()),
    };
    if !model.trace.is_empty() {
        let flag = if spec.session.is_some() { "x-dotbot-resume" } else { "x-dotbot-session" };
        model.trace.push((flag.into(), id.clone()));
    }
    let path = dir.join(format!("{id}.json"));
    let messages: Vec<Value> = if spec.session.is_some() {
        serde_json::from_slice(&std::fs::read(&path).context("读取会话失败")?)?
    } else {
        vec![]
    };
    std::fs::create_dir_all(&dir)?;
    crate::durable::save(&path, &messages)?;
    let (tx, events) = tokio::sync::mpsc::unbounded_channel();
    let (stop, mut cancelled) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(Event::Session(id));
        let control = AgentControl::default();
        let running = run(spec, model, env, path, messages, &tx, &control);
        tokio::pin!(running);
        let result = tokio::select! {
            _ = &mut cancelled => { control.abort(); running.await }
            result = &mut running => result,
        };
        let (text, is_error) = match result {
            Ok(text) => (text, false),
            Err(e) => (format!("{e:#}"), true),
        };
        let _ = tx.send(Event::Done { text, is_error });
    });
    Ok(Turn { events, stop })
}

struct DotbotRuntime {
    model: Model,
    env: Value,
    path: PathBuf,
    tool_client: reqwest::Client,
    mcp: crate::agent_mcp::Registry,
    /// Skill name → its folder, from the turn's skill bundle.
    skills: BTreeMap<String, PathBuf>,
    /// This turn's context, sent after the stable system prompt and never stored.
    turn_context: String,
    window: Window,
    events: tokio::sync::mpsc::UnboundedSender<Event>,
}

impl DotbotRuntime {
    /// Folds the older part of the session into one summary message. The session file takes
    /// it at the next completed batch, so a failed turn leaves the stored session unchanged.
    async fn compact_now(&self, context: &mut AgentContext, force: bool) -> Result<bool> {
        pi_context::drop_old_images(&mut context.messages);
        let extra = pi_context::text_tokens(&self.turn_context);
        let before = pi_context::context_tokens(context, extra);
        if !force && before <= self.window.threshold() {
            return Ok(false);
        }
        let mut keep = self.window.keep_recent;
        let cut = loop {
            match pi_context::cut_point(&context.messages, keep) {
                Some(cut) => break cut,
                None if force && keep > 1_000 => keep /= 4,
                None => return Ok(false),
            }
        };
        let _ = self.events.send(Event::Status("compacting".into()));
        let older = &context.messages[..cut];
        let input = pi_context::transcript(older, self.window.context / 2);
        let summary = match self.model.complete(pi_context::SUMMARY_SYSTEM, &input, 4_096).await {
            Ok(summary) if !summary.trim().is_empty() => summary,
            outcome => {
                // Without a summary the older part still goes; keep what an earlier one said.
                if let Err(error) = outcome {
                    eprintln!("dotbot: context summary unavailable: {error:#}");
                }
                let previous = older.iter().find(|m| m["role"] == pi_context::SUMMARY_ROLE).and_then(|m| m["summary"].as_str()).unwrap_or_default();
                format!("{previous}\n\n（较早的 {cut} 条消息未能整理成摘要，已从上下文中移除。）").trim().to_string()
            }
        };
        let mut kept = context.messages[cut..].to_vec();
        pi_context::forget_usage(&mut kept);
        context.messages = std::iter::once(pi_context::summary_message(&summary, before, cut)).chain(kept).collect();
        let _ = self.events.send(Event::Status("thinking".into()));
        Ok(true)
    }

    async fn team_tool(&self, name: &str, args: &Value) -> Result<Value> {
        let response = self
            .tool_client
            .post(format!(
                "{}/internal/tool",
                self.env["DOTBOT_URL"].as_str().context("缺少服务地址")?
            ))
            .bearer_auth(self.env["DOTBOT_TOKEN"].as_str().context("缺少服务凭据")?)
            .json(&json!({"conversation":self.env["DOTBOT_CONVERSATION"],"name":name,"args":args}))
            .send()
            .await
            .context("工具执行结果未知，请检查执行记录后再恢复")?;
        ensure!(
            response.status().is_success(),
            "工具服务返回 HTTP {}；执行结果可能未知",
            response.status()
        );
        response.json().await.context("工具结果无效；请检查执行记录")
    }
}

impl Runtime for DotbotRuntime {
    fn stream<'a>(
        &'a self,
        context: &'a AgentContext,
        emit: EventSink,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            // A dropped connection, a cut-off stream or a 5xx gateway reply is retried here, up to
            // twice, as long as nothing reached the user yet: a model request has no side effects,
            // and tools only run after a complete answer. Text already shown is never doubled.
            let mut attempt = 0;
            loop {
                let shown = Arc::new(AtomicBool::new(false));
                let (outer, seen) = (emit.clone(), shown.clone());
                let sink: EventSink = Arc::new(move |event| {
                    if matches!(event, AgentEvent::MessageUpdate { .. }) { seen.store(true, Ordering::Relaxed); }
                    outer(event)
                });
                match self.model.stream(context, &self.turn_context, sink).await {
                    Err(error) if attempt < 2 && !shown.load(Ordering::Relaxed) && transient_request_error(&format!("{error:#}")) => {
                        attempt += 1;
                        eprintln!("dotbot: model request retry {attempt}: {}", format!("{error:#}").chars().take(160).collect::<String>());
                        tokio::time::sleep(std::time::Duration::from_millis(1500 * attempt)).await;
                    }
                    result => return result,
                }
            }
        })
    }
    fn transform_context<'a>(
        &'a self,
        context: &'a AgentContext,
    ) -> BoxFuture<'a, Result<AgentContext>> {
        Box::pin(async move {
            let extra = pi_context::text_tokens(&self.turn_context);
            Ok(pi_context::for_request(context, self.window.threshold(), extra))
        })
    }
    fn compact<'a>(&'a self, context: &'a mut AgentContext, force: bool) -> BoxFuture<'a, Result<bool>> {
        Box::pin(self.compact_now(context, force))
    }
    fn context_overflow(&self, error: &anyhow::Error) -> bool {
        pi_context::overflow(&format!("{error:#}"))
    }
    /// Connection tools pass the Bot's permission policy, the same gate the team's own
    /// risky tools use. The standalone browser enforces its policy at its own endpoint.
    fn before_tool_call<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<Option<ToolResult>>> {
        Box::pin(async move {
            if call.name == SKILL_TOOL
                || call.name == crate::web_tools::SEARCH
                || call.name == crate::web_tools::FETCH
                || call.name.starts_with(TEAM_PREFIX)
                || self.mcp.server(&call.name) == Some("browser")
            {
                return Ok(None);
            }
            let result = self
                .team_tool("approve", &json!({"tool_name":call.name,"input":call.arguments}))
                .await
                .context("审批请求失败；工具未执行")?;
            let answer: Value = serde_json::from_str(result["text"].as_str().unwrap_or_default())
                .unwrap_or_default();
            if answer["behavior"] == "allow" {
                Ok(None)
            } else {
                Ok(Some(ToolResult::error(
                    answer["message"]
                        .as_str()
                        .or(result["text"].as_str())
                        .unwrap_or("The user did not allow this."),
                )))
            }
        })
    }
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _emit: EventSink,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            if call.name == SKILL_TOOL {
                return Ok(load_skill(&self.skills, &call.arguments));
            }
            if call.name == crate::web_tools::SEARCH || call.name == crate::web_tools::FETCH {
                return Ok(crate::web_tools::run(&call.name, &call.arguments).await);
            }
            if let Some(name) = call.name.strip_prefix(TEAM_PREFIX) {
                let result = self.team_tool(name, &call.arguments).await?;
                let text = result["text"].as_str().context("工具结果缺少文本")?;
                let mut content = vec![json!({"type":"text","text":text})];
                for image in result["images"].as_array().into_iter().flatten() {
                    content.push(json!({"type":"image","mimeType":image["mime"],"data":image["data"]}));
                }
                return Ok(ToolResult {
                    content,
                    is_error: result["isError"].as_bool().unwrap_or(false),
                    details: Value::Null,
                    // One handoff may be only the first of several independent
                    // assignments. Let the model finish dispatching this batch and
                    // end its turn normally; never cut off the remaining recipients.
                    // The team prompt requires yielding after dispatch, not waiting
                    // for collaborator results or polling them in this turn.
                    terminate: false,
                });
            }
            let result = match self.mcp.call(&call.name, &call.arguments).await? {
                Ok(result) => result,
                Err(rejected) => return Ok(ToolResult::error(rejected)),
            };
            Ok(mcp_result(&result))
        })
    }
    fn finish_turn<'a>(&'a self, context: &'a AgentContext) -> BoxFuture<'a, Result<TurnDecision>> {
        Box::pin(async move {
            // Only complete assistant/tool-result batches reach this hook.
            crate::durable::save(&self.path, &context.messages)?;
            Ok(TurnDecision::Auto)
        })
    }
}

/// An MCP tools/call result in pi's content form: text, images, and other resources as text.
fn mcp_result(result: &Value) -> ToolResult {
    let mut content: Vec<Value> = result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|block| match block["type"].as_str() {
            Some("image") => json!({"type":"image","mimeType":block["mimeType"],"data":block["data"]}),
            Some("text") => json!({"type":"text","text":block["text"]}),
            Some("resource") => json!({"type":"text","text":block["resource"]["text"].as_str().map(str::to_string).unwrap_or_else(|| block["resource"].to_string())}),
            _ => json!({"type":"text","text":block.to_string()}),
        })
        .collect();
    for block in content.iter_mut() {
        if let Some(text) = block["text"].as_str() && text.chars().count() > TOOL_TEXT_MAX {
            let cut: String = text.chars().take(TOOL_TEXT_MAX).collect();
            block["text"] = json!(format!("{cut}\n…[输出过长，已截断为前 {TOOL_TEXT_MAX} 字]"));
        }
    }
    if content.is_empty() {
        let text = if result["structuredContent"].is_null() {
            "(no output)".to_string()
        } else {
            result["structuredContent"].to_string()
        };
        content.push(json!({"type":"text","text":text}));
    }
    ToolResult {
        content,
        is_error: result["isError"] == true,
        details: Value::Null,
        terminate: false,
    }
}

/// The enabled skills in a bundle (`<dir>/skills/<name>/SKILL.md`): name, description, folder.
fn skill_index(plugin_dir: &std::path::Path) -> Vec<(String, String, PathBuf)> {
    let mut skills: Vec<_> = std::fs::read_dir(plugin_dir.join("skills"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let text = crate::sqlite_storage::read_to_string(e.path().join("SKILL.md")).ok()?;
            let (_, description) = crate::skills::front_matter(&text);
            Some((e.file_name().into_string().ok()?, description, e.path()))
        })
        .collect();
    skills.sort_by(|a, b| a.0.cmp(&b.0));
    skills.truncate(SKILL_INDEX_MAX);
    skills
}

fn load_skill(skills: &BTreeMap<String, PathBuf>, args: &Value) -> ToolResult {
    let name = args["skill"].as_str().unwrap_or_default().trim();
    let name = name.rsplit(':').next().unwrap_or(name);
    let Some(dir) = skills.get(name) else {
        return ToolResult::error(format!(
            "No skill named {name}. Available: {}",
            skills.keys().cloned().collect::<Vec<_>>().join(", ")
        ));
    };
    match crate::sqlite_storage::read_to_string(dir.join("SKILL.md")) {
        Ok(text) => ToolResult {
            content: vec![json!({"type":"text","text":format!(
                "Base directory for this skill: {}\n\n{}{}",
                dir.display(),
                crate::skills::body_of(&text),
                args["args"].as_str().filter(|a| !a.trim().is_empty()).map(|a| format!("\n\nARGUMENTS: {a}")).unwrap_or_default()
            )})],
            is_error: false,
            details: Value::Null,
            terminate: false,
        },
        Err(error) => ToolResult::error(format!("Skill {name} is unreadable: {error}")),
    }
}

async fn run(
    mut spec: TurnSpec,
    model: Model,
    env: Value,
    path: PathBuf,
    messages: Vec<Value>,
    tx: &tokio::sync::mpsc::UnboundedSender<Event>,
    control: &AgentControl,
) -> Result<String> {
    let mut definitions = crate::tools::definitions();
    // The permission gate is called by this runtime, never by the model.
    definitions.retain(|d| d["name"] != "approve");
    if env["DOTBOT_DESKTOP"] == "1" {
        definitions.extend(crate::tools::desktop_definitions());
    }
    if env["DOTBOT_LOCAL"] == "1" {
        definitions.extend(crate::tools::local_definitions());
    }
    definitions.extend(crate::tools::media_definitions(
        env["DOTBOT_MEDIA"].as_str().unwrap_or_default(),
    ));
    let mut tools: Vec<Tool> = definitions
        .iter()
        .map(|d| Tool {
            name: format!("{TEAM_PREFIX}{}", d["name"].as_str().unwrap_or_default()),
            description: d["description"].as_str().unwrap_or_default().into(),
            parameters: d["inputSchema"].clone(),
            sequential: true,
        })
        .collect();
    tools.extend(crate::web_tools::definitions());
    let mut turn_context = std::mem::take(&mut spec.turn_context);
    let mut mcp = crate::agent_mcp::Registry::empty();
    let failed = tokio::select! {
        failed = mcp.connect(&spec.mcp, &spec.cwd) => failed,
        _ = wait_abort(control) => { mcp.close().await; bail!("Operation aborted") }
    };
    if !failed.is_empty() {
        turn_context.push_str("\n\n# 本轮不可用的 MCP 服务\n下列服务启动失败，它们的工具本轮不可用。需要时如实告诉用户，可在「设置 → 插件 → 已安装」测试或修改该连接：");
        for (server, error) in &failed {
            eprintln!("dotbot: MCP server {server} unavailable: {error}");
            turn_context.push_str(&format!("\n- {server}：{}", error.chars().take(200).collect::<String>()));
        }
    }
    tools.extend(mcp.tools.iter().cloned());
    let index = spec.plugin_dir.as_deref().map(skill_index).unwrap_or_default();
    if !index.is_empty() {
        spec.system_prompt.push_str("\n\n# 可用技能\n下列技能是团队安装的工作流程。任务与某个技能的说明相符时，先用 Skill 工具按名称加载它的完整流程，再按流程执行；加载后引用的脚本和文档位于返回的技能目录，用 read_skill_resource 或已允许的终端工具读取。技能是方法，不授予新权限。");
        for (name, description, _) in &index {
            spec.system_prompt.push_str(&format!("\n- {name}：{}", description.chars().take(300).collect::<String>()));
        }
        tools.push(Tool {
            name: SKILL_TOOL.into(),
            description: "Load a skill's full workflow by name before doing a task it describes. Returns the instructions and the skill's base directory.".into(),
            parameters: json!({"type":"object","properties":{"skill":{"type":"string","description":"The skill name from the list of available skills"},"args":{"type":"string","description":"Optional arguments for the skill"}},"required":["skill"]}),
            sequential: true,
        });
    }
    let skills = index.into_iter().map(|(name, _, dir)| (name, dir)).collect();
    let mut content: Vec<Value> = spec.images.iter().map(|(mime,data)|json!({"type":if mime == "application/pdf" {"document"} else {"image"},"mimeType":mime,"data":data})).collect();
    content.push(json!({"type":"text","text":spec.prompt}));
    let mut context = AgentContext {
        system_prompt: spec.system_prompt,
        messages,
        tools,
    };
    let window = Window::new(model.context, model.max_tokens as usize);
    let runtime = DotbotRuntime {
        window,
        turn_context: turn_context.trim_start().to_string(),
        events: tx.clone(),
        model,
        env,
        path,
        tool_client: reqwest::Client::builder()
            .timeout(Duration::from_secs(3600))
            .build()?,
        mcp,
        skills,
    };
    let events = tx.clone();
    let sink: EventSink = Arc::new(move |event| match event {
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => {
            if assistant_message_event["type"] == "text_delta" {
                let _ = events.send(Event::TextDelta(
                    assistant_message_event["delta"]
                        .as_str()
                        .unwrap_or_default()
                        .into(),
                ));
            }
        }
        AgentEvent::MessageEnd { message } if message["role"] == "assistant" => {
            let text = message_text(&message);
            if !text.is_empty() {
                let _ = events.send(Event::Text(text));
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            let _ = events.send(Event::ToolUse {
                id: tool_call_id,
                name: tool_name,
                input: args,
            });
        }
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            result,
            ..
        } => {
            let text = result
                .content
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let images = result
                .content
                .iter()
                .filter(|b| b["type"] == "image")
                .filter_map(|b| Some((b["mimeType"].as_str()?.into(), b["data"].as_str()?.into())))
                .collect();
            let _ = events.send(Event::ToolResult {
                id: tool_call_id,
                text,
                images,
                is_error: result.is_error,
            });
        }
        _ => {}
    });
    let outcome = pi_agent_core::agent_loop(
        &mut context,
        vec![json!({"role":"user","content":content})],
        &Config::default(),
        &runtime,
        control,
        sink,
    )
    .await;
    runtime.mcp.close().await;
    outcome?;
    Ok(context
        .messages
        .iter()
        .rev()
        .find(|m| m["role"] == "assistant")
        .map(message_text)
        .unwrap_or_default())
}

/// Resolves once the turn is stopped, so slow MCP start-up does not hold up a stop.
async fn wait_abort(control: &AgentControl) {
    while !control.aborted() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn message_text(message: &Value) -> String {
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::{Json, Router, routing::post};
    use std::sync::{Arc, Mutex};

    struct Fixture {
        url: String,
        requests: Arc<Mutex<Vec<Value>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn fixture() -> Fixture {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let model_requests = requests.clone();
        let router = Router::new().route("/v1/messages", post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
            let requests = model_requests.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer test-token");
                requests.lock().unwrap().push(body.clone());
                if body["system"] == "wait" { std::future::pending::<()>().await; }
                let content = if body["tool_choice"]["name"] == "structured_answer" {
                    json!([{"type":"tool_use","name":"structured_answer","id":"s1","input":{"ok":true}}])
                } else if body["messages"].as_array().unwrap().len() == 1 {
                    json!([{"type":"tool_use","name":"mcp__teambot__list_bots","id":"t1","input":{}}])
                } else {
                    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "t1");
                    assert_eq!(body["messages"][2]["content"][0]["is_error"], false);
                    json!([{"type":"text","text":"完成"}])
                };
                if body["stream"] == true {
                    let mut events = vec![json!({"type":"message_start","message":{"role":"assistant","content":[],"usage":{"input_tokens":10}}})];
                    for (index,block) in content.as_array().unwrap().iter().enumerate() {
                        let mut start = block.clone();
                        if block["type"] == "text" {start["text"] = json!("");}
                        events.push(json!({"type":"content_block_start","index":index,"content_block":start}));
                        if block["type"] == "text" {events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":block["text"]}}));}
                        events.push(json!({"type":"content_block_stop","index":index}));
                    }
                    events.push(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}));
                    events.push(json!({"type":"message_stop"}));
                    let data = events.iter().map(|event|format!("data: {event}\n\n")).collect::<String>();
                    return ([("content-type","text/event-stream")],data).into_response();
                }
                Json(json!({"content":content,"stop_reason":"end_turn"})).into_response()
            }
        })).route("/internal/tool", post(|headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer internal-test");
            assert_eq!(body["conversation"], "bot:test");
            assert_eq!(body["name"], "list_bots");
            Json(json!({"text":"bots","isError":false,"images":[{"mime":"image/png","data":"AAAA"}]}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Fixture {
            url,
            requests,
            task,
        }
    }

    fn spec(home: &std::path::Path, url: &str) -> TurnSpec {
        TurnSpec {
            cwd: home.into(),
            session: None,
            session_dir: home.join("pi-sessions"),
            model: Some("test-model".into()),
            effort: None,
            system_prompt: "sys".into(),
            turn_context: String::new(),
            prompt: "hello".into(),
            mcp: json!({"mcpServers":{"teambot":{"env":{"DOTBOT_URL":url,"DOTBOT_TOKEN":"internal-test","DOTBOT_CONVERSATION":"bot:test"}}}}),
            env: vec![
                ("DOTBOT_MODEL_BASE_URL".into(), url.into()),
                ("DOTBOT_MODEL_KEY".into(), "test-token".into()),
                ("DOTBOT_MODEL_AUTH".into(), "bearer".into()),
            ],
            plugin_dir: Some(home.join("plugin")),
            images: vec![],
        }
    }

    #[test]
    fn reasoning_settings_reach_protocol_and_keep_default_unchanged() {
        let env = spec(std::path::Path::new("/tmp"), "https://example.com").env;
        let mut model = Model::new(&env, Some("test-model")).unwrap();
        let plain = model.anthropic_body("sys", "", &[], &[], false).unwrap();
        assert!(plain.get("thinking").is_none());
        assert!(validate_effort(Some("invalid")).is_err());
        assert_eq!(validate_effort(Some("  ")).unwrap(), None);
        model.effort = validate_effort(Some("high")).unwrap();
        let body = model.anthropic_body("sys", "", &[], &[], true).unwrap();
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body["thinking"]["budget_tokens"].as_u64().unwrap() < model.max_tokens);
        model.max_tokens = 1500;
        let body = model.anthropic_body("sys", "", &[], &[], false).unwrap();
        assert_eq!(body["thinking"]["budget_tokens"], 1499);
        model.max_tokens = 1024;
        assert!(model.anthropic_body("sys", "", &[], &[], false).is_err());
        model.effort = Some("none".into());
        assert_eq!(model.anthropic_body("sys", "", &[], &[], false).unwrap()["thinking"]["type"], "disabled");
        model.api = Api::OpenAi;
        for effort in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            model.effort = validate_effort(Some(effort)).unwrap();
            let mut body = json!({"max_tokens": 16000, "tools": [{"type":"function"}]});
            model.apply_effort(&mut body).unwrap();
            assert_eq!(body["reasoning_effort"], effort);
            assert_eq!(body["max_completion_tokens"], 16000);
            assert!(body.get("max_tokens").is_none());
            assert!(body.get("tools").is_some());
        }
        model.effort = None;
        let mut body = json!({"max_tokens": 100});
        model.apply_effort(&mut body).unwrap();
        assert_eq!(body, json!({"max_tokens":100}));
    }

    #[tokio::test]
    async fn native_loop_executes_tools_persists_and_resumes_without_cli() {
        let fixture = fixture().await;
        let home = tempfile::tempdir().unwrap();
        let mut configured = spec(home.path(), &fixture.url);
        configured.effort = Some("high".into());
        let mut turn = start(configured).unwrap();
        let mut session = String::new();
        let mut tool_result = false;
        let mut streamed = false;
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), turn.events.recv())
                .await
                .unwrap()
                .unwrap();
            match event {
                Event::Session(id) => session = id,
                Event::TextDelta(text) => {
                    assert_eq!(text, "完成");
                    streamed = true;
                }
                Event::ToolResult {
                    images, is_error, ..
                } => {
                    assert!(!is_error);
                    assert_eq!(images.len(), 1);
                    tool_result = true;
                }
                Event::Done { text, is_error } => {
                    assert!(!is_error, "{text}");
                    assert_eq!(text, "完成");
                    break;
                }
                _ => {}
            }
        }
        assert!(tool_result);
        assert!(streamed);
        {
            let requests = fixture.requests.lock().unwrap();
            assert!(requests.len() >= 2);
            assert!(requests.iter().all(|body| body["thinking"]["type"] == "enabled"));
        }
        let path = home
            .path()
            .join("pi-sessions")
            .join(format!("{session}.json"));
        let messages: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2]["role"], "toolResult");
        assert_eq!(messages[2]["content"][1]["mimeType"], "image/png");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut resumed = spec(home.path(), &fixture.url);
        resumed.session = Some(session);
        let mut turn = start(resumed).unwrap();
        while let Some(event) = turn.events.recv().await {
            if let Event::Done { is_error, text } = event {
                assert!(!is_error, "{text}");
                break;
            }
        }
        assert_eq!(
            fixture.requests.lock().unwrap()[2]["messages"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }

    #[tokio::test]
    async fn structured_jobs_and_stop_use_native_backend() {
        let fixture = fixture().await;
        let home = tempfile::tempdir().unwrap();
        let mut spec = spec(home.path(), &fixture.url);
        let answer = ask(
            &spec.env,
            Some("test-model"),
            "sys",
            "prompt",
            &json!({"type":"object","properties":{"ok":{"type":"boolean"}}}),
        )
        .await
        .unwrap();
        assert_eq!(answer, json!({"ok":true}));
        spec.system_prompt = "wait".into();
        let mut turn = start(spec).unwrap();
        assert!(matches!(turn.events.recv().await, Some(Event::Session(_))));
        turn.stop.send(()).unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(2), turn.events.recv())
                .await
                .unwrap(),
            Some(Event::Done { is_error: true, .. })
        ));
    }

    #[test]
    fn a_team_without_a_text_provider_gets_a_clear_error() {
        let error = Model::new(&[], Some("any")).err().unwrap().to_string();
        assert!(error.contains("还没有可用的文本模型供应商"), "{error}");
        let error = Model::new(&[("DOTBOT_MODEL_BASE_URL".into(), "https://x.example".into()), ("DOTBOT_MODEL_KEY".into(), "k".into())], None).err().unwrap().to_string();
        assert!(error.contains("默认模型"), "{error}");
        let keyless = Model::new(&[("DOTBOT_MODEL_API".into(), "openai".into()), ("DOTBOT_MODEL_BASE_URL".into(), "http://127.0.0.1:11434/v1".into()), ("DOTBOT_MODEL_DEFAULT".into(), "qwen3".into())], None).unwrap();
        assert_eq!((keyless.url.as_str(), keyless.model.as_str()), ("http://127.0.0.1:11434/v1/chat/completions", "qwen3"));
    }

    #[test]
    fn legacy_and_missing_sessions_ask_for_a_fresh_start() {
        let home = tempfile::tempdir().unwrap();
        let mut spec = spec(home.path(), "http://localhost:1");
        spec.session = Some("legacy-session".into());
        let error = start(spec.clone()).err().unwrap().to_string();
        assert!(error.contains("No conversation found"), "{error}");
        spec.session = Some(format!("pi-{}", uuid::Uuid::new_v4()));
        let error = start(spec).err().unwrap().to_string();
        assert!(error.contains("No conversation found"), "{error}");
    }

    #[tokio::test]
    async fn external_mcp_tools_are_listed_called_and_gated() {
        let fixture = fixture().await;
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = calls.clone();
        let mcp = Router::new().route("/mcp", post(move |Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                let result = match body["method"].as_str().unwrap() {
                    "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"x","version":"1"}}),
                    "tools/list" => json!({"tools":[{"name":"echo","description":"Echo","inputSchema":{"type":"object","properties":{"v":{"type":"string"}}}}]}),
                    "tools/call" => { seen.lock().unwrap().push(body["params"].clone()); json!({"content":[{"type":"text","text":"echoed"}]}) }
                    _ => return axum::http::StatusCode::ACCEPTED.into_response(),
                };
                Json(json!({"jsonrpc":"2.0","id":body["id"],"result":result})).into_response()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, mcp).await.unwrap() });
        let mut registry = crate::agent_mcp::Registry::empty();
        let config = json!({"mcpServers":{"teambot":{},"conn":{"type":"http","url":url},"broken":{"type":"stdio","command":"/nonexistent/mcp"}}});
        let failed = registry.connect(&config, std::path::Path::new("/")).await;
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "broken");
        assert_eq!(registry.tools.len(), 1);
        assert_eq!(registry.tools[0].name, "mcp__conn__echo");
        let runtime = DotbotRuntime {
            model: Model::new(&spec(std::path::Path::new("/"), &fixture.url).env, Some("m")).unwrap(),
            env: json!({"DOTBOT_URL":fixture.url,"DOTBOT_TOKEN":"internal-test","DOTBOT_CONVERSATION":"bot:test"}),
            path: PathBuf::from("/nonexistent"),
            tool_client: reqwest::Client::new(),
            mcp: registry,
            skills: BTreeMap::new(),
            turn_context: String::new(),
            window: Window::new(128_000, 16_384),
            events: tokio::sync::mpsc::unbounded_channel().0,
        };
        let call = ToolCall { id: "c".into(), name: "mcp__conn__echo".into(), arguments: json!({"v":"x"}) };
        // The fixture's /internal/tool only serves list_bots: without an approval answer
        // the gate fails closed and the server is not called.
        assert!(runtime.before_tool_call(&call).await.is_err());
        assert!(calls.lock().unwrap().is_empty());
        let result = runtime.execute(&call, Arc::new(|_| {})).await.unwrap();
        assert_eq!(result.content[0]["text"], "echoed");
        assert_eq!(calls.lock().unwrap()[0]["name"], "echo");
        runtime.mcp.close().await;
        server.abort();
    }

    #[test]
    fn skills_load_by_name_from_the_turn_bundle() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("skills/weekly-report");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: weekly-report\ndescription: Use for the weekly report\n---\nStep one.").unwrap();
        let index = skill_index(home.path());
        assert_eq!(index.len(), 1);
        assert_eq!(index[0].1, "Use for the weekly report");
        let skills = index.into_iter().map(|(n, _, d)| (n, d)).collect();
        let loaded = load_skill(&skills, &json!({"skill":"dotbot-skills:weekly-report"}));
        assert!(!loaded.is_error);
        assert!(loaded.content[0]["text"].as_str().unwrap().ends_with("Step one."));
        assert!(load_skill(&skills, &json!({"skill":"../etc"})).is_error);
    }

    #[test]
    fn openai_stream_and_messages_round_trip() {
        let events = [
            json!({"choices":[{"delta":{"reasoning_content":"想"}}]}),
            json!({"choices":[{"delta":{"content":"你好"}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"mcp__teambot__list_bots","arguments":"{\"a\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":3}}),
        ];
        let data = events.iter().map(|e| format!("data: {e}\n\n")).collect::<String>() + "data: [DONE]\n\n";
        let mut stream = OpenAiStream::default();
        let mut text = String::new();
        for byte in data.as_bytes() {
            for event in stream.sse.push(&[*byte]).unwrap() {
                if let Some(delta) = stream.apply(event).unwrap() {
                    text.push_str(&delta);
                }
            }
        }
        assert!(stream.done);
        assert_eq!(text, "你好");
        let message = stream.finish().unwrap();
        assert_eq!(message["stopReason"], "toolUse");
        assert_eq!(message["content"][2]["arguments"]["a"], 1);
        assert_eq!(message["usage"]["prompt_tokens"], 3);
        let result = json!({"role":"toolResult","toolCallId":"call_1","toolName":"x","content":[{"type":"text","text":"ok"},{"type":"image","mimeType":"image/png","data":"AA"}],"isError":false});
        let user = json!({"role":"user","content":[{"type":"text","text":"hi"}]});
        let wire = to_openai_messages("sys", &[user, message, result]).unwrap();
        assert_eq!(wire[0]["role"], "system");
        assert_eq!(wire[1]["content"], "hi");
        assert_eq!(wire[2]["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(wire[2]["reasoning_content"], "想");
        assert_eq!(wire[3]["role"], "tool");
        assert_eq!(wire[4]["content"][1]["image_url"]["url"], "data:image/png;base64,AA");
        let full = from_openai_response(&json!({"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]})).unwrap();
        assert_eq!(full["stopReason"], "stop");
        assert_eq!(message_text(&full), "done");
    }

    /// A long session over a small window: the engine asks for a summary, keeps the newest
    /// exchanges, sends cache breakpoints, and the stored session starts with the summary.
    #[tokio::test]
    async fn long_sessions_are_compacted_into_a_stored_summary() {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let seen = requests.clone();
        let router = Router::new().route("/v1/messages", post(move |Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body.clone());
                let text = if body["stream"] == true { "继续完成" } else { "## 用户目标与要求\n整理报告" };
                Json(json!({"content":[{"type":"text","text":text}],"stop_reason":"end_turn","usage":{"input_tokens":900}}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let home = tempfile::tempdir().unwrap();
        let mut spec = spec(home.path(), &url);
        spec.env.push(("DOTBOT_MODEL_CONTEXT".into(), "8000".into()));
        spec.turn_context = "# 相关任务经验\n本轮".into();
        let id = format!("pi-{}", uuid::Uuid::new_v4());
        let mut old = vec![];
        for i in 0..12 {
            old.push(json!({"role":"user","content":[{"type":"text","text":format!("请求 {i} {}", "资料".repeat(400))}]}));
            old.push(json!({"role":"assistant","content":[{"type":"text","text":format!("回答 {i}")}],"stopReason":"stop","usage":{"input_tokens":(i + 1) * 900}}));
        }
        std::fs::create_dir_all(home.path().join("pi-sessions")).unwrap();
        crate::durable::save(&home.path().join("pi-sessions").join(format!("{id}.json")), &old).unwrap();
        spec.session = Some(id.clone());
        let mut turn = start(spec).unwrap();
        let mut statuses = vec![];
        while let Some(event) = turn.events.recv().await {
            match event {
                Event::Status(s) => statuses.push(s),
                Event::Done { text, is_error } => {
                    assert!(!is_error, "{text}");
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(statuses.first().map(String::as_str), Some("compacting"));
        let requests = requests.lock().unwrap().clone();
        let summary_request = requests.iter().find(|r| r["stream"] != true).expect("a summary request");
        assert!(summary_request["system"].as_str().unwrap().contains("已执行的外部动作"));
        let transcript = summary_request["messages"][0]["content"].as_str().unwrap();
        assert!(transcript.contains("[收到的消息]") && pi_context::text_tokens(transcript) <= 4_500, "the summarizer input fits half the window");
        let turn_request = requests.iter().find(|r| r["stream"] == true).unwrap();
        assert_eq!(turn_request["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(turn_request["system"][1]["text"], "# 相关任务经验\n本轮", "turn context follows the cached prefix");
        assert!(turn_request["messages"][0]["content"][0]["text"].as_str().unwrap().starts_with("<conversation_summary>"));
        let last = turn_request["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["content"].as_array().unwrap().last().unwrap()["cache_control"]["type"], "ephemeral");
        assert!(turn_request["tools"].as_array().unwrap().last().unwrap()["cache_control"].is_object());
        let stored: Vec<Value> = serde_json::from_slice(&std::fs::read(home.path().join("pi-sessions").join(format!("{id}.json"))).unwrap()).unwrap();
        assert_eq!(stored[0]["role"], "compactionSummary");
        assert!(stored.len() < old.len());
        assert!(stored.iter().skip(1).filter(|m| m["role"] == "assistant").rev().skip(1).all(|m| m["usage"].is_null()), "kept answers forget the old usage");
        assert!(!stored.iter().any(|m| m.to_string().contains("本轮")), "turn context is never stored");
        server.abort();
    }

    #[test]
    fn only_refusals_of_the_cached_form_are_retried_plainly() {
        assert!(refuses_cached_form("API Error: 400: system: Input should be a valid string (model m)"));
        assert!(refuses_cached_form("API Error: 400: cache_control: Extra inputs are not permitted (model m)"));
        assert!(!refuses_cached_form("API Error: 400: fixture task failed once (model m)"));
        assert!(!refuses_cached_form("API Error: 400: prompt is too long: system 250000 tokens (model m)"));
        assert!(!refuses_cached_form("API Error: 401: invalid x-api-key (model m)"));
    }

    #[tokio::test]
    async fn a_service_that_refuses_cache_control_gets_the_plain_form() {
        let router = Router::new().route("/v1/messages", post(|Json(body): Json<Value>| async move {
            if body["system"].is_array() {
                return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":{"message":"system: expected string"}}))).into_response();
            }
            Json(json!({"content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"})).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let home = tempfile::tempdir().unwrap();
        let mut turn = start(spec(home.path(), &url)).unwrap();
        while let Some(event) = turn.events.recv().await {
            if let Event::Done { text, is_error } = event {
                assert!(!is_error, "{text}");
                assert_eq!(text, "ok");
                break;
            }
        }
        server.abort();
    }

    #[tokio::test]
    async fn openai_structured_answers_use_function_calls() {
        let router = Router::new().route("/v1/chat/completions", post(|headers: axum::http::HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer sk-openai");
            assert_eq!(body["tool_choice"]["function"]["name"], "structured_answer");
            assert_eq!(body["messages"][0]["role"], "system");
            Json(json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"structured_answer","arguments":"{\"ok\":true}"}}]},"finish_reason":"tool_calls"}]}))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let env = vec![("DOTBOT_MODEL_API".into(), "openai".into()), ("DOTBOT_MODEL_BASE_URL".into(), url), ("DOTBOT_MODEL_KEY".into(), "sk-openai".into())];
        let answer = ask(&env, Some("gpt-x"), "sys", "q", &json!({"type":"object"})).await.unwrap();
        assert_eq!(answer, json!({"ok":true}));
        server.abort();
    }

    #[test]
    fn only_broken_transport_is_retried() {
        assert!(transient_request_error("模型请求失败：connection reset: error sending request"));
        assert!(transient_request_error("API Error: 502:  (model x)"));
        assert!(transient_request_error("模型流中断：stream terminated: peer closed connection"));
        for error in ["API Error: 429: 个人额度已用完", "API Error: 401: invalid key", "API Error: 422 model \"x\" is not supported", "API Error: 403 内容安全策略"] {
            assert!(!transient_request_error(error), "{error}");
        }
    }

    #[test]
    fn sse_rejoins_surrogate_pairs_split_across_events() {
        let mut sse = Sse::default();
        let data = concat!(
            "data: {\"t\":\"(=\\ud83d\"}\n\n",
            "data: {\"t\":\"\\ude3a=)\"}\n\n",
            "data: {\"t\":\"x\\ude3a \\ud83d y \\\\ud83d\"}\n\n",
        );
        let texts: Vec<String> = sse.push(data.as_bytes()).unwrap().iter().map(|e| e["t"].as_str().unwrap().into()).collect();
        assert_eq!(texts, ["(=", "😺=)", "x\u{fffd} \u{fffd} y \\ud83d"]);
    }

    #[test]
    fn sse_handles_byte_boundaries_and_strict_tool_arguments() {
        let events = vec![
            json!({"type":"message_start","message":{"content":[],"usage":{"input_tokens":5}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"中文"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t","name":"tool","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"value\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"好\"}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ];
        let data = events
            .iter()
            .map(|e| format!("event: ignored\r\ndata: {e}\r\n\r\n"))
            .collect::<String>();
        let mut parser = AnthropicStream::default();
        let mut text = String::new();
        for byte in data.as_bytes() {
            for event in parser.push(&[*byte]).unwrap() {
                if let Some(delta) = parser.apply(event).unwrap() {
                    text.push_str(&delta);
                }
            }
        }
        assert!(parser.done);
        assert_eq!(text, "中文");
        let message = from_anthropic_message(parser.message).unwrap();
        assert_eq!(message["content"][1]["arguments"]["value"], "好");
        assert_eq!(message["usage"]["input_tokens"], 5);
        assert_eq!(message["usage"]["output_tokens"], 7);
        assert_eq!(message["stopReason"], "toolUse");
        let mut broken = AnthropicStream {
            message: json!({"content":[{"type":"tool_use","input":{}}]}),
            ..Default::default()
        };
        broken.arguments.insert(0, "{".into());
        assert!(
            broken
                .apply(json!({"type":"content_block_stop","index":0}))
                .is_err()
        );
        assert!(broken.apply(json!({"type":"error"})).is_err());
    }
}
