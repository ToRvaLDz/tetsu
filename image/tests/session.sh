#!/usr/bin/env bash
# Pin the session: the greetd config, the Tetsu user units and their order,
# the two launcher scripts (against stub systemctl/systemd-notify), what
# image/stage-session installs, and the base-set packages it needs.
# No root, no network.
# No `-e`: a failed check is counted, not fatal.
set -uo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
IMAGE="$REPO/image"
EXTRA="$IMAGE/mkosi.profiles/payload/mkosi.extra"
PKGBUILD="$IMAGE/tetsu-base/PKGBUILD"
STAGE="$IMAGE/stage-session"
CRATES=(tetsu-comp tetsu-bar tetsu-notifyd tetsu-capd tetsu-agent)

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

# ------------------------------------------------------------------ staging
crates=$("$STAGE" --crates 2>/dev/null | tr -s '[:space:]' ' ' | sed 's/ $//')
[ "$crates" = "${CRATES[*]}" ]; expect "stage-session --crates lists the five session crates" $?

root="$tmp/root"
rel="$tmp/release"
mkdir -p "$root" "$rel"
for c in "${CRATES[@]}"; do printf '#!/bin/sh\n' >"$rel/$c"; chmod 0755 "$rel/$c"; done
"$STAGE" "$root" "$rel" >/dev/null 2>&1; expect "stage-session ROOT RELEASE_DIR succeeds" $?

for c in "${CRATES[@]}"; do
  [ "$(mode_of "$root/usr/bin/$c")" = 755 ]; expect "usr/bin/$c is 0755" $?
  [ "$(mode_of "$root/usr/lib/systemd/user/$c.service")" = 644 ]; expect "$c.service is 0644" $?
done
[ "$(mode_of "$root/usr/bin/tetsu-session")" = 755 ]; expect "usr/bin/tetsu-session is 0755" $?
[ "$(mode_of "$root/usr/libexec/tetsu-comp-ready")" = 755 ]; expect "usr/libexec/tetsu-comp-ready is 0755" $?
[ -f "$root/usr/share/dbus-1/services/org.freedesktop.Notifications.service" ]
expect "the notifyd D-Bus activation file is staged" $?
[ -f "$root/usr/share/tetsu/capabilities.toml" ]; expect "capabilities.toml is staged" $?
[ -z "$(find "$root" -name 'tetsu-notifyd.socket' 2>/dev/null)" ]
expect "no tetsu-notifyd.socket is staged (the daemons bind their own sockets)" $?

# -------------------------------------------------------------- unit keys
U="$root/usr/lib/systemd/user"
comp="$U/tetsu-comp.service"
[ "$(unit_get "$comp" Service Type)" = notify ]; expect "comp: Type=notify" $?
[ "$(unit_get "$comp" Service NotifyAccess)" = all ]; expect "comp: NotifyAccess=all" $?
[ "$(grep -m1 '^ExecStart=' "$comp" 2>/dev/null)" = \
  'ExecStart=/usr/bin/tetsu-comp --backend tty -- /usr/libexec/tetsu-comp-ready' ]
expect "comp: ExecStart runs the tty backend with the readiness hook" $?
has "$comp" Unit BindsTo graphical-session.target; expect "comp: BindsTo graphical-session.target" $?
has "$comp" Unit Before graphical-session.target; expect "comp: Before graphical-session.target" $?
has "$comp" Unit Wants graphical-session-pre.target; expect "comp: Wants graphical-session-pre.target" $?
has "$comp" Unit After graphical-session-pre.target; expect "comp: After graphical-session-pre.target" $?
has "$comp" Service Environment XDG_SESSION_TYPE=wayland; expect "comp: XDG_SESSION_TYPE=wayland" $?
has "$comp" Service Environment LIBSEAT_BACKEND=logind; expect "comp: LIBSEAT_BACKEND=logind" $?
has "$comp" Service UnsetEnvironment WAYLAND_DISPLAY; expect "comp: UnsetEnvironment has WAYLAND_DISPLAY" $?
[ "$(unit_get "$comp" Service Slice)" = session.slice ]; expect "comp: Slice=session.slice" $?
! grep -q '^\[Install\]' "$comp" 2>/dev/null; expect "comp: no [Install]" $?

for d in tetsu-notifyd tetsu-capd tetsu-bar; do
  f="$U/$d.service"
  has "$f" Unit PartOf graphical-session.target; expect "$d: PartOf graphical-session.target" $?
  has "$f" Unit After graphical-session.target; expect "$d: After graphical-session.target" $?
  has "$f" Install WantedBy graphical-session.target; expect "$d: WantedBy graphical-session.target" $?
  [ "$(unit_get "$f" Service Slice)" = session.slice ]; expect "$d: Slice=session.slice" $?
done
has "$U/tetsu-capd.service" Unit Wants tetsu-notifyd.service; expect "capd: Wants tetsu-notifyd.service" $?
has "$U/tetsu-capd.service" Unit After tetsu-notifyd.service; expect "capd: After tetsu-notifyd.service" $?

# ------------------------------------------------------------------ order
# Edges "A<B" (A starts before B) from After= (B after A) and Before= (A
# before B); order holds when a path exists in the transitive closure.
edges=$(
  tgt=graphical-session.target
  for u in tetsu-comp tetsu-notifyd tetsu-capd tetsu-bar; do
    f="$U/$u.service"
    unit_get "$f" Unit After | while read -r a; do printf '%s %s\n' "$a" "$u.service"; done
    unit_get "$f" Unit Before | while read -r b; do printf '%s %s\n' "$u.service" "$b"; done
  done
  : "$tgt"
)
before() { # A B: A is ordered before B (transitively)
  awk -v a="$1" -v b="$2" '
    { nx[$1] = nx[$1] " " $2 }
    END {
      q[1] = a; seen[a] = 1; h = 1; t = 1
      while (h <= t) {
        cur = q[h++]; n = split(nx[cur], arr, " ")
        for (i = 1; i <= n; i++) if (!(arr[i] in seen)) { seen[arr[i]] = 1; q[++t] = arr[i] }
      }
      exit (b in seen && a != b) ? 0 : 1
    }' <<<"$edges"
}
T=graphical-session.target
before tetsu-comp.service "$T"; expect "order: comp before graphical-session.target" $?
before "$T" tetsu-notifyd.service; expect "order: target before notifyd" $?
before tetsu-notifyd.service tetsu-capd.service; expect "order: notifyd before capd" $?
before "$T" tetsu-bar.service; expect "order: target before bar" $?

# ---------------------------------------------------------------- presets
upreset="$EXTRA/usr/lib/systemd/user-preset/60-tetsu-session.preset"
enabled=$(sed -n 's/^enable[[:space:]]\+//p' "$upreset" 2>/dev/null | sort | tr '\n' ' ')
[ "$enabled" = "tetsu-bar.service tetsu-capd.service tetsu-notifyd.service " ]
expect "user preset enables exactly the three daemon services" $?
grep -q '\.socket' "$upreset" 2>/dev/null; [ $? -ne 0 ] && [ -f "$upreset" ]
expect "user preset enables no socket" $?
grep -qx 'enable greetd.service' "$EXTRA/usr/lib/systemd/system-preset/50-tetsu-session.preset" 2>/dev/null
expect "system preset enables greetd.service" $?

# ----------------------------------------------------------------- greetd
cfg="$EXTRA/etc/greetd/config.toml"
grep -qx 'vt = 1' "$cfg" 2>/dev/null; expect "greetd: vt = 1" $?
grep -qxF 'command = "agreety --cmd /usr/bin/tetsu-session"' "$cfg" 2>/dev/null
expect "greetd: agreety runs tetsu-session" $?
grep -qx 'user = "greeter"' "$cfg" 2>/dev/null; expect "greetd: user = greeter" $?

# --------------------------------------------------------------- base set
deps=$(
  # shellcheck disable=SC1090
  source "$PKGBUILD" 2>/dev/null
  printf '%s\n' "${depends[@]:-}"
)
for p in greetd greetd-agreety seatd mesa libinput libxkbcommon ttf-dejavu; do
  grep -qx -- "$p" <<<"$deps"; expect "depends has $p" $?
done

# ----------------------------------------------------------------- wiring
grep -qF 'stage-session --crates' "$IMAGE/build"; expect "image/build asks stage-session for the crates" $?
grep -qF 'stage-session" "$root"' "$IMAGE/build"; expect "image/build stages the session into the payload" $?

# ---------------------------------------------------------------- scripts
stubs="$tmp/stubs"
mkdir -p "$stubs"
cat >"$stubs/systemctl" <<'EOF'
#!/bin/sh
echo "systemctl $*" >>"$STUB_LOG"
case "$*" in
  *" is-active "*) exit "${STUB_ACTIVE:-3}" ;;
  *" --wait start "*) [ -n "${STUB_HANG:-}" ] && sleep 3; exit "${STUB_START:-0}" ;;
esac
exit 0
EOF
cat >"$stubs/systemd-notify" <<'EOF'
#!/bin/sh
echo "systemd-notify $*" >>"$STUB_LOG"
EOF
chmod 0755 "$stubs/systemctl" "$stubs/systemd-notify"
SESSION="$REPO/crates/tetsu-comp/dist/tetsu-session"
READY="$REPO/crates/tetsu-comp/dist/tetsu-comp-ready"

run_stub() { # LOG ENV... -- COMMAND
  local log=$1
  shift
  : >"$log"
  env -u XDG_SESSION_ID -u WAYLAND_DISPLAY -u TETSU_COMP_SOCKET \
    PATH="$stubs:$PATH" STUB_LOG="$log" "$@" >/dev/null 2>&1
}

log="$tmp/a.log"
run_stub "$log" "$SESSION"; rc=$?
[ "$rc" -ne 0 ] && ! grep -q ' start ' "$log"
expect "session: without XDG_SESSION_ID it fails and starts nothing" $?

run_stub "$log" XDG_SESSION_ID=7 STUB_ACTIVE=0 "$SESSION"; rc=$?
[ "$rc" -eq 1 ] && ! grep -q ' start ' "$log"
expect "session: refuses when tetsu-comp.service is already active" $?

want="systemctl --user -q is-active tetsu-comp.service
systemctl --user reset-failed tetsu-comp.service
systemctl --user import-environment XDG_SESSION_ID XDG_SEAT XDG_VTNR
systemctl --user --wait start tetsu-comp.service
systemctl --user unset-environment XDG_SESSION_ID XDG_SEAT XDG_VTNR XDG_SESSION_TYPE WAYLAND_DISPLAY TETSU_COMP_SOCKET"
run_stub "$log" XDG_SESSION_ID=7 "$SESSION"; rc=$?
[ "$rc" -eq 0 ] && [ "$(cat "$log")" = "$want" ]
expect "session: the exact systemctl sequence, exit 0" $?

run_stub "$log" XDG_SESSION_ID=7 STUB_START=3 "$SESSION"; rc=$?
[ "$rc" -eq 3 ] && [ "$(cat "$log")" = "$want" ]
expect "session: a failed start still unsets the environment and keeps its status" $?

# A TERM while waiting stops the unit and still cleans the environment.
want_term="systemctl --user -q is-active tetsu-comp.service
systemctl --user reset-failed tetsu-comp.service
systemctl --user import-environment XDG_SESSION_ID XDG_SEAT XDG_VTNR
systemctl --user --wait start tetsu-comp.service
systemctl --user stop tetsu-comp.service
systemctl --user unset-environment XDG_SESSION_ID XDG_SEAT XDG_VTNR XDG_SESSION_TYPE WAYLAND_DISPLAY TETSU_COMP_SOCKET"
: >"$log"
env -u WAYLAND_DISPLAY -u TETSU_COMP_SOCKET PATH="$stubs:$PATH" STUB_LOG="$log" \
  XDG_SESSION_ID=7 STUB_HANG=1 "$SESSION" >/dev/null 2>&1 &
pid=$!
sleep 1
kill -TERM "$pid"
wait "$pid"; rc=$?
[ "$rc" -ne 0 ] && [ "$(cat "$log")" = "$want_term" ]
expect "session: TERM stops the unit, then unsets the environment" $?

[ "$(grep -m1 '^ExecStopPost=' "$comp")" = \
  'ExecStopPost=-/usr/bin/systemctl --user unset-environment WAYLAND_DISPLAY TETSU_COMP_SOCKET XDG_SESSION_TYPE' ]
expect "comp: ExecStopPost also unsets XDG_SESSION_TYPE" $?

run_stub "$log" WAYLAND_DISPLAY=wayland-1 TETSU_COMP_SOCKET=/run/x/comp "$READY"; rc=$?
[ "$rc" -eq 0 ] && [ "$(cat "$log")" = "systemctl --user import-environment WAYLAND_DISPLAY TETSU_COMP_SOCKET XDG_SESSION_TYPE
systemd-notify --ready" ]
expect "ready: imports the environment, then notifies" $?

run_stub "$log" TETSU_COMP_SOCKET=/run/x/comp "$READY"; rc=$?
[ "$rc" -ne 0 ] && ! grep -q systemd-notify "$log"
expect "ready: without WAYLAND_DISPLAY it fails and does not notify" $?

# --------------------------------------------------------- systemd-analyze
if command -v systemd-analyze >/dev/null 2>&1 && [ -d "$U" ]; then
  # The binaries are absent on the host; that is the only allowed complaint.
  out=$(LC_ALL=C systemd-analyze --user verify --man=no "$U"/tetsu-*.service 2>&1)
  bad=$(grep -vE '^tetsu-[a-z-]+\.service: Command /usr/(bin|libexec)/tetsu-[a-z-]+ is not executable' <<<"$out" | sed '/^$/d')
  [ -z "$bad" ]; expect "systemd-analyze verify: only missing-binary lines" $?
  [ -n "$bad" ] && printf '%s\n' "$bad" >&2
else
  printf 'skip systemd-analyze verify\n'
fi

if [ "$fails" -ne 0 ]; then
  printf '%d check(s) failed\n' "$fails" >&2
  exit 1
fi
