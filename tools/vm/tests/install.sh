#!/usr/bin/env bash
# Acceptance test of the Tetsu installer: install from the medium in a VM
# (OVMF Secure Boot + swtpm), boot the result, check it, reboot, check again.
#
#   tools/vm/tests/install.sh answers paranoid|comfort|balanced
#   tools/vm/tests/install.sh interactive [comfort|balanced]   (default balanced)
#   tools/vm/tests/install.sh sb-off
#   tools/vm/tests/install.sh etc-sync
#   tools/vm/tests/install.sh all
#
# Needs the image (image/build), /dev/kvm, qemu, swtpm, socat, mkfs.ext4,
# mkfs.vfat, mtools, sgdisk. Exit 77 (skip) without the image or KVM. Work files go to
# target/vm-install/<scenario>/ (TETSU_INSTALL_WORK); the serial logs of every
# phase stay there. Test-only secrets (guest.sh) are the ones in answers/*.toml.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
WORK_ROOT=${TETSU_INSTALL_WORK:-$(cd "$HERE/../../.." && pwd)/target/vm-install}
# shellcheck source=guest.sh
. "$HERE/guest.sh"

# --------------------------------------------------------------- scenarios
scenario_answers() {
  local level=$1
  LEVEL=$level
  setup "answers-$level"
  echo "== $SCENARIO (work: $WORK)"
  make_blank_target
  # Paranoid takes its answers from a FAT volume, the others from ext4.
  if [ "$level" = paranoid ]; then
    make_answers_image "$level" "$WORK/answers.img" fat
  else
    make_answers_image "$level" "$WORK/answers.img"
  fi
  boot_medium "$WORK/answers.img"
  expect 'answer file is in use' 300
  wait_installed
  stop_after_install
  first_boot "$level"
  second_boot_checks "$level"
  # A fresh system has no sync databases and install never fetches them (no
  # partial upgrade): past the base guard, tx either commits or stops at the
  # lookup and proposes an update. A refusal by the guard, a bare "package
  # not found", a sudo failure or a crash matches neither.
  check "tx install passes the guard and proposes an update" \
    'install tree committed as snapshot|try: tetsu-tx update, then install again' \
    "sudo -n tetsu-tx install tree 2>&1" 600
  poweroff_guest
  stop_vm
}

# Index of the menu line matching REGEX in the options printed after MARK.
menu_number() {
  local re=$1 line
  line=$(since_mark | LC_ALL=C grep -aE -- "^ *[0-9]+\\) .*$re" | tail -n 1)
  [ -n "$line" ] || die "no menu entry matching /$re/"
  line=${line#"${line%%[![:space:]]*}"}
  printf '%s' "${line%%)*}"
}

recovery_key() {
  since_mark | LC_ALL=C grep -aE '^[cbdefghijklnrtuv]{8}(-[cbdefghijklnrtuv]{8}){7}$' | tail -n 1
}

scenario_interactive() {
  local level=${1:-balanced} n key disk
  LEVEL=$level
  setup "interactive-$level"
  echo "== $SCENARIO (work: $WORK)"
  make_used_target
  boot_medium ""
  expect 'Keyboard layout:' 300
  n=$(menu_number '\bit\b')
  say "$n"
  expect 'Disk to wipe'
  expect 'Choice \[1-[0-9]+\]: ?$'
  disk=$(since_mark | LC_ALL=C grep -aoE '^ *[0-9]+\) vd[a-z]+ ' | head -n 1)
  n=$(menu_number 'vd[a-z]')
  disk=$(since_mark | LC_ALL=C grep -aE "^ *$n\\) vd[a-z]" | awk '{print $2}')
  say "$n"
  expect "Type $disk to erase it"
  say "not-the-disk"
  expect "type $disk exactly"
  pass "$SCENARIO: the disk name guard rejects a wrong name"
  say "$disk"
  expect 'Unlock level:'
  case "$level" in
    paranoid) n=$(menu_number 'Paranoid') ;;
    comfort) n=$(menu_number 'Comfort') ;;
    balanced) n=$(menu_number 'Balanced') ;;
  esac
  say "$n"
  expect 'Disk passphrase \(not shown\): ?$'
  say_secret "$PASSPHRASE"
  expect 'Repeat the disk passphrase \(not shown\): ?$'
  say_secret "$PASSPHRASE"
  expect 'Retype the recovery key: ?$' 60
  key=$(recovery_key)
  [ -n "$key" ] || die "the recovery key is not on the console"
  evidence "recovery key read from the console: ${key:0:8}-..."
  say "wrong-key"
  expect 'not the recovery key'
  pass "$SCENARIO: the recovery key retype guard rejects a wrong key"
  say "$key"
  expect 'User name: ?$'
  say "$USER_NAME"
  expect 'Full name \(optional\): ?$'
  say "VM Tester"
  expect 'Session password \(not shown\): ?$'
  say_secret "$USER_PW"
  expect 'Repeat the session password \(not shown\): ?$'
  say_secret "$USER_PW"
  expect 'Agent backend:'
  n=$(menu_number 'Skip')
  say "$n"
  expect 'Summary:'
  expect 'Choice \[1-[0-9]+\]: ?$'
  say "1"
  expect 'Wipe .* and install\? \[y/n\]: ?$'
  say "y"
  wait_installed
  stop_after_install
  first_boot "$level"
  second_boot_checks "$level"
  poweroff_guest
  stop_vm
}

# GUIDs of the db variable (image security) and of a throwaway owner, and the
# directories of the wrapper that fails the PK write on purpose and of the seed.
DB_GUID=d719b2cb-3d3a-4596-a3bc-dad00e67656f
SEED_GUID=5d2c8e3a-6f41-4b7d-9a10-0c3e5f7a9b21
FAULT_DIR=/run/tetsu-fi
SEED_DIR=/tmp/sbseed

# An efi-updatevar that fails when it writes the PK and is the real one
# otherwise: the enrollment dies after db and KEK are in, before Setup Mode ends.
install_pk_fault() {
  local wrapper
  wrapper=$(cat <<'WRAP'
sudo -n mkdir -p /run/tetsu-fi; printf '%s\n' '#!/bin/sh' 'for last; do :; done' '[ "$last" = PK ] && { echo injected PK write failure >&2; exit 1; }' 'exec /usr/bin/efi-updatevar "$@"' | sudo -n tee /run/tetsu-fi/efi-updatevar >/dev/null; sudo -n chmod 755 /run/tetsu-fi/efi-updatevar; echo wrapper-rc=$?
WRAP
)
  gexec "$wrapper"
  assert_contains "$SCENARIO: the PK fault wrapper is in place" "$GOUT" "wrapper-rc=0"
}

# fault_enroll NAME: tetsu secureboot enroll with the failing PK write; leaves
# the output in GOUT and checks it failed.
fault_enroll() {
  gexec "sudo -n env PATH=$FAULT_DIR:\$PATH tetsu secureboot enroll 2>&1; echo enroll-rc=\$?" 180
  if LC_ALL=C grep -aEq 'enroll-rc=[1-9]' <<<"$GOUT"; then
    pass "$SCENARIO: $1: the faulted enrollment fails"
  else
    fail "$SCENARIO: $1: the faulted enrollment fails" "$GOUT"
  fi
  evidence "$(tail -n 3 <<<"$GOUT" | tr '\n' '|')"
}

# check_still_in_setup_mode NAME: Setup Mode is on and no PK was left behind.
check_still_in_setup_mode() {
  check "$1: still in Setup Mode" '(^| )6 +0 +0 +0 +1$' \
    "od -An -tu1 /sys/firmware/efi/efivars/SetupMode-$GLOBAL_GUID"
  check "$1: the PK is not there" '^0$' \
    "ls /sys/firmware/efi/efivars | grep -c '^PK-' || true"
}

# efivar_hashes: HASHES = sha256 of the whole db and KEK efivarfs files
# (attributes included). Not run in a subshell: gexec counts its calls.
efivar_hashes() {
  gexec "sudo -n sha256sum /sys/firmware/efi/efivars/db-$DB_GUID /sys/firmware/efi/efivars/KEK-$GLOBAL_GUID 2>&1"
  HASHES=$(LC_ALL=C grep -a 'efivars/' <<<"$GOUT" || true)
}

# Rollback and de-duplication of a partial enrollment, in Setup Mode with the
# real efitools and OVMF: the faulted enrollment must leave db and KEK as found.
sb_rollback_checks() {
  local before after
  install_pk_fault
  # (a) Empty variables: the restore deletes what the enrollment appended.
  fault_enroll "empty variables"
  assert_contains "$SCENARIO: empty variables: the message says db and KEK were restored" \
    "$GOUT" "db and KEK were restored"
  check "empty variables: db and KEK are absent again" '^0$' \
    "ls /sys/firmware/efi/efivars | grep -cE '^(db|KEK)-' || true"
  check_still_in_setup_mode "empty variables"
  # (b) Variables that were there before: byte for byte as found.
  gexec "rm -rf $SEED_DIR; mkdir -p $SEED_DIR && openssl req -x509 -newkey rsa:2048 -nodes -keyout $SEED_DIR/seed.key -out $SEED_DIR/seed.crt -subj /CN=tetsu-vm-seed -days 2 >/dev/null 2>&1 && cert-to-efi-sig-list -g $SEED_GUID $SEED_DIR/seed.crt $SEED_DIR/seed.esl && for v in db KEK; do sign-efi-sig-list -a -k $SEED_DIR/seed.key -c $SEED_DIR/seed.crt \$v $SEED_DIR/seed.esl $SEED_DIR/seed-\$v.auth && sudo -n efi-updatevar -a -f $SEED_DIR/seed-\$v.auth \$v; done; echo seed-rc=\$?" 120
  assert_contains "$SCENARIO: db and KEK are seeded in Setup Mode" "$GOUT" "seed-rc=0"
  efivar_hashes
  before=$HASHES
  [ "$(wc -l <<<"$before")" -eq 2 ] || fail "$SCENARIO: the seeded db and KEK are hashed" "$before"
  fault_enroll "pre-existing variables"
  efivar_hashes
  after=$HASHES
  assert_eq "$SCENARIO: pre-existing variables: db and KEK are byte for byte as found" \
    "$before" "$after"
  check_still_in_setup_mode "pre-existing variables"
  # (c) Proves the firmware drops an identical append; that two Tetsu enrollments produce
  # identical entries rests on the derived-GUID unit test, since a second real enrollment
  # cannot run once the PK ends Setup Mode.
  # (c) The same lists appended again (same owner GUID, same certificate) add nothing.
  gexec "for v in db KEK; do sudo -n efi-updatevar -a -f $SEED_DIR/seed-\$v.auth \$v; done; echo append-rc=\$?"
  assert_contains "$SCENARIO: the seed lists are appended a second time" "$GOUT" "append-rc=0"
  efivar_hashes
  after=$HASHES
  assert_eq "$SCENARIO: appending the same entries again changes nothing" "$before" "$after"
  gexec "sudo -n rm -rf $FAULT_DIR $SEED_DIR; echo cleaned-rc=\$?"
  assert_contains "$SCENARIO: the fault wrapper is removed" "$GOUT" "cleaned-rc=0"
}

# Secure Boot off at install time, enrolled later.
scenario_sb_off() {
  LEVEL=comfort
  setup "sb-off"
  echo "== $SCENARIO (work: $WORK)"
  [ -r "$OVMF_CODE_PLAIN" ] || die "no OVMF build without Secure Boot at $OVMF_CODE_PLAIN"
  make_blank_target
  make_answers_image comfort "$WORK/answers.img"
  # Phase 1: firmware without Secure Boot support at all.
  export TETSU_OVMF_CODE=$OVMF_CODE_PLAIN
  boot_medium "$WORK/answers.img"
  wait_installed
  if expect_soon 'Secure Boot stays off' 5; then
    pass "$SCENARIO: the installer keeps the keys and leaves Secure Boot off"
  else
    fail "$SCENARIO: the installer keeps the keys and leaves Secure Boot off"
  fi
  stop_after_install
  # Phase 2: boots with the passphrase; level pending.
  boot_installed
  boot_to_login
  login
  check "Secure Boot is off" '(^| )6 +0 +0 +0 +0$|No such file' \
    "od -An -tu1 /sys/firmware/efi/efivars/SecureBoot-$GLOBAL_GUID 2>&1; ls /sys/firmware/efi/efivars | grep -c ^SecureBoot || true"
  check "the level file says pending" 'state = "pending"' "cat /etc/tetsu/unlock-level.toml"
  check_not "no TPM2 token yet" 'tpm2' "sudo -n systemd-cryptenroll /dev/disk/by-partlabel/TETSU-ROOT"
  check "the generated keys are on the disk" 'PK\.' "sudo -n ls /var/lib/tetsu/keys"
  poweroff_guest
  archive_log boot-sb-off
  stop_vm
  # Phase 3: the user clears the keys: fresh VARS (Setup Mode), Secure Boot capable firmware.
  unset TETSU_OVMF_CODE
  rm -f "$STATE/vars.fd"
  boot_installed
  boot_to_login
  login
  check "the firmware is in Setup Mode" '(^| )6 +0 +0 +0 +1$' \
    "od -An -tu1 /sys/firmware/efi/efivars/SetupMode-$GLOBAL_GUID"
  sb_rollback_checks
  gexec "sudo -n tetsu secureboot enroll 2>&1; echo enroll-rc=\$?" 120
  if LC_ALL=C grep -aq 'enroll-rc=0' <<<"$GOUT"; then
    pass "$SCENARIO: tetsu secureboot enroll succeeds"
  else
    fail "$SCENARIO: tetsu secureboot enroll succeeds" "$GOUT"
  fi
  evidence "$(tail -n 3 <<<"$GOUT" | tr '\n' '|')"
  # Phase 4: the next boot seals.
  reboot_guest
  boot_to_login
  login
  common_checks comfort
  # Phase 5: and the next one unlocks through the TPM.
  second_boot_checks comfort
  poweroff_guest
  stop_vm
}

# First boot and blessing of snapshot 1, a transaction, the user's changes
# after it, and the first boot of snapshot 2 with those changes carried over.
scenario_etc_sync() {
  LEVEL=paranoid
  setup "etc-sync"
  echo "== $SCENARIO (work: $WORK)"
  make_blank_target
  make_answers_image paranoid "$WORK/answers.img" fat
  boot_medium "$WORK/answers.img"
  expect 'answer file is in use' 300
  wait_installed
  stop_after_install
  first_boot paranoid

  # The first boot is blessed and recorded good by the boot check.
  check "the boot check is active" '^active$' \
    "for i in \$(seq 1 60); do systemctl is-active tetsu-boot-check && break; sleep 1; done; systemctl is-active tetsu-boot-check" 120
  check "snapshot 1 is recorded good" '"boot": "good"' \
    "for i in \$(seq 1 30); do sudo -n grep -q good /var/lib/tetsu/transactions/1.json && break; sleep 1; done; sudo -n cat /var/lib/tetsu/transactions/1.json" 90
  check "snapshot 1 is the initial install" '"kind": "initial_install"' \
    "sudo -n cat /var/lib/tetsu/transactions/1.json"
  # The hook is in the initrd the installer built.
  check "the initrd holds the sync binary" 'usr/bin/tetsu-etc-sync$' \
    "sudo -n lsinitcpio /usr/lib/tetsu/initrd.img | grep tetsu-etc-sync"
  check "the initrd holds the sync unit" 'tetsu-etc-sync\.service$' \
    "sudo -n lsinitcpio /usr/lib/tetsu/initrd.img | grep tetsu-etc-sync"

  # A transaction, then the user's changes made after it. An update, not an
  # install: a fresh system has no sync databases and install never fetches
  # them (no partial upgrade), while update does and always commits.
  reprime_sudo
  check "tetsu-tx update succeeds" 'update-rc=0' \
    "sudo -n tetsu-tx update >/tmp/tx.log 2>&1; echo update-rc=\$?; tail -n 5 /tmp/tx.log" 1500
  reprime_sudo
  gexec "echo SYNC-MARK-42 | sudo -n tee /etc/tetsu-vm-after-tx.conf >/dev/null; echo TETSU_VM=after | sudo -n tee -a /etc/environment >/dev/null; sudo -n useradd vmafter; echo changes-rc=\$?"
  assert_contains "$SCENARIO: the user's changes are made" "$GOUT" "changes-rc=0"
  check "the transaction is snapshot 2" 'tetsu-2' "sudo -n ls /efi/EFI/Linux"

  # Boot snapshot 2.
  reboot_guest
  boot_to_login
  login
  check "snapshot 2 is booted" 'subvol=@snapshots/2/root' "cat /proc/cmdline"
  check "the user's new file is carried over" 'SYNC-MARK-42' "cat /etc/tetsu-vm-after-tx.conf"
  check "the user's appended line is carried over" 'TETSU_VM=after' "cat /etc/environment"
  check "the user's account is carried over" 'uid=[0-9]+\(vmafter\)' "id vmafter"
  check "snapshot 2 is the update" '"kind": "update"' \
    "sudo -n cat /var/lib/tetsu/transactions/2.json"
  check "the initrd ran the sync" 'tetsu-etc-sync: synced' \
    "sudo -n journalctl -b -u tetsu-etc-sync --no-pager"
  check "the sync report names the user's file" 'tetsu-vm-after-tx\.conf' \
    "sudo -n cat /var/lib/tetsu/etc-sync/2/report.json"
  check "the boot check is active on snapshot 2" '^active$' \
    "for i in \$(seq 1 60); do systemctl is-active tetsu-boot-check && break; sleep 1; done; systemctl is-active tetsu-boot-check" 120
  check "snapshot 2 is blessed (no boot counter in its name)" '^tetsu-2\.efi$' \
    "for i in \$(seq 1 30); do sudo -n ls -1 /efi/EFI/Linux | grep -qx tetsu-2.efi && break; sleep 2; done; sudo -n ls -1 /efi/EFI/Linux" 120
  check "snapshot 2 is recorded good" '"boot": "good"' \
    "sudo -n cat /var/lib/tetsu/transactions/2.json"

  # Once more: the marker says the sync is done.
  reboot_guest
  boot_to_login
  login
  check "snapshot 2 is booted again" 'subvol=@snapshots/2/root' "cat /proc/cmdline"
  check "the sync is skipped by its marker" 'tetsu-etc-sync: skipped: the marker is present' \
    "sudo -n journalctl -b -u tetsu-etc-sync --no-pager"
  check "the user's file is still there" 'SYNC-MARK-42' "cat /etc/tetsu-vm-after-tx.conf"
  poweroff_guest
  stop_vm
}

# ------------------------------------------------------------------ main
usage() {
  sed -n '2,11p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
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
    answers)
      case "${1:-}" in paranoid | comfort | balanced) scenario_answers "$1" ;; *) usage; exit 64 ;; esac
      ;;
    interactive) scenario_interactive "${1:-balanced}" ;;
    sb-off) scenario_sb_off ;;
    etc-sync) scenario_etc_sync ;;
    all)
      scenario_answers paranoid
      scenario_answers comfort
      scenario_answers balanced
      scenario_interactive balanced
      scenario_sb_off
      scenario_etc_sync
      ;;
    *) usage; exit 64 ;;
  esac
  summary
}

main "$@"
