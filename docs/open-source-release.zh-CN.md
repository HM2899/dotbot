# GitHub 源码发布与部署打包

## 源码包（不需要 Docker）

需要 Python 3.11+。在项目根目录运行：

```sh
bash deploy/source-package.sh
cd dist
sha256sum -c dotbot-*-source.tar.gz.sha256
```

输出源码压缩包、压缩包校验文件和文件清单；包内另有逐文件 SHA-256 清单。
脚本只选择网页端与服务端源码、测试、部署、文档和许可证目录（不包含 `mobile/`），排除各级编译目录、依赖缓存、运行数据、环境文件及密钥文件；遇到常见真实令牌格式或私钥内容时拒绝打包，不打印密钥值。它不会读取或包含 Git 历史，不上传任何内容。新增源码目录须显式加入脚本的白名单。

解压后检查文件清单与图片。将解压目录作为新的 GitHub 仓库上传，可避免误带当前工作目录中的个人配置或历史。`Cargo.lock`、`dev/package-lock.json` 属于可复现构建输入，保留在源码中；`target/`、APK、安装包和 Docker 镜像不提交到源码仓库。

`.gitignore` 只影响未跟踪文件，不能清除已提交的凭据。若旧仓库曾提交密钥，应先撤销或轮换，再清理历史；不要直接推送含旧密钥的历史。打包检查是辅助措施，不是任意格式秘密的完整检测器。发布前核对 README 图片、项目自定义提示词和测试夹具中没有个人内容。

## Docker 源码部署

详见 [Docker 部署说明](../deploy/docker/README.md)。需要 Linux rootful Docker Engine 和 Compose 插件：

```sh
docker compose config --quiet
docker compose build
docker compose up -d
docker compose logs dotbot
```

使用首次启动日志中的初始密码，在 `http://127.0.0.1:4870/login` 完成管理员设置，再通过页面录入供应商密钥。不要把密钥写进 Dockerfile、Compose 或 GitHub。运行数据保存在宿主 `/srv/dotbot`；不要与原生服务共享同一团队目录。

此方案挂载宿主 Docker socket，用于按需启动独立云电脑容器，拥有宿主级控制能力，只运行可信镜像。默认 host 网络只提供 loopback 登录入口；公网使用 HTTPS 反向代理并设置 `--public-url`。Docker Desktop 不适用此桥接方式。

## 可转移镜像包与原生安装包

```sh
# 构建并导出主服务及 webtop 基础镜像
bash deploy/docker/package.sh
# 输出 dist/docker-linux-<架构>/ 及同名 tar.gz、校验文件

# 静态 Linux 二进制、自解压安装器及 tar.gz
bash deploy/package.sh
```

接收方解压镜像包后执行 `sha256sum -c images.sha256`、`docker load -i images.tar.gz`、`docker compose up -d`。镜像包与目标 CPU 架构必须匹配，默认按构建宿主架构生成。包中不含 `/srv/dotbot` 或个人数据；首次桌面初始化、模型及网站仍需网络。

编译文件只能按需作为 GitHub Release 附件或容器仓库镜像分发，不加入源码提交。分发时一并保留 `LICENSE`、`NOTICE`、`THIRD_PARTY.md`、`licenses/` 以及镜像内系统软件的许可证信息。第三方组件来源见 [开源说明](../THIRD_PARTY.md)。
