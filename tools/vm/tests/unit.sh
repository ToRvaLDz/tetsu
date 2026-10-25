#!/usr/bin/env bash
# Unit tests for tetsu-vm. Needs no ISO, no KVM, no qemu and no swtpm.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM="$HERE/../tetsu-vm"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT

# Fake firmware and media so discovery succeeds on any machine.
: >"$SCRATCH/CODE.fd"
: >"$SCRATCH/VARS.fd"
: >"$SCRATCH/disk.img"
: >"$SCRATCH/boot.iso"
export TETSU_OVMF_CODE="$SCRATCH/CODE.fd"
export TETSU_OVMF_VARS="$SCRATCH/VARS.fd"

# ---------------------------------------------------------------- dry-run
test_dry_run() {
  local st="$SCRATCH/dry" out
  out=$(TETSU_VM_KVM=1 "$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" \
    --iso "$SCRATCH/boot.iso" 2>&1)
  assert_contains "dry-run: q35 + smm" "$out" "q35,smm=on,accel=kvm"
  assert_contains "dry-run: kvm cpu host" "$out" "-cpu host"
  assert_contains "dry-run: secure pflash" "$out" "driver=cfi.pflash01,property=secure,value=on"
  assert_contains "dry-run: readonly CODE" "$out" "if=pflash,format=raw,unit=0,readonly=on,file=$SCRATCH/CODE.fd"
  assert_contains "dry-run: per-VM VARS" "$out" "if=pflash,format=raw,unit=1,file=$st/vars.fd"
  assert_contains "dry-run: tpm-crb" "$out" "tpm-crb,tpmdev=tpm0"
  assert_contains "dry-run: swtpm socket" "$out" "path=$st/swtpm.sock"
  assert_contains "dry-run: virtio disk" "$out" "file=$SCRATCH/disk.img,if=virtio"
  assert_contains "dry-run: iso cdrom" "$out" "file=$SCRATCH/boot.iso,media=cdrom,readonly=on"
  assert_not_contains "dry-run: no -snapshot by default" "$out" "-snapshot"
  assert_contains "dry-run: serial unix socket + logfile" "$out" "logfile=$st/serial.log"
  assert_contains "dry-run: serial socket path" "$out" "path=$st/serial.sock"
  assert_contains "dry-run: qmp socket" "$out" "$st/qmp.sock"
  assert_contains "dry-run: pidfile" "$out" "-pidfile $st/qemu.pid"
  assert_contains "dry-run: daemonize" "$out" "-daemonize"
  [ ! -e "$st" ] && pass "dry-run: creates no state" || fail "dry-run: creates no state"

  out=$(TETSU_VM_KVM=0 "$VM" start --dry-run --state "$st" --iso "$SCRATCH/boot.iso" 2>&1)
  assert_contains "dry-run: tcg without kvm" "$out" "q35,smm=on,accel=tcg"
  assert_contains "dry-run: cpu max without kvm" "$out" "-cpu max"

  out=$("$VM" start --dry-run --state "$st" --iso "$SCRATCH/boot.iso" --snapshot 2>&1)
  assert_contains "dry-run: --snapshot" "$out" "-snapshot"

  out=$("$VM" start --dry-run --state "$st" --iso "$SCRATCH/boot.iso" --mem 4096 --cpus 4 2>&1)
  assert_contains "dry-run: --mem" "$out" "-m 4096"
  assert_contains "dry-run: --cpus" "$out" "-smp 4"
}

test_dry_run_smbios() {
  local st="$SCRATCH/dry" out
  out=$("$VM" start --dry-run --state "$st" --iso "$SCRATCH/boot.iso" --serial-console 2>&1)
  assert_contains "smbios: serial cmdline, commas doubled" "$out" \
    "type=11,value=io.systemd.boot.kernel-cmdline-extra=console=ttyS0,,115200"
  assert_contains "smbios: serial getty credential" "$out" \
    "type=11,value=io.systemd.credential:getty.ttys.serial=ttyS0"

  out=$("$VM" start --dry-run --state "$st" --iso "$SCRATCH/boot.iso" \
    --cmdline-extra "a=1,2 b" --credential "x.y=1,2" 2>&1)
  assert_contains "smbios: cmdline-extra commas doubled" "$out" "kernel-cmdline-extra=a=1,,2"
  assert_contains "smbios: credential commas doubled" "$out" "io.systemd.credential:x.y=1,,2"
}

test_dry_run_errors() {
  assert_status "start: needs disk or iso" 1 "$VM" start --dry-run --state "$SCRATCH/dry"
  assert_status "start: missing iso file" 1 "$VM" start --dry-run --state "$SCRATCH/dry" --iso "$SCRATCH/none.iso"
  assert_status "start: unknown flag" 1 "$VM" start --dry-run --bogus
}

test_dry_run
test_dry_run_smbios
test_dry_run_errors

# ---------------------------------------------------------------- wait-for
# A fake state dir whose qemu.pid names a live dummy process.
fake_state() {
  local st=$1
  mkdir -p "$st"
  : >"$st/serial.log"
  sleep 60 &
  FAKE_PID=$!
  echo "$FAKE_PID" >"$st/qemu.pid"
}

test_wait_for() {
  local st="$SCRATCH/wf" code
  export TETSU_VM_QEMU_COMM=sleep
  fake_state "$st"
  local pid=$FAKE_PID

  (sleep 1; printf '\033[1;32mOK\033[0m Reached\r target\r\n' >>"$st/serial.log") &
  code=0; "$VM" wait-for --state "$st" --timeout 10 'Reached target$' || code=$?
  assert_eq "wait-for: match through ANSI and CR" 0 "$code"

  code=0; "$VM" wait-for --state "$st" --timeout 2 'never appears' || code=$?
  assert_eq "wait-for: timeout is 1" 1 "$code"

  kill "$pid"; wait "$pid" 2>/dev/null
  code=0; "$VM" wait-for --state "$st" --timeout 5 'never appears' || code=$?
  assert_eq "wait-for: dead VM is 2" 2 "$code"

  printf 'late line\n' >>"$st/serial.log"
  code=0; "$VM" wait-for --state "$st" --timeout 5 'late line' || code=$?
  assert_eq "wait-for: match wins over dead VM" 0 "$code"
  unset TETSU_VM_QEMU_COMM
}

test_wait_for

summary
