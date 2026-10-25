#!/usr/bin/env bash
# `tetsu-vm wait-for`: poll the serial log for a regex.

# The serial log as plain text: ANSI escape sequences and CRs removed.
clean_serial() {
  # OSC (BEL or ST terminated), CSI, then two-character escapes; "#" is the
  # s### delimiter so that "/" and "|" stay literal. LC_ALL=C because range
  # expressions like [@-~] are locale-collated otherwise.
  LC_ALL=C sed -E $'s#\x1b\\][^\x07\x1b]*(\x07|\x1b\\\\)##g; s#\x1b\\[[0-9;?]*[ -/]*[@-~]##g; s#\x1b[@-Z\\\\^_0-9=>]##g' "$1" \
    | tr -d '\r'
}

# Fail fast on an invalid extended regex (grep exits 2 on a bad pattern).
validate_regex() {
  local rc=0
  LC_ALL=C grep -aEq -- "$1" </dev/null 2>/dev/null || rc=$?
  [ "$rc" -lt 2 ] || usage_die "wait-for: invalid extended regex '$1'"
}

wait_usage() {
  cat <<'USAGE'
Usage: tetsu-vm wait-for [--state DIR] [--timeout SECS] REGEX
  Extended regex matched against the serial log (ANSI escapes and CRs removed).
  Exit 0 on match, 1 on timeout (default 120 s), 2 if the VM died before a match.
USAGE
}

cmd_wait_for() {
  local st=$DEFAULT_STATE timeout=120 regex=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --state) [ $# -ge 2 ] || usage_die "--state needs a value"; st=$2; shift 2 ;;
      --timeout) [ $# -ge 2 ] || usage_die "--timeout needs a value"; timeout=$2; shift 2 ;;
      -h | --help) wait_usage; return 0 ;;
      -*) usage_die "wait-for: unknown option '$1'" ;;
      *) [ -z "$regex" ] || usage_die "wait-for: only one REGEX is accepted"; regex=$1; shift ;;
    esac
  done
  [ -n "$regex" ] || usage_die "wait-for: REGEX is required"
  case "$timeout" in '' | *[!0-9]*) usage_die "--timeout must be an integer" ;; esac
  st=$(readlink -m "$st")
  validate_regex "$regex"

  local deadline=$((SECONDS + timeout)) log="$st/serial.log" running text rc
  while :; do
    running=0
    qemu_pid "$st" >/dev/null && running=1
    # Capture first: grep -q exiting early would SIGPIPE the writer under pipefail.
    text=$([ -r "$log" ] && clean_serial "$log" || true)
    rc=0
    LC_ALL=C grep -aEq -- "$regex" <<<"$text" || rc=$?
    [ "$rc" -ne 0 ] || return 0
    [ "$rc" -lt 2 ] || die "wait-for: grep failed (status $rc) on regex '$regex'"
    if [ "$running" -eq 0 ]; then
      warn "wait-for: VM is not running and '$regex' never appeared"
      return 2
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      warn "wait-for: timeout after ${timeout}s waiting for '$regex'"
      return 1
    fi
    sleep 0.5
  done
}
