#!/usr/bin/env bash
# `tetsu-vm wait-for`: poll the serial log for a regex.

# The serial log as plain text: ANSI escape sequences and CRs removed.
clean_serial() {
  sed -E $'s/\x1b\\[[0-9;?]*[ -\\/]*[@-~]//g; s/\x1b[@-Z\\\\-_]//g' "$1" | tr -d '\r'
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
      --state) [ $# -ge 2 ] || die "--state needs a value"; st=$2; shift 2 ;;
      --timeout) [ $# -ge 2 ] || die "--timeout needs a value"; timeout=$2; shift 2 ;;
      -h | --help) wait_usage; return 0 ;;
      -*) die "wait-for: unknown option '$1'" ;;
      *) [ -z "$regex" ] || die "wait-for: only one REGEX is accepted"; regex=$1; shift ;;
    esac
  done
  [ -n "$regex" ] || die "wait-for: REGEX is required"
  case "$timeout" in '' | *[!0-9]*) die "--timeout must be an integer" ;; esac
  st=$(readlink -m "$st")

  local deadline=$((SECONDS + timeout)) log="$st/serial.log" running
  while :; do
    running=0
    qemu_pid "$st" >/dev/null && running=1
    if [ -r "$log" ] && clean_serial "$log" | grep -Eq -- "$regex"; then
      return 0
    fi
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
