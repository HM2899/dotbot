#!/bin/bash
set -eu
umask 077
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" > "$TEAMBOT_SCREEN_RUNTIME/bus"
/usr/bin/python3 /opt/teambot-desktop/layout.py >"$TEAMBOT_SCREEN_RUNTIME/layout.log" 2>&1 || true
xfce4-session >"$TEAMBOT_SCREEN_RUNTIME/desktop.log" 2>&1 &
wm=$!
selkies --unix-socket "$TEAMBOT_SCREEN_RUNTIME/stream.sock" \
    --web-root /usr/share/selkies/web --audio-enabled false \
    --microphone-enabled false --webcam-enabled false --gamepad-enabled false \
    --printing-enabled false --command-enabled false --enable-sharing false \
    --js-socket-path "$XDG_RUNTIME_DIR" --webcam-socket-path "$XDG_RUNTIME_DIR" \
    --manual-width 1280 --manual-height 900 --manual-resolution true \
    --file-manager-path /config/workspace \
    >"$TEAMBOT_SCREEN_RUNTIME/stream.log" 2>&1 &
stream=$!
wait -n "$wm" "$stream"
