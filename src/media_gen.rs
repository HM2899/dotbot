//! Image and video generation for the Bots' generate_image and generate_video tools, on the
//! image or video provider in use. Images go through the OpenAI-style images endpoint most
//! services offer (`<address>/images/generations`). A video is a job: started, then polled
//! until it is done, in the shape of the provider's format (智谱 by default).

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde_json::{json, Value};

use crate::providers::Provider;

const VIDEO_WAIT: Duration = Duration::from_secs(20 * 60);
const VIDEO_POLL: Duration = Duration::from_secs(5);

/// Generation work reports progress through activity events. Hold assistant drafts so
/// an internal prompt written before the tool call never becomes chat text.
pub(crate) fn private_image_turn(text: &str) -> bool {
    static IMAGE_TASK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    IMAGE_TASK.get_or_init(|| regex::Regex::new(r"(?i)generate_image|生图|画一|画个|画张|绘制|配图|海报|插画|生成.{0,16}(图片|图像|图|照片)|做.{0,8}(图片|图像)|来一张|\b(draw|illustrate)\b|\b(generate|create|make)\b.{0,40}\b(image|picture|photo|illustration|artwork)\b").unwrap()).is_match(text)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenerationKind { Image, Video }

pub(crate) fn private_generation_turn(text: &str) -> Option<GenerationKind> {
    static VIDEO_TASK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if VIDEO_TASK.get_or_init(|| regex::Regex::new(r"(?i)generate_video|生视频|文生视频|图生视频|生成.{0,16}(视频|短片|动画)|制作.{0,12}(视频|短片)|\b(generate|create|make)\b.{0,40}\b(video|clip|footage|animation)\b").unwrap()).is_match(text) {
        Some(GenerationKind::Video)
    } else if private_image_turn(text) { Some(GenerationKind::Image) } else { None }
}

pub(crate) fn public_generation_text(text: &str, prompts: &[String], kind: GenerationKind, success: bool) -> String {
    static PROMPT_LABEL: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if prompts.iter().any(|p| !p.is_empty() && text.contains(p)) || PROMPT_LABEL.get_or_init(|| regex::Regex::new(r#"(?i)(生图提示词|视频提示词|生视频提示词|负面提示词|生成参数|revised[_ ]prompt|negative[_ ]prompt|image[_ ]prompt|video[_ ]prompt|seed|sampler|guidance[_ ]scale|"prompt")\s*[:：]"#).unwrap()).is_match(text) {
        match (kind, success) {
            (GenerationKind::Image, true) => "图片已生成。",
            (GenerationKind::Image, false) => "图片生成未完成，请检查生图服务配置或稍后重试。",
            (GenerationKind::Video, true) => "视频已生成。",
            (GenerationKind::Video, false) => "视频生成未完成，请检查视频生成服务配置或稍后重试。",
        }.into()
    } else { text.into() }
}

/// Also scrub prompt captions saved by older releases, without altering uploads.
pub(crate) fn public_file(item: &mut Value) {
    let generated = item["generated"].as_str().is_some_and(|kind| ["image", "video"].contains(&kind))
        || item["name"].as_str().is_some_and(|name| {
            static LEGACY: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            LEGACY.get_or_init(|| regex::Regex::new(r"^(image|video)-\d{8}-\d{6}-[0-9a-f]{4}\.").unwrap()).is_match(name)
        });
    if item["kind"] == "file" && generated && let Some(object) = item.as_object_mut() {
        for key in ["caption", "prompt", "negative_prompt", "revised_prompt"] { object.remove(key); }
    }
    match item {
        Value::Array(values) => { for value in values { public_file(value); } }
        Value::Object(values) => { for value in values.values_mut() { public_file(value); } }
        _ => {}
    }
}

#[derive(Debug)]
pub struct Generated {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub model: String,
}

pub struct VideoAsk<'a> {
    pub prompt: &'a str,
    pub seconds: Option<u64>,
    pub size: Option<&'a str>,
    /// A picture the video starts from, for services that take one.
    pub image_url: Option<&'a str>,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(300)).build()?)
}

fn url(p: &Provider, path: &str) -> String {
    format!("{}/{path}", p.base_url.trim_end_matches('/'))
}

fn message_of(body: &Value) -> Option<String> {
    [&body["error"]["message"], &body["error"], &body["message"], &body["msg"], &body["reason"]].iter().find_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
}

async fn send(request: reqwest::RequestBuilder) -> Result<Value> {
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        bail!("The provider answered {status}: {}", message_of(&body).unwrap_or_else(|| text.chars().take(300).collect()));
    }
    Ok(body)
}

async fn download(request: reqwest::RequestBuilder) -> Result<Vec<u8>> {
    let response = request.send().await?;
    if !response.status().is_success() {
        bail!("The result could not be downloaded ({}).", response.status());
    }
    Ok(response.bytes().await?.to_vec())
}

/// The file type, from the first bytes.
fn ext_of(bytes: &[u8], fallback: &'static str) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "png"
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        "jpg"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.len() > 8 && &bytes[4..8] == b"ftyp" {
        "mp4"
    } else if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "webm"
    } else {
        fallback
    }
}

fn size_of<'a>(p: &'a Provider, asked: Option<&'a str>) -> Option<&'a str> {
    asked.filter(|s| !s.is_empty()).or(Some(p.size.as_str()).filter(|s| !s.is_empty()))
}

pub async fn image(p: &Provider, prompt: &str, size: Option<&str>) -> Result<Generated> {
    let model = p.media_model();
    let mut body = json!({ "model": model, "prompt": prompt, "n": 1 });
    if let Some(size) = size_of(p, size) {
        body["size"] = json!(size);
    }
    let answer = send(client()?.post(url(p, "images/generations")).bearer_auth(&p.api_key).json(&body)).await?;
    let first = &answer["data"][0];
    let bytes = if let Some(data) = first["b64_json"].as_str() {
        base64::engine::general_purpose::STANDARD.decode(data.trim()).context("the image came back unreadable")?
    } else if let Some(link) = first["url"].as_str() {
        download(client()?.get(link)).await?
    } else {
        bail!("The provider sent no image: {}", message_of(&answer).unwrap_or_else(|| answer.to_string().chars().take(300).collect()));
    };
    Ok(Generated { ext: ext_of(&bytes, "png"), bytes, model })
}

pub async fn video(p: &Provider, ask: &VideoAsk<'_>) -> Result<Generated> {
    video_polling(p, ask, VIDEO_POLL).await
}

async fn video_polling(p: &Provider, ask: &VideoAsk<'_>, poll: Duration) -> Result<Generated> {
    let model = p.media_model();
    let size = size_of(p, ask.size);
    let c = client()?;
    let format = p.format.as_str();
    let started = match format {
        "ark" => {
            let mut content = vec![json!({ "type": "text", "text": ask.prompt })];
            if let Some(image) = ask.image_url {
                content.push(json!({ "type": "image_url", "image_url": { "url": image } }));
            }
            send(c.post(url(p, "contents/generations/tasks")).bearer_auth(&p.api_key).json(&json!({ "model": model, "content": content }))).await?
        }
        "siliconflow" => {
            let mut body = json!({ "model": model, "prompt": ask.prompt });
            if let Some(image) = ask.image_url {
                body["image"] = json!(image);
            }
            if let Some(size) = size {
                body["image_size"] = json!(size);
            }
            send(c.post(url(p, "video/submit")).bearer_auth(&p.api_key).json(&body)).await?
        }
        _ => {
            let mut body = json!({ "model": model, "prompt": ask.prompt });
            if let Some(image) = ask.image_url {
                body["image_url"] = json!(image);
            }
            if let Some(size) = size {
                body["size"] = json!(size);
            }
            if let Some(seconds) = ask.seconds {
                body["duration"] = json!(seconds);
            }
            send(c.post(url(p, "videos/generations")).bearer_auth(&p.api_key).json(&body)).await?
        }
    };
    let id = [&started["id"], &started["requestId"]].iter().find_map(|v| v.as_str()).map(str::to_string).ok_or_else(|| anyhow!("The provider started no job: {}", message_of(&started).unwrap_or_else(|| started.to_string().chars().take(300).collect())))?;
    let since = std::time::Instant::now();
    loop {
        if since.elapsed() > VIDEO_WAIT {
            bail!("The video was not ready after 20 minutes (job {id}).");
        }
        tokio::time::sleep(poll).await;
        let request = match format {
            "ark" => c.get(url(p, &format!("contents/generations/tasks/{id}"))),
            "siliconflow" => c.post(url(p, "video/status")).json(&json!({ "requestId": id })),
            _ => c.get(url(p, &format!("async-result/{id}"))),
        };
        let job = send(request.bearer_auth(&p.api_key)).await?;
        let state = [&job["task_status"], &job["status"]].iter().find_map(|v| v.as_str()).unwrap_or_default().to_lowercase();
        if ["failed", "fail", "cancelled", "canceled", "expired"].contains(&state.as_str()) {
            bail!("The video failed: {}", message_of(&job).unwrap_or(state));
        }
        if !["completed", "succeeded", "success", "succeed"].contains(&state.as_str()) {
            continue;
        }
        let link = |v: &Value| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("The video is done but the provider gave no address for it."));
        let bytes = match format {
            "ark" => download(c.get(link(&job["content"]["video_url"])?)).await?,
            "siliconflow" => download(c.get(link(&job["results"]["videos"][0]["url"])?)).await?,
            _ => download(c.get(link(&job["video_result"][0]["url"])?)).await?,
        };
        return Ok(Generated { ext: ext_of(&bytes, "mp4"), bytes, model });
    }
}

/// Longest side and bytes of a picture handed back to the model; larger ones are scaled.
const SEEN_SIDE: u32 = 1568;
const SEEN_BYTES: usize = 3_500_000;

/// A generated image as the Bot gets it back.
pub struct Seen {
    pub mime: String,
    pub base64: String,
    /// Its size and main colours, in words.
    pub note: String,
}

pub fn seen(bytes: &[u8], mime: &str) -> Option<Seen> {
    let picture = image::load_from_memory(bytes).ok()?;
    let (w, h) = (picture.width(), picture.height());
    let note = format!("The attached image is {w}×{h} pixels. {}", colours(&picture));
    let (mime, data) = if w.max(h) <= SEEN_SIDE && bytes.len() <= SEEN_BYTES && ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(&mime) {
        (mime.to_string(), bytes.to_vec())
    } else {
        let mut out = std::io::Cursor::new(Vec::new());
        picture.resize(SEEN_SIDE, SEEN_SIDE, image::imageops::FilterType::Triangle).to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
        ("image/jpeg".to_string(), out.into_inner())
    };
    Some(Seen { mime, base64: base64::engine::general_purpose::STANDARD.encode(data), note })
}

/// The main colours of a picture: up to three, with their share, from a small copy.
fn colours(picture: &image::DynamicImage) -> String {
    let small = picture.thumbnail(64, 64).to_rgb8();
    let mut buckets: std::collections::HashMap<(u8, u8, u8), (u32, [u64; 3])> = Default::default();
    for p in small.pixels() {
        let slot = buckets.entry((p[0] >> 5, p[1] >> 5, p[2] >> 5)).or_default();
        slot.0 += 1;
        for i in 0..3 {
            slot.1[i] += p[i] as u64;
        }
    }
    let total = small.pixels().len().max(1) as f64;
    let mut list: Vec<(u32, [u8; 3])> = buckets.into_values().map(|(n, sum)| (n, [0, 1, 2].map(|i| (sum[i] / n as u64) as u8))).collect();
    list.sort_by_key(|item| std::cmp::Reverse(item.0));
    let named = |(n, c): &(u32, [u8; 3])| format!("{} #{:02X}{:02X}{:02X} ({:.0}%)", colour_name(*c), c[0], c[1], c[2], *n as f64 * 100.0 / total);
    if list[0].0 as f64 / total > 0.95 {
        return format!("It is almost a single colour: {}. A plain image is still the picture itself, not a missing one.", named(&list[0]));
    }
    format!("Main colours: {}.", list.iter().take(3).map(named).collect::<Vec<_>>().join(", "))
}

fn colour_name([r, g, b]: [u8; 3]) -> &'static str {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    if l < 0.12 {
        return "black";
    }
    if l > 0.92 {
        return "white";
    }
    if s < 0.15 {
        return "grey";
    }
    let hue = if d == 0.0 { 0.0 } else if max == r { 60.0 * (((g - b) / d) % 6.0) } else if max == g { 60.0 * ((b - r) / d + 2.0) } else { 60.0 * ((r - g) / d + 4.0) };
    let hue = (hue + 360.0) % 360.0;
    match hue {
        h if !(15.0..345.0).contains(&h) => if l < 0.3 { "dark red" } else { "red" },
        h if h < 45.0 => if l < 0.35 { "brown" } else { "orange" },
        h if h < 70.0 => "yellow",
        h if h < 165.0 => "green",
        h if h < 200.0 => "cyan",
        h if h < 255.0 => "blue",
        h if h < 290.0 => "purple",
        _ => "pink",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generated_prompts_stay_out_of_public_text_and_legacy_captions() {
        assert!(private_image_turn("请生成一张图片"));
        assert!(private_image_turn("create an image of a cat"));
        assert!(!private_image_turn("请核对今天的账单"));
        assert_eq!(private_generation_turn("请用这张图片生成一段视频"), Some(GenerationKind::Video));
        assert_eq!(private_generation_turn("make a video of a cat"), Some(GenerationKind::Video));
        assert_eq!(private_generation_turn("请核对今天的账单"), None);
        assert_eq!(public_generation_text("video prompt: secret", &[], GenerationKind::Video, true), "视频已生成。");
        assert!(public_generation_text("seed: 123", &[], GenerationKind::Video, false).starts_with("视频生成未完成"));
        assert_eq!(public_generation_text("prompt: secret-render-text", &["secret-render-text".into()], GenerationKind::Image, true), "图片已生成。");
        assert!(!public_generation_text("image prompt: unfinished", &[], GenerationKind::Image, false).contains("unfinished"));
        let mut old = json!({"kind":"file","name":"image-20261024-123456-ab12.png","caption":"private prompt","src":"media/image.png"});
        public_file(&mut old);
        assert!(old.get("caption").is_none());
        assert_eq!(old["src"], "media/image.png");
        let mut upload = json!({"kind":"file","name":"image-1.png","caption":"User's own description"});
        public_file(&mut upload);
        assert_eq!(upload["caption"], "User's own description");
    }
    use axum::extract::Path;
    use axum::routing::{get, post};
    use axum::{Json, Router};

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-picture";
    const MP4: &[u8] = b"\x00\x00\x00\x18ftypmp42fake-video";

    /// One fake service per format, under its own path.
    async fn serve() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let file = |name: &str| format!("{base}/files/{name}");
        let (png, mp4) = (file("a.png"), file("a.mp4"));
        let (zp, ark, sf) = (mp4.clone(), mp4.clone(), mp4.clone());
        let app = Router::new()
            .route("/files/{name}", get(|Path(name): Path<String>| async move { if name.ends_with(".png") { PNG.to_vec() } else { MP4.to_vec() } }))
            .route("/img/images/generations", post(move |Json(body): Json<Value>| {
                let png = png.clone();
                async move { Json(if body["model"] == "url-model" { json!({ "data": [{ "url": png }] }) } else { json!({ "data": [{ "b64_json": base64::engine::general_purpose::STANDARD.encode(PNG) }] }) }) }
            }))
            .route("/zp/videos/generations", post(|| async { Json(json!({ "id": "z1", "task_status": "PROCESSING" })) }))
            .route("/zp/async-result/{id}", get(move || { let u = zp.clone(); async move { Json(json!({ "task_status": "SUCCESS", "video_result": [{ "url": u }] })) } }))
            .route("/ark/contents/generations/tasks", post(|| async { Json(json!({ "id": "a1" })) }))
            .route("/ark/contents/generations/tasks/{id}", get(move || { let u = ark.clone(); async move { Json(json!({ "status": "succeeded", "content": { "video_url": u } })) } }))
            .route("/sf/video/submit", post(|| async { Json(json!({ "requestId": "s1" })) }))
            .route("/sf/video/status", post(move || { let u = sf.clone(); async move { Json(json!({ "status": "Succeed", "results": { "videos": [{ "url": u }] } })) } }))
            .route("/bad/videos/generations", post(|| async { Json(json!({ "id": "b1" })) }))
            .route("/bad/async-result/{id}", get(|| async { Json(json!({ "task_status": "FAIL", "error": { "message": "blocked by policy" } })) }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    fn provider(base: &str, path: &str, kind: &str, format: &str, model: &str) -> Provider {
        Provider { kind: kind.into(), base_url: format!("{base}/{path}"), api_key: "k".into(), format: format.into(), models: vec![model.into()], ..Default::default() }
    }

    #[tokio::test]
    async fn every_format_makes_a_file() {
        let base = serve().await;
        let ask = VideoAsk { prompt: "a cat", seconds: Some(4), size: None, image_url: None };
        let quick = Duration::from_millis(10);
        for (path, format) in [("zp", "zhipu"), ("ark", "ark"), ("sf", "siliconflow")] {
            let made = video_polling(&provider(&base, path, "video", format, "m"), &ask, quick).await.unwrap();
            assert_eq!((made.ext, made.bytes.as_slice()), ("mp4", MP4), "{format}");
        }
        let failed = video_polling(&provider(&base, "bad", "video", "zhipu", "m"), &ask, quick).await.unwrap_err();
        assert!(failed.to_string().contains("blocked by policy"));
        for model in ["b64-model", "url-model"] {
            let made = image(&provider(&base, "img", "image", "", model), "a cat", None).await.unwrap();
            assert_eq!((made.ext, made.bytes.as_slice()), ("png", PNG), "{model}");
        }
    }

    #[test]
    fn a_plain_image_is_described_and_a_large_one_scaled() {
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(256, 256, image::Rgb([255, 165, 0])).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let plain = seen(png.get_ref(), "image/png").unwrap();
        assert_eq!(plain.mime, "image/png");
        assert!(plain.note.contains("256×256") && plain.note.contains("single colour: orange #"), "{}", plain.note);
        let mut big = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_fn(3000, 1000, |x, _| if x < 1500 { image::Rgb([0, 0, 255]) } else { image::Rgb([0, 200, 0]) }).write_to(&mut big, image::ImageFormat::Png).unwrap();
        let scaled = seen(big.get_ref(), "image/png").unwrap();
        assert_eq!(scaled.mime, "image/jpeg");
        assert!(scaled.note.contains("3000×1000") && scaled.note.contains("blue") && scaled.note.contains("green"), "{}", scaled.note);
        assert!(seen(b"not a picture", "image/png").is_none());
    }
}
