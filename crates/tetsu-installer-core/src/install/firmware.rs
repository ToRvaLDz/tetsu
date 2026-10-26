//! Stage 3: everything that changes firmware state, last. Key enrollment
//! and the boot entry are soft (the off path, the `BOOTX64.EFI` fallback);
//! they come after everything on the disk is signed and verified.

use std::fs;

use crate::error::InstallError;
use crate::secureboot::trust::{Trust, trust_steps};
use crate::secureboot::{Enrollment, enrollment_steps, guid, run_steps};

use super::exec::Executor;
use super::plan::Action;
use super::target::MAPPER_NAME;

/// Label of our firmware boot entry.
pub const BOOT_LABEL: &str = "Tetsu";
/// The loader, as firmware paths spell it.
const LOADER: &str = r"\EFI\systemd\systemd-bootx64.efi";

/// One `Boot####` line of `efibootmgr -v`.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct BootEntry {
    /// For example `0002`.
    pub num: String,
    /// For example `Tetsu`.
    pub label: String,
    /// The GUID of the `HD(n,GPT,<guid>,...)` partition it points at.
    pub partition: Option<String>,
}

/// What `efibootmgr` printed: the boot order and the entries.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct BootEntries {
    pub order: Vec<String>,
    pub entries: Vec<BootEntry>,
}

/// The partition GUID of the first `HD(n,GPT,<guid>,...)` node of a device path.
fn partition_guid(device_path: &str) -> Option<String> {
    let node = device_path.split_once("HD(")?.1.split(')').next()?;
    let mut fields = node.split(',');
    let guid = fields.nth(2)?.trim();
    (!guid.is_empty()).then(|| guid.to_ascii_lowercase())
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
                    let (label, path) = label.split_once('\t').unwrap_or((label, ""));
                    found.entries.push(BootEntry {
                        num: num.to_owned(),
                        label: label.trim().to_owned(),
                        partition: partition_guid(path),
                    });
                }
            }
        }
        found
    }

    fn tetsu(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.label == BOOT_LABEL)
            .map(|e| e.num.as_str())
            .collect()
    }

    /// The `Tetsu` entry that points at the partition `guid`, if any.
    fn tetsu_on(&self, guid: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.label == BOOT_LABEL && e.partition.as_deref() == Some(guid))
            .map(|e| e.num.as_str())
    }

    /// The `Tetsu` entries whose partition is not in `present`: dead ones.
    /// An entry whose path names no GPT partition is not judged.
    fn dead_tetsu(&self, present: &[String]) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.label == BOOT_LABEL)
            .filter(|e| e.partition.as_ref().is_some_and(|g| !present.contains(g)))
            .map(|e| e.num.as_str())
            .collect()
    }

    /// The listing as it is once the entries `gone` are deleted.
    fn without(self, gone: &[&str]) -> Self {
        Self {
            order: self
                .order
                .into_iter()
                .filter(|n| !gone.contains(&n.as_str()))
                .collect(),
            entries: self
                .entries
                .into_iter()
                .filter(|e| !gone.contains(&e.num.as_str()))
                .collect(),
        }
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
    Ok(order_with_first(entry, &after.order))
}

/// `entry` first, the others of `order` after it in their place.
fn order_with_first(entry: &str, order: &[String]) -> String {
    let mut out = vec![entry.to_owned()];
    out.extend(order.iter().filter(|n| n.as_str() != entry).cloned());
    out.join(",")
}

impl Executor<'_> {
    pub(super) fn firmware_action(&mut self, action: Action) -> Result<(), InstallError> {
        match action {
            Action::EnrollKeys => self.enroll_keys(),
            Action::TrustCertificate => self.trust_certificate(),
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
            without_microsoft: self.target.without_microsoft,
        };
        let result =
            enrollment_steps(&enrollment).and_then(|steps| run_steps(self.deps.runner, &steps));
        let _ = fs::remove_dir_all(&work_dir);
        result
    }

    /// Add the imported certificate to the db, signed with the KEK just
    /// enrolled: the steps of `tetsu secureboot trust`.
    fn trust_certificate(&mut self) -> Result<(), InstallError> {
        let Some(cert) = self.target.trust_cert.clone() else {
            return Ok(());
        };
        if !self.keys_enrolled() {
            return Err(InstallError::Failed(
                "the Secure Boot keys are not enrolled".into(),
            ));
        }
        let mut bytes = [0u8; 16];
        self.deps.entropy.fill(&mut bytes)?;
        let work_dir = self.run.subdir("trust")?;
        let result = trust_steps(&Trust {
            cert,
            keys_dir: self.target.keys_dir(),
            work_dir: work_dir.clone(),
            owner_guid: guid(bytes),
        })
        .and_then(|steps| run_steps(self.deps.runner, &steps));
        let _ = fs::remove_dir_all(&work_dir);
        result
    }

    /// Lines of a listing tool's output, lowercased; `None` when it failed or
    /// printed nothing, so that nothing is judged on a missing answer.
    fn partuuids(
        &self,
        program: &str,
        args: &[&dyn AsRef<std::ffi::OsStr>],
    ) -> Option<Vec<String>> {
        let out = self.tool_output(program, args).ok()?;
        let ids: Vec<String> = out
            .lines()
            .map(|l| l.trim().to_ascii_lowercase())
            .filter(|l| !l.is_empty())
            .collect();
        (!ids.is_empty()).then_some(ids)
    }

    /// Delete the `Tetsu` entries of partitions that are gone; a failure only
    /// warns. Returns the numbers actually removed.
    fn remove_dead_entries(&mut self, listing: &BootEntries, esp: &str) -> Vec<String> {
        let Some(present) = self.partuuids("lsblk", &[&"-rno", &"PARTUUID"]) else {
            return Vec::new();
        };
        // The new ESP must be in the list, or the list is not to be trusted.
        if !present.iter().any(|p| p == esp) {
            return Vec::new();
        }
        let mut removed = Vec::new();
        for num in listing.dead_tetsu(&present) {
            match self.tool("efibootmgr", &[&"-B", &"-b", &num]) {
                Ok(()) => removed.push(num.to_owned()),
                Err(e) => self.warn(&format!(
                    "could not remove the stale {BOOT_LABEL} boot entry Boot{num}: {e}"
                )),
            }
        }
        removed
    }

    fn create_boot_entry(&mut self) -> Result<(), InstallError> {
        let listing = BootEntries::parse(&self.tool_output("efibootmgr", &[&"-v"])?);
        let esp = self
            .partuuids(
                "blkid",
                &[
                    &"-s",
                    &"PARTUUID",
                    &"-o",
                    &"value",
                    &self.target.esp_partition(),
                ],
            )
            .and_then(|ids| ids.into_iter().next());
        let reused = esp
            .as_deref()
            .and_then(|g| listing.tetsu_on(g))
            .map(str::to_owned);
        let removed = match &esp {
            Some(esp) => self.remove_dead_entries(&listing, esp),
            None => Vec::new(),
        };
        let removed: Vec<&str> = removed.iter().map(String::as_str).collect();
        let before = listing.without(&removed);
        if let Some(num) = reused {
            self.state.boot_order = Some(order_with_first(&num, &before.order));
            return Ok(());
        }
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
                BootEntry {
                    num: "0000".into(),
                    label: "Windows Boot Manager".into(),
                    partition: Some("x".into())
                },
                BootEntry {
                    num: "0001".into(),
                    label: "Fedora".into(),
                    partition: None
                }
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
        // An old entry that stays (a live partition on another disk).
        let before = parse("BootOrder: 0002,0000\nBoot0000* Windows\nBoot0002* Tetsu\n");
        let after = parse(
            "BootOrder: 0003,0002,0000\nBoot0000* Windows\nBoot0002* Tetsu\nBoot0003* Tetsu\n",
        );
        assert_eq!(new_boot_order(&before, &after).unwrap(), "0003,0002,0000");
        // An old entry that was deleted: the firmware may hand out its number
        // again, and that one is the new entry.
        let before = before.without(&["0002"]);
        let after = parse("BootOrder: 0002,0000\nBoot0000* Windows\nBoot0002* Tetsu\n");
        assert_eq!(new_boot_order(&before, &after).unwrap(), "0002,0000");
    }

    #[test]
    fn dead_entries_are_the_tetsu_ones_on_a_missing_partition() {
        let e = parse(
            "BootOrder: 0001\nBoot0001* Tetsu\tHD(1,GPT,AA-1,0x800,0x400000)/File(\\EFI)\n\
             Boot0002* Tetsu\tHD(1,GPT,bb-2,0x800,0x400000)/File(\\EFI)\n\
             Boot0003* Tetsu\tPciRoot(0x0)\n\
             Boot0004* Fedora\tHD(1,GPT,cc-3,0x800,0x400000)\n",
        );
        assert_eq!(e.dead_tetsu(&["aa-1".to_owned()]), ["0002"]);
        assert_eq!(e.tetsu_on("bb-2"), Some("0002"));
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
