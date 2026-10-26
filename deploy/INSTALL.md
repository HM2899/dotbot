# dotbot Linux 一键安装

支持 x86_64 / ARM64 的 Ubuntu、Debian、Fedora、RHEL 系、Arch、openSUSE、Alpine。dotbot 本体是静态编译的单个文件，不依赖系统 glibc 版本。

## 安装

```sh
bash dotbot-<版本>-linux-<架构>.run          # 一键安装（自解压）
bash dotbot-<版本>-linux-<架构>.run --cn     # 国内网络：使用国内镜像
```

也可以解压 `.tar.gz` 后执行 `bash install.sh`，参数完全相同。请用普通用户运行，需要安装系统软件时脚本会调用 sudo。

安装器会依次完成：

| 组件 | 用途 | 安装位置 |
|---|---|---|
| curl、tar、Python 3 | 基础工具 | 系统包管理器 |
| Node.js ≥ 20 | npx 类 MCP 服务、独立浏览器模式 | 系统已有则复用，否则 `~/.local/share/dotbot/node` |
| uv / uvx | Python 类 MCP 服务 | `~/.local/bin` |
| Docker | 团队共享的虚拟电脑（每个 Bot 独立屏幕） | 系统包管理器，必要时用 get.docker.com |
| 桌面镜像 | `lscr.io/linuxserver/webtop:ubuntu-xfce`，约 2 GB | Docker |
| dotbot | 服务本体 | `~/.local/bin/dotbot` |
| systemd 用户服务 | 开机自启、退出登录后继续运行（linger） | `~/.config/systemd/user/dotbot.service` |

交互安装时可以直接设置管理员账号；跳过时，安装结束会显示首次登录用的初始密码。打开 `http://127.0.0.1:4870/login` 登录，再到「设置 → 模型与供应商」填写模型 API 密钥。

## 常用选项

```text
--cn                 国内镜像（npmmirror、清华 PyPI、阿里云 Docker 源、DaoCloud 镜像）
--no-desktop         不装 Docker，不使用虚拟电脑
--no-deps            只装 dotbot 本体和服务，依赖自行准备
--no-service         不装后台服务，只装可执行文件
--skip-pull          不预拉桌面镜像，首次打开桌面时再拉取
--port 4870          监听端口（只监听 127.0.0.1）
--public-url https://team.example.com   公网 HTTPS 地址，配合 Caddyfile.example / nginx.example.conf
--home DIR           团队数据目录（默认 ~/.dotbot）
--admin-user NAME    安装时设置管理员用户名
-y                   非交互安装
```

## 升级与卸载

升级：重新运行新版本的安装包。服务正在运行时会平滑升级（SIGUSR2），共享电脑和已打开的桌面保持不变。

卸载：`bash ~/.local/share/dotbot/uninstall.sh`。团队数据默认保留；加 `--purge` 会同时删除数据、工作目录和虚拟电脑容器。

## 日常操作

```sh
systemctl --user status dotbot
journalctl --user -u dotbot -f
systemctl --user restart dotbot      # 会关闭已打开的虚拟电脑
bash ~/.local/share/dotbot/preflight.sh --check-image   # 环境自检
```

远程访问可以用 SSH 转发：`ssh -L 4870:127.0.0.1:4870 用户@服务器`。公网部署、备份、迁移等内容见 `docs/deploy.zh-CN.md`。

## 从源码打包

```sh
bash deploy/package.sh     # 需要 Docker；产物在 dist/
```

打包时会在 `rust:alpine` 容器里编译出静态二进制，生成 `.run`、`.tar.gz` 和 `.sha256`。产物架构与构建机一致；ARM64 安装包需要在 ARM64 机器上打包。
