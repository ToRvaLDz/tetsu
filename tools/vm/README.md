# tetsu-vm: QEMU test harness

Boots an image or ISO in QEMU with OVMF Secure Boot and a software TPM 2.0
(swtpm), keeps all per-VM state in one directory, and lets tests drive the VM
from the serial log. It is plain Bash; nothing besides the tools below is needed.

## Prerequisites

| Distro        | Packages                                                        |
|---------------|-----------------------------------------------------------------|
| Arch          | `qemu-system-x86 edk2-ovmf swtpm socat curl`                    |
| Debian/Ubuntu | `qemu-system-x86 ovmf swtpm socat curl`                         |
| Fedora        | `qemu-system-x86 edk2-ovmf swtpm socat curl`                    |

`/dev/kvm` is used when readable and writable, otherwise QEMU falls back to TCG
(`-cpu max`, slow). Firmware is discovered at:

- Arch: `/usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd` + `OVMF_VARS.4m.fd`
- Debian/Ubuntu: `/usr/share/OVMF/OVMF_CODE_4M.secboot.fd` + `OVMF_VARS_4M.fd`
- Fedora: `/usr/share/edk2/ovmf/OVMF_CODE.secboot.fd` + `OVMF_VARS.fd`

Override with `TETSU_OVMF_CODE` and `TETSU_OVMF_VARS`. The default VARS is the
distro's empty template, so the VM starts in Secure Boot setup mode.
`socat` is only needed by `stop` and `console`.

## Boot the stock Arch ISO

```sh
tools/vm/fetch-arch-iso
tools/vm/tetsu-vm start --iso ~/.cache/tetsu/vm/archlinux-x86_64.iso --serial-console
tools/vm/tetsu-vm wait-for 'login:'
tools/vm/tetsu-vm console        # Ctrl-] detaches
tools/vm/tetsu-vm stop
```

`fetch-arch-iso` downloads `archlinux-x86_64.iso` and `sha256sums.txt` from
`https://geo.mirror.pkgbuild.com/iso/latest/` (override with `TETSU_ARCH_MIRROR`)
into `${XDG_CACHE_HOME:-$HOME/.cache}/tetsu/vm/` and verifies the checksum. It
skips the download when the cached ISO already verifies.

## Commands

All commands except `start` take `--state DIR` (default `<repo>/target/vm/default`,
which is gitignored).

### `start`

| Flag                  | Meaning                                                        |
|-----------------------|----------------------------------------------------------------|
| `--disk IMG`          | virtio disk (format auto-probed)                               |
| `--iso ISO`           | ISO attached as a read-only cdrom (at least one of disk/iso)   |
| `--state DIR`         | state directory                                                |
| `--mem MB`            | memory, default 2048                                           |
| `--cpus N`            | vCPUs, default 2                                               |
| `--snapshot`          | discard disk writes on exit (`-snapshot`)                      |
| `--vars TEMPLATE`     | VARS template for a new state dir (default: distro empty VARS) |
| `--fresh`             | reset VARS, TPM state and serial log before starting           |
| `--cmdline-extra STR` | extra kernel cmdline via SMBIOS (`io.systemd.boot.kernel-cmdline-extra`, needs systemd-boot) |
| `--credential N=V`    | systemd credential via SMBIOS, repeatable                      |
| `--serial-console`    | shorthand: `console=ttyS0,115200` on the cmdline plus a getty on `ttyS0` |
| `--gui`               | show a display window (default `-display none`)                |
| `--dry-run`           | print the QEMU command line, start nothing, create no state    |

Machine: `q35,smm=on`, `cfi.pflash01 secure=on`, read-only secboot CODE, a
per-VM copy of VARS, `tpm-crb` on a swtpm socket. swtpm runs with `--tpm2` and
keeps its state in `<state>/tpm`, so the TPM persists across runs unless
`--fresh`. QEMU is daemonized. `start` refuses to run when the state dir's VM is
already running and stops swtpm again if QEMU fails to start. SMBIOS values have
their commas doubled automatically. QEMU's default devices are kept on purpose:
with `-nodefaults` the firmware printed nothing on the serial port.

### `wait-for [--state DIR] [--timeout SECS] REGEX`

Polls `serial.log` (ANSI escapes and CRs removed) with an extended regex.

| Exit | Meaning                                  |
|------|------------------------------------------|
| 0    | matched                                  |
| 1    | timeout (default 120 s)                  |
| 2    | the VM died before the pattern appeared  |

A match wins over a dead VM, so output written just before a crash is still found.

### `power-cut [--state DIR]`

SIGKILLs QEMU (no flush, no ACPI shutdown), waits until it is gone, then ends
swtpm with SIGTERM. The TPM state is non-volatile and survives, as on real
hardware; the disk gets exactly what the guest had written. Idempotent when
nothing runs. A pidfile is only trusted if the pid's command name matches
(`qemu-system-*`, `swtpm`), so a reused pid is never killed.

### `stop [--state DIR] [--timeout SECS]`

QMP `quit` over the QMP socket (needs `socat`), SIGKILL after the timeout
(default 10 s), then swtpm.

### `status [--state DIR]`

Exit 0 and `running` when QEMU runs, exit 1 and `stopped` otherwise.

### `console [--state DIR]`

Interactive serial console through `socat`. Ctrl-] detaches. One client at a time.

## State directory

| File                         | Content                                         |
|------------------------------|-------------------------------------------------|
| `vars.fd`                    | per-VM copy of the OVMF VARS                    |
| `tpm/`                       | swtpm non-volatile state                        |
| `swtpm.sock`, `swtpm.pid`, `swtpm.log` | swtpm control socket, pid, log        |
| `qemu.pid`, `qemu.log`       | QEMU pid and startup stderr                     |
| `qmp.sock`                   | QMP monitor socket                              |
| `serial.sock`                | serial console socket (`console`)               |
| `serial.log`                 | everything the guest wrote to the serial port   |

## Asserting on the serial log

```sh
tools/vm/tetsu-vm start --iso "$ISO" --serial-console --snapshot
tools/vm/tetsu-vm wait-for --timeout 300 'Found device /dev/tpm0' || exit 1
tools/vm/tetsu-vm wait-for --timeout 300 'login:'
tools/vm/tetsu-vm power-cut
```

For direct checks, `serial.log` holds raw terminal output with escapes; read it
through `tetsu-vm wait-for` rather than `grep` to get the cleaned text.

## Tests

```sh
tools/vm/tests/unit.sh          # dry-run command line, wait-for, power-cut; no ISO, KVM or QEMU
tools/vm/fetch-arch-iso         # once
tools/vm/tests/boot-arch-iso.sh # integration, about a minute
```

`boot-arch-iso.sh` boots the Arch ISO (`TETSU_ARCH_ISO`, default the cache path)
in a temporary state dir, waits for `Found device /dev/tpm0` and `login:`, cuts
the power and asserts QEMU and swtpm are gone. It exits 77 (skip) without a
usable `/dev/kvm` or without the ISO. CI runs only `unit.sh`.

Test hooks: `TETSU_VM_KVM=0|1` forces TCG/KVM in the command line,
`TETSU_VM_QEMU_COMM` / `TETSU_VM_SWTPM_COMM` change the expected process names.
