//! Durable per-part platform delivery. A crash during a send is ambiguous and
//! requires review; confirmed parts are never replayed by an ordinary retry.
use crate::{
    durable,
    store::now_ms,
    team::{Reply, Sink, Team},
};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    #[serde(default)]
    pub trace: Option<String>,
    #[serde(default)]
    pub diagnostic_progress: bool,
    pub channel: String,
    pub recipient: String,
    pub target: String,
    pub kind: String,
    pub max: usize,
    pub data: Value,
    pub require_allowed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum Part {
    Text(String),
    File(Value),
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub id: String,
    pub route: Route,
    pub parts: Vec<Part>,
    pub cursor: usize,
    pub status: String,
    pub created: i64,
    pub updated: i64,
    pub attempts: u32,
    pub error: Option<String>,
    pub sequence: u32,
}
pub struct Outbox {
    path: PathBuf,
    list: Mutex<Vec<Delivery>>,
    pub notify: tokio::sync::Notify,
}
impl Outbox {
    pub fn open(home: &Path) -> Result<Self> {
        let path = home.join("outbox.json");
        let mut list: Vec<Delivery> = match crate::sqlite_storage::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("reading outbox.json")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        for entry in list.iter_mut().filter(|e| e.status == "sending") {
            entry.status = "uncertain".into();
            entry.error =
                Some("服务在发送过程中中断，请核对平台是否已收到当前片段后再重试。".into());
        }
        durable::save(&path, &list)?;
        Ok(Self {
            path,
            list: Mutex::new(list),
            notify: Default::default(),
        })
    }
    /// What became of each final delivery that carries a diagnostics trace: (trace, status, last
    /// update). Workflow progress messages are left out: they never close a trace.
    pub fn trace_states(&self) -> Vec<(String, String, i64)> {
        self.list.lock().unwrap().iter().filter(|d| !d.route.diagnostic_progress).filter_map(|d| d.route.trace.clone().map(|trace| (trace, d.status.clone(), d.updated))).collect()
    }
    pub fn view(&self) -> Vec<Value> {
        let mut recent = 0;
        self.list.lock().unwrap().iter().rev().filter(|d|{if !matches!(d.status.as_str(),"delivered"|"cancelled"){true}else{recent+=1;recent<=100}}).map(|d|json!({"id":d.id,"channel":d.route.channel,"target":d.route.target,"recipient":d.route.recipient,"status":d.status,"sent":d.cursor,"total":d.parts.len(),"created":d.created,"updated":d.updated,"attempts":d.attempts,"error":d.error})).collect()
    }
    pub fn enqueue(&self, route: Route, reply: &Reply) -> Result<String> {
        // Diagnostics must not change existing delivery identity on upgrade.
        let mut identity=serde_json::to_value(&route)?;
        let fields=identity.as_object_mut().unwrap();fields.shift_remove("diagnosticProgress");
        if route.trace.is_none(){fields.shift_remove("trace");}
        let id = format!(
            "delivery-{}",
            hex::encode(Sha256::digest(serde_json::to_vec(&json!([identity, reply]))?))
        );
        let mut list = self.list.lock().unwrap();
        if list.iter().any(|d| d.id == id) {
            return Ok(id);
        }
        let mut parts = crate::channels::pieces(&reply.platform_text(), route.max)
            .into_iter()
            .map(Part::Text)
            .collect::<Vec<_>>();
        parts.extend(reply.files.iter().cloned().map(Part::File));
        if parts.is_empty() {
            return Ok(id);
        }
        let sequence = 1 + list
            .iter()
            .filter(|d| d.route.channel == route.channel && d.route.data == route.data)
            .map(|d| d.parts.len() as u32)
            .sum::<u32>();
        let entry = Delivery {
            id: id.clone(),
            route,
            parts,
            cursor: 0,
            status: "queued".into(),
            created: now_ms(),
            updated: now_ms(),
            attempts: 0,
            error: None,
            sequence,
        };
        let mut next = list.clone();
        while next.len() >= 1000 {
            if let Some(at) = next
                .iter()
                .position(|d| matches!(d.status.as_str(), "delivered" | "cancelled"))
            {
                next.remove(at);
            } else {
                break;
            }
        }
        next.push(entry);
        durable::save(&self.path, &next)?;
        *list = next;
        self.notify.notify_one();
        Ok(id)
    }
    pub fn next(&self) -> Option<Delivery> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .find(|d| d.status == "queued")
            .cloned()
    }
    pub fn change(&self, id: &str, f: impl FnOnce(&mut Delivery)) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let entry = next
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow!("Unknown delivery"))?;
        f(entry);
        entry.updated = now_ms();
        durable::save(&self.path, &next)?;
        *list = next;
        Ok(())
    }
    pub fn retry(&self, id: &str, reviewed: bool, skip: bool) -> Result<()> {
        let mut list = self.list.lock().unwrap();
        let mut next = list.clone();
        let entry = next
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow!("Unknown delivery"))?;
        if !matches!(entry.status.as_str(), "failed" | "uncertain" | "blocked") {
            bail!("Only failed, blocked or uncertain deliveries can be retried.");
        }
        if entry.status == "uncertain" && !reviewed {
            bail!(
                "Check whether the current part already arrived, then acknowledge the review before retrying."
            );
        }
        if skip && entry.status == "uncertain" {
            entry.cursor = (entry.cursor + 1).min(entry.parts.len());
        }
        entry.status = if entry.cursor == entry.parts.len() {
            "delivered"
        } else {
            "queued"
        }
        .into();
        entry.error = None;
        entry.updated = now_ms();
        durable::save(&self.path, &next)?;
        *list = next;
        self.notify.notify_one();
        Ok(())
    }
}
impl Team {
    pub(crate) fn platform_sink(self: &Arc<Self>, route: Route) -> Sink {
        let team = Arc::downgrade(self);
        let destination = route.clone();
        Sink::new(Some(route), move |mut reply| {
            if let Some(team) = team.upgrade() {
                if let Some(channel) = team.channels.get(&destination.channel) {
                    reply.text = crate::channels::redact_channel(&channel, &reply.text, &team);
                    reply.error = reply
                        .error
                        .map(|e| crate::channels::redact_channel(&channel, &e, &team));
                }
                let mut outgoing=destination.clone();
                outgoing.diagnostic_progress=destination.trace.as_ref().is_some_and(|trace|team.workflows.list().iter().any(|flow|matches!(flow.status.as_str(),"running"|"reviewing")&&flow.routes.iter().any(|r|r.trace.as_ref()==Some(trace))));
                if let Some(id)=&destination.trace{team.channel_diagnostics.mark(id,if outgoing.diagnostic_progress{"progress_generated"}else{"generated"},None);}
                if let Err(error) = team.outbox.enqueue(outgoing, &reply) {
                    eprintln!("dotbot: cannot persist platform reply: {error}");
                    team.channels.delivery_error(
                        &destination.channel,
                        "回复无法写入持久发送队列，请检查磁盘。".into(),
                    );
                }
            }
        })
    }
    pub(crate) fn route_allowed(&self, route: &Route) -> bool {
        self.channels.get(&route.channel).is_some_and(|c| {
            c.enabled
                && c.kind == route.kind
                && c.target == route.target
                && (!route.require_allowed
                    || c.allow.iter().any(|a| a == "*" || a == &route.recipient))
        })
    }
    pub fn start_outbox(self: &Arc<Self>) {
        let team = self.clone();
        tokio::spawn(async move {
            loop {
                if team
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    break;
                }
                let Some(entry) = team.outbox.next() else {
                    tokio::select! {_=team.outbox.notify.notified()=>{},_=tokio::time::sleep(std::time::Duration::from_secs(1))=>{}}
                    continue;
                };
                if !team.route_allowed(&entry.route) {
                    let _ = team.outbox.change(&entry.id, |d| {
                        d.status = "blocked".into();
                        d.error = Some("平台已暂停、目标已改变或发送者授权已撤销。".into());
                    });
                    if let Some(id)=&entry.route.trace{team.channel_diagnostics.mark(id,"blocked",Some("平台暂停或发送授权已变更。".into()));}
                    team.team_changed().await;
                    continue;
                }
                let result:Result<()>=async{
                    for index in entry.cursor..entry.parts.len(){
                        if !team.route_allowed(&entry.route){bail!("发送权限已改变，请核对设置后重试。");}
                        let outgoing=match &entry.parts[index]{
                            Part::Text(text)=>crate::channel_media::Outgoing::Text(team.secrets.redact(text)),
                            Part::File(file)=>{
                                let path=team.authorized_media(&entry.route.target,file["src"].as_str().unwrap_or_default()).await?;
                                if tokio::fs::metadata(&path).await?.len()>crate::attachments::FILE_MAX as u64{bail!("附件超过 25 MB，请从网页下载。");}
                                crate::channel_media::Outgoing::File(crate::channel_media::PlatformFile{name:file["name"].as_str().unwrap_or("file.bin").into(),mime:file["mime"].as_str().unwrap_or("application/octet-stream").into(),bytes:tokio::fs::read(path).await?})
                            }
                        };
                        if !team.route_allowed(&entry.route){bail!("发送权限已改变，请核对设置后重试。");}
                        team.outbox.change(&entry.id,|d|{d.status="sending".into();d.attempts+=1;})?;
                        if let Some(id)=&entry.route.trace{team.channel_diagnostics.mark(id,if entry.route.diagnostic_progress{"progress_sending"}else{"sending"},None);}
                        match team.send_delivery(&entry.route,outgoing,&format!("{}-{index}",entry.id),entry.sequence+index as u32,index==0).await{
                            Ok(())=>team.outbox.change(&entry.id,|d|{d.cursor=index+1;d.status=if d.cursor==d.parts.len(){"delivered"}else{"queued"}.into();d.error=None;})?,
                            Err(error)=>{
                                let detail=team.channels.get(&entry.route.channel).map(|c|crate::channels::channel_error(&c,&error,&team)).unwrap_or_else(||"平台发送失败".into());
                                // Transport failure can mean the provider accepted a request before the connection dropped.
                                let ambiguous=error.downcast_ref::<reqwest::Error>().is_some_and(|e|!e.is_status()||e.status().is_some_and(|s|s.is_server_error()));
                                team.outbox.change(&entry.id,|d|{d.status=if ambiguous{"uncertain"}else{"failed"}.into();d.error=Some(detail.clone());})?;
                                team.channels.delivery_error(&entry.route.channel,detail);
                                if let Some(id)=&entry.route.trace{team.channel_diagnostics.mark(id,"failed",Some("平台投递失败，请查看发送队列中的脱敏错误。".into()));}
                                if matches!(entry.parts[index],Part::File(_)) {
                                    team.platform_sink(entry.route.clone()).send(format!("附件发送失败，已生成的文件仍可在 dotbot 网页原会话中下载。发送队列保留了进度，可由主人核对后重试。记录：{}",entry.id).into());
                                }
                                return Ok(());
                            }
                        }
                    }
                    team.channels.delivery_sent(&entry.route.channel);
                    if let Some(id)=&entry.route.trace{team.channel_diagnostics.mark(id,if entry.route.diagnostic_progress{"progress_sent"}else{"sent"},None);}
                    Ok(())
                }.await;
                if let Err(error) = result {
                    let detail = team.secrets.redact(&error.to_string());
                    let _ = team.outbox.change(&entry.id, |d| {
                        d.status = if d.status == "sending" {
                            "uncertain"
                        } else {
                            "failed"
                        }
                        .into();
                        d.error = Some(detail.clone());
                    });
                    if let Some(id)=&entry.route.trace{team.channel_diagnostics.mark(id,"failed",Some("平台投递失败，请查看发送队列中的脱敏错误。".into()));}
                    team.channels.delivery_error(&entry.route.channel, detail);
                }
                team.team_changed().await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_checkpoints_survive_reopen_and_require_review_for_ambiguous_sends() {
        let home = tempfile::tempdir().unwrap();
        let queue = Outbox::open(home.path()).unwrap();
        let route = Route { diagnostic_progress:false,
            trace: None,
            channel: "c".into(),
            recipient: "u".into(),
            target: "bot:b".into(),
            kind: "telegram".into(),
            max: 4,
            data: json!({"message":1}),
            require_allowed: true,
        };
        let reply: Reply = "abcdefgh".into();
        let id = queue.enqueue(route.clone(), &reply).unwrap();
        assert_eq!(queue.enqueue(route.clone(), &reply).unwrap(), id);
        let mut legacy=serde_json::to_value(&route).unwrap();legacy.as_object_mut().unwrap().shift_remove("trace");legacy.as_object_mut().unwrap().shift_remove("diagnosticProgress");
        assert_eq!(id,format!("delivery-{}",hex::encode(Sha256::digest(serde_json::to_vec(&json!([legacy,reply])).unwrap()))));
        let mut progress=route;progress.diagnostic_progress=true;
        assert_eq!(queue.enqueue(progress,&reply).unwrap(),id);
        assert_eq!(queue.view().len(), 1);
        queue
            .change(&id, |d| {
                d.cursor = 1;
                d.status = "sending".into();
            })
            .unwrap();
        drop(queue);
        let queue = Outbox::open(home.path()).unwrap();
        assert_eq!(queue.view()[0]["status"], "uncertain");
        assert!(queue.retry(&id, false, false).is_err());
        queue.retry(&id, true, true).unwrap();
        assert_eq!(queue.view()[0]["status"], "delivered");
        assert_eq!(queue.view()[0]["sent"], 2);
        assert!(queue.next().is_none());
    }
}
