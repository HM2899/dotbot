# Rust 原生智能体引擎（pi-agent）

dotbot 不再依赖 Claude Code。每个 Bot 的对话、后台任务（记忆整理、自动审批、回复前规划、验收）都由内置的 Rust 引擎直接调用模型服务商：

- `crates/pi-agent-core`：参考 [pi-agent-core](https://github.com/badlogic/pi-mono/tree/main/packages/agent) 移植的消息 → 工具调用 → 工具结果循环（转向/追加队列、中止、轮次上限、参数校验）。不是 TypeScript 包的绑定，也不启动任何子进程。
- `src/pi_agent.rs`：与本项目耦合的运行时——模型协议、团队工具、MCP、技能、会话持久化。
- `src/agent_mcp.rs`：内置 MCP 客户端（stdio 与 Streamable HTTP）。

无需安装或登录 `claude` CLI，也不读取 `~/.claude` 下的任何配置或凭据。

## 启动

```bash
cargo build --release
./target/release/dotbot serve
```

登录后到 **设置 → 模型与供应商 → 文本**，从预设（Anthropic、DeepSeek、通义千问、Ollama 等）添加供应商，填写 API 密钥和模型，再点「启用」。

## 供应商

| 协议 | 地址示例 | 说明 |
| --- | --- | --- |
| Anthropic Messages | `https://api.anthropic.com`、`https://api.deepseek.com/anthropic`、Kimi、智谱、MiniMax、OpenRouter | 密钥可按 `Authorization: Bearer` 或 `x-api-key` 发送 |
| OpenAI Chat Completions | `https://api.openai.com/v1`、通义千问 `…/compatible-mode/v1`、硅基流动、`http://127.0.0.1:11434/v1`（Ollama） | 地址通常以 `/v1` 结尾；本机 Ollama 等可不填密钥 |

- 每个供应商只有模型列表和默认模型，没有 Claude Code 的 opus/sonnet/haiku 档位。Bot 的模型填当前供应商的模型 ID，留空用默认模型；「后台任务的模型」为自动审批、记忆整理、记忆面板、回复前规划分别指定模型 ID，留空用默认模型。
- 新团队没有内置供应商；未启用文本供应商时，Bot 会提示到本页添加。旧版的「Claude Code」官方卡片在打开团队时自动移除（填过密钥的会保留为普通 Anthropic 供应商并去掉预置的 Claude 模型名），旧的档位设置被清除，Bot 上残留的 opus/sonnet/haiku 等别名按默认模型处理。
- 两种协议都使用 SSE 流式输出；工具调用、图片与 PDF 输入、推理内容（`reasoning_content`，DeepSeek 等需要在工具循环中回传）均已适配。部分网关对 `stream=true` 返回整段 JSON，也能处理。
- 结构化后台请求强制模型调用唯一的 `structured_answer` 工具；个别兼容服务忽略 `tool_choice` 时，会从文本中提取 JSON 对象。
- 每个文本供应商可以填写「上下文窗口」（默认 128000 tokens），引擎据此决定何时整理对话；填写这里所用模型中最小的窗口。
- 可用 `DOTBOT_MAX_TOKENS` 调整单次回复上限（默认 16384，且不超过上下文窗口的四分之一）。
- 切换供应商后，每个对话下一轮会在新供应商上重新开始，记忆保留。

## 工具、MCP 与技能

- **团队工具**（`mcp__teambot__*`）经 `/internal/tool` 在服务端执行，鉴权、只读/审批策略、桌面与浏览器接管、本机终端占用都在服务端检查。
- **本机终端协同**：开启后 `local_run`、`local_status`、`local_release`、`local_send_file`、`local_receive_file` 进入工具列表，行为与权限见[本机终端](local-terminal.zh-CN.md)。关闭时这些工具不会出现在模型的工具列表中。
- **独立浏览器**（`--no-desktop` 模式）和 **设置 → 连接** 里的 MCP 服务由引擎自己作为 MCP 客户端连接，工具名为 `mcp__<服务名>__<工具名>`。某个服务启动失败时，本轮只缺少它的工具，并在系统提示里告诉 Bot 哪个服务不可用；其他服务照常工作。
- **连接工具需要审批**：调用前走与团队工具相同的权限策略（只读拒绝；`auto` 先自动审查；`ask`/`edits` 由用户在审批卡片中决定）。浏览器的策略在它自己的入口执行。
- **联网检索**：引擎自带 `web_search`（Bing，失败时换 cn.bing.com、DuckDuckGo；设置 `DOTBOT_SEARXNG_URL` 时优先用自建 SearXNG）和 `web_fetch`（把公开网页转为正文、标题和主要链接，长页面用 `start` 分段读取）。两者只读、不需要审批；解析到本机、局域网、链路本地等内部地址的请求一律拒绝，并把连接固定在检查过的地址上，逐跳检查重定向。
- **技能**：启用的技能（`<home>/plugin/skills/<name>/SKILL.md`）以名称和说明列在系统提示里；模型用 `Skill` 工具按名称加载完整流程，返回内容包含技能目录，资源用 `read_skill_resource` 或已允许的终端工具读取。候选技能评测时，列出的是候选版本自己的技能目录。回复前规划预备的技能仍直接放进系统提示。

## 会话与上下文

- 会话保存为团队目录下的 `pi-sessions/pi-<UUID>.json`（原子写入、权限 0600），只在一批完整的工具调用与结果之后写入，因此恢复时不会出现没有结果的工具调用。
- 旧 Claude Code 会话 ID 或丢失的会话文件会自动按「会话不存在」处理：对话在新会话上重新开始（群聊会重建有界的群聊记录和附件），不会把旧 ID 发给模型服务。
- **系统提示词分两段**：稳定段（身份、规则、冻结记忆、环境、技能目录、中文要求）在同一会话内逐字节不变；本轮段（相关经验、预备技能、委派上下文、本轮不可用的 MCP 服务）单独附在其后，不写入会话。会变化的状态（如本机终端被谁占用）不进稳定段，由 `local_status` 查询。
- **提示词缓存**：Anthropic 协议在稳定系统提示词、工具列表和最新一条消息上设置 `cache_control`，每轮都能从缓存读取此前的整段对话。兼容服务若因请求格式（块状 system、`cache_control`、未知字段）拒绝，会自动改用普通格式并记住；其他 400 错误照常报告，不重试。OpenAI 兼容服务按前缀自动缓存，稳定段在前即可命中。
- **上下文压缩**：每次请求前估算上下文（优先采用服务商上一次返回的 usage，其后的消息按字符估算；中文约 1 字 1 token，图片按 1600 token 计，不按 base64 长度计）。超过「窗口 − 预留回复空间」时，保留最近约 1/5 窗口（最多 40000 token）的原文，较早部分由当前模型整理成一份摘要：用户目标与要求、已完成的工作与产物、**已执行的外部动作（不得重复）**、协作与委派状态、关键数据、进行中与下一步。再次压缩时会把上一份摘要合并进来。摘要作为会话第一条消息写入会话文件，之后的请求只发送摘要和保留的原文；保留部分的旧 usage 会清除，避免立即再次触发。切分点只落在用户或助手消息上，工具调用与结果不会被拆开。
- 服务商仍提示超出上下文长度（`prompt is too long`、`context_length_exceeded` 等）时，强制压缩一次后重试；摘要生成失败时仍移除较早部分并在摘要位置注明。压缩期间对话状态显示「正在整理较早的对话…」。
- 单次请求另外做轻量处理（不改会话文件）：只重发最近 4 张截图，最近 8 次以外的工具输出截短到约 4000 字；外部 MCP 工具的单次输出超过 10 万字时在写入会话前截断。
- 停止会取消模型请求、MCP 启动和等待中的工具；工具执行结果不确定时直接报错，不自动重试。只在调用工具之前的网络中断（5xx、`overloaded_error`、流中断）才按原有机制重试，最多两次。

## 测试

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo build && node dev/e2e.mjs
node dev/local.e2e.mjs
```

e2e 使用 `dev/fake-model.mjs` 作为本地 Anthropic Messages 服务：提示中的 `TOOL <name> <json>` 让它发起对应的工具调用，由真实引擎执行。测试框架设置 `DOTBOT_TEST_MODEL_HEADERS=1`，让请求带上仅供测试的头（会话、工作目录、MCP 配置），正常运行时不会发送。

## 尚未移植

- OAuth 订阅登录（Claude Pro/Max 等）：只支持 API 密钥。
- 会话分支（从历史某一点另开分支）。
