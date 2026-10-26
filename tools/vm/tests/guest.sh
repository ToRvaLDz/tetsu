#!/usr/bin/env bash
# Shared helpers of the VM acceptance tests (install.sh, tx.sh): the serial
# console, the guest shell, the media builders and the boot steps. Source it
# after setting WORK_ROOT; a scenario starts with `setup NAME`.

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM="$HERE/../tetsu-vm"
REPO=$(cd "$HERE/../../.." && pwd)
# shellcheck source=lib.sh
. "$HERE/lib.sh"
# shellcheck source=../lib/wait.sh
. "$HERE/../lib/wait.sh"

IMAGE=${TETSU_INSTALLER_IMAGE:-$REPO/target/image/tetsu-installer.raw}

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
# make_answers_image LEVEL OUT [fat]: an ext4 volume TETSU-ANSWERS holding
# answers.toml, or with "fat" a FAT one labelled TETSU-ANS (a FAT label holds
# 11 characters).
make_answers_image() {
  local level=$1 out=$2 fs=${3:-ext4} dir="$WORK/answers.d"
  rm -rf "$dir"
  mkdir -p "$dir"
  sed "s/@DISK_BY_ID@/virtio-tetsu-target/" "$HERE/answers/$level.toml" >"$dir/answers.toml"
  rm -f "$out"
  truncate -s 8M "$out"
  if [ "$fs" = fat ]; then
    mkfs.vfat -n TETSU-ANS "$out" >/dev/null
    mcopy -i "$out" "$dir/answers.toml" ::answers.toml
  else
    mkfs.ext4 -q -L TETSU-ANSWERS -d "$dir" "$out"
  fi
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

poweroff_guest() {
  "$VM" send --state "$STATE" 'sudo -n systemctl poweroff\n'
  local i
  for i in $(seq 1 90); do vm_running || return 0; sleep 1; done
  "$VM" stop --state "$STATE" >/dev/null 2>&1 || true
}

# Prime the sudo ticket again: a long step outlives the first one.
reprime_sudo() {
  gexec "echo '$USER_PW' | sudo -S -p '' -v 2>&1; echo sudo-primed"
  assert_contains "$SCENARIO: sudo is primed again" "$GOUT" "sudo-primed"
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
  check "tetsu-base is explicitly installed" 'Install Reason +: Explicitly installed' \
    "pacman -Qi tetsu-base"
  check "the primary group is named after the user" "$USER_NAME\$" "id -gn"
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
