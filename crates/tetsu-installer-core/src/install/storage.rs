//! Stage 1, devices: signatures, partitions, LUKS2, Btrfs and the payload.

use std::fs;

use tetsu_tx::store::btrfs::CliBtrfs;
use tetsu_tx::store::snapshot::{FIRST_COUNTER, init_layout};

use crate::error::InstallError;

use super::exec::{Executor, Shared};
use super::log::LOG_FILE;
use super::plan::Action;
use super::target::{BTRFS_LABEL, ESP_LABEL, MAPPER_NAME, ROOT_LABEL};

/// GPT type of an EFI System Partition.
const ESP_TYPE: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
/// GPT type of a LUKS partition.
const LUKS_TYPE: &str = "CA7D7CCB-63ED-4C53-861C-1742536059CC";
/// Size of the ESP.
const ESP_SIZE: &str = "2GiB";

/// The `sfdisk` script: a GPT with the ESP and a LUKS partition that takes
/// the rest of the disk.
pub(super) fn partition_script() -> String {
    format!(
        "label: gpt\n\
         size={ESP_SIZE}, type={ESP_TYPE}, name={ESP_LABEL}\n\
         type={LUKS_TYPE}, name={ROOT_LABEL}\n"
    )
}

/// `sfdisk` reads its script from stdin and the runner has none, so a
/// shell redirects the script file into it: `$0` is the disk, `$1` the
/// script.
/// New partitions are wiped too: an old filesystem at the same offset would
/// otherwise stay next to the new ESP and make `mount` refuse it as ambiguous.
const SFDISK_SH: &str = "exec sfdisk --wipe always --wipe-partitions always \"$0\" < \"$1\"";

impl Executor<'_> {
    pub(super) fn disk_action(&mut self, action: Action) -> Result<(), InstallError> {
        match action {
            Action::WipeSignatures => self.tool("wipefs", &[&"-a", &self.target.disk]),
            Action::Partition => self.partition(),
            Action::SettleDevices => self.tool("udevadm", &[&"settle"]),
            Action::FormatEsp => self.tool(
                "mkfs.vfat",
                &[
                    &"-F",
                    &"32",
                    &"-n",
                    &ESP_LABEL,
                    &self.target.esp_partition(),
                ],
            ),
            Action::MountEsp => self.mount_esp(),
            Action::LuksFormat => self.luks_format(),
            Action::LuksAddRecoveryKey => self.luks_add_recovery_key(),
            Action::LuksOpen => self.luks_open(),
            Action::FormatRoot => {
                self.tool("mkfs.btrfs", &[&"-L", &BTRFS_LABEL, &self.target.mapper()])
            }
            Action::MountTop => self.mount_top(),
            Action::InitLayout => self.init_layout(),
            Action::ExtractPayload => self.extract_payload(),
            other => self.system_action(other),
        }
    }

    fn partition(&mut self) -> Result<(), InstallError> {
        let script = self.run.secret("sfdisk.script", &partition_script())?;
        self.tool(
            "sh",
            &[&"-c", &SFDISK_SH, &self.target.disk, &script.path()],
        )?;
        script.remove()
    }

    fn mount_esp(&mut self) -> Result<(), InstallError> {
        let esp = self.target.esp();
        fs::create_dir_all(&esp).map_err(|e| InstallError::io(&esp, e))?;
        self.tool("mount", &[&self.target.esp_partition(), &esp])?;
        // From here the log is mirrored on the ESP; a failure only warns.
        if let Err(e) = self.log.attach(&esp.join(LOG_FILE)) {
            self.warn(&format!("cannot write the log to the ESP: {e}"));
        }
        Ok(())
    }

    fn luks_format(&mut self) -> Result<(), InstallError> {
        let pass = self
            .run
            .secret("luks-passphrase", self.answers.unlock.passphrase.expose())?;
        let part = self.target.root_partition();
        self.tool(
            "cryptsetup",
            &[
                &"luksFormat",
                &"--type",
                &"luks2",
                &"--batch-mode",
                &"--key-file",
                &pass.path(),
                &part,
            ],
        )?;
        pass.remove()?;
        let uuid = self.tool_output("cryptsetup", &[&"luksUUID", &part])?;
        let uuid = uuid.trim();
        if uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(InstallError::Failed(format!(
                "cryptsetup reported an unusable UUID: {uuid:?}"
            )));
        }
        self.state.luks_uuid = Some(uuid.to_owned());
        Ok(())
    }

    /// systemd recovery keys are plain passphrases, so this is a second
    /// keyslot holding the dashed key text.
    fn luks_add_recovery_key(&mut self) -> Result<(), InstallError> {
        let pass = self
            .run
            .secret("luks-passphrase", self.answers.unlock.passphrase.expose())?;
        let recovery = self
            .run
            .secret("luks-recovery", self.answers.unlock.recovery_key.expose())?;
        self.tool(
            "cryptsetup",
            &[
                &"luksAddKey",
                &"--batch-mode",
                &"--key-file",
                &pass.path(),
                &self.target.root_partition(),
                &recovery.path(),
            ],
        )?;
        pass.remove()?;
        recovery.remove()
    }

    fn luks_open(&mut self) -> Result<(), InstallError> {
        let pass = self
            .run
            .secret("luks-passphrase", self.answers.unlock.passphrase.expose())?;
        self.tool(
            "cryptsetup",
            &[
                &"open",
                &"--key-file",
                &pass.path(),
                &self.target.root_partition(),
                &MAPPER_NAME,
            ],
        )?;
        pass.remove()
    }

    fn mount_top(&mut self) -> Result<(), InstallError> {
        let top = self.target.top();
        fs::create_dir_all(&top).map_err(|e| InstallError::io(&top, e))?;
        self.tool(
            "mount",
            &[&"-o", &"subvolid=5", &self.target.mapper(), &top],
        )
    }

    fn init_layout(&mut self) -> Result<(), InstallError> {
        let btrfs = CliBtrfs::new(Shared(self.deps.runner));
        init_layout(&self.target.layout(), &btrfs)?;
        Ok(())
    }

    fn extract_payload(&mut self) -> Result<(), InstallError> {
        let root = self.target.layout().root(FIRST_COUNTER);
        self.tool(
            "tar",
            &[
                &"--numeric-owner",
                &"--xattrs",
                &"--acls",
                &"-xpf",
                &self.target.payload,
                &"-C",
                &root,
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_describes_the_two_partitions() {
        let s = partition_script();
        assert_eq!(
            s,
            "label: gpt\n\
             size=2GiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=TETSU-ESP\n\
             type=CA7D7CCB-63ED-4C53-861C-1742536059CC, name=TETSU-ROOT\n"
        );
    }
}
