#!/bin/sh
# Exits 0 when the UAT notifyd answers on its Varlink socket.
varlinkctl call "unix:${XDG_RUNTIME_DIR:?}/tetsu/org.tetsu.notify.v1" \
    org.tetsu.notify.v1.GetPending '{}' > /dev/null 2>&1
