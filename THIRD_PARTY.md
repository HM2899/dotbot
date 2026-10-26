# 开源来源与第三方组件

dotbot 自有代码以 [Apache-2.0](LICENSE) 发布；第三方代码、字体、依赖和镜像保留各自许可证，项目许可证不替换它们的条款。来源摘要同时保留在 [NOTICE](NOTICE)。本项目不是 Grok Bot 官方产品，界面与工作流参考不代表取得其内部代码或提示词。

| 项目 | 本项目中的用途 | 来源与许可说明 |
|---|---|---|
| DeepSeek-Bot / DS Bot | 团队及记忆文件格式、协议兼容，以及部分提示词与规则的参考和改写 | [Fei2411/DeepSeek-Bot](https://github.com/Fei2411/DeepSeek-Bot)，Apache-2.0；保留 NOTICE 声明 |
| pi-agent-core | Rust 移植并定制的代理循环，位于 `crates/pi-agent-core`，不是调用上游 JS 子进程 | [earendil-works/pi](https://github.com/earendil-works/pi)，MIT；固定来源提交及映射见 [UPSTREAM.md](crates/pi-agent-core/UPSTREAM.md)，许可见 [LICENSE](crates/pi-agent-core/LICENSE) |
| Agent Memory Repo | Markdown 记忆布局格式参考 | [AgentMemoryRepo/agentmemoryrepo](https://github.com/AgentMemoryRepo/agentmemoryrepo)；此处说明格式来源，不声明其仓库代码被整体包含或重新许可 |
| Rust 依赖 | HTTP、异步运行时、SQLite、序列化、加密、图片等 | 版本由 `Cargo.lock` 固定；[依赖与许可清单](licenses/RUST-DEPENDENCIES.md)，对应包内的许可与版权文件保存在 `licenses/rust/` |
| LinuxServer Webtop | 运行时按需拉取的 Linux 桌面、浏览器和应用环境 | [linuxserver/docker-webtop](https://github.com/linuxserver/docker-webtop)；镜像含多种独立许可软件，不属于主服务源码。镜像转分发时保留镜像内软件许可与来源说明，并按各组件条款提供所需来源 |
| Node.js / Debian / Rust Alpine 构建镜像 | Docker 运行时、系统工具和构建环境 | [Node.js](https://github.com/nodejs/node)、[Debian](https://www.debian.org/legal/licenses/)、[Rust](https://github.com/rust-lang/rust)、[Alpine](https://alpinelinux.org/)；基础镜像及系统包按各自许可，Debian 版权文件通常位于 `/usr/share/doc/*/copyright` |
| Playwright | 仅开发及浏览器回归测试 | [microsoft/playwright](https://github.com/microsoft/playwright)，Apache-2.0；`dev/package-lock.json` 固定版本，不提交 `node_modules` 或下载的浏览器 |
| Anthropic Skills 和外部 MCP 服务 | 可选安装的独立工作流或连接服务 | [anthropics/skills](https://github.com/anthropics/skills) 中各技能许可不同；项目提供目录和安装机制，不将外部技能统一许可为 Apache-2.0。安装时保留技能自己的 LICENSE、脚本、资源和来源；其他 MCP 包亦按各自条款 |

模型供应商、社交平台和 OAuth 服务是外部服务，并不因此成为本项目包含的开源代码。使用这些服务仍需相应账号及授权。

`licenses/RUST-DEPENDENCIES.md` 是当前 Linux 目标的构建清单，不是所有平台所有运行时软件的完整 SBOM；换平台、更新锁文件、再分发桌面镜像时应同步审查。Docker 分发包携带本文件、NOTICE、项目许可和随源码保存的许可文件。
