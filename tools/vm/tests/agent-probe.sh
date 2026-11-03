#!/usr/bin/env bash
# The probe of the agent sandbox (#82), run by tools/vm/tests/agent.sh twice:
# `in` as the ExecStart= of tetsu-agent.service (a runtime drop-in swaps only
# that key, so it runs under the exact shipped sandbox) and `out` as a plain
# user service, the positive control. Each probe is an operation, not an
# existence test, and prints one `P:<name>=<value>` line.
#
#   agent-probe.sh in|out ENV-FILE
#
# ENV-FILE sets SIB (a cgroup outside the agent slice, in the user's delegated
# subtree), GO (the file that releases the `in` probe once the late socket
# exists), CLONE3 (a binary that returns the errno of clone3(NULL, 0)) and
# CAPD_PID.
set -u

mode=$1
# shellcheck disable=SC1090
. "$2"
rt=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}

say() { printf 'P:%s=%s\n' "$1" "$2"; }

# try SCRIPT [ARG...]: yes when the script succeeds, else no (bounded).
try() {
  if timeout 20 bash -c "$1" probe "${@:2}" >/dev/null 2>&1; then echo yes; else echo no; fi
}

# sockconn PATH: yes when a connection is made, whatever the peer then
# answers; no when the connect itself fails (no socket, no access).
sockconn() {
  local e
  e=$(LC_ALL=C timeout 8 varlinkctl info "unix:$1" 2>&1 >/dev/null)
  case $e in *"Failed to connect"*) echo no ;; *) echo yes ;; esac
}

# The real binary, through the read-only %t/tetsu: both daemons must be
# reached and verified (a pinned interface that fails its check is left out
# of the tools and named in "skipped").
j=$(timeout 30 /usr/bin/tetsu-agent tools --json 2>&1)
say tools_rc $?
say tools_cap "$(grep -q '"id":"org\.tetsu\.cap\.v1\.' <<<"$j" && echo yes || echo no)"
say tools_notify "$(grep -q '"id":"org\.tetsu\.notify\.v1\.' <<<"$j" && echo yes || echo no)"
say tools_left_out "$(grep -q '"subject":"org\.tetsu\.\(cap\|notify\)\.v1 at' <<<"$j" && echo yes || echo no)"

# Writes: the agent's own state dir is writable, nothing else of the session is.
say w_statedir "$(try ': >"$1/vm-probe-w" && rm -f "$1/vm-probe-w"' "${STATE_DIRECTORY:-$HOME/.local/state/tetsu-agent}")"
say w_config "$(try ': >"$1/vm-probe-w" && rm -f "$1/vm-probe-w"' "$HOME/.config/tetsu")"
say w_state "$(try ': >"$1/vm-probe-w" && rm -f "$1/vm-probe-w"' "$HOME/.local/state/tetsu")"
say w_runtime "$(try ': >"$1/vm-probe-w" && rm -f "$1/vm-probe-w"' "$rt/tetsu")"
say r_config "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.config/tetsu/vm-marker")"
say r_state "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.local/state/tetsu/vm-marker")"
say r_runtime "$(try 'ls "$1" >/dev/null' "$rt/tetsu")"

# Reads of the secret stores.
say r_cred_cfg "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.config/credstore/vm-marker")"
say r_cred_cfg_enc "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.config/credstore.encrypted/vm-marker")"
say r_cred_lib "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.local/lib/credstore/vm-marker")"
say r_cred_lib_enc "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.local/lib/credstore.encrypted/vm-marker")"
say r_vault "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.local/share/tetsu/vm-marker")"
say r_keyring "$(try '[ "$(cat "$1")" = vm-marker ]' "$HOME/.local/share/keyrings/vm-marker")"

# Connections to the buses, the managers and the credential store.
say c_userbus "$(try 'busctl --user --no-pager status')"
say c_userbus_path "$(sockconn "$rt/bus")"
say c_sysbus "$(try 'busctl --system --no-pager status')"
say c_sysbus_path "$(sockconn /run/dbus/system_bus_socket)"
say c_usermgr "$(sockconn "$rt/systemd/private")"
say c_cred "$(sockconn /run/systemd/io.systemd.Credentials)"
say c_mgr "$(sockconn /run/systemd/io.systemd.Manager)"
say c_oo7_pam "$(for s in "$rt/oo7/pam.sock" "$rt/oo7-pam.sock"; do [ "$(sockconn "$s")" = yes ] && { echo yes; exit; }; done; echo no)"
say systemctl_user "$(try 'systemctl --user show -P Version')"
say systemd_run_user "$(try 'systemd-run --user --wait --collect -q true')"
say dns "$(try 'resolvectl query --legend=no localhost')"
say tpm "$(try 'test -c /dev/tpmrm0')"
say dbus_env "$([ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] && echo unset || echo set)"
say abstract_bus "$(awk '$NF ~ /^@/ && $NF ~ /bus/ { f = 1 } END { print f ? "yes" : "no" }' /proc/net/unix)"

# The cgroup tree: the probe and a forked child sit in the agent slice, and
# neither the clone3 route nor a write to the sibling cgroup is left.
say cg_self "$(grep -q '/tetsu-agent\.slice/' /proc/self/cgroup && echo yes || echo no)"
say cg_child "$(sh -c 'cat /proc/self/cgroup' | grep -q '/tetsu-agent\.slice/' && echo yes || echo no)"
"$CLONE3" 2>/dev/null
rc=$?
say clone3 "$([ "$rc" -eq 38 ] && echo enosys || echo "errno-$rc")"

# The mount sandbox in force.
say userns "$([ "$(cat /proc/self/uid_map)" != "$(cat /proc/1/uid_map)" ] && echo yes || echo no)"
say m_mount "$(try 'mount -t tmpfs tmpfs /tmp')"
say m_umount "$(try 'umount /sys/fs/cgroup')"
say m_unshare "$(try 'unshare -Urm true')"
say m_unshare_mount "$(try 'unshare -Urm mount -t tmpfs tmpfs /mnt')"

# Known open item (#144): the shared PID namespace leaves /proc/<pid>/root of
# a same-user process reachable. Reported, never counted as a pass.
say proc_root "$(try 'ls "/proc/$1/root/" >/dev/null' "$CAPD_PID")"

# A socket made in the runtime dir after the unit started must not show up.
if [ "$mode" = in ]; then
  say late_ready 1
  for _ in $(seq 1 90); do [ -e "$GO" ] && break; sleep 1; done
fi
say late_visible "$(try 'test -S "$1"' "$rt/vm-late.sock")"
say c_late "$(sockconn "$rt/vm-late.sock")"

# Last: moving the probe into the sibling cgroup (it leaves the slice).
say cg_join "$(try 'echo "$2" >"$1/cgroup.procs"' "$SIB" "$$")"
say done 1
