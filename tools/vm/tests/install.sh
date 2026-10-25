#!/usr/bin/env bash
# Acceptance test of the Tetsu installer: install from the medium in a VM
# (OVMF Secure Boot + swtpm), boot the result, check it, reboot, check again.
#
#   tools/vm/tests/install.sh answers paranoid|comfort|balanced
#   tools/vm/tests/install.sh interactive [comfort|balanced]   (default balanced)
#   tools/vm/tests/install.sh sb-off
#   tools/vm/tests/install.sh all
#
# Needs the image (image/build), /dev/kvm, qemu, swtpm, socat, mkfs.ext4,
# sgdisk. Exit 77 (skip) without the image or KVM. Work files go to
# target/vm-install/<scenario>/ (TETSU_INSTALL_WORK); the serial logs of every
# phase stay there. Test-only secrets below are the ones in answers/*.toml.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM="$HERE/../tetsu-vm"
REPO=$(cd "$HERE/../../.." && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=../lib/wait.sh
. "$HERE/../lib/wait.sh"

IMAGE=${TETSU_INSTALLER_IMAGE:-$REPO/target/image/tetsu-installer.raw}
WORK_ROOT=${TETSU_INSTALL_WORK:-$REPO/target/vm-install}
MEM=3072

# Test-only secrets (answers/*.toml hold the hash of USER_PW).
PASSPHRASE='Tetsu-Vm-Test-Passphrase-1'
USER_NAME=tester
USER_PW='tetsu-vm-test-pw'
PIN='4711-Vm-Pin'
GLOBAL_GUID=8be4df61-93ca-11d2-aa0d-00e098032b8c

# Per-scenario state (set by setup).
SCENARIO="" LEVEL="" WORK="" STATE="" TARGET="" MARK=0 GSEQ=0 GOUT="" GRC=0
OVMF_CODE_PLAIN=${TETSU_OVMF_CODE_PLAIN:-/usr/share/edk2/x64/OVMF_CODE.4m.fd}

say_info() { printf '  .. %s\n' "$*"; }
evidence() { printf '     > %s\n' "$*"; }

# -------------------------------------------------------------- failure
tail_log() {
  [ -r "$STATE/serial.log" ] || return 0
  echo "--- serial log of $SCENARIO (last 60 lines, cleaned) ---"
  clean_serial "$STATE/serial.log" | tail -n 60
}

stop_vm() {
  [ -n "$STATE" ] || return 0
  "$VM" power-cut --state "$STATE" >/dev/null 2>&1 || true
}

die() {
  printf 'FATAL %s: %s\n' "$SCENARIO" "$*" >&2
  tail_log >&2
  stop_vm
  exit 1
}

# ------------------------------------------------------------- serial io
log_size() { stat -c %s "$STATE/serial.log" 2>/dev/null || echo 0; }
mark() { MARK=$(log_size); }

# Cleaned serial text written after MARK.
since_mark() {
  local tmp="$WORK/since.tmp"
  tail -c +$((MARK + 1)) "$STATE/serial.log" >"$tmp" 2>/dev/null || : >"$tmp"
  clean_serial "$tmp"
}

vm_running() { "$VM" status --state "$STATE" >/dev/null 2>&1; }

# expect REGEX [TIMEOUT]: wait for REGEX in the output after MARK; the last
# matching line goes to EXPECT_LINE.
EXPECT_LINE=""
expect() {
  local re=$1 timeout=${2:-120} deadline text
  deadline=$((SECONDS + timeout))
  while :; do
    text=$(since_mark)
    if LC_ALL=C grep -aEq -- "$re" <<<"$text"; then
      EXPECT_LINE=$(LC_ALL=C grep -aE -- "$re" <<<"$text" | tail -n 1)
      return 0
    fi
    vm_running || die "the VM stopped while waiting for /$re/"
    [ "$SECONDS" -lt "$deadline" ] || die "timeout (${timeout}s) waiting for /$re/"
    sleep 0.5
  done
}

# Like expect, but returns 1 on timeout instead of dying.
expect_soon() {
  local re=$1 timeout=${2:-5} deadline text
  deadline=$((SECONDS + timeout))
  while [ "$SECONDS" -lt "$deadline" ]; do
    text=$(since_mark)
    if LC_ALL=C grep -aEq -- "$re" <<<"$text"; then return 0; fi
    sleep 0.5
  done
  return 1
}

# say TEXT: type a line. The mark moves first, so the next expect only sees
# what the guest prints afterwards. Backslashes are doubled for printf %b.
say() {
  mark
  "$VM" send --state "$STATE" "${1//\\/\\\\}\\n"
}

# Secrets are read with echo off; the terminal mode changes just after the
# prompt, and a change in between would drop the input, so wait a moment.
say_secret() {
  sleep 1
  say "$1"
}

# ---------------------------------------------------------- guest shell
# gexec CMD: run CMD in the guest shell; output in GOUT, status in GRC.
gexec() {
  GSEQ=$((GSEQ + 1))
  local n=$((7000 + GSEQ)) cmd=$1
  mark
  "$VM" send --state "$STATE" "echo BEG-\$(($n)); ${cmd//\\/\\\\}; echo END-\$(($n))-\$?\\n"
  expect "END-$n-[0-9]+\$" "${2:-60}"
  GRC=${EXPECT_LINE##*-}
  GOUT=$(since_mark | awk -v b="BEG-$n" -v e="END-$n-" '
    substr($0, length($0) - length(b) + 1) == b { on = 1; next }
    index($0, e) { on = 0 }
    on { print }')
}

# check NAME REGEX CMD: CMD (as the user) must print something matching REGEX.
check() {
  local name=$1 re=$2
  gexec "$3" "${4:-60}"
  if LC_ALL=C grep -aEq -- "$re" <<<"$GOUT"; then
    pass "$SCENARIO: $name"
    evidence "$(LC_ALL=C grep -aE -- "$re" <<<"$GOUT" | head -n 1)"
  else
    fail "$SCENARIO: $name" "wanted /$re/ from: $3 -- got: $GOUT"
  fi
}

# check_not NAME REGEX CMD
check_not() {
  local name=$1 re=$2
  gexec "$3" "${4:-60}"
  if LC_ALL=C grep -aEq -- "$re" <<<"$GOUT"; then
    fail "$SCENARIO: $name" "unexpected /$re/ in: $GOUT"
  else
    pass "$SCENARIO: $name"
  fi
}

login() {
  expect 'login: ?$' 300
  say "$USER_NAME"
  expect 'Password: ?$' 30
  say_secret "$USER_PW"
  expect "$USER_NAME@tetsu-[a-z0-9]+ [^]]*\\]\\$ ?\$" 60
  # The installed locale is the user's; the checks read English.
  gexec "export LC_ALL=C LANG=C"
  # Prime sudo; -n afterwards, so a missing ticket fails instead of hanging.
  gexec "echo '$USER_PW' | sudo -S -p '' -v 2>&1; echo sudo-primed"
  assert_contains "$SCENARIO: sudo works for the session password" "$GOUT" "sudo-primed"
}

# --------------------------------------------------------------- media
# make_answers_image LEVEL OUT: an ext4 volume TETSU-ANSWERS holding answers.toml
# (FAT cannot carry a 13 character label).
make_answers_image() {
  local level=$1 out=$2 dir="$WORK/answers.d"
  rm -rf "$dir"
  mkdir -p "$dir"
  sed "s/@DISK_BY_ID@/virtio-tetsu-target/" "$HERE/answers/$level.toml" >"$dir/answers.toml"
  rm -f "$out"
  truncate -s 8M "$out"
  mkfs.ext4 -q -L TETSU-ANSWERS -d "$dir" "$out"
}

make_blank_target() {
  rm -f "$TARGET"
  qemu-img create -q -f qcow2 "$TARGET" 16G
}

# A target that already holds an "operating system", so that the installer
# asks to retype the disk name.
make_used_target() {
  local raw="$WORK/used.raw" dir="$WORK/used.d"
  rm -rf "$dir" "$raw" "$TARGET"
  mkdir -p "$dir/etc"
  printf 'NAME="Old Linux"\nPRETTY_NAME="Old Linux"\nID=old\n' >"$dir/etc/os-release"
  truncate -s 16G "$raw"
  sgdisk -n 1:2048:+512M -t 1:8300 "$raw" >/dev/null
  mkfs.ext4 -q -E offset=$((2048 * 512)) -d "$dir" "$raw" 512M
  qemu-img convert -f raw -O qcow2 "$raw" "$TARGET"
  rm -f "$raw"
}

setup() {
  SCENARIO=$1
  WORK="$WORK_ROOT/$SCENARIO"
  STATE="$WORK/vm"
  TARGET="$WORK/target.qcow2"
  rm -rf "$WORK"
  mkdir -p "$WORK"
  MARK=0
  GSEQ=0
}

# archive_log TAG: keep the serial log of the phase that ends, start a new one.
archive_log() {
  cp "$STATE/serial.log" "$WORK/serial-$1.log"
  : >"$STATE/serial.log"
  MARK=0
}

# ----------------------------------------------------------------- boot
boot_medium() { # boot_medium [ANSWERS_IMG] : fresh state unless FRESH=0
  local args=(--state "$STATE" --disk "$IMAGE" --snapshot --target-disk "$TARGET"
    --serial-console --mem "$MEM")
  [ "${FRESH:-1}" = 0 ] || args+=(--fresh)
  [ -z "${1:-}" ] || args+=(--extra-disk "$1")
  "$VM" start "${args[@]}" >/dev/null || die "cannot start the medium"
  mark
}

# The medium's serial console: wait for the installer's outcome.
wait_installed() {
  expect 'Installation complete\.|install failed|tetsu-installer: ' 600
  case "$EXPECT_LINE" in
    *"Installation complete."*) pass "$SCENARIO: the installer reports success" ;;
    *) die "the installer failed: $EXPECT_LINE" ;;
  esac
  evidence "$EXPECT_LINE"
}

# Power the medium off the way a user pulling the plug after the message would.
stop_after_install() {
  "$VM" stop --state "$STATE" >/dev/null 2>&1 || true
  archive_log install
}

boot_installed() { # boot from the target only; same VARS and TPM
  "$VM" start --state "$STATE" --disk "$TARGET" --serial-console --mem "$MEM" \
    >/dev/null || die "cannot start the installed system"
  mark
}

# Answer the initrd prompts until the login prompt: the disk passphrase
# (the unlock, and the seal's own question) and the PIN. How many of each were
# asked ends up in SEEN_PASS and SEEN_PIN.
SEEN_PASS=0 SEEN_PIN=0
PASS_RE='Please enter passphrase for|Disk passphrase \(to seal'
PIN_RE='\bPIN\b'
boot_to_login() {
  local deadline=$((SECONDS + 420)) text
  SEEN_PASS=0 SEEN_PIN=0
  while [ "$SECONDS" -lt "$deadline" ]; do
    text=$(since_mark)
    if LC_ALL=C grep -aEq 'login: ?$' <<<"$text"; then return 0; fi
    if LC_ALL=C grep -aEq -- "$PIN_RE" <<<"$text"; then
      SEEN_PIN=$((SEEN_PIN + 1))
      say_info "PIN prompt: $(LC_ALL=C grep -aE -- "$PIN_RE" <<<"$text" | tail -n 1 | cut -c1-80)"
      say_secret "$PIN"
    elif LC_ALL=C grep -aEq -- "$PASS_RE" <<<"$text"; then
      SEEN_PASS=$((SEEN_PASS + 1))
      say_info "passphrase prompt: $(LC_ALL=C grep -aE -- "$PASS_RE" <<<"$text" | tail -n 1 | cut -c1-80)"
      say_secret "$PASSPHRASE"
    fi
    vm_running || die "the VM stopped before the login prompt"
    sleep 1
  done
  die "no login prompt within 420 s"
}

reboot_guest() {
  mark
  "$VM" send --state "$STATE" 'sudo -n systemctl reboot\n'
  expect 'reboot: Restarting system' 120
  mark
}

# ---------------------------------------------------------------- checks
common_checks() { # LEVEL
  local level=$1
  check "Secure Boot is enabled (SecureBoot efivar)" '(^| )6 +0 +0 +0 +1$' \
    "od -An -tu1 /sys/firmware/efi/efivars/SecureBoot-$GLOBAL_GUID"
  check "bootctl reports Secure Boot enabled (user)" 'Secure Boot: enabled \(user\)' \
    "bootctl status 2>/dev/null | grep -i 'secure boot'"
  check "the booted entry is Tetsu's UKI" 'tetsu-[0-9]+' \
    "bootctl status 2>/dev/null | grep -iE 'Current Entry|Selected Entry|id: ' | head -n 3"
  check "the kernel runs from the encrypted root" 'rd.luks.name=.* root=/dev/mapper/root' \
    "cat /proc/cmdline"
  check "the user is in wheel" 'wheel' "id -nG"
  check "the root account is locked" 'root +L' "sudo -n passwd -S root"
  case "$level" in
    paranoid)
      check "the level file says paranoid" 'level = "paranoid"' "cat /etc/tetsu/unlock-level.toml"
      check_not "no TPM2 token in the LUKS header" 'tpm2' \
        "sudo -n systemd-cryptenroll /dev/disk/by-partlabel/TETSU-ROOT"
      ;;
    comfort | balanced)
      check "the level file says sealed" 'state = "sealed"' "cat /etc/tetsu/unlock-level.toml"
      check "the LUKS header has a tpm2 token" 'tpm2' \
        "sudo -n systemd-cryptenroll /dev/disk/by-partlabel/TETSU-ROOT"
      ;;
  esac
}

poweroff_guest() {
  "$VM" send --state "$STATE" 'sudo -n systemctl poweroff\n'
  local i
  for i in $(seq 1 90); do vm_running || return 0; sleep 1; done
  "$VM" stop --state "$STATE" >/dev/null 2>&1 || true
}

# After the first boot: reboot and check how the disk unlocks.
second_boot_checks() { # LEVEL
  local level=$1
  reboot_guest
  boot_to_login
  case "$level" in
    paranoid)
      assert_eq "$SCENARIO: paranoid asks the passphrase once" "1" "$SEEN_PASS"
      ;;
    comfort)
      assert_eq "$SCENARIO: comfort unlocks through the TPM with no passphrase prompt" "0" "$SEEN_PASS"
      assert_eq "$SCENARIO: comfort asks no PIN" "0" "$SEEN_PIN"
      ;;
    balanced)
      assert_eq "$SCENARIO: balanced asks no passphrase" "0" "$SEEN_PASS"
      assert_eq "$SCENARIO: balanced asks the PIN once" "1" "$SEEN_PIN"
      ;;
  esac
  login
  check "still booted under Secure Boot after the reboot" '(^| )6 +0 +0 +0 +1$' \
    "od -An -tu1 /sys/firmware/efi/efivars/SecureBoot-$GLOBAL_GUID"
}

# First boot of an installed system, by level.
first_boot() { # LEVEL
  boot_installed
  boot_to_login
  say_info "first boot: $SEEN_PASS passphrase prompt(s), $SEEN_PIN PIN prompt(s)"
  login
  common_checks "$1"
}

# --------------------------------------------------------------- scenarios
scenario_answers() {
  local level=$1
  LEVEL=$level
  setup "answers-$level"
  echo "== $SCENARIO (work: $WORK)"
  make_blank_target
  make_answers_image "$level" "$WORK/answers.img"
  boot_medium "$WORK/answers.img"
  expect 'TETSU-ANSWERS|answer file is in use' 300
  wait_installed
  stop_after_install
  first_boot "$level"
  second_boot_checks "$level"
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
  expect_soon 'Secure Boot stays off|not in Setup Mode' 5 || \
    fail "$SCENARIO: the installer says keys are kept and Secure Boot stays off"
  pass "$SCENARIO: the installer keeps the keys and leaves Secure Boot off"
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

# ------------------------------------------------------------------ main
usage() {
  sed -n '2,10p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

preflight() {
  if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then echo "SKIP: /dev/kvm is not usable"; exit 77; fi
  if [ ! -r "$IMAGE" ]; then echo "SKIP: no installer image at $IMAGE (run image/build)"; exit 77; fi
  local t
  for t in qemu-img qemu-system-x86_64 swtpm socat mkfs.ext4 sgdisk; do
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
    all)
      scenario_answers paranoid
      scenario_answers comfort
      scenario_answers balanced
      scenario_interactive balanced
      scenario_sb_off
      ;;
    *) usage; exit 64 ;;
  esac
  summary
}

main "$@"
