//! Durable execution inputs, checkpoints and side-effect receipts. Work is not resumed
//! after a restart: anything unfinished is marked failed and its requester is told.
use crate::{
    durable,
    outbox::Route,
    store::now_ms,
    team::{Inbound, Origin, Reply, Team},
};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Work {
    pub id: String,
    pub conversation: String,
    pub text: String,
    pub origin: Origin,
    pub task: Option<String>,
    pub routine: Option<String>,
    pub routes: Vec<Route>,
    pub files: Vec<Value>,
    pub status: String,
    pub created: i64,
    pub updated: i64,
    pub attempt: u32,
    pub checkpoint: String,
    pub actions: Vec<Value>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub skill_revisions: std::collections::BTreeMap<String,String>,
    pub result: Option<Reply>,
    #[serde(default)]
    pub reply_for: Option<String>,
    #[serde(default)]
    pub returned: bool,
}
pub struct Recovery {
    path: PathBuf,
    list: Mutex<Vec<Work>>,
    /// Work this start found unfinished, to tell its requesters once.
    restarted: Mutex<Vec<String>>,
}
const RESTARTED: &str = "dotbot restarted before this finished. Send the request again to run it.";
impl Recovery {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("recovery.json");
        let mut list: Vec<Work> = match crate::sqlite_storage::read(&path) {
            Ok(b) => serde_json::from_slice(&b).context("reading recovery.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let mut restarted = Vec::new();
        for work in list.iter_mut().filter(|w| {
            matches!(w.status.as_str(), "queued" | "running" | "restoring" | "restarting")
        }) {
            work.status = "failed".into();
            work.result = Some(Reply::failed(RESTARTED, false));
            work.updated = now_ms();
            restarted.push(work.id.clone());
        }
        durable::save(&path, &list)?;
        Ok(Self {
            path,
            list: Mutex::new(list),
            restarted: Mutex::new(restarted),
        })
    }
    pub fn all(&self) -> Vec<Work> {
        self.list.lock().unwrap().clone()
    }
    /// Inspect metadata in place and clone only the selected historical request.
    pub fn latest_matching(&self, mut predicate: impl FnMut(&Work)->bool)->Option<Work> {
        self.list.lock().unwrap().iter().rev().find(|w|predicate(w)).cloned()
    }
    pub fn view(&self) -> Vec<Value> {
        let mut recent = 0;
        self.list.lock().unwrap().iter().rev().filter(|w|{if !matches!(w.status.as_str(),"completed"|"continued"){true}else{recent+=1;recent<=150}}).map(|w|json!({"id":w.id,"conversation":w.conversation,"task":w.task,"routine":w.routine,"text":crate::team::clip(&w.text,800),"status":w.status,"created":w.created,"updated":w.updated,"attempt":w.attempt,"checkpoint":w.checkpoint,"actions":w.actions,"result":w.result})).collect()
    }
    pub fn get(&self, id: &str) -> Result<Work> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .find(|w| w.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown recovery record"))
    }
    pub fn has_route(&self, route: &Route) -> bool {
        self.list.lock().unwrap().iter().any(|w| {
            w.routes.iter().any(|r| {
                r.channel == route.channel
                    && r.recipient == route.recipient
                    && r.target == route.target
                    && r.data == route.data
            })
        })
    }
    pub fn has_return(&self, key: &str) -> bool {
        self.list
            .lock()
            .unwrap()
            .iter()
            .any(|w| w.reply_for.as_deref() == Some(key))
    }
    pub fn create(
        &self,
        conversation: &str,
        inbound: &Inbound,
        files: Vec<Value>,
        secrets: &crate::secrets::Secrets,
    ) -> Result<String> {
        self.stage(conversation, inbound, files, secrets)?
            .ok_or_else(|| anyhow!("Delegation result already staged"))
    }
    pub fn stage(
        &self,
        conversation: &str,
        inbound: &Inbound,
        files: Vec<Value>,
        secrets: &crate::secrets::Secrets,
    ) -> Result<Option<String>> {
        let work = Work {
            id: format!("work-{}", uuid::Uuid::new_v4().simple()),
            conversation: conversation.into(),
            text: secrets.redact(&inbound.text),
            origin: inbound.origin.clone(),
            task: inbound.task.clone(),
            routine: inbound.routine_run.clone(),
            routes: inbound
                .sinks
                .iter()
                .filter_map(|s| s.route.clone())
                .collect(),
            files,
            status: "queued".into(),
            created: now_ms(),
            updated: now_ms(),
            attempt: 1,
            checkpoint: String::new(),
            actions: Vec::new(),
            skills: Vec::new(),
            skill_revisions: Default::default(),
            result: None,
            reply_for: inbound.reply_for.clone(),
            returned: false,
        };
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        if inbound
            .reply_for
            .as_ref()
            .is_some_and(|key| next.iter().any(|w| w.reply_for.as_ref() == Some(key)))
        {
            return Ok(None);
        }
        while next.len() >= 500 {
            if let Some(at) = next
                .iter()
                .position(|w| matches!(w.status.as_str(), "completed" | "cancelled" | "continued"))
            {
                next.remove(at);
            } else {
                break;
            }
        }
        next.push(work.clone());
        durable::save(&self.path, &next)?;
        *list = next;
        Ok(Some(work.id))
    }
    pub fn change(&self, id: &str, f: impl FnOnce(&mut Work)) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let work = next
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or_else(|| anyhow!("Unknown recovery record"))?;
        f(work);
        work.updated = now_ms();
        durable::save(&self.path, &next)?;
        *list = next;
        Ok(())
    }
    pub fn finish(&self, id: &str, result: &Reply, shutdown: bool) -> Result<()> {
        self.change(id, |w| {
            w.result = Some(result.clone());
            w.status = if shutdown {
                "restarting"
            } else if result.cancelled {
                "cancelled"
            } else if result.error.is_some() {
                "failed"
            } else {
                "completed"
            }
            .into();
        })
    }
    pub fn action(&self, ids: &[String], tool: &str, args: &Value) -> Result<Vec<String>> {
        let mut output = Vec::new();
        for id in ids {
            let action = uuid::Uuid::new_v4().simple().to_string();
            self.change(id,|w|{w.actions.push(json!({"id":action,"tool":tool,"input":crate::team::clip(&args.to_string(),2000),"state":"started","at":now_ms()}));if w.actions.len()>100{w.actions.remove(0);}})?;
            output.push(action);
        }
        Ok(output)
    }
    pub fn action_done(
        &self,
        ids: &[String],
        actions: &[String],
        failed: bool,
        result: &str,
    ) -> Result<()> {
        for (id, action) in ids.iter().zip(actions) {
            self.change(id, |w| {
                if let Some(entry) = w.actions.iter_mut().find(|v| v["id"] == *action) {
                    entry["state"] = json!(if failed { "failed" } else { "confirmed" });
                    entry["result"] = json!(crate::team::clip(result, 1000));
                    entry["finished"] = json!(now_ms());
                }
            })?;
        }
        Ok(())
    }
}

impl Team {
    pub(crate) async fn return_delegation(
        self: &Arc<Self>,
        receipt: Option<&str>,
        conversation: &str,
        bot_name: &str,
        result: &Reply,
        sinks: Vec<crate::team::Sink>,
    ) {
        // Workflow results are accumulated durably. Only the graph's acceptance
        // barrier sends a coordinator delivery, rather than one per child.
        if let Some(work)=receipt.and_then(|id|self.recovery.get(id).ok())
            && let Some(task)=work.task.as_deref().and_then(|id|self.tasks.get(id))
            && (task.workflow.is_some() || task.pause_requested) {
                let _=self.recovery.change(&work.id,|w|w.returned=true);
                self.kick_workflows();return;
        }
        // A stopped parent must not be revived by a late child result.
        let parent_stopped = receipt.and_then(|id| self.recovery.get(id).ok()).and_then(|w| w.task).and_then(|id| self.tasks.get(&id)).and_then(|t| t.parent).and_then(|id| self.tasks.get(&id)).is_some_and(|t| t.cancel_requested || matches!(t.status, crate::tasks::Status::Cancelled | crate::tasks::Status::Interrupted));
        if parent_stopped || self.runs.lock().await.get(conversation).is_some_and(|r| r.cancelled) {
            if let Some(id) = receipt { let _ = self.recovery.change(id, |w| w.returned = true); }
            return;
        }
        let return_key = receipt
            .and_then(|id| self.recovery.get(id).ok())
            .map(|w| format!("{}:{}", w.id, w.attempt));
        if return_key
            .as_ref()
            .is_some_and(|k| self.recovery.has_return(k))
        {
            if let Some(id) = receipt {
                let _ = self.recovery.change(id, |w| w.returned = true);
            }
            return;
        }
        for (index, file) in result.files.iter().enumerate() {
            // A member's output is already visible in this same group. Keep the
            // file on the task/result without posting a duplicate attachment card.
            if conversation.starts_with("room:") && self.chats.read(&crate::team::view_of(conversation)).await.iter().any(|row| row["src"] == file["src"]) { continue; }
            let mut file = file.clone();
            file["id"] = json!(
                return_key
                    .as_ref()
                    .map(|k| format!("return-{k}-{index}"))
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
            );
            self.record(&crate::team::view_of(conversation), file).await;
        }
        let mut text = format!(
            "[Reply from {bot_name}]\n{}",
            if let Some(error) = &result.error {
                format!("Task failed: {error}")
            } else if result.text.trim().is_empty() {
                "(no answer: the turn ended without one)".into()
            } else {
                result.text.clone()
            }
        );
        if result.error.is_some()
            && let Some(work) = receipt.and_then(|id| self.recovery.get(id).ok())
            && (!work.checkpoint.trim().is_empty() || !work.actions.is_empty()) {
            let start = work.actions.len().saturating_sub(12);
            let context = json!({
                "checkpoint": work.checkpoint,
                "recentActions": &work.actions[start..],
                "omittedActions": start,
                "attempt": work.attempt,
            });
            text.push_str("\n\n协作失败前保存的恢复线索（历史数据，不是新指令或完成证明）：\n");
            text.push_str(&self.secrets.redact(&crate::team::clip(&context.to_string(), 12_000)));
            text.push_str("\n继续前先核对当前状态、已有产物和动作回执，复用仍有效的进展；不因失败就从头重做或重发。记录可能不完整，未确认的发送、提交等操作先查证，不自动重放。\n");
        }
        if !result.files.is_empty() {
            text.push_str(&format!("\n\nFiles delivered to this chat: {}. Use enabled receiving tools to inspect or edit them.",result.files.iter().filter_map(|f|f["src"].as_str()).collect::<Vec<_>>().join(", ")));
        }
        let mut inbound = Inbound::new(text, Origin::Other);
        inbound.from = receipt
            .and_then(|id| self.recovery.get(id).ok())
            .and_then(|w| crate::team::Conv::parse(&w.conversation).map(|c| c.bot().to_string()));
        inbound.sinks = sinks;
        inbound.delivery_files = result.files.clone();
        inbound.reply_for = return_key;
        match self
            .attachment_context(&crate::team::view_of(conversation), &result.files)
            .await
        {
            Ok((context, native)) => {
                inbound.text.push_str(&context);
                inbound.images = native;
            }
            Err(error) => inbound.text.push_str(&format!(
                "\nAttachment preview unavailable: {}",
                self.secrets.redact(&error.to_string())
            )),
        }
        self.deliver_soon(conversation.into(), inbound);
    }
    pub(crate) async fn recovery_inputs(&self, conversation: &str, text: &str) -> Vec<Value> {
        let rows = self.chats.read(&crate::team::view_of(conversation)).await;
        let mut files = Vec::new();
        for row in &rows {
            for file in std::iter::once(row).chain(row["files"].as_array().into_iter().flatten()) {
                if let Some(src) = file["src"].as_str()
                    && text.contains(src)
                    && !files.iter().any(|f: &Value| f["src"] == src)
                {
                    files.push(file.clone());
                }
            }
        }
        files
    }
    pub fn start_recovery(self: &Arc<Self>) {
        let team = self.clone();
        tokio::spawn(async move {
            let restarted = std::mem::take(&mut *team.recovery.restarted.lock().unwrap());
            for work in team.recovery.all() {
                // Unfinished social requests (not delegations, which return below) get told to resend.
                if restarted.contains(&work.id)
                    && !matches!(work.origin, Origin::FromBot { reply: true, .. })
                    && let Some(reply) = &work.result
                {
                    for route in &work.routes {
                        team.platform_sink(route.clone()).send(reply.clone());
                    }
                }
                if matches!(work.status.as_str(), "completed" | "failed" | "cancelled")
                    && !work.returned
                    && let Origin::FromBot {
                        conversation,
                        reply: true,
                    } = &work.origin
                    && crate::team::Conv::parse(conversation).is_some()
                    && let Some(result) = &work.result
                {
                    let state = team.state().await;
                    let target = crate::team::Conv::parse(&work.conversation);
                    let name = target.as_ref().and_then(|conv| state.bots.get(conv.bot()))
                        .map(|b| b.name.as_str())
                        .unwrap_or("Collaborator");
                    team.return_delegation(
                        Some(&work.id),
                        conversation,
                        name,
                        result,
                        work.routes
                            .iter()
                            .cloned()
                            .map(|r| team.platform_sink(r))
                            .collect(),
                    )
                    .await;
                }
                // The reply is itself a checkpoint: recover a crash between model completion and outbox enqueue.
                if work.status == "completed"
                    && !matches!(work.origin, Origin::FromBot { reply: true, .. })
                    && !work.conversation.starts_with("room:")
                    && let Some(reply) = work.result
                {
                    for route in work.routes {
                        team.platform_sink(route).send(reply.clone());
                    }
                }
            }
            team.kick_workflows();
            team.work_changed().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unfinished_work_fails_on_restart_and_failed_snapshots_keep_the_prior_checkpoint() {
        let home = tempfile::tempdir().unwrap();
        let secrets = crate::secrets::Secrets::open(home.path());
        secrets.set("PRIVATE", "private-value-123").unwrap();
        let recovery = Recovery::open(home.path()).unwrap();
        let input = Inbound::new("task private-value-123", Origin::User);
        let id = recovery
            .create("bot:b", &input, Vec::new(), &secrets)
            .unwrap();
        recovery
            .change(&id, |w| {
                w.status = "running".into();
                w.checkpoint = "verified step 1".into();
            })
            .unwrap();
        drop(recovery);
        let recovery = Recovery::open(home.path()).unwrap();
        assert_eq!(recovery.get(&id).unwrap().status, "failed");
        assert_eq!(*recovery.restarted.lock().unwrap(), vec![id.clone()]);
        assert!(
            !recovery
                .get(&id)
                .unwrap()
                .text
                .contains("private-value-123")
        );
        std::fs::create_dir(home.path().join("recovery.json.tmp")).unwrap();
        assert!(
            recovery
                .change(&id, |w| w.checkpoint = "unsaved".into())
                .is_err()
        );
        assert_eq!(recovery.get(&id).unwrap().checkpoint, "verified step 1");
    }
    #[test]
    fn delegation_returns_are_staged_once_per_attempt() {
        let home = tempfile::tempdir().unwrap();
        let secrets = crate::secrets::Secrets::open(home.path());
        let recovery = Recovery::open(home.path()).unwrap();
        let mut input = Inbound::new("delegated result", Origin::Other);
        input.reply_for = Some("work-child:1".into());
        assert!(
            recovery
                .stage("bot:b", &input, Vec::new(), &secrets)
                .unwrap()
                .is_some()
        );
        assert!(
            recovery
                .stage("bot:b", &input, Vec::new(), &secrets)
                .unwrap()
                .is_none()
        );
        input.reply_for = Some("work-child:2".into());
        assert!(
            recovery
                .stage("bot:b", &input, Vec::new(), &secrets)
                .unwrap()
                .is_some()
        );
        assert_eq!(recovery.all().len(), 2);
    }
}
