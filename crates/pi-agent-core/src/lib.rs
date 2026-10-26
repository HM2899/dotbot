//! Rust translation/customization of pi's packages/agent/src/{agent-loop,agent,types}.ts.
//! Upstream revision and MIT notice: ../UPSTREAM.md and ../LICENSE.
//! Message values use pi's open AgentMessage representation, not any provider's wire format.
use anyhow::{Result, bail, ensure};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::watch;

pub type EventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default)]
    pub sequential: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<Value>,
    pub is_error: bool,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub terminate: bool,
}
impl ToolResult {
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![json!({"type":"text","text":text.into()})],
            is_error: true,
            details: Value::Null,
            terminate: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<Value>,
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<Value>,
    },
    TurnStart,
    TurnEnd {
        message: Value,
        tool_results: Vec<Value>,
    },
    MessageStart {
        message: Value,
    },
    MessageUpdate {
        message: Value,
        assistant_message_event: Value,
    },
    MessageEnd {
        message: Value,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: Value,
        partial_result: Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: ToolResult,
        is_error: bool,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueueMode {
    All,
    OneAtATime,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolExecution {
    Sequential,
    Parallel,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TurnDecision {
    Auto,
    Continue,
    End,
}

pub struct Config {
    pub max_turns: usize,
    pub tool_execution: ToolExecution,
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
}
impl Default for Config {
    fn default() -> Self {
        // dotbot customization: serialize tools unless the caller opts into parallel execution.
        Self {
            max_turns: 64,
            tool_execution: ToolExecution::Sequential,
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
        }
    }
}

#[derive(Default)]
struct Queues {
    steering: VecDeque<Value>,
    follow_up: VecDeque<Value>,
}
#[derive(Clone)]
pub struct AgentControl {
    queues: Arc<Mutex<Queues>>,
    abort: watch::Sender<bool>,
}
impl Default for AgentControl {
    fn default() -> Self {
        Self {
            queues: Arc::new(Mutex::new(Queues::default())),
            abort: watch::channel(false).0,
        }
    }
}
impl AgentControl {
    pub fn steer(&self, message: Value) {
        self.queues.lock().unwrap().steering.push_back(message);
    }
    pub fn follow_up(&self, message: Value) {
        self.queues.lock().unwrap().follow_up.push_back(message);
    }
    pub fn abort(&self) {
        self.abort.send_replace(true);
    }
    pub fn aborted(&self) -> bool {
        *self.abort.borrow()
    }
    pub fn clear_queues(&self) {
        *self.queues.lock().unwrap() = Queues::default();
    }
    pub fn has_queued_messages(&self) -> bool {
        let q = self.queues.lock().unwrap();
        !q.steering.is_empty() || !q.follow_up.is_empty()
    }
    fn take(&self, steering: bool, mode: QueueMode) -> Vec<Value> {
        let mut q = self.queues.lock().unwrap();
        let queue = if steering {
            &mut q.steering
        } else {
            &mut q.follow_up
        };
        let count = if mode == QueueMode::All {
            queue.len()
        } else {
            queue.len().min(1)
        };
        queue.drain(..count).collect()
    }
}

/// Rust equivalents of StreamFn and the loop's transform/tool/finish hooks.
/// Implementors perform network I/O; the core never launches processes.
pub trait Runtime: Send + Sync {
    fn stream<'a>(
        &'a self,
        context: &'a AgentContext,
        emit: EventSink,
    ) -> BoxFuture<'a, Result<Value>>;
    fn execute<'a>(
        &'a self,
        call: &'a ToolCall,
        emit: EventSink,
    ) -> BoxFuture<'a, Result<ToolResult>>;
    fn transform_context<'a>(
        &'a self,
        context: &'a AgentContext,
    ) -> BoxFuture<'a, Result<AgentContext>> {
        Box::pin(async { Ok(context.clone()) })
    }
    fn before_tool_call<'a>(
        &'a self,
        _call: &'a ToolCall,
    ) -> BoxFuture<'a, Result<Option<ToolResult>>> {
        Box::pin(async { Ok(None) })
    }
    fn after_tool_call<'a>(
        &'a self,
        _call: &'a ToolCall,
        result: ToolResult,
    ) -> BoxFuture<'a, Result<ToolResult>> {
        Box::pin(async { Ok(result) })
    }
    fn finish_turn<'a>(
        &'a self,
        _context: &'a AgentContext,
    ) -> BoxFuture<'a, Result<TurnDecision>> {
        Box::pin(async { Ok(TurnDecision::Auto) })
    }
    /// dotbot customization (pi's coding-agent compaction, moved into the loop): shrink the
    /// transcript in place before a request, e.g. by summarizing older exchanges. `force` is
    /// set after the provider rejected a request as too long. Ok(true) when it changed.
    fn compact<'a>(
        &'a self,
        _context: &'a mut AgentContext,
        _force: bool,
    ) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    /// Whether a failed request was refused for exceeding the model's context window.
    fn context_overflow(&self, _error: &anyhow::Error) -> bool {
        false
    }
}

/// Core of runToolCall: lookup, validation, before hook, execution, after hook.
/// Recoverable tool failures are sent back to the model; unknown execution outcomes
/// must be returned as Err by the runtime and terminate the run without replay.
pub async fn run_tool_call(
    runtime: &dyn Runtime,
    tools: &[Tool],
    call: &ToolCall,
    emit: EventSink,
) -> Result<ToolResult> {
    let Some(tool) = tools.iter().find(|t| t.name == call.name) else {
        return Ok(ToolResult::error(format!("Tool {} not found", call.name)));
    };
    if let Err(error) = validate_arguments(&tool.parameters, &call.arguments) {
        return Ok(ToolResult::error(error.to_string()));
    }
    if let Some(result) = runtime.before_tool_call(call).await? {
        return Ok(result);
    }
    let result = runtime.execute(call, emit).await?;
    runtime.after_tool_call(call, result).await
}

// The supported schema subset is explicit: no argument coercion or best-effort JSON salvage.
// dotbot's server remains the final argument and authorization boundary.
pub fn validate_arguments(schema: &Value, value: &Value) -> Result<()> {
    if let Some(kind) = schema["type"].as_str() {
        let valid = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "null" => value.is_null(),
            _ => false,
        };
        ensure!(valid, "Invalid tool argument type: expected {kind}");
    }
    if let Some(options) = schema["enum"].as_array() {
        ensure!(options.contains(value), "Tool argument is outside enum");
    }
    if let Some(required) = schema["required"].as_array() {
        for name in required.iter().filter_map(Value::as_str) {
            ensure!(
                value.get(name).is_some(),
                "Missing required tool argument: {name}"
            );
        }
    }
    if let (Some(properties), Some(object)) = (schema["properties"].as_object(), value.as_object())
    {
        for (name, item) in object {
            if let Some(child) = properties.get(name) {
                validate_arguments(child, item)?;
            } else {
                ensure!(
                    schema["additionalProperties"] != false,
                    "Unknown tool argument: {name}"
                );
            }
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(schema) = schema.get("items") {
            for item in items {
                validate_arguments(schema, item)?;
            }
        }
    }
    Ok(())
}

fn append(context: &mut AgentContext, new: &mut Vec<Value>, message: Value, emit: &EventSink) {
    emit(AgentEvent::MessageStart {
        message: message.clone(),
    });
    context.messages.push(message.clone());
    new.push(message.clone());
    emit(AgentEvent::MessageEnd { message });
}

/// Translation of runAgentLoop/runAgentLoopContinue and runLoop's inner/outer queues.
/// Empty prompts mean continue; continuation from an assistant message is rejected.
pub async fn agent_loop(
    context: &mut AgentContext,
    prompts: Vec<Value>,
    config: &Config,
    runtime: &dyn Runtime,
    control: &AgentControl,
    emit: EventSink,
) -> Result<Vec<Value>> {
    if prompts.is_empty() {
        ensure!(
            !context.messages.is_empty(),
            "Cannot continue: no messages in context"
        );
        ensure!(
            context.messages.last().unwrap()["role"] != "assistant",
            "Cannot continue from message role: assistant"
        );
    }
    let mut abort = control.abort.subscribe();
    let mut new = Vec::new();
    emit(AgentEvent::AgentStart);
    emit(AgentEvent::TurnStart);
    for message in prompts {
        append(context, &mut new, message, &emit);
    }
    let outcome = if *abort.borrow() {
        Err(anyhow::anyhow!("Operation aborted"))
    } else {
        tokio::select! {
            result = run_loop(context,&mut new,config,runtime,control,&emit) => result,
            _ = abort.changed() => Err(anyhow::anyhow!("Operation aborted")),
        }
    };
    if let Err(error) = &outcome {
        // Match pi's error/aborted assistant lifecycle. No synthetic successful tool results.
        let message = json!({"role":"assistant","content":[],"stopReason":if *abort.borrow() {"aborted"} else {"error"},"errorMessage":error.to_string()});
        append(context, &mut new, message.clone(), &emit);
        emit(AgentEvent::TurnEnd {
            message,
            tool_results: vec![],
        });
    }
    emit(AgentEvent::AgentEnd {
        messages: new.clone(),
    });
    outcome?;
    Ok(new)
}

async fn run_loop(
    context: &mut AgentContext,
    new: &mut Vec<Value>,
    config: &Config,
    runtime: &dyn Runtime,
    control: &AgentControl,
    emit: &EventSink,
) -> Result<()> {
    let mut pending = Vec::new();
    for turn in 0..config.max_turns {
        ensure!(!*control.abort.borrow(), "Operation aborted");
        if turn > 0 {
            emit(AgentEvent::TurnStart);
        }
        if pending.is_empty() {
            pending = control.take(true, config.steering_mode);
        }
        for message in pending.drain(..) {
            append(context, new, message, emit);
        }
        runtime.compact(context, false).await?;
        // One forced compaction and retry when the provider still finds the request too long.
        let mut retried = false;
        let message = loop {
            let request_context = runtime.transform_context(context).await?;
            match runtime.stream(&request_context, emit.clone()).await {
                Err(error) if !retried && runtime.context_overflow(&error) => {
                    retried = true;
                    if !runtime.compact(context, true).await? {
                        return Err(error);
                    }
                }
                result => break result?,
            }
        };
        ensure!(
            message["role"] == "assistant",
            "Model returned a non-assistant message"
        );
        // A provider emits start/update while streaming; the core commits only a complete message.
        context.messages.push(message.clone());
        new.push(message.clone());
        emit(AgentEvent::MessageEnd {
            message: message.clone(),
        });
        if message["stopReason"] == "error" || message["stopReason"] == "aborted" {
            bail!(
                "{}",
                message["errorMessage"]
                    .as_str()
                    .unwrap_or("Model response failed")
            );
        }
        let calls: Vec<ToolCall> = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "toolCall")
            .map(|b| serde_json::from_value(b.clone()))
            .collect::<std::result::Result<_, _>>()?;
        let truncated = message["stopReason"] == "length";
        let execute = |call: ToolCall| {
            let emit = emit.clone();
            let tools = &context.tools;
            async move {
                ensure!(!*control.abort.borrow(), "Operation aborted");
                emit(AgentEvent::ToolExecutionStart {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    args: call.arguments.clone(),
                });
                let start = Instant::now();
                let result = if truncated {
                    ToolResult::error(
                        "Tool call was not executed: output token limit; re-issue complete arguments",
                    )
                } else {
                    run_tool_call(runtime, tools, &call, emit.clone()).await?
                };
                emit(AgentEvent::ToolExecutionEnd {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    is_error: result.is_error,
                    result: result.clone(),
                    duration_ms: start.elapsed().as_millis().min(u64::MAX as u128) as u64,
                });
                Ok::<_, anyhow::Error>((call, result))
            }
        };
        let parallel = config.tool_execution == ToolExecution::Parallel
            && !calls.iter().any(|c| {
                context
                    .tools
                    .iter()
                    .any(|t| t.name == c.name && t.sequential)
            });
        let finalized = if parallel {
            // Preserve input order in the transcript, irrespective of completion order.
            futures::future::join_all(calls.into_iter().map(execute))
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?
        } else {
            let mut results = Vec::new();
            for call in calls {
                results.push(execute(call).await?);
            }
            results
        };
        let has_tools = !finalized.is_empty();
        let terminate = has_tools && finalized.iter().all(|(_, r)| r.terminate);
        let mut results = Vec::new();
        for (call, result) in finalized {
            let result = json!({"role":"toolResult","toolCallId":call.id,"toolName":call.name,"content":result.content,"isError":result.is_error,"details":result.details});
            append(context, new, result.clone(), emit);
            results.push(result);
        }
        let decision = runtime.finish_turn(context).await?;
        emit(AgentEvent::TurnEnd {
            message,
            tool_results: results,
        });
        if decision == TurnDecision::End {
            return Ok(());
        }
        if (has_tools && !terminate) || !control.queues.lock().unwrap().steering.is_empty() {
            continue;
        }
        let follow = control.take(false, config.follow_up_mode);
        if !follow.is_empty() {
            pending = follow;
            continue;
        }
        if decision == TurnDecision::Continue {
            continue;
        }
        return Ok(());
    }
    bail!("Agent exceeded the configured turn limit")
}
