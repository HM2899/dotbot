//! Native MCP client transports. Each turn owns its sessions and stdio process groups.
use anyhow::{Context, Result, ensure};
use serde_json::{Value,json};
use std::{collections::BTreeMap, process::Stdio, time::Duration};
use tokio::{io::{AsyncBufReadExt,AsyncWriteExt,BufReader},process::{Child,ChildStdin,ChildStdout},sync::Mutex};

const PROTOCOL: &str = "2025-03-26";

/// A JSON-RPC error answer: the server refused the request, so nothing ran.
#[derive(Debug)]
struct Rejected(String);
impl std::fmt::Display for Rejected {fn fmt(&self,f:&mut std::fmt::Formatter) -> std::fmt::Result {f.write_str(&self.0)}}
impl std::error::Error for Rejected {}

struct Process {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    pid: Option<u32>,
}
impl Drop for Process {
    fn drop(&mut self) {
        // Only the process group created by this MCP client; never user-owned browsers.
        #[cfg(unix)] if let Some(pid) = self.pid {
            unsafe extern "C" {fn kill(pid:i32,signal:i32)->i32;}
            unsafe {kill(-(pid as i32),9);}
        }
        let _ = self.child.start_kill();
    }
}

enum Transport {
    Stdio(Box<Process>),
    Http {client:reqwest::Client,url:String,headers:BTreeMap<String,String>,session:Option<String>,protocol:String},
}
struct Client {transport:Transport,next_id:i64}

impl Client {
    fn new(config:&Value,cwd:&std::path::Path) -> Result<Self> {
        let transport = if config["type"] == "http" || config.get("url").is_some() {
            let url = config["url"].as_str().context("MCP URL missing")?.to_string();
            ensure!(matches!(reqwest::Url::parse(&url)?.scheme(),"http"|"https"),"MCP URL must use HTTP(S)");
            let headers = config["headers"].as_object().into_iter().flatten().map(|(k,v)|Ok((k.clone(),v.as_str().context("MCP header must be text")?.into()))).collect::<Result<_>>()?;
            Transport::Http {client:reqwest::Client::builder().timeout(Duration::from_secs(3600)).redirect(reqwest::redirect::Policy::none()).build()?,url,headers,session:None,protocol:PROTOCOL.into()}
        } else {
            let mut command = tokio::process::Command::new(config["command"].as_str().context("MCP command missing")?);
            command.args(config["args"].as_array().into_iter().flatten().filter_map(Value::as_str)).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
            #[cfg(unix)] command.process_group(0);
            for (key,value) in config["env"].as_object().into_iter().flatten() {command.env(key,value.as_str().context("MCP env must be text")?);}
            let mut child = command.spawn().context("MCP process failed to start")?;
            let pid = child.id();
            let input = child.stdin.take().context("MCP stdin missing")?;
            let output = BufReader::new(child.stdout.take().context("MCP stdout missing")?);
            Transport::Stdio(Box::new(Process {child,input,output,pid}))
        };
        Ok(Self {transport,next_id:0})
    }
    async fn initialize(&mut self) -> Result<()> {
        let result = self.request("initialize",json!({"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"dotbot-pi-rust","version":env!("CARGO_PKG_VERSION")}})).await?;
        ensure!(result["serverInfo"].is_object(),"Invalid MCP initialization");
        if let Transport::Http {protocol,..} = &mut self.transport { *protocol = result["protocolVersion"].as_str().unwrap_or(PROTOCOL).into(); }
        self.notify("notifications/initialized",json!({})).await?;
        Ok(())
    }
    async fn notify(&mut self,method:&str,params:Value) -> Result<()> {
        let value = json!({"jsonrpc":"2.0","method":method,"params":params});
        match &mut self.transport {
            Transport::Stdio(process) => process.input.write_all(format!("{value}\n").as_bytes()).await?,
            Transport::Http {client,url,headers,session,protocol} => {
                http_request(client,url,headers,session.as_deref(),protocol,&value).send().await?.error_for_status()?;
            }
        }
        Ok(())
    }
    async fn request(&mut self,method:&str,params:Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let value = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let response = match &mut self.transport {
            Transport::Stdio(process) => {
                process.input.write_all(format!("{value}\n").as_bytes()).await?;
                loop {
                    let mut line = String::new();
                    ensure!(process.output.read_line(&mut line).await? > 0,"MCP process closed");
                    ensure!(line.len() <= 16*1024*1024,"MCP response too large");
                    // Servers that print banners or logs to stdout are tolerated: only JSON-RPC lines count.
                    let Ok(received) = serde_json::from_str::<Value>(&line) else {continue};
                    if received.get("method").is_some() && received.get("id").is_some() {
                        let reply = json!({"jsonrpc":"2.0","id":received["id"],"error":{"code":-32601,"message":"Client sampling and elicitation are not enabled"}});
                        process.input.write_all(format!("{reply}\n").as_bytes()).await?;
                    } else if received["id"] == id {break received;}
                }
            }
            Transport::Http {client,url,headers,session,protocol} => {
                let response = http_request(client,url,headers,session.as_deref(),protocol,&value).send().await?.error_for_status()?;
                if method == "initialize" {*session = response.headers().get("mcp-session-id").and_then(|v|v.to_str().ok()).map(String::from);}
                crate::connections::rpc_response(response,id).await?
            }
        };
        if let Some(error) = response.get("error") {
            return Err(Rejected(format!("MCP server rejected {method}: {}",error["message"].as_str().unwrap_or("no message"))).into());
        }
        response.get("result").cloned().context("MCP response missing result")
    }
    async fn close(&mut self) {
        match &mut self.transport {
            Transport::Stdio(process) => {let _ = process.child.kill().await;let _ = process.child.wait().await;}
            Transport::Http {client,url,headers,session,protocol} => {
                if let Some(id) = session.take() {
                    let mut request = client.delete(&*url).header("Mcp-Session-Id",id).header("MCP-Protocol-Version",&*protocol).timeout(Duration::from_secs(3));
                    for (key,value) in headers.iter() {request = request.header(key.as_str(),value.as_str());}
                    let _ = request.send().await;
                }
            }
        }
    }
}

fn http_request(client:&reqwest::Client,url:&str,headers:&BTreeMap<String,String>,session:Option<&str>,protocol:&str,value:&Value) -> reqwest::RequestBuilder {
    let mut request = client.post(url).header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version",protocol).json(value);
    for (key,value) in headers {request = request.header(key,value);}
    if let Some(id) = session {request = request.header("Mcp-Session-Id",id);}
    request
}

pub struct Registry {
    clients: BTreeMap<String,Mutex<Client>>,
    pub tools: Vec<pi_agent_core::Tool>,
    names: BTreeMap<String,(String,String)>,
}
impl Registry {
    pub fn empty() -> Self {Self {clients:BTreeMap::new(),tools:vec![],names:BTreeMap::new()}}
    /// Connects every configured server except dotbot's own. A server that fails to start
    /// or list its tools is left out of this turn (its error returned), like an MCP client
    /// marking one server failed; the others stay usable.
    pub async fn connect(&mut self,config:&Value,cwd:&std::path::Path) -> Vec<(String,String)> {
        let mut failed = vec![];
        for (server,config) in config["mcpServers"].as_object().into_iter().flatten() {
            if server == "teambot" {continue;}
            if let Err(error) = self.connect_one(server,config,cwd).await {
                if let Some(client) = self.clients.remove(server) {client.lock().await.close().await;}
                self.names.retain(|_,(s,_)| s != server);
                let names = &self.names;
                self.tools.retain(|t| names.contains_key(&t.name));
                failed.push((server.clone(),format!("{error:#}")));
            }
        }
        failed
    }
    async fn connect_one(&mut self,server:&str,config:&Value,cwd:&std::path::Path) -> Result<()> {
        let client = Client::new(config,cwd)?;
        self.clients.insert(server.to_string(),Mutex::new(client));
        tokio::time::timeout(Duration::from_secs(40),self.clients[server].lock().await.initialize()).await.context("MCP initialization timed out")??;
        let mut cursor = Value::Null;
        let mut seen = std::collections::HashSet::new();
        loop {
            let params = if cursor.is_null() {json!({})} else {json!({"cursor":cursor})};
            let result = tokio::time::timeout(Duration::from_secs(30),self.clients[server].lock().await.request("tools/list",params)).await.context("MCP tools/list timed out")??;
            for tool in result["tools"].as_array().context("MCP tools/list missing tools")? {
                let name = tool["name"].as_str().context("MCP tool missing name")?;
                let raw = format!("mcp__{server}__{name}");
                let alias = if raw.len()<=64 && raw.chars().all(|c|c.is_ascii_alphanumeric() || c=='_' || c=='-') {raw} else {
                    use sha2::{Digest,Sha256};format!("mcp_{}",hex::encode(Sha256::digest(raw.as_bytes())))[..64].to_string()
                };
                ensure!(!self.names.contains_key(&alias),"Duplicate MCP tool name");
                self.names.insert(alias.clone(),(server.to_string(),name.into()));
                let mut parameters = tool["inputSchema"].clone();
                if !parameters.is_object() {parameters = json!({"type":"object","properties":{}});}
                self.tools.push(pi_agent_core::Tool {name:alias,description:tool["description"].as_str().unwrap_or(name).into(),parameters,sequential:true});
            }
            cursor = result["nextCursor"].clone();
            if cursor.is_null() {break;}
            ensure!(self.tools.len()<4096 && seen.insert(cursor.to_string()),"Invalid MCP pagination");
        }
        Ok(())
    }
    pub fn server(&self,name:&str) -> Option<&str> {self.names.get(name).map(|(s,_)|s.as_str())}
    /// Ok(Err) when the server refused the call outright; Err when the outcome is unknown.
    pub async fn call(&self,name:&str,args:&Value) -> Result<std::result::Result<Value,String>> {
        let (server,tool) = self.names.get(name).context("Unknown MCP tool")?;
        match self.clients[server].lock().await.request("tools/call",json!({"name":tool,"arguments":args})).await {
            Ok(result) => Ok(Ok(result)),
            Err(error) => match error.downcast_ref::<Rejected>() {
                Some(rejected) => Ok(Err(rejected.0.clone())),
                None => Err(error.context("MCP execution outcome unknown; inspect action before resuming")),
            },
        }
    }
    pub async fn close(&self) {for client in self.clients.values() {client.lock().await.close().await;}}
}
