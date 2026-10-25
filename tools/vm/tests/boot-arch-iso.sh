#!/usr/bin/env bash
# Integration test: boot the stock Arch ISO under OVMF Secure Boot + swtpm,
# wait for the TPM device and a login prompt, then cut the power.
# Exit 77 (skip) without a usable /dev/kvm or without the ISO.
set -uo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
VM="$HERE/../tetsu-vm"
# shellcheck source=lib.sh
. "$HERE/lib.sh"

ISO=${TETSU_ARCH_ISO:-${XDG_CACHE_HOME:-$HOME/.cache}/tetsu/vm/archlinux-x86_64.iso}

if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  echo "SKIP: /dev/kvm is not usable"
  exit 77
fi
if [ ! -r "$ISO" ]; then
  echo "SKIP: no ISO at $ISO (run tools/vm/fetch-arch-iso or set TETSU_ARCH_ISO)"
  exit 77
fi

STATE=$(mktemp -d)
cleanup() {
  if [ "$TESTS_FAILED" -gt 0 ] && [ -r "$STATE/serial.log" ]; then
    echo "--- serial log (last 60 lines, escapes shown) ---"
    tail -n 60 "$STATE/serial.log" | cat -v
  fi
  "$VM" power-cut --state "$STATE" >/dev/null 2>&1 || true
  rm -rf "$STATE"
}
trap cleanup EXIT

if "$VM" start --iso "$ISO" --state "$STATE" --fresh --serial-console --snapshot; then
  pass "start"
else
  fail "start"; summary; exit 1
fi
QPID=$(cat "$STATE/qemu.pid" 2>/dev/null || true)
SPID=$(cat "$STATE/swtpm.pid" 2>/dev/null || true)

assert_status "wait-for: TPM device" 0 "$VM" wait-for --state "$STATE" --timeout 300 'Found device /dev/tpm0'
assert_status "wait-for: login prompt" 0 "$VM" wait-for --state "$STATE" --timeout 300 'login:'
assert_status "status: running" 0 "$VM" status --state "$STATE"
assert_status "power-cut" 0 "$VM" power-cut --state "$STATE"

assert_gone "qemu gone" "${QPID:-0}"
assert_gone "swtpm gone" "${SPID:-0}"
summary
