//! Bounded project copies and optimistic three-way merging, without git commits.
use crate::{
    tasks::Task,
    team::{Conv, Team},
};
use anyhow::{Result, anyhow, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub root: PathBuf,
    pub base: PathBuf,
    pub working: PathBuf,
    pub hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub desktop_bot: Option<String>,
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}
fn excluded(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".dotbot-inbox"
            | "node_modules"
            | "target"
            | ".venv"
            | "venv"
            | ".codepod"
            | ".next"
            | "dist"
            | "build"
            | "__pycache__"
    ) || name.starts_with(".env")
        || matches!(name, ".ssh" | ".aws" | ".gnupg")
}
fn files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(
        root: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, Vec<u8>>,
        size: &mut u64,
    ) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if excluded(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                walk(root, &entry.path(), out, size)?;
            } else if kind.is_file() {
                let len = entry.metadata()?.len();
                *size += len;
                ensure!(
                    len <= 8_000_000 && *size <= 64_000_000 && out.len() < 3000,
                    "项目副本超过限额（3000 文件 / 64 MB），请指定更小的项目目录。"
                );
                out.insert(
                    entry.path().strip_prefix(root)?.to_string_lossy().into(),
                    crate::sqlite_storage::read(entry.path())?,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out, &mut 0)?;
    Ok(out)
}
fn write(root: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    ensure!(
        !Path::new(name).is_absolute()
            && Path::new(name)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "无效项目路径"
    );
    let mut p = root.to_path_buf();
    for part in Path::new(name).components() {
        p.push(part);
        if let Ok(m) = std::fs::symlink_metadata(&p) {
            ensure!(!m.file_type().is_symlink(), "项目路径包含符号链接。");
        }
    }
    std::fs::create_dir_all(p.parent().unwrap())?;
    crate::durable::save_bytes(&p, bytes)
}
// None represents a deletion. A file edited in both branches uses git's text
// merge driver; binary/conflicting changes stay isolated for review.
async fn combine(
    base: Option<&[u8]>,
    ours: Option<&[u8]>,
    theirs: Option<&[u8]>,
    scratch: &Path,
) -> Result<Option<Vec<u8>>> {
    if ours == theirs || theirs == base {
        return Ok(ours.map(Vec::from));
    }
    if ours == base {
        return Ok(theirs.map(Vec::from));
    }
    let (Some(base), Some(ours), Some(theirs)) = (base, ours, theirs) else {
        bail!("修改与删除冲突");
    };
    if [base, ours, theirs].iter().any(|b| b.contains(&0)) {
        bail!("二进制文件冲突");
    }
    std::fs::create_dir_all(scratch)?;
    let a = scratch.join("ours");
    let b = scratch.join("base");
    let c = scratch.join("theirs");
    std::fs::write(&a, ours)?;
    std::fs::write(&b, base)?;
    std::fs::write(&c, theirs)?;
    let merged = crate::validation::parser(
        "git",
        &[
            "merge-file",
            "-p",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
            c.to_str().unwrap(),
        ],
    )
    .await?;
    // parser output is capped: never truncate a project file into a merge.
    ensure!(merged.len() < 32768, "合并输出超过限额，请人工合并此文件。");
    Ok(Some(merged.into_bytes()))
}
fn dependency_order(tasks:&[Task])->Vec<String>{
    let ids:std::collections::HashSet<_>=tasks.iter().map(|t|t.id.as_str()).collect();let mut done=vec![];
    loop{let before=done.len();for task in tasks{if !done.contains(&task.id)&&task.depends_on.iter().all(|id|!ids.contains(id.as_str())||done.contains(id)){done.push(task.id.clone());}}if done.len()==tasks.len()||done.len()==before{break;}}done
}

impl Team {
    fn snapshot_path(&self, id: &str) -> PathBuf {
        self.cfg
            .home
            .join("project-snapshots")
            .join(format!("{id}.json"))
    }
    pub(crate) fn project_snapshot(&self, id: &str) -> Option<Snapshot> {
        crate::sqlite_storage::read(self.snapshot_path(id))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }
    pub(crate) async fn prepare_project(self: &Arc<Self>, task: &Task) -> Result<()> {
        if task.workspace.is_none()
            || task.workflow.is_none()
        {
            return Ok(());
        }
        if let Some(mut snapshot)=self.project_snapshot(&task.id){
            if snapshot.desktop_bot.is_some(){
                let destination=self.desktops.as_ref().ok_or_else(||anyhow!("桌面未启用。"))?.host_dir(&task.target,&format!("/config/task-workspaces/{}/project",task.id)).ok_or_else(||anyhow!("无法创建任务副本。"))?;
                if destination!=snapshot.working{for(name,bytes)in files(&snapshot.working)?{write(&destination,&name,&bytes)?;}snapshot.working=destination;crate::durable::save(&self.snapshot_path(&task.id),&snapshot)?;}
            }
            self.tasks.change(&task.id,|t|{t.workspace=Some(snapshot.working.display().to_string());Ok(())})?;
            return Ok(());
        }
        let local = self.local_computer.enabled();
        let desktop_bot = if local {
            None
        } else {
            Conv::parse(&task.source)
                .filter(|_| self.desktops.is_some())
                .map(|c| c.bot().to_string())
        };
        let root = if local {
            self.local_computer.check(&task.source, false)?;
            self.local_work_dir(&task.source).await?
        } else if let Some(bot) = &desktop_bot {
            self.desktops
                .as_ref()
                .unwrap()
                .host_dir(bot, "/config/workspace")
                .ok_or_else(|| anyhow!("无法读取桌面项目目录。"))?
        } else {
            return Ok(());
        };
        if !root.is_dir() {
            return Ok(());
        }
        let root = root.canonicalize()?;
        let home = self.cfg.home.canonicalize()?;
        ensure!(
            !home.starts_with(&root),
            "项目目录包含团队数据，请指定独立项目目录。"
        );
        let parent = PathBuf::from(task.workspace.as_ref().unwrap());
        let working = if desktop_bot.is_some() {
            self.desktops
                .as_ref()
                .unwrap()
                .host_dir(
                    &task.target,
                    &format!("/config/task-workspaces/{}/project", task.id),
                )
                .ok_or_else(|| anyhow!("无法创建桌面任务目录。"))?
        } else {
            parent.join("project")
        };
        let base = self.cfg.home.join("project-bases").join(&task.id);
        let bytes = files(&root)?;
        if bytes.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&working)?;
        std::fs::create_dir_all(&base)?;
        for (name, data) in &bytes {
            write(&working, name, data)?;
            write(&base, name, data)?;
            std::fs::set_permissions(
                working.join(name),
                std::fs::metadata(root.join(name))?.permissions(),
            )?;
        }
        let mut snapshot = Snapshot {
            root,
            base,
            working: working.clone(),
            desktop_bot,
            hashes: bytes.iter().map(|(n, b)| (n.clone(), hash(b))).collect(),
        };
        // Dependencies are visible in the downstream copy before it starts.
        let mut ancestors=std::collections::HashSet::new();let mut pending=task.depends_on.clone();while let Some(id)=pending.pop(){if ancestors.insert(id.clone())&&let Some(dep)=self.tasks.get(&id){pending.extend(dep.depends_on);}}
        let dependencies:Vec<_>=self.tasks.list().into_iter().filter(|t|ancestors.contains(&t.id)).collect();
        for id in dependency_order(&dependencies) {
            if let Some(dep) = self.project_snapshot(&id)
                && dep.root == snapshot.root
            {
                let proposed = files(&dep.working)?;
                let depbase = files(&dep.base)?;
                for name in depbase
                    .keys()
                    .chain(proposed.keys())
                    .collect::<std::collections::BTreeSet<_>>()
                {
                    let current = crate::sqlite_storage::read(working.join(name)).ok();
                    let next = combine(
                        depbase.get(name).map(Vec::as_slice),
                        current.as_deref(),
                        proposed.get(name).map(Vec::as_slice),
                        &parent.join("merge-scratch"),
                    )
                    .await?;
                    match next {
                        Some(b) => write(&working, name, &b)?,
                        None => {
                            let _ = std::fs::remove_file(working.join(name));
                        }
                    }
                }
            }
        }
        // A downstream task owns only its changes after inheriting ancestors.
        // Otherwise revising a line changed upstream would look like a conflict.
        let inherited=files(&working)?;
        for name in bytes.keys().filter(|name|!inherited.contains_key(*name)){std::fs::remove_file(snapshot.base.join(name))?;}
        for(name,data)in &inherited{write(&snapshot.base,name,data)?;}
        snapshot.hashes=inherited.iter().map(|(n,b)|(n.clone(),hash(b))).collect();
        let _=std::fs::remove_dir_all(parent.join("merge-scratch"));
        crate::durable::save(&self.snapshot_path(&task.id), &snapshot)?;
        self.tasks.change(&task.id, |t| {
            t.workspace = Some(working.display().to_string());
            Ok(())
        })?;
        Ok(())
    }
    pub(crate) async fn merge_projects(self: &Arc<Self>, source: &str, tasks: &[Task]) -> Value {
        match self.merge_projects_inner(source, tasks).await {
            Ok(value) => value,
            Err(e) => {
                json!({"passed":false,"status":"blocked","reason":self.secrets.redact(&e.to_string())})
            }
        }
    }
    async fn merge_projects_inner(self: &Arc<Self>, source: &str, tasks: &[Task]) -> Result<Value> {
        let snapshots: Vec<_> = tasks
            .iter()
            .filter_map(|t| self.project_snapshot(&t.id).map(|s| (t, s)))
            .collect();
        if snapshots.is_empty() {
            return Ok(
                json!({"passed":null,"status":"not-applicable","reason":"没有项目副本改动。"}),
            );
        }
        let bot_id = Conv::parse(source)
            .ok_or_else(|| anyhow!("无效来源"))?
            .bot()
            .to_string();
        let state = self.state().await;
        let bot = state
            .bots
            .get(&bot_id)
            .ok_or_else(|| anyhow!("Bot 已删除"))?;
        ensure!(
            bot.permission.as_deref() != Some("readonly"),
            "当前 Bot 为只读权限，项目改动保留在任务副本中。"
        );
        let mut roots: BTreeMap<PathBuf, Vec<(&Task, Snapshot)>> = BTreeMap::new();
        for (t, s) in snapshots {
            roots.entry(s.root.clone()).or_default().push((t, s));
        }
        let mut changed = vec![];
        let order=dependency_order(tasks);
        for (root, mut group) in roots {
            group.sort_by_key(|(task,_)|order.iter().position(|id|id==&task.id).unwrap_or(usize::MAX));
            let cloud = group.first().and_then(|(_, s)| s.desktop_bot.clone());
            let lease = format!(
                "project-merge:{}",
                tasks
                    .first()
                    .and_then(|t| t.workflow.as_deref())
                    .unwrap_or(source)
            );
            let operation = if let Some(bot) = &cloud {
                let desktops = self
                    .desktops
                    .as_ref()
                    .ok_or_else(|| anyhow!("桌面未启用。"))?;
                ensure!(
                    !desktops.user_has_control(bot).await,
                    "用户正在控制桌面，项目改动保留在副本中。"
                );
                let mut attempts = 0;
                loop {
                    match self.claim_computer(bot, &lease).await {
                        Ok(()) => break,
                        Err(e) if attempts >= 100 => return Err(e),
                        Err(_) => {
                            attempts += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                    }
                }
                None
            } else {
                Some(self.local_computer.claim_scope(source, &root)?)
            };
            let _lock = if let Some(operation) = &operation {
                Some(operation.lock().await)
            } else {
                None
            };
            let result:Result<Value>=async{
                for entry in std::fs::read_dir(self.cfg.home.join("merge-journals")).into_iter().flatten().filter_map(Result::ok){
                    let receipt=entry.path().join("receipt.json");let value:Value=crate::sqlite_storage::read(&receipt).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
                    ensure!(!(value["root"]==json!(root)&&matches!(value["status"].as_str(),Some("applying"|"needs-recovery"))),"上次合并尚未确认，请先核对合并日志及修改前副本：{}",receipt.display());
                }
                let original=files(&root)?;let mut merged=original.clone();let scratch=self.cfg.home.join("merge-staging").join(uuid::Uuid::new_v4().simple().to_string());
                let mut conflicts=vec![];
                for (task,snapshot) in group{
                    ensure!(task.status==crate::tasks::Status::Completed,"失败任务的项目副本不会合并。");
                    let base=files(&snapshot.base)?;ensure!(base.iter().all(|(n,b)|snapshot.hashes.get(n)==Some(&hash(b)))&&base.len()==snapshot.hashes.len(),"项目基线已被修改，不能合并。");
                    let proposed=files(&snapshot.working)?;
                    for name in base.keys().chain(proposed.keys()).collect::<std::collections::BTreeSet<_>>(){
                        if base.get(name)==proposed.get(name){continue;}
                        match combine(base.get(name).map(Vec::as_slice),merged.get(name).map(Vec::as_slice),proposed.get(name).map(Vec::as_slice),&scratch).await{Ok(Some(b))=>{merged.insert(name.clone(),b);},Ok(None)=>{merged.remove(name);},Err(_)=>conflicts.push(name.clone())}
                    }
                }
                let _=std::fs::remove_dir_all(&scratch);
                if !conflicts.is_empty(){return Ok(json!({"passed":false,"status":"conflict","root":root,"conflicts":conflicts,"reason":"存在并发修改冲突，所有改动保留在任务副本中，原项目未写入。"}));}
                let names:Vec<_>=original.keys().chain(merged.keys()).collect::<std::collections::BTreeSet<_>>().into_iter().filter(|n|original.get(*n)!=merged.get(*n)).cloned().collect();
                let active=||tasks.iter().filter_map(|t|t.workflow.as_deref()).all(|id|self.workflows.get(id).is_some_and(|w|w.status=="reviewing"));
                ensure!(active()&&!self.shutting_down.load(std::sync::atomic::Ordering::Relaxed),"工作流已停止，项目副本未合并。");
                if !names.is_empty(){
                    let changes:Vec<_>=names.iter().map(|n|json!({"path":n,"before":original.get(n).and_then(|b|std::str::from_utf8(b).ok()).map(|s|crate::team::clip(s,1000)),"after":merged.get(n).and_then(|b|std::str::from_utf8(b).ok()).map(|s|crate::team::clip(s,1000)),"deleted":!merged.contains_key(n)})).collect();
                    let input=json!({"root":root,"changes":changes});
                    let decision=if bot.permission.as_deref().unwrap_or("auto")=="auto"{self.auto_review(bot,source,"project_merge",&input).await}else{None};
                    match decision{Some((true,_))=>{},Some((false,reason))=>bail!("自动审批拒绝项目合并：{reason}"),None=>ensure!(self.ask_approval(source,"project_merge",input).await,"用户未允许项目合并，改动保留在副本中。")}
                }
                ensure!(active(),"工作流已停止，项目副本未合并。");
                if let Some(bot)=&cloud{ensure!(!self.desktops.as_ref().unwrap().user_has_control(bot).await,"用户已接管桌面，项目副本未合并。");}
                // Recheck immediately before applying. No dirty user file is discarded.
                ensure!(files(&root)?==original,"原项目在合并期间发生变化，请重新核对。");
                let journal=self.cfg.home.join("merge-journals").join(uuid::Uuid::new_v4().simple().to_string());
                std::fs::create_dir_all(&journal)?;for name in &names{if let Some(b)=original.get(name){write(&journal.join("before"),name,b)?;}}
                crate::durable::save(&journal.join("receipt.json"),&json!({"status":"applying","root":root,"files":names}))?;
                let applied:Result<()>= (||{for name in &names{if let Some(b)=merged.get(name){write(&root,name,b)?;}else{std::fs::remove_file(root.join(name))?;}}Ok(())})();
                if let Err(e)=applied{let mut rollback=true;for name in &names{if let Some(b)=original.get(name){rollback&=write(&root,name,b).is_ok();}else{rollback&=std::fs::remove_file(root.join(name)).is_ok()||!root.join(name).exists();}}crate::durable::save(&journal.join("receipt.json"),&json!({"status":if rollback{"rolled-back"}else{"needs-recovery"},"root":root,"files":names}))?;return Err(e);}
                crate::durable::save(&journal.join("receipt.json"),&json!({"status":"merged","root":root,"files":names}))?;
                // Only auto-permission Bots reach this path, with opt-in terminal access.
                let command=if root.join("Cargo.toml").is_file(){Some("cargo test --offline --quiet")}else if root.join("package.json").is_file(){let p:Value=serde_json::from_slice(&crate::sqlite_storage::read(root.join("package.json"))?)?;p["scripts"]["test"].as_str().filter(|s|!s.contains("no test specified")).map(|_|"npm test")}else{None};
                let check=if let Some(cmd)=command{
                    let run:Result<Value>=async{let(output,ok)=if let Some(cloud_bot)=&cloud{
                        let desktops=self.desktops.as_ref().ok_or_else(||anyhow!("桌面未启用。"))?;
                        let command=format!("cd /config/workspace && CI=1 {cmd}");let input=json!({"command":command,"timeout_s":60});
                        let decision=if bot.permission.as_deref().unwrap_or("auto")=="auto"{self.auto_review(bot,source,"desktop_run",&input).await}else{None};
                        match decision{Some((true,_))=>{},Some((false,reason))=>bail!("自动审批拒绝项目测试：{reason}"),None=>ensure!(self.ask_approval(source,"desktop_run",input).await,"用户未允许项目测试。")}
                        ensure!(active()&&!desktops.user_has_control(cloud_bot).await,"任务已停止或桌面已被接管，取消后续测试。");
                        desktops.run_with_env(cloud_bot,&command,60,&self.secrets.env()).await?
                    }else{
                        self.authorize_tool(source,"local_run",&json!({"command":cmd,"cwd":root,"timeout_s":60})).await?;ensure!(active(),"工作流已停止，取消后续测试。");self.local_computer.run(source,cmd,&root,60,&[("CI".into(),"1".into())]).await?
                    };Ok(json!({"passed":ok,"command":cmd,"output":crate::team::clip(&self.secrets.redact(&output),4000)}))}.await;
                    run.unwrap_or_else(|e|json!({"passed":false,"command":cmd,"reason":format!("项目已合并，但测试未完成：{}",self.secrets.redact(&e.to_string()))}))
                }else{
                    let mut checks=vec![];
                    for name in names.iter().filter(|n|merged.contains_key(*n)){let path=root.join(name);let result=crate::validation::artifact(&path,"").await;checks.push(json!({"file":name,"validation":result}));}
                    let passed=if checks.iter().any(|c|c["validation"]["passed"]==false){json!(false)}else if !checks.is_empty()&&checks.iter().all(|c|c["validation"]["passed"]==true){json!(true)}else{Value::Null};
                    json!({"passed":passed,"checks":checks,"reason":"未检测到测试入口，已检查适用文件的语法或结构；运行行为与业务正确性需另行验收。"})
                };
                Ok(json!({"passed":check["passed"],"status":"merged","root":root,"files":names,"validation":check,"journal":journal}))
            }.await;
            if cloud.is_some() {
                self.computer_owners
                    .lock()
                    .await
                    .retain(|_, owner| owner != &lease);
            } else {
                self.local_computer.release(source);
            }
            let value = result?;
            if value["status"] == "conflict" {
                return Ok(value);
            }
            changed.push(value);
        }
        Ok(
            json!({"passed":if changed.iter().any(|c|c["passed"]==false){json!(false)}else if changed.iter().all(|c|c["passed"]==true){json!(true)}else{Value::Null},"status":"merged","projects":changed}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn three_way_preserves_dirty_changes_and_detects_conflicts() {
        let h = tempfile::tempdir().unwrap();
        assert_eq!(
            combine(
                Some(b"a\nb\nc\n"),
                Some(b"A\nb\nc\n"),
                Some(b"a\nb\nC\n"),
                h.path()
            )
            .await
            .unwrap()
            .unwrap(),
            b"A\nb\nC\n"
        );
        assert!(
            combine(Some(b"a\n"), Some(b"b\n"), Some(b"c\n"), h.path())
                .await
                .is_err()
        );
        assert!(
            combine(Some(b"a"), None, Some(b"b"), h.path())
                .await
                .is_err()
        );
    }
}
