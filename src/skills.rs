//! The team's skills, as one skill bundle: `<home>/plugin/skills/<name>/SKILL.md`.
//! Every turn lists it to the native engine, whose Skill tool finds and uses the
//! skills. A skill is made by a Bot (save_skill), by the user, or from a task the user
//! showed once in a Bot's browser ("teach a task").

use std::{collections::BTreeMap, path::{Component, Path, PathBuf}};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

#[path="skill_candidates.rs"]
mod candidates;
pub use candidates::is_regression_request;

pub struct Skills {
    dir: PathBuf,
    writing: std::sync::Mutex<()>,
    profiles: std::sync::Mutex<BTreeMap<String, Vec<Value>>>,
}

pub fn clean_name(raw: &str) -> Result<String> {
    let name: String = raw.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let name = name.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    // Skill names stay in a-z, 0-9 and "-"; a name in another script gets a
    // stable one from its hash, and save() keeps the original in the description.
    let name = if name.is_empty() && !raw.trim().is_empty() { format!("skill-{}", &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(raw.trim().as_bytes()))[..8]) } else { name };
    if name.is_empty() || name.len() > 48 {
        bail!("A skill's name is a few words in letters and digits, such as \"weekly-report\".");
    }
    Ok(name)
}

pub(crate) fn front_matter(text: &str) -> (String, String) {
    let mut name = String::new();
    let mut description = String::new();
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---") {
            let lines: Vec<_> = rest[..end].lines().collect();
            for (at,line) in lines.iter().enumerate() {
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().trim_matches(['"','\'']).to_string();
                } else if let Some(v) = line.strip_prefix("description:") {
                    let v = v.trim();
                    description = if [">",">-","|","|-"].contains(&v) {
                        lines[at+1..].iter().take_while(|line|line.starts_with([' ','\t'])).map(|line|line.trim()).collect::<Vec<_>>().join(" ")
                    } else { serde_json::from_str::<String>(v).unwrap_or_else(|_| v.trim_matches(['"','\'']).to_string()) };
                }
            }
        }
    (name, description)
}

pub(crate) fn body_of(text: &str) -> &str {
    text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n").map(|(_, body)| body.trim())).unwrap_or(text.trim())
}

fn revision(text: &str) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()))
}

/// The revision covers every instruction, script and resource; metadata is separate.
fn bundle_files(root:&Path)->Result<BTreeMap<String,Vec<u8>>>{
    fn visit(root:&Path,dir:&Path,out:&mut BTreeMap<String,Vec<u8>>,total:&mut usize,depth:usize)->Result<()>{
        if depth>12{bail!("技能资源目录层级过深。");}
        for entry in std::fs::read_dir(dir)?{let entry=entry?;let name=entry.file_name();if name.to_string_lossy().starts_with('.') {continue;}
            let kind=entry.file_type()?;if kind.is_symlink(){bail!("技能包不能包含符号链接。");}
            if kind.is_dir(){visit(root,&entry.path(),out,total,depth+1)?;}else if kind.is_file(){let len=entry.metadata()?.len();if len>2_000_000||*total+len as usize>16_000_000||out.len()>=200{bail!("技能包超过限额。");}let bytes=crate::sqlite_storage::read(entry.path())?;*total+=bytes.len();out.insert(entry.path().strip_prefix(root)?.to_string_lossy().into(),bytes);}
        }Ok(())
    }
    let mut out=BTreeMap::new();visit(root,root,&mut out,&mut 0,0)?;Ok(out)
}
fn bundle_revision(root:&Path)->Result<String>{
    use sha2::Digest;let mut digest=sha2::Sha256::new();
    for(name,bytes)in bundle_files(root)?{digest.update((name.len() as u64).to_be_bytes());digest.update(name.as_bytes());digest.update((bytes.len() as u64).to_be_bytes());digest.update(bytes);}
    Ok(hex::encode(digest.finalize()))
}

fn title_of(text: &str, fallback: &str) -> String {
    text.strip_prefix("---\n").and_then(|rest| rest.split_once("\n---\n")).and_then(|(head, _)| head.lines().find_map(|line| line.strip_prefix("title:").and_then(|v| serde_json::from_str::<String>(v.trim()).ok()))).unwrap_or_else(|| fallback.into())
}

impl Skills {
    pub fn open(home: &Path) -> Result<Self> {
        let dir = home.join("plugin");
        let manifest = dir.join(".claude-plugin").join("plugin.json");
        if !manifest.exists() {
            std::fs::create_dir_all(manifest.parent().unwrap())?;
            std::fs::write(&manifest, serde_json::to_string_pretty(&json!({ "name": "dotbot-skills", "version": "1.0.0", "description": "Skills the dotbot team made and shares" }))?)?;
        }
        std::fs::create_dir_all(dir.join("skills"))?;
        std::fs::create_dir_all(home.join("disabled-skills"))?;
        let profiles = match crate::sqlite_storage::read_to_string(home.join("skill-profiles.json")) {
            Ok(text) => serde_json::from_str(&text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Skills { dir, writing: std::sync::Mutex::new(()), profiles: std::sync::Mutex::new(profiles) })
    }

    pub fn plugin_dir(&self) -> &Path {
        &self.dir
    }

    pub fn list(&self) -> Vec<Value> {
        let _guard = self.writing.lock().unwrap();
        self.list_unlocked()
    }

    fn list_unlocked(&self) -> Vec<Value> {
        let mut skills: Vec<Value> = [true, false].into_iter().flat_map(|enabled| {
            std::fs::read_dir(self.root(enabled)).into_iter().flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                if !e.file_type().ok()?.is_dir() { return None; }
                let text = crate::sqlite_storage::read_to_string(e.path().join("SKILL.md")).ok()?;
                let (_, description) = front_matter(&text);
                let folder = e.file_name().into_string().ok()?;
                let origin: Value = crate::sqlite_storage::read_to_string(e.path().join(".teambot.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({"kind":"local"}));
                let bundle=bundle_revision(&e.path());
                let quality:Value=crate::sqlite_storage::read(e.path().join(".quality.json")).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
                Some(json!({ "bundleValid":bundle.is_ok(),"bundleError":bundle.as_ref().err().map(ToString::to_string),"quality":quality,"name": folder, "folder": folder, "title": title_of(&text, &folder), "description": description, "body": body_of(&text), "revision": bundle.unwrap_or_else(|_| revision(&text)), "enabled": enabled, "text": text, "origin":origin }))
            }).collect::<Vec<_>>()
        }).collect();
        skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        skills
    }

    fn root(&self, enabled: bool) -> PathBuf {
        if enabled { self.dir.join("skills") } else { self.dir.parent().unwrap().join("disabled-skills") }
    }

    pub fn get(&self, name: &str) -> Result<Value> {
        let name = clean_name(name)?;
        self.list().into_iter().find(|s| s["name"] == name).ok_or_else(|| anyhow!("No skill named {name}"))
    }

    pub fn profile(&self, bot: &str) -> Vec<Value> {
        self.profiles.lock().unwrap().get(bot).cloned().unwrap_or_default()
    }

    pub fn specialties(&self, bot: &crate::store::Bot) -> Vec<Value> {
        let profile = self.profile(&bot.id);
        self.list().into_iter().filter(|s| bot.skills.iter().any(|n| s["name"] == *n) || profile.iter().any(|p| p["name"] == s["name"])).map(|s| {
            let loaded = profile.iter().find(|p| p["name"] == s["name"]).cloned().unwrap_or_else(|| json!({}));
            let evaluations=loaded["evaluations"].as_array().cloned().unwrap_or_default();
            let current:Vec<_>=evaluations.iter().filter(|e|e["revision"]==s["revision"]).collect();
            json!({"name":s["name"],"title":s["title"],"description":s["description"],"enabled":s["enabled"],"pinned":bot.skills.iter().any(|n| s["name"] == *n),"loads":loaded["loads"].as_u64().unwrap_or(0),"verifiedPasses":current.iter().filter(|e|e["passed"]==true && e["evidence"].as_str().is_some_and(|v|matches!(v,"tool-supported-review"|"artifact-validation"))).count(),"verifiedFailures":current.iter().filter(|e|e["passed"]==false).count(),"reviewedPasses":current.iter().filter(|e|e["passed"]==true).count(),"evaluations":evaluations,"lastLoaded":loaded["lastLoaded"],"lastOutcome":loaded["lastOutcome"],"origin":s["origin"]})
        }).collect()
    }

    pub fn note_loaded(&self, bot: &str, name: &str) -> Result<()> {
        let skill = self.get(name)?;
        if skill["enabled"] != true || skill["bundleValid"]==false { bail!("技能已停用或技能包验证未通过。"); }
        self.versions(name)?;
        let mut guard = self.profiles.lock().unwrap();
        let mut next = guard.clone();
        let entries = next.entry(bot.into()).or_default();
        if let Some(entry) = entries.iter_mut().find(|p| p["name"] == name) {
            entry["loads"] = json!(entry["loads"].as_u64().unwrap_or(0) + 1);
            entry["lastLoaded"] = json!(crate::store::now_ms());
            entry["lastOutcome"] = json!("prepared");
        } else {
            entries.push(json!({"name":name,"loads":1,"lastLoaded":crate::store::now_ms(),"lastOutcome":"prepared"}));
        }
        crate::durable::save(&self.dir.parent().unwrap().join("skill-profiles.json"), &next)?;
        *guard = next;
        Ok(())
    }

    pub fn note_outcome(&self, bot: &str, names: &[String], outcome: &str) -> Result<()> {
        if names.is_empty() { return Ok(()); }
        let mut guard = self.profiles.lock().unwrap();
        let mut next = guard.clone();
        for entry in next.entry(bot.into()).or_default().iter_mut().filter(|p| names.iter().any(|n| p["name"] == *n)) { entry["lastOutcome"] = json!(outcome); }
        crate::durable::save(&self.dir.parent().unwrap().join("skill-profiles.json"), &next)?;
        *guard = next;
        Ok(())
    }

    /// Quality comes from explicit acceptance checks, never from load counts or
    /// a model process merely exiting successfully. Receipts deduplicate retries.
    pub fn note_evaluation(&self,bot:&str,names:&[String],revisions:&BTreeMap<String,String>,receipt:&str,passed:bool,evidence:&str)->Result<()> {
        let mut guard=self.profiles.lock().unwrap();let mut next=guard.clone();
        for name in names {
            let Some(revision)=revisions.get(name)else{continue;};
            let entries=next.entry(bot.into()).or_default();
            if !entries.iter().any(|e|e["name"]==*name){entries.push(json!({"name":name,"loads":0}));}
            let entry=entries.iter_mut().find(|e|e["name"]==*name).unwrap();
            if !entry["evaluations"].is_array(){entry["evaluations"]=json!([]);}
            let evaluations=entry["evaluations"].as_array_mut().unwrap();
            if evaluations.iter().any(|e|e["receipt"]==receipt){continue;}
            evaluations.push(json!({"receipt":receipt,"revision":revision,"passed":passed,"evidence":evidence,"at":crate::store::now_ms()}));
            if evaluations.len()>200{evaluations.remove(0);}
            entry["verifiedPasses"]=json!(entry["evaluations"].as_array().unwrap().iter().filter(|e|e["passed"]==true && e["evidence"].as_str().is_some_and(|v|matches!(v,"tool-supported-review"|"artifact-validation"))).count());
            entry["verifiedFailures"]=json!(entry["evaluations"].as_array().unwrap().iter().filter(|e|e["passed"]==false).count());
        }
        crate::durable::save(&self.dir.parent().unwrap().join("skill-profiles.json"),&next)?;*guard=next;drop(guard);
        self.quarantine_regressions(names)

    }

    fn archive_unlocked(&self,skill:&Value)->Result<()> {
        let name=skill["name"].as_str().ok_or_else(||anyhow!("Invalid skill"))?;
        let revision=skill["revision"].as_str().unwrap_or("");
        let file=self.dir.parent().unwrap().join("skill-history").join(name).join(format!("{revision}.json"));
        if file.exists(){return Ok(());}
        let root=self.root(skill["enabled"]==true).join(name);
        fn collect(root:&Path,dir:&Path,files:&mut BTreeMap<String,String>,total:&mut usize)->Result<()> {
            use base64::Engine;
            for entry in std::fs::read_dir(dir)? {
                let entry=entry?;let kind=entry.file_type()?;
                if kind.is_symlink(){bail!("技能包不能包含符号链接。");}
                if entry.file_name().to_string_lossy().starts_with('.') && entry.file_name()!=".teambot.json"{continue;}
                if kind.is_dir(){collect(root,&entry.path(),files,total)?;}
                else if kind.is_file(){let bytes=crate::sqlite_storage::read(entry.path())?;*total+=bytes.len();if bytes.len()>2_000_000||*total>16_000_000||files.len()>=200{bail!("技能版本附件超出限额。");}files.insert(entry.path().strip_prefix(root)?.to_string_lossy().into(),base64::engine::general_purpose::STANDARD.encode(bytes));}
            }Ok(())
        }
        let mut files=BTreeMap::new();collect(&root,&root,&mut files,&mut 0)?;
        std::fs::create_dir_all(file.parent().unwrap())?;
        crate::durable::save(&file,&json!({"name":name,"revision":revision,"created":crate::store::now_ms(),"description":skill["description"],"files":files,"format":"bundle-v2"}))
    }

    pub fn versions(&self,name:&str)->Result<Vec<Value>> {
        let _guard=self.writing.lock().unwrap();let name=clean_name(name)?;
        let current=self.list_unlocked().into_iter().find(|s|s["name"]==name).ok_or_else(||anyhow!("找不到技能。"))?;
        self.archive_unlocked(&current)?;
        let mut versions:Vec<Value>=std::fs::read_dir(self.dir.parent().unwrap().join("skill-history").join(&name))?.filter_map(Result::ok).filter_map(|e|crate::sqlite_storage::read(e.path()).ok()).filter_map(|b|serde_json::from_slice::<Value>(&b).ok()).map(|v|json!({"revision":v["revision"],"created":v["created"],"description":v["description"],"current":v["revision"]==current["revision"],"format":v["format"],"runs":self.benchmark_runs_unlocked(&name,v["revision"].as_str().unwrap_or(""))})).collect();
        versions.sort_by_key(|v|std::cmp::Reverse(v["created"].as_i64().unwrap_or(0)));Ok(versions)
    }

    pub fn rollback(&self,name:&str,revision:&str,expected:&str)->Result<()> {
        use base64::Engine;
        let _guard=self.writing.lock().unwrap();let name=clean_name(name)?;
        if revision.len()!=64||!revision.bytes().all(|c|c.is_ascii_hexdigit()){bail!("无效技能版本。");}
        let current=self.list_unlocked().into_iter().find(|s|s["name"]==name).ok_or_else(||anyhow!("找不到技能。"))?;
        if current["revision"]!=expected{bail!("技能已更新，请重新读取后再回滚。");}self.archive_unlocked(&current)?;
        let snapshot:Value=serde_json::from_slice(&crate::sqlite_storage::read(self.dir.parent().unwrap().join("skill-history").join(&name).join(format!("{revision}.json")))?)?;
        let bundle=snapshot["files"].as_object().ok_or_else(||anyhow!("版本记录损坏。"))?;
        let stage=self.dir.parent().unwrap().join("skill-staging").join(uuid::Uuid::new_v4().simple().to_string());
        let destination=self.root(current["enabled"]==true).join(&name);
        let backup=stage.with_extension("previous");
        let result=(||->Result<()>{
            let mut total=0;
            for(path,data)in bundle {
                if path!=".teambot.json"&&!safe_relative(path){bail!("版本包含无效附件路径。");}
                let bytes=base64::engine::general_purpose::STANDARD.decode(data.as_str().unwrap_or(""))?;total+=bytes.len();if total>16_000_000{bail!("版本超出限额。");}
                let file=stage.join(path);std::fs::create_dir_all(file.parent().unwrap())?;std::fs::write(file,bytes)?;
            }
            if (if snapshot["format"]=="bundle-v2"{bundle_revision(&stage)?}else{self::revision(&crate::sqlite_storage::read_to_string(stage.join("SKILL.md"))?)})!=revision{bail!("版本校验失败。");}
            std::fs::rename(&destination,&backup)?;
            if let Err(e)=std::fs::rename(&stage,&destination){let _=std::fs::rename(&backup,&destination);return Err(e.into());}Ok(())
        })();
        let _=std::fs::remove_dir_all(&stage);if result.is_ok(){let _=std::fs::remove_dir_all(backup);}result
    }

    fn quarantine_regressions(&self,names:&[String])->Result<()>{
        let _writing=self.writing.lock().unwrap();let profiles=self.profiles.lock().unwrap();
        for name in names{let Some(skill)=self.list_unlocked().into_iter().find(|s|s["name"]==*name&&s["enabled"]==true)else{continue;};
            let mut checks:Vec<_>=profiles.values().flatten().filter(|p|p["name"]==*name).flat_map(|p|p["evaluations"].as_array().into_iter().flatten()).filter(|e|e["revision"]==skill["revision"]&&e["evidence"]=="artifact-validation").collect();
            checks.sort_by_key(|e|std::cmp::Reverse(e["at"].as_i64().unwrap_or(0)));
            if checks.len()>=3&&checks.iter().take(3).all(|e|e["passed"]==false){
                self.archive_unlocked(&skill)?;
                crate::durable::save(&self.root(true).join(name).join(".quality.json"),&json!({"state":"quarantined","revision":skill["revision"],"reason":"当前版本连续三次交付物实检未通过，已停用；请修复或回滚后重新评估。","at":crate::store::now_ms()}))?;
                std::fs::rename(self.root(true).join(name),self.root(false).join(name))?;
            }
        }Ok(())
    }

    fn benchmark_file(&self,name:&str)->PathBuf{self.dir.parent().unwrap().join("skill-benchmarks").join(format!("{name}.json"))}
    fn benchmark_data(&self,name:&str)->Value{crate::sqlite_storage::read(self.benchmark_file(name)).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_else(||json!({"cases":[],"runs":[]}))}
    pub fn benchmarks(&self,name:&str)->Result<Value>{let name=clean_name(name)?;let _guard=self.writing.lock().unwrap();Ok(self.benchmark_data(&name))}
    pub fn save_benchmark(&self,name:&str,goal:&str,acceptance:&str)->Result<String>{
        let name=clean_name(name)?;let _guard=self.writing.lock().unwrap();
        if goal.trim().is_empty()||acceptance.trim().is_empty()||goal.len()>12000||acceptance.len()>4000{bail!("请填写测试任务和验收条件。");}
        let id=revision(&format!("{goal}\0{acceptance}"))[..16].to_string();let mut data=self.benchmark_data(&name);
        if !data["cases"].as_array().unwrap().iter().any(|v|v["id"]==id){if data["cases"].as_array().unwrap().len()>=30{bail!("每个技能最多保存 30 个评测用例。");}data["cases"].as_array_mut().unwrap().push(json!({"id":id,"goal":goal,"acceptance":acceptance,"created":crate::store::now_ms()}));crate::durable::save(&self.benchmark_file(&name),&data)?;}Ok(id)
    }
    pub fn record_benchmark(&self,name:&str,case:&str,revision:&str,flow:&str,verification:&Value)->Result<()>{
        let name=clean_name(name)?;let _guard=self.writing.lock().unwrap();let mut data=self.benchmark_data(&name);
        if !data["cases"].as_array().unwrap().iter().any(|v|v["id"]==case){bail!("评测用例不存在。");}
        if !data["runs"].as_array().unwrap().iter().any(|v|v["flow"]==flow){data["runs"].as_array_mut().unwrap().push(json!({"case":case,"revision":revision,"flow":flow,"passed":verification["passed"],"verification":verification,"at":crate::store::now_ms()}));if data["runs"].as_array().unwrap().len()>200{data["runs"].as_array_mut().unwrap().remove(0);}crate::durable::save(&self.benchmark_file(&name),&data)?;}Ok(())
    }
    fn benchmark_runs_unlocked(&self,name:&str,revision:&str)->Vec<Value>{self.benchmark_data(name)["runs"].as_array().into_iter().flatten().filter(|r|r["revision"]==revision).cloned().collect()}

    pub fn set_origin(&self, name: &str, origin: Value) -> Result<()> {
        let _guard = self.writing.lock().unwrap();
        let name = clean_name(name)?;
        let skill = self.list_unlocked().into_iter().find(|s| s["name"] == name).ok_or_else(|| anyhow!("No skill named {name}"))?;
        crate::durable::save(&self.root(skill["enabled"] == true).join(name).join(".teambot.json"), &origin)
    }

    pub async fn validate_bundle(&self,name:&str)->Result<Value>{
        let skill=self.get(name)?;let root=self.root(skill["enabled"]==true).join(clean_name(name)?);
        Self::validate_bundle_at(&root,skill["name"].as_str().unwrap_or(""),skill["revision"].as_str().unwrap_or("")).await
    }
    pub(crate) async fn validate_bundle_at(root:&Path,name:&str,expected:&str)->Result<Value>{
        let files=bundle_files(root)?;
        let text=std::str::from_utf8(files.get("SKILL.md").ok_or_else(||anyhow!("技能缺少 SKILL.md。"))?)?;
        let(declared,description)=front_matter(text);let metadata=declared==name&&!description.trim().is_empty()&&!body_of(text).is_empty();
        let mut checks=vec![json!({"path":"SKILL.md","passed":metadata,"reason":if metadata{"技能名称、用途与步骤齐全。"}else{"技能名称、用途或步骤缺失，请修复元数据。"}})];
        for path in files.keys().filter(|p|p.as_str()!="SKILL.md").take(200){
            let full=root.join(path);let file=full.to_string_lossy();
            let ext=full.extension().and_then(|e|e.to_str()).unwrap_or("");
            let result=match ext{
                "py"=>Some(crate::validation::parser("python3",&["-I","-c","import ast,sys; ast.parse(open(sys.argv[1],encoding='utf-8').read(),filename=sys.argv[1])",&file]).await),
                "sh"|"bash"=>Some(crate::validation::parser("bash",&["-n",&file]).await),
                "js"|"mjs"|"cjs"=>Some(crate::validation::parser("node",&["--check",&file]).await),
                "json"=>Some(serde_json::from_slice::<Value>(&files[path]).map(|_|String::new()).map_err(Into::into)),
                _=>None,
            };
            checks.push(match result{Some(Ok(_))=>json!({"path":path,"passed":true,"reason":"语法或结构检查通过，未执行脚本。"}),Some(Err(e))=>json!({"path":path,"passed":false,"reason":crate::team::clip(&e.to_string(),500)}),None=>json!({"path":path,"passed":null,"reason":"资源已纳入版本哈希；此格式需试运行或人工核对。"})});
        }
        if bundle_revision(root)?!=expected{bail!("技能包在校验期间发生变化，请重新校验。");}
        Ok(json!({"revision":expected,"passed":!checks.iter().any(|v|v["passed"]==false),"files":files.len(),"checks":checks,"reason":"完整包路径、大小与版本检查通过；脚本仅解析语法，功能结果需运行保存的评测用例验证。"}))
    }

    pub fn resources(&self, name: &str) -> Result<Vec<Value>> {
        let skill = self.get(name)?;
        let root = self.root(skill["enabled"] == true).join(clean_name(name)?);
        fn visit(root: &Path, dir: &Path, out: &mut Vec<Value>, depth: usize) -> Result<()> {
            if depth > 10 || out.len() >= 200 { return Ok(()); }
            for entry in std::fs::read_dir(dir)? {
                if out.len() >= 200 { break; }
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with('.') { continue; }
                let kind = entry.file_type()?;
                if kind.is_dir() { visit(root, &entry.path(), out, depth + 1)?; }
                else if kind.is_file() {
                    let path = entry.path(); let relative = path.strip_prefix(root)?.to_string_lossy();
                    if relative != "SKILL.md" && !relative.starts_with('.') { out.push(json!({"path":relative,"size":entry.metadata()?.len()})); }
                }
            }
            Ok(())
        }
        let mut out = Vec::new(); visit(&root, &root, &mut out, 0)?; out.sort_by(|a,b|a["path"].as_str().cmp(&b["path"].as_str())); Ok(out)
    }

    pub fn resource(&self, name: &str, path: &str) -> Result<Vec<u8>> {
        if !safe_relative(path) { bail!("Invalid skill resource path."); }
        let skill = self.get(name)?;
        if skill["enabled"] != true || skill["bundleValid"]==false { bail!("技能已停用或技能包验证未通过。"); }
        let root = self.root(true).join(clean_name(name)?).canonicalize()?;
        let candidate = root.join(path);
        let mut current = root.clone();
        for part in Path::new(path).components() { current.push(part); if std::fs::symlink_metadata(&current)?.file_type().is_symlink() { bail!("Invalid skill resource path."); } }
        let candidate = candidate.canonicalize()?;
        if !candidate.starts_with(&root) || !candidate.is_file() || std::fs::metadata(&candidate)?.len() > 2_000_000 { bail!("Invalid or oversized skill resource."); }
        Ok(crate::sqlite_storage::read(candidate)?)
    }

    /// Publish a complete resource bundle atomically; partial downloads never load.
    pub fn install_bundle(&self, name: &str, files: &BTreeMap<String, Vec<u8>>, origin: Value) -> Result<String> {
        let name = clean_name(name)?;
        let _guard = self.writing.lock().unwrap();
        if self.list_unlocked().iter().any(|s| s["name"] == name) { bail!("A skill with this name already exists."); }
        let skill = std::str::from_utf8(files.get("SKILL.md").ok_or_else(|| anyhow!("The source has no SKILL.md."))?)?.replace("\r\n", "\n");
        let (declared, description) = front_matter(&skill);
        if declared != name || description.is_empty() || skill.len() > 128_000 { bail!("Invalid skill metadata."); }
        if files.len() > 200 || files.values().map(Vec::len).sum::<usize>() > 16_000_000 || files.iter().any(|(p,b)| !safe_relative(p) || b.len() > 2_000_000) { bail!("Invalid or oversized skill bundle."); }
        let staging = self.dir.parent().unwrap().join("skill-staging").join(uuid::Uuid::new_v4().simple().to_string());
        let staged = (|| -> Result<()> {
            for (path, bytes) in files {
                let target = staging.join(path); std::fs::create_dir_all(target.parent().unwrap())?; std::fs::write(target, bytes)?;
            }
            std::fs::write(staging.join("SKILL.md"), skill)?;
            crate::durable::save(&staging.join(".teambot.json"), &origin)?;
            std::fs::rename(&staging, self.root(true).join(&name))?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&staging);
        staged?; Ok(name)
    }

    /// Writes the skill whole; an existing one with that name is replaced.
    #[cfg(test)]
    pub fn save(&self, name: &str, description: &str, body: &str) -> Result<String> {
        let current = self.get(name).ok();
        self.save_checked(name, description, body, current.as_ref().and_then(|s| s["revision"].as_str()))
    }

    pub fn save_checked(&self, name: &str, description: &str, body: &str, expected: Option<&str>) -> Result<String> {
        let given = name.trim();
        let name = clean_name(given)?;
        let _guard = self.writing.lock().unwrap();
        let existing = self.list_unlocked().into_iter().find(|s| s["name"] == name);
        if let Some(skill) = &existing {
            if expected != skill["revision"].as_str() { bail!("Skill changed. Read the latest skill before saving and merge your changes."); }
        } else if expected.is_some_and(|r| !r.is_empty()) {
            bail!("Skill changed. Read the latest skill before saving and merge your changes.");
        }
        let mut description = description.replace(['\n', '\r'], " ").trim().to_string();
        if !given.is_ascii() && !description.is_empty() && !description.starts_with(given) {
            description = format!("{given}: {description}");
        }
        if description.is_empty() {
            bail!("Say in one line when the skill is for: the Bot reads that to pick it.");
        }
        let normalized = body.replace("\r\n", "\n");
        let body = body_of(&normalized);
        if body.is_empty() {
            bail!("Write the skill's steps.");
        }
        if description.len() > 4096 || body.len() > 128_000 { bail!("Skill content is too large."); }
        if let Some(skill)=&existing {self.archive_unlocked(skill)?;}
        let enabled = existing.as_ref().is_none_or(|s| s["enabled"] == true);
        let folder = self.root(enabled).join(&name);
        if std::fs::symlink_metadata(&folder).is_ok_and(|m| m.file_type().is_symlink()) { bail!("Invalid skill folder"); }
        std::fs::create_dir_all(&folder)?;
        let title = existing.as_ref().filter(|_| given == name).and_then(|s| s["title"].as_str()).unwrap_or(given);
        let text = format!("---\nname: {name}\ndescription: {}\ntitle: {}\n---\n\n{body}\n", serde_json::to_string(&description)?, serde_json::to_string(title)?);
        std::fs::write(folder.join("SKILL.md.tmp"), text)?;
        std::fs::rename(folder.join("SKILL.md.tmp"), folder.join("SKILL.md"))?;
        Ok(name)
    }

    /// Add a narrow evidence-backed refinement without rewriting upstream metadata.
    /// The receipt marker makes replay after a crash idempotent.
    #[cfg(test)]
    pub fn append_learning(&self,name:&str,expected:&str,receipt:&str,note:&str)->Result<()> {
        let _guard=self.writing.lock().unwrap();
        let name=clean_name(name)?;
        if receipt.len()>128 || !receipt.bytes().all(|c|c.is_ascii_alphanumeric()||b"-_:".contains(&c)) { bail!("Invalid learning receipt"); }
        let skill=self.list_unlocked().into_iter().find(|s|s["name"]==name).ok_or_else(||anyhow!("No such skill"))?;
        if skill["enabled"]!=true { bail!("This skill is disabled."); }
        let marker=format!("<!-- teambot-learning:{receipt} -->");
        let original=skill["text"].as_str().unwrap_or("");
        if original.contains(&marker){return Ok(());}
        if skill["revision"]!=expected { bail!("Skill changed. Read the latest skill before saving and merge your changes."); }
        self.archive_unlocked(&skill)?;
        if (note.contains("teambot-learning:") || note.contains("dotbot-learning:")) || note.trim().len()<20 || note.len()>3000 || original.len()+note.len()+marker.len()+100>128_000 { bail!("Invalid or oversized skill refinement"); }
        let text=format!("{}\n\n{marker}\n### 整理改进 / Task-derived refinement\n{}\n",original.trim_end(),note.trim());
        let folder=self.root(true).join(name);
        std::fs::write(folder.join("SKILL.md.tmp"),text)?;
        std::fs::rename(folder.join("SKILL.md.tmp"),folder.join("SKILL.md"))?;Ok(())
    }

    pub fn set_enabled(&self, name: &str, enabled: bool, expected: &str) -> Result<()> {
        let _guard = self.writing.lock().unwrap();
        let name = clean_name(name)?;
        let skill = self.list_unlocked().into_iter().find(|s| s["name"] == name).ok_or_else(|| anyhow!("No skill named {name}"))?;
        if skill["revision"].as_str() != Some(expected) { bail!("Skill changed. Read the latest skill before saving and merge your changes."); }
        let was_enabled = skill["enabled"] == true;
        if was_enabled != enabled {
            let destination = self.root(enabled).join(&name);
            if destination.exists() { bail!("A skill with this name already exists."); }
            std::fs::rename(self.root(was_enabled).join(&name), destination)?;
        }
        if enabled{let _=std::fs::remove_file(self.root(true).join(&name).join(".quality.json"));}
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let _guard = self.writing.lock().unwrap();
        let name = clean_name(name)?;
        let existing = self.list_unlocked().into_iter().find(|s| s["name"] == name).ok_or_else(|| anyhow!("No skill named {name}"))?;
        let folder = self.root(existing["enabled"] == true).join(&name);
        if !folder.exists() {
            bail!("No skill named {name}");
        }
        std::fs::remove_dir_all(folder)?;
        Ok(())
    }
}

pub fn safe_relative(path: &str) -> bool {
    !path.is_empty() && !path.contains('\\') && Path::new(path).components().all(|c| matches!(c, Component::Normal(p) if !p.to_string_lossy().starts_with('.')))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_chinese_name_gets_a_stable_english_one() {
        let a = clean_name("周报").unwrap();
        assert!(a.starts_with("skill-") && a.len() == 14);
        assert_eq!(a, clean_name(" 周报 ").unwrap());
        assert_eq!(clean_name("Weekly Report 周报").unwrap(), "weekly-report");
        assert!(clean_name("  ").is_err());
    }

    use super::*;

    #[tokio::test]
    async fn bundle_script_checks_parse_without_executing_resources(){
        let h=tempfile::tempdir().unwrap();let skills=Skills::open(h.path()).unwrap();skills.save("parser","检查脚本","检查脚本语法及任务结果").unwrap();
        let root=skills.plugin_dir().join("skills/parser");std::fs::create_dir_all(root.join("scripts")).unwrap();
        let marker=h.path().join("must-not-exist");let code=format!("from pathlib import Path\nPath({}).write_text('executed')\n",serde_json::to_string(&marker).unwrap());
        std::fs::write(root.join("scripts/run.py"),code).unwrap();let good=skills.validate_bundle("parser").await.unwrap();assert_eq!(good["passed"],true);assert!(!marker.exists());
        std::fs::write(root.join("scripts/run.py"),"def broken(:\n").unwrap();let bad=skills.validate_bundle("parser").await.unwrap();assert_eq!(bad["passed"],false);assert_ne!(good["revision"],bad["revision"]);
    }

    #[test]
    fn resource_versions_benchmarks_and_quarantine_are_evidence_scoped(){
        let h=tempfile::tempdir().unwrap();let s=Skills::open(h.path()).unwrap();
        s.save("sample","校验技能包","检查真实产物").unwrap();
        let root=s.plugin_dir().join("skills/sample");std::fs::create_dir_all(root.join("scripts")).unwrap();
        std::fs::write(root.join("scripts/check.py"),"print(1)").unwrap();let old=s.get("sample").unwrap();s.versions("sample").unwrap();
        std::fs::write(root.join("scripts/check.py"),"print(2)").unwrap();let new=s.get("sample").unwrap();assert_ne!(old["revision"],new["revision"]);
        s.versions("sample").unwrap();s.rollback("sample",old["revision"].as_str().unwrap(),new["revision"].as_str().unwrap()).unwrap();assert_eq!(std::fs::read_to_string(root.join("scripts/check.py")).unwrap(),"print(1)");
        let case=s.save_benchmark("sample","生成 JSON","必须能解析").unwrap();assert_eq!(s.save_benchmark("sample","生成 JSON","必须能解析").unwrap(),case);
        let rev=old["revision"].as_str().unwrap();s.record_benchmark("sample",&case,rev,"flow1",&json!({"passed":false})).unwrap();s.record_benchmark("sample",&case,rev,"flow1",&json!({"passed":true})).unwrap();assert_eq!(s.benchmarks("sample").unwrap()["runs"].as_array().unwrap().len(),1);
        let names=vec!["sample".into()];let revisions=BTreeMap::from([("sample".into(),rev.into())]);
        for i in 0..3{s.note_evaluation("a",&names,&revisions,&format!("model{i}"),false,"model-review").unwrap();}assert_eq!(s.get("sample").unwrap()["enabled"],true);
        for i in 0..3{s.note_evaluation("a",&names,&revisions,&format!("real{i}"),false,"artifact-validation").unwrap();}assert_eq!(s.get("sample").unwrap()["enabled"],false);assert_eq!(s.get("sample").unwrap()["quality"]["state"],"quarantined");
        s.set_enabled("sample",true,rev).unwrap();assert!(s.get("sample").unwrap()["quality"].is_null());
    }

    #[test]
    fn history_restores_resources_and_evaluations_remain_version_scoped() {
        let home=tempfile::tempdir().unwrap();let skills=Skills::open(home.path()).unwrap();
        skills.save("proof","核对结果","原版流程").unwrap();
        let folder=skills.plugin_dir().join("skills/proof");
        std::fs::create_dir_all(folder.join("references")).unwrap();
        std::fs::write(folder.join("references/data.txt"),"原版资源").unwrap();
        let original=skills.get("proof").unwrap();let revision=original["revision"].as_str().unwrap();
        skills.save("proof","核对结果","新版流程").unwrap();
        std::fs::write(folder.join("references/data.txt"),"新版资源").unwrap();
        let changed=skills.get("proof").unwrap();assert_eq!(skills.versions("proof").unwrap().len(),2);
        skills.rollback("proof",revision,changed["revision"].as_str().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("references/data.txt")).unwrap(),"原版资源");
        assert!(skills.rollback("proof",revision,changed["revision"].as_str().unwrap()).is_err());
        let names=vec!["proof".into()];let revisions=BTreeMap::from([("proof".into(),revision.into())]);
        skills.note_evaluation("bot",&names,&revisions,"review",true,"model-review").unwrap();
        skills.note_evaluation("bot",&names,&revisions,"review",true,"model-review").unwrap();
        skills.note_evaluation("bot",&names,&revisions,"tools",true,"tool-supported-review").unwrap();
        assert_eq!(skills.profile("bot")[0]["evaluations"].as_array().unwrap().len(),2);
        assert_eq!(skills.profile("bot")[0]["verifiedPasses"],1);
        let bot=crate::store::Bot{id:"bot".into(),..Default::default()};
        assert_eq!(skills.specialties(&bot)[0]["verifiedPasses"],1);
        skills.save("proof","核对结果","第三版流程").unwrap();
        assert_eq!(skills.specialties(&bot)[0]["verifiedPasses"],0);
    }

    #[test]
    fn a_skill_is_a_plugin_skill_file() {
        let home = tempfile::tempdir().unwrap();
        let skills = Skills::open(home.path()).unwrap();
        assert!(home.path().join("plugin/.claude-plugin/plugin.json").exists());
        let name = skills.save("Weekly Report!", "Use when the user asks for the weekly report", "1. Open the tracker\n2. Sum up").unwrap();
        assert_eq!(name, "weekly-report");
        let text = std::fs::read_to_string(home.path().join("plugin/skills/weekly-report/SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: weekly-report\ndescription: \"Use when the user asks for the weekly report\"\ntitle: \"Weekly Report!\"\n---\n\n1. Open"));
        assert_eq!(skills.list()[0]["description"], "Use when the user asks for the weekly report");
        let description = "用于周报: 引用 \"进展\" # 每周";
        skills.save("Weekly Report!", description, "Check results.").unwrap();
        assert_eq!(skills.list()[0]["description"], description);
        skills.delete("weekly-report").unwrap();
        assert!(skills.list().is_empty());
    }

    #[test]
    fn disabled_skills_keep_resources_and_edits_require_the_latest_revision() {
        let home = tempfile::tempdir().unwrap();
        let skills = Skills::open(home.path()).unwrap();
        let name = skills.save_checked("周报", "每周汇总工作时使用", "检查来源并生成报告。", None).unwrap();
        let original = skills.get(&name).unwrap();
        assert_eq!(original["title"], "周报");
        let folder = skills.plugin_dir().join("skills").join(&name);
        std::fs::write(folder.join("reference.md"), "required reference").unwrap();
        assert!(skills.set_enabled(&name,false,original["revision"].as_str().unwrap()).is_err(),"resource changes must invalidate stale revisions");
        let original=skills.get(&name).unwrap();
        skills.set_enabled(&name, false, original["revision"].as_str().unwrap()).unwrap();
        assert!(!folder.exists(), "disabled workflows must leave the CLI plugin catalog");
        assert!(skills.save_checked(&name, "每周汇总", "不带版本的覆盖", None).is_err());
        skills.save_checked(&name, "每周汇总", "先验证，再生成报告。", original["revision"].as_str()).unwrap();
        let changed = skills.get(&name).unwrap();
        assert_eq!(changed["enabled"], false);
        assert_eq!(changed["title"], "周报");
        assert!(skills.save_checked(&name, "旧版本", "覆盖", original["revision"].as_str()).is_err());
        assert_eq!(skills.get(&name).unwrap()["body"], "先验证，再生成报告。");
        assert!(skills.set_enabled(&name, true, original["revision"].as_str().unwrap()).is_err());
        skills.set_enabled(&name, true, changed["revision"].as_str().unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("reference.md")).unwrap(), "required reference");
        let reopened = Skills::open(home.path()).unwrap();
        assert_eq!(reopened.get(&name).unwrap()["enabled"], true);
        reopened.set_enabled(&name, false, changed["revision"].as_str().unwrap()).unwrap();
        reopened.delete(&name).unwrap();
        assert!(reopened.list().is_empty());
    }

    #[test]
    fn concurrent_skill_updates_cannot_silently_overwrite_each_other() {
        let home = tempfile::tempdir().unwrap();
        let skills = Skills::open(home.path()).unwrap();
        skills.save_checked("shared", "shared workflow", "original", None).unwrap();
        let old = skills.get("shared").unwrap();
        let rev = old["revision"].as_str().unwrap();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| skills.save_checked("shared", "shared workflow", "first edit", Some(rev)));
            let second = scope.spawn(|| skills.save_checked("shared", "shared workflow", "second edit", Some(rev)));
            assert_ne!(first.join().unwrap().is_ok(), second.join().unwrap().is_ok());
        });
    }
    #[test]
    fn bundles_publish_all_resources_and_reject_invalid_partial_or_disabled_overwrites() {
        let home = tempfile::tempdir().unwrap();
        let skills = Skills::open(home.path()).unwrap();
        let mut files = BTreeMap::from([
            ("SKILL.md".into(), b"---\nname: pdf\ndescription: >-\n  Extract PDF text and verify\n  page coverage.\n---\n\nRead references/checks.md before processing.".to_vec()),
            ("references/checks.md".into(), b"Compare source page counts".to_vec()),
            ("scripts/extract.py".into(), b"raise RuntimeError('never run at install')".to_vec()),
            ("assets/input.bin".into(), vec![0,255,1]),
        ]);
        let origin=json!({"kind":"imported","validation":"untested"});
        let mut invalid=files.clone(); invalid.insert("../escape".into(),vec![1]);
        assert!(skills.install_bundle("pdf",&invalid,origin.clone()).is_err());
        assert!(skills.list().is_empty());
        assert!(!home.path().join("plugin/escape").exists());
        files.remove("SKILL.md");
        assert!(skills.install_bundle("pdf",&files,origin.clone()).is_err());
        files.insert("SKILL.md".into(),invalid.remove("SKILL.md").unwrap());
        skills.install_bundle("pdf",&files,origin).unwrap();
        assert_eq!(skills.get("pdf").unwrap()["description"],"Extract PDF text and verify page coverage.");
        assert_eq!(skills.resources("pdf").unwrap().len(),3);
        assert_eq!(skills.resource("pdf","assets/input.bin").unwrap(),vec![0,255,1]);
        for path in ["../escape","/etc/passwd","references/../../escape","references\\checks.md",".teambot.json"] { assert!(skills.resource("pdf",path).is_err()); }
        #[cfg(unix)] {
            std::os::unix::fs::symlink(home.path(),skills.plugin_dir().join("skills/pdf/link")).unwrap();
            assert!(skills.resource("pdf","link/skill-profiles.json").is_err());
        }
        let original=skills.get("pdf").unwrap();
        skills.set_enabled("pdf",false,original["revision"].as_str().unwrap()).unwrap();
        assert!(skills.resource("pdf","assets/input.bin").is_err());
        assert!(skills.install_bundle("pdf",&files,json!({})).is_err());
        assert_eq!(skills.get("pdf").unwrap()["enabled"],false);
        assert_eq!(skills.get("pdf").unwrap()["origin"]["validation"],"untested");
    }

    #[test]
    fn bot_profiles_survive_restart_and_remain_individual() {
        let home=tempfile::tempdir().unwrap();
        let skills=Skills::open(home.path()).unwrap();
        skills.save("source-review","Review sources","Check provenance").unwrap();
        skills.save("writing","Write copy","Draft then revise").unwrap();
        skills.note_loaded("reviewer","source-review").unwrap();
        skills.note_loaded("reviewer","source-review").unwrap();
        skills.note_loaded("writer","writing").unwrap();
        skills.note_outcome("reviewer",&["source-review".into()],"turn-completed").unwrap();
        let reopened=Skills::open(home.path()).unwrap();
        assert_eq!(reopened.profile("reviewer")[0]["loads"],2);
        assert_eq!(reopened.profile("reviewer")[0]["lastOutcome"],"turn-completed");
        assert_eq!(reopened.profile("writer")[0]["name"],"writing");
        assert!(reopened.profile("unknown").is_empty());
    }

    #[test]
    fn learning_refinements_preserve_licenses_are_idempotent_and_cannot_overwrite_edits(){
        let home=tempfile::tempdir().unwrap();let skills=Skills::open(home.path()).unwrap();
        let files=BTreeMap::from([("SKILL.md".into(),b"---\nname: records\ndescription: Compare records\nlicense: LICENSE.txt\n---\n\nOriginal workflow".to_vec())]);
        skills.install_bundle("records",&files,json!({"validation":"untested"})).unwrap();
        let original=skills.get("records").unwrap();let rev=original["revision"].as_str().unwrap();
        let note="Compare source and output record counts before delivering results.";
        skills.append_learning("records",rev,"work-1:1",note).unwrap();
        skills.append_learning("records",rev,"work-1:1",note).unwrap();
        let changed=skills.get("records").unwrap();let text=changed["text"].as_str().unwrap();
        assert!(text.contains("license: LICENSE.txt"));assert_eq!(text.matches(note).count(),1);
        assert_eq!(changed["origin"]["validation"],"untested");
        assert!(skills.append_learning("records",rev,"work-2:1",note).is_err());
        skills.set_enabled("records",false,changed["revision"].as_str().unwrap()).unwrap();
        assert!(skills.append_learning("records",changed["revision"].as_str().unwrap(),"work-3:1",note).is_err());
    }

}
