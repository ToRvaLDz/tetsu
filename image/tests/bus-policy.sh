#!/usr/bin/env bash
# Pin the bus access policy files: socket units, tmpfiles, polkit actions and
# rules. Runs image/stage-bus-policy into a temporary root and checks it.
# No root; the network only for cargo's cache.
# No `-e`: a failed check is counted, not fatal.
set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IDL="$REPO/crates/tetsu-bus/idl"
EXTRA="$REPO/image/mkosi.profiles/payload/mkosi.extra"

fails=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }
expect() { # DESCRIPTION CONDITION-EXIT-STATUS
  if [ "$2" -eq 0 ]; then pass "$1"; else fail "$1"; fi
}

# The value of KEY in a unit file (first match), empty when absent.
unit_value() { sed -n "s/^$2=//p" "$1" | head -n1; }

ROOT=$(mktemp -d)
trap 'rm -rf "$ROOT"' EXIT

"$REPO/image/stage-bus-policy" "$ROOT" >/dev/null
expect "image/stage-bus-policy runs" $?

# Staged files, all 0644.
for f in usr/lib/systemd/system/tetsu-tx.socket \
  usr/lib/systemd/system/tetsu-tx.service \
  usr/share/polkit-1/actions/org.tetsu.policy; do
  [ -f "$ROOT/$f" ] && [ "$(stat -c %a "$ROOT/$f" 2>/dev/null)" = 644 ]
  expect "$f is staged 0644" $?
done

# Every socket unit of the crates follows the registry rules.
shopt -s nullglob
sockets=("$REPO"/crates/*/systemd/*.socket "$REPO"/crates/*/dist/*.socket)
[ "${#sockets[@]}" -gt 0 ]; expect "the crates ship a socket unit" $?
for u in "${sockets[@]}"; do
  rel=${u#"$REPO"/}
  listen=$(unit_value "$u" ListenStream)
  base=${listen##*/}
  dir=${listen%/*}
  [ -n "$base" ] && [ -f "$IDL/$base.varlink" ]; expect "$rel: $base is an interface of the IDL" $?
  [ "$(unit_value "$u" FileDescriptorName)" = "$base" ]; expect "$rel: FileDescriptorName is $base" $?
  [ "$(unit_value "$u" Symlinks)" = "$dir/registry/$base" ]; expect "$rel: Symlinks is the registry entry" $?
  [ "$(unit_value "$u" RemoveOnStop)" = yes ]; expect "$rel: RemoveOnStop=yes" $?
  case "$dir" in
    /run/tetsu) want_sock=0666 want_dir=0755 ;;
    %t/tetsu) want_sock=0600 want_dir=0700 ;;
    *) want_sock=none want_dir=none ;;
  esac
  [ "$want_sock" != none ] && [ "$(unit_value "$u" SocketMode)" = "$want_sock" ] \
    && [ "$(unit_value "$u" DirectoryMode)" = "$want_dir" ]
  expect "$rel: socket and directory modes fit $dir" $?
done

# The service and the preset.
svc="$ROOT/usr/lib/systemd/system/tetsu-tx.service"
[ "$(unit_value "$svc" ExecStart)" = "/usr/bin/tetsu-tx serve" ]; expect "tetsu-tx.service ExecStart" $?
! grep -q '^\[Install\]' "$svc"; expect "tetsu-tx.service has no [Install]" $?
grep -qx 'enable tetsu-tx.socket' "$EXTRA/usr/lib/systemd/system-preset/50-tetsu.preset"
expect "50-tetsu.preset enables tetsu-tx.socket" $?

# tmpfiles.
tf() { # FILE EXPECTED-CONTENT
  [ "$(grep -v '^#' "$1" 2>/dev/null | sed '/^$/d')" = "$2" ]
}
tf "$EXTRA/usr/lib/tmpfiles.d/tetsu.conf" 'd /run/tetsu 0755 root root -
d /run/tetsu/registry 0755 root root -'
expect "system tmpfiles lines" $?
tf "$EXTRA/usr/share/user-tmpfiles.d/tetsu.conf" 'd %t/tetsu 0700 - - -
d %t/tetsu/registry 0700 - - -'
expect "user tmpfiles lines" $?

# polkit: one action per capability class of the IDL.
policy="$ROOT/usr/share/polkit-1/actions/org.tetsu.policy"
want=$(grep -h '^# @capability ' "$IDL"/*.varlink | awk '{print $3}' | sort -u | sed 's/_/-/g; s/^/org.tetsu./')
got=$(grep -o '<action id="[^"]*"' "$policy" 2>/dev/null | sed 's/<action id="//; s/"$//' | sort)
[ -n "$want" ] && [ "$got" = "$want" ]; expect "policy actions equal the IDL capability classes" $?
command -v xmllint >/dev/null; expect "xmllint is installed (libxml2)" $?
xmllint --noout "$policy" 2>/dev/null; expect "the policy is well-formed XML" $?

rules="$EXTRA/usr/share/polkit-1/rules.d/50-tetsu.rules"
[ -f "$rules" ] && grep -q 'org\.tetsu\.' "$rules" && grep -q wheel "$rules" \
  && grep -q 'polkit.Result.YES' "$rules" && grep -q 'polkit.Result.AUTH_ADMIN_KEEP' "$rules"
expect "50-tetsu.rules handles org.tetsu.* for wheel" $?
if command -v node >/dev/null && [ -f "$rules" ]; then
  node --check "$rules" 2>/dev/null; expect "50-tetsu.rules is valid JavaScript" $?
fi

if [ "$fails" -ne 0 ]; then
  printf '%d check(s) failed\n' "$fails" >&2
  exit 1
fi
