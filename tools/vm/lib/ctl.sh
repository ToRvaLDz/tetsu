#!/usr/bin/env bash
# `tetsu-vm` power-cut, stop, status, console.

# Parse "--state DIR" (and optional extra value flags) into CTL_STATE.
CTL_STATE=""
CTL_TIMEOUT=10
parse_ctl_args() {
  CTL_STATE=$DEFAULT_STATE
  while [ $# -gt 0 ]; do
    case "$1" in
      --state) [ $# -ge 2 ] || usage_die "--state needs a value"; CTL_STATE=$2; shift 2 ;;
      --timeout) [ $# -ge 2 ] || usage_die "--timeout needs a value"; CTL_TIMEOUT=$2; shift 2 ;;
      -h | --help) printf 'Usage: tetsu-vm <power-cut|stop|status|console> [--state DIR] [--timeout SECS]\n'; exit 0 ;;
      *) usage_die "unknown option '$1'" ;;
    esac
  done
  case "$CTL_TIMEOUT" in '' | *[!0-9]*) usage_die "--timeout must be an integer" ;; esac
  CTL_STATE=$(readlink -m "$CTL_STATE")
}

# SIGKILL QEMU; non-zero if it is still alive afterwards.
kill_qemu() {
  local pid
  pid=$(qemu_pid "$1") || return 0
  kill -KILL "$pid" 2>/dev/null || true
  wait_gone "$pid" 10 || { warn "QEMU (pid $pid) did not die after SIGKILL"; return 1; }
}

# power-cut: SIGKILL QEMU (no flush, no ACPI), then stop swtpm. Idempotent.
cmd_power_cut() {
  parse_ctl_args "$@"
  local rc=0
  kill_qemu "$CTL_STATE" || rc=1
  # Stop swtpm even when QEMU survived, then report the failure.
  term_swtpm "$CTL_STATE"
  [ "$rc" -eq 0 ] || die "power-cut incomplete: QEMU is still running in $CTL_STATE"
  printf 'power cut: %s\n' "$CTL_STATE"
}

# QMP handshake + quit over the monitor socket.
qmp_quit() {
  local sock=$1
  [ -S "$sock" ] || return 1
  command -v socat >/dev/null 2>&1 || return 1
  { printf '{"execute":"qmp_capabilities"}\n'; sleep 0.5; printf '{"execute":"quit"}\n'; sleep 0.5; } \
    | socat -T2 - "UNIX-CONNECT:$sock" >/dev/null 2>&1 || true
}

# stop: graceful QMP quit, SIGKILL after the timeout, then swtpm.
cmd_stop() {
  parse_ctl_args "$@"
  local pid
  if pid=$(qemu_pid "$CTL_STATE"); then
    qmp_quit "$CTL_STATE/qmp.sock" || true
    if ! wait_gone "$pid" "$CTL_TIMEOUT"; then
      warn "QEMU ignored QMP quit for ${CTL_TIMEOUT}s, sending SIGKILL"
      kill_qemu "$CTL_STATE"
    fi
  fi
  term_swtpm "$CTL_STATE"
  printf 'stopped: %s\n' "$CTL_STATE"
}

# status: exit 0 when QEMU runs, 1 otherwise.
cmd_status() {
  parse_ctl_args "$@"
  local pid
  if pid=$(qemu_pid "$CTL_STATE"); then
    printf 'running (qemu pid %s, state %s)\n' "$pid" "$CTL_STATE"
    return 0
  fi
  if pid=$(swtpm_pid "$CTL_STATE"); then
    printf 'stopped (stale swtpm pid %s, state %s)\n' "$pid" "$CTL_STATE"
    return 1
  fi
  printf 'stopped (state %s)\n' "$CTL_STATE"
  return 1
}

# console: interactive serial console. Ctrl-] detaches.
cmd_console() {
  parse_ctl_args "$@"
  need_cmd socat
  [ -S "$CTL_STATE/serial.sock" ] || die "no serial socket in $CTL_STATE (is the VM running?)"
  printf 'Attached to the serial console, Ctrl-] to detach.\n' >&2
  socat -,raw,echo=0,escape=0x1d "UNIX-CONNECT:$CTL_STATE/serial.sock"
}

# send [--state DIR] TEXT: write TEXT to the serial console as if typed. Backslash
# escapes (\n, \r, \t, \xHH, ...) are interpreted with printf %b.
cmd_send() {
  local st=$DEFAULT_STATE text="" have_text=0
  while [ $# -gt 0 ]; do
    case "$1" in
      --state) [ $# -ge 2 ] || usage_die "--state needs a value"; st=$2; shift 2 ;;
      -h | --help) printf 'Usage: tetsu-vm send [--state DIR] TEXT\n'; return 0 ;;
      *) [ "$have_text" -eq 0 ] || usage_die "send: only one TEXT is accepted"
         text=$1; have_text=1; shift ;;
    esac
  done
  [ "$have_text" -eq 1 ] || usage_die "send: TEXT is required"
  st=$(readlink -m "$st")
  need_cmd socat
  [ -S "$st/serial.sock" ] || die "no serial socket in $st (is the VM running?)"
  # The short sleep keeps the connection open until the bytes are delivered.
  { printf '%b' "$text"; sleep 0.3; } | socat -u - "UNIX-CONNECT:$st/serial.sock" \
    || die "could not write to the serial socket in $st"
}
