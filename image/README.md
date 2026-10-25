# image

The Tetsu image build (mkosi, decision #17). One tree, two images, selected
with mkosi profiles:

| profile | format | output | what it is |
| --- | --- | --- | --- |
| `payload` | `tar` | `target/image/tetsu-payload.tar` | the system extracted into the first Btrfs snapshot of an install |
| `installer` | `disk` | `target/image/tetsu-installer.raw` | the USB installer medium, which carries the payload |

## Build

```sh
image/build
```

One command, from a checkout of this repository. It

1. builds the Rust binaries with `cargo +1.99.1 build --release`;
2. downloads the Microsoft Secure Boot certificates and checks every file
   against `image/microsoft-certs.sha256` (a mismatch aborts the build);
3. stages the payload tree (binaries, the seal unit and initcpio hook from
   `crates/tetsu-installer-core/initrd/`, `locale.gen`), runs mkosi, and
   publishes `target/image/tetsu-payload.tar`;
4. stages the installer tree (the installer binary, the payload tar, the
   certificates), runs mkosi, and publishes `target/image/tetsu-installer.raw`.

Re-running rebuilds both images (`mkosi --force`); the package cache
(`target/image/pkgcache`) and the downloaded certificates
(`target/image/microsoft`) are kept. Needs `mkosi` (27), `cargo`, `curl`, and
for the installer image the host tools mkosi uses for a UKI (`systemd-ukify`,
`mtools`, `dosfstools`). mkosi runs unprivileged when user namespaces are
available, else through `sudo`.

| variable | effect |
| --- | --- |
| `TETSU_IMAGE_FIRMWARE=1` | add `linux-firmware` to the payload |
| `TETSU_IMAGE_SUDO=auto\|1\|0` | run mkosi through sudo (default `auto`) |
| `TETSU_IMAGE_OUT=DIR` | output directory (default `target/image`) |

The binaries are built against the host's glibc and run on the image's: build
on an up-to-date Arch host.

## Payload

`base linux intel-ucode amd-ucode systemd systemd-ukify mkinitcpio btrfs-progs
cryptsetup tpm2-tss sbsigntools efitools efibootmgr util-linux sudo
archlinux-keyring pacman dosfstools`, plus `/usr/bin/{tetsu-tx,tetsu,
tetsu-unlock-seal,tetsu-secrets}`.

- `linux-firmware` is left out to keep the medium small (it is several
  hundred MB with all vendors): hardware that needs firmware (Wi-Fi, some GPUs)
  wants `TETSU_IMAGE_FIRMWARE=1`, or the package installed after the first boot.
- `/etc/mkinitcpio.conf.d/10-tetsu.conf` sets
  `HOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole
  block sd-encrypt filesystems fsck tetsu-seal)`. The seal unit and the
  `tetsu-seal` hook are installed as the initrd README says. Checked by
  running `mkinitcpio -k VER -g /usr/lib/tetsu/initrd.img` in
  `systemd-nspawn` over the extracted payload (what `tetsu-tx` does): all hooks
  run and the initrd holds `tetsu-unlock-seal`, `systemd-cryptenroll` and the
  `libtss2` libraries.
- `/usr/lib/tetsu/` exists (mkinitcpio does not create the directory of `-g`).
  The kernel is `/usr/lib/modules/VER/vmlinuz`; `/etc/os-release` and
  `/usr/lib/os-release` come from the `filesystem` package. `/boot` is empty:
  the initrd and the UKI are built at install time.
- The pacman database is at `/usr/lib/pacman` and `/var/lib/pacman` links to it
  (the layout `tetsu-tx` expects, with `@var` mounted over `/var`).
- Nothing machine-specific: no `/etc/machine-id`, no SSH host keys (no sshd), no
  `/etc/pacman.d/gnupg`. The keyring is made at the first boot by
  `tetsu-keyring-init.service` (`pacman-key --init` and `--populate archlinux`),
  so each device has its own local signing key and the payload holds public
  keys only.
- `%wheel ALL=(ALL:ALL) ALL` in `/etc/sudoers.d/10-wheel` (0440).
- `systemd-firstboot` is masked: the installer writes keyboard, language,
  timezone and hostname. systemd-networkd (DHCP on `en*`/`eth*`),
  systemd-resolved and systemd-timesyncd are enabled, so the first
  `tx update` has a network. The pacman mirror is
  `https://geo.mirror.pkgbuild.com`.
- Locales named by the installer's keyboard table are generated.

## Installer medium

Unsigned: the target firmware is in Setup Mode (or Secure Boot is off) when it
boots. systemd-boot with the UKI mkosi builds; kernel command line
`console=tty0 console=ttyS0,115200`. The medium never needs the network.

- Packages: `base linux systemd systemd-ukify mkinitcpio cryptsetup
  btrfs-progs dosfstools util-linux gptfdisk openssl sbsigntools efitools
  efibootmgr tar kbd tzdata tpm2-tss`.
- The payload is stored in the image as
  `/usr/share/tetsu/medium/tetsu-payload.tar` and the installer is started with
  `--medium /usr/share/tetsu/medium`. `usr-share-tetsu-medium.mount` bind-mounts
  that directory onto itself, so `findmnt -o SOURCE` gives the medium's
  partition (`/dev/vda2[/usr/share/tetsu/medium]`) and the installer excludes
  the disk it booted from. This is simpler than a separate medium partition and
  needs no mount unit tied to a partition label.
- `tetsu-installer@.service` runs one session per console, on `tty1` and
  `ttyS0` (115200). The getty on those lines is masked and root is locked: no
  login. The session (`/usr/libexec/tetsu-installer-session`) lets one console
  run the installer at a time (`flock`); the kernel's primary console (the
  serial line when the machine has one, else the screen) takes it first and
  the other waits and, once the first has ended, only offers the maintenance
  shell (a second install would wipe the system just installed). A `ttyS0` without a UART does nothing. When the installer
  exits, the console drops into a maintenance shell (`reboot`, or
  `tetsu-installer --medium /usr/share/tetsu/medium` again).
- `/usr/share/tetsu/secureboot/microsoft/{kek,db}/*.der`: Microsoft KEK CA
  2011 and KEK 2K CA 2023 in `kek/`; UEFI CA 2011, Windows Production PCA 2011,
  Microsoft UEFI CA 2023 and Windows UEFI CA 2023 in `db/`. The Option ROM UEFI
  CA 2023 is not included. Sources and SHA-256 pins are in
  `image/microsoft-certs.sha256`. The payload carries the same files, since
  `tetsu secureboot enroll` on the installed system refuses to run without them
  (unless `--without-microsoft`).
- When the target has other systems and an empty Secure Boot db, the installer
  asks for a certificate to trust. It takes a path on a filesystem that is
  already mounted: nothing mounts a USB stick for this question, and the
  session has no shell to do it from. Leave it empty to skip, then mount the
  stick on the installed system and run `tetsu secureboot trust <file>`. An
  answer file only warns and never asks.
- An answer file reaches the installer as the installer expects it (`tetsu.answers=`
  on the command line, or a filesystem labelled `TETSU-ANSWERS`, `TETSU-ANS` on
  FAT).

## Smoke test

```sh
qemu-img create -f qcow2 target/image/blank.qcow2 16G
tools/vm/tetsu-vm start --disk target/image/tetsu-installer.raw --snapshot \
  --serial-console --fresh --target-disk target/image/blank.qcow2 --mem 3072
tools/vm/tetsu-vm wait-for --timeout 240 'Keyboard layout'
tools/vm/tetsu-vm stop
```
