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

Every command takes `--state DIR` (default `<repo>/target/vm/default`, which is
gitignored); `start` also takes it, together with the flags below.

### `start`

| Flag                  | Meaning                                                        |
|-----------------------|----------------------------------------------------------------|
| `--disk IMG`          | virtio disk (format auto-probed)                               |
| `--target-disk IMG`   | second virtio disk, always written to (an install target; `--snapshot` does not apply); serial `tetsu-target`, so the guest sees `/dev/disk/by-id/virtio-tetsu-target`; needs a boot disk or ISO |
| `--extra-disk IMG`    | further virtio disk, written to, repeatable (serials `tetsu-extra1`, `tetsu-extra2`, ...; e.g. a `TETSU-ANSWERS` volume); needs a boot disk or ISO |
| `--iso ISO`           | ISO attached as a read-only cdrom (at least one of disk/iso)   |
| `--state DIR`         | state directory                                                |
| `--mem MB`            | memory, default 2048                                           |
| `--cpus N`            | vCPUs, default 2                                               |
| `--snapshot`          | discard writes to `--disk` on exit (`snapshot=on` on that drive only; VARS and the TPM still persist, the ISO is read-only anyway) |
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
already running, terminates a leftover swtpm of that state dir first (checked by
command name) and stops swtpm again if QEMU fails to start. SMBIOS values have
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

Exit 0 and `running` when QEMU runs, exit 1 and `stopped` otherwise. A leftover
swtpm without QEMU is reported as `stopped (stale swtpm pid N ...)`, still exit 1.

### `send [--state DIR] TEXT`

Types TEXT into the serial console through `socat`, as if entered on the
keyboard. Backslash escapes (`\n`, `\r`, `\t`, `\xHH`, ...) are interpreted with
`printf %b`, so `send 'root\n'` logs in. Send a trailing `\n` to run a command.
Mind the single quotes: a `$((6*7))` inside them is evaluated by the guest shell,
not by yours.

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

## Exit codes

| Code | Meaning                                                                 |
|------|-------------------------------------------------------------------------|
| 0    | success (`status`: running; `wait-for`: matched)                        |
| 1    | runtime failure (missing tool, firmware or image, QEMU failed, ...); `status`: stopped; `wait-for`: timeout |
| 2    | `wait-for` only: the VM died before the pattern appeared                |
| 64   | usage error: unknown command or flag, missing value or argument, invalid `wait-for` regex |

`power-cut` exits 1 when QEMU survives SIGKILL (swtpm is still stopped).

## Asserting on the serial log

Do not rely on a systemd status line such as `Found device /dev/tpm0`: it is only
printed when the device job was queued before the device appeared, so it is racy.
Log in on the serial console and ask the guest instead:

```sh
tools/vm/tetsu-vm start --iso "$ISO" --serial-console --snapshot
tools/vm/tetsu-vm wait-for --timeout 300 'login:'
tools/vm/tetsu-vm send 'root\n'                  # archiso root has an empty password
tools/vm/tetsu-vm wait-for --timeout 60 'root@archiso'
# $((6*7)) is expanded by the guest, so TPM-OK-42 only appears in the command
# output and never in the echoed command line.
tools/vm/tetsu-vm send 'test -c /dev/tpm0 && echo TPM-OK-$((6*7)) || echo TPM-MISSING\n'
tools/vm/tetsu-vm wait-for --timeout 60 'TPM-OK-42'
tools/vm/tetsu-vm power-cut
```

`serial.log` holds raw terminal output with escapes; read it through
`tetsu-vm wait-for` rather than `grep` to get the cleaned text.

## Tests

```sh
tools/vm/tests/unit.sh          # dry-run command line, wait-for, send, power-cut; no ISO, KVM or QEMU (send tests need socat)
tools/vm/fetch-arch-iso         # once
tools/vm/tests/boot-arch-iso.sh # integration, about a minute
```

`boot-arch-iso.sh` boots the Arch ISO (`TETSU_ARCH_ISO`, default the cache path)
in a temporary state dir, logs in as root over the serial console, checks
`test -c /dev/tpm0` inside the guest (marker `TPM-OK-42`), cuts the power and
asserts QEMU and swtpm are gone. It exits 77 (skip) without a
usable `/dev/kvm` or without the ISO. CI runs only `unit.sh`.

## Installer acceptance

`tools/vm/tests/install.sh` is the acceptance test of the installer (ticket #41):
it installs the medium built by `image/build` onto a blank 16 GiB qcow2 target
in a VM (OVMF Secure Boot in Setup Mode, swtpm), boots the result from the
target disk alone (same VARS and TPM), logs in on the serial console, checks it,
reboots, and checks again.

```sh
image/build                                  # about 2 minutes, once per change
tools/vm/tests/install.sh answers paranoid   # one scenario, about 2 minutes each
tools/vm/tests/install.sh answers comfort
tools/vm/tests/install.sh answers balanced
tools/vm/tests/install.sh interactive        # balanced; or: interactive comfort
tools/vm/tests/install.sh sb-off
tools/vm/tests/install.sh all
```

Exit 77 (skip) without `/dev/kvm`, the image (`TETSU_INSTALLER_IMAGE`, default
`target/image/tetsu-installer.raw`) or a tool (`qemu-img`, `qemu-system-x86_64`,
`swtpm`, `socat`, `mkfs.ext4`, `mkfs.vfat`, `mcopy`, `sgdisk`). Work files and the serial log of every
phase are kept in `target/vm-install/<scenario>/` (`TETSU_INSTALL_WORK`).

| Scenario | What it does |
|----------|--------------|
| `answers LEVEL` | the answer file `tests/answers/LEVEL.toml` (with `@DISK_BY_ID@` replaced by `virtio-tetsu-target`) goes on an image attached with `--extra-disk`: ext4 labelled `TETSU-ANSWERS`, and for `paranoid` FAT labelled `TETSU-ANS` (a FAT label holds 11 characters); the installer runs unattended |
| `interactive [LEVEL]` | no answers volume; every prompt is answered over serial input: keyboard layout, disk (on a target that already holds "Old Linux", so the retype guard fires, first with a wrong name), level, passphrase twice, recovery key (read from the serial log, first retyped wrong), user, session password twice, agent backend skip, summary, destructive confirm |
| `sb-off` | the install boot uses the OVMF build without Secure Boot (`OVMF_CODE.4m.fd`, `TETSU_OVMF_CODE_PLAIN`): comfort installs with keys generated but not enrolled and the level `pending`, boots with the passphrase; then a fresh empty VARS (Setup Mode, as after clearing the keys) and the Secure Boot CODE, `tetsu secureboot enroll`, reboot (the seal happens), reboot (TPM unlock) |

After each install the test checks over serial, as the created user (root is
locked; `sudo` with the session password `tetsu-vm-test-pw`, a test-only constant
whose hash is in the answer files): the `SecureBoot` efivar is 1 and `bootctl`
says `Secure Boot: enabled (user)`, the booted entry is `tetsu-1.efi`, the
command line, wheel membership, the locked root, `/etc/tetsu/unlock-level.toml`
(`sealed` for comfort and balanced), and `systemd-cryptenroll` listing a `tpm2`
token (none for paranoid). The reboot then checks the unlock: paranoid asks the
passphrase once, comfort asks nothing (TPM), balanced asks only the PIN
(`4711-Vm-Pin`, chosen by the test and typed at the first boot). The first boot
of comfort and balanced asks the disk passphrase twice (the unlock and the
seal's own question), and balanced the new PIN twice.

The guest's serial line now and then doubles one byte of output; the checks
avoid `^` anchors where it matters.

Test hooks: `TETSU_VM_KVM=0|1` forces TCG/KVM in the command line,
`TETSU_VM_QEMU_COMM` / `TETSU_VM_SWTPM_COMM` change the expected process names.
