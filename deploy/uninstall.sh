#!/bin/bash
# Removes the dotbot service and binary installed by install.sh.
# Team data (~/.dotbot), the workspace and the virtual computer are kept unless --purge.
set -euo pipefail
purge=0; yes=0
for a in "$@"; do
    case "$a" in
        --purge) purge=1;;
        -y|--yes) yes=1;;
        -h|--help) echo "用法：bash uninstall.sh [--purge] [-y]
  --purge  同时删除团队数据、工作目录、虚拟电脑容器、网络与镜像（不可恢复）"; exit 0;;
        *) echo "未知选项：$a" >&2; exit 2;;
    esac
done

bin_dir=$HOME/.local/bin; share_dir=$HOME/.local/share/dotbot
unit=$HOME/.config/systemd/user/dotbot.service

if command -v systemctl >/dev/null && [ -f "$unit" ]; then
    systemctl --user disable --now dotbot 2>/dev/null || true
    rm -f "$unit"; systemctl --user daemon-reload 2>/dev/null || true
    echo "✓ 已停止并移除 systemd 用户服务"
fi
rm -f "$bin_dir/dotbot"
# Node.js / uv are only removed when the installer put them there.
for b in node npm npx corepack; do
    [ "$(readlink "$bin_dir/$b" 2>/dev/null)" = "$share_dir/node/bin/$b" ] && rm -f "$bin_dir/$b"
done
rm -rf "$share_dir"
echo "✓ 已移除 dotbot 程序（uv/uvx、Docker、系统软件包保留）"

if [ "$purge" = 0 ]; then
    echo "团队数据保留在 ~/.dotbot（或 ~/.teambot）与 ~/dotbot-workspace；彻底删除请加 --purge。"
    exit 0
fi

targets=()
for d in "$HOME/.dotbot" "$HOME/.teambot" "$HOME/dotbot-workspace" "$HOME/teambot-workspace"; do [ -e "$d" ] && targets+=("$d"); done
containers=$(docker ps -aq --filter label=teambot.desktop=1 2>/dev/null || true)
echo "将永久删除："
for d in "${targets[@]}"; do echo "  $d"; done
[ -n "$containers" ] && echo "  虚拟电脑容器：$(docker ps -a --filter label=teambot.desktop=1 --format '{{.Names}}' | tr '\n' ' ')"
echo "  Docker 网络 teambot-desktops 与 :migration 快照镜像"
if [ "$yes" = 0 ]; then
    read -r -p "确认删除？输入 yes 继续：" answer
    [ "$answer" = yes ] || { echo "已取消。"; exit 1; }
fi
[ -n "$containers" ] && docker rm -f $containers >/dev/null
docker network rm teambot-desktops >/dev/null 2>&1 || true
docker images --format '{{.Repository}}:{{.Tag}}' 2>/dev/null | grep '^teambot-computer-.*:migration$' | xargs -r docker rmi >/dev/null 2>&1 || true
# Desktop homes are written by the container's user; fall back to a container for removal.
for d in "${targets[@]}"; do
    rm -rf "$d" 2>/dev/null || docker run --rm -v "$d:/x" alpine sh -c 'rm -rf /x/* /x/.[!.]* /x/..?*' >/dev/null 2>&1 && rm -rf "$d" || echo "! 无法完全删除 $d，请手动处理"
done
echo "✓ 已清除。虚拟电脑基础镜像保留：docker rmi lscr.io/linuxserver/webtop:ubuntu-xfce"
