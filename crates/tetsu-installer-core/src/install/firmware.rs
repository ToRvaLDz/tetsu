//! Stage 3: everything that changes firmware state, last. Key enrollment
//! and the boot entry are soft (the off path, the `BOOTX64.EFI` fallback);
//! they come after everything on the disk is signed and verified.

use std::fs;

use crate::error::InstallError;
use crate::secureboot::{Enrollment, enrollment_steps, guid, run_steps};

use super::exec::Executor;
use super::plan::Action;
use super::target::MAPPER_NAME;

/// Label of our firmware boot entry.
pub const BOOT_LABEL: &str = "Tetsu";
/// The loader, as firmware paths spell it.
const LOADER: &str = r"\EFI\systemd\systemd-bootx64.efi";

/// What `efibootmgr` printed: the boot order and the entries.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct BootEntries {
    pub order: Vec<String>,
    /// `(number, label)`, for example `("0002", "Tetsu")`.
    pub entries: Vec<(String, String)>,
}

impl BootEntries {
    pub(super) fn parse(output: &str) -> Self {
        let mut found = Self::default();
        for line in output.lines() {
            if let Some(order) = line.strip_prefix("BootOrder:") {
                found.order = order
                    .split(',')
                    .map(|n| n.trim().to_owned())
                    .filter(|n| !n.is_empty())
                    .collect();
            } else if let Some(rest) = line.strip_prefix("Boot") {
                let Some(num) = rest.get(..4) else { continue };
                if num.chars().all(|c| c.is_ascii_hexdigit()) {
                    let label = rest[4..].trim_start_matches('*').trim_start();
                    let label = label.split('\t').next().unwrap_or("").trim();
                    found.entries.push((num.to_owned(), label.to_owned()));
                }
            }
        }
        found
    }

    fn tetsu(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|(_, label)| label == BOOT_LABEL)
            .map(|(num, _)| num.as_str())
            .collect()
    }
}

/// The new `BootOrder`: the entry created by the last `--create` first,
/// every other entry kept in its place. `before` and `after` are the
/// listings around the creation.
pub(super) fn new_boot_order(before: &BootEntries, after: &BootEntries) -> Result<String, String> {
    let old = before.tetsu();
    let new: Vec<&str> = after
        .tetsu()
        .into_iter()
        .filter(|n| !old.contains(n))
        .collect();
    let [entry] = new.as_slice() else {
        return Err(format!(
            "expected one new {BOOT_LABEL} boot entry, found {}",
            new.len()
        ));
    };
    let mut order = vec![(*entry).to_owned()];
    order.extend(after.order.iter().filter(|n| n.as_str() != *entry).cloned());
    Ok(order.join(","))
}

impl Executor<'_> {
    pub(super) fn firmware_action(&mut self, action: Action) -> Result<(), InstallError> {
        match action {
            Action::EnrollKeys => self.enroll_keys(),
            Action::SecureBootOff => {
                self.warn(
                    "firmware is not in Setup Mode: the Secure Boot keys are kept in \
                     /var/lib/tetsu/keys, Secure Boot stays off and the unlock level stays \
                     pending; enroll later with `tetsu secureboot enroll`",
                );
                Ok(())
            }
            Action::CreateBootEntry => self.create_boot_entry(),
            Action::SetBootOrder => self.set_boot_order(),
            Action::Finish => self.finish(),
            other => Err(InstallError::Failed(format!(
                "{other:?} is not a firmware action"
            ))),
        }
    }

    /// Enroll the keys; the scratch files are removed either way.
    fn enroll_keys(&mut self) -> Result<(), InstallError> {
        let mut bytes = [0u8; 16];
        self.deps.entropy.fill(&mut bytes)?;
        let work_dir = self.run.subdir("secureboot")?;
        let enrollment = Enrollment {
            keys_dir: self.target.keys_dir(),
            work_dir: work_dir.clone(),
            owner_guid: guid(bytes),
            microsoft: self.target.microsoft_certs.clone(),
        };
        let result = run_steps(self.deps.runner, &enrollment_steps(&enrollment));
        let _ = fs::remove_dir_all(&work_dir);
        result
    }

    fn create_boot_entry(&mut self) -> Result<(), InstallError> {
        let before = BootEntries::parse(&self.tool_output("efibootmgr", &[])?);
        let created = self.tool_output(
            "efibootmgr",
            &[
                &"--create",
                &"--disk",
                &self.target.disk,
                &"--part",
                &"1",
                &"--label",
                &BOOT_LABEL,
                &"--loader",
                &LOADER,
            ],
        )?;
        let order =
            new_boot_order(&before, &BootEntries::parse(&created)).map_err(InstallError::Failed)?;
        self.state.boot_order = Some(order);
        Ok(())
    }

    fn set_boot_order(&mut self) -> Result<(), InstallError> {
        // No entry: its creation already warned.
        let Some(order) = self.state.boot_order.clone() else {
            return Ok(());
        };
        self.tool("efibootmgr", &[&"--bootorder", &order])
    }

    /// Flush and release everything: the log file first, since an open file
    /// keeps the ESP busy.
    fn finish(&mut self) -> Result<(), InstallError> {
        self.log.detach();
        self.tool("sync", &[])?;
        self.tool("umount", &[&self.target.esp()])?;
        self.tool("umount", &[&self.target.top()])?;
        self.tool("cryptsetup", &[&"close", &MAPPER_NAME])?;
        // Closing a filesystem or a mapping does not push the device's write
        // cache out; a last sync does, before the user cuts the power.
        self.tool("sync", &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> BootEntries {
        BootEntries::parse(s)
    }

    #[test]
    fn parses_the_order_and_the_labels() {
        let out = "BootCurrent: 0001\nTimeout: 1 seconds\nBootOrder: 0001,0000\n\
                   Boot0000* Windows Boot Manager\tHD(1,GPT,x)/File(\\EFI\\Microsoft\\bootmgfw.efi)\n\
                   Boot0001  Fedora\n";
        let e = parse(out);
        assert_eq!(e.order, ["0001", "0000"]);
        assert_eq!(
            e.entries,
            [
                ("0000".to_owned(), "Windows Boot Manager".to_owned()),
                ("0001".to_owned(), "Fedora".to_owned())
            ]
        );
    }

    #[test]
    fn the_new_entry_goes_first_and_the_others_keep_their_order() {
        let before = parse("BootOrder: 0001,0000\nBoot0000* Windows\nBoot0001* Fedora\n");
        // `--create` already put it first in some firmware, last in others.
        for order in ["0002,0001,0000", "0001,0000,0002", "0001,0002,0000"] {
            let after = parse(&format!(
                "BootOrder: {order}\nBoot0000* Windows\nBoot0001* Fedora\nBoot0002* Tetsu\n"
            ));
            assert_eq!(new_boot_order(&before, &after).unwrap(), "0002,0001,0000");
        }
    }

    #[test]
    fn an_old_tetsu_entry_is_not_taken_for_the_new_one() {
        let before = parse("BootOrder: 0002,0000\nBoot0000* Windows\nBoot0002* Tetsu\n");
        let after = parse(
            "BootOrder: 0003,0002,0000\nBoot0000* Windows\nBoot0002* Tetsu\nBoot0003* Tetsu\n",
        );
        assert_eq!(new_boot_order(&before, &after).unwrap(), "0003,0002,0000");
    }

    #[test]
    fn no_boot_order_yet_gives_just_the_entry() {
        let after = parse("Boot0000* Tetsu\n");
        assert_eq!(
            new_boot_order(&BootEntries::default(), &after).unwrap(),
            "0000"
        );
    }

    #[test]
    fn zero_or_two_new_entries_are_refused() {
        let none = parse("BootOrder: 0000\nBoot0000* Windows\n");
        assert!(new_boot_order(&BootEntries::default(), &none).is_err());
        let two = parse("Boot0000* Tetsu\nBoot0001* Tetsu\n");
        assert!(new_boot_order(&BootEntries::default(), &two).is_err());
    }
}
