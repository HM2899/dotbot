"""Discover as the desktop user; signal only verified command groups as container root."""
import json
import os
import re
import signal
import sys
import time
from pathlib import Path

identifier = sys.argv[1]
if not re.fullmatch(r'[a-f0-9-]{36}', identifier):
    raise SystemExit(2)
marker = ('TEAMBOT_COMMAND_ID=' + identifier).encode()


def processes():
    for path in Path('/proc').iterdir():
        if not path.name.isdigit():
            continue
        try:
            pid = int(path.name)
            stat = (path / 'stat').read_text().rsplit(') ', 1)[1].split()
            yield pid, os.getpgid(pid), stat[19], stat[0], path
        except (OSError, ValueError, IndexError):
            pass


if sys.argv[2] == '--discover':
    # Run through docker exec -u abc: a root process with a temporary UID change
    # does not have the same /proc access as the managed desktop user.
    root = Path('/run/teambot/cancelled')
    root.mkdir(mode=0o700, exist_ok=True)
    (root / identifier).touch(mode=0o600)
    for file in root.iterdir():
        if re.fullmatch(r'[a-f0-9-]{36}', file.name) and time.time() - file.stat().st_mtime > 86400:
            file.unlink(missing_ok=True)
    found = []
    for pid, group, started, _, path in processes():
        try:
            args = (path / 'cmdline').read_bytes().split(b'\0')
            if pid == group and args[0].rsplit(b'/', 1)[-1] == b'timeout' and path.stat().st_uid == os.geteuid():
                if marker in (path / 'environ').read_bytes().split(b'\0'):
                    found.append([pid, started])
        except (FileNotFoundError, ProcessLookupError, IndexError):
            pass
    print(json.dumps(found))
    raise SystemExit(0)

# No command text or credentials are passed to the privileged half. PID start
# times prevent a stale discovery from signalling a reused process group.
found = json.loads(sys.argv[2])
if not isinstance(found, list) or len(found) > 32:
    raise SystemExit(2)
found = {tuple(pair) for pair in found if isinstance(pair, list) and len(pair) == 2 and isinstance(pair[0], int) and pair[0] > 1 and isinstance(pair[1], str) and pair[1].isdigit()}
groups = set()
for pid, group, started, _, path in processes():
    if pid == group and (pid, started) in found:
        try:
            if (path / 'cmdline').read_bytes().split(b'\0')[0].rsplit(b'/', 1)[-1] == b'timeout':
                groups.add(group)
        except FileNotFoundError:
            pass
# Include elevated package-manager children without touching detached app groups.
members = {(pid, started) for pid, group, started, _, _ in processes() if group in groups}
for group in groups:
    try:
        os.killpg(group, signal.SIGTERM)
    except ProcessLookupError:
        pass
if groups:
    time.sleep(.2)
    surviving = {group for pid, group, started, state, _ in processes() if group in groups and state != 'Z' and (pid, started) in members}
    for group in surviving:
        try:
            os.killpg(group, signal.SIGKILL)
        except ProcessLookupError:
            pass
