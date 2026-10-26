//! Measured routing hints. No private historical task text leaves this module;
//! model/permission changes and interrupted/cancelled tasks do not earn expertise.
use crate::{
    store::Bot,
    tasks::{Status, Task},
};
use serde_json::{Value, json};
pub fn profile(bot: &Bot, provider: &str, model: Option<String>) -> Value {
    json!({"provider":provider,"model":model,"permission":bot.permission.as_deref().unwrap_or("auto")})
}
pub fn performance(bot: &Bot, tasks: &[Task], query: &str, context: &Value) -> Value {
    let running = tasks
        .iter()
        .filter(|t| t.target == bot.id && t.status == Status::Running)
        .count();
    let queued = tasks
        .iter()
        .filter(|t| t.target == bot.id && matches!(t.status, Status::Queued | Status::Waiting))
        .count();
    let mut samples = 0;
    let mut passed = 0;
    let mut failed = 0;
    let mut reviewed = 0;
    let mut durations = vec![];
    for task in tasks.iter().filter(|t| {
        t.target == bot.id
            && t.execution_profile.as_ref() == Some(context)
            && matches!(t.status, Status::Completed | Status::Failed)
            && t.attempt == 1
            && !t
                .request
                .as_deref()
                .is_some_and(crate::skills::is_regression_request)
    }) {
        let text = task
            .assignment
            .as_ref()
            .map(|a| a.objective.as_str())
            .unwrap_or(&task.text);
        if !query.is_empty() && crate::relevance::similarity(query, text) < 0.18 {
            continue;
        }
        samples += 1;
        let check = task.verification.as_ref();
        if task.status == Status::Failed {
            failed += 1;
        } else if let Some(v) = check {
            if matches!(
                v["basis"].as_str(),
                Some("artifact-validation" | "tool-supported-review")
            ) {
                if v["passed"] == true {
                    passed += 1;
                } else if v["passed"] == false {
                    failed += 1;
                }
            } else {
                reviewed += 1;
            }
        }
        if let (Some(start), Some(end)) = (task.started, task.finished)
            && end >= start
        {
            durations.push((end - start) as f64);
        }
    }
    fn median(values: &mut [f64]) -> Option<f64> {
        if values.is_empty() {
            None
        } else {
            values.sort_by(f64::total_cmp);
            Some((values[(values.len() - 1) / 2] + values[values.len() / 2]) / 2.0)
        }
    }
    let checked = passed + failed;
    let success = if checked > 0 {
        Some((passed as f64 + 1.0) / (checked as f64 + 2.0))
    } else {
        None
    };
    json!({"scope":if query.is_empty(){"current-settings"}else{"similar-tasks-current-settings"},"samples":samples,"verifiedPasses":passed,"verifiedFailures":failed,"modelOnlyReviews":reviewed,"smoothedSuccess":success,"medianDurationMs":median(&mut durations),"running":running,"queued":queued,"confidence":if checked>=10{"moderate"}else{"limited"},"note":"按当前模型与权限统计；取消、中断、重试和技能回归不算成功样本。小样本仅供分工参考。"})
}

impl crate::team::Team {
    pub(crate) fn routing_performance(&self, bot: &Bot, query: &str) -> Value {
        let provider = self.providers.active();
        let mut value = performance(
            bot,
            &self.tasks.list(),
            query,
            &profile(bot, &provider.id, provider.chat_model(bot.model.as_deref())),
        );
        if let Ok(runs) = self.runs.try_lock() {
            let active = runs.iter().filter(|(key, _)| {
                crate::team::Conv::parse(key).is_some_and(|c| c.bot() == bot.id)
            });
            let (running, queued) = active.fold((0usize, 0usize), |(a, b), (_, r)| {
                (a + usize::from(r.running), b + r.queue.len())
            });
            let running = running.max(value["running"].as_u64().unwrap_or(0) as usize);
            let queued = queued.max(value["queued"].as_u64().unwrap_or(0) as usize);
            value["running"] = json!(running);
            value["queued"] = json!(queued);
            value["loadKnown"] = json!(true);
        } else {
            value["loadKnown"] = json!(false);
        }
        value
    }
    pub(crate) fn routing_receipt(&self, bot: &Bot, query: &str) -> Value {
        json!({"bot":bot.id,"at":crate::store::now_ms(),"performance":self.routing_performance(bot,query),"reason":"协调 Bot 按当前任务与职责、技能匹配选择；历史任务记录与耗时作为参考，保留原模型和权限。"})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn task(id: &str, status: &str, basis: &str, pass: bool) -> Task {
        serde_json::from_value(json!({"id":id,"source":"bot:chief","target":"a","text":"核对电子表导出数量","parent":null,"path":["chief","a"],"status":status,"created":0,"started":1000,"finished":5000,"result":"done","verification":{"passed":pass,"basis":basis},"executionProfile":{"provider":"p","model":"m","permission":"auto"}})).unwrap()
    }
    #[test]
    fn routing_counts_checked_similar_tasks_with_current_settings() {
        let bot = Bot {
            id: "a".into(),
            ..Default::default()
        };
        let context = profile(&bot, "p", Some("m".into()));
        let mut tasks = vec![
            task("good", "completed", "artifact-validation", true),
            task("unverified", "completed", "model-review", true),
            task("failure", "failed", "", false),
            task(
                "cancel",
                "cancelled",
                "artifact-validation",
                true,
            ),
        ];
        let mut changed = task("old", "completed", "artifact-validation", true);
        changed.execution_profile = Some(json!({"model":"old"}));
        tasks.push(changed);
        let mut unrelated = task("poem", "completed", "artifact-validation", true);
        unrelated.text = "写一首诗".into();
        tasks.push(unrelated);
        let v = performance(&bot, &tasks, "verify spreadsheet export counts", &context);
        assert_eq!(v["samples"], 3);
        assert_eq!(v["verifiedPasses"], 1);
        assert_eq!(v["verifiedFailures"], 1);
        let empty = performance(&bot, &tasks, "video production", &context);
        assert!(empty["smoothedSuccess"].is_null());
    }
}
