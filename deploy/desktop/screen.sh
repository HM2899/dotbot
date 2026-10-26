#!/bin/bash
# One Bot's owned process group inside the shared computer.
set -eu
umask 077
exec 9>"$TEAMBOT_SCREEN_RUNTIME/lock"
flock -n 9 || exit 0
printf '%s\n' "$$" > "$TEAMBOT_SCREEN_RUNTIME/pid"
cleanup() {
    trap - EXIT TERM INT
    for record in "$TEAMBOT_SCREEN_RUNTIME"/apps/*; do
        [ -f "$record" ] || continue
        pid=$(cat "$record")
        case "$pid" in ''|*[!0-9]*) continue;; esac
        if tr '\0' '\n' <"/proc/$pid/environ" 2>/dev/null | grep -Fxq "TEAMBOT_SCREEN_RUNTIME=$TEAMBOT_SCREEN_RUNTIME"; then
            kill -TERM -- -"$pid" 2>/dev/null || true
        fi
    done
    rm -f "$TEAMBOT_SCREEN_RUNTIME/stream.sock" "$TEAMBOT_SCREEN_RUNTIME/bus"
    kill -- -"$$" 2>/dev/null || true
}
trap cleanup EXIT TERM INT
rm -rf "$TEAMBOT_SCREEN_RUNTIME/apps"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" /config/workspace /config/Desktop
chmod 700 "$XDG_RUNTIME_DIR"
if [ ! -d "$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml" ]; then
    mkdir -p "$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml"
    cp /defaults/xfce/* "$XDG_CONFIG_HOME/xfce4/xfconf/xfce-perchannel-xml/"
fi
# The desktop menu, XFCE browser shortcut and tool/button entry points all
# launch the managed profile, so a browser opened by clicking the desktop also
# participates in login synchronization.
mkdir -p "$XDG_DATA_HOME/applications" "$XDG_DATA_HOME/xfce4/helpers" "$XDG_CONFIG_HOME/xfce4"
cat >"$XDG_CONFIG_HOME/xfce4/helpers.rc" <<'HELPERS'
WebBrowser=teambot-browser
HELPERS
cat >"$XDG_DATA_HOME/xfce4/helpers/teambot-browser.desktop.$$" <<'HELPER'
[Desktop Entry]
Version=1.0
Type=X-XFCE-Helper
Name=团队浏览器
X-XFCE-Category=WebBrowser
X-XFCE-Commands=/bin/bash /opt/teambot-desktop/browser.sh
X-XFCE-CommandsWithParameter=/bin/bash /opt/teambot-desktop/browser.sh "%s"
HELPER
mv "$XDG_DATA_HOME/xfce4/helpers/teambot-browser.desktop.$$" "$XDG_DATA_HOME/xfce4/helpers/teambot-browser.desktop"
for desktop_id in teambot-browser chromium chromium-browser; do
    cat >"$XDG_DATA_HOME/applications/$desktop_id.desktop.$$" <<'BROWSER'
[Desktop Entry]
Version=1.0
Type=Application
Name=Chromium
Name[zh_CN]=网页浏览器
Exec=/bin/bash /opt/teambot-desktop/browser.sh %U
Icon=chromium
Terminal=false
Categories=Network;WebBrowser;
MimeType=text/html;x-scheme-handler/http;x-scheme-handler/https;
BROWSER
    mv "$XDG_DATA_HOME/applications/$desktop_id.desktop.$$" "$XDG_DATA_HOME/applications/$desktop_id.desktop"
done
xdg-settings set default-web-browser teambot-browser.desktop >/dev/null 2>&1 || true
Xvfb "$DISPLAY" -screen 0 1280x900x24 -dpi 96 -nolisten tcp -ac +extension RANDR +extension XTEST &
xpid=$!
for attempt in $(seq 1 100); do
    if xdpyinfo >/dev/null 2>&1; then break; fi
    kill -0 "$xpid"
    sleep .1
done
# A private session bus keeps single-instance apps on the correct screen.
dbus-run-session -- /bin/bash /opt/teambot-desktop/session.sh &
wait $!
