#!/bin/sh
# Stops what tools/uat/notifyd-up.sh started.
set -u
state="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}/tetsu-uat"
systemctl --user stop tetsu-notifyd 2>/dev/null
[ -f "$state/dbus.pid" ] && kill "$(cat "$state/dbus.pid")" 2>/dev/null
rm -rf "$state"
