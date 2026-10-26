use anyhow::{Result, bail};
use futures::future::BoxFuture;
use pi_agent_core::*;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Fake {
    responses: Mutex<VecDeque<Value>>,
    requests: Mutex<Vec<Vec<Value>>>,
    calls: Mutex<Vec<String>>,
    block: bool,
    unknown_outcome: bool,
}
impl Runtime for Fake {
    fn stream<'a>(
        &'a self,
        context: &'a AgentContext,
        emit: EventSink,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            self.requests.lock().unwrap().push(context.messages.clone());
            if self.block {
                std::future::pending::<()>().await;
            }
            let message = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| answer("done"));
            emit(AgentEvent::MessageStart {
                message: json!({"role":"assistant","content":[]}),
            });
            Ok(message)
        })
    }
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        _emit: EventSink,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(call.id.clone());
            if self.unknown_outcome {
                bail!("unknown external effect");
            }
            if call.id == "slow" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(ToolResult {
                content: vec![json!({"type":"text","text":call.id})],
                is_error: false,
                details: Value::Null,
                terminate: false,
            })
        })
    }
    fn before_tool_call<'a>(
        &'a self,
        call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<Option<ToolResult>>> {
        Box::pin(async move {
            Ok((call.id == "blocked").then(|| ToolResult::error("blocked by policy")))
        })
    }
}
fn answer(text: &str) -> Value {
    json!({"role":"assistant","content":[{"type":"text","text":text}],"stopReason":"stop"})
}
fn tool_call(id: &str) -> Value {
    json!({"type":"toolCall","id":id,"name":"test","arguments":{"value":"ok"}})
}
fn tools_message(calls: Vec<Value>, reason: &str) -> Value {
    json!({"role":"assistant","content":calls,"stopReason":reason})
}
fn context() -> AgentContext {
    AgentContext {
        system_prompt: "sys".into(),
        messages: vec![],
        tools: vec![Tool {
            name: "test".into(),
            description: "test".into(),
            parameters: json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}),
            sequential: false,
        }],
    }
}
fn events() -> (EventSink, Arc<Mutex<Vec<AgentEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    (Arc::new(move |e| captured.lock().unwrap().push(e)), events)
}
fn prompt() -> Vec<Value> {
    vec![json!({"role":"user","content":[{"type":"text","text":"start"}]})]
}

#[tokio::test]
async fn lifecycle_tools_and_parallel_results_keep_source_order() {
    let runtime = Fake {
        responses: Mutex::new(
            vec![
                tools_message(vec![tool_call("slow"), tool_call("fast")], "toolUse"),
                answer("done"),
            ]
            .into(),
        ),
        ..Fake::default()
    };
    let (emit, events) = events();
    let mut context = context();
    let config = Config {
        tool_execution: ToolExecution::Parallel,
        ..Config::default()
    };
    let new = agent_loop(
        &mut context,
        prompt(),
        &config,
        &runtime,
        &AgentControl::default(),
        emit,
    )
    .await
    .unwrap();
    assert_eq!(new[2]["toolCallId"], "slow");
    assert_eq!(new[3]["toolCallId"], "fast");
    assert_eq!(runtime.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert!(matches!(events.first(), Some(AgentEvent::AgentStart)));
    assert!(matches!(events.last(), Some(AgentEvent::AgentEnd { .. })));
    let completed: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let AgentEvent::ToolExecutionEnd { tool_call_id, .. } = e {
                Some(tool_call_id.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(completed, vec!["fast", "slow"]);
}

#[tokio::test]
async fn truncated_and_invalid_calls_never_execute() {
    let mut invalid = tool_call("invalid");
    invalid["arguments"] = json!({"value":8});
    let runtime = Fake {
        responses: Mutex::new(
            vec![
                tools_message(vec![tool_call("truncated")], "length"),
                tools_message(vec![invalid, tool_call("blocked")], "toolUse"),
                answer("done"),
            ]
            .into(),
        ),
        ..Fake::default()
    };
    let (emit, _) = events();
    let mut context = context();
    agent_loop(
        &mut context,
        prompt(),
        &Config::default(),
        &runtime,
        &AgentControl::default(),
        emit,
    )
    .await
    .unwrap();
    assert!(runtime.calls.lock().unwrap().is_empty());
    let results: Vec<_> = context
        .messages
        .iter()
        .filter(|m| m["role"] == "toolResult")
        .collect();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|m| m["isError"] == true));
}

#[tokio::test]
async fn steering_precedes_model_and_followup_waits_for_quiescence() {
    let runtime = Fake {
        responses: Mutex::new(
            vec![
                tools_message(vec![tool_call("one")], "toolUse"),
                answer("first"),
                answer("second"),
            ]
            .into(),
        ),
        ..Fake::default()
    };
    let control = AgentControl::default();
    control.steer(json!({"role":"user","content":[{"type":"text","text":"steer"}]}));
    control.follow_up(json!({"role":"user","content":[{"type":"text","text":"follow"}]}));
    let (emit, _) = events();
    agent_loop(
        &mut context(),
        prompt(),
        &Config::default(),
        &runtime,
        &control,
        emit,
    )
    .await
    .unwrap();
    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0][1]["content"][0]["text"], "steer");
    assert!(
        !requests[1]
            .iter()
            .any(|m| m["content"][0]["text"] == "follow")
    );
    assert_eq!(requests[2].last().unwrap()["content"][0]["text"], "follow");
    assert!(!control.has_queued_messages());
}

#[tokio::test]
async fn cancellation_always_emits_agent_end_and_drops_request() {
    let runtime = Fake {
        block: true,
        ..Fake::default()
    };
    let control = AgentControl::default();
    let (emit, events) = events();
    let mut context = context();
    let config = Config::default();
    let run = agent_loop(&mut context, prompt(), &config, &runtime, &control, emit);
    let abort = async {
        tokio::task::yield_now().await;
        control.abort();
    };
    let (result, _) = tokio::join!(run, abort);
    assert!(result.is_err());
    assert_eq!(context.messages.last().unwrap()["stopReason"], "aborted");
    assert!(matches!(
        events.lock().unwrap().last(),
        Some(AgentEvent::AgentEnd { .. })
    ));
}

#[tokio::test]
async fn continuation_and_unknown_effects_do_not_replay() {
    let runtime = Fake {
        responses: Mutex::new(vec![tools_message(vec![tool_call("one")], "toolUse")].into()),
        unknown_outcome: true,
        ..Fake::default()
    };
    let (emit, _) = events();
    assert!(
        agent_loop(
            &mut context(),
            vec![],
            &Config::default(),
            &runtime,
            &AgentControl::default(),
            emit.clone()
        )
        .await
        .is_err()
    );
    let mut context = context();
    assert!(
        agent_loop(
            &mut context,
            prompt(),
            &Config::default(),
            &runtime,
            &AgentControl::default(),
            emit.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(runtime.calls.lock().unwrap().len(), 1);
    assert_eq!(runtime.requests.lock().unwrap().len(), 1);
    // A successful assistant tail is not a valid continuation point either.
    context.messages = vec![answer("done")];
    assert!(
        agent_loop(
            &mut context,
            vec![],
            &Config::default(),
            &runtime,
            &AgentControl::default(),
            emit
        )
        .await
        .is_err()
    );
}

/// Refuses the first request as too long; compaction replaces the transcript with a summary.
struct Overflowing {
    attempts: Mutex<usize>,
    compactions: Mutex<Vec<bool>>,
}
impl Runtime for Overflowing {
    fn stream<'a>(&'a self, context: &'a AgentContext, _emit: EventSink) -> BoxFuture<'a, Result<Value>> {
        Box::pin(async move {
            let mut attempts = self.attempts.lock().unwrap();
            *attempts += 1;
            if *attempts == 1 {
                bail!("prompt is too long");
            }
            assert_eq!(context.messages[0]["role"], "compactionSummary");
            Ok(answer("after compaction"))
        })
    }
    fn execute<'a>(&'a self, _call: &'a ToolCall, _emit: EventSink) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async { bail!("no tools") })
    }
    fn compact<'a>(&'a self, context: &'a mut AgentContext, force: bool) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            self.compactions.lock().unwrap().push(force);
            if !force {
                return Ok(false);
            }
            let last = context.messages.pop().unwrap();
            context.messages = vec![json!({"role":"compactionSummary","summary":"earlier"}), last];
            Ok(true)
        })
    }
    fn context_overflow(&self, error: &anyhow::Error) -> bool {
        error.to_string().contains("too long")
    }
}

#[tokio::test]
async fn an_overflowing_request_is_compacted_once_and_retried() {
    let runtime = Overflowing { attempts: Mutex::new(0), compactions: Mutex::new(vec![]) };
    let mut context = AgentContext {
        system_prompt: "s".into(),
        messages: vec![json!({"role":"user","content":[{"type":"text","text":"old"}]}), answer("old answer")],
        tools: vec![],
    };
    let control = AgentControl::default();
    agent_loop(&mut context, vec![json!({"role":"user","content":[{"type":"text","text":"new"}]})], &Config::default(), &runtime, &control, Arc::new(|_| {}))
        .await
        .unwrap();
    assert_eq!(*runtime.attempts.lock().unwrap(), 2);
    assert_eq!(*runtime.compactions.lock().unwrap(), vec![false, true]);
    assert_eq!(context.messages[0]["role"], "compactionSummary");
    assert_eq!(context.messages.last().unwrap()["content"][0]["text"], "after compaction");
}
