//! Installing a skill from a command or address the user pastes, in Settings or with
//! `dotbot skill install …`. The text is only parsed, never run: it names a store skill,
//! a GitHub repository or folder, or (in the terminal only) a local folder.
//!
//! Accepted forms:
//!   npx skills add owner/repo --skill name      npx skills add owner/repo@name
//!   https://github.com/owner/repo/tree/ref/path  owner/repo/path   owner/repo
//!   anbeime/ppt-generator  anthropic/pdf  ppt-generator (store ids and names)
//!   ./folder  /abs/folder  ~/folder (terminal only)
use std::{collections::BTreeMap, path::{Path, PathBuf}};
use anyhow::{Result, anyhow, bail};
use serde_json::json;
use crate::skills::Skills;

#[derive(Debug, PartialEq)]
pub enum Spec {
    /// An entry of the built-in catalog, by its id ("anbeime/ppt-generator").
    Catalog(String),
    GitHub { repo: String, git_ref: Option<String>, path: Option<String>, skill: Option<String> },
    Local(PathBuf),
}

/// Words of an installer command that are not the source itself.
const COMMAND_WORDS: [&str; 15] = ["npx", "pnpx", "bunx", "pnpm", "dlx", "yarn", "skills", "skills@latest", "add", "install", "dotbot", "teambot", "skill", "claude", "plugin"];

pub fn parse(input: &str) -> Result<Spec> {
    let words: Vec<String> = input.split_whitespace().map(|w| w.trim_matches(['"', '\'', '`']).to_string()).filter(|w| !w.is_empty()).collect();
    if words.is_empty() { bail!("Paste an install command, a GitHub address or a store skill name."); }
    let (mut skill, mut rest) = (None, Vec::new());
    let mut iter = words.into_iter();
    while let Some(word) = iter.next() {
        if let Some(value) = word.strip_prefix("--skill=") { skill = Some(value.to_string()); }
        else if ["--skill", "-s"].contains(&word.as_str()) { skill = iter.next(); }
        // Options that take a value (target agents) say nothing about the source.
        else if ["--agent", "-a"].contains(&word.as_str()) { iter.next(); }
        else if word.starts_with('-') {}
        else { rest.push(word); }
    }
    let target = rest.iter().find(|w| w.contains('/') || w.contains(':') || w.starts_with('~') || w.starts_with('.'))
        .or_else(|| rest.iter().rev().find(|w| !COMMAND_WORDS.contains(&w.as_str())))
        .ok_or_else(|| anyhow!("Paste an install command, a GitHub address or a store skill name."))?.clone();
    if target.starts_with('/') || target.starts_with("./") || target.starts_with("../") || target.starts_with('~') || target == "." {
        let path = match target.strip_prefix('~') { Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest.trim_start_matches('/')), None => PathBuf::from(&target) };
        return Ok(Spec::Local(path));
    }
    let catalog = crate::skill_sources::catalog("");
    if let Some(entry) = catalog.iter().find(|e| e["id"] == target.as_str() || (!target.contains('/') && e["name"] == target.as_str())) {
        return Ok(Spec::Catalog(entry["id"].as_str().unwrap().into()));
    }
    if !target.contains('/') { bail!("No store skill is named {target}. Use owner/repo or a GitHub address for other skills."); }
    let address = target.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let address = address.strip_prefix("github.com/").unwrap_or(address).trim_end_matches('/');
    if address.contains(':') || address.split('/').next().is_some_and(|host| host.contains('.')) { bail!("Only GitHub addresses can be installed from a command."); }
    let parts: Vec<&str> = address.split(['?', '#']).next().unwrap_or("").split('/').collect();
    if parts.len() < 2 { bail!("Use owner/repo or a GitHub address."); }
    let owner = parts[0];
    let (name, at) = match parts[1].split_once('@') { Some((name, at)) => (name, Some(at.to_string())), None => (parts[1], None) };
    let name = name.trim_end_matches(".git");
    let valid = |part: &str| !part.is_empty() && part != "." && part != ".." && part.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !valid(owner) || !valid(name) { bail!("Use owner/repo or a GitHub address."); }
    let skill = skill.or(at);
    let (git_ref, path) = match parts.get(2) {
        // github.com/o/r/tree/<ref>/<path> or …/blob/<ref>/<path>/SKILL.md; a ref containing
        // "/" cannot be told apart from the path here, so refs are taken as one segment.
        Some(&("tree" | "blob")) => {
            let git_ref = parts.get(3).filter(|r| !r.is_empty()).ok_or_else(|| anyhow!("Use owner/repo or a GitHub address."))?.to_string();
            let mut path = parts[4..].join("/");
            if parts.get(2) == Some(&"blob") || path.ends_with("SKILL.md") { path = path.rsplit_once('/').map(|(dir, _)| dir.to_string()).unwrap_or_default(); }
            (Some(git_ref), Some(path).filter(|p| !p.is_empty()))
        }
        Some(_) => (None, Some(parts[2..].join("/").trim_end_matches("/SKILL.md").to_string())),
        None => (None, None),
    };
    if git_ref.as_deref().is_some_and(|r| !r.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) || r.contains(".."))
        || path.as_deref().is_some_and(|p| p != "SKILL.md" && !p.split('/').all(|part| !part.is_empty() && part != "." && part != ".."))
    { bail!("Use owner/repo or a GitHub address."); }
    let path = path.filter(|p| p != "SKILL.md");
    let repo = format!("{owner}/{name}");
    // The same skill as a catalog entry keeps the catalog's pinned source and provenance.
    if let Some(wanted) = skill.as_deref().or(path.as_deref().and_then(|p| p.rsplit('/').next()))
        && git_ref.is_none()
        && let Some(entry) = catalog.iter().find(|e| e["repository"] == repo.as_str() && e["name"] == wanted && path.as_deref().is_none_or(|p| e["path"] == p)) {
        return Ok(Spec::Catalog(entry["id"].as_str().unwrap().into()));
    }
    Ok(Spec::GitHub { repo, git_ref, path, skill })
}

/// Which folder of `tree` holds the wanted skill ("" for the repository root).
fn choose_dir(tree: &serde_json::Value, path: Option<&str>, skill: Option<&str>) -> Result<String> {
    let dirs: Vec<String> = tree["tree"].as_array().into_iter().flatten()
        .filter(|item| item["type"] == "blob")
        .filter_map(|item| item["path"].as_str())
        .filter(|p| *p == "SKILL.md" || p.ends_with("/SKILL.md"))
        .map(|p| p.strip_suffix("SKILL.md").unwrap().trim_end_matches('/').to_string())
        .filter(|dir| !dir.split('/').any(|part| part.starts_with('.') && !dir.is_empty() && !matches!(part, ".claude" | ".agents")))
        .collect();
    if dirs.is_empty() { bail!("This repository has no SKILL.md."); }
    let base = |dir: &str| dir.rsplit('/').next().unwrap_or("").to_string();
    let under: Vec<&String> = dirs.iter().filter(|d| path.is_none_or(|p| *d == p || d.starts_with(&format!("{p}/")))).collect();
    if let Some(p) = path && under.iter().any(|d| *d == p) { return Ok(p.to_string()); }
    let picked: Vec<&String> = match skill {
        Some(name) => { let name = name.to_lowercase(); under.iter().copied().filter(|d| base(d).to_lowercase() == name || crate::skills::clean_name(&base(d)).ok().as_deref() == Some(&name)).collect() }
        None if under.iter().any(|d| d.is_empty()) => return Ok(String::new()),
        None => under.clone(),
    };
    match picked.as_slice() {
        [one] => Ok(one.to_string()),
        [] if skill.is_some() => bail!("No skill named {} is in this repository.", skill.unwrap()),
        [] => bail!("No SKILL.md is under {}.", path.unwrap_or("")),
        many => {
            if let Some(name) = skill && let Some(found) = many.iter().find(|d| **d == &format!("skills/{name}")) { return Ok(found.to_string()); }
            let names: Vec<String> = many.iter().take(12).map(|d| base(d)).collect();
            bail!("This repository has {} skills. Choose one with --skill <name>: {}", many.len(), names.join(", "))
        }
    }
}

/// The skill's own name from SKILL.md, made into a valid skill name; the file is
/// rewritten to carry it, since the engine loads a skill by that name.
fn named(files: &mut BTreeMap<String, Vec<u8>>, fallback: &str) -> Result<String> {
    let text = files.get("SKILL.md").and_then(|b| std::str::from_utf8(b).ok()).ok_or_else(|| anyhow!("The source has no SKILL.md."))?.replace("\r\n", "\n");
    let declared = text.strip_prefix("---\n").and_then(|rest| rest.split("\n---").next())
        .and_then(|head| head.lines().find_map(|line| line.strip_prefix("name:"))).map(|v| v.trim().trim_matches(['"', '\'']).to_string()).unwrap_or_default();
    let name = crate::skills::clean_name(if declared.is_empty() { fallback } else { &declared })?;
    files.insert("SKILL.md".into(), crate::skill_sources::rename_skill(&text, &name).into_bytes());
    Ok(name)
}

/// Installs what `input` names. `local` allows folders on this machine (the terminal);
/// the web page never reads host files.
pub async fn install(skills: &Skills, input: &str, local: bool) -> Result<String> {
    match parse(input)? {
        Spec::Catalog(id) => crate::skill_sources::install(skills, &id, None).await,
        Spec::Local(_) if !local => bail!("Install a local folder from the terminal: dotbot skill install <folder>"),
        Spec::Local(path) => install_local(skills, &path),
        Spec::GitHub { repo, git_ref, path, skill } => {
            let git_ref = match git_ref {
                Some(r) => r,
                None => {
                    let (base, client) = crate::skill_sources::github_api()?;
                    let info = crate::skill_sources::fetch(&client, &format!("{base}/repos/{repo}"), 200_000).await?;
                    info["default_branch"].as_str().filter(|b| !b.is_empty()).unwrap_or("main").to_string()
                }
            };
            let (tree, revision) = crate::skill_sources::github_tree(&repo, &git_ref).await?;
            let dir = choose_dir(&tree, path.as_deref(), skill.as_deref())?;
            let mut files = crate::skill_sources::github_files(&repo, &git_ref, &tree, &dir).await?;
            let fallback = if dir.is_empty() { repo.rsplit('/').next().unwrap() } else { dir.rsplit('/').next().unwrap() };
            let name = named(&mut files, fallback)?;
            if skills.get(&name).is_ok() { bail!("A skill with this name already exists."); }
            let url = format!("https://github.com/{repo}/tree/{git_ref}/{dir}");
            skills.install_bundle(&name, &files, json!({"kind":"imported","source":format!("github:{repo}/{dir}"),"url":url.trim_end_matches('/'),"repository":repo,"ref":git_ref,"tree":revision,"created":crate::store::now_ms(),"validation":"untested"}))
        }
    }
}

/// Copies a skill folder from this machine. Hidden files and folders (.git…) are left out;
/// links are refused so the bundle holds only what is inside the folder.
pub fn install_local(skills: &Skills, dir: &Path) -> Result<String> {
    let dir = dir.canonicalize().map_err(|_| anyhow!("No such folder: {}", dir.display()))?;
    let dir = if dir.is_file() && dir.file_name().is_some_and(|n| n == "SKILL.md") { dir.parent().unwrap().to_path_buf() } else { dir };
    if !dir.join("SKILL.md").is_file() { bail!("The folder has no SKILL.md."); }
    let mut files = BTreeMap::new();
    let mut total = 0;
    fn walk(root: &Path, at: &Path, files: &mut BTreeMap<String, Vec<u8>>, total: &mut usize) -> Result<()> {
        for entry in std::fs::read_dir(at)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with('.') { continue; }
            let kind = entry.file_type()?;
            if kind.is_symlink() { bail!("The skill folder contains a link: {}", entry.path().display()); }
            if kind.is_dir() { walk(root, &entry.path(), files, total)?; continue; }
            let bytes = crate::sqlite_storage::read(entry.path())?;
            *total += bytes.len();
            if bytes.len() > 2_000_000 || *total > 16_000_000 || files.len() >= 200 { bail!("Invalid or oversized skill bundle."); }
            files.insert(entry.path().strip_prefix(root)?.to_string_lossy().replace('\\', "/"), bytes);
        }
        Ok(())
    }
    walk(&dir, &dir, &mut files, &mut total)?;
    let name = named(&mut files, &dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())?;
    if skills.get(&name).is_ok() { bail!("A skill with this name already exists."); }
    skills.install_bundle(&name, &files, json!({"kind":"imported","source":format!("local:{}", dir.display()),"created":crate::store::now_ms(),"validation":"untested"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn gh(repo: &str, git_ref: Option<&str>, path: Option<&str>, skill: Option<&str>) -> Spec {
        Spec::GitHub { repo: repo.into(), git_ref: git_ref.map(Into::into), path: path.map(Into::into), skill: skill.map(Into::into) }
    }
    #[test]
    fn commands_and_addresses_parse() {
        assert_eq!(parse("npx skills add vercel-labs/agent-skills --skill web-design").unwrap(), gh("vercel-labs/agent-skills", None, None, Some("web-design")));
        assert_eq!(parse("npx -y skills@latest add owner/repo@thing -a claude-code -g").unwrap(), gh("owner/repo", None, None, Some("thing")));
        assert_eq!(parse("https://github.com/o/r/tree/dev/skills/foo").unwrap(), gh("o/r", Some("dev"), Some("skills/foo"), None));
        assert_eq!(parse("https://github.com/o/r/blob/main/skills/foo/SKILL.md").unwrap(), gh("o/r", Some("main"), Some("skills/foo"), None));
        assert_eq!(parse("github.com/o/r.git").unwrap(), gh("o/r", None, None, None));
        assert_eq!(parse("o/r/skills/foo").unwrap(), gh("o/r", None, Some("skills/foo"), None));
        assert_eq!(parse("dotbot skill install anbeime/ppt-generator").unwrap(), Spec::Catalog("anbeime/ppt-generator".into()));
        assert_eq!(parse("ppt-generator").unwrap(), Spec::Catalog("anbeime/ppt-generator".into()));
        assert_eq!(parse("npx skills add anthropics/skills --skill pdf").unwrap(), Spec::Catalog("anthropic/pdf".into()));
        assert_eq!(parse("./my-skill").unwrap(), Spec::Local("./my-skill".into()));
        for bad in ["", "npx skills add", "https://gitlab.com/o/r", "o/../r", "o/r/../../etc", "https://github.com/o/r/tree/a..b/x", "no-such-skill-here"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn the_skill_folder_is_chosen_from_the_tree() {
        let tree = json!({"tree":[{"path":"skills/a/SKILL.md","type":"blob"},{"path":"skills/b/SKILL.md","type":"blob"},{"path":".github/x/SKILL.md","type":"blob"}]});
        assert_eq!(choose_dir(&tree, None, Some("b")).unwrap(), "skills/b");
        assert_eq!(choose_dir(&tree, Some("skills/a"), None).unwrap(), "skills/a");
        assert!(choose_dir(&tree, None, None).unwrap_err().to_string().contains("2 skills"));
        assert!(choose_dir(&tree, None, Some("c")).is_err());
        let root = json!({"tree":[{"path":"SKILL.md","type":"blob"},{"path":"docs/SKILL.md","type":"blob"}]});
        assert_eq!(choose_dir(&root, None, None).unwrap(), "");
    }
    #[test]
    fn local_folders_install_without_hidden_files() {
        let home = tempfile::tempdir().unwrap();
        let skills = Skills::open(home.path()).unwrap();
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("SKILL.md"), "---\nname: My Report\ndescription: Weekly report\n---\nSteps").unwrap();
        std::fs::create_dir_all(src.path().join(".git")).unwrap();
        std::fs::write(src.path().join(".git/config"), "x").unwrap();
        std::fs::create_dir_all(src.path().join("scripts")).unwrap();
        std::fs::write(src.path().join("scripts/run.sh"), "echo hi").unwrap();
        assert_eq!(install_local(&skills, src.path()).unwrap(), "my-report");
        assert!(skills.resource("my-report", "scripts/run.sh").is_ok());
        assert!(install_local(&skills, src.path()).is_err(), "no silent overwrite");
    }
}
