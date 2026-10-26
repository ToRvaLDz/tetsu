# Initrd files of the first-boot `/etc` sync

Installed by `image/build` into the payload:

| file | installed as |
| --- | --- |
| `tetsu-etc-sync.service` | `/usr/lib/systemd/system/tetsu-etc-sync.service` |
| `tetsu.install` | `/usr/lib/initcpio/install/tetsu` (mode 0644) |
| binary `tetsu-etc-sync` (this crate) | `/usr/bin/tetsu-etc-sync` |

`tetsu` comes in `HOOKS` after `tetsu-seal`
(`image/mkosi.profiles/payload/mkosi.extra/etc/mkinitcpio.conf.d/10-tetsu.conf`).

The service runs after `sysroot.mount` and before `initrd.target` (reaching it
starts the switch, which would stop a sync still running) and before
`tetsu-unlock-seal.service`, which reads `/sysroot/etc`. It mounts the Btrfs
top level of the device behind `/sysroot` at `/run/tetsu-etc-sync/top`, syncs,
unmounts, and always exits 0. The rules, the report and the marker are in
`src/etcsync/mod.rs` and `src/etcsync/firstboot.rs`.
