#!/usr/bin/env bash
# Acceptance test of the transactions (update, rollback, power loss, fallback)
# on a system installed by the medium, in a VM (OVMF Secure Boot + swtpm).
#
#   tools/vm/tests/tx.sh base
#   tools/vm/tests/tx.sh rollback
#   tools/vm/tests/tx.sh power-cut POINT|all
#   tools/vm/tests/tx.sh fallback
#   tools/vm/tests/tx.sh all
#
# POINT is a pause point of tetsu-tx (TETSU_TX_PAUSE_AT): gc-done, snapshot,
# downloaded, installed, sealed, uki-staged, renamed, committed, recorded.
# Needs the same as install.sh. Exit 77 (skip) without the image, KVM or a
# tool. One install builds the base (target/vm-tx/base/); every case runs on a
# qcow2 overlay of it with its own copy of VARS and TPM state. Work files and
# serial logs go to target/vm-tx/<case>/ (TETSU_TX_WORK).
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
WORK_ROOT=${TETSU_TX_WORK:-$(cd "$HERE/../../.." && pwd)/target/vm-tx}
# shellcheck source=guest.sh
. "$HERE/guest.sh"

BASE="$WORK_ROOT/base"
POINTS=(gc-done snapshot downloaded installed sealed uki-staged renamed committed recorded)
ESP_DIR=/efi/EFI/Linux
TX_SECS=1800

# ------------------------------------------------------------------ base
# Paranoid install (an answers volume, no TPM binding, so copies of the disk
# are independent), first boot, snapshot 1 blessed and recorded good.
build_base() {
  LEVEL=paranoid
  setup base
  echo "== $SCENARIO (work: $WORK)"
  make_blank_target
  make_answers_image paranoid "$WORK/answers.img" fat
  boot_medium "$WORK/answers.img"
  expect 'answer file is in use' 300
  wait_installed
  stop_after_install
  first_boot paranoid
  check "snapshot 1 is recorded good" '"boot": "good"' \
    "for i in \$(seq 1 30); do sudo -n grep -q good /var/lib/tetsu/transactions/1.json && break; sleep 1; done; sudo -n cat /var/lib/tetsu/transactions/1.json" 90
  check "snapshot 1 is blessed (no boot counter in its name)" '^tetsu-1\.efi$' \
    "for i in \$(seq 1 30); do sudo -n ls -1 $ESP_DIR | grep -qx tetsu-1.efi && break; sleep 2; done; sudo -n ls -1 $ESP_DIR" 120
  poweroff_guest
  stop_vm
  touch "$BASE/ready"
}

ensure_base() {
  [ -e "$BASE/ready" ] || build_base
}

# A case: its own copy of the base state on a qcow2 overlay of the base disk.
case_setup() {
  ensure_base
  setup "$1"
  echo "== $SCENARIO (work: $WORK)"
  cp -a "$BASE/vm" "$STATE"
  rm -f "$STATE"/swtpm.sock "$STATE"/swtpm.pid "$STATE"/qemu.pid "$STATE"/qmp.sock "$STATE"/serial.sock
  : >"$STATE/serial.log"
  qemu-img create -q -f qcow2 -b "$BASE/target.qcow2" -F qcow2 "$TARGET"
}

boot_login() {
  boot_installed
  boot_to_login
  login
}

# ------------------------------------------------------------ transactions
# start_paused POINT ARGS...: tetsu-tx ARGS... in the background, held at
# POINT; returns once it waits there. A failed run or a pause file that cannot
# be written ends the wait at once.
start_paused() {
  local point=$1 log
  log=/tmp/tx-$point.log
  shift
  reprime_sudo
  gexec "(sudo -n env TETSU_TX_PAUSE_AT=$point tetsu-tx $* >$log 2>&1 &); echo started"
  gexec "$(guest_poll "$TX_SECS" "sudo -n grep -qx $point /run/tetsu/paused-at 2>/dev/null" \
    "grep -Eq '^error|tetsu-tx: cannot write' $log"); cat $log" $((TX_SECS + 60))
  case $GOUT in
    *"tetsu-tx: cannot write"*) die "the pause file cannot be written at $point: $GOUT" ;;
  esac
  if LC_ALL=C grep -aEq -- "tetsu-tx: paused at $point\$" <<<"$GOUT"; then
    pass "$SCENARIO: the transaction waits at $point"
  else
    fail "$SCENARIO: the transaction waits at $point" "got: $GOUT"
  fi
}

# release_and_wait POINT: let the transaction held at POINT go on and wait
# for its report (the log of start_paused).
release_and_wait() {
  local point=$1 log=/tmp/tx-$1.log
  check "the transaction held at $point commits" 'committed as snapshot [0-9]+' \
    "sudo -n rm -f /run/tetsu/paused-at; $(guest_poll "$TX_SECS" "grep -q 'committed as snapshot' $log" \
      "grep -q '^error' $log"); cat $log" $((TX_SECS + 60))
}

# The newest snapshot number in the booted command line.
booted_snapshot() {
  gexec "cat /proc/cmdline"
  sed -n 's/.*subvol=@snapshots\/\([0-9][0-9]*\)\/root.*/\1/p' <<<"$GOUT" | head -n 1
}

wait_boot_check() {
  check "the boot check is active" '^active$' \
    "for i in \$(seq 1 60); do systemctl is-active tetsu-boot-check && break; sleep 1; done; systemctl is-active tetsu-boot-check" 120
}

# ----------------------------------------------------------------- rollback
case_rollback() {
  case_setup rollback
  boot_login
  check "snapshot 1 is booted" 'subvol=@snapshots/1/root' "cat /proc/cmdline"
  gexec "pacman -Q | sha256sum"
  local before=$GOUT
  gexec "echo BEFORE-48 | sudo -n tee /etc/tetsu-vm-before.conf >/dev/null; echo before-rc=\$?"
  assert_contains "$SCENARIO: the marker before the transactions is written" "$GOUT" "before-rc=0"

  # 2 is pending (never booted); 3 is built from it.
  reprime_sudo
  check "tetsu-tx update succeeds" 'update-rc=0' \
    "sudo -n tetsu-tx update >/tmp/tx.log 2>&1; echo update-rc=\$?; tail -n 5 /tmp/tx.log" $TX_SECS
  reprime_sudo
  check "tetsu-tx install tree succeeds" 'install-rc=0' \
    "sudo -n tetsu-tx install tree >/tmp/tx.log 2>&1; echo install-rc=\$?; tail -n 5 /tmp/tx.log" $TX_SECS
  check "the install is snapshot 3" 'tetsu-3' "sudo -n ls /efi/EFI/Linux"

  reboot_guest
  boot_to_login
  login
  check "snapshot 3 is booted" 'subvol=@snapshots/3/root' "cat /proc/cmdline"
  check "tree is installed on 3" '^tree ' "pacman -Q tree"
  gexec "echo ON-3 | sudo -n tee /etc/tetsu-vm-on-3.conf >/dev/null; sudo -n useradd vmon3; echo on3-rc=\$?"
  assert_contains "$SCENARIO: the changes on 3 are made" "$GOUT" "on3-rc=0"

  # No target: the newest good snapshot older than 3 is 1, since 2 never booted.
  reprime_sudo
  check "tetsu-tx rollback succeeds" 'rollback-rc=0' \
    "sudo -n tetsu-tx rollback >/tmp/tx.log 2>&1; echo rollback-rc=\$?; tail -n 5 /tmp/tx.log" $TX_SECS
  check "the rollback is snapshot 4" 'tetsu-4' "sudo -n ls /efi/EFI/Linux"

  reboot_guest
  boot_to_login
  login
  check "snapshot 4 is booted" 'subvol=@snapshots/4/root' "cat /proc/cmdline"
  check "snapshot 4 is the rollback" '"kind": "rollback"' \
    "sudo -n cat /var/lib/tetsu/transactions/4.json"
  check "the rollback restored snapshot 1" '"source": 1' \
    "sudo -n cat /var/lib/tetsu/transactions/4.json"
  gexec "pacman -Q | sha256sum"
  assert_eq "$SCENARIO: the package list is the one of snapshot 1" "$before" "$GOUT"
  check "tree is gone" 'was not found' "pacman -Q tree 2>&1"
  check "the file from before the transactions is there" 'BEFORE-48' "cat /etc/tetsu-vm-before.conf"
  # A rollback does not merge /etc, only the accounts.
  check "the /etc file made on 3 is not carried over" 'No such file' "ls /etc/tetsu-vm-on-3.conf 2>&1"
  check "the account made on 3 is merged" 'uid=[0-9]+\(vmon3\)' "id vmon3"
  wait_boot_check
  poweroff_guest
  stop_vm
}

# ---------------------------------------------------------------- power cut
# The snapshots the system may boot after the power is cut at POINT.
allowed_boots() {
  case $1 in
    gc-done | snapshot | downloaded | installed | sealed | uki-staged) echo "1" ;;
    renamed) echo "1 2" ;; # the rename may or may not have reached the FAT
    committed | recorded) echo "2" ;;
  esac
}

# The cut system never boots a partial snapshot: a complete one, read-only,
# from an entry that is no leftover, and the boot check passes on it.
verify_boot() { # POINT
  local point=$1 n allowed
  n=$(booted_snapshot)
  allowed=$(allowed_boots "$point")
  case " $allowed " in
    *" $n "*) pass "$SCENARIO: snapshot $n is one of the allowed (${allowed// /, })" ;;
    *) fail "$SCENARIO: snapshot $n is one of the allowed (${allowed// /, })" "booted $n" ;;
  esac
  say_info "booted snapshot $n after the cut at $point"
  gexec "sudo -n sh -c 'stat -c %n=%s $ESP_DIR/*' | paste -sd,"
  local esp=$GOUT
  say_info "ESP after the cut: $esp"
  # An orphan .tmp is allowed by the protocol, even a short one; a committed
  # entry (.efi) of zero bytes is not: the rename follows the sync of the data.
  # An empty listing (a failed sudo) is a failure, not a pass on nothing.
  if [ -z "$esp" ]; then
    fail "$SCENARIO: no committed entry on the ESP is zero bytes" "the ESP listing is empty"
  elif [ -n "$n" ] && ! LC_ALL=C grep -aEq -- "tetsu-$n(\.efi|\+)" <<<"$esp"; then
    fail "$SCENARIO: no committed entry on the ESP is zero bytes" "no entry of snapshot $n in: $esp"
  elif LC_ALL=C grep -aEq -- '\.efi=0(,|$)' <<<"$esp"; then
    fail "$SCENARIO: no committed entry on the ESP is zero bytes" "got: $esp"
  else
    pass "$SCENARIO: no committed entry on the ESP is zero bytes"
  fi
  [ -n "$n" ] || return 0
  check "the booted snapshot is read-only" 'ro=true' "sudo -n btrfs property get / ro"
  check "the booted entry is a committed one, not a leftover" "^tetsu-$n(\\.efi|\\+[0-9]+(-[0-9]+)?\\.efi)\$" \
    "sudo -n ls -1 $ESP_DIR | grep -E '^tetsu-$n(\\.efi|\\+)'"
  wait_boot_check
}

# What the next transaction collects: held at gc-done, which comes right after
# the collection of the snapshots, then let go on, which clears the ESP.
verify_collected() { # POINT
  local point=$1
  case $point in
    snapshot | downloaded | installed | sealed | uki-staged)
      check "the snapshot of the cut transaction is a leftover" '@snapshots/2/root$' \
        "sudo -n btrfs subvolume list / | grep -E '@snapshots/2/root$'"
      ;;
  esac
  if [ "$point" = uki-staged ]; then
    check "the staged UKI is on the ESP" '\.efi\.tmp$' "sudo -n ls -1 $ESP_DIR"
  fi
  start_paused gc-done update
  check_not "no snapshot is left without a committed UKI" 'ORPHAN' \
    "for d in /run/tetsu/top/@snapshots/*; do n=\${d##*/}; sudo -n ls -1 $ESP_DIR | grep -Eq \"^tetsu-\$n(\\.efi|\\+)\" || echo ORPHAN-\$n; done; echo scan-done"
  # The UKI write clears the staged files before it stages its own.
  release_and_wait gc-done
  check_not "no staged UKI is left on the ESP" '\.tmp' "sudo -n ls -1 $ESP_DIR"
  check_not "every entry on the ESP has its record" 'NORECORD' \
    "for f in \$(sudo -n ls -1 $ESP_DIR); do n=\${f#tetsu-}; n=\${n%%[+.]*}; sudo -n test -e /var/lib/tetsu/transactions/\$n.json || echo NORECORD-\$n; done; echo scan-done"
}

case_power_cut() {
  local point=$1
  case_setup "power-cut-$point"
  boot_login
  start_paused "$point" update
  "$VM" power-cut --state "$STATE" >/dev/null
  archive_log cut
  boot_login
  verify_boot "$point"
  verify_collected "$point"
  stop_vm
}

power_cut() {
  local arg=${1:-} p
  case $arg in
    all) for p in "${POINTS[@]}"; do case_power_cut "$p"; done ;;
    *)
      for p in "${POINTS[@]}"; do
        [ "$p" != "$arg" ] || { case_power_cut "$p"; return 0; }
      done
      usage
      exit 64
      ;;
  esac
}

# ----------------------------------------------------------------- fallback
# An update whose snapshot carries an enabled failing unit: the boot check
# fails on every try, and after the third the boot counter hands the boot
# back to snapshot 1.
case_fallback() {
  local k
  case_setup fallback
  boot_login
  start_paused installed update
  gexec "u=/run/tetsu/top/@snapshots/2/root/usr/lib/systemd/system; printf '%s\n' '[Service]' 'Type=oneshot' 'ExecStart=/bin/false' '[Install]' 'WantedBy=multi-user.target' | sudo -n tee \$u/tetsu-vm-broken.service >/dev/null && sudo -n mkdir -p \$u/multi-user.target.wants && sudo -n ln -s ../tetsu-vm-broken.service \$u/multi-user.target.wants/tetsu-vm-broken.service; echo planted-rc=\$?"
  assert_contains "$SCENARIO: the failing unit is planted in the new snapshot" "$GOUT" "planted-rc=0"
  release_and_wait installed

  for k in 1 2 3; do
    reboot_guest
    boot_to_login
    login
    check "boot $k runs snapshot 2" 'subvol=@snapshots/2/root' "cat /proc/cmdline"
    check "boot $k: the broken unit has failed" 'tetsu-vm-broken' \
      "systemctl --failed --no-legend"
    check "boot $k: the boot check fails" '^failed$' \
      "for i in \$(seq 1 60); do systemctl is-failed tetsu-boot-check >/dev/null && break; sleep 1; done; systemctl is-failed tetsu-boot-check"
    check "boot $k: the entry is not blessed" "^tetsu-2\\+$((3 - k))-$k\\.efi\$" \
      "sudo -n ls -1 $ESP_DIR | grep -E '^tetsu-2'"
  done

  reboot_guest
  boot_to_login
  login
  check "boot 4 falls back to snapshot 1" 'subvol=@snapshots/1/root' "cat /proc/cmdline"
  check "the ESP shows the spent tries" '^tetsu-2\+0-3\.efi$' \
    "sudo -n ls -1 $ESP_DIR | grep -E '^tetsu-2'"
  wait_boot_check
  check "snapshot 2 is recorded bad" '"boot": "bad"' \
    "for i in \$(seq 1 30); do sudo -n grep -q bad /var/lib/tetsu/transactions/2.json && break; sleep 1; done; sudo -n cat /var/lib/tetsu/transactions/2.json" 90
  poweroff_guest
  stop_vm
}

# ------------------------------------------------------------------ main
usage() {
  sed -n '2,9p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

preflight() {
  if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then echo "SKIP: /dev/kvm is not usable"; exit 77; fi
  if [ ! -r "$IMAGE" ]; then echo "SKIP: no installer image at $IMAGE (run image/build)"; exit 77; fi
  local t
  for t in qemu-img qemu-system-x86_64 swtpm socat mkfs.ext4 mkfs.vfat mcopy sgdisk; do
    command -v "$t" >/dev/null || { echo "SKIP: $t is missing"; exit 77; }
  done
}

main() {
  local cmd=${1:-}
  [ $# -gt 0 ] && shift
  case "$cmd" in
    -h | --help | help | "") usage; [ -n "$cmd" ] || exit 64; exit 0 ;;
  esac
  preflight
  trap 'stop_vm' EXIT
  case "$cmd" in
    base) build_base ;;
    rollback) case_rollback ;;
    power-cut) power_cut "${1:-}" ;;
    fallback) case_fallback ;;
    all)
      build_base
      case_rollback
      power_cut all
      case_fallback
      ;;
    *) usage; exit 64 ;;
  esac
  summary
}

main "$@"
