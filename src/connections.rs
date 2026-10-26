//! Connections: MCP servers the whole team shares (GitHub, Notion, a database…), in
//! `<home>/connections.json`. Connect once and every Bot's turns get it in their
//! --mcp-config, as grokbot's shared connections do. `${NAME}` in a setting is filled
//! from the saved keys when a turn starts, so values stay out of this file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::secrets::Secrets;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub name: String,
    /// "stdio" or "http".
    pub kind: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// Names an MCP server may take (tool names are mcp__<server>__<tool>), and none of dotbot's own.
pub fn clean_name(raw: &str) -> Result<String> {
    let name: String = raw.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() || name.len() > 32 || ["dotbot", "teambot", "browser"].contains(&name.as_str()) {
        bail!("A connection's name is a short word, not dotbot or browser.");
    }
    Ok(name)
}

impl Connection {
    pub fn check(mut self) -> Result<Self> {
        self.name = clean_name(&self.name)?;
        match self.kind.as_str() {
            "stdio" if self.command.trim().is_empty() => bail!("A local connection needs the command that starts it."),
            "http" if !self.url.starts_with("http://") && !self.url.starts_with("https://") => bail!("A remote connection needs its http(s) address."),
            "stdio" | "http" => Ok(self),
            _ => bail!("A connection is stdio (a local command) or http (an address)."),
        }
    }

    pub fn to_mcp(&self, secrets: &Secrets) -> Value {
        let fill = |map: &BTreeMap<String, String>| map.iter().map(|(k, v)| (k.clone(), json!(secrets.fill(v)))).collect::<serde_json::Map<_, _>>();
        if self.kind == "http" {
            json!({ "type": "http", "url": secrets.fill(&self.url), "headers": fill(&self.headers) })
        } else {
            json!({ "type": "stdio", "command": self.command, "args": self.args.iter().map(|a| secrets.fill(a)).collect::<Vec<_>>(), "env": fill(&self.env) })
        }
    }

    /// What the page shows: settings as written, with ${NAME} and no values.
    pub fn view(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }

    fn fingerprint(&self) -> String {
        hex::encode(Sha256::digest(serde_json::to_vec(self).unwrap()))
    }
}

pub struct Connections {
    path: PathBuf,
    pub list: std::sync::Mutex<Vec<Connection>>,
    checks: std::sync::Mutex<BTreeMap<String, Value>>,
}

impl Connections {
    pub fn open(home: &Path) -> Self {
        let path = home.join("connections.json");
        let list = crate::sqlite_storage::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let checks = crate::sqlite_storage::read(home.join("connection-checks.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Connections { path, list: std::sync::Mutex::new(list), checks: std::sync::Mutex::new(checks) }
    }

    fn save(&self, list: &[Connection]) -> Result<()> {
        crate::durable::save(&self.path, &list)
    }

    pub fn upsert(&self, connection: Connection) -> Result<Connection> {
        let connection = connection.check()?;
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        match next.iter_mut().find(|c| c.name == connection.name) {
            Some(slot) => *slot = connection.clone(),
            None => next.push(connection.clone()),
        }
        self.save(&next)?; *list = next;
        self.checks.lock().unwrap().remove(&connection.name);
        Ok(connection)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let next: Vec<_> = list.iter().filter(|c| c.name != name).cloned().collect();
        self.save(&next)?; *list = next;
        self.checks.lock().unwrap().remove(name);
        Ok(())
    }

    pub fn enabled(&self) -> Vec<Connection> {
        self.list.lock().unwrap().iter().filter(|c| c.enabled).cloned().collect()
    }

    pub fn views(&self) -> Vec<Value> {
        let list = self.list.lock().unwrap(); let checks = self.checks.lock().unwrap();
        list.iter().map(|c| { let mut v = c.view(); v["check"] = checks.get(&c.name).filter(|check|check["configuration"]==c.fingerprint()).cloned().unwrap_or(Value::Null); v }).collect()
    }

    pub async fn test(&self, name: &str, secrets: &Secrets) -> Result<Value> {
        let connection = self.list.lock().unwrap().iter().find(|c| c.name == name).cloned().ok_or_else(|| anyhow::anyhow!("Unknown connection"))?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(40), probe(&connection.to_mcp(secrets))).await;
        let value = match result {
            Ok(Ok(tools)) => json!({"state":"connected","checked":crate::store::now_ms(),"tools":tools}),
            Ok(Err(error)) => json!({"state":"error","checked":crate::store::now_ms(),"error":secrets.redact(&format!("MCP initialization or tools/list failed: {error:#}"))}),
            Err(_) => json!({"state":"error","checked":crate::store::now_ms(),"error":"MCP connection test timed out."}),
        };
        let list = self.list.lock().unwrap();
        let current = list.iter().find(|c| c.name == name);
        if current.as_ref().is_none_or(|c| c.view() != connection.view()) { bail!("Connection changed during its test; test again."); }
        let mut checks = self.checks.lock().unwrap(); let mut next = checks.clone(); let mut saved=value.clone();saved["configuration"]=json!(connection.fingerprint());next.insert(name.into(),saved);
        crate::durable::save(&self.path.with_file_name("connection-checks.json"),&next)?; *checks = next;
        Ok(value)
    }
}

async fn probe(config: &Value) -> Result<Vec<String>> {
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"dotbot-connection-test","version":"0.1"}}});
    let initialized = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
    let tools = json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}});
    let value = if config["type"] == "http" {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).redirect(reqwest::redirect::Policy::none()).build()?;
        let send = |value: &Value, session: Option<&str>| {
            let mut request = client.post(config["url"].as_str().unwrap_or_default()).header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version","2025-03-26");
            for (key,value) in config["headers"].as_object().into_iter().flatten() { request = request.header(key,value.as_str().unwrap_or_default()); }
            if let Some(session) = session { request = request.header("Mcp-Session-Id",session); }
            request.json(value)
        };
        let response = send(&initialize,None).send().await?.error_for_status()?;
        let session = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(String::from);
        let result: Result<Value> = async {
            let init = rpc_response(response,1).await?;
            if !init["result"]["serverInfo"].is_object() { bail!("Invalid MCP initialization"); }
            send(&initialized,session.as_deref()).send().await?.error_for_status()?;
            rpc_response(send(&tools,session.as_deref()).send().await?.error_for_status()?,2).await
        }.await;
        if let Some(session) = session {
            let mut request = client.delete(config["url"].as_str().unwrap_or_default()).header("Mcp-Session-Id",session);
            for (key,value) in config["headers"].as_object().into_iter().flatten() { request = request.header(key,value.as_str().unwrap_or_default()); }
            let _ = request.send().await;
        }
        result?
    } else {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
        use std::process::Stdio;
        let mut command = tokio::process::Command::new(config["command"].as_str().unwrap_or_default());
        command.args(config["args"].as_array().into_iter().flatten().filter_map(Value::as_str)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        #[cfg(unix)] command.process_group(0);
        for (key,value) in config["env"].as_object().into_iter().flatten() { command.env(key,value.as_str().unwrap_or_default()); }
        let mut child = command.spawn().map_err(|e| anyhow::anyhow!("cannot start {}: {e}", config["command"].as_str().unwrap_or_default()))?;
        // Keep the last stderr lines so a crashing server explains itself in the check result.
        let mut errors = child.stderr.take().unwrap();
        let stderr = tokio::spawn(async move { let mut tail = Vec::new(); let mut chunk = [0u8; 4096]; while let Ok(n) = errors.read(&mut chunk).await { if n == 0 { break; } tail.extend_from_slice(&chunk[..n]); if tail.len() > 8192 { tail.drain(..tail.len()-8192); } } String::from_utf8_lossy(&tail).lines().rev().filter(|l| !l.trim().is_empty()).take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n") });
        struct Cleanup(Option<u32>);
        impl Drop for Cleanup { fn drop(&mut self) { #[cfg(unix)] if let Some(pid) = self.0 { unsafe extern "C" { fn kill(pid:i32,sig:i32)->i32; } unsafe { kill(-(pid as i32),9); } } } }
        let _cleanup = Cleanup(child.id());
        let mut input = child.stdin.take().unwrap(); let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap().take(1024*1024));
        let read = async |output: &mut tokio::io::BufReader<tokio::io::Take<tokio::process::ChildStdout>>, id: i64| -> Result<Value> {
            // Servers that print banners or logs to stdout are tolerated: only JSON-RPC lines count.
            loop { let mut line = String::new(); if output.read_line(&mut line).await? == 0 { bail!("MCP process closed"); } let Ok(value) = serde_json::from_str::<Value>(&line) else { continue }; if value["id"] == id { return Ok(value); } }
        };
        let result: Result<Value> = async {
            input.write_all(format!("{initialize}\n").as_bytes()).await?;
            if !read(&mut output,1).await?["result"]["serverInfo"].is_object() { bail!("Invalid MCP initialization"); }
            input.write_all(format!("{initialized}\n{tools}\n").as_bytes()).await?;
            read(&mut output,2).await
        }.await;
        let _ = child.kill().await; let _ = child.wait().await;
        match result {
            Ok(value) => value,
            Err(error) => { let tail = tokio::time::timeout(std::time::Duration::from_secs(2), stderr).await.ok().and_then(Result::ok).unwrap_or_default(); if tail.is_empty() { return Err(error); } bail!("{error:#}; stderr: {tail}") }
        }
    };
    if value.get("error").is_some() { bail!("MCP rejected tools/list"); }
    let tools = value["result"]["tools"].as_array().ok_or_else(|| anyhow::anyhow!("Invalid tools/list response"))?;
    Ok(tools.iter().filter_map(|t|t["name"].as_str().map(String::from)).collect())
}

pub(crate) async fn rpc_response(mut response: reqwest::Response, id:i64) -> Result<Value> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len()+chunk.len()>1024*1024 { bail!("MCP response too large"); } bytes.extend_from_slice(&chunk);
        let text = String::from_utf8_lossy(&bytes);
        if let Ok(value) = serde_json::from_str::<Value>(&text) && value["id"] == id { return Ok(value); }
        for frame in text.replace("\r\n","\n").split("\n\n") {
            for line in frame.lines().filter_map(|l|l.strip_prefix("data:")) { if let Ok(value)=serde_json::from_str::<Value>(line.trim()) && value["id"]==id { return Ok(value); } }
        }
    }
    bail!("No matching MCP response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_reaches_every_turn_with_its_keys_filled() {
        let home = tempfile::tempdir().unwrap();
        let secrets = Secrets::open(home.path());
        secrets.set("GITHUB", "ghp_123456789").unwrap();
        let connections = Connections::open(home.path());
        let saved = connections
            .upsert(Connection { name: "GitHub".into(), kind: "http".into(), url: "https://api.example.com/mcp".into(), headers: [("Authorization".to_string(), "Bearer ${GITHUB}".to_string())].into(), enabled: true, ..Default::default() })
            .unwrap();
        assert_eq!(saved.name, "github");
        assert_eq!(saved.to_mcp(&secrets)["headers"]["Authorization"], "Bearer ghp_123456789");
        assert!(std::fs::read_to_string(home.path().join("connections.json")).unwrap().contains("${GITHUB}"));
        assert!(connections.upsert(Connection { name: "browser".into(), kind: "stdio".into(), command: "x".into(), ..Default::default() }).is_err());
        assert_eq!(Connections::open(home.path()).enabled().len(), 1);
    }

    #[test]
    fn changed_connection_does_not_reuse_old_test_evidence_after_restart() {
        let home=tempfile::tempdir().unwrap();let connections=Connections::open(home.path());
        let saved=connections.upsert(Connection{name:"service".into(),kind:"http".into(),url:"https://old.example.com/mcp".into(),enabled:true,..Default::default()}).unwrap();
        crate::durable::save(&home.path().join("connection-checks.json"),&json!({"service":{"state":"connected","configuration":saved.fingerprint()}})).unwrap();
        let connections=Connections::open(home.path());assert_eq!(connections.views()[0]["check"]["state"],"connected");
        connections.upsert(Connection{url:"https://new.example.com/mcp".into(),..saved}).unwrap();
        assert!(Connections::open(home.path()).views()[0]["check"].is_null());
    }
}


