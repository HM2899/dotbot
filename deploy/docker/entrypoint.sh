#!/bin/sh
set -eu
if [ "${1:-serve}" = serve ]; then
    if ! docker info >/dev/null 2>&1; then
        echo '无法连接宿主机 Docker；请挂载 /var/run/docker.sock，云电脑桥接需要此连接。' >&2
        exit 1
    fi
    mkdir -p "$DOTBOT_HOME/workspace"
fi
exec dotbot "$@"
