#!/bin/bash
# Builds a Linux release: a static (musl) dotbot binary plus the one-click installer.
#   bash deploy/package.sh            # dist/dotbot-<version>-linux-<arch>.{run,tar.gz}
#   DOTBOT_BUILD_IMAGE=... bash deploy/package.sh   # another rust:alpine image (mirror)
# The binary is built in a rust:alpine container so it runs on any glibc or musl distro.
# Docker builds for the host architecture; run this on an ARM64 machine for an ARM64 release.
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m); case "$arch" in amd64) arch=x86_64;; arm64) arch=aarch64;; esac
image=${DOTBOT_BUILD_IMAGE:-rust:1-alpine}
name=dotbot-$version-linux-$arch
out=$root/dist; stage=$out/$name

if [ -n "${DOTBOT_RELEASE_BINARY:-}" ]; then
    binary=$DOTBOT_RELEASE_BINARY
else
    echo "==> 构建静态二进制（$image，$arch）"
    mkdir -p "${CARGO_HOME:-$HOME/.cargo}/registry"
    docker run --rm -u "$(id -u):$(id -g)" -e HOME=/tmp -e CARGO_HOME=/cargo \
        -v "${CARGO_HOME:-$HOME/.cargo}/registry:/cargo/registry" -v "$root:/src" -w /src \
        "$image" cargo build --locked --release --target-dir target/dist-musl
    binary=$root/target/dist-musl/release/dotbot
fi
file "$binary" | grep -q 'static' || { echo "✗ 二进制不是静态链接" >&2; exit 1; }

echo "==> 组装 $name"
rm -rf "$stage"; mkdir -p "$stage/bin" "$stage/docs"
install -m 755 "$binary" "$stage/bin/dotbot"
strip "$stage/bin/dotbot" 2>/dev/null || true
install -m 755 deploy/install.sh deploy/uninstall.sh deploy/preflight.sh "$stage/"
install -m 644 deploy/dotbot.service deploy/Caddyfile.example deploy/nginx.example.conf "$stage/"
install -m 644 deploy/INSTALL.md "$stage/README.md"
install -m 644 README.md "$stage/docs/README.md"
install -m 644 deploy/README.md "$stage/docs/deploy.zh-CN.md"
cp -r docs/. "$stage/docs/"
install -m 644 LICENSE NOTICE THIRD_PARTY.md "$stage/"
cp -R licenses "$stage/licenses"
echo "$version" > "$stage/VERSION"
"$stage/bin/dotbot" --version >/dev/null

tar -C "$out" -czf "$out/$name.tar.gz" "$name"

# Self-extracting installer: `bash dotbot-…run [install options]`.
{
    cat <<'HEADER'
#!/bin/bash
# dotbot self-extracting installer. Options are passed to install.sh (see --help).
set -e
dir=$(mktemp -d "${TMPDIR:-/tmp}/dotbot-install.XXXXXX")
trap 'rm -rf "$dir"' EXIT
line=$(awk '/^__DOTBOT_ARCHIVE__$/{print NR+1; exit}' "$0")
tail -n +"$line" "$0" | tar -xz -C "$dir"
bash "$dir"/dotbot-*/install.sh "$@"
exit $?
__DOTBOT_ARCHIVE__
HEADER
    cat "$out/$name.tar.gz"
} > "$out/$name.run"
chmod 755 "$out/$name.run"

(cd "$out" && sha256sum "$name.tar.gz" "$name.run" > "$name.sha256")
rm -rf "$stage"
echo "==> 完成"
ls -lh "$out/$name".*
