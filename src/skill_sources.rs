//! Discoverable public workflows. Downloads use a pinned Git tree and blob IDs;
//! installation copies resources without running scripts or package installers.
//!
//! Two sources: a few official `anthropics/skills` workflows, and the specialist skill
//! store (`anbeime/skill`), indexed at one commit in `skill_store.json` with every file's
//! Git blob ID. A handful of common skills install once by default; the rest install by
//! hand from Settings, or by a Bot when a delegated task needs one.
use std::{collections::BTreeMap, time::Duration};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use futures::{StreamExt, TryStreamExt};
use serde_json::{Value, json};
use crate::skills::Skills;

/// Official workflows installed once on first start. Store skills mark theirs in the index.
/// Official files come through the GitHub API (60 unauthenticated requests an hour, one per
/// file); docx/xlsx/pptx alone are ~170 files, so they install on demand instead.
const ANTHROPIC_COMMON: [&str; 1] = ["pdf"];

fn store() -> &'static Value {
    static STORE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    STORE.get_or_init(|| serde_json::from_str(include_str!("skill_store.json")).expect("valid skill store index"))
}

pub fn store_categories() -> Value { store()["categories"].clone() }

/// How well `query` matches `haystack`: whole words, or for Chinese (written without
/// spaces) shared two-character pieces.
fn relevance(words: &[String], haystack: &str) -> usize {
    words.iter().map(|w| {
        if haystack.contains(w.as_str()) { return 3; }
        let chars: Vec<char> = w.chars().filter(|c| !c.is_ascii()).collect();
        let shared = chars.windows(2).filter(|pair| haystack.contains(&pair.iter().collect::<String>())).count();
        if shared >= 2 { shared.min(3) } else { 0 }
    }).sum()
}

/// Store skills are installed automatically only for delegated work; elsewhere the user
/// installs them from Settings.
pub fn on_demand(source: &str) -> bool { source.starts_with("anbeime/") }

pub fn catalog(query: &str) -> Vec<Value> {
    let words: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut list = anthropic_catalog(&words);
    let index = store();
    let (repo, commit) = (index["repository"].as_str().unwrap(), index["commit"].as_str().unwrap());
    let categories = index["categories"].as_array().unwrap();
    for skill in index["skills"].as_array().unwrap() {
        let name = skill["name"].as_str().unwrap();
        let category = categories.iter().find(|c| c[0] == skill["category"]).and_then(|c| c[1].as_str()).unwrap_or("Other");
        let haystack = format!("{name} {} {} {category}", skill["title"].as_str().unwrap_or(""), skill["description"].as_str().unwrap_or("")).to_lowercase();
        let score = relevance(&words, &haystack);
        if !words.is_empty() && score == 0 { continue; }
        list.push(json!({"id":format!("anbeime/{name}"),"name":name,"title":skill["title"],"description":skill["description"],"category":skill["category"],"common":skill["common"],"autoInstall":"delegated","files":skill["files"].as_array().map_or(0, Vec::len),"repository":repo,"path":skill["path"],"url":format!("https://github.com/{repo}/tree/{commit}/{}", skill["path"].as_str().unwrap()),"score":score}));
    }
    if !words.is_empty() { list.sort_by(|a, b| b["score"].as_u64().cmp(&a["score"].as_u64())); }
    list
}

fn anthropic_catalog(words: &[String]) -> Vec<Value> {
    let entries = [
        ("pdf", "PDF documents", "PDF 提取 合并 拆分 表单 OCR extract merge split forms"),
        ("docx", "Word documents", "Word DOCX 文档 报告 修订 document report"),
        ("xlsx", "Spreadsheets", "Excel XLSX CSV 电子表格 数据 分析 spreadsheet data analysis"),
        ("pptx", "Presentations", "PowerPoint PPTX 幻灯片 演示文稿 presentation slides"),
        ("frontend-design", "Frontend design", "前端 界面 网页 设计 UI CSS frontend design"),
        ("webapp-testing", "Web application testing", "网页 应用 浏览器 测试 Playwright browser web testing"),
        ("mcp-builder", "MCP server development", "MCP 服务 工具 接入 集成 server tools integration"),
        ("skill-creator", "Skill authoring", "技能 编写 评估 改进 skill workflow authoring evaluation"),
        ("web-artifacts-builder", "Web artifacts", "网页 交互 组件 React web artifact components"),
        ("canvas-design", "Static visual design", "海报 视觉 排版 静态 设计 poster visual design"),
    ];
    entries.into_iter().filter_map(|(id,title,description)| {
        let haystack = format!("{id} {title} {description}").to_lowercase();
        let score = words.iter().filter(|w| haystack.contains(w.as_str()) || (w.chars().count()>2 && description.split_whitespace().any(|k| w.contains(&k.to_lowercase())))).count();
        if !words.is_empty() && score == 0 { return None; }
        Some(json!({"id":format!("anthropic/{id}"),"name":id,"title":title,"description":description,"category":"official","common":ANTHROPIC_COMMON.contains(&id),"autoInstall":"any","repository":"anthropics/skills","path":format!("skills/{id}"),"url":format!("https://github.com/anthropics/skills/tree/main/skills/{id}"),"score":score}))
    }).collect()
}

pub(crate) async fn fetch(client: &reqwest::Client, url: &str, limit: usize) -> Result<Value> {
    let response = client.get(url).send().await?;
    // GitHub answers 403/429 once the hourly API allowance is used up; say so plainly.
    let status = response.status().as_u16();
    if [403, 429].contains(&status) && response.headers().get("x-ratelimit-remaining").is_some_and(|v| v == "0") {
        let reset = response.headers().get("x-ratelimit-reset").and_then(|v| v.to_str().ok()?.parse::<i64>().ok())
            .and_then(|at| chrono::DateTime::from_timestamp(at, 0)).map(|at| at.with_timezone(&chrono::Local).format("%H:%M").to_string());
        match reset {
            Some(at) => bail!("GitHub's hourly request limit is used up; it resets at {at}. Set GITHUB_TOKEN to raise the limit."),
            None => bail!("GitHub's hourly request limit is used up. Try again later, or set GITHUB_TOKEN to raise the limit."),
        }
    }
    let mut response = response.error_for_status()?;
    if response.content_length().is_some_and(|size| size > limit as u64) { bail!("Skill source response is too large."); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > limit { bail!("Skill source response is too large."); }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Whether `source` is already installed (enabled) under its own name.
pub fn installed(skills: &Skills, source: &str) -> bool {
    catalog("").iter().find(|s| s["id"] == source)
        .and_then(|entry| skills.get(entry["name"].as_str().unwrap()).ok())
        .is_some_and(|existing| existing["enabled"] == true && existing["origin"]["source"] == source)
}

pub async fn install(skills: &Skills, source: &str, actor: Option<&str>) -> Result<String> {
    let entry = catalog("").into_iter().find(|s| s["id"] == source).ok_or_else(|| anyhow!("Unknown skill source. Search the skill catalog first."))?;
    if let Ok(existing)=skills.get(entry["name"].as_str().unwrap()) {
        if existing["enabled"]==true && existing["origin"]["source"]==source { return Ok(entry["name"].as_str().unwrap().into()); }
        bail!("A skill with this name already exists. Read it instead; disabled skills must be enabled in Settings.");
    }
    if on_demand(source) { return install_store(skills, source, &entry, actor).await; }
    let repo = entry["repository"].as_str().unwrap();
    let (files, revision) = github_bundle(repo, "main", entry["path"].as_str().unwrap()).await?;
    // License notices and provenance travel with the resource bundle.
    skills.install_bundle(entry["name"].as_str().unwrap(), &files, json!({"kind":"imported","source":source,"url":entry["url"],"repository":repo,"tree":revision,"by":actor,"created":crate::store::now_ms(),"validation":"untested"}))
}

/// The GitHub API base and a client for it. Only the server operator can replace the base
/// (also used by local fixtures); tool arguments cannot select arbitrary network hosts.
/// A GITHUB_TOKEN in the environment raises the 60-requests-an-hour anonymous limit.
pub(crate) fn github_api() -> Result<(String, reqwest::Client)> {
    let base = crate::branding::env("DOTBOT_SKILL_API_BASE").unwrap_or_else(|_| "https://api.github.com".into());
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var("GITHUB_TOKEN") && !token.trim().is_empty() {
        let mut value = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token.trim()))?;
        value.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    let client = reqwest::Client::builder().user_agent("dotbot-skills/1").default_headers(headers).redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(20)).build()?;
    Ok((base, client))
}

/// The recursive file tree of `repo` at `git_ref`, with its immutable tree ID.
pub(crate) async fn github_tree(repo: &str, git_ref: &str) -> Result<(Value, String)> {
    let (base, client) = github_api()?;
    let tree = fetch(&client, &format!("{base}/repos/{repo}/git/trees/{}?recursive=1", urlencoding::encode(git_ref)), 4_000_000).await.context("Fetching the skill source tree")?;
    if tree["truncated"] == true { bail!("Skill source tree was truncated."); }
    let revision = tree["sha"].as_str().filter(|s| s.len() == 40 && s.bytes().all(|c|c.is_ascii_hexdigit())).ok_or_else(|| anyhow!("Skill source has no immutable tree revision."))?.to_string();
    Ok((tree, revision))
}

/// Downloads every file under `dir` ("" for the repository root) plus the repository's
/// license notices. The whole tree is validated before any file is downloaded.
pub(crate) async fn github_bundle(repo: &str, git_ref: &str, dir: &str) -> Result<(BTreeMap<String, Vec<u8>>, String)> {
    let (tree, revision) = github_tree(repo, git_ref).await?;
    github_files(repo, git_ref, &tree, dir).await.map(|files| (files, revision))
}

/// Files come from raw.githubusercontent.com, which does not count against the API's hourly
/// allowance (one request per file used it up within a single large skill); each is checked
/// against its blob ID from the tree. Fixtures that only replace the API base use its blobs.
pub(crate) async fn github_files(repo: &str, git_ref: &str, tree: &Value, dir: &str) -> Result<BTreeMap<String, Vec<u8>>> {
    let (base, client) = github_api()?;
    let raw = crate::branding::env("DOTBOT_SKILL_RAW_BASE").ok()
        .or_else(|| crate::branding::env("DOTBOT_SKILL_API_BASE").is_err().then(|| "https://raw.githubusercontent.com".into()));
    let prefix = if dir.is_empty() { String::new() } else { format!("{dir}/") };
    let items = tree["tree"].as_array().ok_or_else(|| anyhow!("Skill source has no file tree."))?;
    // Hidden files (.github, .gitignore…) are repository plumbing, not skill resources.
    let hidden = |path: &str| path.split('/').any(|part| part.starts_with('.'));
    let mut resources: Vec<_> = items.iter().filter_map(|item| item["path"].as_str().and_then(|path|path.strip_prefix(&prefix)).map(|path|(path.to_string(),item))).filter(|(path,item)|item["type"] != "tree" && !hidden(path)).collect();
    for item in items {
        if let Some(path)=item["path"].as_str() && ["LICENSE","LICENSE.md","NOTICE"].contains(&path) { resources.push((format!("UPSTREAM-{path}"),item)); }
    }
    if resources.is_empty() || resources.len()>200 { bail!("The skill source has no files or exceeds the file limit."); }
    // Limit network fan-out; large workflows should not pay one round trip per
    // file serially. Validate the complete tree before downloading any resources.
    let resources: Vec<_> = resources.into_iter().map(|(path,item)| -> Result<_> {
        if !crate::skills::safe_relative(&path) || item["type"] != "blob" || !["100644","100755"].contains(&item["mode"].as_str().unwrap_or("")) { bail!("Skill source contains an unsafe path, link or submodule."); }
        if item["size"].as_u64().unwrap_or(0)>2_000_000 { bail!("Skill resource exceeds the size limit."); }
        let sha = item["sha"].as_str().filter(|s| s.len() == 40 && s.bytes().all(|c|c.is_ascii_hexdigit())).ok_or_else(|| anyhow!("Invalid skill blob ID."))?;
        Ok((path,item["path"].as_str().unwrap().to_owned(),sha.to_owned(),item["size"].as_u64().unwrap_or(0) as usize))
    }).collect::<Result<_>>()?;
    let encode = |p: &str| p.split('/').map(|part| urlencoding::encode(part).into_owned()).collect::<Vec<_>>().join("/");
    let downloads = futures::stream::iter(resources.into_iter().map(|(path,full,sha,size)| {
        let client=&client; let base=&base; let url=raw.as_ref().map(|raw| format!("{raw}/{repo}/{}/{}", encode(git_ref), encode(&full)));
        async move {
            if let Some(url) = url {
                let mut response = client.get(&url).send().await?.error_for_status()?;
                let mut bytes = Vec::with_capacity(size);
                while let Some(chunk) = response.chunk().await? {
                    if bytes.len() + chunk.len() > 2_000_000 { bail!("Skill resource exceeds the size limit."); }
                    bytes.extend_from_slice(&chunk);
                }
                if blob_id(&bytes) != sha { bail!("A skill file does not match the source tree."); }
                return Ok::<_,anyhow::Error>((path,bytes));
            }
            let blob = fetch(client, &format!("{base}/repos/{repo}/git/blobs/{sha}"), 2_800_000).await?;
            if blob["encoding"] != "base64" || blob["sha"] != sha { bail!("Skill source returned an unexpected blob."); }
            let encoded = blob["content"].as_str().ok_or_else(||anyhow!("Skill resource is missing."))?.replace(['\n','\r'], "");
            let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
            if bytes.len()>2_000_000 { bail!("Skill resource exceeds the size limit."); }
            Ok::<_,anyhow::Error>((path,bytes))
        }
    })).buffer_unordered(6);
    tokio::pin!(downloads);
    let mut files = BTreeMap::new(); let mut total = 0;
    while let Some((path,bytes))=downloads.try_next().await? {
        total += bytes.len(); if total>16_000_000 { bail!("Skill bundle exceeds the size limit."); }
        if files.insert(path,bytes).is_some() { bail!("Skill source contains a duplicate path."); }
    }
    Ok(files)
}

/// Downloads one store skill file by file at the indexed commit. Each file must match its
/// indexed size and Git blob ID, so the content is exactly what was reviewed into the index.
async fn install_store(skills: &Skills, source: &str, entry: &Value, actor: Option<&str>) -> Result<String> {
    let index = store();
    let name = entry["name"].as_str().unwrap();
    let skill = index["skills"].as_array().unwrap().iter().find(|s| s["name"] == name).ok_or_else(|| anyhow!("Unknown skill source."))?;
    let (repo, commit) = (index["repository"].as_str().unwrap(), index["commit"].as_str().unwrap());
    // Only the server operator can replace the download base (also used by local fixtures).
    let base = crate::branding::env("DOTBOT_SKILL_RAW_BASE").unwrap_or_else(|_| "https://raw.githubusercontent.com".into());
    let client = reqwest::Client::builder().user_agent("dotbot-skills/1").redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(30)).build()?;
    let path = skill["path"].as_str().unwrap();
    let wanted: Vec<(String, String, usize)> = skill["files"].as_array().unwrap().iter().map(|f| (f[0].as_str().unwrap().to_string(), f[1].as_str().unwrap().to_string(), f[2].as_u64().unwrap_or(0) as usize)).collect();
    if wanted.is_empty() || wanted.len() > 200 || wanted.iter().any(|(p, sha, size)| !crate::skills::safe_relative(p) || sha.len() != 40 || *size > 2_000_000) { bail!("The skill store entry is invalid."); }
    let encode = |p: &str| p.split('/').map(|part| urlencoding::encode(part).into_owned()).collect::<Vec<_>>().join("/");
    let downloads = futures::stream::iter(wanted.into_iter().map(|(file, sha, size)| {
        let client = &client; let url = format!("{base}/{repo}/{commit}/{}/{}", encode(path), encode(&file));
        async move {
            let mut response = client.get(&url).send().await?.error_for_status()?;
            let mut bytes = Vec::with_capacity(size);
            while let Some(chunk) = response.chunk().await? {
                if bytes.len() + chunk.len() > size { bail!("A skill file does not match the store index."); }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() != size || blob_id(&bytes) != sha { bail!("A skill file does not match the store index."); }
            Ok::<_, anyhow::Error>((file, bytes))
        }
    })).buffer_unordered(6);
    tokio::pin!(downloads);
    let mut files = BTreeMap::new(); let mut total = 0;
    while let Some((file, bytes)) = downloads.try_next().await.context("Downloading the skill from the store")? {
        total += bytes.len(); if total > 16_000_000 { bail!("Skill bundle exceeds the size limit."); }
        files.insert(file, bytes);
    }
    // Some store skills declare a display name (\"PDF Processing Pro\"); the installed
    // skill is named by its id so the engine's Skill tool can load it.
    if let Some(text) = files.get("SKILL.md").and_then(|b| std::str::from_utf8(b).ok()) {
        let renamed = rename_skill(text, name);
        files.insert("SKILL.md".into(), renamed.into_bytes());
    }
    skills.install_bundle(name, &files, json!({"kind":"imported","source":source,"url":entry["url"],"repository":repo,"tree":commit,"license":index["license"],"by":actor,"created":crate::store::now_ms(),"validation":"untested"}))
}

/// What the user sees when a manual install fails: a known message as is, otherwise the
/// network cause rather than only the outer step ("Downloading the skill from the store").
pub fn install_error(error: &anyhow::Error) -> String {
    if error.chain().count() == 1 { return error.to_string(); }
    if let Some(cause) = error.chain().find_map(|e| e.downcast_ref::<reqwest::Error>()) {
        if cause.is_timeout() { return "The skill source timed out. Check the network connection to GitHub and try again.".into(); }
        if let Some(status) = cause.status() { return format!("The skill source returned HTTP {}. Try again later.", status.as_u16()); }
        return "Could not reach the skill source. Check the network connection to GitHub and try again.".into();
    }
    format!("Skill install failed: {}", error.root_cause())
}

/// The Git blob ID of `bytes`: SHA-1 of "blob <length>\0" and the content.
fn blob_id(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    hex::encode(hash.finalize())
}

pub(crate) fn rename_skill(text: &str, name: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else { return text };
    let Some(end) = rest.find("\n---") else { return text };
    let head: Vec<String> = rest[..end].lines().map(|line| if line.starts_with("name:") { format!("name: {name}") } else { line.to_string() }).collect();
    format!("---\n{}{}", head.join("\n"), &rest[end..])
}

/// Installs the common skills once. A common skill the user later removes stays removed;
/// one that failed to download is tried again at the next start. Turned off with
/// DOTBOT_SKILL_DEFAULTS=0, and whenever a source base is overridden (test fixtures).
pub async fn install_defaults(skills: &Skills, home: &std::path::Path) {
    if crate::branding::env("DOTBOT_SKILL_DEFAULTS").is_ok_and(|v| ["0", "off", "false"].contains(&v.as_str()))
        || crate::branding::env("DOTBOT_SKILL_API_BASE").is_ok() || crate::branding::env("DOTBOT_SKILL_RAW_BASE").is_ok() { return; }
    let path = home.join("skill-store.json");
    let mut state: Value = crate::sqlite_storage::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_else(|| json!({"defaults":[]}));
    let done: Vec<String> = state["defaults"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
    let mut finished = done.clone();
    for entry in catalog("").into_iter().filter(|e| e["common"] == true) {
        let id = entry["id"].as_str().unwrap().to_string();
        if done.contains(&id) { continue; }
        // A skill of that name made another way counts as present.
        let present = skills.get(entry["name"].as_str().unwrap()).is_ok();
        match if present { Ok(String::new()) } else { tokio::time::timeout(Duration::from_secs(90), install(skills, &id, None)).await.unwrap_or_else(|_| Err(anyhow!("timed out"))) } {
            Ok(_) => finished.push(id),
            Err(error) => eprintln!("dotbot: default skill {id} not installed yet: {error}"),
        }
    }
    if finished != done {
        state["defaults"] = json!(finished);
        if let Err(error) = crate::durable::save(&path, &state) { eprintln!("dotbot: saving skill store state: {error}"); }
    }
}

/// The store page: every source with whether it is installed.
pub fn store_view(skills: &Skills) -> Value {
    let installed = skills.list();
    let rows: Vec<Value> = catalog("").into_iter().map(|mut entry| {
        let name = entry["name"].as_str().unwrap().to_string();
        let skill = installed.iter().find(|s| s["name"] == name);
        entry["state"] = json!(match skill {
            None => "available",
            Some(s) if s["origin"]["source"] != entry["id"] => "conflict",
            Some(s) if s["enabled"] != true => "disabled",
            Some(_) => "installed",
        });
        entry
    }).collect();
    json!({"categories":store_categories(),"skills":rows,"repository":store()["repository"],"commit":store()["commit"],"license":store()["license"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_store_index_is_valid_and_searchable() {
        let index = store();
        assert_eq!(index["commit"].as_str().unwrap().len(), 40);
        for skill in index["skills"].as_array().unwrap() {
            let name = skill["name"].as_str().unwrap();
            assert_eq!(crate::skills::clean_name(name).unwrap(), name);
            let files = skill["files"].as_array().unwrap();
            assert!(files.iter().any(|f| f[0] == "SKILL.md"), "{name}");
            assert!(files.len() <= 200 && files.iter().all(|f| crate::skills::safe_relative(f[0].as_str().unwrap()) && f[2].as_u64().unwrap() <= 2_000_000), "{name}");
        }
        let all = catalog("");
        let ids: std::collections::HashSet<_> = all.iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(ids.len(), all.len(), "store names must not repeat official ones");
        assert!(all.iter().filter(|e| e["common"] == true).count() >= 4);
        assert!(on_demand("anbeime/ppt-generator") && !on_demand("anthropic/pdf"));
        let found = catalog("制作带货短视频");
        assert!(found.first().is_some_and(|e| e["id"].as_str().unwrap().starts_with("anbeime/")), "Chinese search finds store skills");
        assert!(catalog("ppt-generator").iter().any(|e| e["id"] == "anbeime/ppt-generator"));
    }
    #[test]
    fn install_errors_name_the_cause() {
        let mismatch = anyhow!("A skill file does not match the store index.").context("Downloading the skill from the store");
        assert_eq!(install_error(&mismatch), "Skill install failed: A skill file does not match the store index.");
        assert_eq!(install_error(&anyhow!("Unknown skill source.")), "Unknown skill source.");
    }
    #[test]
    fn blob_ids_and_renames_match_git() {
        // `printf hello | git hash-object --stdin`
        assert_eq!(blob_id(b"hello"), "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0");
        assert_eq!(rename_skill("---\nname: PDF Processing Pro\ndescription: x\n---\nbody", "pdf-processing-pro"), "---\nname: pdf-processing-pro\ndescription: x\n---\nbody");
    }
}
