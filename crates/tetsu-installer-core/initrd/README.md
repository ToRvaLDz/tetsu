# Initrd files of the first-boot seal

For Phase 6 (the image) to install:

| file | installed as |
| --- | --- |
| `tetsu-unlock-seal.service` | `/usr/lib/systemd/system/tetsu-unlock-seal.service` |
| `tetsu-seal.install` | `/usr/lib/initcpio/install/tetsu-seal` (mode 0644) |
| binary `tetsu-unlock-seal` (this crate) | `/usr/bin/tetsu-unlock-seal` |

Add `tetsu-seal` to `HOOKS` in the mkinitcpio configuration after `systemd`
and `sd-encrypt`, and make sure the image has `systemd-cryptenroll`, `tpm2-tss`
and `util-linux`.

The service runs after `sysroot.mount` and before `initrd.target`: reaching that
target starts the switch, which isolates `initrd-switch-root.target` and would
stop a seal still running.
It reads `/sysroot/etc/tetsu/unlock-level.toml`, and for a pending Comfort or
Balanced level with Secure Boot on and Tetsu's PK enrolled it runs
`systemd-cryptenroll` for the TPM2 slot, then writes `state = "sealed"`.
It always exits 0. The design decisions (where the state is written, how the
keys under `@var` are reached, how the passphrase and the PIN are passed) are
documented in `src/seal/mod.rs`.

The hook was written against the mkinitcpio hook API but not run in this
repository: building the real initrd belongs to Phase 6 and its VM test.
