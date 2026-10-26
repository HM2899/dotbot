#!/bin/bash
# The team's one browser: a single profile and process, shown on the screen of
# the Bot (or user) that opens it. Opening it from another screen moves it
# there: the old window closes normally (Chromium saves its session) and the
# same profile reopens here with its tabs, so every Bot shares the same logins.
set -eu
umask 077
: "${TEAMBOT_BROWSER_HOME:?No Bot screen is assigned}"
profile=$TEAMBOT_BROWSER_HOME
if [ ! -d "$profile" ]; then
    # Earlier versions kept one profile per screen. Start from the one used
    # last, so its logins carry over; the old profiles stay where they are.
    latest=$(ls -td /config/.teambot/s*/browser/Default 2>/dev/null | head -n1 || true)
    if [ -n "$latest" ]; then
        tmp="$profile.tmp.$$"
        cp -a "$(dirname "$latest")" "$tmp"
        rm -f "$tmp"/Singleton* "$tmp/DevToolsActivePort"
        mv "$tmp" "$profile" 2>/dev/null || rm -rf "$tmp"
    fi
    mkdir -p "$profile"
fi
# The browser running on another screen is closed, so this screen gets the window.
moved=
for pid in $(pgrep -u "$(id -u)" -f -- "--user-data-dir=$profile" || true); do
    tr '\0' '\n' <"/proc/$pid/cmdline" 2>/dev/null | grep -q '^--type=' && continue
    tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -Fxq "DISPLAY=${DISPLAY:-}" && continue
    kill -TERM "$pid" 2>/dev/null && moved="$moved $pid"
done
for pid in $moved; do
    for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep .1; done
    kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
done
if [ -n "$moved" ]; then
    rm -f "$profile"/Singleton* "$profile/DevToolsActivePort"
    set -- --restore-last-session "$@"
fi
for browser in chromium chromium-browser google-chrome; do
    if command -v "$browser" >/dev/null 2>&1; then
        # Already running on this screen: Chromium opens the address as a new tab.
        exec "$browser" --user-data-dir="$profile" --no-first-run --no-default-browser-check "$@"
    fi
done
printf '%s\n' 'The team browser needs Chromium in the team computer.' >&2
exit 127
