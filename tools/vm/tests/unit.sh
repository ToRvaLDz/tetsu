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

  out=$("$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" --iso "$SCRATCH/boot.iso" --snapshot 2>&1)
  assert_contains "dry-run: --snapshot on the disk drive" "$out" "file=$SCRATCH/disk.img,if=virtio,snapshot=on"
  assert_not_contains "dry-run: no global -snapshot (would hit VARS)" "$out" " -snapshot"
  assert_not_contains "dry-run: iso drive not snapshotted" "$out" "readonly=on,snapshot"

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

test_dry_run_target_disk() {
  local st="$SCRATCH/dry" out
  : >"$SCRATCH/target.img"
  out=$("$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" \
    --target-disk "$SCRATCH/target.img" --snapshot 2>&1)
  assert_contains "target-disk: second virtio drive" "$out" "file=$SCRATCH/target.img,if=none,id=target0"
  assert_contains "target-disk: known serial for the by-id link" "$out" "virtio-blk-pci,drive=target0,serial=tetsu-target"
  assert_not_contains "target-disk: never snapshotted" "$out" "target.img,if=none,id=target0,snapshot"
  assert_contains "target-disk: boot disk still snapshotted" "$out" "file=$SCRATCH/disk.img,if=virtio,snapshot=on"
  assert_status "target-disk: needs a boot disk or iso (usage 64)" 64 \
    "$VM" start --dry-run --state "$st" --target-disk "$SCRATCH/target.img"
  assert_status "target-disk: missing file" 1 \
    "$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" --target-disk "$SCRATCH/none.img"
}

test_dry_run_extra_disk() {
  local st="$SCRATCH/dry" out
  : >"$SCRATCH/answers.img"
  : >"$SCRATCH/more.img"
  out=$("$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" --snapshot \
    --extra-disk "$SCRATCH/answers.img" --extra-disk "$SCRATCH/more.img" 2>&1)
  assert_contains "extra-disk: first drive and serial" "$out" \
    "file=$SCRATCH/answers.img,if=none,id=extra1 -device virtio-blk-pci,drive=extra1,serial=tetsu-extra1"
  assert_contains "extra-disk: repeatable" "$out" "drive=extra2,serial=tetsu-extra2"
  assert_not_contains "extra-disk: never snapshotted" "$out" "answers.img,if=none,id=extra1,snapshot"
  assert_status "extra-disk: needs a boot disk or iso (usage 64)" 64 \
    "$VM" start --dry-run --state "$st" --extra-disk "$SCRATCH/answers.img"
  assert_status "extra-disk: missing file" 1 \
    "$VM" start --dry-run --state "$st" --disk "$SCRATCH/disk.img" --extra-disk "$SCRATCH/none.img"
}

test_dry_run_errors() {
  assert_status "start: needs disk or iso (usage 64)" 64 "$VM" start --dry-run --state "$SCRATCH/dry"
  assert_status "start: missing iso file" 1 "$VM" start --dry-run --state "$SCRATCH/dry" --iso "$SCRATCH/none.iso"
  assert_status "start: unknown flag (usage 64)" 64 "$VM" start --dry-run --bogus
}

test_usage_errors() {
  assert_status "usage: unknown command is 64" 64 "$VM" frobnicate
  assert_status "usage: no command is 64" 64 "$VM"
  assert_status "usage: wait-for without regex is 64" 64 "$VM" wait-for --state "$SCRATCH/none"
  assert_status "usage: status bad option is 64" 64 "$VM" status --bogus
  assert_status "usage: power-cut bad timeout is 64" 64 "$VM" stop --timeout x
  assert_status "usage: start --mem not a number is 64" 64 "$VM" start --dry-run --iso "$SCRATCH/boot.iso" --mem x
  assert_status "usage: send without text is 64" 64 "$VM" send --state "$SCRATCH/none"
  # Invalid regex: fail fast with 64, not poll until the timeout.
  local code=0
  timeout 10 "$VM" wait-for --state "$SCRATCH/none" --timeout 60 '(' >/dev/null 2>&1 || code=$?
  assert_eq "wait-for: invalid regex exits 64 quickly" 64 "$code"
}

# ---------------------------------------------------------------- stale swtpm
test_stale_swtpm() {
  local st="$SCRATCH/stale" t
  export TETSU_VM_SWTPM_COMM=sleep TETSU_VM_QEMU_COMM=nomatch
  mkdir -p "$st"
  sleep 60 & t=$!
  echo "$t" >"$st/swtpm.pid"

  local out
  out=$("$VM" status --state "$st" 2>&1) || true
  assert_contains "status: reports a stale swtpm" "$out" "stale swtpm"
  assert_status "status: stale swtpm is still non-zero" 1 "$VM" status --state "$st"

  # Seam: the helper start uses before touching tpm/ and the sockets.
  (
    VM_DIR=$(cd "$HERE/.." && pwd)
    # shellcheck source=../lib/common.sh
    . "$VM_DIR/lib/common.sh"
    # shellcheck source=../lib/start.sh
    . "$VM_DIR/lib/start.sh"
    terminate_stale_swtpm "$st"
  )
  wait "$t" 2>/dev/null
  assert_gone "start: leftover swtpm terminated" "$t"

  # An unrelated process behind swtpm.pid must survive.
  export TETSU_VM_SWTPM_COMM=nomatch
  sleep 60 & t=$!
  echo "$t" >"$st/swtpm.pid"
  (
    VM_DIR=$(cd "$HERE/.." && pwd)
    . "$VM_DIR/lib/common.sh"
    . "$VM_DIR/lib/start.sh"
    terminate_stale_swtpm "$st"
  )
  if kill -0 "$t" 2>/dev/null; then pass "start: unrelated pid untouched"
  else fail "start: unrelated pid untouched"; fi
  kill "$t" 2>/dev/null
  unset TETSU_VM_SWTPM_COMM TETSU_VM_QEMU_COMM
}

# ---------------------------------------------------------------- send
test_send() {
  if ! command -v socat >/dev/null 2>&1; then
    echo "skip send tests: socat not installed"
    return 0
  fi
  local st="$SCRATCH/send" got want
  mkdir -p "$st"
  # A fake serial socket that records everything written to it.
  socat "UNIX-LISTEN:$st/serial.sock,fork" "OPEN:$st/received,creat,append" &
  local lpid=$!
  local tries=50
  while [ ! -S "$st/serial.sock" ] && [ "$tries" -gt 0 ]; do sleep 0.1; tries=$((tries - 1)); done

  assert_status "send: succeeds" 0 "$VM" send --state "$st" 'root\n'
  assert_status "send: second call, \\r escape" 0 "$VM" send --state "$st" 'a b\r'
  sleep 0.5
  got=$(od -An -c "$st/received" | tr -s ' ' | tr -d '\n')
  want=' r o o t \n a b \r'
  assert_eq "send: escapes interpreted" "$want" "$got"
  kill "$lpid" 2>/dev/null; wait "$lpid" 2>/dev/null

  assert_status "send: no VM is an error" 1 "$VM" send --state "$SCRATCH/none" 'x'
}

# ---------------------------------------------------------------- send drain
# QEMU writes the guest's serial output one byte at a time; a client that
# never reads makes those writes fail with EAGAIN, and QEMU's chardev logfile
# then records the retried byte twice. A fake server emits 1-byte writes
# while `send` holds the connection and counts the EAGAINs.
test_send_drains() {
  if ! command -v socat >/dev/null 2>&1 || ! command -v python3 >/dev/null 2>&1; then
    echo "skip send drain test: socat or python3 not installed"
    return 0
  fi
  local st="$SCRATCH/drain" lpid tries=50 blocked
  mkdir -p "$st"
  python3 -I - "$st/serial.sock" "$st/blocked" <<'PY' &
import socket, sys, time

srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
srv.bind(sys.argv[1])
srv.listen(1)
conn, _ = srv.accept()
conn.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 4096)
conn.setblocking(False)
blocked = 0
deadline = time.time() + 0.25
while time.time() < deadline:
    try:
        conn.send(b"x")
    except BlockingIOError:
        blocked += 1
    except OSError:
        break
    time.sleep(0.001)
with open(sys.argv[2], "w") as f:
    f.write(str(blocked))
PY
  lpid=$!
  while [ ! -S "$st/serial.sock" ] && [ "$tries" -gt 0 ]; do sleep 0.1; tries=$((tries - 1)); done
  assert_status "send drain: succeeds" 0 "$VM" send --state "$st" 'echo hi\n'
  wait "$lpid" 2>/dev/null
  blocked=$(cat "$st/blocked" 2>/dev/null || echo missing)
  assert_eq "send drain: guest output never blocked on the socket" 0 "$blocked"
}


test_dry_run
test_send
test_send_drains
test_stale_swtpm
test_usage_errors
test_dry_run_smbios
test_dry_run_target_disk
test_dry_run_extra_disk
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

  # Real systemd status line: colour codes inside the message, OSC and cursor queries.
  printf '[\033[0;32m  OK  \033[0m] Found device \033[0;1;39m/dev/tpm0\033[0m.\r\r\n\033]104\033\\\033[6n\n' >>"$st/serial.log"
  code=0; "$VM" wait-for --state "$st" --timeout 5 'Found device /dev/tpm0' || code=$?
  assert_eq "wait-for: match across colour codes" 0 "$code"

  # zsh redraws the prompt with charset designations (ESC ( B) between words.
  printf '\033(B\033[mroot\033(B@archiso ~ \033(B#\n' >>"$st/serial.log"
  code=0; "$VM" wait-for --state "$st" --timeout 5 'root@archiso ~ #' || code=$?
  assert_eq "wait-for: match across charset escapes" 0 "$code"

  kill "$pid"; wait "$pid" 2>/dev/null
  code=0; "$VM" wait-for --state "$st" --timeout 5 'never appears' || code=$?
  assert_eq "wait-for: dead VM is 2" 2 "$code"

  printf 'late line\n' >>"$st/serial.log"
  code=0; "$VM" wait-for --state "$st" --timeout 5 'late line' || code=$?
  assert_eq "wait-for: match wins over dead VM" 0 "$code"
  unset TETSU_VM_QEMU_COMM
}

test_wait_for

# ---------------------------------------------------------------- power-cut
test_power_cut() {
  local st="$SCRATCH/pc" q t
  export TETSU_VM_QEMU_COMM=sleep TETSU_VM_SWTPM_COMM=sleep
  mkdir -p "$st"
  sleep 60 & q=$!
  sleep 60 & t=$!
  echo "$q" >"$st/qemu.pid"
  echo "$t" >"$st/swtpm.pid"

  assert_status "status: running" 0 "$VM" status --state "$st"
  assert_status "power-cut: succeeds" 0 "$VM" power-cut --state "$st"
  wait "$q" "$t" 2>/dev/null
  assert_gone "power-cut: qemu gone" "$q"
  assert_gone "power-cut: swtpm gone" "$t"
  assert_status "status: stopped after power-cut" 1 "$VM" status --state "$st"
  assert_status "power-cut: idempotent" 0 "$VM" power-cut --state "$st"

  # Stale pidfile: the pid belongs to an unrelated process (wrong command name).
  export TETSU_VM_QEMU_COMM=nomatch TETSU_VM_SWTPM_COMM=nomatch
  sleep 60 & q=$!
  echo "$q" >"$st/qemu.pid"
  assert_status "power-cut: stale pidfile is a no-op" 0 "$VM" power-cut --state "$st"
  if kill -0 "$q" 2>/dev/null; then pass "power-cut: unrelated pid untouched"
  else fail "power-cut: unrelated pid untouched"; fi
  kill "$q" 2>/dev/null
  assert_status "status: stale pidfile is stopped" 1 "$VM" status --state "$st"
  unset TETSU_VM_QEMU_COMM TETSU_VM_SWTPM_COMM
}

test_power_cut

# ------------------------------------------------------- install.sh usage
test_install_usage() {
  local inst="$HERE/install.sh"
  assert_status "install.sh: help" 0 "$inst" help
  assert_contains "install.sh: help lists the scenarios" "$OUT" "answers paranoid|comfort|balanced"
  assert_status "install.sh: no scenario (usage 64)" 64 "$inst"
  # An unknown scenario is a usage error where the host can run the test at
  # all; elsewhere the test skips (77) before looking at its arguments.
  local got=0
  "$inst" bogus >/dev/null 2>&1 || got=$?
  case "$got" in 64 | 77) pass "install.sh: unknown scenario" ;; *) fail "install.sh: unknown scenario" "exit $got" ;; esac
}

test_install_usage

summary
