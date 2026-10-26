#!/bin/bash
# Read-only checks on the destination machine. No package installation or container start.
set -eu
failures=0
check() {
    if command -v "$1" >/dev/null 2>&1; then printf '✓ %s\n' "$1"
    else printf '✗ 缺少 %s\n' "$1"; failures=$((failures+1)); fi
}
printf '系统：%s / %s\n' "$(uname -s)" "$(uname -m)"
case "$(uname -m)" in
    aarch64|arm64|x86_64|amd64) ;;
    *) printf '✗ 建议使用 64 位 ARM64 或 x86_64 系统。\n'; failures=$((failures+1));;
esac
check node
check docker
check python3
if command -v node >/dev/null 2>&1; then
    node -e 'console.log("Node.js："+process.version+" / "+process.arch);if(Number(process.versions.node.split(".")[0])<20)process.exit(1)' || { printf '✗ 请使用 Node.js 20 或更新的 LTS 版本。\n'; failures=$((failures+1)); }
fi
if command -v docker >/dev/null 2>&1; then
    docker info --format 'Docker：{{.Architecture}} / {{.OSType}}' || { printf '✗ 当前用户无法连接 Docker。\n'; failures=$((failures+1)); }
fi
printf '内存与磁盘：\n'
free -h
df -h "${DOTBOT_HOME:-${TEAMBOT_HOME:-$HOME}}"
if [ "${1:-}" = '--check-image' ]; then
    image="${DOTBOT_DESKTOP_IMAGE:-${TEAMBOT_DESKTOP_IMAGE:-lscr.io/linuxserver/webtop:ubuntu-xfce}}"
    printf '检查镜像架构：%s\n' "$image"
    if manifest_json=$(docker manifest inspect "$image"); then
      if ! python3 -c '
import json,platform,sys
try:
 data=json.load(sys.stdin)
except (ValueError,TypeError):
 print("✗ 镜像平台清单格式无效，请检查镜像地址及仓库响应。")
 sys.exit(1)
arch={"aarch64":"arm64","arm64":"arm64","x86_64":"amd64","amd64":"amd64"}.get(platform.machine())
platforms=[m.get("platform",{}) for m in data.get("manifests",[])]
print("支持："+", ".join(p.get("os","")+"/"+p.get("architecture","") for p in platforms if p.get("architecture")!="unknown"))
if not any(p.get("architecture")==arch and p.get("os")=="linux" for p in platforms):
 print("无法确认当前架构的镜像支持；单架构镜像请另行 inspect 核实。")
 sys.exit(1)
' <<< "$manifest_json"; then failures=$((failures+1)); fi
    else
      printf '✗ 无法读取镜像平台清单，请检查镜像仓库网络与 TLS，再重试。\n'
      failures=$((failures+1))
    fi
fi
printf '\n检查结束：%s 项未通过。还需在服务同一用户下确认模型认证，设置管理员密码和 HTTPS 域名。\n' "$failures"
test "$failures" -eq 0
