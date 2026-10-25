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

# Terminate swtpm with SIGTERM. Its state in <state>/tpm is non-volatile and
# survives, as a TPM does on real hardware.
term_swtpm() {
  local pid
  pid=$(swtpm_pid "$1") || return 0
  kill "$pid" 2>/dev/null || true
  wait_gone "$pid" 5 || { kill -KILL "$pid" 2>/dev/null || true; wait_gone "$pid" 5 || true; }
}

kill_qemu() {
  local pid
  pid=$(qemu_pid "$1") || return 0
  kill -KILL "$pid" 2>/dev/null || true
  wait_gone "$pid" 10 || die "QEMU (pid $pid) did not die after SIGKILL"
}

# power-cut: SIGKILL QEMU (no flush, no ACPI), then stop swtpm. Idempotent.
cmd_power_cut() {
  parse_ctl_args "$@"
  kill_qemu "$CTL_STATE"
  term_swtpm "$CTL_STATE"
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
