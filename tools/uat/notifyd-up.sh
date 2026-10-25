#!/bin/sh
# Starts tetsu-notifyd for a UAT next to the session's own notification
# server: a private D-Bus daemon carries org.freedesktop.Notifications, the
# Varlink socket and the surfaces are the real ones. The bus address is
# written to $XDG_RUNTIME_DIR/tetsu-uat/bus for notify-send:
#   DBUS_SESSION_BUS_ADDRESS="$(cat "$XDG_RUNTIME_DIR/tetsu-uat/bus")" notify-send ...
# Stop it with tools/uat/notifyd-down.sh.
set -eu
state="${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR is not set}/tetsu-uat"
mkdir -p "$state"
cargo build -p tetsu-notifyd
dbus-daemon --session --fork --print-address=1 --print-pid=1 > "$state/dbus"
head -n 1 "$state/dbus" > "$state/bus"
sed -n 2p "$state/dbus" > "$state/dbus.pid"
DBUS_SESSION_BUS_ADDRESS="$(cat "$state/bus")" \
    nohup target/debug/tetsu-notifyd > "$state/notifyd.log" 2>&1 &
echo $! > "$state/notifyd.pid"
