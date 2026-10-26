# 共享账号、事件接入与可靠恢复

对应功能差距审计第 5—7 项。所有配置仍在设置中，侧栏保留 Bot 和群聊。本机协同仍只提供终端和文件能力。

## 共享账号与连接测试（第 5 项）

打开「设置 → 连接与密钥」，选择 Gmail、Google Drive、GitHub 或 Microsoft 365。每个连接可以指定名称、OAuth Client ID、Client Secret、请求权限、是否允许写操作，以及启用／暂停状态。一个连接供整个团队使用。

1. 在平台开发者后台注册自己的 OAuth 应用，把窗口显示的 `/oauth/callback` 完整地址加入允许的回调地址。远程使用先配置 HTTPS 公网域名；Google 需要启用对应 API、配置同意屏幕及测试用户，Microsoft 需允许授权所用的租户／账号类型，组织账号可能要求管理员同意。
2. 填入应用凭据，只勾选任务所需权限。默认读取；开启写操作后才可申请写权限。修改应用凭据或权限范围会清除当前本地授权，需要重新授权。
3. 保存后点击「授权」，打开平台授权页面。确认权限并返回设置，点击「测试连接」：服务会实际调用身份／资源 API，显示测试时间、账号信息或错误。仅保存配置和完成 OAuth 回调不等于 API 已验证可用。
4. 可暂停连接，或撤销／删除。Google 调用 token 撤销接口，GitHub 撤销应用授权；Microsoft 先清除本地凭据，仍须到 Microsoft 的账号／组织应用管理页面移除平台侧同意。平台撤销失败也会清除本地 token，并显示平台处理结果。

授权使用一次性 state 和 PKCE S256；待授权链接 10 分钟后失效，服务重启后需重新发起。服务按过期时间自动刷新支持刷新的授权；平台不返回 refresh token 或拒绝续期时，需要重新授权。请求权限和平台实际授予权限分别保存。

Bot 使用 `list_accounts` 查询连接，再通过 `account_request` 调用该服务的相对 JSON API 路径。例如 GitHub 的 `/user`、Drive 的 `/files?pageSize=10`、Graph 的 `/me/messages?$top=10`。请求限制在已配置 API 的域名与基础路径内；响应最多 1 MiB，请按需选择字段并分页。此接口不是文件流下载器。外部内容仍是不可信任务数据。

写操作同时要求连接允许写入、平台授予相应 scope，以及 Bot 权限允许；每次写请求都经过批准卡片。只读 Bot 不能写入。OAuth token 和 Client Secret 留在服务端，不传入模型、网页状态或自定义 MCP 配置。`accounts.json` 在 Unix 上为 `0600`；这属于文件权限保护，不是磁盘加密，备份也应妥善保护。

自定义 MCP 仍单独配置，可以与共享账号并存。点击「测试连接」会实际执行 MCP `initialize`、`notifications/initialized` 和 `tools/list`，展示工具名或错误；支持 stdio 和 Streamable HTTP 的 JSON／SSE 响应，并关闭测试会话／进程。它不会自动为任意 MCP 服务发现或完成 OAuth，也不代表服务内部每个工具都已验收。

## 事件配置与到达证据（第 6 项）

在「设置 → 例行任务」创建 webhook 或 GitHub／Slack／Teams 事件任务，打开「配置与测试」。可在同一窗口保存签名密钥、配置公网 HTTPS 域名、查看平台步骤及运行测试。公网地址必须为不带路径的 HTTPS origin，如 `https://team.example.com`；填写地址只改变回调 URL，不会部署反向代理或自动开放网络端口。部署方法见 [部署说明](../deploy/README.md)。OAuth 回调也使用这一地址。

GitHub 可选择已授权且具备仓库 webhook 管理权限的 GitHub 账号，填写 `owner/repo` 并注册订阅。服务检查该仓库前 100 个 hook，更新其中同回调地址的 hook，或创建一个新 hook。仓库 hook 数超过该范围时需先在平台核对。支持请求 GitHub 发送 ping、读取订阅状态及删除订阅；删除本地任务或暂停本地任务不等于删除外部 hook，应通过订阅删除入口处理平台配置。

Slack 需要在自己的 Slack App 配置 Signing Secret、Event Subscriptions、目标事件和权限，并安装到工作区。设置显示 URL 验证证据；从目标频道发送实际消息后再查看事件到达记录。Teams 使用 Workflows 的 HTTP 步骤向私有回调 POST JSON；设置提供步骤和可选 HMAC 签名说明，不自动创建 Microsoft Graph 订阅。上述事件入口不等于 Slack／Teams 双向聊天接入。

证据分别展示：

| 状态 | 证明的内容 |
|---|---|
| 本地测试 | 经真实 HTTP handler 检查签名及筛选；不会启动模型，也不算平台到达 |
| 已注册订阅 | 平台 API 已接受订阅配置；不保证公网能到达 |
| 回调已验证 | Slack 已完成 URL challenge；不代表已订阅正确事件 |
| 平台事件到达 | 收到外部请求并通过该平台签名／私有 webhook 检查；还需看事件类型与筛选是否命中 |
| 运行回执 | 命中且排队的任务实际执行状态及结果 |

GitHub ping 可以证明到达，但不会冒充匹配的 push 任务。回调地址或事件配置变化后，旧配置的成功证据不再显示为当前结果。通用 webhook／Teams 的证据依赖私有地址及所配置的签名，并不额外认证发送方的平台身份。不要公开私有回调地址。

## 持久投递与任务恢复（第 7 项）

「设置 → 社交平台」显示持久发送队列：正文片段和文件逐项保存进度、尝试次数和错误。服务重启自动补发尚未开始的排队片段；已经确认的片段不因普通重试而重发。发送前会复查平台启用状态、原目标及发送者授权，并读取当前平台凭据。

| 发送状态 | 处理方式 |
|---|---|
| queued / sending | 等待发送／正在向平台发送 |
| delivered | 所有片段已获发送成功确认 |
| failed | 显示错误，修复后点击「重试剩余」 |
| blocked | 平台暂停、目标改变或授权撤销；恢复原授权和目标后重试 |
| uncertain | 发送中断或网络响应不确定；先核对当前片段是否已到达，再选择重发或「当前片段已收到」跳过 |

平台可能已接收请求却未返回确认，所以不确定状态不会自动盲目补发。文件发送失败仍保留文件和进度，必要时另发网页下载指引；完全断网时，错误通知也可能无法送达。QQ 的被动回复时限等平台限制仍适用。保存的路由与回复摘要用于重复入队去重，微信使用稳定 client ID，QQ 保存消息序号；平台不支持幂等键时不能承诺全局 exactly-once。

已接收执行的输入、会话、附件引用、任务／例行回执、进度摘要和结果保存在 `recovery.json`。普通聊天请求重启后不会自动重放，保留失败回执及原输入，用户可以核对现有状态后重新发送并附上原文件。

Bot 用 `task_checkpoint` 保存已完成、剩余工作及交付文件。重要操作和浏览器变更另记录 started／confirmed／failed 及有界结果；started 没有确认表示结果不确定，应先查看外部状态。当前委派工作流另外保存完整依赖图和任务记录：中断步骤在任务面板核对检查点后继续，成功步骤保留，依赖自动接续，最终统一复核和交付。汇总中断可单独重试汇总，无需重新执行成功子任务。详见[工作流与质量说明](workflows-quality-diagnostics.zh-CN.md)。

这不是平台收件箱的完整事务实现：平台 ACK／轮询游标与持久输入写入之间仍有中断窗口，也不会替第三方 MCP 内部的每一个外部副作用记账。会话恢复依赖 `pi-sessions/` 中保留的会话文件；如果 session 丢失，按现有失败处理使用新会话和可用上下文。检查点和提示词降低重复风险，不能证明真实模型永远不会重复操作。

## 数据与验证

新增文件均位于 `<home>`：`accounts.json`、`connection-checks.json`、`event-settings.json`、`event-status.json`、`outbox.json`、`recovery.json`。快照采用临时文件、fsync、原子替换，保存失败时不提交内存状态。备份完整数据目录，尤其保留 `media/`、`chats/` 及模型 session。完成的恢复记录保留约 500 条、完成的投递约 1000 条；未解决投递不因该阈值清理，去重只覆盖仍保留的记录。动作摘要最多保留最近 100 项，不能替代外部系统审计日志。

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
node dev/integrations.e2e.mjs
node dev/e2e.mjs
node dev/channels.e2e.mjs
node dev/attachments.e2e.mjs
node dev/local.e2e.mjs
DOTBOT_CHROME=/opt/google/chrome/chrome node dev/ui.e2e.mjs
cargo build --release && node dev/release.e2e.mjs
```

OAuth、MCP、GitHub 事件与社交发送使用本地协议模拟服务，恢复检查实际停止并重启隔离的 dotbot 服务。浏览器检查覆盖桌面、手机表单与恢复操作，并清理所有测试创建的浏览器。测试不使用真实平台账号、不向外部发送消息，不能替代平台权限、真实模型及生产网络验收。网页资源内嵌于二进制，发布构建后须重启原服务才会加载新功能。
