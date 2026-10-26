//! `dotbot mcp`: the Bot tools as a stdio MCP server, one per turn. An external MCP client starts
//! it from the turn's --mcp-config; each call goes to the dotbot server, which knows
//! the team and answers for the conversation named in DOTBOT_CONVERSATION.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::tools;

const PROTOCOL: &str = "2025-06-18";

pub struct Link {
    pub url: String,
    pub token: String,
    pub conversation: String,
}

impl Link {
    pub fn from_env() -> Result<Self> {
        Ok(Link {
            url: crate::branding::env("DOTBOT_URL").context("DOTBOT_URL is not set")?,
            token: crate::branding::env("DOTBOT_TOKEN").context("DOTBOT_TOKEN is not set")?,
            conversation: crate::branding::env("DOTBOT_CONVERSATION").context("DOTBOT_CONVERSATION is not set")?,
        })
    }
}

/// The answer to one JSON-RPC request, or None for a notification.
/// A tool's answer: its text, whether it failed, and images as (media type, base64).
pub type Answer = (String, bool, Vec<(String, String)>);

pub async fn answer(request: &Value, call: &(impl Fn(String, Value) -> futures::future::BoxFuture<'static, Answer> + ?Sized)) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "dotbot", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let mut list = tools::definitions();
            if crate::branding::env("DOTBOT_DESKTOP").is_ok_and(|v| v == "1") {
                list.extend(tools::desktop_definitions());
            }
            if crate::branding::env("DOTBOT_LOCAL").is_ok_and(|v| v == "1") {
                list.extend(tools::local_definitions());
            }
            list.extend(tools::media_definitions(&crate::branding::env("DOTBOT_MEDIA").unwrap_or_default()));
            json!({ "tools": list })
        }
        "tools/call" => {
            let name = request["params"]["name"].as_str().unwrap_or_default().to_string();
            let args = request["params"]["arguments"].clone();
            let (text, is_error, images) = call(name, if args.is_null() { json!({}) } else { args }).await;
            let mut content = vec![json!({ "type": "text", "text": text })];
            content.extend(images.into_iter().map(|(mime, data)| json!({ "type": "image", "mimeType": mime, "data": data })));
            json!({ "content": content, "isError": is_error })
        }
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Unknown method {method}") } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub async fn serve(link: Link) -> Result<()> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(60 * 60)).build()?;
    let call = move |name: String, args: Value| -> futures::future::BoxFuture<'static, Answer> {
        let client = client.clone();
        let url = format!("{}/internal/tool", link.url);
        let token = link.token.clone();
        let conversation = link.conversation.clone();
        Box::pin(async move {
            let sent = client.post(url).bearer_auth(token).json(&json!({ "conversation": conversation, "name": name, "args": args })).send().await;
            match sent {
                Ok(response) => match response.json::<Value>().await {
                    Ok(value) => {
                        let images = value["images"].as_array().into_iter().flatten().filter_map(|i| Some((i["mime"].as_str()?.to_string(), i["data"].as_str()?.to_string()))).collect();
                        (value["text"].as_str().unwrap_or_default().to_string(), value["isError"].as_bool().unwrap_or(false), images)
                    }
                    Err(error) => (format!("dotbot answered badly: {error}"), true, Vec::new()),
                },
                Err(error) => (format!("dotbot is not reachable: {error}"), true, Vec::new()),
            }
        })
    };
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply) = answer(&request, &call).await {
            stdout.write_all(format!("{reply}\n").as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_speaks_mcp_and_forwards_calls() {
        let call = |name: String, args: Value| -> futures::future::BoxFuture<'static, Answer> { Box::pin(async move { (format!("{name} {args}"), false, Vec::new()) }) };
        let init = answer(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}), &call).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(answer(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}), &call).await.is_none());
        let list = answer(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}), &call).await.unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"message_bot") && names.contains(&"approve") && names.contains(&"remember"));
        let called = answer(&json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"recall","arguments":{"query":"tea"}}}), &call).await.unwrap();
        assert_eq!(called["result"]["content"][0]["text"], r#"recall {"query":"tea"}"#);
        let unknown = answer(&json!({"jsonrpc":"2.0","id":4,"method":"resources/list"}), &call).await.unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }
}
