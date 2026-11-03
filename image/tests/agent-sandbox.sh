#!/usr/bin/env bash
# Pin the sandbox of the agent unit (#82): what image/stage-session installs,
# the keys that keep the agent off the secret stores, the system and session
# buses and the cgroup tree, and the user tmpfiles that make the denied
# directories stable. No root, no network; the behaviour is proved in a VM by
# tools/vm/tests/agent.sh.
# No `-e`: a failed check is counted, not fatal.
set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE="$REPO/image"
EXTRA="$IMAGE/mkosi.profiles/payload/mkosi.extra"
STAGE="$IMAGE/stage-session"
TMPFILES="$EXTRA/usr/share/user-tmpfiles.d/tetsu.conf"

fails=0
pass() { printf 'ok   %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }
expect() { # DESCRIPTION CONDITION-EXIT-STATUS
  if [ "$2" -eq 0 ]; then pass "$1"; else fail "$1"; fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# unit_get FILE SECTION KEY: every value of KEY in SECTION, one per line,
# whitespace-separated values split (multi-valued keys accumulate).
unit_get() {
  awk -v sec="[$2]" -v key="$3" '
    /^\[/ { cur = $0; next }
    cur == sec && index($0, key "=") == 1 {
      v = substr($0, length(key) + 2)
      n = split(v, a, /[[:space:]]+/)
      for (i = 1; i <= n; i++) if (a[i] != "") print a[i]
    }
  ' "$1" 2>/dev/null
}
has() { # FILE SECTION KEY VALUE
  unit_get "$1" "$2" "$3" | grep -qxF -- "$4"
}
mode_of() { stat -c %a "$1" 2>/dev/null; }

# ----------------------------------------------------------------- staging
crates=" $("$STAGE" --crates 2>/dev/null | tr -s '[:space:]' ' ') "
case "$crates" in *" tetsu-agent "*) r=0 ;; *) r=1 ;; esac
expect "stage-session --crates lists tetsu-agent" $r

root="$tmp/root"
rel="$tmp/release"
mkdir -p "$root" "$rel"
for c in $("$STAGE" --crates 2>/dev/null); do
  printf '#!/bin/sh\n' >"$rel/$c"
  chmod 0755 "$rel/$c"
done
"$STAGE" "$root" "$rel" >/dev/null 2>&1; expect "stage-session ROOT RELEASE_DIR succeeds" $?

U="$root/usr/lib/systemd/user"
svc="$U/tetsu-agent.service"
slice="$U/tetsu-agent.slice"
[ "$(mode_of "$root/usr/bin/tetsu-agent")" = 755 ]; expect "usr/bin/tetsu-agent is 0755" $?
[ "$(mode_of "$svc")" = 644 ]; expect "tetsu-agent.service is 0644" $?
[ "$(mode_of "$slice")" = 644 ]; expect "tetsu-agent.slice is 0644" $?

# ------------------------------------------------------------------- unit
one() { # KEY VALUE: the key is set once, to exactly VALUE
  [ "$(unit_get "$svc" Service "$1" | tr '\n' ' ')" = "$2 " ]
  expect "agent: $1=$2" $?
}
[ "$(unit_get "$svc" Service Slice)" = tetsu-agent.slice ]; expect "agent: Slice=tetsu-agent.slice" $?
one Type exec
one ExecStart /usr/bin/tetsu-agent
! grep -q '^\[Install\]' "$svc" 2>/dev/null; expect "agent: no [Install] (activation is #79/#80)" $?
has "$svc" Unit PartOf graphical-session.target; expect "agent: PartOf graphical-session.target" $?
has "$svc" Unit After tetsu-capd.service; expect "agent: After tetsu-capd.service" $?
has "$svc" Unit After tetsu-notifyd.service; expect "agent: After tetsu-notifyd.service" $?
[ -f "$slice" ] && ! grep -q '^\[Install\]' "$slice"; expect "agent slice: a plain slice" $?
grep -q 'Peer::resolve' "$slice" 2>/dev/null; expect "agent slice: the comment names Peer::resolve (must not be renamed)" $?

one ProtectSystem strict
one ProtectHome read-only
one PrivateTmp yes
one PrivateDevices yes
one StateDirectory tetsu-agent
one CacheDirectory tetsu-agent
has "$svc" Service ReadOnlyPaths %t/tetsu; expect "agent: ReadOnlyPaths has %t/tetsu" $?
has "$svc" Service ReadOnlyPaths /sys/fs/cgroup; expect "agent: ReadOnlyPaths has /sys/fs/cgroup" $?
! grep -qE '^(ReadWritePaths|ReadWriteDirectories)=' "$svc" 2>/dev/null
expect "agent: no ReadWritePaths= (CLI backend dirs are #79's)" $?

# The runtime dirs by allowlist: nothing but the listed binds survives.
has "$svc" Service TemporaryFileSystem '%t:ro'; expect "agent: TemporaryFileSystem=%t:ro" $?
has "$svc" Service TemporaryFileSystem /run/systemd:ro; expect "agent: TemporaryFileSystem=/run/systemd:ro" $?
[ "$(unit_get "$svc" Service TemporaryFileSystem | wc -l)" -eq 2 ]
expect "agent: no other TemporaryFileSystem=" $?
binds=$(unit_get "$svc" Service BindReadOnlyPaths | LC_ALL=C sort | tr '\n' ' ')
[ "$binds" = "%t/tetsu /run/systemd/journal /run/systemd/resolve " ]
expect "agent: BindReadOnlyPaths= is exactly %t/tetsu and the resolve and journal sockets" $?
! grep -qE '^(BindPaths|Bind)=' "$svc" 2>/dev/null; expect "agent: no writable bind" $?
has "$svc" Service InaccessiblePaths /run/dbus; expect "agent: InaccessiblePaths has /run/dbus (the system bus)" $?

# Home secret stores: fixed directories made by user tmpfiles, listed without
# a "-" so a missing one fails the start closed.
inacc=$(unit_get "$svc" Service InaccessiblePaths)
[ -n "$inacc" ]; expect "agent: InaccessiblePaths= is set" $?
for want in %E/credstore %E/credstore.encrypted %h/.local/lib/credstore \
  %h/.local/lib/credstore.encrypted %h/.local/share/tetsu %h/.local/share/keyrings; do
  grep -qxF -- "$want" <<<"$inacc"; expect "agent: InaccessiblePaths has $want" $?
done
while IFS= read -r p; do
  [ -n "$p" ] || continue
  case "$p" in -*) r=1 ;; *) r=0 ;; esac
  expect "agent: InaccessiblePaths entry $p has no '-' prefix" $r
  case "$p" in
    /*) r=0 ;; # a fixed system path
    *)
      # tmpfiles has no %E: the config dir is %h/.config there.
      t=${p/#%E/%h/.config}
      awk -v p="$t" '$1 == "d" && $2 == p && $3 == "0700" { ok = 1 } END { exit !ok }' "$TMPFILES" 2>/dev/null
      r=$?
      ;;
  esac
  expect "agent: $p is a fixed system path or made 0700 by user tmpfiles" $r
done <<<"$inacc"

# Keeping the mounts in force.
[ "$(grep -c '^CapabilityBoundingSet=$' "$svc" 2>/dev/null)" -eq 1 ]; expect "agent: CapabilityBoundingSet= is empty" $?
has "$svc" Service SystemCallFilter '~@mount'; expect "agent: SystemCallFilter=~@mount" $?
has "$svc" Service SystemCallFilter '~clone3:ENOSYS'; expect "agent: SystemCallFilter=~clone3:ENOSYS (CLONE_INTO_CGROUP)" $?
one RestrictNamespaces yes
one NoNewPrivileges yes
one RestrictSUIDSGID yes
one LockPersonality yes
one RestrictRealtime yes
one SystemCallArchitectures native
one UMask 0077
one LimitCORE 0
one ProtectControlGroups yes
one RestrictAddressFamilies 'AF_UNIX AF_INET AF_INET6 AF_NETLINK'
one UnsetEnvironment DBUS_SESSION_BUS_ADDRESS
! grep -qE '^(MemoryDenyWriteExecute|PrivatePIDs)=' "$svc" 2>/dev/null
expect "agent: no MemoryDenyWriteExecute= (JIT) and no PrivatePIDs= (option c, #144)" $?
grep -q '#144' "$svc" 2>/dev/null; expect "agent: the unit comment records the /proc gap (#144)" $?
! grep -qE '^(Environment|PassEnvironment)=.*DBUS' "$svc" 2>/dev/null
expect "agent: no session bus address is given" $?

# ---------------------------------------------------------------- presets
leaks=$(grep -rl 'tetsu-agent' "$EXTRA/usr/lib/systemd" 2>/dev/null | grep -c preset)
[ "$leaks" -eq 0 ]; expect "no preset names tetsu-agent" $?

# --------------------------------------------------------- systemd-analyze
if command -v systemd-analyze >/dev/null 2>&1 && [ -f "$svc" ]; then
  out=$(LC_ALL=C systemd-analyze --user verify --man=no "$svc" 2>&1)
  bad=$(grep -vE '^tetsu-agent\.service: Command /usr/bin/tetsu-agent is not executable' <<<"$out" | sed '/^$/d')
  [ -z "$bad" ]; expect "systemd-analyze verify: only the missing-binary line" $?
  [ -n "$bad" ] && printf '%s\n' "$bad" >&2
else
  printf 'skip systemd-analyze verify\n'
fi

if [ "$fails" -ne 0 ]; then
  printf '%d check(s) failed\n' "$fails" >&2
  exit 1
fi
