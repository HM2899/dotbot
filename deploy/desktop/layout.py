#!/usr/bin/python3
"""Hide container mounts without unmounting or removing any user files."""
import os
from pathlib import Path
from gi.repository import Gio, GLib


def main():
    proxy = Gio.DBusProxy.new_for_bus_sync(Gio.BusType.SESSION, 0, None, 'org.xfce.Xfconf', '/org/xfce/Xfconf', 'org.xfce.Xfconf', None)

    def read(channel, key, default):
        try:
            return proxy.call_sync('GetProperty', GLib.Variant('(ss)', (channel, key)), 0, 3000, None).unpack()[0]
        except GLib.Error:
            return default

    def write(channel, key, kind, value):
        if read(channel, key, None) != value:
            proxy.call_sync('SetProperty', GLib.Variant('(ssv)', (channel, key, GLib.Variant(kind, value))), 0, 3000, None)

    legacy = Path('/config/legacy')
    hidden = set(read('thunar', '/hidden-devices', []) or [])
    if legacy.is_dir():
        hidden.update(p.name for p in legacy.iterdir() if p.is_dir())
    monitor = Gio.VolumeMonitor.get()
    for mount in monitor.get_mounts():
        path = mount.get_root().get_path()
        if path and (path == '/' or path.startswith(('/config/legacy/', '/run/', '/etc/', '/opt/', '/custom-cont-init.d'))):
            hidden.add(mount.get_uuid() or mount.get_name())
    if hidden:
        write('thunar', '/hidden-devices', 'as', sorted(hidden))
    write('thunar', '/hidden-bookmarks', 'as', sorted(set(read('thunar', '/hidden-bookmarks', []) or []) | {'file:///'}))
    for key in ('show-device-volumes', 'show-removable', 'show-filesystem'):
        write('xfce4-desktop', '/desktop-icons/file-icons/' + key, 'b', False)
    config = Path(os.environ.get('XDG_CONFIG_HOME', '/config/.config'))
    bookmarks = config / 'gtk-3.0/bookmarks'
    wanted = ['file:///config 我的文件', 'file:///config/workspace 工作区']
    existing = bookmarks.read_text().splitlines() if bookmarks.exists() else []
    # Old per-Bot homes are no longer mounted; drop the bookmark earlier versions added.
    uris = {line.split(' ', 1)[0] for line in wanted} | {'file:///config/legacy'}
    result = wanted + [line for line in existing if line and line.split(' ', 1)[0] not in uris]
    contents = '\n'.join(result) + '\n'
    if not bookmarks.exists() or bookmarks.read_text() != contents:
        bookmarks.parent.mkdir(parents=True, exist_ok=True)
        temporary = bookmarks.with_suffix('.dotbot.tmp')
        temporary.write_text(contents)
        temporary.replace(bookmarks)


if __name__ == '__main__':
    main()
