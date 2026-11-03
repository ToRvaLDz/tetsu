#!/usr/bin/env bash
# Acceptance test of the Tetsu session: install in a VM, boot to greetd, log
# in through greetd's own IPC, check the compositor and the three daemons
# (units, order, sockets), prove the agent unit's sandbox, then end the session.
#
#   tools/vm/tests/session.sh
#
# Needs the image (image/build), /dev/kvm, qemu, swtpm, socat, mkfs.vfat,
# mtools. Exit 77 (skip) without the image, KVM or a tool. Work files go to
# target/vm-session/ (TETSU_SESSION_WORK); the serial logs stay there.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
WORK_ROOT=${TETSU_SESSION_WORK:-$(cd "$HERE/../../.." && pwd)/target/vm-session}
# shellcheck source=guest.sh
. "$HERE/guest.sh"

# On a failure, the journals are the evidence.
journal_evidence() {
  gexec "journalctl --user -b --no-pager -n 60 -u tetsu-comp -u tetsu-notifyd -u tetsu-capd -u tetsu-bar -u tetsu-agent 2>&1" 60 || true
  evidence "user journal: $GOUT"
  gexec "sudo -n journalctl -b --no-pager -n 60 -u greetd 2>&1" 60 || true
  evidence "greetd journal: $GOUT"
}

# ts UNIT PROPERTY: a monotonic timestamp of a user unit, in the guest.
ts() {
  gexec "systemctl --user show -P $2 $1"
  printf '%s' "${GOUT//[^0-9]/}"
}

session_checks() {
  local sock out
  # The serial console is a terminal: keep the systemd tools from paging.
  gexec "export SYSTEMD_PAGER= PAGER=cat"
  # 1. Boots to greetd.
  check "the default target is graphical" '^graphical\.target$' "systemctl get-default"
  check "greetd is active" '^active$' "systemctl is-active greetd.service"
  check "the greeter is agreety on tty1" '^tty1$' \
    "$(guest_poll 60 'ps -o tty= -p "$(pgrep -u greeter -x agreety | head -n 1)" 2>/dev/null | grep -qx tty1'); ps -o tty= -p \"\$(pgrep -u greeter -x agreety | head -n 1)\""
  check "the three daemons are enabled for every user" '^enabled$' \
    "systemctl --user is-enabled tetsu-notifyd.service tetsu-capd.service tetsu-bar.service | sort -u"
  check_not "no session binary has a missing library" 'not found' \
    "ldd /usr/bin/tetsu-comp /usr/bin/tetsu-bar /usr/bin/tetsu-notifyd /usr/bin/tetsu-capd"

  # 2. Logs in, through greetd's IPC: the path of the tty1 greeter.
  gexec "ls /run/greetd-*.sock"
  sock=$(head -n 1 <<<"$GOUT")
  [ -n "$sock" ] || die "no greetd socket"
  mark
  "$VM" send --state "$STATE" "sudo -n env GREETD_SOCK=$sock agreety --max-failures 1 --cmd /usr/bin/tetsu-session; echo AGREETY-RC-\$?\\n"
  expect 'login: ?$' 60
  say "$USER_NAME"
  expect 'Password: ?$' 30
  say_secret "$USER_PW"
  expect 'AGREETY-RC-[0-9]+$' 60
  assert_contains "$SCENARIO: agreety logged in through greetd" "$EXPECT_LINE" "AGREETY-RC-0"

  # 3. The session is up.
  gexec "$(guest_poll 90 '[ "$(systemctl --user is-active tetsu-bar.service tetsu-capd.service tetsu-notifyd.service | grep -cx active)" = 3 ]'); true" 120
  gexec "systemctl --user is-active tetsu-comp.service graphical-session.target tetsu-notifyd.service tetsu-capd.service tetsu-bar.service | grep -cx active"
  assert_eq "$SCENARIO: five units active" "5" "$(tail -n 1 <<<"$GOUT")"
  check "the greetd logind session is a wayland one on VT1" 'Type=wayland' \
    "s=\$(loginctl list-sessions --no-legend | awk '{print \$1}' | while read -r i; do loginctl show-session \$i -p Service | grep -q greetd && echo \$i; done | tail -n 1); loginctl show-session \$s -p Type -p VTNr -p Active -p Name"
  assert_contains "$SCENARIO: the session sits on VT1" "$GOUT" "VTNr=1"
  assert_contains "$SCENARIO: the session is active" "$GOUT" "Active=yes"
  assert_contains "$SCENARIO: the session is the user's" "$GOUT" "Name=$USER_NAME"
  check "the manager environment has WAYLAND_DISPLAY" '^WAYLAND_DISPLAY=' "systemctl --user show-environment"
  check "the manager environment has TETSU_COMP_SOCKET" '^TETSU_COMP_SOCKET=' "systemctl --user show-environment"
  check "the Wayland socket exists" 'sock-ok' \
    "e=\$(systemctl --user show-environment); d=\$(sed -n 's/^WAYLAND_DISPLAY=//p' <<<\"\$e\"); test -S \"/run/user/\$(id -u)/\$d\" && echo sock-ok"
  check "the compositor bus socket exists" 'sock-ok' \
    "s=\$(systemctl --user show-environment | sed -n 's/^TETSU_COMP_SOCKET=//p'); test -S \"\$s\" && echo sock-ok"
  check "ListOutputs names an output" '"id" : "out-' \
    "varlinkctl call \"\$(systemctl --user show-environment | sed -n 's/^TETSU_COMP_SOCKET=//p')\" org.tetsu.comp.output.v1.ListOutputs '{}'"
  check "notifyd answers org.tetsu.notify.v1" 'org\.tetsu\.notify\.v1' \
    "varlinkctl info /run/user/\$(id -u)/tetsu/org.tetsu.notify.v1"
  check "capd answers org.tetsu.cap.v1" 'org\.tetsu\.cap\.v1' \
    "varlinkctl info /run/user/\$(id -u)/tetsu/org.tetsu.cap.v1"
  check "org.freedesktop.Notifications is owned by tetsu-notifyd" 'tetsu-notifyd\.service' \
    "busctl --user status org.freedesktop.Notifications"
  for u in tetsu-bar tetsu-notifyd tetsu-capd; do
    check "$u has never restarted" '^NRestarts=0$' "systemctl --user show -p NRestarts $u.service"
  done

  # 4. In order.
  local comp_a tgt_a nfy_a nfy_s cap_s bar_s
  comp_a=$(ts tetsu-comp.service ActiveEnterTimestampMonotonic)
  tgt_a=$(ts graphical-session.target ActiveEnterTimestampMonotonic)
  nfy_a=$(ts tetsu-notifyd.service ActiveEnterTimestampMonotonic)
  nfy_s=$(ts tetsu-notifyd.service ExecMainStartTimestampMonotonic)
  cap_s=$(ts tetsu-capd.service ExecMainStartTimestampMonotonic)
  bar_s=$(ts tetsu-bar.service ExecMainStartTimestampMonotonic)
  evidence "comp.active=$comp_a target.active=$tgt_a notifyd.active=$nfy_a notifyd.start=$nfy_s capd.start=$cap_s bar.start=$bar_s"
  out=ok
  [ -n "$comp_a$tgt_a$nfy_a$nfy_s$cap_s$bar_s" ] || out=empty
  [ "$out" = ok ] && [ "$comp_a" -le "$tgt_a" ] && [ "$tgt_a" -le "$nfy_s" ] \
    && [ "$nfy_a" -le "$cap_s" ] && [ "$tgt_a" -le "$bar_s" ] || out=wrong-order
  assert_eq "$SCENARIO: comp <= target <= notifyd <= capd, target <= bar" ok "$out"

  # 5. The agent unit's sandbox (#82), while the daemons it talks to are up.
  agent_sandbox_checks

  # 6. Ends cleanly.
  gexec "systemctl --user stop tetsu-comp.service; echo stopped"
  gexec "$(guest_poll 60 '! systemctl --user is-active -q graphical-session.target tetsu-notifyd.service tetsu-capd.service tetsu-bar.service'); systemctl --user is-active graphical-session.target tetsu-notifyd.service tetsu-capd.service tetsu-bar.service | sort -u" 90
  assert_eq "$SCENARIO: the target and the daemons are inactive" "inactive" "$(tail -n 1 <<<"$GOUT")"
  check "the greeter is back" '^[0-9]+$' \
    "$(guest_poll 60 'pgrep -u greeter -x agreety'); pgrep -u greeter -x agreety | head -n 1"
  check_not "WAYLAND_DISPLAY is gone from the manager environment" '^WAYLAND_DISPLAY=' \
    "systemctl --user show-environment"
}

scenario_session() {
  LEVEL=paranoid
  setup "session"
  echo "== $SCENARIO (work: $WORK)"
  make_blank_target
  make_answers_image paranoid "$WORK/answers.img" fat
  boot_medium "$WORK/answers.img"
  expect 'answer file is in use' 300
  wait_installed
  stop_after_install
  first_boot paranoid
  session_checks
  if [ "$TESTS_FAILED" -ne 0 ]; then journal_evidence; fi
  poweroff_guest
  stop_vm
}

preflight() {
  if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then echo "SKIP: /dev/kvm is not usable"; exit 77; fi
  if [ ! -r "$IMAGE" ]; then echo "SKIP: no installer image at $IMAGE (run image/build)"; exit 77; fi
  local t
  for t in qemu-img qemu-system-x86_64 swtpm socat mkfs.vfat mcopy; do
    command -v "$t" >/dev/null || { echo "SKIP: $t is missing"; exit 77; }
  done
}

main() {
  case "${1:-}" in -h | --help) sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;; esac
  preflight
  trap 'stop_vm' EXIT
  scenario_session
  summary
}

main "$@"
