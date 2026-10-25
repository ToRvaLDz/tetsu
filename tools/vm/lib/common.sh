#!/usr/bin/env bash
# Shared helpers for tetsu-vm. Sourced, never executed.

REPO_ROOT=$(cd "$VM_DIR/../.." && pwd)
DEFAULT_STATE="$REPO_ROOT/target/vm/default"

die() { printf 'tetsu-vm: error: %s\n' "$*" >&2; exit 1; }
# Usage errors (bad flags, missing arguments) exit 64, like EX_USAGE.
usage_die() { printf 'tetsu-vm: usage error: %s\n' "$*" >&2; exit 64; }
warn() { printf 'tetsu-vm: %s\n' "$*" >&2; }

need_cmd() { command -v "$1" >/dev/null 2>&1 || die "'$1' not found in PATH (see tools/vm/README.md)"; }

# Expected process names, overridable so tests can use dummy processes.
qemu_comm() { printf '%s' "${TETSU_VM_QEMU_COMM:-qemu-system-*}"; }
swtpm_comm() { printf '%s' "${TETSU_VM_SWTPM_COMM:-swtpm}"; }

# pid_of PIDFILE COMM_GLOB: print the pid when the file names a live process
# whose command name matches. A stale pidfile (dead pid, or a pid reused by an
# unrelated process) yields a non-zero status and no output.
pid_of() {
  local pidfile=$1 glob=$2 pid comm
  [ -r "$pidfile" ] || return 1
  pid=$(tr -d '[:space:]' <"$pidfile")
  case "$pid" in '' | *[!0-9]*) return 1 ;; esac
  [ -r "/proc/$pid/comm" ] || return 1
  comm=$(<"/proc/$pid/comm")
  # shellcheck disable=SC2254
  case "$comm" in $glob) printf '%s' "$pid" ;; *) return 1 ;; esac
}

qemu_pid() { pid_of "$1/qemu.pid" "$(qemu_comm)"; }
swtpm_pid() { pid_of "$1/swtpm.pid" "$(swtpm_comm)"; }

# Wait up to $2 seconds (default 10) for a pid to disappear.
wait_gone() {
  local pid=$1 tries=$(( ${2:-10} * 10 ))
  while kill -0 "$pid" 2>/dev/null && [ "$tries" -gt 0 ]; do
    sleep 0.1
    tries=$((tries - 1))
  done
  ! kill -0 "$pid" 2>/dev/null
}
