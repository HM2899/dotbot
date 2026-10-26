//! Bounded platform media transfers. No credentials or signed CDN URLs enter chat logs.
use anyhow::{anyhow, bail, Result};
use base64::Engine;
use futures::future::BoxFuture;
use reqwest::{multipart, Client, Response, Url};
use serde_json::{json, Value};

pub struct IncomingFile {
    pub name: String,
    pub size: Option<u64>,
    pub get: BoxFuture<'static, Result<Vec<u8>>>,
}

pub struct PlatformFile {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub enum Outgoing { Text(String), File(PlatformFile) }

pub async fn read_response(mut response: Response) -> Result<Vec<u8>> {
    if !response.status().is_success() { bail!("Media download returned HTTP {}", response.status()); }
    let max = crate::attachments::FILE_MAX + 16; // encrypted media includes one padding block
    if response.content_length().is_some_and(|n| n > max as u64) { bail!("A file is at most 25 MB."); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > max { bail!("A file is at most 25 MB."); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub fn checked_url(raw: &str, configured_base: &str, suffixes: &[&str]) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow!("Invalid media address"))?;
    if !url.username().is_empty() || url.password().is_some() { bail!("Media URLs must not contain credentials."); }
    if let Ok(base) = Url::parse(configured_base)
        && base.origin() == url.origin() && matches!(url.scheme(), "http" | "https") {
        return Ok(url);
    }
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https" || url.port_or_known_default() != Some(443) || !suffixes.iter().any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}"))) {
        bail!("The media address is outside this platform's permitted hosts.");
    }
    Ok(url)
}

pub fn url_file(client: Client, url: String, base: String, name: String, size: Option<u64>, suffixes: &'static [&'static str]) -> IncomingFile {
    IncomingFile { name, size, get: Box::pin(async move {
        let url = checked_url(&url, &base, suffixes)?;
        follow_download(&client, client.get(url).send().await?, &base, suffixes).await
    }) }
}

pub async fn follow_download(client: &Client, mut response: Response, base: &str, suffixes: &[&str]) -> Result<Vec<u8>> {
    for _ in 0..4 {
        if !response.status().is_redirection() { return read_response(response).await; }
        let location = response.headers().get(reqwest::header::LOCATION).and_then(|v| v.to_str().ok()).ok_or_else(|| anyhow!("Media redirect returned no address"))?;
        let next = response.url().join(location).map_err(|_| anyhow!("Invalid media redirect"))?;
        let next = checked_url(next.as_str(), base, suffixes)?;
        // OAuth headers from the original API request must not follow a CDN redirect.
        response = client.get(next).send().await?;
    }
    bail!("Too many media redirects")
}

pub fn part(file: PlatformFile) -> Result<multipart::Part> {
    Ok(multipart::Part::bytes(file.bytes).file_name(file.name).mime_str(&file.mime)?)
}

pub async fn telegram_send(client: Client, api: String, chat: Value, reply_to: Option<i64>, outgoing: Outgoing) -> Result<()> {
    let response = match outgoing {
        Outgoing::Text(text) => client.post(format!("{api}/sendMessage")).json(&json!({ "chat_id": chat, "text": text, "reply_to_message_id": reply_to })).send().await?,
        Outgoing::File(file) => {
            // sendDocument preserves the original bytes and supports every file type.
            let mut form = multipart::Form::new().text("chat_id", chat.to_string()).part("document", part(file)?);
            if let Some(id) = reply_to { form = form.text("reply_to_message_id", id.to_string()); }
            client.post(format!("{api}/sendDocument")).multipart(form).send().await?
        }
    };
    let answer: Value = response.error_for_status()?.json().await?;
    if answer["ok"] != true { bail!("Telegram: {}", answer["description"]); }
    Ok(())
}

pub fn telegram_files(client: &Client, api: &str, base: &str, message: &Value) -> Vec<IncomingFile> {
    let mut files = Vec::new();
    for (key, default_name) in [("document", "file.bin"), ("video", "video.mp4"), ("audio", "audio.mp3"), ("voice", "voice.ogg"), ("animation", "animation.mp4"), ("photo", "image.jpg"), ("sticker", "sticker.webp")] {
        let value = if key == "photo" { message[key].as_array().and_then(|a| a.last()).unwrap_or(&Value::Null) } else { &message[key] };
        let Some(id) = value["file_id"].as_str() else { continue };
        let (client, api, base, id) = (client.clone(), api.to_string(), base.to_string(), id.to_string());
        files.push(IncomingFile { name: value["file_name"].as_str().unwrap_or(default_name).into(), size: value["file_size"].as_u64(), get: Box::pin(async move {
            let answer: Value = client.get(format!("{api}/getFile")).query(&[("file_id", id)]).send().await?.error_for_status()?.json().await?;
            if answer["ok"] != true { bail!("Telegram could not retrieve the attachment: {}", answer["description"]); }
            let path = answer["result"]["file_path"].as_str().ok_or_else(|| anyhow!("Telegram returned no file path"))?;
            if path.contains("..") || path.starts_with('/') || path.contains(['?', '#', '\\']) { bail!("Invalid Telegram file path"); }
            let bot = api.rsplit('/').next().unwrap_or_default();
            let url = format!("{}/file/{bot}/{path}", base.trim_end_matches('/'));
            follow_download(&client, client.get(url).send().await?, &base, &["telegram.org"]).await
        }) });
    }
    files
}

pub async fn feishu_send(client: &Client, domain: &str, access: &str, message_id: &str, outgoing: Outgoing) -> Result<()> {
    let (kind, content) = match outgoing {
        Outgoing::Text(text) => ("text", json!({ "text": text })),
        Outgoing::File(file) => {
            let image = matches!(file.mime.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp");
            let (endpoint, form) = if image {
                ("images", multipart::Form::new().text("image_type", "message").part("image", part(file)?))
            } else {
                let form = multipart::Form::new().text("file_type", "stream").text("file_name", file.name.clone()).part("file", part(file)?);
                ("files", form)
            };
            let uploaded: Value = client.post(format!("{domain}/open-apis/im/v1/{endpoint}")).bearer_auth(access).multipart(form).send().await?.error_for_status()?.json().await?;
            if uploaded["code"].as_i64() != Some(0) { bail!("Feishu upload: {}", uploaded["msg"]); }
            let key = if image { "image_key" } else { "file_key" };
            let value = uploaded["data"][key].as_str().ok_or_else(|| anyhow!("Feishu returned no media key"))?;
            (if image { "image" } else { "file" }, json!({ key: value }))
        }
    };
    let answer: Value = client.post(format!("{domain}/open-apis/im/v1/messages/{}/reply", urlencoding::encode(message_id))).bearer_auth(access)
        .json(&json!({ "msg_type": kind, "content": content.to_string() })).send().await?.error_for_status()?.json().await?;
    if answer["code"].as_i64() != Some(0) { bail!("Feishu: {}", answer["msg"]); }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn qq_send(client: &Client, api: &str, access: &str, path: &str, msg_id: &str, seq: u32, v2: bool, outgoing: Outgoing) -> Result<()> {
    let url = format!("{api}{path}");
    let auth = format!("QQBot {access}");
    let response = match outgoing {
        Outgoing::Text(text) => {
            let body = if v2 { json!({ "content": text, "msg_type": 0, "msg_id": msg_id, "msg_seq": seq }) } else { json!({ "content": text, "msg_id": msg_id }) };
            client.post(url).header("Authorization", auth).json(&body).send().await?
        }
        Outgoing::File(file) if v2 => {
            let kind = if file.mime.starts_with("image/") { 1 } else if file.mime.starts_with("video/") { 2 } else if file.mime.starts_with("audio/") { 3 } else { 4 };
            let uploaded: Value = client.post(format!("{api}{}/files", path.trim_end_matches("/messages"))).header("Authorization", &auth)
                .json(&json!({ "file_type": kind, "file_data": base64::engine::general_purpose::STANDARD.encode(file.bytes), "srv_send_msg": false })).send().await?.error_for_status()?.json().await?;
            let info = uploaded["file_info"].as_str().ok_or_else(|| anyhow!("QQ 不接受此附件（文件类型或应用权限受限）：{}", uploaded["message"]))?;
            client.post(url).header("Authorization", auth).json(&json!({ "content": file.name, "msg_type": 7, "media": { "file_info": info }, "msg_id": msg_id, "msg_seq": seq })).send().await?
        }
        Outgoing::File(file) => {
            if !file.mime.starts_with("image/") { bail!("QQ 频道及频道私信接口只允许图片附件；此文件请从 dotbot 网页下载。"); }
            let form = multipart::Form::new().text("msg_id", msg_id.to_string()).part("file_image", part(file)?);
            client.post(url).header("Authorization", auth).multipart(form).send().await?
        }
    };
    if !response.status().is_success() { bail!("QQ delivery: {}", response.text().await.unwrap_or_default()); }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn dingtalk_send(client: &Client, access: &str, legacy: &str, api: &str, robot: &str, recipient: &str, group: bool, outgoing: Outgoing) -> Result<()> {
    let Outgoing::File(file) = outgoing else { bail!("Expected a DingTalk file"); };
    let file_name = file.name.clone();
    let ext = file_name.rsplit('.').next().unwrap_or("bin").to_string();
    let uploaded: Value = client.post(format!("{legacy}/media/upload")).query(&[("access_token", access), ("type", "file")]).multipart(multipart::Form::new().part("media", part(file)?)).send().await?.error_for_status()?.json().await?;
    let media = uploaded["media_id"].as_str().ok_or_else(|| anyhow!("DingTalk upload: {}", uploaded["errmsg"]))?;
    let mut body = json!({ "robotCode": robot, "msgKey": "sampleFile", "msgParam": json!({ "mediaId": media, "fileName": file_name, "fileType": ext }).to_string() });
    let endpoint = if group { body["openConversationId"] = json!(recipient); "groupMessages/send" } else { body["userIds"] = json!([recipient]); "oToMessages/batchSend" };
    let answer: Value = client.post(format!("{api}/v1.0/robot/{endpoint}")).header("x-acs-dingtalk-access-token", access).json(&body).send().await?.error_for_status()?.json().await?;
    if answer.get("code").is_some_and(|code| code != "0" && code != 0) { bail!("DingTalk file: {}", answer["message"]); }
    if answer["success"].as_bool() == Some(false) || answer["invalidStaffIdList"].as_array().is_some_and(|a| !a.is_empty()) || answer["flowControlledStaffIdList"].as_array().is_some_and(|a| !a.is_empty()) { bail!("DingTalk did not accept the file delivery: {answer}"); }
    Ok(())
}

pub fn wechat_files(client: &Client, cdn: &str, items: &[Value]) -> Vec<IncomingFile> {
    items.iter().filter_map(|item| {
        let (field, fallback) = match item["type"].as_u64()? { 2 => ("image_item", "image.jpg"), 3 => ("voice_item", "voice.silk"), 4 => ("file_item", "file.bin"), 5 => ("video_item", "video.mp4"), _ => return None };
        let data = &item[field];
        let media = data["media"].clone();
        let (client, cdn) = (client.clone(), cdn.trim_end_matches('/').to_string());
        let aes_hex = data["aeskey"].as_str().map(String::from);
        Some(IncomingFile { name: data["file_name"].as_str().unwrap_or(fallback).into(), size: data["len"].as_str().and_then(|n| n.parse().ok()).or(data["len"].as_u64()), get: Box::pin(async move {
            let raw = media["full_url"].as_str().map(String::from).or_else(|| media["encrypt_query_param"].as_str().map(|p| format!("{cdn}/download?encrypted_query_param={}", urlencoding::encode(p)))).ok_or_else(|| anyhow!("WeChat returned no attachment address"))?;
            let url = checked_url(&raw, &cdn, &["weixin.qq.com"])?;
            let bytes = follow_download(&client, client.get(url).send().await?, &cdn, &["weixin.qq.com"]).await?;
            let key = if let Some(hex) = aes_hex { Some(hex::decode(hex).map_err(|_| anyhow!("Invalid WeChat media key"))?) }
            else if let Some(key) = media["aes_key"].as_str().filter(|k| !k.is_empty()) {
                let decoded = base64::engine::general_purpose::STANDARD.decode(key).map_err(|_| anyhow!("Invalid WeChat media key"))?;
                Some(if decoded.len() == 32 { hex::decode(decoded).map_err(|_| anyhow!("Invalid WeChat media key"))? } else { decoded })
            } else { None };
            match key { Some(key) => decrypt(&bytes, &key), None => Ok(bytes) }
        }) })
    }).collect()
}

fn encrypt(bytes: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let cipher = aes::Aes128::new_from_slice(key).map_err(|_| anyhow!("Invalid media key length"))?;
    let padding = 16 - bytes.len() % 16;
    let mut out = bytes.to_vec(); out.resize(out.len() + padding, padding as u8);
    for chunk in out.as_chunks_mut::<16>().0 {
        let mut block = aes::cipher::Block::<aes::Aes128>::default(); block.copy_from_slice(chunk);
        cipher.encrypt_block(&mut block); chunk.copy_from_slice(&block);
    }
    Ok(out)
}

fn decrypt(bytes: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    let cipher = aes::Aes128::new_from_slice(key).map_err(|_| anyhow!("Invalid media key length"))?;
    if bytes.is_empty() || !bytes.len().is_multiple_of(16) { bail!("Invalid encrypted attachment length"); }
    let mut out = bytes.to_vec();
    for chunk in out.as_chunks_mut::<16>().0 {
        let mut block = aes::cipher::Block::<aes::Aes128>::default(); block.copy_from_slice(chunk);
        cipher.decrypt_block(&mut block); chunk.copy_from_slice(&block);
    }
    let padding = *out.last().unwrap() as usize;
    if padding == 0 || padding > 16 || !out[out.len()-padding..].iter().all(|b| *b as usize == padding) { bail!("Invalid attachment encryption padding"); }
    out.truncate(out.len() - padding);
    Ok(out)
}

pub async fn wechat_item(client: &Client, base: &str, token: &str, cdn: &str, to: &str, outgoing: Outgoing) -> Result<Value> {
    let file = match outgoing { Outgoing::Text(text) => return Ok(json!({ "type": 1, "text_item": { "text": text } })), Outgoing::File(file) => file };
    use md5::Digest;
    let mut key = [0u8; 16]; getrandom::fill(&mut key).map_err(|_| anyhow!("Could not generate a media encryption key"))?;
    let encrypted = encrypt(&file.bytes, &key)?;
    let filekey = uuid::Uuid::new_v4().simple().to_string();
    // Sending everything as a file preserves original names and avoids lossy image processing.
    let answer = crate::channels::wechat_post(client, base, token, "getuploadurl", json!({ "filekey": filekey, "media_type": 3, "to_user_id": to, "rawsize": file.bytes.len(), "rawfilemd5": hex::encode(md5::Md5::digest(&file.bytes)), "filesize": encrypted.len(), "no_need_thumb": true, "aeskey": hex::encode(key), "base_info": { "channel_version": env!("CARGO_PKG_VERSION") } }), 60).await?;
    let raw = answer["upload_full_url"].as_str().map(String::from).or_else(|| answer["upload_param"].as_str().map(|p| format!("{}/upload?encrypted_query_param={}&filekey={filekey}", cdn.trim_end_matches('/'), urlencoding::encode(p)))).ok_or_else(|| anyhow!("WeChat did not return a media upload address"))?;
    let url = checked_url(&raw, cdn, &["weixin.qq.com"])?;
    let response = client.post(url).header("Content-Type", "application/octet-stream").body(encrypted).send().await?.error_for_status()?;
    let param = response.headers().get("x-encrypted-param").and_then(|v| v.to_str().ok()).ok_or_else(|| anyhow!("WeChat CDN returned no media reference"))?;
    Ok(json!({ "type": 4, "file_item": { "file_name": file.name, "len": file.bytes.len().to_string(), "md5": hex::encode(md5::Md5::digest(&file.bytes)), "media": { "encrypt_query_param": param, "aes_key": base64::engine::general_purpose::STANDARD.encode(key), "encrypt_type": 1 } } }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_crypto_matches_aes_and_rejects_invalid_padding() {
        let key = [0u8; 16];
        let ciphertext = encrypt(&[0u8; 16], &key).unwrap();
        assert_eq!(hex::encode(&ciphertext[..16]), "66e94bd4ef8a2c3b884cfa59ca342b2e");
        assert_eq!(decrypt(&ciphertext, &key).unwrap(), [0u8; 16]);
        assert!(decrypt(&[0; 3], &key).is_err());
        assert!(decrypt(&[0; 16], &key).is_err());
    }
    #[test]
    fn remote_files_cannot_target_arbitrary_servers_or_redirect_hosts() {
        assert!(checked_url("https://cdn.qq.com/file", "https://api.sgroup.qq.com", &["qq.com"]).is_ok());
        for url in ["http://127.0.0.1/file", "https://qq.com.evil.example/file", "https://evil.example/file", "https://x:secret@cdn.qq.com/file", "https://cdn.qq.com:1234/file"] {
            assert!(checked_url(url, "https://api.sgroup.qq.com", &["qq.com"]).is_err());
        }
    }

    #[tokio::test]
    async fn platform_uploads_use_real_multipart_and_deliver_returned_media_keys() {
        use axum::{extract::{Request, State}, response::IntoResponse, Json, Router};
        use std::sync::Arc;
        type Trace = Arc<tokio::sync::Mutex<Vec<(String, Vec<u8>, Option<String>)>>>;
        async fn mock(State(trace): State<Trace>, request: Request) -> axum::response::Response {
            let path = request.uri().path().to_string();
            let auth = request.headers().get("Authorization").and_then(|v| v.to_str().ok()).map(String::from);
            let body = axum::body::to_bytes(request.into_body(), 1_000_000).await.unwrap().to_vec();
            trace.lock().await.push((path.clone(), body, auth));
            if path == "/redirect-media" { return (axum::http::StatusCode::FOUND, [("location", "/download-media")]).into_response(); }
            if path == "/redirect-blocked" { return (axum::http::StatusCode::FOUND, [("location", "https://example.invalid/media")]).into_response(); }
            if path == "/download-media" { return b"downloaded bytes".to_vec().into_response(); }
            let value = if path.ends_with("/images") { json!({ "code": 0, "data": { "image_key": "image-token" } }) }
            else if path.ends_with("/open-apis/im/v1/files") { json!({ "code": 0, "data": { "file_key": "file-token" } }) }
            else if path == "/v2/users/u/files" { json!({ "file_info": "qq-media-token" }) }
            else if path == "/media/upload" { json!({ "errcode": 0, "media_id": "ding-media-token" }) }
            else { json!({ "code": 0, "id": "delivered", "processQueryKey": "delivery-query" }) };
            Json(value).into_response()
        }
        let trace = Trace::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(axum::serve(listener, Router::new().fallback(mock).with_state(trace.clone())).into_future());
        let client = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let make_file = |mime: &str| Outgoing::File(PlatformFile { name: "report.pdf".into(), mime: mime.into(), bytes: b"original\x00binary".to_vec() });
        let result: Result<()> = async {
            feishu_send(&client, &base, "test-access", "message", make_file("application/pdf")).await?;
            feishu_send(&client, &base, "test-access", "message", make_file("image/png")).await?;
            qq_send(&client, &base, "test-access", "/v2/users/u/messages", "message", 3, true, make_file("application/pdf")).await?;
            dingtalk_send(&client, "test-access", &base, &base, "robot", "group", true, make_file("application/pdf")).await?;
            let response = client.get(format!("{base}/redirect-media")).bearer_auth("private-token").send().await?;
            assert_eq!(follow_download(&client, response, &base, &[]).await?, b"downloaded bytes");
            let response = client.get(format!("{base}/redirect-blocked")).send().await?;
            assert!(follow_download(&client, response, &base, &[]).await.is_err());
            Ok(())
        }.await;
        server.abort(); let _ = server.await;
        result.unwrap();
        let trace = trace.lock().await;
        let bodies = |path: &str| trace.iter().filter(|(p, _, _)| p == path).map(|(_, b, _)| b.clone()).collect::<Vec<_>>();
        assert!(trace.iter().find(|(p, _, _)| p == "/download-media").unwrap().2.is_none());
        let replies = bodies("/open-apis/im/v1/messages/message/reply");
        let file: Value = serde_json::from_slice(&replies[0]).unwrap();
        assert_eq!(file["msg_type"], "file");
        assert_eq!(serde_json::from_str::<Value>(file["content"].as_str().unwrap()).unwrap()["file_key"], "file-token");
        let image: Value = serde_json::from_slice(&replies[1]).unwrap();
        assert_eq!(image["msg_type"], "image");
        assert_eq!(serde_json::from_str::<Value>(image["content"].as_str().unwrap()).unwrap()["image_key"], "image-token");
        assert!(bodies("/open-apis/im/v1/files")[0].windows(b"original\0binary".len()).any(|w| w == b"original\0binary"));
        let qq: Value = serde_json::from_slice(&bodies("/v2/users/u/messages")[0]).unwrap();
        assert_eq!(qq["media"]["file_info"], "qq-media-token"); assert_eq!(qq["msg_seq"], 3);
        let ding: Value = serde_json::from_slice(&bodies("/v1.0/robot/groupMessages/send")[0]).unwrap();
        assert_eq!(ding["openConversationId"], "group");
        assert_eq!(serde_json::from_str::<Value>(ding["msgParam"].as_str().unwrap()).unwrap()["mediaId"], "ding-media-token");
    }
}
