//! The local web server: the page, the protocol (POST /api/bot and the feed), the media
//! files, and /internal/tool for `dotbot mcp`. It listens on 127.0.0.1 only. The page
//! uses administrator password sessions. The MCP servers use a separate token.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use rust_embed::RustEmbed;
use serde_json::{json, Value};

use crate::memory::Scope;
use crate::team::Team;

#[derive(RustEmbed)]
#[folder = "web/"]
struct Assets;

#[derive(Clone)]
pub struct Server {
    pub team: Arc<Team>,
    pub admin: Arc<crate::admin_auth::AdminAuth>,
}

// Stable cookie name preserves administrator sessions; legacy tokens are rejected.
const COOKIE: &str = "teambot";

fn asset_revision() -> &'static str {
    static REVISION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    REVISION.get_or_init(|| {
        use sha2::Digest;
        let mut hash = sha2::Sha256::new();
        let mut names: Vec<_> = Assets::iter().collect(); names.sort();
        for name in names {
            if let Some(file) = Assets::get(&name) { hash.update(name.as_bytes()); hash.update(&file.data); }
        }
        hex::encode(hash.finalize())
    })
}

fn signed_in(server: &Server, headers: &HeaderMap) -> bool {
    let Some(token) = session_token(headers) else { return false };
    !server.admin.setup_required() && server.admin.valid(token)
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(&format!("{COOKIE}=")))
}

pub fn router(server: Server) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login_page))
        .route("/setup", get(setup_page))
        .route("/auth/status", get(auth_status))
        .route("/auth/login", post(admin_login).layer(axum::extract::DefaultBodyLimit::max(4096)))
        .route("/auth/logout", post(admin_logout).layer(axum::extract::DefaultBodyLimit::max(4096)))
        .route("/auth/password", post(admin_password).layer(axum::extract::DefaultBodyLimit::max(4096)))
        .route("/auth/setup", post(admin_setup).layer(axum::extract::DefaultBodyLimit::max(4096)))
        .route("/api/bot", post(api))
        .route("/api/bot/feed", get(feed))
        .route("/api/files", post(upload).layer(axum::extract::DefaultBodyLimit::max(crate::attachments::FILE_MAX)))
        .route("/oauth/callback", get(oauth_callback))
        .route("/internal/tool", post(internal_tool))
        .route("/internal/browser/{bot}", post(internal_browser))
        .route("/hook/{id}/{key}", post(hook).layer(axum::extract::DefaultBodyLimit::max(256 * 1024)))
        .route("/media/{file}", get(media))
        .route("/desktop/{bot}/", axum::routing::any(desktop_proxy))
        .route("/desktop/{bot}/{*rest}", axum::routing::any(desktop_proxy))
        .route("/assets/{*path}", get(asset))
        // Images ride in chat.send as data URLs.
        .layer(axum::extract::DefaultBodyLimit::max(40 * 1024 * 1024))
        .layer(axum::middleware::from_fn_with_state(server.clone(), web_boundary))
        .with_state(server)
}

async fn index(State(server): State<Server>, headers: HeaderMap) -> Response {
    if server.admin.setup_required() && session_token(&headers).is_some_and(|token| server.admin.valid(token)) {
        return Redirect::to("/setup").into_response();
    }
    if !signed_in(&server, &headers) {
        return Redirect::to("/login").into_response();
    }
    serve_asset("index.html")
}

/// Only explicitly configured origins are trusted; forwarded headers are not credentials.
async fn web_boundary(State(server): State<Server>, request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let headers = request.headers();
    let internal = request.uri().path().starts_with("/internal/");
    let webhook = request.uri().path().starts_with("/hook/");
    if !internal && !webhook {
        let mut allowed = vec![server.team.cfg.url.as_str()];
        if let Some(public) = &server.team.cfg.public_url { allowed.push(public); }
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
        let known_host = host.is_some_and(|host| allowed.iter().any(|url| url.split_once("://").is_some_and(|(_, authority)| authority == host)));
        let unsafe_request = !matches!(*request.method(), axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS)
            || headers.get(header::UPGRADE).is_some();
        if !known_host || (unsafe_request && (origin.is_some_and(|origin| !allowed.contains(&origin)) || headers.get("sec-fetch-site").is_some_and(|v| v == "cross-site"))) {
            return (StatusCode::FORBIDDEN, "请求来源不受信任。").into_response();
        }
    }
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response.headers_mut().insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    response.headers_mut().entry(header::X_FRAME_OPTIONS).or_insert("SAMEORIGIN".parse().unwrap());
    if !path.starts_with("/assets/") { response.headers_mut().insert(header::CACHE_CONTROL, "no-store".parse().unwrap()); }
    if server.team.cfg.public_url.as_ref().is_some_and(|v| v.starts_with("https://")) {
        response.headers_mut().insert(header::STRICT_TRANSPORT_SECURITY, "max-age=31536000".parse().unwrap());
    }
    response
}

fn login_cookie(server: &Server, token: &str, seconds: i64) -> String {
    let secure = if server.team.cfg.public_url.as_ref().is_some_and(|v| v.starts_with("https://")) { "; Secure" } else { "" };
    format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={seconds}{secure}")
}

async fn login_page(State(server): State<Server>, headers: HeaderMap) -> Response {
    if signed_in(&server, &headers) { return Redirect::to("/").into_response(); }
    if server.admin.setup_required() && session_token(&headers).is_some_and(|token| server.admin.valid(token)) { return Redirect::to("/setup").into_response(); }
    auth_page("login.html")
}

fn auth_page(asset: &str) -> Response {
    let mut response = serve_asset(asset);
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'self'; frame-ancestors 'none'".parse().unwrap());
    response.headers_mut().insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    response
}

async fn setup_page(State(server): State<Server>, headers: HeaderMap) -> Response {
    if !server.admin.setup_required() { return Redirect::to(if signed_in(&server, &headers) { "/" } else { "/login" }).into_response(); }
    if !session_token(&headers).is_some_and(|token| server.admin.valid(token)) { return Redirect::to("/login").into_response(); }
    auth_page("setup.html")
}

async fn auth_status(State(server): State<Server>, headers: HeaderMap) -> Response {
    let signed_in = session_token(&headers).is_some_and(|token| server.admin.valid(token));
    Json(json!({"configured":true,"signedIn":signed_in,"setupRequired":server.admin.setup_required(),"username":signed_in.then(|| server.admin.username())})).into_response()
}

#[derive(serde::Deserialize)]
struct LoginForm { username: String, password: String, #[serde(default)] remember: bool }

fn login_error(error: crate::admin_auth::LoginError) -> Response {
    use crate::admin_auth::LoginError;
    let (status, message) = match error {
        LoginError::Invalid => (StatusCode::UNAUTHORIZED, "管理员账号或密码不正确。"),
        LoginError::Validation(message) => (StatusCode::BAD_REQUEST, message),
        LoginError::Limited => (StatusCode::TOO_MANY_REQUESTS, "尝试次数过多，请稍后再试。"),
        LoginError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "登录服务暂时不可用，请稍后重试。"),
    };
    let mut response = (status, Json(json!({"error":message}))).into_response();
    if status == StatusCode::TOO_MANY_REQUESTS { response.headers_mut().insert(header::RETRY_AFTER, "60".parse().unwrap()); }
    response
}

async fn admin_login(State(server): State<Server>, Json(form): Json<LoginForm>) -> Response {
    match server.admin.login(form.username, form.password, form.remember).await {
        Ok((token, seconds)) => ([(header::SET_COOKIE,login_cookie(&server,&token,seconds))], Json(json!({"ok":true,"setupRequired":server.admin.setup_required()}))).into_response(),
        Err(error) => login_error(error),
    }
}

async fn admin_logout(State(server): State<Server>, headers: HeaderMap) -> Response {
    if !session_token(&headers).is_some_and(|token| server.admin.valid(token)) { return StatusCode::UNAUTHORIZED.into_response(); }
    if server.admin.logout(session_token(&headers).unwrap()).is_err() { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
    ([(header::SET_COOKIE,login_cookie(&server,"",0))], Json(json!({"ok":true}))).into_response()
}

#[derive(serde::Deserialize)]
struct SetupForm { username: String, password: String, name: String, #[serde(default)] bio: String }

async fn admin_setup(State(server): State<Server>, headers: HeaderMap, Json(form): Json<SetupForm>) -> Response {
    if !server.admin.setup_required() { return (StatusCode::CONFLICT, Json(json!({"error":"安装引导已完成，请前往登录。"}))).into_response(); }
    let Some(token) = session_token(&headers).filter(|token| server.admin.valid(token)) else { return StatusCode::UNAUTHORIZED.into_response(); };
    let name = form.name.trim();
    let bio = form.bio.trim();
    if name.is_empty() || name.chars().count() > 40 || bio.chars().count() > 500 {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":"昵称须为 1–40 个字符，个人简介不超过 500 个字符。"}))).into_response();
    }
    let profile = json!({"name":name,"bio":bio});
    match server.admin.complete_setup(token.to_string(), form.username, form.password, || async {
        server.team.set_profile(&profile).await.map(|_| ())
    }).await {
        Ok(()) => ([(header::SET_COOKIE,login_cookie(&server,"",0))], Json(json!({"ok":true}))).into_response(),
        Err(error) => login_error(error),
    }
}

#[derive(serde::Deserialize)]
struct PasswordForm { current: String, #[serde(default)] username: Option<String>, #[serde(default)] password: Option<String> }

async fn admin_password(State(server): State<Server>, headers: HeaderMap, Json(form): Json<PasswordForm>) -> Response {
    if !signed_in(&server,&headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    match server.admin.change_credentials(session_token(&headers).unwrap().to_string(),form.current,form.username,form.password).await {
        Ok(()) => ([(header::SET_COOKIE,login_cookie(&server,"",0))],Json(json!({"ok":true}))).into_response(),
        Err(error) => login_error(error),
    }
}

async fn oauth_callback(State(server): State<Server>, Query(query): Query<std::collections::HashMap<String, String>>) -> Response {
    let result = server.team.accounts.callback(query.get("state").map(String::as_str).unwrap_or_default(), query.get("code").map(String::as_str).unwrap_or_default(), query.contains_key("error")).await;
    // Check the new grant right away so the page shows who signed in.
    let tested = match &result { Ok(id) => server.team.accounts.test(id).await.is_ok(), Err(_) => false };
    server.team.team_changed().await;
    let message = if tested { "登录完成，账号已连接。可以关闭此页，回到 dotbot 设置调整各项服务的权限。" } else if result.is_ok() { "授权已保存，但连接测试未通过。请回到 dotbot 设置查看错误信息。" } else { "账号授权未完成或已过期。请从 dotbot 设置重新发起授权。" };
    ([(header::CONTENT_TYPE,"text/html; charset=utf-8"),(header::CACHE_CONTROL,"no-store"),(header::REFERRER_POLICY,"no-referrer"),(header::CONTENT_SECURITY_POLICY,"default-src 'none'; frame-ancestors 'none'")], format!("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><title>dotbot</title><p>{message}</p></html>")).into_response()
}

fn serve_asset(path: &str) -> Response {
    serve_asset_if(path, None)
}

/// Assets are revalidated on every load; a matching ETag answers 304, so a reload does
/// not re-download every script while the page sits blank.
fn serve_asset_if(path: &str, if_none_match: Option<&str>) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let etag = format!("\"{}\"", hex::encode(&file.metadata.sha256_hash()[..16]));
            let cache = [(header::CACHE_CONTROL, "no-cache".to_string()), (header::ETAG, etag.clone())];
            if if_none_match.is_some_and(|tags| tags.split(',').any(|t| t.trim().trim_start_matches("W/") == etag)) {
                return (StatusCode::NOT_MODIFIED, cache).into_response();
            }
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], cache, file.data.into_owned()).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn asset(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve_asset_if(&path, headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()))
}

async fn upload(State(server): State<Server>, headers: HeaderMap, Query(query): Query<std::collections::HashMap<String, String>>, bytes: axum::body::Bytes) -> Response {
    if !signed_in(&server, &headers) { return StatusCode::UNAUTHORIZED.into_response(); }
    let result = server.team.stage_attachment(query.get("conversation").map(String::as_str).unwrap_or_default(), query.get("name").map(String::as_str).unwrap_or_default(), &bytes).await;
    match result {
        Ok(file) => Json(json!({ "ok": true, "file": file })).into_response(),
        Err(error) => (StatusCode::BAD_REQUEST, Json(json!({ "ok": false, "error": server.team.secrets.redact(&format!("{error:#}")) }))).into_response(),
    }
}

async fn media(State(server): State<Server>, headers: HeaderMap, Path(file): Path<String>) -> Response {
    if !signed_in(&server, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if file.contains(['/', '\\']) || file == "." || file == ".." {
        return StatusCode::NOT_FOUND.into_response();
    }
    let path = server.team.chats.media_dir().join(&file);
    let Ok(size) = tokio::fs::metadata(&path).await.map(|m| m.len()) else { return StatusCode::NOT_FOUND.into_response() };
    let Ok(mut handle) = tokio::fs::File::open(&path).await else { return StatusCode::NOT_FOUND.into_response() };
    let mime = mime_guess::from_path(&file).first_or_octet_stream().as_ref().to_string();
    // Video and audio seek with Range; everything is streamed, so a large file is never held in memory.
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|v| byte_range(v, size));
    let (status, start, end) = match range {
        Some((start, end)) => (StatusCode::PARTIAL_CONTENT, start, end),
        None if headers.contains_key(header::RANGE) && size > 0 => {
            return (StatusCode::RANGE_NOT_SATISFIABLE, [(header::CONTENT_RANGE, format!("bytes */{size}"))]).into_response();
        }
        None => (StatusCode::OK, 0, size.saturating_sub(1)),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    if handle.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let chunks = futures::stream::unfold((handle, length), |(mut handle, left)| async move {
        if left == 0 {
            return None;
        }
        let mut buffer = vec![0u8; left.min(256 * 1024) as usize];
        match handle.read(&mut buffer).await {
            Ok(0) => None,
            Ok(n) => {
                buffer.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buffer)), (handle, left - n as u64)))
            }
            Err(error) => Some((Err(error), (handle, 0))),
        }
    });
    let mut response = axum::body::Body::from_stream(chunks).into_response();
    *response.status_mut() = status;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, mime.parse().unwrap_or(header::HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CONTENT_LENGTH, length.into());
    h.insert(header::ACCEPT_RANGES, header::HeaderValue::from_static("bytes"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, header::HeaderValue::from_static("nosniff"));
    h.insert(header::CONTENT_SECURITY_POLICY, header::HeaderValue::from_static(if mime == "application/pdf" { "sandbox allow-scripts; default-src 'none'; style-src 'unsafe-inline'" } else { "sandbox; default-src 'none'; style-src 'unsafe-inline'" }));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("private, no-store"));
    let inline = matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "application/pdf" | "text/plain") || mime.starts_with("audio/") || mime.starts_with("video/");
    let disposition = format!("{}; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, urlencoding::encode(&file));
    if let Ok(value) = disposition.parse() { h.insert(header::CONTENT_DISPOSITION, value); }
    if status == StatusCode::PARTIAL_CONTENT {
        h.insert(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}").parse().unwrap());
    }
    response
}

/// One "bytes=a-b", "bytes=a-" or "bytes=-n" range inside a file of `size` bytes, inclusive.
fn byte_range(value: &str, size: u64) -> Option<(u64, u64)> {
    let spec = value.trim().strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    if size == 0 {
        return None;
    }
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (size.saturating_sub(n), size - 1)
    } else {
        let start: u64 = a.parse().ok()?;
        let end = if b.is_empty() { size - 1 } else { b.parse::<u64>().ok()?.min(size - 1) };
        (start, end)
    };
    (start <= end && start < size).then_some((start, end))
}

/// The Bot desktops' web UI, through dotbot: the containers listen on this machine only,
/// so the page reaches them wherever it is opened, behind the same sign-in. WebSocket
/// upgrades are passed through as raw byte streams.
async fn desktop_proxy(State(server): State<Server>, Path(params): Path<std::collections::HashMap<String, String>>, request: axum::extract::Request) -> Response {
    if !signed_in(&server, request.headers()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(desktops) = server.team.desktops.as_ref() else { return StatusCode::NOT_FOUND.into_response() };
    let bot = params.get("bot").cloned().unwrap_or_default();
    // Validate the Bot even for cached endpoints, and restart stale streaming seats.
    if let Err(error) = server.team.desktop_url(&bot).await {
        return (StatusCode::BAD_GATEWAY, error.to_string()).into_response();
    }
    let Some(socket) = desktops.socket(&bot).await else { return StatusCode::BAD_GATEWAY.into_response() };
    let result = async {
        let stream = tokio::net::UnixStream::connect(socket).await?;
        let headers = request.headers().clone();
        proxy_stream(stream, "localhost", &format!("/desktop/{bot}"), request, server.clone(), headers).await
    }.await;
    match result {
        Ok(response) => response,
        Err(error) => (StatusCode::BAD_GATEWAY, format!("桌面暂时未响应：{error}")).into_response(),
    }
}

async fn proxy_stream<S>(stream: S, backend: &str, prefix: &str, mut request: axum::extract::Request, server: Server, headers: HeaderMap) -> anyhow::Result<Response>
where S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {
    let path_and_query = request.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
    let rest = path_and_query.strip_prefix(prefix).unwrap_or("/");
    let rest = if rest.starts_with('/') { rest.to_string() } else { format!("/{rest}") };
    let incoming_upgrade = hyper::upgrade::on(&mut request);
    let (mut parts, body) = request.into_parts();
    parts.uri = rest.parse()?;
    parts.headers.insert(header::HOST, backend.parse()?);
    if parts.headers.contains_key(header::ORIGIN) {
        parts.headers.insert(header::ORIGIN, format!("http://{backend}").parse()?);
    }
    // The page's sign-in cookie stays with dotbot.
    parts.headers.remove(header::COOKIE);
    let upstream = axum::extract::Request::from_parts(parts, body);
    let (mut sender, connection) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = sender.send_request(upstream).await?;
    if response.status() == StatusCode::SWITCHING_PROTOCOLS {
        let outgoing_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            if let (Ok(client), Ok(desktop)) = (incoming_upgrade.await, outgoing_upgrade.await) {
                let mut client = hyper_util::rt::TokioIo::new(client);
                let mut desktop = hyper_util::rt::TokioIo::new(desktop);
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut client, &mut desktop) => {},
                    _ = wait_signed_out(server, headers) => {},
                }
            }
        });
    }
    let (parts, body) = response.into_parts();
    Ok(Response::from_parts(parts, axum::body::Body::new(body)))
}

async fn internal_tool(State(server): State<Server>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let bearer = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default();
    if bearer != format!("Bearer {}", server.team.cfg.internal_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let conversation = body["conversation"].as_str().unwrap_or_default();
    let name = body["name"].as_str().unwrap_or_default();
    let (text, is_error, images) = server.team.call_tool(conversation, name, &body["args"]).await;
    let images: Vec<Value> = images.into_iter().map(|(mime, data)| json!({ "mime": mime, "data": data })).collect();
    Json(json!({ "text": text, "isError": is_error, "images": images })).into_response()
}

/// A stateless MCP gate in front of the persistent browser. User takeover and
/// read-only policies apply to model calls, not just to buttons in the UI.
async fn internal_browser(State(server): State<Server>, Path(bot_id): Path<String>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) != Some(&format!("Bearer {}", server.team.cfg.internal_token)) { return StatusCode::UNAUTHORIZED.into_response(); }
    let conversation = headers.get("x-teambot-conversation").and_then(|v| v.to_str().ok()).unwrap_or_default();
    let state = server.team.state().await;
    let Some(bot) = state.bots.get(&bot_id) else { return StatusCode::NOT_FOUND.into_response() };
    let Some(conv) = crate::team::Conv::parse(conversation).filter(|c| c.bot() == bot_id) else { return StatusCode::FORBIDDEN.into_response() };
    if let Some(room) = conv.room() && !state.rooms.get(room).is_some_and(|r| r.members.contains(&bot_id)) { return StatusCode::FORBIDDEN.into_response(); }
    let Some(browsers) = server.team.browsers.as_ref() else { return StatusCode::NOT_FOUND.into_response() };
    let id = body["id"].clone();
    let method = body["method"].as_str().unwrap_or_default();
    if method.starts_with("notifications/") { return StatusCode::ACCEPTED.into_response(); }
    let result: anyhow::Result<Value> = async {
        Ok(match method {
            "initialize" => json!({ "protocolVersion": body["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"), "capabilities": { "tools": {} }, "serverInfo": { "name": "dotbot-browser", "version": env!("CARGO_PKG_VERSION") } }),
            "ping" => json!({}),
            "tools/list" => {
                let mut result = browsers.request(&bot_id, method, body["params"].clone()).await?;
                if let Some(tools) = result["tools"].as_array_mut() {
                    tools.retain(|t| {
                        let name = t["name"].as_str().unwrap_or_default();
                        !matches!(name, "browser_run_code_unsafe" | "browser_file_upload" | "browser_install") && (bot.permission.as_deref() != Some("readonly") || crate::browser::readonly_tool(name))
                    });
                }
                result
            }
            "tools/call" => {
                let name = body["params"]["name"].as_str().unwrap_or_default();
                let denial = if matches!(name, "browser_run_code_unsafe" | "browser_file_upload" | "browser_install") { Some("This browser tool accesses the host and is disabled.") }
                    else if browsers.user_has_control(&bot_id).await && !crate::browser::observation(name) { Some("The user has control of this browser. Observe only until they give it back.") }
                    else if bot.permission.as_deref() == Some("readonly") && !crate::browser::readonly_tool(name) { Some("This Bot is read-only; that browser action is disabled.") }
                    else { None };
                let mut answer = if let Some(text) = denial { json!({ "isError": true, "content": [{ "type": "text", "text": text }] }) }
                else {
                    if !crate::browser::observation(name){server.team.claim_computer(&bot_id,conversation).await?;}
                    let receipts=if crate::browser::readonly_tool(name){Vec::new()}else{server.team.runs.lock().await.get(conversation).map(|r|r.receipts.clone()).unwrap_or_default()};
                    let actions=server.team.recovery.action(&receipts,name,&server.team.secrets.redact_value(body["params"]["arguments"].clone()))?;
                    let result=browsers.request(&bot_id,method,body["params"].clone()).await;
                    let(failed,detail)=match &result{Ok(value)=>(value["isError"]==true,value.to_string()),Err(error)=>(true,error.to_string())};
                    server.team.recovery.action_done(&receipts,&actions,failed,&server.team.secrets.redact(&detail))?;
                    // A failed step is a tool error with the page attached, not a protocol error.
                    result.unwrap_or_else(|error| json!({ "isError": true, "content": [{ "type": "text", "text": server.team.secrets.redact(&format!("{error:#}")) }] }))
                };
                // Every step that acts on the page, done, failed or refused, comes back with
                // the page as it is now, so the Bot checks the result instead of assuming it.
                if !crate::browser::observation(name) {
                    let shot = tokio::time::timeout(std::time::Duration::from_secs(15), browsers.request(&bot_id, "tools/call", json!({ "name": "browser_take_screenshot", "arguments": {} }))).await;
                    let images: Vec<Value> = match shot { Ok(Ok(value)) => value["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "image").cloned().collect(), _ => vec![] };
                    let failed = answer["isError"] == true;
                    if let Some(content) = answer["content"].as_array_mut() {
                        if images.is_empty() {
                            content.push(json!({ "type": "text", "text": "（未能获取页面截图；需要确认结果时用 browser_take_screenshot 查看。）" }));
                        } else {
                            content.push(json!({ "type": "text", "text": if failed { "已附当前页面截图：先看截图判断失败原因，再决定下一步。" } else { "已附操作后的页面截图：确认结果后再继续。" } }));
                            content.extend(images);
                        }
                    }
                }
                answer
            }
            _ => anyhow::bail!("Unsupported browser MCP method"),
        })
    }.await;
    Json(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": server.team.secrets.redact(&error.to_string()) } }),
    }).into_response()
}

/// A routine's webhook: the body comes along to the Bot. The key in the address is the
/// only check, as with any webhook URL.
async fn hook(State(server): State<Server>, Path((id, key)): Path<(String, String)>, headers: HeaderMap, body: String) -> Response {
    match server.team.webhook(&id, &key, &headers, body).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn feed(State(server): State<Server>, headers: HeaderMap) -> Response {
    if !signed_in(&server, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let receiver = server.team.feed.subscribe();
    let mut hello = server.team.hello().await;
    hello["serverInstance"] = json!(server.team.server_instance);
    hello["assetRevision"] = json!(asset_revision());
    let work = server.team.work_view().await;
    let stream = feed_stream(hello, work, receiver).take_until(wait_signed_out(server, headers));
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(20))).into_response()
}

async fn wait_signed_out(server: Server, headers: HeaderMap) {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if !signed_in(&server, &headers) { return; }
    }
}

fn feed_stream(hello: Value, work: Value, receiver: tokio::sync::broadcast::Receiver<(String, Value)>) -> impl Stream<Item = Result<SseEvent, Infallible>> {
    let initial = std::collections::VecDeque::from([
        SseEvent::default().event("hello").data(hello.to_string()),
        SseEvent::default().event("work").data(work.to_string()),
    ]);
    futures::stream::unfold((initial, receiver), |(mut initial, mut receiver)| async move {
        if let Some(event) = initial.pop_front() { return Some((Ok(event), (initial, receiver))); }
        match receiver.recv().await {
            Ok((event, _)) if event == "shutdown" => None,
            Ok((event, data)) => Some((Ok(SseEvent::default().event(event).data(data.to_string())), (initial, receiver))),
            // A window that fell behind reads the state again from a new hello.
            Err(_) => None,
        }
    })
}

fn ok(value: Value) -> Json<Value> {
    Json(json!({ "ok": true, "value": value }))
}
fn fail(code: &str, message: impl ToString) -> Json<Value> {
    Json(json!({ "ok": false, "error": { "code": code, "message": message.to_string() } }))
}

/// The model of the Bot whose memory it is (a group's admin for a group), for the
/// memory panel's calls to the model.
fn scope_of(params: &Value) -> Option<Scope> {
    let id = params["id"].as_str().unwrap_or_default().to_string();
    match params["scope"].as_str()? {
        "team" => Some(Scope::Team),
        "bot" if !id.is_empty() => Some(Scope::Bot(id)),
        "group" if !id.is_empty() => Some(Scope::Group(id)),
        _ => None,
    }
}

async fn api(State(server): State<Server>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    if !signed_in(&server, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let method = body["method"].as_str().unwrap_or_default().to_string();
    let params = if body["params"].is_object() { body["params"].clone() } else { json!({}) };
    let team = server.team.clone();
    let s = |k: &str| params[k].as_str().unwrap_or_default().to_string();
    let result: anyhow::Result<Value> = async {
        Ok(match method.as_str() {
            "team.state" => {
                let mut view = team.team_view().await;
                view["work"] = team.work_view().await;
                view
            }
            "bot.create" => serde_json::to_value(team.create_bot_with_description(&s("name"), &s("role"), &s("instructions"), Some(s("model")).filter(|m| !m.is_empty()), None, params["description"].as_str().map(str::to_string)).await?)?,
            "bot.update" => serde_json::to_value(team.update_bot(&s("id"), &params).await?)?,
            "sidebar.group.create" | "sidebar.group.update" | "sidebar.group.delete" => {
                team.sidebar_group(method.rsplit('.').next().unwrap_or_default(), &params).await?;
                json!(true)
            }
            "sidebar.move" => { team.sidebar_move(&s("conversation"), &s("group")).await?; json!(true) }
            "bot.delete" => {
                team.delete_bot(&s("id")).await?;
                json!(true)
            }
            "bot.setMain" => {
                team.set_main(&s("id"), params["main"].as_bool().unwrap_or(true)).await?;
                json!(true)
            }
            "room.create" => {
                let members: Vec<String> = params["members"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(String::from)).collect();
                let admin = params["admin"].as_str().map(String::from).or_else(|| members.first().cloned());
                let room = team.create_room(&s("name"), members, admin, &s("notice"), None).await?;
                serde_json::to_value(room)?
            }
            "room.update" => {
                team.update_room(&s("id"), &params).await?;
                json!(true)
            }
            "room.delete" => {
                team.delete_room(&s("id")).await?;
                json!(true)
            }
            "chat.send" => {
                let images: Vec<String> = params["images"].as_array().into_iter().flatten().filter_map(|i| i.as_str().map(String::from)).collect();
                let files: Vec<String> = params["files"].as_array().into_iter().flatten().filter_map(|i| i.as_str().map(String::from)).collect();
                team.send_files(&s("conversation"), &s("text"), images, files).await?;
                json!(true)
            }
            "chat.correct" => {
                let files: Vec<String> = serde_json::from_value(params.get("files").cloned().unwrap_or_else(|| json!([])))?;
                let auto = params["auto"].as_bool().unwrap_or(false);
                // A message the page routed to running work is classified first: a new request
                // waits in the queue and a progress question is answered, both without
                // interrupting; only an amendment or a stop goes into the running work.
                let triage = if auto { team.triage_message(&s("conversation"), params["task"].as_str(), params["run"].as_str(), &s("text")).await } else { None };
                match triage.as_ref().map(|t| t.kind.as_str()) {
                    Some("new") => { team.send_files(&s("conversation"), &s("text"), vec![], files).await?; return Ok(json!({"sent": true, "triage": "new"})); }
                    Some("progress") => {
                        let bot = match params["task"].as_str().and_then(|id| team.tasks.get(id)) { Some(task) => task.target, None => params["run"].as_str().and_then(|k| k.rsplit(':').next()).unwrap_or_default().to_string() };
                        let records = team.attachment_records(&s("conversation"), &files).await?;
                        team.answer_progress(&s("conversation"), &bot, &s("text"), &records, &triage.as_ref().unwrap().answer).await?;
                        return Ok(json!({"answered": true, "triage": "progress"}));
                    }
                    _ => {}
                }
                match team.correct_work(&s("conversation"), params["task"].as_str(), params["run"].as_str(), params["since"].as_i64(), &s("text"), files.clone(), auto).await {
                    Ok(value) => value,
                    // The work finished between the page's view and this message: an
                    // automatically routed message simply becomes a new one.
                    Err(_) if auto => { team.send_files(&s("conversation"), &s("text"), vec![], files).await?; json!({"sent": true}) }
                    Err(error) => return Err(error),
                }
            }
            "chat.page" => team.history_page(&s("conversation"), params["before"].as_u64().map(|b| b as usize), params["after"].as_u64().map(|b| b as usize), params["anchor"].as_str()).await,
            "chat.search" => team.history_search(&s("conversation"), &s("query"), params["offset"].as_u64().unwrap_or(0) as usize).await,
            "chat.read" => team.history_read(&s("conversation"), params["through"].as_str(), params["anchor"].as_str(), params["offset"].as_f64().unwrap_or(0.0)).await?,
            "chat.stop" => json!(team.stop(&s("conversation")).await),
            "chat.stop_item" => json!(team.stop_item(&s("conversation"), &s("item")).await),
            "chat.rate" => team.rate_reply(&s("conversation"), &s("item"), &s("rating"), params["retract"].as_bool().unwrap_or(false), &s("reason")).await?,
            "chat.clear" => {
                team.clear_history(&s("conversation")).await?;
                json!(true)
            }
            "chat.reset" => {
                team.reset(&s("conversation")).await;
                json!(true)
            }
            "approval.answer" => {
                team.answer_approval(&s("id"), params["allow"].as_bool().unwrap_or(false)).await?;
                json!(true)
            }
            "memory.view" => team.memory_view(&scope_of(&params).ok_or_else(|| anyhow::anyhow!("Unknown memory"))?).await,
            "memory.add" => {
                let scope = scope_of(&params).ok_or_else(|| anyhow::anyhow!("Unknown memory"))?;
                json!(team.memory_user_change(&scope, Some(&s("text")), params["topic"].as_str(), None).await?)
            }
            "memory.forget" => {
                let scope = scope_of(&params).ok_or_else(|| anyhow::anyhow!("Unknown memory"))?;
                json!(team.memory_user_change(&scope, None, params["topic"].as_str(), Some(&s("entry"))).await?)
            }
            // Several entries at once, one by one: each that cannot be removed is reported, the rest still go.
            "memory.forget_many" => {
                let items = params["items"].as_array().cloned().unwrap_or_default();
                if items.len() > 500 { anyhow::bail!("Remove at most 500 entries at a time"); }
                let (mut removed, mut failed) = (0, vec![]);
                for item in &items {
                    let entry = item["entry"].as_str().unwrap_or_default();
                    let result = match scope_of(item) {
                        Some(scope) => team.memory_user_change(&scope, None, item["topic"].as_str(), Some(entry)).await,
                        None => Err(anyhow::anyhow!("Unknown memory")),
                    };
                    match result { Ok(_) => removed += 1, Err(error) => failed.push(json!({ "entry": entry, "error": error.to_string() })) }
                }
                json!({ "removed": removed, "failed": failed })
            }
            "bot.pin" => serde_json::to_value(team.update_bot(&s("id"), &json!({ "pinned": params["pinned"].as_bool().unwrap_or(true) })).await?)?,
            "card.answer" => {
                team.answer_card(&s("id"), &s("answer")).await?;
                json!(true)
            }
            "card.dismiss" => {
                team.dismiss_card(&s("id")).await?;
                json!(true)
            }
            "routine.list" => team.routines_view().await,
            "routine.history" => json!(team.routines.history(params["id"].as_str()).await),
            "event.setup" => team.event_setup(&s("id")).await?,
            "event.test" => team.event_local_test(&s("id"),&params["payload"]).await?,
            "event.register" => team.event_register(&s("id"),&s("account"),&s("repository")).await?,
            "event.provider-test" => team.event_provider_test(&s("id")).await?,
            "event.public-url" => { team.events.set_origin(&s("url"))?;team.team_changed().await;team.publish("routines",json!({}));json!({"publicUrl":team.public_origin()}) }
            "event.subscription" => team.event_subscription(&s("id"),params["delete"].as_bool().unwrap_or(false)).await?,
            "routine.create" => {
                let timezone = params["timezone"].as_str().map(str::to_string);
                let when = crate::routines::parse_when_in(&params["when"], timezone.as_deref())?;
                serde_json::to_value(team.routine_create(&s("title"), &s("prompt"), &s("target"), when, timezone, None).await?)?
            }
            "routine.update" => {
                team.routine_update(&s("id"), &params).await?;
                json!(true)
            }
            "routine.run" => {
                let routine = team.routines.list.lock().await.iter().find(|r| r.id == s("id")).cloned().ok_or_else(|| anyhow::anyhow!("No such routine"))?;
                json!(team.run_routine(&routine, None, "manual", None).await?)
            }
            "task.list" => json!(team.tasks.list()),
            "task.get" => json!(team.tasks.get(&s("id")).ok_or_else(|| anyhow::anyhow!("Unknown task"))?),
            "task.cancel" => json!(team.cancel_task(&s("id")).await?),
            "task.retry" => json!(team.task_retry(&s("id"),params["reviewed"].as_bool().unwrap_or(false)).await?),
            "workflow.list" => json!(team.workflows.list()),
            "recovery.list" => json!(team.recovery.view()),
            "recovery.get" => { let work=team.recovery.get(&s("id"))?; json!({"id":work.id,"conversation":work.conversation,"status":work.status,"text":work.text,"checkpoint":work.checkpoint,"actions":work.actions,"result":work.result}) }
            "delivery.list" => json!(team.outbox.view()),
            "delivery.retry" => { team.outbox.retry(&s("id"),params["reviewed"].as_bool().unwrap_or(false),params["skip"].as_bool().unwrap_or(false))?; team.team_changed().await; json!(true) }
            "skill.save" => {
                let name = team.skills.save_checked(&s("name"), &s("description"), &s("text"), params["revision"].as_str())?;
                team.team_changed().await;
                json!(name)
            }
            "skill.delete" => {
                team.skills.delete(&s("name"))?;
                team.team_changed().await;
                json!(true)
            }
            "skill.get" => team.skills.get(&s("name"))?,
            "skill.validate" => tokio::time::timeout(std::time::Duration::from_secs(45),team.skills.validate_bundle(&s("name"))).await.map_err(|_|anyhow::anyhow!("技能包校验超过 45 秒，请拆分技能包后重试。"))??,
            "skill.candidate.get" => team.skills.candidate(&s("id"))?,
            "skill.candidates" => json!(team.skills.candidates(&s("name"))),
            "skill.candidate.test" => team.start_candidate_tests(&s("id")).await?,
            "skill.candidate.reject" => {team.skills.candidate_reject(&s("id"))?;team.team_changed().await;json!(true)}
            "skill.benchmarks" => team.skills.benchmarks(&s("name"))?,
            "skill.benchmark.save" => json!(team.skills.save_benchmark(&s("name"),&s("goal"),&s("acceptance"))?),
            "skill.versions" => json!(team.skills.versions(&s("name"))?),
            "skill.rollback" => {team.skills.rollback(&s("name"),&s("version"),&s("revision"))?;team.team_changed().await;json!(true)}
            "skill.trial" => {
                let skill=team.skills.get(&s("name"))?;
                if skill["enabled"]!=true||skill["bundleValid"]!=true{anyhow::bail!("请先修复并启用技能。");}
                let _=team.skills.versions(&s("name"))?;
                let state=team.state().await;
                let bot=state.bots.get(&s("bot")).ok_or_else(||anyhow::anyhow!("请选择试运行 Bot。"))?;
                let suite=team.skills.benchmarks(&s("name"))?;
                let saved=suite["cases"].as_array().and_then(|cases|cases.iter().find(|c|c["id"]==s("case")));
                let goal=saved.and_then(|v|v["goal"].as_str()).map(str::to_string).unwrap_or_else(||s("goal"));let acceptance=saved.and_then(|v|v["acceptance"].as_str()).map(str::to_string).unwrap_or_else(||s("acceptance"));
                if goal.trim().is_empty()||acceptance.trim().is_empty()||goal.len()>12000||acceptance.len()>4000{anyhow::bail!("请填写试运行任务和可核对的验收条件。");}
                let case=team.skills.save_benchmark(&s("name"),&goal,&acceptance)?;
                let details=crate::tasks::AssignmentDetails{objective:goal.clone(),expected_output:"交付试运行产物、执行证据及未解决问题。".into(),acceptance,explicit:false};
                json!(team.create_workflow(&format!("bot:{}",bot.id),Some(format!("trial|{}|{}|{}|{}",s("name"),case,skill["revision"].as_str().unwrap_or(""),uuid::Uuid::new_v4().simple())),&goal,vec![crate::workflows::Stage{id:"trial".into(),bot:bot.id.clone(),text:format!("[技能试运行]\n请先 read_skill 读取 {}，按该技能执行以下实际测试任务，保存 task_checkpoint，核对验收条件并交付证据。\n任务：{}\n验收：{}",s("name"),goal,details.acceptance),files:vec![],details,depends_on:vec![],priority:0}]).await?)
            }
            "skill.catalog" => json!(crate::skill_sources::catalog(&s("query"))),
            "skill.store" => crate::skill_sources::store_view(&team.skills),
            "mcp.store" => crate::mcp_store::index().clone(),
            "skill.install" => {
                // A pasted command or address goes through the parser; a store card sends its id.
                let command=s("command");
                let install=async{ if command.trim().is_empty(){crate::skill_sources::install(&team.skills,&s("source"),None).await}else{crate::skill_command::install(&team.skills,&command,false).await} };
                let name=match tokio::time::timeout(std::time::Duration::from_secs(90),install).await{
                    Ok(result)=>result.map_err(|error|anyhow::anyhow!(crate::skill_sources::install_error(&error)))?,
                    Err(_)=>anyhow::bail!("The skill source timed out. Check the network connection to GitHub and try again."),
                };
                team.team_changed().await; json!(name)
            }
            "skill.toggle" => {
                team.skills.set_enabled(&s("name"), params["enabled"].as_bool().ok_or_else(|| anyhow::anyhow!("Choose whether to enable the skill."))?, &s("revision"))?;
                team.team_changed().await;
                json!(true)
            }
            "connection.save" => {
                let connection: crate::connections::Connection = serde_json::from_value(params.clone())?;
                let saved = team.connections.upsert(connection)?;
                team.team_changed().await;
                saved.view()
            }
            "connection.test" => { let value = team.connections.test(&s("name"), &team.secrets).await?; team.team_changed().await; value }
            "account.catalog" => crate::accounts::catalog(),
            "account.save" => { let value = team.accounts.save(&params)?; team.team_changed().await; value }
            "account.authorize" => team.accounts.authorize(&s("id"), &team.public_origin())?,
            "account.connect" => { let value = team.accounts.connect(&s("provider"), &team.public_origin())?; team.team_changed().await; value }
            "account.app.save" => { let value = team.accounts.save_app(&params)?; team.team_changed().await; value }
            "account.test" => { let result = team.accounts.test(&s("id")).await; team.team_changed().await; result? }
            "account.revoke" => { let value = team.accounts.revoke(&s("id"), params["delete"].as_bool().unwrap_or(false)).await?; team.team_changed().await; value }
            "connection.delete" => {
                team.connections.remove(&s("name"))?;
                team.team_changed().await;
                json!(true)
            }
            "channel.save" => team.channel_save(&params).await?,
            "channel.delete" => {
                team.channel_delete(&s("id")).await?;
                json!(true)
            }
            "channel.allow" => {
                team.channel_allow(&s("id"), &s("sender"), params["allow"].as_bool().unwrap_or(true)).await?;
                json!(true)
            }
            "channel.reconnect" => { team.channel_reconnect(&s("id")).await?; json!(true) }
            "channel.diagnostics" => {if team.channels.get(&s("id")).is_none(){anyhow::bail!("找不到社交平台连接。");}team.channel_diagnostics.reconcile(&team.outbox.trace_states());team.channel_diagnostics.view(&s("id"))}
            "channel.login" => team.channel_login(&s("id")).await?,
            "secret.set" => {
                let name = team.secrets.set(&s("name"), &s("value"))?;
                team.team_changed().await;
                json!(name)
            }
            "secret.delete" => {
                team.secrets.delete(&s("name"))?;
                team.team_changed().await;
                json!(true)
            }
            "memory.ask" => {
                let scope = scope_of(&params).ok_or_else(|| anyhow::anyhow!("Unknown memory"))?;
                team.memory_ask(&scope, &s("text")).await?
            }
            "desktop.launch" => team.desktop_launch(&s("bot"), &s("app"), &s("url")).await?,
            "desktop.open" => json!(team.desktop_url(&s("bot")).await?),
            "desktop.status" => team.desktop_status(&s("bot")).await?,
            "desktop.control" => {
                team.desktop_control(&s("bot"), params["user"].as_bool().unwrap_or(false)).await?;
                json!(true)
            }
            "browser.peek" => team.browser_peek(&s("bot")).await?,
            "browser.control" => {
                if team.state().await.bots.get(&s("bot")).is_none() { anyhow::bail!("Unknown Bot"); }
                let browsers = team.browsers.as_ref().ok_or_else(|| anyhow::anyhow!("Browsers are off"))?;
                browsers.set_control(&s("bot"), params["user"].as_bool().unwrap_or(false)).await;
                json!(true)
            }
            "browser.act" => team.browser_act(&s("bot"), &params).await?,
            "desktop.teach" => team.desktop_lesson(&s("bot"),&s("action"),&s("goal")).await?,
            "desktop.teach.events" => team.desktop_lesson_events(&s("bot"),&s("token"),&params["events"]).await?,
            "browser.teach" => team.lesson(&s("bot"), &s("action"), &s("goal")).await?,
            "browser.share" => json!(team.share_logins(&s("bot")).await?),
            "browser.call" => team.browser_call(&s("bot"), &s("tool"), if params["args"].is_object() { params["args"].clone() } else { json!({}) }).await?,
            "settings.workspace" => json!(team.set_workspace(&s("path")).await?),
            "settings.local-computer" => {
                team.local_computer.set_enabled(params["enabled"].as_bool().ok_or_else(|| anyhow::anyhow!("enabled must be a boolean"))?)?;
                team.team_changed().await;
                team.publish("local-computer", team.local_computer.status());
                team.local_computer.status()
            }
            "local.status" => team.local_computer.status(),
            "local.pause" => {
                team.local_computer.pause(params["paused"].as_bool().ok_or_else(|| anyhow::anyhow!("paused must be a boolean"))?)?;
                team.team_changed().await;
                team.publish("local-computer", team.local_computer.status());
                team.local_computer.status()
            }
            "settings.learning" => {
                let enabled=params["enabled"].as_bool().ok_or_else(||anyhow::anyhow!("enabled must be a boolean"))?;
                team.change(|state,_|{state.prefs.insert("learningEnabled".into(),json!(enabled));Ok(())}).await?;
                json!(enabled)
            }
            "learning.list" => json!(team.learning.list(&s("bot"))),
            "learning.forget" => {if let Some(id)=team.learning.get(&s("id")).and_then(|r|r.candidate){let _=team.skills.candidate_reject(&id);}team.learning.forget(&s("id"))?;team.team_changed().await;json!(true)}
            "settings.lang" => {
                let lang = if s("lang") == "en" { "en" } else { "zh" };
                team.change(|state, _| {
                    state.prefs.insert("lang".into(), json!(lang));
                    Ok(())
                })
                .await?;
                json!(lang)
            }
            "profile.update" => team.set_profile(&params).await?,
            "provider.list" => team.providers.view(),
            "provider.save" => {
                let provider: crate::providers::Provider = serde_json::from_value(params.clone())?;
                let saved = team.providers.upsert(provider)?;
                team.team_changed().await;
                saved.view(false)
            }
            "provider.delete" => {
                team.providers.remove(&s("id"))?;
                json!(true)
            }
            "provider.use" => {
                let provider = team.providers.use_provider(&s("id"))?;
                team.team_changed().await;
                json!(provider.name)
            }
            "provider.stop" => {
                team.providers.stop(&s("kind"))?;
                team.team_changed().await;
                json!(true)
            }
            "provider.effort" => {
                team.providers.set_effort(&s("id"), &s("effort"))?;
                team.team_changed().await;
                json!(true)
            }
            "provider.jobs" => {
                team.providers.set_jobs(&params["jobs"])?;
                json!(true)
            }
            "provider.test" => team.provider_test(&s("id")).await?,
            "provider.models" => {
                let saved = team.providers.get(&s("id"));
                let kind = saved.as_ref().map(|p|p.kind.clone()).unwrap_or_else(||s("kind"));
                let key = Some(s("apiKey")).filter(|k|!k.trim().is_empty()).or_else(||saved.as_ref().map(|p|p.api_key.clone())).unwrap_or_default();
                let base = Some(s("baseUrl")).filter(|b|!b.trim().is_empty()).or_else(||saved.as_ref().map(|p|p.base_url.clone())).unwrap_or_default();
                let api = Some(s("api")).filter(|v|!v.is_empty()).or_else(||saved.as_ref().map(|p|p.api.clone())).unwrap_or_else(||"anthropic".into());
                json!(crate::providers::fetch_models(&base,&key,if kind.is_empty() {"text"} else {&kind},&api).await?)
            }
            _ => return Err(anyhow::anyhow!("NOTFOUND Unknown method {method}")),
        })
    }
    .await;
    match result {
        Ok(mut value) => { crate::media_gen::public_file(&mut value); ok(team.secrets.redact_value(value)).into_response() },
        Err(error) => {
            let message = team.secrets.redact(&error.to_string());
            match message.strip_prefix("NOTFOUND ") {
                Some(rest) => fail("bot/not-found", rest).into_response(),
                None => fail("bot/refused", message).into_response(),
            }
        }
    }
}

#[cfg(test)]
mod range_tests {
    use super::byte_range;

    #[test]
    fn ranges_are_read_like_browsers_send_them() {
        assert_eq!(byte_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(byte_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(byte_range("bytes=90-500", 100), Some((90, 99)));
        assert_eq!(byte_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(byte_range("bytes=100-", 100), None);
        assert_eq!(byte_range("items=0-1", 100), None);
    }
}
