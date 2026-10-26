# 持续运行与远程访问

需要容器分发（包括云电脑桥接）时，参见 [Docker 打包与部署](docker/README.md)。

在持续开机的 Linux 主机上运行 dotbot，关闭聊天页面或客户端电脑不会中断服务器上的代理。运行主机本身关机时无法继续执行任务。每个 Bot 的桌面需要该主机上的 Docker；Node.js（用于 npx 启动的 MCP 服务）及用户级服务的 PATH 也须在同一用户下可用。模型由内置 Rust 引擎直接调用，不需要 Claude CLI；启动后在「设置 → 模型与供应商」填写 API 密钥。

构建并准备用户级服务：

```sh
cargo build --release
mkdir -p ~/.local/bin ~/.config/systemd/user
install -m 755 target/release/dotbot ~/.local/bin/dotbot
cp deploy/dotbot.service ~/.config/systemd/user/dotbot.service
systemctl --user daemon-reload
systemctl --user enable --now dotbot
journalctl --user -u dotbot -f
```

按安装位置修改服务中的 PATH。如需在用户退出登录后持续运行，由该服务器的管理员配置用户 linger。以上是部署模板，本次项目修改不会自动安装或启动服务。

仅远程访问网页时，可以通过 SSH 端口转发连接服务器的 `127.0.0.1:4870`，在浏览器打开日志中的登录地址。需要接收 GitHub/Slack/Teams 事件时，在你管理的域名上配置 TLS 反向代理，参考 `nginx.example.conf`，并在服务命令中添加：

```text
--public-url https://team.example.com
```

首次启动会生成初始管理员密码，按下节完成安装引导；也可在启动前用命令配置账号。此参数设置受信任的 HTTPS 域名及 webhook 域名；服务仍监听 loopback，不会自动建立 DNS、隧道、TLS 或平台订阅。直接在所选域名的 `/login` 登录；`/internal/` 应始终拒绝公网访问。检查 SSE 消息流及桌面 WebSocket 均能通过代理。

新安装的数据默认保存在 `~/.dotbot`；若该目录不存在而 `~/.teambot` 已存在，则自动沿用旧目录。`--home` 优先于 `DOTBOT_HOME`，后者优先于旧 `TEAMBOT_HOME`；已有环境变量的旧前缀仍兼容。备份时同时保存状态、记忆、聊天、技能、桌面 home、浏览器 profiles、密钥与任务记录。停止服务会退出当前执行和浏览器；持久文件保留。升级时避免两个实例同时写同一个 home。中断的任务不会自动重放，重试前先核对已发生的外部操作。

从 teambot 更名升级时，先停用旧 `teambot.service`，再启用 `dotbot.service`；不可同时启动。不要为改名移动团队目录、桌面目录或重建共享容器。内部状态文件、Cookie、MCP 工具命名空间及容器标识保留旧名称，以复用聊天会话与登录态。下面显式指定 `~/.dotbot` 的命令用于新安装；已有团队应替换为其实际目录。


## 登录凭据与兼容性

本地和公网统一仅支持管理员账号密码登录。没有管理员配置时自动创建待初始化账号，旧 `?token=` 链接和旧令牌 Cookie 均不能授权；旧 `<home>/web-auth/token` 文件不再读取或生成，可自行删除。已有管理员账号及会话保持有效。管理员密码和会话摘要保存在 `<home>/web-auth/admin.json`，普通重启和 SIGUSR2 升级复用有效会话；`--rotate-login` 撤销全部会话。内部 MCP 凭据仍在每次进程启动时重新生成，只用于工具进程通信，不能登录网页。登录文件损坏会阻止启动，不会自动覆盖或降级到其他登录方式。

## 首次安装引导

首次运行 `dotbot serve` 时，默认账号为 `admin`，初始密码按安装独立随机生成，只在第一次启动终端显示；使用 systemd 时在该服务的私有启动日志中查看。初始密码仅保存 Argon2id 摘要，接口不返回明文。

1. 打开 `/login`，输入 `admin` 和初始密码。
2. 引导第一步设置管理员用户名、新密码并确认密码。新密码至少 6 个字符、最多 256 字节，必须与初始密码不同。
3. 第二步填写昵称（1–40 个字符）和个人简介（选填，最多 500 个字符），保存到团队资料；头像可稍后在设置中添加。
4. 完成后使用新账号重新登录。所有初始会话撤销，引导入口关闭。

初始登录只授予 15 分钟的引导会话，无法访问聊天、附件或桌面；过期后可用初始密码重新登录。未完成引导时重启会保留初始化状态、密码和有效会话，不会再次生成或显示密码。若初始密码遗失，停止服务后用下节命令重设账号。已有管理员配置不会出现首次引导；启动前用 `admin-password` 显式配置账号也会跳过引导。

## 仅管理员登录

停止服务后，在运行服务的同一用户下设置密码（若使用自定义目录，所有命令使用同一个 `--home`）：

```sh
dotbot --home ~/.dotbot admin-password
dotbot --home ~/.dotbot serve --public-url https://team.example.com
```

终端隐藏输入，密码至少 6 个字符、最多 256 字节。仅有一个管理员账号，初始用户名为 `admin`；可用 `dotbot --home ~/.dotbot admin-password --username owner` 指定用户名，不传 `--username` 时保留已有用户名。用户名为 1–64 个字符，可用中文、字母、数字、点、下划线和短横线。没有公开注册、找回密码或其他账号入口。密码使用 Argon2id 加盐摘要，服务器不保存明文。自动化部署可用 `admin-password --password-stdin` 从私密输入流读取一行；不要将密码写入命令参数、服务配置、命令历史或日志。忘记密码时停止服务后重新运行此命令，旧会话全部撤销；登录文件损坏时可同时指定 `--username` 重置账号。

页面采用居中、简洁的登录布局，适配手机及深浅主题。默认登录 7 天，选择「保持登录」后为 30 天。会话持久化的仅是随机凭据的 SHA-256 摘要；最多保留 32 个会话。启用 HTTPS 公网地址后 Cookie 设置 Secure、HttpOnly、SameSite=Strict。设置 → 通用 → 账号与安全可修改管理员用户名、密码或退出；修改账号须验证当前密码，新密码留空时保留原密码。保存修改撤销所有设备的登录，需用新账号信息重新登录；退出只撤销当前会话。已打开的消息流和桌面连接在约一秒内断开。代理本身可能仍有已缓冲数据。任务继续在服务器运行，不因退出登录而取消。旧版管理员文件自动沿用 `admin` 用户名和有效会话。

登录及密码验证共用每分钟 20 次的全局限流，最多同时执行两次密码哈希。该限制不信任客户端自报 IP，避免代理配置错误绕过限制；大量恶意流量应由入口代理或 CDN 进一步限制。浏览器写操作和桌面 WebSocket 验证 Origin，Host 只接受本地实际监听地址和启动时配置的公网域名；不要将公网请求的 Host 改写成其他名称。Webhook 签名/密钥及内部 MCP 的独立认证保留。

公网模式的 `--public-url` 必须为 HTTPS，否则拒绝启动。首次安装完成引导后才启用聊天与桌面。日常运行仍只监听 `127.0.0.1:4870`，安全组只开放实际需要的 SSH 与 HTTPS 端口，桌面、Docker API 和 4870 不直接开放公网。可使用 `Caddyfile.example` 自动申请证书并代理 WebSocket/SSE；使用 Nginx 时，将示例里的 `map` 放入 `http {}`，locations 放入已有 TLS server。首次安装/更新二进制不自动改动正在运行的服务。

## ARM64 公网主机

推荐 64 位 Linux（例如 Ubuntu 24.04 ARM64），Node.js 20 或更新的 LTS 和 Docker。构建时使用 Rust 1.89 或更新稳定版；以依赖要求的版本为准。在目标机原生构建最直接：

```sh
cargo build --locked --release
mkdir -p ~/.local/bin
install -m 755 target/release/dotbot ~/.local/bin/dotbot
bash deploy/preflight.sh --check-image
dotbot admin-password
```

`preflight.sh` 只检查命令可用性、CLI 运行、Node 架构、Docker 连接、内存和磁盘；`--check-image` 还联网检查默认桌面镜像的当前平台清单。当前 `lscr.io/linuxserver/webtop:ubuntu-xfce` 同时提供 `linux/arm64` 和 `linux/amd64`，代码没有强制 amd64，Docker 按本机选择。自定义镜像通过 `DOTBOT_DESKTOP_IMAGE=镜像 bash deploy/preflight.sh --check-image` 检查，并在启动时显式使用相同的 `--desktop-image 镜像`。建议上线时记录、固定经验证的多架构镜像摘要，避免浮动标签改变环境。

服务建议原生运行在宿主机，由 Docker 提供共享电脑；不需要把服务再放入容器或挂载 Docker socket 给公网代理。无桌面需求时可加 `--no-desktop --no-browser` 降低占用。多个 Bot 的屏幕和浏览器仍消耗内存；建议从 4 核 / 8 GB 起步，多浏览器工作负载优先 16 GB，并按实际并发测量，磁盘为浏览器、文件和镜像预留空间。冷启动拉镜像和安装桌面工具需要可用的镜像及软件源网络，首次上线提前检查。

跨机器迁移时，在维护窗口停止旧服务后备份完整团队目录和工作目录；新机重新安装原生 CLI/运行依赖，调整 workspace、Bot 文件夹和用户级服务 PATH。共享数据在 `desktops/shared`，旧文件在 `desktops/bot-*`，二者一起保留；`web-auth` 是管理员及会话数据，备份须私密。不要复制 x86 可执行文件、node_modules 或运行锁来代替安装，也不要导入 amd64 容器快照期待在 ARM 原生运行。遇到旧 `:migration` 镜像缺失或架构不符，服务会提示并使用配置的桌面镜像；文件保留，但旧电脑里额外安装的软件需在 ARM 重装。部分网站或系统绑定的登录凭据可能需要重新登录。

本仓库测试在当前 x86_64 主机运行；镜像清单已核实 ARM64 支持，ARM 实机上的 CLI、网站、图形应用和完整任务流程仍须在目标机验收。

## 平滑升级

将新二进制原子替换到原启动路径后，向**已核实的 dotbot 服务 PID**发送 `SIGUSR2`（如 `kill -USR2 <PID>`）。服务保存中断记录、停止自己的在途命令并退出旧监听，然后从原启动路径执行新二进制。共享容器、独立屏幕、已打开的桌面浏览器及有效的用户接管租约保留，网页消息流自动重连。已有任务的外部操作不会盲目重放，需依据原有检查点恢复。普通 SIGTERM/Ctrl-C 仍执行完整关闭；`systemctl --user restart` 默认使用 SIGTERM，因此也会关闭共享电脑。旧版本未安装 SIGUSR2 处理器时，首次升级应按普通停止/启动流程进行，不能直接向旧进程发送该信号。

当前进程保留 PID 并执行新文件，不创建第二个同 home 服务；同一团队目录使用进程锁阻止重复启动。长连接最多额外等待五秒；若远程命令停止无法确认，先等待其远程超时，不在旧命令仍可能写文件时升级。二进制替换须在同一文件系统准备临时文件后用 rename/mv 发布，不能截断正在运行的可执行文件。如果新程序启动依赖缺失仍可能启动失败，原共享电脑和持久数据保留，修复依赖后重新启动即可。

网页发现静态资源版本改变时显示「更新界面」按钮。点击后保留当前标签页的聊天文字、已上传附件和纠正目标；上传未完成、正在发送或设置窗口尚未关闭时先保留原页面。断线重连不自动重发用户消息。窗口持续无数据超过 65 秒时重建消息流，联网事件可提前触发重连。

## SQLite 数据保存

服务默认使用团队目录内的 `dotbot.sqlite3`，首次启动自动迁移旧 JSON 和 JSONL，原文件保留。SQLite 内置编译，ARM64 无需额外安装 SQLite 服务。迁移后请以数据库为准；备份须包含 WAL 状态、管理员认证目录、附件、记忆、技能和桌面文件，参见 [SQLite 迁移与备份](../docs/sqlite.zh-CN.md)。
