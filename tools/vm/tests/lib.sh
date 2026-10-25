#!/usr/bin/env bash
# Tiny assert helpers for the plain-Bash tests. Source this file.

TESTS_RUN=0
TESTS_FAILED=0

pass() { TESTS_RUN=$((TESTS_RUN + 1)); printf 'ok   %s\n' "$1"; }
fail() {
  TESTS_RUN=$((TESTS_RUN + 1))
  TESTS_FAILED=$((TESTS_FAILED + 1))
  printf 'FAIL %s\n' "$1"
  [ -n "${2:-}" ] && printf '     %s\n' "$2"
  return 0
}

# assert_eq NAME EXPECTED ACTUAL
assert_eq() {
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1" "expected '$2', got '$3'"; fi
}

# assert_contains NAME HAYSTACK NEEDLE (fixed string)
assert_contains() {
  case "$2" in
    *"$3"*) pass "$1" ;;
    *) fail "$1" "missing '$3' in: $2" ;;
  esac
}

# assert_not_contains NAME HAYSTACK NEEDLE (fixed string)
assert_not_contains() {
  case "$2" in
    *"$3"*) fail "$1" "unexpected '$3' in: $2" ;;
    *) pass "$1" ;;
  esac
}

# assert_status NAME EXPECTED_CODE COMMAND...  (runs the command, captures output in $OUT)
assert_status() {
  local name=$1 want=$2 got=0
  shift 2
  OUT=$("$@" 2>&1) || got=$?
  if [ "$got" = "$want" ]; then pass "$name"; else fail "$name" "exit $got, wanted $want: $OUT"; fi
}

# assert_gone NAME PID: the process no longer exists
assert_gone() {
  if kill -0 "$2" 2>/dev/null; then fail "$1" "pid $2 still alive"; else pass "$1"; fi
}

# summary: print totals, return non-zero on any failure
summary() {
  printf '\n%d tests, %d failed\n' "$TESTS_RUN" "$TESTS_FAILED"
  [ "$TESTS_FAILED" -eq 0 ]
}
