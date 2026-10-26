//! Independent, bounded parsers. These never execute code from a deliverable.
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub async fn parser(program: &str, args: &[&str]) -> Result<String> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(std::env::temp_dir())
        .env_remove("NODE_OPTIONS").env_remove("PYTHONPATH").env_remove("PYTHONSTARTUP").env_remove("BASH_ENV").env_remove("ENV")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0)
        .spawn()?;
    let pid = child.id();
    async fn drain(mut input: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
        let mut out = vec![];
        let mut buf = [0; 4096];
        while let Ok(n) = input.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let keep = n.min(32768usize.saturating_sub(out.len()));
            out.extend_from_slice(&buf[..keep]);
        }
        out
    }
    let out = tokio::spawn(drain(child.stdout.take().unwrap()));
    let err = tokio::spawn(drain(child.stderr.take().unwrap()));
    let status = tokio::time::timeout(Duration::from_secs(20), child.wait()).await;
    if let Some(pid) = pid {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe {
            kill(-(pid as i32), 9);
        }
    }
    let _ = child.wait().await;
    let output = String::from_utf8_lossy(&out.await?).to_string();
    let error = String::from_utf8_lossy(&err.await?).to_string();
    if !status??.success() {
        bail!("解析未通过：{}", crate::team::clip(&error, 500));
    }
    Ok(output)
}

pub async fn artifact(path: &Path, mime: &str) -> Value {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        return json!({"passed":false,"method":"file","reason":"文件为空或无法读取。"});
    }
    if size > 256_000_000 {
        return json!({"passed":null,"method":"size-limit","reason":"文件超过自动实检上限，未做解析检查。"});
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let filename = path.to_string_lossy();
    let result: Result<(String, String)> = async {
        if mime.starts_with("image/")
            || matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
        {
            let path = path.to_path_buf();
            let dimensions = tokio::task::spawn_blocking(move || -> Result<_> {
                let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(128_000_000);
                limits.max_image_width = Some(16384);
                limits.max_image_height = Some(16384);
                reader.limits(limits);
                let decoded = reader.decode()?;
                Ok((decoded.width(), decoded.height()))
            })
            .await??;
            return Ok((
                "image-decode".into(),
                format!("图像完整解码，{} × {} 像素。", dimensions.0, dimensions.1),
            ));
        }
        if mime.starts_with("video/")
            || mime.starts_with("audio/")
            || matches!(ext.as_str(), "mp4" | "webm" | "mov" | "mp3" | "wav")
        {
            let probe = parser(
                "ffprobe",
                &[
                    "-v",
                    "error",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-format_whitelist",
                    "mov,matroska,webm,mp3,wav,ogg,flac,avi,aac",
                    "-show_entries",
                    "format=duration:stream=codec_type,width,height",
                    "-of",
                    "json",
                    &filename,
                ],
            )
            .await?;
            let data: Value = serde_json::from_str(&probe)?;
            if data["streams"].as_array().is_none_or(|s| s.is_empty())
                || data["format"]["duration"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .is_none_or(|d| d <= 0.0)
            {
                bail!("没有可播放的媒体流或有效时长。");
            }
            parser(
                "ffmpeg",
                &[
                    "-v",
                    "error",
                    "-xerror",
                    "-protocol_whitelist",
                    "file,pipe",
                    "-format_whitelist",
                    "mov,matroska,webm,mp3,wav,ogg,flac,avi,aac",
                    "-i",
                    &filename,
                    "-t",
                    "10",
                    "-f",
                    "null",
                    "-",
                ],
            )
            .await?;
            return Ok((
                "media-decode-sample".into(),
                format!(
                    "媒体流有效，前 10 秒解码通过；时长 {} 秒。",
                    data["format"]["duration"].as_str().unwrap_or("")
                ),
            ));
        }
        if ext == "pdf" || mime == "application/pdf" {
            let info = parser("pdfinfo", &[&filename]).await?;
            parser("pdftotext", &["-f", "1", "-l", "3", &filename, "-"]).await?;
            return Ok((
                "pdf-parse".into(),
                format!(
                    "PDF 结构及前三页解析通过。{}",
                    crate::team::clip(&info, 350)
                ),
            ));
        }
        if matches!(ext.as_str(), "docx" | "pptx" | "xlsx" | "zip") {
            parser("unzip", &["-t", &filename]).await?;
            let entries = parser("unzip", &["-Z1", &filename]).await?;
            let required = match ext.as_str() {
                "docx" => "word/document.xml",
                "pptx" => "ppt/presentation.xml",
                "xlsx" => "xl/workbook.xml",
                _ => "",
            };
            if !required.is_empty() && !entries.lines().any(|e| e == required) {
                bail!("缺少文档主体。");
            }
            return Ok((
                "archive-integrity".into(),
                "压缩包完整性及文档主体检查通过；版式需预览核对。".into(),
            ));
        }
        if ext == "json" || mime == "application/json" {
            let bytes = tokio::fs::read(path).await?;
            let _: Value = serde_json::from_slice(&bytes)?;
            return Ok(("json-parse".into(), "JSON 解析通过。".into()));
        }
        if ext == "py" {
            parser(
                "python3",
                &[
                    "-I", "-c",
                    "import ast,sys; ast.parse(open(sys.argv[1],encoding='utf-8').read())",
                    &filename,
                ],
            )
            .await?;
            return Ok((
                "python-syntax".into(),
                "Python 语法检查通过；未执行程序。".into(),
            ));
        }
        if matches!(ext.as_str(), "js" | "mjs" | "cjs") {
            parser("node", &["--check", &filename]).await?;
            return Ok((
                "javascript-syntax".into(),
                "JavaScript 语法检查通过；未执行程序。".into(),
            ));
        }
        bail!("unsupported")
    }
    .await;
    match result {
        Ok((method, reason)) => json!({"passed":true,"method":method,"reason":reason}),
        Err(e) if e.to_string() == "unsupported" => {
            json!({"passed":null,"method":"manual","reason":"此格式尚无自动解析器，需按验收条件核对。"})
        }
        Err(e) => {
            json!({"passed":false,"method":"parser","reason":crate::team::clip(&e.to_string(),700)})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn verifies_real_video_stream_and_rejects_a_header_only_file(){
        let h=tempfile::tempdir().unwrap();let file=h.path().join("proof.mp4");let name=file.to_str().unwrap();
        parser("ffmpeg",&["-v","error","-f","lavfi","-i","color=c=blue:s=32x32:r=5","-t","0.4","-c:v","mpeg4",name]).await.unwrap();
        let result=artifact(&file,"video/mp4").await;assert_eq!(result["passed"],true,"{result}");
        std::fs::write(&file,b"fake-video-header").unwrap();assert_eq!(artifact(&file,"video/mp4").await["passed"],false);
    }
    #[tokio::test]
    async fn rejects_corrupt_artifacts_and_decodes_real_images() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().join("a.json");
        std::fs::write(&p, "{bad").unwrap();
        assert_eq!(artifact(&p, "application/json").await["passed"], false);
        std::fs::write(&p, "{\"ok\":true}").unwrap();
        assert_eq!(artifact(&p, "application/json").await["passed"], true);
        let p = home.path().join("a.png");
        image::RgbImage::new(5, 7).save(&p).unwrap();
        assert_eq!(artifact(&p, "image/png").await["passed"], true);
        std::fs::write(&p, "fake").unwrap();
        assert_eq!(artifact(&p, "image/png").await["passed"], false);
    }
}
