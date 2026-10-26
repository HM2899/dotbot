//! Correlated local receipt-to-delivery timings, without bodies or sender IDs.
use std::{path::{Path,PathBuf},sync::Mutex};
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use crate::{durable,outbox::Route,store::now_ms};
#[derive(Clone,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
struct Trace {id:String,channel:String,received:i64,queued:Option<i64>,model_started:Option<i64>,model_finished:Option<i64>,send_started:Option<i64>,sent:Option<i64>,status:String,error:Option<String>}
pub struct Diagnostics {path:PathBuf,entries:Mutex<Vec<Trace>>}
impl Diagnostics {
    pub fn open(home:&Path)->Self{let path=home.join("channel-traces.json");let entries=crate::sqlite_storage::read(&path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default();Self{path,entries:Mutex::new(entries)}}
    pub fn received(&self,channel:&str)->String{let id=format!("trace-{}",uuid::Uuid::new_v4().simple());let mut entries=self.entries.lock().unwrap();entries.push(Trace{id:id.clone(),channel:channel.into(),received:now_ms(),queued:None,model_started:None,model_finished:None,send_started:None,sent:None,status:"receiving".into(),error:None});if entries.len()>500{entries.remove(0);}let _=durable::save(&self.path,&*entries);id}
    pub fn mark(&self,id:&str,stage:&str,error:Option<String>){let mut entries=self.entries.lock().unwrap();let Some(trace)=entries.iter_mut().find(|t|t.id==id)else{return;};if stage.starts_with("progress_")&&trace.sent.is_some(){return;}let now=now_ms();match stage{"queued"=>{trace.queued.get_or_insert(now);},"model_started"=>{trace.model_started.get_or_insert(now);},"model_finished"=>{trace.model_finished=Some(now);},"generated"=>{trace.send_started=None;trace.sent=None;},"sending"=>{trace.send_started.get_or_insert(now);},"sent"=>{trace.sent=Some(now);},_=>{}}trace.status=stage.into();trace.error=error;let _=durable::save(&self.path,&*entries);}
    pub fn routes(&self,routes:impl IntoIterator<Item=Route>,stage:&str){for route in routes{if let Some(id)=route.trace{self.mark(&id,stage,None);}}}
    /// A trace the live stages never closed (an older version, a restart mid-delivery) is settled
    /// from what the delivery queue recorded; one with no result for a long time says so.
    pub fn reconcile(&self,deliveries:&[(String,String,i64)]){
        let now=now_ms();let mut entries=self.entries.lock().unwrap();let mut changed=false;
        for trace in entries.iter_mut().filter(|t|t.sent.is_none()&&!matches!(t.status.as_str(),"sent"|"failed"|"blocked"|"incomplete")){
            let own:Vec<_>=deliveries.iter().filter(|(id,_,_)|*id==trace.id).collect();
            if own.is_empty(){
                if now-trace.received>15*60_000{trace.status="incomplete".into();changed=true;}
            }else if own.iter().any(|(_,status,_)|matches!(status.as_str(),"failed"|"uncertain"|"blocked"|"cancelled")){
                trace.status="failed".into();trace.error=Some("平台投递失败或结果不确定，请查看发送队列。".into());changed=true;
            }else if own.iter().all(|(_,status,_)|status=="delivered"){
                // Every part went out: the last one closes the trace.
                let done=own.iter().map(|(_,_,updated)|*updated).max().unwrap_or(trace.received);
                trace.sent=Some(done.max(trace.received));trace.status="sent".into();trace.error=None;changed=true;
            }
        }
        if changed{let _=durable::save(&self.path,&*entries);}
    }
    pub fn view(&self,channel:&str)->Value{let entries=self.entries.lock().unwrap();let traces:Vec<_>=entries.iter().rev().filter(|t|t.channel==channel).take(50).collect();let mut totals:Vec<_>=traces.iter().filter_map(|t|t.sent.map(|sent|(sent-t.received).max(0))).collect();totals.sort();let percentile=|p:usize|totals.get((totals.len().saturating_sub(1)*p)/100).copied();json!({"samples":totals.len(),"p50Ms":percentile(50),"p95Ms":percentile(95),"traces":traces.iter().map(|t|{let duration=|end:Option<i64>,start:Option<i64>|end.zip(start).map(|(end,start)|(end-start).max(0));json!({"id":t.id,"received":t.received,"status":t.status,"error":t.error,"attachmentMs":duration(t.queued,Some(t.received)),"queueMs":duration(t.model_started,t.queued),"modelMs":duration(t.model_finished,t.model_started),"deliveryQueueMs":duration(t.send_started,t.model_finished),"sendMs":duration(t.sent,t.send_started),"totalMs":duration(t.sent,Some(t.received))})}).collect::<Vec<_>>()})}
}
#[cfg(test)]mod tests{use super::*;#[test]fn timings_correlate_and_persist(){let home=tempfile::tempdir().unwrap();let d=Diagnostics::open(home.path());let a=d.received("a");let b=d.received("b");for stage in ["queued","model_started","model_finished","progress_generated","progress_sending","progress_sent"]{d.mark(&a,stage,None);}assert_eq!(d.view("a")["samples"],0);for stage in ["generated","sending","sent"]{d.mark(&a,stage,None);}assert_eq!(d.view("a")["samples"],1);d.mark(&a,"progress_sent",None);assert_eq!(d.view("a")["traces"][0]["status"],"sent");assert_eq!(d.view("b")["samples"],0);assert_eq!(Diagnostics::open(home.path()).view("a")["traces"][0]["id"],a);d.mark(&b,"failed",Some("模拟失败".into()));assert_eq!(d.view("b")["traces"][0]["status"],"failed");}
#[test]fn unclosed_traces_are_settled_from_the_delivery_queue(){
    let home=tempfile::tempdir().unwrap();let d=Diagnostics::open(home.path());
    let delivered=d.received("c");let failed=d.received("c");let waiting=d.received("c");let lost=d.received("c");
    for id in [&delivered,&failed,&waiting,&lost]{d.mark(id,"queued",None);d.mark(id,"generated",None);}
    let received=now_ms()-60_000;
    d.entries.lock().unwrap().iter_mut().for_each(|t|t.received=if t.id==lost{received-20*60_000}else{received});
    d.reconcile(&[(delivered.clone(),"delivered".into(),received+900),(failed.clone(),"uncertain".into(),received+5),(waiting.clone(),"queued".into(),received+1)]);
    let status=|id:&str|d.view("c")["traces"].as_array().unwrap().iter().find(|t|t["id"]==id).unwrap().clone();
    assert_eq!(status(&delivered)["status"],"sent");assert_eq!(status(&delivered)["totalMs"],900);assert_eq!(d.view("c")["samples"],1);
    assert_eq!(status(&failed)["status"],"failed");assert!(status(&failed)["error"].is_string());
    assert_eq!(status(&waiting)["status"],"generated","a delivery still queued stays open");
    // One trace with a delivered part and a queued part stays open until the last part goes out.
    let split=d.received("c");d.mark(&split,"queued",None);d.mark(&split,"generated",None);
    d.entries.lock().unwrap().iter_mut().filter(|t|t.id==split).for_each(|t|t.received=received);
    d.reconcile(&[(split.clone(),"delivered".into(),received+10),(split.clone(),"queued".into(),received+20)]);
    assert_eq!(status(&split)["status"],"generated");
    d.reconcile(&[(split.clone(),"delivered".into(),received+10),(split.clone(),"delivered".into(),received+40)]);
    assert_eq!(status(&split)["totalMs"],40);
    assert_eq!(status(&lost)["status"],"incomplete","no delivery and a long time: said so");
    d.reconcile(&[]);assert_eq!(Diagnostics::open(home.path()).view("c")["samples"],2,"settled traces persist");
}}
