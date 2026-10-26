#!/usr/bin/env bash
# Build a transferable bundle, including the cloud desktop image.
set -euo pipefail
cd "$(dirname "$0")/../.."
image=${DOTBOT_IMAGE:-dotbot:0.1.0}
desktop=${DOTBOT_DESKTOP_IMAGE:-lscr.io/linuxserver/webtop:ubuntu-xfce}
platform=$(docker version --format '{{.Server.Os}}-{{.Server.Arch}}')
out="dist/docker-$platform"
mkdir -p "$out"
docker build -t "$image" .
if ! docker image inspect "$desktop" >/dev/null 2>&1; then
    docker pull "$desktop"
fi
docker image save "$image" "$desktop" | gzip > "$out/images.tar.gz"
cp compose.yaml "$out/compose.yaml"
printf 'DOTBOT_IMAGE=%s\n' "$image" > "$out/.env"
# Compose passes the image explicitly so custom desktop builds work as well.
printf '\n' >> "$out/compose.yaml"
python3 - "$out/compose.yaml" "$desktop" <<'PY'
import json, pathlib, sys
p = pathlib.Path(sys.argv[1])
s = p.read_text()
s = s.replace('"--workspace", "/srv/dotbot/workspace"]', '"--workspace", "/srv/dotbot/workspace", "--desktop-image", ' + json.dumps(sys.argv[2]) + ']')
# Recipients load the image; the source tree is not needed.
s = s.replace('    build:\n      context: .\n', '')
p.write_text(s)
PY
cp deploy/docker/README.md "$out/README.md"
cp LICENSE NOTICE THIRD_PARTY.md "$out/"
cp -R licenses "$out/licenses"
(cd "$out" && sha256sum images.tar.gz > images.sha256)
tar -C dist -czf "$out.tar.gz" "docker-$platform"
(cd dist && sha256sum "docker-$platform.tar.gz" > "docker-$platform.tar.gz.sha256")
printf '分发目录：%s（仅适用于 %s）\n' "$out" "$platform"
