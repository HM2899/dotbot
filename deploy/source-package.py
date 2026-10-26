#!/usr/bin/env python3
"""Create a reviewable source-only archive, independent of git's staging area."""
import hashlib
import io
import os
from pathlib import Path
import re
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FILES = {"Cargo.toml", "Cargo.lock", "README.md", "LICENSE", "NOTICE", "THIRD_PARTY.md",
         "Dockerfile", "compose.yaml", ".gitignore", ".dockerignore"}
DIRS = {"src", "web", "prompts", "crates", "deploy", "docs", "dev", "licenses", ".github"}
BLOCKED = {"target", "node_modules", "build", "dist", "test-results", ".git", ".dart_tool",
           ".gradle", ".cxx", "__pycache__", ".idea", ".aws", ".codex", ".agents",
           ".dotbot", ".teambot", ".pub-cache", "coverage", "backups"}
PRIVATE = {"secrets.json", "credentials.json", "providers.json", "key.properties", "local.properties"}
SUFFIXES = {".pem", ".key", ".p12", ".pfx", ".keystore", ".jks", ".db", ".sqlite", ".sqlite3",
            ".apk", ".aab", ".exe", ".so", ".dll", ".dylib", ".o", ".rlib", ".rmeta", ".run",
            ".log", ".tmp", ".pyc", ".zip", ".gz", ".xz", ".tar"}
SECRET = re.compile(rb"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----|"
                    rb"\bgh[pousr]_[A-Za-z0-9]{30,}\b|\bgithub_pat_[A-Za-z0-9_]{40,}\b|"
                    rb"\bAKIA[A-Z0-9]{16}\b|\bsk-(?:proj-|ant-)?[A-Za-z0-9_-]{32,}\b")


def excluded(path):
    return (any(p in BLOCKED for p in path.parts) or path.name in PRIVATE
            or path.suffix.lower() in SUFFIXES or path.name.startswith(".flutter-plugins")
            or (path.name.startswith(".env") and not path.name.endswith(".example"))
            or any(s in path.name for s in (".db-", ".sqlite-", ".sqlite3-")))


def main():
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    name = f"dotbot-{version}-source"
    selected = []
    for folder, dirs, files in os.walk(ROOT, followlinks=False):
        parent = Path(folder).relative_to(ROOT)
        dirs[:] = sorted(d for d in dirs if not excluded(parent / d)
                         and not (Path(folder) / d).is_symlink()
                         and (parent != Path(".") or d in DIRS))
        for filename in sorted(files):
            relative = parent / filename
            if parent == Path(".") and filename not in FILES:
                continue
            path = ROOT / relative
            if excluded(relative):
                continue
            if path.is_symlink():
                raise SystemExit(f"Refusing symbolic link: {relative}")
            content = path.read_bytes()
            if SECRET.search(content):
                raise SystemExit(f"Potential credential in {relative}; archive not created. Review locally.")
            selected.append((relative.as_posix(), content, path.stat().st_mode & 0o111 != 0))
    if not selected:
        raise SystemExit("No source files selected")
    manifest = "".join(f"{hashlib.sha256(data).hexdigest()}  {path}\n" for path, data, _ in selected)
    output = ROOT / "dist"
    output.mkdir(exist_ok=True)
    archive = output / f"{name}.tar.gz"
    temporary = output / f".{name}.tmp"
    try:
        with tarfile.open(temporary, "w:gz") as tar:
            for path, data, executable in selected + [("SOURCE-MANIFEST.sha256", manifest.encode(), False)]:
                info = tarfile.TarInfo(f"{name}/{path}")
                info.size, info.mode, info.mtime = len(data), 0o755 if executable else 0o644, 0
                tar.addfile(info, io.BytesIO(data))
        temporary.replace(archive)
    finally:
        temporary.unlink(missing_ok=True)
    (output / f"{name}.manifest.txt").write_text(manifest)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    (output / f"{name}.tar.gz.sha256").write_text(f"{digest}  {archive.name}\n")
    print(f"Created {archive.relative_to(ROOT)} ({len(selected)} source files)")
    print("Review dist/*.manifest.txt and the extracted archive before publishing.")


if __name__ == "__main__":
    main()
