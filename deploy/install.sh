#!/bin/bash
# dotbot one-click installer for Linux (x86_64 / ARM64).
# Installs, for the current user: the dotbot binary, its runtimes (Docker for the shared
# virtual computer, Node.js for npx MCP servers and the standalone browser, uv for uvx MCP
# servers, Python 3), the default desktop image, and a systemd user service.
# System packages are installed with sudo; everything else stays under ~/.local.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
NODE_VERSION=${DOTBOT_NODE_VERSION:-v22.20.0}
DEFAULT_IMAGE=lscr.io/linuxserver/webtop:ubuntu-xfce

usage() {
    cat <<'EOF'
用法：bash install.sh [选项]

  --cn                 使用国内镜像（Node.js、npm、PyPI、uv、Docker 安装源、桌面镜像）
  --no-desktop         不安装 Docker，以无虚拟电脑模式运行（serve --no-desktop）
  --no-deps            不安装任何依赖，只安装 dotbot 本体和服务
  --no-service         不安装 systemd 用户服务，只安装可执行文件
  --skip-pull          不预先拉取虚拟电脑镜像（首次打开桌面时再拉取，约 2 GB）
  --port N             监听端口，默认 4870
  --public-url URL     公网 HTTPS 地址（需自行配置反向代理，见 Caddyfile.example）
  --home DIR           团队数据目录，默认 ~/.dotbot（已有 ~/.teambot 时沿用）
  --desktop-image IMG  虚拟电脑镜像，默认 lscr.io/linuxserver/webtop:ubuntu-xfce
  --admin-user NAME    安装时设置管理员用户名（随后提示输入密码）
  -y, --yes            非交互：不询问，跳过管理员设置（使用首次启动的初始密码）
  -h, --help           显示帮助

环境变量：DOTBOT_NODE_VERSION（默认 v22.20.0）、DOTBOT_GITHUB_PROXY（GitHub 下载前缀）。
EOF
}

cn=0; desktop=1; deps=1; service=1; pull=1; yes=0
port=4870; public_url=; home_dir=; image=; admin_user=
while [ $# -gt 0 ]; do
    case "$1" in
        --cn) cn=1;;
        --no-desktop) desktop=0;;
        --no-deps) deps=0;;
        --no-service) service=0;;
        --skip-pull) pull=0;;
        --port) port=${2:?}; shift;;
        --public-url) public_url=${2:?}; shift;;
        --home) home_dir=${2:?}; shift;;
        --desktop-image) image=${2:?}; shift;;
        --admin-user) admin_user=${2:?}; shift;;
        -y|--yes) yes=1;;
        -h|--help) usage; exit 0;;
        *) echo "未知选项：$1" >&2; usage >&2; exit 2;;
    esac
    shift
done

step() { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
ok() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }
die() { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }
interactive() { [ "$yes" = 0 ] && [ -t 0 ]; }

[ "$(uname -s)" = Linux ] || die "仅支持 Linux。"
case "$(uname -m)" in
    x86_64|amd64) arch=x86_64; node_arch=x64;;
    aarch64|arm64) arch=aarch64; node_arch=arm64;;
    *) die "不支持的架构：$(uname -m)（需要 x86_64 或 ARM64）";;
esac
[ -x "$here/bin/dotbot" ] || die "找不到 $here/bin/dotbot，请从完整发布包运行。"
"$here/bin/dotbot" --version >/dev/null 2>&1 || die "发布包中的 dotbot 无法在本机运行（架构不符？本机为 $arch）。"
[ -n "$public_url" ] && case "$public_url" in https://*) ;; *) die "--public-url 必须以 https:// 开头";; esac
[ -n "$image" ] || image=${DOTBOT_DESKTOP_IMAGE:-$DEFAULT_IMAGE}

if [ "$(id -u)" = 0 ]; then
    warn "正在以 root 安装。建议用普通用户运行（需要时会调用 sudo）；服务和数据将属于 root。"
    interactive && { read -r -p "  继续？[y/N] " a; [ "$a" = y ] || [ "$a" = Y ] || exit 1; }
    sudo() { "$@"; }
elif [ "$deps" = 1 ] && ! have sudo; then
    die "未找到 sudo，无法安装系统依赖。请用 root 安装 sudo，或加 --no-deps 并自行安装依赖。"
fi

bin_dir=$HOME/.local/bin
share_dir=$HOME/.local/share/dotbot
mkdir -p "$bin_dir" "$share_dir"
export PATH="$bin_dir:$PATH"
gh() { echo "${DOTBOT_GITHUB_PROXY:-}$1"; }
pypi_index() { [ "$cn" = 1 ] && echo https://mirrors.tuna.tsinghua.edu.cn/pypi/web/simple || echo https://pypi.org/simple; }
fetch() { curl -fL --retry 3 --connect-timeout 20 --progress-bar -o "$2" "$1"; }

# ---------- system packages ----------
pm=
if have apt-get; then pm=apt; elif have dnf; then pm=dnf; elif have yum; then pm=yum
elif have pacman; then pm=pacman; elif have zypper; then pm=zypper; elif have apk; then pm=apk; fi
apt_updated=0
pkg_install() {
    case "$pm" in
        apt) [ "$apt_updated" = 1 ] || { sudo apt-get update -qq >/dev/null; apt_updated=1; }
             echo "  apt-get install $*"
             sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq -o Dpkg::Use-Pty=0 "$@" >/dev/null;;
        dnf) sudo dnf install -y -q "$@";;
        yum) sudo yum install -y -q "$@";;
        pacman) sudo pacman -S --needed --noconfirm "$@";;
        zypper) sudo zypper --non-interactive install "$@";;
        apk) sudo apk add "$@";;
        *) return 1;;
    esac
}

if [ "$deps" = 1 ]; then
    step "基础工具"
    need=()
    for c in curl tar gzip; do have "$c" || need+=("$c"); done
    have python3 || need+=("$([ "$pm" = pacman ] && echo python || echo python3)")
    have update-ca-certificates || [ -d /etc/ssl/certs ] || need+=(ca-certificates)
    if [ ${#need[@]} -gt 0 ]; then
        [ -n "$pm" ] || die "无法识别包管理器，请先手动安装：${need[*]}"
        pkg_install "${need[@]}" || die "安装 ${need[*]} 失败"
    fi
    ok "curl / tar / python3 $(python3 -c 'import sys;print(sys.version.split()[0])')"

    # ---------- Node.js ≥ 20 (npx MCP servers, standalone browser) ----------
    step "Node.js"
    node_ok() { have node && node -e 'process.exit(Number(process.versions.node.split(".")[0])<20?1:0)' 2>/dev/null; }
    if node_ok; then
        ok "已有 Node.js $(node -v)"
    else
        mirror=${DOTBOT_NODE_MIRROR:-$([ "$cn" = 1 ] && echo https://npmmirror.com/mirrors/node || echo https://nodejs.org/dist)}
        tarball=node-$NODE_VERSION-linux-$node_arch.tar.gz
        tmp=$(mktemp -d)
        echo "  下载 $mirror/$NODE_VERSION/$tarball"
        fetch "$mirror/$NODE_VERSION/$tarball" "$tmp/$tarball" || die "下载 Node.js 失败（国内网络可加 --cn）"
        rm -rf "$share_dir/node"; mkdir -p "$share_dir/node"
        tar -xzf "$tmp/$tarball" -C "$share_dir/node" --strip-components=1; rm -rf "$tmp"
        for b in node npm npx corepack; do ln -sf "$share_dir/node/bin/$b" "$bin_dir/$b"; done
        hash -r; node_ok || die "Node.js 安装后仍不可用"
        ok "Node.js $(node -v) → $share_dir/node"
    fi

    # ---------- uv / uvx (Python MCP servers) ----------
    step "uv（uvx）"
    if have uvx; then
        ok "已有 $(uv --version 2>/dev/null || echo uvx)"
    else
        tmp=$(mktemp -d)
        uv_github() {
            fetch "$(gh https://github.com/astral-sh/uv/releases/latest/download/uv-$arch-unknown-linux-musl.tar.gz)" "$tmp/uv.tar.gz" &&
            tar -xzf "$tmp/uv.tar.gz" -C "$tmp" && install -m 755 "$tmp"/uv-*/uv "$tmp"/uv-*/uvx "$bin_dir/"
        }
        # The PyPI wheel carries the same static binaries, so any PyPI mirror works.
        uv_pypi() {
            echo "  从 $(pypi_index) 下载 uv"
            python3 -I - "$(pypi_index)" "$arch" "$bin_dir" <<'PY'
import io, re, sys, urllib.parse, urllib.request, zipfile, os
index, arch, dest = sys.argv[1:]
page = urllib.request.urlopen(f"{index}/uv/", timeout=60).read().decode()
links = re.findall(r'href="([^"]*uv-([0-9.]+)-py3-none-musllinux_1_1_%s\.whl[^"]*)"' % arch, page)
if not links: sys.exit("no uv wheel for " + arch)
href, _ = max(links, key=lambda l: tuple(int(x) for x in l[1].split(".")))
wheel = zipfile.ZipFile(io.BytesIO(urllib.request.urlopen(urllib.parse.urljoin(f"{index}/uv/", href), timeout=300).read()))
for name in ("uv", "uvx"):
    member = next(m for m in wheel.namelist() if re.fullmatch(r"uv-[^/]+\.data/scripts/" + name, m))
    path = os.path.join(dest, name)
    with open(path + ".new", "wb") as f: f.write(wheel.read(member))
    os.chmod(path + ".new", 0o755); os.replace(path + ".new", path)
PY
        }
        if [ "$cn" = 1 ]; then uv_pypi || uv_github; else uv_github || uv_pypi; fi \
            || die "下载 uv 失败（国内网络可加 --cn，或设置 DOTBOT_GITHUB_PROXY）"
        rm -rf "$tmp"
        ok "$("$bin_dir/uv" --version) → $bin_dir"
    fi

    # ---------- Docker (shared virtual computer) ----------
    if [ "$desktop" = 1 ]; then
        step "Docker（虚拟电脑）"
        if ! have docker; then
            case "$pm" in
                apt) pkg_install docker.io || true;;
                dnf) pkg_install moby-engine || true;;
                pacman|zypper|apk) pkg_install docker || true;;
            esac
            if ! have docker; then
                echo "  发行版仓库未提供 Docker，使用 get.docker.com 安装"
                curl -fsSL https://get.docker.com -o /tmp/get-docker.sh || die "下载 Docker 安装脚本失败"
                sudo sh /tmp/get-docker.sh $([ "$cn" = 1 ] && echo --mirror Aliyun) || die "Docker 安装失败"
            fi
        fi
        if have systemctl && [ -d /run/systemd/system ]; then sudo systemctl enable --now docker >/dev/null 2>&1 || true
        elif have rc-service; then sudo rc-update add docker default >/dev/null 2>&1 || true; sudo rc-service docker start || true; fi
        if [ "$(id -u)" != 0 ] && ! id -nG | tr ' ' '\n' | grep -qx docker; then
            getent group docker >/dev/null || sudo groupadd docker
            sudo usermod -aG docker "$USER"
            # Group membership applies at next login; grant the socket now so the service works today.
            have setfacl && sudo setfacl -m "u:$USER:rw" /var/run/docker.sock 2>/dev/null || true
            warn "已将 $USER 加入 docker 组；重新登录后永久生效。"
        fi
        for _ in $(seq 1 15); do docker info >/dev/null 2>&1 && break; sleep 1; done
        if docker info >/dev/null 2>&1; then ok "Docker $(docker version --format '{{.Server.Version}}')"
        else warn "当前用户暂时无法连接 Docker；重新登录后执行：systemctl --user restart dotbot"; fi
    fi
fi

# ---------- desktop image ----------
if [ "$desktop" = 1 ] && [ "$pull" = 1 ]; then
    step "虚拟电脑镜像 $image（约 2 GB，首次较慢）"
    dk=docker; docker info >/dev/null 2>&1 || dk="sudo docker"
    if ! $dk info >/dev/null 2>&1; then
        warn "Docker 不可用，跳过镜像拉取。"
    elif $dk image inspect "$image" >/dev/null 2>&1; then
        ok "镜像已存在"
    else
        pulled=0
        if [ "$cn" = 1 ] && [ "$image" = "$DEFAULT_IMAGE" ]; then
            mirror_image=m.daocloud.io/$image
            if $dk pull "$mirror_image"; then $dk tag "$mirror_image" "$image"; $dk rmi "$mirror_image" >/dev/null 2>&1 || true; pulled=1; fi
        fi
        [ "$pulled" = 1 ] || $dk pull "$image" || warn "镜像拉取失败；首次打开桌面时会重试（国内网络可加 --cn）。"
        $dk image inspect "$image" >/dev/null 2>&1 && ok "镜像已就绪"
    fi
fi

# ---------- dotbot binary ----------
step "安装 dotbot $(cat "$here/VERSION" 2>/dev/null || true)"
# Atomic replace on the same filesystem; a running dotbot keeps its old inode.
install -m 755 "$here/bin/dotbot" "$bin_dir/.dotbot.new"
mv -f "$bin_dir/.dotbot.new" "$bin_dir/dotbot"
for f in uninstall.sh preflight.sh Caddyfile.example nginx.example.conf README.md; do
    [ -f "$here/$f" ] && install -m 644 "$here/$f" "$share_dir/$f"
done
chmod 755 "$share_dir/uninstall.sh" "$share_dir/preflight.sh"
[ -d "$here/docs" ] && { rm -rf "$share_dir/docs"; cp -r "$here/docs" "$share_dir/docs"; }
ok "$bin_dir/dotbot"
case ":$(sh -lc 'echo $PATH' 2>/dev/null):" in *":$bin_dir:"*) ;; *)
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        [ -f "$rc" ] && ! grep -q 'dotbot installer' "$rc" && printf '\nexport PATH="$HOME/.local/bin:$PATH"  # dotbot installer\n' >> "$rc"
    done
    warn "已把 ~/.local/bin 加入 PATH（新终端生效）";;
esac

# ---------- team home & administrator ----------
if [ -n "$home_dir" ]; then team_home=$home_dir
elif [ -n "${DOTBOT_HOME:-}" ]; then team_home=$DOTBOT_HOME
elif [ ! -e "$HOME/.dotbot" ] && [ -e "$HOME/.teambot" ]; then team_home=$HOME/.teambot
else team_home=$HOME/.dotbot; fi
home_args=(); [ -n "$home_dir" ] && home_args=(--home "$home_dir")

running_elsewhere=0
if [ -e "$team_home/server.lock" ] && have flock && ! flock -n "$team_home/server.lock" true 2>/dev/null; then
    running_elsewhere=1
fi
unit_active=0
have systemctl && systemctl --user is-active --quiet dotbot 2>/dev/null && unit_active=1

if [ ! -f "$team_home/web-auth/admin.json" ] && [ "$running_elsewhere" = 0 ] && { [ -n "$admin_user" ] || interactive; }; then
    step "管理员账号"
    if [ -z "$admin_user" ]; then
        read -r -p "  管理员用户名（直接回车跳过，改用首次启动显示的初始密码）：" admin_user
    fi
    if [ -n "$admin_user" ]; then
        while :; do
            read -r -s -p "  密码（至少 6 位）：" p1; echo
            read -r -s -p "  再次输入：" p2; echo
            [ "$p1" = "$p2" ] && [ ${#p1} -ge 6 ] && break
            warn "两次输入不一致或少于 6 位，请重试。"
        done
        printf '%s\n' "$p1" | "$bin_dir/dotbot" "${home_args[@]}" admin-password --username "$admin_user" --password-stdin >/dev/null
        unset p1 p2
        ok "管理员 $admin_user 已设置"
    fi
fi

# ---------- service ----------
serve_args=(serve --port "$port")
[ -n "$public_url" ] && serve_args+=(--public-url "$public_url")
[ "$desktop" = 0 ] && serve_args+=(--no-desktop)
[ "$image" != "$DEFAULT_IMAGE" ] && serve_args+=(--desktop-image "$image")
exec_line="$bin_dir/dotbot ${home_args[*]} ${serve_args[*]}"
exec_line=${exec_line//  / }

has_user_systemd=0
if have systemctl && [ -d /run/systemd/system ] && systemctl --user show-environment >/dev/null 2>&1; then has_user_systemd=1; fi

if [ "$service" = 1 ] && [ "$has_user_systemd" = 1 ]; then
    step "systemd 用户服务"
    unit_dir=$HOME/.config/systemd/user; mkdir -p "$unit_dir"
    {
        cat <<EOF
[Unit]
Description=dotbot persistent agent team
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=PATH=$bin_dir:$share_dir/node/bin:/usr/local/bin:/usr/bin:/bin
EOF
        if [ "$cn" = 1 ]; then
            echo "Environment=npm_config_registry=https://registry.npmmirror.com"
            echo "Environment=UV_DEFAULT_INDEX=https://mirrors.tuna.tsinghua.edu.cn/pypi/web/simple"
        fi
        cat <<EOF
ExecStart=$exec_line
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
KillMode=control-group

[Install]
WantedBy=default.target
EOF
    } > "$unit_dir/dotbot.service"
    systemctl --user daemon-reload
    systemctl --user enable dotbot >/dev/null 2>&1
    ok "$unit_dir/dotbot.service"
    # Keep running after logout / across reboots without a login.
    if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" != yes ]; then
        loginctl enable-linger "$USER" 2>/dev/null || sudo loginctl enable-linger "$USER" 2>/dev/null \
            && ok "已开启 linger：退出登录后服务继续运行" || warn "未能开启 linger，退出登录后服务会停止"
    fi
    if [ "$running_elsewhere" = 1 ] && [ "$unit_active" = 0 ]; then
        warn "检测到 $team_home 已有手动启动的 dotbot 在运行，未启动服务。"
        warn "停止那个进程后执行：systemctl --user start dotbot"
    elif [ "$unit_active" = 1 ]; then
        # Upgrade in place: the running service re-execs the new binary and keeps the shared computer.
        systemctl --user kill -s USR2 --kill-whom=main dotbot && ok "已平滑升级正在运行的服务"
    else
        systemctl --user start dotbot && ok "服务已启动"
    fi
else
    [ "$service" = 1 ] && warn "此系统没有可用的 systemd 用户服务，未安装后台服务。"
    printf '  手动启动：%s\n' "$exec_line"
fi

# ---------- summary ----------
url=${public_url:-http://127.0.0.1:$port}
step "安装完成"
echo "  访问地址：$url/login"
if [ "$service" = 1 ] && [ "$has_user_systemd" = 1 ]; then
    sleep 2
    pw=$(journalctl --user -u dotbot -n 200 --no-pager 2>/dev/null | sed -n 's/.*初始管理员密码（仅首次显示）：//p' | tail -1)
    [ -n "$pw" ] && echo "  首次登录：用户名 admin，初始密码 $pw（登录后按引导修改）"
    echo "  查看日志：journalctl --user -u dotbot -f"
    echo "  重启服务：systemctl --user restart dotbot（会关闭已打开的虚拟电脑）"
fi
echo "  模型配置：登录后在「设置 → 模型与供应商」添加 API 密钥"
[ "$url" = "http://127.0.0.1:$port" ] && echo "  远程访问：ssh -L $port:127.0.0.1:$port 用户@服务器，或用 --public-url 配合 HTTPS 反向代理"
echo "  卸载：bash $share_dir/uninstall.sh"
