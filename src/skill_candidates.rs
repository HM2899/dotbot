//! Staged automatic refinements never replace the active skill before a paired
//! regression. Both phases load frozen bundles through separate plugin folders.
use super::*;
use crate::{tasks::Task, team::Team};
use std::sync::Arc;

pub fn is_regression_request(request: &str) -> bool {
    request.starts_with("regbase|") || request.starts_with("regcandidate|")
}
fn regression(request: &str) -> Option<(&str, &str, &str, &str)> {
    let p: Vec<_> = request.split('|').collect();
    (p.len() == 5 && is_regression_request(request)).then(|| (p[0], p[1], p[2], p[3]))
}
impl Skills {
    fn candidate_root(&self, id: &str) -> Result<PathBuf> {
        if id.len() != 29
            || !id.starts_with("cand-")
            || !id[5..].bytes().all(|c| c.is_ascii_hexdigit())
        {
            bail!("候选版本编号无效。");
        }
        Ok(self.dir.parent().unwrap().join("skill-candidates").join(id))
    }
    pub fn candidate(&self, id: &str) -> Result<Value> {
        let mut value: Value = serde_json::from_slice(&crate::sqlite_storage::read(self.candidate_root(id)?.join("candidate.json"))?)?;
        if let Some(object)=value.as_object_mut(){object.remove("budgetUsd");object.remove("perPhaseBudgetUsd");}
        Ok(value)
    }
    pub fn candidates(&self, name: &str) -> Vec<Value> {
        let _guard = self.writing.lock().unwrap();
        let mut entries: Vec<Value> =
            std::fs::read_dir(self.dir.parent().unwrap().join("skill-candidates"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|id| self.candidate(id).ok())
                })
                .filter(|v| name.is_empty() || v["name"] == name)
                .collect();
        for entry in &mut entries {
            entry.as_object_mut().unwrap().remove("draft");
        }
        entries.sort_by_key(|v| std::cmp::Reverse(v["created"].as_i64().unwrap_or(0)));
        entries.truncate(100);
        entries
    }
    pub fn stage_learning(
        &self,
        name: &str,
        expected: &str,
        receipt: &str,
        note: &str,
        bot: &str,
    ) -> Result<String> {
        self.stage_candidate(name, expected, receipt, note, bot, None)
    }
    pub fn stage_update(
        &self,
        name: &str,
        expected: &str,
        receipt: &str,
        bot: &str,
        description: &str,
        body: &str,
    ) -> Result<String> {
        let summary = format!(
            "Bot 提交已有技能的新流程草稿。正式版本在对照评测通过前保留。\n{}",
            crate::team::clip(body, 1800)
        );
        self.stage_candidate(
            name,
            expected,
            receipt,
            &summary,
            bot,
            Some((description, body)),
        )
    }
    fn stage_candidate(
        &self,
        name: &str,
        expected: &str,
        receipt: &str,
        note: &str,
        bot: &str,
        proposed: Option<(&str, &str)>,
    ) -> Result<String> {
        let id = format!("cand-{}", &revision(receipt)[..24]);
        let root = self.candidate_root(&id)?;
        let _guard = self.writing.lock().unwrap();
        if root.join("candidate.json").exists() {
            return Ok(id);
        }
        let name = clean_name(name)?;
        let mut existing: Vec<_> =
            std::fs::read_dir(self.dir.parent().unwrap().join("skill-candidates"))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|id| self.candidate(id).ok())
                })
                .collect();
        if existing
            .iter()
            .filter(|v| {
                v["name"] == name
                    && matches!(
                        v["status"].as_str(),
                        Some("awaiting-tests" | "testing" | "needs-cases")
                    )
            })
            .count()
            >= 3
        {
            bail!("此技能已有 3 个待处理候选，请先评测或弃用；正式技能保留。");
        }
        existing.sort_by_key(|v| v["created"].as_i64().unwrap_or(0));
        while existing.len() >= 100 {
            let Some(index) = existing.iter().position(|v| {
                matches!(
                    v["status"].as_str(),
                    Some("promoted" | "failed" | "conflict" | "rejected")
                )
            }) else {
                bail!("待处理候选达到上限，请先处理现有候选。");
            };
            let old = existing.remove(index);
            std::fs::remove_dir_all(self.candidate_root(old["id"].as_str().unwrap())?)?;
        }
        let skill = self
            .list_unlocked()
            .into_iter()
            .find(|s| s["name"] == name)
            .ok_or_else(|| anyhow!("找不到技能。"))?;
        if skill["enabled"] != true || skill["bundleValid"] != true || skill["revision"] != expected
        {
            bail!("正式技能已修改或停用，请基于当前版本重新整理。");
        }
        if note.contains("teambot-learning:") || note.contains("dotbot-learning:") || note.trim().len() < 20 || note.len() > 3000 {
            bail!("候选改进内容无效。");
        }
        let original = skill["text"].as_str().unwrap_or("");
        let improved = if let Some((description, body)) = proposed {
            let description = description.replace(['\n', '\r'], " ").trim().to_string();
            let normalized = body.replace("\r\n", "\n");
            let body = body_of(&normalized);
            if description.is_empty()
                || description.len() > 4096
                || body.is_empty()
                || body.len() > 128_000
            {
                bail!("技能用途或正文无效或超出限额。");
            }
            let (head, _) = original
                .split_once("\n---\n")
                .ok_or_else(|| anyhow!("原技能元数据不完整，请先修复。"))?;
            let mut lines = vec![];
            let mut skipping = false;
            for line in head.lines() {
                if line.starts_with("description:") {
                    lines.push(format!(
                        "description: {}",
                        serde_json::to_string(&description)?
                    ));
                    skipping = true;
                } else if !(skipping && line.starts_with([' ', '\t'])) {
                    skipping = false;
                    lines.push(line.into());
                }
            }
            format!("{}\n---\n\n{}\n", lines.join("\n"), body)
        } else {
            format!(
                "{}\n\n<!-- teambot-learning:{receipt} -->\n### 整理改进\n{}\n",
                original.trim_end(),
                note.trim()
            )
        };
        if improved.len() > 128_000 {
            bail!("改进后的技能过大。");
        }
        self.archive_unlocked(&skill)?;
        let files = bundle_files(&self.root(true).join(&name))?;
        for phase in ["baseline", "candidate"] {
            let plugin = root.join(phase);
            let folder = plugin.join("skills").join(&name);
            for (path, bytes) in &files {
                let file = folder.join(path);
                std::fs::create_dir_all(file.parent().unwrap())?;
                crate::durable::save_bytes(&file, bytes)?;
                std::fs::set_permissions(
                    &file,
                    std::fs::metadata(self.root(true).join(&name).join(path))?.permissions(),
                )?;
            }
            crate::durable::save(
                &plugin.join(".claude-plugin/plugin.json"),
                &json!({"name":"dotbot-regression","version":"1.0.0"}),
            )?;
        }
        crate::durable::save_bytes(
            &root.join("candidate/skills").join(&name).join("SKILL.md"),
            improved.as_bytes(),
        )?;
        let candidate_revision = bundle_revision(&root.join("candidate/skills").join(&name))?;
        crate::durable::save(
            &root.join("candidate.json"),
            &json!({"id":id,"name":name,"bot":bot,"review":receipt,"kind":if proposed.is_some(){"bot-update"}else{"retrospective"},"draft":if proposed.is_some(){Some(body_of(&improved))}else{None},"baseRevision":expected,"revision":candidate_revision,"created":crate::store::now_ms(),"updated":crate::store::now_ms(),"status":"awaiting-tests","improvement":note,"cases":[],"runs":[],"reason":"候选已保存，正式技能未改动；需运行基线和候选的全部保存用例。"}),
        )?;
        Ok(id)
    }
    pub fn candidate_begin(&self, id: &str, context: Value) -> Result<Value> {
        let _guard = self.writing.lock().unwrap();
        let mut value = self.candidate(id)?;
        if !matches!(
            value["status"].as_str(),
            Some("awaiting-tests" | "needs-cases")
        ) {
            bail!("此候选已开始评测或已结束，请查看现有结果。");
        }
        let name = value["name"].as_str().unwrap();
        let skill = self
            .list_unlocked()
            .into_iter()
            .find(|s| s["name"] == name)
            .ok_or_else(|| anyhow!("找不到技能。"))?;
        if skill["enabled"] != true || skill["revision"] != value["baseRevision"] {
            bail!("正式技能已变更或停用，不能继续评测旧候选。");
        }
        let suite = self.benchmark_data(name);
        let cases = suite["cases"]
            .as_array()
            .filter(|c| !c.is_empty())
            .ok_or_else(|| anyhow!("尚无评测用例，请先在技能试运行中保存任务与验收条件。"))?;
        value["evaluationId"] = json!(uuid::Uuid::new_v4().simple().to_string());
        value["cases"] = json!(cases);
        value["context"] = context;
        value["status"] = json!("testing");
        value["reason"] =
            json!("正在比较相同用例的基线和候选；全部通过且有实检或执行证据后才启用。");
        self.save_candidate(&mut value)?;
        Ok(value)
    }
    fn save_candidate(&self, value: &mut Value) -> Result<()> {
        value["updated"] = json!(crate::store::now_ms());
        crate::durable::save(
            &self
                .candidate_root(value["id"].as_str().unwrap())?
                .join("candidate.json"),
            value,
        )
    }
    pub fn candidate_error(&self, id: &str, reason: &str) -> Result<()> {
        let _guard = self.writing.lock().unwrap();
        let mut v = self.candidate(id)?;
        if v["status"] != "promoted" && v["status"] != "rejected" {
            v["status"] = json!("failed");
            v["reason"] = json!(reason);
            self.save_candidate(&mut v)?;
        }
        Ok(())
    }
    pub fn candidate_reject(&self, id: &str) -> Result<()> {
        let _guard = self.writing.lock().unwrap();
        let mut v = self.candidate(id)?;
        if v["status"] == "promoted" {
            bail!("此候选已启用，请使用技能版本回滚。");
        }
        v["status"] = json!("rejected");
        v["reason"] = json!("候选已弃用，正式技能保留。");
        self.save_candidate(&mut v)
    }
    pub fn regression_skill(&self, request: &str) -> Result<Option<(Value, PathBuf)>> {
        let Some((phase, id, _, expected)) = regression(request) else {
            return Ok(None);
        };
        let candidate = self.candidate(id)?;
        if candidate["status"] != "testing" {
            bail!("候选评测已结束或取消。");
        }
        let phase = if phase == "regbase" {
            "baseline"
        } else {
            "candidate"
        };
        let plugin = self.candidate_root(id)?.join(phase);
        let name = candidate["name"].as_str().unwrap();
        let folder = plugin.join("skills").join(name);
        let actual = bundle_revision(&folder)?;
        if actual != expected {
            bail!("评测技能包发生变化，请重新创建候选。");
        }
        let text = crate::sqlite_storage::read_to_string(folder.join("SKILL.md"))?;
        let mut skill = json!({"name":name,"description":front_matter(&text).1,"text":text,"body":body_of(&text),"enabled":true,"bundleValid":true,"revision":actual,"candidate":id,"phase":phase});
        skill["resources"] = json!(
            bundle_files(&folder)?
                .keys()
                .filter(|p| p.as_str() != "SKILL.md")
                .map(|p| json!({"path":p}))
                .collect::<Vec<_>>()
        );
        Ok(Some((skill, plugin)))
    }
    pub fn regression_resource(&self, request: &str, path: &str) -> Result<Vec<u8>> {
        if !super::safe_relative(path) {
            bail!("资源路径无效。");
        }
        let (skill, plugin) = self
            .regression_skill(request)?
            .ok_or_else(|| anyhow!("不是技能评测任务。"))?;
        let root = plugin.join("skills").join(skill["name"].as_str().unwrap());
        bundle_files(&root)?
            .remove(path)
            .ok_or_else(|| anyhow!("候选中没有此资源。"))
    }
    pub fn record_regression(
        &self,
        request: &str,
        flow: &str,
        passed: bool,
        evidence: bool,
        context: Value,
        input_hash: Value,
    ) -> Result<()> {
        let Some((phase, id, case, rev)) = regression(request) else {
            return Ok(());
        };
        let _guard = self.writing.lock().unwrap();
        let mut v = self.candidate(id)?;
        if v["status"] != "testing" {
            return Ok(());
        }
        if !v["cases"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["id"] == case))
        {
            bail!("回归用例不属于此候选。");
        }
        if request.split('|').nth(4) != v["evaluationId"].as_str() {
            bail!("评测批次已变化，旧结果不会覆盖新评测。");
        }
        let runs = v["runs"].as_array_mut().unwrap();
        if !runs.iter().any(|r| r["flow"] == flow) {
            runs.push(json!({"phase":phase,"case":case,"revision":rev,"flow":flow,"passed":passed,"evidence":evidence,"context":context,"inputHash":input_hash}));
        }
        self.save_candidate(&mut v)
    }
    pub fn promote_candidate(&self, id: &str) -> Result<bool> {
        let _guard = self.writing.lock().unwrap();
        let mut v = self.candidate(id)?;
        if v["status"] != "testing" {
            return Ok(false);
        }
        let cases = v["cases"].as_array().unwrap();
        let runs = v["runs"].as_array().unwrap();
        let complete = cases.iter().all(|c| {
            ["regbase", "regcandidate"].iter().all(|phase| {
                runs.iter()
                    .any(|r| r["case"] == c["id"] && r["phase"] == *phase)
            })
        });
        if !complete {
            return Ok(false);
        }
        let matches = cases.iter().all(|c| {
            let a = runs
                .iter()
                .find(|r| r["case"] == c["id"] && r["phase"] == "regbase")
                .unwrap();
            let b = runs
                .iter()
                .find(|r| r["case"] == c["id"] && r["phase"] == "regcandidate")
                .unwrap();
            a["passed"] == true
                && b["passed"] == true
                && a["evidence"] == true
                && b["evidence"] == true
                && a["context"] == v["context"]
                && b["context"] == v["context"]
                && a["inputHash"] == b["inputHash"]
                && a["revision"] == v["baseRevision"]
                && b["revision"] == v["revision"]
        });
        if !matches {
            v["status"] = json!("failed");
            v["reason"] = json!(
                "基线或候选未通过、缺少实际证据、模型权限环境或项目输入不一致；正式技能保留。"
            );
            self.save_candidate(&mut v)?;
            return Ok(false);
        }
        let name = v["name"].as_str().unwrap().to_string();
        let current = self.list_unlocked().into_iter().find(|s| s["name"] == name);
        let result = (|| -> Result<()> {
            let skill = current.ok_or_else(|| anyhow!("正式技能已删除。"))?;
            if skill["enabled"] != true {
                bail!("正式技能已停用。");
            }
            if skill["revision"] == v["revision"] {
                return Ok(());
            }
            if skill["revision"] != v["baseRevision"] {
                bail!("正式技能已被修改，请重新整理；用户编辑优先。");
            }
            let root = self.candidate_root(id)?;
            if bundle_revision(&root.join("candidate/skills").join(&name))?
                != v["revision"].as_str().unwrap()
                || bundle_revision(&root.join("baseline/skills").join(&name))?
                    != v["baseRevision"].as_str().unwrap()
            {
                bail!("冻结的技能包已改变。");
            }
            self.archive_unlocked(&skill)?;
            crate::durable::save_bytes(
                &self.root(true).join(&name).join("SKILL.md"),
                &crate::sqlite_storage::read(root.join("candidate/skills").join(&name).join("SKILL.md"))?,
            )?;
            Ok(())
        })();
        let promoted = result.is_ok();
        v["status"] = json!(if promoted { "promoted" } else { "conflict" });
        v["reason"] = json!(result.err().map(|e| e.to_string()).unwrap_or_else(|| {
            "全部保存用例的基线和候选均通过，已启用候选版本，旧版可回滚。".into()
        }));
        self.save_candidate(&mut v)?;
        Ok(promoted)
    }
}
impl Team {
    pub(crate) fn candidate_can_auto_test(&self, candidate: &Value) -> bool {
        let Ok(suite) = self
            .skills
            .benchmarks(candidate["name"].as_str().unwrap_or(""))
        else {
            return false;
        };
        let Some(cases) = suite["cases"].as_array().filter(|c| !c.is_empty()) else {
            return false;
        };
        cases.iter().all(|case| {
            self.tasks.list().iter().any(|t| {
                let matches = t.request.as_deref().is_some_and(|r| {
                    let p: Vec<_> = r.split('|').collect();
                    p.len() == 5
                        && p[0] == "trial"
                        && p[1] == candidate["name"].as_str().unwrap_or("")
                        && p[2] == case["id"].as_str().unwrap_or("")
                        && p[3] == candidate["baseRevision"].as_str().unwrap_or("")
                });
                matches
                    && t.target == candidate["bot"]
                    && t.status == crate::tasks::Status::Completed
                    && t.verification.as_ref().is_some_and(|v| v["passed"] == true)
                    && self
                        .recovery
                        .latest_matching(|w| w.task.as_deref() == Some(&t.id))
                        .is_some_and(|w| {
                            !w.actions.is_empty()
                                && w.actions.iter().all(|a| {
                                    a["state"] == "confirmed"
                                        && matches!(
                                            a["tool"].as_str(),
                                            Some(
                                                "read_skill"
                                                    | "read_skill_resource"
                                                    | "task_checkpoint"
                                                    | "local_status"
                                                    | "local_send_file"
                                                    | "desktop_screenshot"
                                            )
                                        )
                                })
                        })
            })
        })
    }
    pub(crate) async fn regression_request(&self, key: &str) -> Option<String> {
        self.runs
            .lock()
            .await
            .get(key)
            .and_then(|r| r.task.as_deref())
            .and_then(|id| self.tasks.get(id))
            .and_then(|t| t.request)
            .filter(|s| is_regression_request(s))
    }
    pub(crate) async fn start_candidate_tests(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<Value> {
        let candidate = self.skills.candidate(id)?;
        let state = self.state().await;
        let bot = state
            .bots
            .get(candidate["bot"].as_str().unwrap_or(""))
            .ok_or_else(|| anyhow!("原 Bot 已不存在。"))?;
        if bot.permission.as_deref() == Some("readonly") {
            bail!("只读 Bot 不自动发布技能改进。");
        }
        let provider = self.providers.active();
        let mut context =
            crate::routing::profile(bot, &provider.id, provider.chat_model(bot.model.as_deref()));
        context["localTerminal"] = json!(self.local_computer.enabled());
        context["desktop"] = json!(self.desktops.is_some());
        for phase in ["baseline", "candidate"] {
            let root = self
                .skills
                .candidate_root(id)?
                .join(phase)
                .join("skills")
                .join(candidate["name"].as_str().unwrap());
            let expected = if phase == "baseline" {
                "baseRevision"
            } else {
                "revision"
            };
            let check = tokio::time::timeout(
                std::time::Duration::from_secs(45),
                Skills::validate_bundle_at(
                    &root,
                    candidate["name"].as_str().unwrap(),
                    candidate[expected].as_str().unwrap(),
                ),
            )
            .await??;
            if check["passed"] != true {
                bail!("候选或基线技能包校验失败：{}", check);
            }
        }
        let candidate = self.skills.candidate_begin(id, context)?;
        // Each case uses isolated sessions and project copies. Commands still use
        // the Bot's approval policy. No writes are merged to the original project.
        let scheduled:Result<()>=async {
        for case in candidate["cases"].as_array().unwrap(){for(phase,rev)in[("regbase","baseRevision"),("regcandidate","revision")]{
            let goal=case["goal"].as_str().unwrap();let acceptance=case["acceptance"].as_str().unwrap();
            let details=crate::tasks::AssignmentDetails{objective:goal.into(),expected_output:"返回实际产物和验证依据；保留所有失败。".into(),acceptance:acceptance.into(),explicit:false};
            self.create_workflow(&format!("bot:{}",bot.id),Some(format!("{phase}|{id}|{}|{}|{}",case["id"].as_str().unwrap(),candidate[rev].as_str().unwrap(),candidate["evaluationId"].as_str().unwrap())),goal,vec![crate::workflows::Stage{id:"regression".into(),bot:bot.id.clone(),text:format!("[技能候选对照评测]\n请先 read_skill 读取 {}，只使用本次冻结的技能版本。\n任务：{}\n验收：{}\n在独立任务目录完成，不写回原项目。不得发送、发布、删除外部内容或修改正式技能；执行命令仍遵守原权限审批。必须交付实际检查证据，不把模型退出当作通过。",candidate["name"].as_str().unwrap(),goal,acceptance),files:vec![],details,depends_on:vec![],priority:-2}]).await?;
        }}
        Ok(())}.await;
        if let Err(e) = scheduled {
            let _ = self
                .skills
                .candidate_error(id, &self.secrets.redact(&e.to_string()));
            return Err(e);
        }
        self.team_changed().await;
        self.skills.candidate(id)
    }
    pub(crate) fn regression_finished(
        &self,
        request: &str,
        flow: &str,
        tasks: &[Task],
        verification: &Value,
        live_context: Value,
    ) -> Result<()> {
        let Some((phase, id, _, expected)) = regression(request) else {
            return Ok(());
        };
        let candidate = self.skills.candidate(id)?;
        let name = candidate["name"].as_str().unwrap();
        let clean_actions = tasks.iter().all(|t| {
            self.recovery
                .latest_matching(|w| w.task.as_deref() == Some(&t.id))
                .is_some_and(|w| w.actions.iter().all(|a| a["state"] == "confirmed"))
        });
        let loaded = tasks.iter().all(|t| {
            self.recovery
                .latest_matching(|w| w.task.as_deref() == Some(&t.id))
                .is_some_and(|w| w.skill_revisions.get(name).map(String::as_str) == Some(expected))
        });
        let evidence = tasks.iter().all(|t| {
            self.tasks
                .get(&t.id)
                .and_then(|t| t.verification)
                .is_some_and(|v| {
                    matches!(
                        v["basis"].as_str(),
                        Some("artifact-validation" | "tool-supported-review")
                    )
                })
        });
        let mut context = tasks
            .first()
            .and_then(|t| self.tasks.get(&t.id))
            .and_then(|t| t.execution_profile)
            .unwrap_or(Value::Null);
        if !context.is_object() {
            context = json!({});
        }
        context["localTerminal"] = json!(self.local_computer.enabled());
        context["desktop"] = json!(self.desktops.is_some());
        let settings_match = live_context == context;
        let input = json!(
            tasks
                .iter()
                .map(|t| self.project_snapshot(&t.id).map(|s| s.hashes))
                .collect::<Vec<_>>()
        );
        let passed = loaded && clean_actions && settings_match && verification["passed"] == true;
        self.skills
            .record_regression(request, flow, passed, evidence, context.clone(), input)?;
        let mut report = verification.clone();
        report["passed"] = json!(passed && evidence);
        report["regression"] = json!(id);
        report["phase"] = json!(phase);
        report["executionProfile"] = context;
        if let Some(case) = request.split('|').nth(2) {
            self.skills
                .record_benchmark(name, case, expected, flow, &report)?;
        }
        let _ = phase;
        self.skills.promote_candidate(id)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, Skills, String, Value) {
        let dir = tempfile::tempdir().unwrap();
        let skills = Skills::open(dir.path()).unwrap();
        skills
            .save(
                "records",
                "检查导出",
                "先核对源数据和导出数量，然后生成产物并检查。",
            )
            .unwrap();
        let skill = skills.get("records").unwrap();
        let id = skills
            .stage_learning(
                "records",
                skill["revision"].as_str().unwrap(),
                "work-1:1",
                "核对导出前后的数量，数量不一致时定位缺失记录并停止交付，保存实际检查结果。",
                "a",
            )
            .unwrap();
        skills
            .save_benchmark("records", "核对导出数量", "数量一致且输出结构有效")
            .unwrap();
        (dir, skills, id, skill)
    }
    fn finish(skills: &Skills, id: &str, passed: bool, evidence: bool, context: Value) {
        let c = skills.candidate(id).unwrap();
        let case = c["cases"][0]["id"].as_str().unwrap();
        for (phase, rev) in [("regbase", "baseRevision"), ("regcandidate", "revision")] {
            skills
                .record_regression(
                    &format!(
                        "{phase}|{id}|{case}|{}|{}",
                        c[rev].as_str().unwrap(),
                        c["evaluationId"].as_str().unwrap()
                    ),
                    phase,
                    passed,
                    evidence,
                    context.clone(),
                    json!([{"input":"same"}]),
                )
                .unwrap();
        }
    }
    #[test]
    fn bot_drafts_preserve_metadata_resources_and_do_not_replace_live_skill() {
        let (dir, skills, _, old) = setup();
        let file = dir.path().join("plugin/skills/records/SKILL.md");
        let text = std::fs::read_to_string(&file)
            .unwrap()
            .replace("---\n\n", "license: MIT\ncompatibility: linux\n---\n\n");
        std::fs::write(&file, text).unwrap();
        std::fs::write(
            dir.path().join("plugin/skills/records/LICENSE"),
            "Original license",
        )
        .unwrap();
        let old = skills.get("records").unwrap_or(old);
        let id = skills
            .stage_update(
                "records",
                old["revision"].as_str().unwrap(),
                "bot-edit",
                "a",
                "新的触发条件",
                "新的步骤需要先核对输入，再检查产物，失败时停止交付。",
            )
            .unwrap();
        let draft = skills.candidate(&id).unwrap();
        assert_eq!(draft["kind"], "bot-update");
        assert_eq!(skills.get("records").unwrap()["revision"], old["revision"]);
        let folder = skills
            .candidate_root(&id)
            .unwrap()
            .join("candidate/skills/records");
        let text = std::fs::read_to_string(folder.join("SKILL.md")).unwrap();
        assert!(text.contains("license: MIT"));
        assert!(text.contains("compatibility: linux"));
        assert_eq!(
            std::fs::read_to_string(folder.join("LICENSE")).unwrap(),
            "Original license"
        );
        assert!(
            !skills.candidates("records")[0]
                .as_object()
                .unwrap()
                .contains_key("draft")
        );
    }
    #[test]
    fn candidate_only_publishes_after_paired_evidence_and_survives_restart() {
        let (dir, skills, id, old) = setup();
        assert_eq!(skills.get("records").unwrap()["revision"], old["revision"]);
        assert!(!skills.promote_candidate(&id).unwrap());
        let ctx = json!({"model":"m"});
        skills.candidate_begin(&id, ctx.clone()).unwrap();
        finish(&skills, &id, true, true, ctx);
        drop(skills);
        let skills = Skills::open(dir.path()).unwrap();
        assert!(skills.promote_candidate(&id).unwrap());
        assert_ne!(skills.get("records").unwrap()["revision"], old["revision"]);
        assert!(!skills.promote_candidate(&id).unwrap());
        let versions = skills.versions("records").unwrap();
        assert!(versions.iter().any(|v| v["revision"] == old["revision"]));
    }
    #[test]
    fn missing_evidence_failures_environment_drift_and_user_changes_keep_active_version() {
        for mode in [
            "no-evidence",
            "failed",
            "context",
            "user-edit",
            "resource-edit",
            "reject",
        ] {
            let (dir, skills, id, old) = setup();
            let ctx = json!({"model":"m"});
            skills.candidate_begin(&id, ctx.clone()).unwrap();
            finish(
                &skills,
                &id,
                mode != "failed",
                mode != "no-evidence",
                if mode == "context" {
                    json!({"model":"different"})
                } else {
                    ctx
                },
            );
            if mode == "user-edit" {
                skills
                    .save(
                        "records",
                        "用户用途",
                        "用户的新方法必须保留，不可被候选覆盖。",
                    )
                    .unwrap();
            }
            if mode == "resource-edit" {
                std::fs::write(
                    dir.path().join("plugin/skills/records/new.txt"),
                    "user resource",
                )
                .unwrap();
            }
            if mode == "reject" {
                skills.candidate_reject(&id).unwrap();
            }
            let before = skills.get("records").unwrap();
            assert!(!skills.promote_candidate(&id).unwrap(), "{mode}");
            assert_eq!(
                skills.get("records").unwrap()["revision"],
                before["revision"],
                "{mode}"
            );
            if !["user-edit", "resource-edit"].contains(&mode) {
                assert_eq!(before["revision"], old["revision"]);
            }
        }
    }
    #[test]
    fn frozen_plugins_differ_and_cannot_escape_or_publish_without_cases() {
        let (_dir, skills, id, old) = setup();
        let ctx = json!({});
        let c = skills.candidate_begin(&id, ctx).unwrap();
        let request =
            |phase: &str, rev: &Value| format!("{phase}|{id}|case|{}|nonce", rev.as_str().unwrap());
        let (base, _) = skills
            .regression_skill(&request("regbase", &c["baseRevision"]))
            .unwrap()
            .unwrap();
        let (candidate, _) = skills
            .regression_skill(&request("regcandidate", &c["revision"]))
            .unwrap()
            .unwrap();
        assert_eq!(base["body"], old["body"]);
        assert_ne!(base["body"], candidate["body"]);
        assert!(
            skills
                .regression_resource(&request("regcandidate", &c["revision"]), "../../outside")
                .is_err()
        );
        assert!(skills.candidate("../outside").is_err());
    }
}
