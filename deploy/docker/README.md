# Docker 分发（含云电脑桥接）

适用 Linux Docker Engine（rootful），接收方安装 Docker 与 Compose 插件。镜像与目标 CPU 架构必须一致；ARM64 需在 ARM64 上重新打包。不支持 Docker Desktop 的此部署方式。

运行后是两个独立容器：`dotbot` 主服务容器，以及服务首次使用电脑时自动创建的 `teambot-computer-<团队标识>` 云电脑容器。云电脑不打进主服务镜像，也不会为每个 Bot 创建容器。Compose 管理主服务，项目现有桌面生命周期代码管理云电脑，避免重复创建和挂载冲突。

## 接收方运行

在收到的分发目录执行：

```sh
sha256sum -c images.sha256
docker load -i images.tar.gz
docker compose up -d
docker compose logs dotbot
```

浏览器打开 `http://127.0.0.1:4870/login`，使用首次日志中的管理员初始密码完成安装引导，然后在设置填写模型供应商密钥。远程访问可运行 `ssh -L 4870:127.0.0.1:4870 user@server`，再在本机打开同一地址。

服务及云电脑文件都在宿主机 `/srv/dotbot`。该目录必须独占用于这个团队，不要与现有原生服务同时使用。Compose 自动创建目录；服务目前以 root 运行，Docker socket 授予容器宿主机级控制权，只部署可信镜像。不要将该 socket 暴露给其他服务。

主服务使用 host 网络，仍只监听宿主机 loopback；两个容器的桌面画面与接管连接通过共享 runtime 目录内的 Unix socket 桥接。云电脑由服务在 `teambot-desktops` Docker bridge 网络按需创建，包含独立 Bot 屏幕、桌面代理、浏览器和共享文件挂载，无需手动发布桌面端口。容器内外 `/srv/dotbot` 必须同路径：桌面是宿主 Docker 创建的同级容器，不是在主服务内部启动 Docker daemon。

公网部署在宿主机配置 HTTPS 反向代理，并在 compose 的 command 添加 `"--public-url", "https://你的域名"`；代理保留 Host、支持 SSE/WebSocket，并拒绝 `/internal/`。不要直接对公网暴露 Docker API 或桌面端口。

```sh
docker compose down             # 停服务，保留数据，服务负责停止团队桌面
# 停止后备份整个 /srv/dotbot，包括数据库、密钥、附件及 desktops
# 升级：加载新的镜像后重新创建服务
docker compose up -d --force-recreate
```

归档包含主服务和基础 webtop 镜像，不包含个人数据、密钥或已登录的云电脑。桌面首次初始化仍需联网安装工具；模型、网站和 npx MCP 也需要网络。这不是完全离线运行包。停机目录备份保留桌面文件；额外安装在桌面容器系统层的软件需另行保存或重装。

## 从源码构建

在项目根目录运行：

```sh
docker compose build
docker compose up -d
# 或导出可发给别人的目录（包含两套镜像）：
bash deploy/docker/package.sh
```

输出 `dist/docker-linux-amd64/` 或对应架构目录，将整个目录发给对方。构建需要网络下载 Rust、Node、系统依赖和桌面镜像。可用 `DOTBOT_IMAGE=registry.example/dotbot:版本` 指定主服务标签，`DOTBOT_DESKTOP_IMAGE=镜像` 指定兼容的桌面镜像。不要把正在使用的数据目录打入镜像。
