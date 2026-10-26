//! Durable delegation receipts. Each task gets its own turn and one result; a
//! restart leaves an explicit interruption instead of an indefinitely busy card.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::now_ms;

const KEEP: usize = 500;
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Status { Waiting, Queued, Running, Paused, Blocked, Completed, Failed, Cancelled, Interrupted }

impl Status {
    pub fn active(&self) -> bool { matches!(self, Self::Waiting | Self::Queued | Self::Running | Self::Paused | Self::Blocked) }
    pub fn settled(&self) -> bool { matches!(self, Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Blocked) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub source: String,
    pub target: String,
    pub text: String,
    pub parent: Option<String>,
    pub path: Vec<String>,
    pub status: Status,
    pub created: i64,
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub result: Option<String>,
    #[serde(default)]
    pub files: Vec<Value>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub direct: bool,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub target_conversation: Option<String>,
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub automatic: bool,
    #[serde(default)]
    pub assignment: Option<AssignmentDetails>,
    #[serde(default)]
    pub corrections: Vec<serde_json::Value>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub upstream_context: String,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "first_attempt")]
    pub attempt: u32,
    #[serde(default)]
    pub workspace: Option<String>,
    #[serde(default)]
    pub pause_requested: bool,
    #[serde(default)]
    pub verification: Option<Value>,
    #[serde(default)]
    pub execution_profile: Option<Value>,
    #[serde(default)]
    pub routing: Option<Value>,
}

fn first_attempt() -> u32 { 1 }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentDetails {
    pub objective: String,
    pub expected_output: String,
    pub acceptance: String,
    /// Runtime-only authorization; cannot be set through model tool arguments.
    #[serde(skip)]
    pub(crate) explicit: bool,
}

#[derive(Default)]
pub struct TaskScope {
    pub target_conversation: Option<String>,
    pub request: Option<String>,
    pub automatic: bool,
    pub assignment: Option<AssignmentDetails>,
}

pub struct Tasks {
    path: PathBuf,
    list: Mutex<Vec<Task>>,
}

impl Tasks {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("tasks.json");
        let mut list: Vec<Task> = match crate::sqlite_storage::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).context("reading tasks.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        for task in list.iter_mut().filter(|t| matches!(t.status, Status::Queued | Status::Running)) {
            task.status = Status::Interrupted;
            task.finished = Some(now_ms());
            task.result = Some("The server restarted before this task finished. Review its output before retrying external actions.".into());
        }
        let tasks = Self { path, list: Mutex::new(list) };
        tasks.save(&tasks.list.lock().unwrap())?;
        Ok(tasks)
    }

    fn save(&self, list: &[Task]) -> Result<()> {
        crate::durable::save(&self.path,&list)
    }

    pub fn list(&self) -> Vec<Task> { self.list.lock().unwrap().iter().rev().cloned().collect() }

    pub fn get(&self, id: &str) -> Option<Task> { self.list.lock().unwrap().iter().find(|t| t.id == id).cloned() }

    /// Persist before delivery. An identical outstanding request reuses its receipt.
    #[cfg(test)]
    pub fn create(&self, source: &str, target: &str, text: &str, parent: Option<&str>, inputs: Vec<String>, direct: bool) -> Result<(Task, bool)> {
        self.create_in(source, target, text, parent, inputs, direct, TaskScope::default())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_in(&self, source: &str, target: &str, text: &str, parent: Option<&str>, inputs: Vec<String>, direct: bool, scope: TaskScope) -> Result<(Task, bool)> {
        let mut list = self.list.lock().unwrap();
        let mut path = if let Some(id) = parent {
            let parent = list.iter().find(|t| t.id == id).ok_or_else(|| anyhow!("Unknown parent task"))?;
            if parent.cancel_requested || matches!(parent.status, Status::Cancelled | Status::Failed | Status::Interrupted) { bail!("The parent task is no longer active. Do not start more delegated work."); }
            parent.path.clone()
        } else {
            vec![crate::team::Conv::parse(source).ok_or_else(|| anyhow!("Unknown sender"))?.bot().to_string()]
        };
        if path.iter().any(|id| id == target) { bail!("Delegation loop: that Bot is already in this task's chain. Complete the work or report the missing capability."); }
        if path.len() >= MAX_DEPTH { bail!("This task reached the delegation depth limit. Complete the work instead of creating another handoff."); }
        if let Some(task) = list.iter().find(|t| t.source == source && t.target == target && t.parent.as_deref() == parent && t.request == scope.request && t.text == text && t.inputs == inputs && t.direct == direct && t.status.active()) {
            return Ok((task.clone(), false));
        }
        path.push(target.into());
        let task = Task {
            id: format!("task-{}", uuid::Uuid::new_v4().simple()), source: source.into(), target: target.into(), text: text.into(),
            parent: parent.map(str::to_string), path, status: Status::Queued, created: now_ms(), started: None, finished: None, result: None,
            files: Vec::new(), inputs, direct, cancel_requested: false,
            target_conversation: scope.target_conversation, request: scope.request, automatic: scope.automatic, assignment: scope.assignment, corrections: Vec::new(),
            depends_on: Vec::new(), upstream_context: String::new(), workflow: None, priority: 0, attempt: 1, workspace: None, pause_requested: false, verification: None,  execution_profile: None,  routing: None,
        };
        let mut next = list.clone();
        // Never evict outstanding work to make room for history.
        while next.len() >= KEEP {
            if let Some(at) = next.iter().position(|t| !t.status.active()) { next.remove(at); } else { break; }
        }
        next.push(task.clone());
        self.save(&next)?;
        *list = next;
        Ok((task, true))
    }

    pub fn update(&self, id: &str, status: Status, result: Option<String>, files: Vec<Value>) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let task = next.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow!("Unknown task"))?;
        if !task.status.active() { return Ok(()); }
        if status == Status::Running { task.started = Some(now_ms()); }
        if !status.active() { task.finished = Some(now_ms()); }
        task.status = status;
        task.result = result;
        task.files = files;
        self.save(&next)?;
        *list = next;
        Ok(())
    }

    pub(crate) fn change(&self, id: &str, f: impl FnOnce(&mut Task) -> Result<()>) -> Result<Task> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let task = next.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow!("找不到任务。"))?;
        f(task)?;
        let updated = task.clone();
        self.save(&next)?;
        *list = next;
        Ok(updated)
    }

    pub(crate) fn insert_plan(&self, tasks: Vec<Task>) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        next.extend(tasks);
        self.save(&next)?;
        *list = next;
        Ok(())
    }

    pub fn correct(&self, id: &str, text: &str, inputs: &[String]) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let task = next.iter_mut().find(|t| t.id == id).ok_or_else(|| anyhow!("找不到当前任务。"))?;
        if !task.status.active() { bail!("该任务已结束，请发送新消息。"); }
        task.corrections.push(serde_json::json!({"text": text, "time": now_ms()}));
        for src in inputs { if !task.inputs.contains(src) { task.inputs.push(src.clone()); } }
        self.save(&next)?;
        *list = next;
        Ok(())
    }

    /// Commit cancellation before stopping processes so a late tool cannot add descendants.
    pub fn cancel_tree(&self, id: &str) -> Result<Vec<Task>> {
        let mut list = self.list.lock().unwrap();
        if !list.iter().any(|t| t.id == id) { bail!("Unknown task"); }
        let mut ids = std::collections::HashSet::from([id.to_string()]);
        loop {
            let count = ids.len();
            for task in list.iter() {
                if task.parent.as_ref().is_some_and(|p| ids.contains(p)) { ids.insert(task.id.clone()); }
            }
            if ids.len() == count { break; }
        }
        let mut next = list.clone();
        let mut cancelled = Vec::new();
        if !next.iter().any(|t| ids.contains(&t.id) && t.status.active()) { return Ok(cancelled); }
        for task in &mut next {
            if ids.contains(&task.id) { task.cancel_requested = true; }
            if ids.contains(&task.id) && task.status.active() {
                task.status = Status::Cancelled;
                task.finished = Some(now_ms());
                task.result = Some("Stopped by the user, including dependent work.".into());
                cancelled.push(task.clone());
            }
        }
        if !cancelled.is_empty() { self.save(&next)?; *list = next; }
        Ok(cancelled)
    }
}

impl crate::team::Team {
    pub async fn cancel_task(self: &std::sync::Arc<Self>, id: &str) -> Result<bool> {
        let cancelled = self.tasks.cancel_tree(id)?;
        let changed = !cancelled.is_empty();
        if !changed { return Ok(false); }
        for task in cancelled {
            self.stop_task_delivery(&task).await;
        }
        self.work_changed().await;
        self.kick_workflows();
        Ok(changed)
    }

    async fn stop_task_delivery(self: &std::sync::Arc<Self>, task: &Task) {
        let id = task.id.as_str();
        let conversation = task.target_conversation.clone().unwrap_or_else(|| format!("bot:{}", task.target));
        let (active, pending) = {
            let mut runs = self.runs.lock().await;
            let Some(run) = runs.get_mut(&conversation) else { return };
            if run.task.as_deref() == Some(id) { (true, None) }
            else { (false, run.queue.iter().position(|i| i.task.as_deref() == Some(id)).map(|at| run.queue.remove(at))) }
        };
        if active { self.stop_active(&conversation).await; }
        else if let Some(inbound) = pending { self.reject_inbound(inbound, "This queued task was cancelled by the user.").await; self.work_changed().await; }
    }

    pub async fn delegate(self: &std::sync::Arc<Self>, source: &str, target: &crate::store::Bot, text: &str, files: Option<Vec<String>>, direct: bool) -> Result<String> {
        let files = match files { Some(files) => files, None => self.delegation_inputs(source, text).await? };
        self.delegate_with(source, target, text, files, direct, None).await
    }

    pub async fn delegate_with(self: &std::sync::Arc<Self>, source: &str, target: &crate::store::Bot, text: &str, files: Vec<String>, direct: bool, assignment: Option<AssignmentDetails>) -> Result<String> {
        use crate::team::Conv;
        let execution = self.execution_epoch(source).await;
        let state = self.state().await;
        let sender = Conv::parse(source).and_then(|c| state.bots.get(c.bot()).cloned()).ok_or_else(|| anyhow!("Unknown sender"))?;
        if !state.bots.values().any(|bot| bot.id == target.id) { bail!("The recipient no longer exists."); }
        if let Some(room_id) = Conv::parse(source).and_then(|c| c.room().map(str::to_string)) {
            let room = state.rooms.get(&room_id).ok_or_else(|| anyhow!("The group no longer exists."))?;
            if !room.members.contains(&sender.id) { bail!("The sender is no longer a member of this group."); }
            let explicit = assignment.as_ref().is_some_and(|a| a.explicit)
                && room.admin.as_ref().filter(|id| room.members.contains(id)).or_else(|| room.members.first()) == Some(&sender.id);
            if assignment.is_some() && ((!explicit && room.mode != "auto") || !room.members.contains(&target.id)) { bail!("Automatic collaboration settings or group membership changed. Review the current members before assigning again."); }
        }
        let destination = Conv::parse(source).and_then(|c| c.room().map(str::to_string)).and_then(|id| state.rooms.get(&id).filter(|r| r.members.contains(&target.id)).map(|_| Conv::Member { room: id, bot: target.id.clone() }.key())).filter(|_| !direct).unwrap_or_else(|| Conv::Bot(target.id.clone()).key());
        if text.trim().is_empty() { bail!("Write the message."); }
        if files.len() > 20 { bail!("Attach at most 20 files to a task."); }
        // Copy only artifacts already visible to this conversation, never an
        // arbitrary host path. The destination has its own persistent inbox.
        for file in &files { self.authorized_media(source, file).await?; }
        let (parent, request) = {
            let runs = self.runs.lock().await;
            if runs.get(source).is_some_and(|r| r.cancelled) { bail!("Stopped. Do not start more delegated work."); }
            if runs.get(source).is_some_and(|r| !r.corrections.is_empty()) { bail!("用户已纠正当前任务，请先处理最新要求。"); }
            if runs.get(source).filter(|r| r.running).map(|r| (r.since, r.execution_revision)) != execution { bail!("任务已更新，旧委派未发送。"); }
            let parent = runs.get(source).and_then(|r| r.task.clone().or_else(|| r.task_context.clone()));
            let request = parent.as_deref().and_then(|id| self.tasks.get(id)).and_then(|t| t.request).or_else(|| runs.get(source).and_then(|r| r.request.clone()));
            (parent, request)
        };
        let assignment = assignment.map(|a| AssignmentDetails { objective: self.secrets.redact(&a.objective), expected_output: self.secrets.redact(&a.expected_output), acceptance: self.secrets.redact(&a.acceptance), explicit: a.explicit });
        let (task, fresh) = self.tasks.create_in(source, &target.id, &self.secrets.redact(text.trim()), parent.as_deref(), files.clone(), direct, TaskScope { target_conversation: Some(destination.clone()), request, automatic: assignment.is_some(), assignment })?;
        if fresh && task.automatic {let receipt=self.routing_receipt(target,task.assignment.as_ref().map(|a|a.objective.as_str()).unwrap_or(text));self.tasks.change(&task.id,|t|{t.routing=Some(receipt);Ok(())})?;}
        if self.turn_cancelled(source).await || self.correction_pending(source).await || self.execution_epoch(source).await != execution {
            self.tasks.cancel_tree(&task.id)?;
            self.work_changed().await;
            bail!("Stopped. Do not start more delegated work.");
        }
        if !fresh { return Ok(format!("Task {} is already queued or running on {}. Its result returns automatically; end this dispatch turn and release your execution slot.", task.id, target.name)); }
        let dispatched=async {
            self.track_delegation(&task).await?;
            self.dispatch_task(self.tasks.get(&task.id).unwrap_or_else(||task.clone())).await
        }.await;
        if let Err(error)=dispatched {
            self.tasks.update(&task.id,Status::Failed,Some(self.secrets.redact(&error.to_string())),vec![])?;
            self.kick_workflows();return Err(error);
        }

        if self.turn_cancelled(source).await || self.correction_pending(source).await || self.execution_epoch(source).await != execution {
            self.cancel_task(&task.id).await?;
            bail!("任务已停止或更新，旧委派已撤销。");
        }
        Ok(format!("Sent to {} (task {}). Its reply and files return automatically. Do independent work; never poll.",target.name,task.id))
    }

    pub(crate) async fn dispatch_task(self: &std::sync::Arc<Self>, mut task: Task) -> Result<()> {
        use crate::team::{Conv, Inbound, Origin};
        let state = self.state().await;
        let source_key = task.source.clone();
        let source = source_key.as_str();
        let sender = Conv::parse(source).and_then(|c|state.bots.get(c.bot())).ok_or_else(||anyhow!("指派 Bot 已不存在。"))?;
        let target = state.bots.get(&task.target).ok_or_else(||anyhow!("执行 Bot 已不存在。"))?;
        if let Some(room) = Conv::parse(source).and_then(|c|c.room().map(str::to_string))
            && !state.rooms.get(&room).is_some_and(|r|r.members.contains(&sender.id)) { bail!("指派 Bot 已不在群组中。"); }
        if !matches!(task.status,Status::Queued) || task.cancel_requested || task.pause_requested { return Ok(()); }
        let base = task.target_conversation.clone().unwrap_or_else(||format!("bot:{}",task.target));
        let isolated = task.workflow.is_some() || {
            let runs=self.runs.lock().await;
            runs.iter().any(|(key,r)|r.running && (key==&base || key.starts_with(&format!("{base}~task~"))))
        };
        let destination = if isolated && !base.contains("~task~") { format!("{base}~task~{}",task.id) } else { base };
        task=self.tasks.change(&task.id,|saved|{
            saved.target_conversation=Some(destination.clone());
            if isolated && saved.workspace.is_none() { saved.workspace=Some(self.cfg.home.join("task-workspaces").join(&saved.id).display().to_string()); }
            Ok(())
        })?;
        self.prepare_project(&task).await?;
        task=self.tasks.get(&task.id).ok_or_else(||anyhow!("任务已不存在。"))?;
        let files=task.inputs.clone();
        let direct=task.direct;
        let prepared: Result<Inbound> = async {
            let mut paths = Vec::new();
            let mut attached = Vec::new();
            let source_items = self.chats.read(&crate::team::view_of(source)).await;
            let local_files = self.local_computer.enabled() || self.desktops.is_none();
            for src in files {
                if attached.iter().any(|f: &Value| f["src"] == src) { continue; }
                let source_file = self.authorized_media(source, &src).await?;
                if local_files { paths.push(src.clone()); }
                else {
                    paths.push(self.copy_media_to(&target.id, &src).await?);
                }
                let metadata = source_items.iter().find(|i| i["src"] == src).or_else(|| source_items.iter().flat_map(|i| i["files"].as_array().into_iter().flatten()).find(|f| f["src"] == src));
                let name = metadata.and_then(|f| f["name"].as_str()).map(String::from).unwrap_or_else(|| source_file.file_name().unwrap_or_default().to_string_lossy().into_owned());
                let mime = metadata.and_then(|f| f["mime"].as_str()).map(String::from).unwrap_or_else(|| mime_guess::from_path(&source_file).first_or_octet_stream().to_string());
                let record = crate::chats::item("file", serde_json::json!({
                    "src": src, "name": name, "mime": mime, "bot": sender.id,
                    "size": tokio::fs::metadata(&source_file).await?.len(), "caption": "Delegated attachment"
                }));
                if crate::team::view_of(source) != crate::team::view_of(&destination) {
                    self.record(&crate::team::view_of(&destination), record.clone()).await;
                }
                attached.push(record);
            }
            let suffix = if paths.is_empty() { String::new() } else if local_files {
                format!("\n\nAttached media references (native images/PDFs may be included; use the enabled receiving tools for other files):\n{}", paths.join("\n"))
            } else { format!("\n\nAttached files on your Docker desktop:\n{}", paths.join("\n")) };
            let heading = if direct { format!("[Message from {} · the user sees your reply here]", sender.name) } else { format!("[Message from {}]", sender.name) };
            let origin = if direct { Origin::Other } else { Origin::FromBot { conversation: source.into(), reply: true } };
            let mut inbound = Inbound::new(format!("{heading}\n{}{suffix}", task.text), origin);
            inbound.from = Some(sender.id.clone());
            inbound.task = Some(task.id.clone());
            inbound.sinks = self.runs.lock().await.get(source).map(|r| r.sinks.clone()).unwrap_or_default();
            let (context, native) = self.attachment_context(&crate::team::view_of(&destination), &attached).await?;
            inbound.text.push_str(&context); inbound.images = native;
            Ok(inbound)
        }.await;
        let inbound = match prepared {
            Ok(inbound) => inbound,
            Err(error) => {
                self.tasks.update(&task.id, Status::Failed, Some(self.secrets.redact(&format!("Attachment preparation failed: {error}"))), vec![])?;
                self.work_changed().await;
                return Err(error);
            }
        };
        self.deliver(&destination, inbound).await;
        Ok(())
    }

    pub async fn authorized_media(&self, conversation: &str, src: &str) -> Result<PathBuf> {
        let Some(name) = src.strip_prefix("media/") else { bail!("Use a media reference returned by send_file or a file in this chat."); };
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." { bail!("Invalid media reference"); }
        let items = self.chats.read(&crate::team::view_of(conversation)).await;
        let visible = items.iter().any(|item| item["src"].as_str() == Some(src)
            || item["images"].as_array().is_some_and(|a| a.iter().any(|i| i.as_str() == Some(src)))
            || item["files"].as_array().is_some_and(|a| a.iter().any(|f| f["src"].as_str() == Some(src))));
        if !visible { bail!("That file is not attached to this conversation. Ask its owner to share it."); }
        let media = self.chats.media_dir().canonicalize()?;
        let path = media.join(name).canonicalize()?;
        if !path.starts_with(&media) || !path.is_file() { bail!("Invalid media file"); }
        Ok(path)
    }

    async fn copy_media_to(&self, bot: &str, src: &str) -> Result<String> {
        let desktops = self.desktops.as_ref().ok_or_else(|| anyhow!("Desktops are off; file transfer needs the recipient's desktop."))?;
        let name = src.strip_prefix("media/").ok_or_else(|| anyhow!("Invalid media reference"))?;
        let inbox = desktops.host_dir(bot, "/config/inbox").ok_or_else(|| anyhow!("Cannot create the recipient's inbox"))?;
        let destination = format!("{}-{}", uuid::Uuid::new_v4().simple(), crate::chats::original_name(name));
        let mut input = tokio::fs::File::open(self.chats.media_dir().join(name)).await?;
        let mut output = tokio::fs::OpenOptions::new().write(true).create_new(true).open(inbox.join(&destination)).await?;
        tokio::io::copy(&mut input, &mut output).await?;
        Ok(format!("/config/inbox/{destination}"))
    }

    pub async fn receive_file(&self, conversation: &str, src: &str) -> Result<String> {
        self.authorized_media(conversation, src).await?;
        let conv = crate::team::Conv::parse(conversation).ok_or_else(|| anyhow!("Unknown conversation"))?;
        self.copy_media_to(conv.bot(), src).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipts_deduplicate_and_prevent_cycles_and_survive_restarts() {
        let home = tempfile::tempdir().unwrap();
        let tasks = Tasks::open(home.path()).unwrap();
        let (first, new) = tasks.create("bot:a", "b", "check sources", None, vec![], false).unwrap();
        assert!(new);
        assert!(!tasks.create("bot:a", "b", "check sources", None, vec![], false).unwrap().1);
        assert!(tasks.create("bot:b", "a", "send it back", Some(&first.id), vec![], false).is_err());
        let (second, _) = tasks.create("bot:b", "c", "find primary source", Some(&first.id), vec![], false).unwrap();
        assert_eq!(second.path, ["a", "b", "c"]);
        tasks.update(&first.id, Status::Completed, Some("verified".into()), vec![]).unwrap();
        let reopened = Tasks::open(home.path()).unwrap();
        assert_eq!(reopened.get(&first.id).unwrap().status, Status::Completed);
        assert_eq!(reopened.get(&second.id).unwrap().status, Status::Interrupted);
        assert!(tasks.create("bot:a", "b", "check sources", None, vec![], false).unwrap().1);
    }

    #[test]
    fn corrections_keep_the_receipt_inputs_and_status_and_survive_reopening() {
        let home = tempfile::tempdir().unwrap();
        let tasks = Tasks::open(home.path()).unwrap();
        let (task, _) = tasks.create("bot:a", "b", "原始任务", None, vec!["media/original.txt".into()], false).unwrap();
        tasks.update(&task.id, Status::Running, None, vec![]).unwrap();
        let started = tasks.get(&task.id).unwrap().started;
        tasks.correct(&task.id, "使用新要求", &["media/extra.txt".into(), "media/original.txt".into()]).unwrap();
        let revised = tasks.get(&task.id).unwrap();
        assert_eq!(revised.status, Status::Running);
        assert_eq!(revised.started, started);
        assert_eq!(revised.text, "原始任务");
        assert_eq!(revised.inputs, vec!["media/original.txt", "media/extra.txt"]);
        tasks.update(&task.id, Status::Completed, Some("交付结果".into()), vec![]).unwrap();
        assert!(tasks.correct(&task.id, "迟到的纠正", &[]).is_err());
        let reopened = Tasks::open(home.path()).unwrap();
        assert_eq!(reopened.get(&task.id).unwrap().corrections.len(), 1);
        assert_eq!(reopened.get(&task.id).unwrap().corrections[0]["text"], "使用新要求");
    }

    #[test]
    fn distinct_parent_jobs_do_not_share_receipts_and_cancellation_covers_descendants() {
        let home = tempfile::tempdir().unwrap();
        let tasks = Tasks::open(home.path()).unwrap();
        let (first, _) = tasks.create("bot:a", "b", "first project", None, vec![], false).unwrap();
        let (other, _) = tasks.create("bot:x", "b", "other project", None, vec![], false).unwrap();
        let (child, _) = tasks.create("bot:b", "c", "same review", Some(&first.id), vec![], false).unwrap();
        let (separate, fresh) = tasks.create("bot:b", "c", "same review", Some(&other.id), vec![], false).unwrap();
        assert!(fresh);
        assert_ne!(child.id, separate.id);
        let (grandchild, _) = tasks.create("bot:c", "d", "verify", Some(&child.id), vec![], false).unwrap();
        tasks.update(&first.id, Status::Completed, Some("waiting for review".into()), vec![]).unwrap();
        let stopped = tasks.cancel_tree(&first.id).unwrap();
        assert_eq!(stopped.len(), 2);
        assert_eq!(tasks.get(&separate.id).unwrap().status, Status::Queued);
        assert_eq!(tasks.get(&first.id).unwrap().status, Status::Completed);
        assert!(tasks.create("bot:b", "e", "late task", Some(&first.id), vec![], false).is_err());
        assert!(tasks.create("bot:c", "e", "late task", Some(&child.id), vec![], false).is_err());
        let reopened = Tasks::open(home.path()).unwrap();
        assert_eq!(reopened.get(&grandchild.id).unwrap().status, Status::Cancelled);
        assert!(reopened.get(&first.id).unwrap().cancel_requested);
    }

    #[tokio::test]
    async fn files_transfer_to_a_separate_inbox_only_when_attached_to_the_chat() {
        use crate::team::{Config, Team};
        let home = tempfile::tempdir().unwrap();
        let config = Config { home: home.path().into(), self_exe: "unused".into(), url: "http://127.0.0.1:1".into(), public_url: None, internal_token: "test".into(), workspace: home.path().join("work") };
        let team = Team::open(config, None, Some(crate::desktop::Desktops::new("unused".into(), home.path().join("desktops")))).unwrap();
        let owner = team.state().await.bots.values().next().unwrap().id.clone();
        let own = format!("bot:{owner}");
        let other = team.create_bot_with_description("Other", "Reviewer", "check", None, None, None).await.unwrap();
        std::fs::create_dir_all(home.path().join("media")).unwrap();
        std::fs::write(home.path().join("media/report.txt"), "verified evidence").unwrap();
        team.record(&own, crate::chats::item("file", serde_json::json!({"src":"media/report.txt"}))).await;
        let path = team.receive_file(&own, "media/report.txt").await.unwrap();
        let host = team.desktops.as_ref().unwrap().host_path(&owner, &path).unwrap();
        assert_eq!(std::fs::read_to_string(host).unwrap(), "verified evidence");
        assert!(team.receive_file(&format!("bot:{}", other.id), "media/report.txt").await.is_err());
        assert!(team.receive_file(&own, "media/../state.json").await.is_err());
        assert!(team.receive_file(&own, "/etc/passwd").await.is_err());
    }
}
