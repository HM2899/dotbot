#!/usr/bin/env bash
# Isolated startup test. No browser or desktop is launched.
set -euo pipefail
image=${DOTBOT_IMAGE:-dotbot:0.1.0}
name="dotbot-docker-smoke-$$"
port=${DOTBOT_SMOKE_PORT:-14870}
cleanup() { docker rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT INT TERM
docker run -d --name "$name" --network host \
    -v /var/run/docker.sock:/var/run/docker.sock \
    "$image" serve --port "$port" --workspace /srv/dotbot/workspace >/dev/null
for ((i=0; i<30; i++)); do
    if docker exec "$name" curl -fsS -o /dev/null "http://127.0.0.1:$port/login"; then
        docker exec "$name" dotbot --version
        docker exec "$name" node --version
        docker exec "$name" docker version --format '{{.Server.Version}}'
        echo 'PASS: 容器启动、登录页、Node 与宿主 Docker 桥接'
        exit 0
    fi
    sleep 1
done
echo 'FAIL: 登录页未就绪' >&2
exit 1
