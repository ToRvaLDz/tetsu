#!/bin/sh
# Stops what tools/uat/notifyd-up.sh started.
set -u
state="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}/tetsu-uat"
for pid in "$state/notifyd.pid" "$state/dbus.pid"; do
    [ -f "$pid" ] && kill "$(cat "$pid")" 2>/dev/null
done
rm -rf "$state"
