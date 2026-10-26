# Upstream provenance

- Project: https://github.com/earendil-works/pi (formerly badlogic/pi-mono)
- Commit: `d5b3de22d0e24a3e9c38a9410d3329b099717283`
- License: MIT, Copyright (c) 2025 Mario Zechner; retained in LICENSE.
- Port: Rust translation and dotbot customization, not the JavaScript package or a subprocess wrapper.

Source mapping:

| Upstream | Rust |
| --- | --- |
| `packages/agent/src/types.ts`: AgentContext, AgentEvent, AgentToolResult, hooks | `src/lib.rs`: corresponding public Rust structs/enums, Runtime |
| `packages/agent/src/agent.ts`: steering/follow-up queues, queue modes, abort | `AgentControl`, `QueueMode` |
| `packages/agent/src/agent-loop.ts`: runAgentLoop, runAgentLoopContinue, runLoop | `agent_loop`, `run_loop` |
| executeToolCallsSequential / executeToolCallsParallel | configured sequential execution or ordered `join_all` |
| runToolCall / prepareToolCall / finalizeExecutedToolCall | `run_tool_call`, Runtime before/after hooks |
| failToolCallsFromTruncatedMessage | length stop guard before execution |
| `packages/ai`: provider boundary | dotbot `src/pi_agent.rs`: Rust HTTP adapter; not a port of every pi-ai provider |

Customizations: tools default to sequential; a configurable turn limit; transport failures with unknown tool effects terminate instead of being returned to the model as retryable failures. Cancellation drops Rust futures and emits lifecycle events. Authorization stays in dotbot's server.

Not yet ported: dynamic tool declaration deltas, prepareRequest/prepareNextTurn snapshot hooks, automatic context compaction, full JSON Schema/Ajv semantics and coercion, pi-ai provider catalog/OAuth, proxy transport. AgentMessage uses serde_json::Value for pi's extensible union; this is distinct from provider JSON and converted at the HTTP boundary.
