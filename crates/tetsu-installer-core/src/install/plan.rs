//! The install plan: an ordered list of typed actions in three stages
//! (decision #39). Building it touches nothing.
//!
//! 1. Disk: partitions, LUKS2, Btrfs layout, payload, configuration.
//! 2. Boot: Secure Boot keys, the signed UKI and systemd-boot, signature
//!    checks, snapshot 1 sealed and recorded.
//! 3. Firmware: key enrollment and the boot entry. No Tetsu boot entry
//!    exists before this stage, so an interrupted install never boots
//!    half-done.

use crate::agent::AgentBackend;
use crate::answers::Answers;
use crate::preflight::SecureBoot;
use crate::user::Password;

use super::target::InstallTarget;

/// The three stages, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Disk,
    Boot,
    Firmware,
}

impl Stage {
    /// Short name used as the `step` of progress lines and log entries.
    pub fn name(self) -> &'static str {
        match self {
            Self::Disk => "disk",
            Self::Boot => "boot",
            Self::Firmware => "firmware",
        }
    }
}

/// The keys generated in stage 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    /// Secure Boot platform key.
    Pk,
    /// Secure Boot key exchange key.
    Kek,
    /// Secure Boot signature database key; signs the UKI and systemd-boot.
    Db,
    /// PCR policy key; signs the UKI's PCR predictions.
    Pcr,
}

impl KeyKind {
    /// File stem inside the keys directory.
    pub fn stem(self) -> &'static str {
        match self {
            Self::Pk => "PK",
            Self::Kek => "KEK",
            Self::Db => "db",
            Self::Pcr => "pcr",
        }
    }
}

/// One step of the installation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    // Stage 1: disk.
    WipeSignatures,
    Partition,
    SettleDevices,
    FormatEsp,
    MountEsp,
    LuksFormat,
    LuksAddRecoveryKey,
    LuksOpen,
    FormatRoot,
    MountTop,
    InitLayout,
    ExtractPayload,
    /// Hash the typed session password (not planned for a given hash).
    HashPassword,
    WriteUser,
    LockRoot,
    WriteLocale,
    WriteHostname,
    WriteFstab,
    WriteCmdline,
    WriteAgentBackend,
    /// Stage the API key in the owner's home (API backends only).
    StageApiKey,
    WriteUnlockLevel,
    BuildInitrd,
    // Stage 2: boot.
    GenerateKey(KeyKind),
    SealSnapshot,
    BuildUki,
    InstallBootloader,
    WriteLoaderConf,
    VerifySignatures,
    WriteRecord,
    // Stage 3: firmware.
    /// Enroll our keys; a failure is not fatal (the off path).
    EnrollKeys,
    /// Trust the certificate the user imported, in the db (after the keys,
    /// so the KEK just enrolled signs it; planned only when one was given).
    TrustCertificate,
    /// Secure Boot is not in Setup Mode: keep the keys, warn, carry on.
    SecureBootOff,
    CreateBootEntry,
    SetBootOrder,
    /// Flush, unmount and close the encrypted volume.
    Finish,
}

impl Action {
    pub fn stage(self) -> Stage {
        match self {
            Self::GenerateKey(_)
            | Self::SealSnapshot
            | Self::BuildUki
            | Self::InstallBootloader
            | Self::WriteLoaderConf
            | Self::VerifySignatures
            | Self::WriteRecord => Stage::Boot,
            Self::EnrollKeys
            | Self::TrustCertificate
            | Self::SecureBootOff
            | Self::CreateBootEntry
            | Self::SetBootOrder
            | Self::Finish => Stage::Firmware,
            _ => Stage::Disk,
        }
    }

    /// A failure of a soft action is logged and the install carries on.
    /// The firmware actions are all soft: `\EFI\BOOT\BOOTX64.EFI` on the
    /// ESP boots the system whatever the firmware accepts.
    pub fn is_soft(self) -> bool {
        matches!(
            self,
            Self::EnrollKeys | Self::TrustCertificate | Self::CreateBootEntry | Self::SetBootOrder
        )
    }

    /// What a failed soft action leaves undone, for the warning.
    pub fn soft_consequence(self) -> &'static str {
        match self {
            Self::EnrollKeys => "Secure Boot keys not enrolled; the install goes on without them",
            Self::TrustCertificate => {
                "the certificate was not added to the db; add it later with \
                 `tetsu secureboot trust <file>`"
            }
            Self::CreateBootEntry => {
                "the firmware boot entry was not created; pick the disk in the firmware boot \
                 menu, \\EFI\\BOOT\\BOOTX64.EFI is the fallback"
            }
            Self::SetBootOrder => {
                "the boot order was not changed; pick Tetsu in the firmware boot menu, \
                 \\EFI\\BOOT\\BOOTX64.EFI is the fallback"
            }
            _ => "the install goes on",
        }
    }

    /// The external programs the action runs, directly or through
    /// `tetsu-tx`. The executor checks them all before the first action.
    pub fn programs(self) -> &'static [&'static str] {
        match self {
            Self::WipeSignatures => &["wipefs"],
            Self::Partition => &["sh", "sfdisk"],
            Self::SettleDevices => &["udevadm"],
            Self::FormatEsp => &["mkfs.vfat"],
            Self::MountEsp | Self::MountTop => &["mount"],
            Self::LuksFormat | Self::LuksAddRecoveryKey | Self::LuksOpen => &["cryptsetup"],
            Self::FormatRoot => &["mkfs.btrfs"],
            Self::InitLayout | Self::SealSnapshot => &["btrfs"],
            Self::ExtractPayload => &["tar"],
            Self::HashPassword | Self::GenerateKey(_) => &["openssl"],
            Self::WriteUser | Self::StageApiKey => &["chown"],
            Self::BuildInitrd => &["systemd-nspawn"],
            Self::BuildUki => &["ukify", "sbsign"],
            Self::InstallBootloader => &["sbsign"],
            Self::VerifySignatures => &["sbverify"],
            Self::EnrollKeys | Self::TrustCertificate => &[
                "openssl",
                "cert-to-efi-sig-list",
                "sign-efi-sig-list",
                "efi-updatevar",
            ],
            Self::CreateBootEntry => &["efibootmgr", "blkid", "lsblk"],
            Self::SetBootOrder => &["efibootmgr"],
            Self::Finish => &["sync", "umount", "cryptsetup"],
            Self::LockRoot
            | Self::WriteLocale
            | Self::WriteHostname
            | Self::WriteFstab
            | Self::WriteCmdline
            | Self::WriteAgentBackend
            | Self::WriteUnlockLevel
            | Self::WriteLoaderConf
            | Self::WriteRecord
            | Self::SecureBootOff => &[],
        }
    }

    /// One line for the progress display and the log. Never a secret.
    pub fn describe(self) -> String {
        match self {
            Self::WipeSignatures => "wipe the old signatures from the disk",
            Self::Partition => "create the GPT: 2 GiB ESP and the encrypted root partition",
            Self::SettleDevices => "wait for the partition nodes",
            Self::FormatEsp => "format the ESP as FAT32",
            Self::MountEsp => "mount the ESP",
            Self::LuksFormat => "create the LUKS2 volume with the passphrase",
            Self::LuksAddRecoveryKey => "add the recovery key to the LUKS2 volume",
            Self::LuksOpen => "open the LUKS2 volume",
            Self::FormatRoot => "format the root as Btrfs",
            Self::MountTop => "mount the Btrfs top level",
            Self::InitLayout => "create the subvolume layout and snapshot 1",
            Self::ExtractPayload => "extract the system image into snapshot 1",
            Self::HashPassword => "hash the session password",
            Self::WriteUser => "create the user and add it to wheel",
            Self::LockRoot => "lock the root account",
            Self::WriteLocale => "write keymap, language, timezone and clock settings",
            Self::WriteHostname => "write the hostname",
            Self::WriteFstab => "write the mounts of the shared subvolumes and the ESP",
            Self::WriteCmdline => "write the kernel command line",
            Self::WriteAgentBackend => "write the agent backend choice",
            Self::StageApiKey => "stage the agent API key in the user's home",
            Self::WriteUnlockLevel => "write the unlock level",
            Self::BuildInitrd => "build the initrd",
            Self::GenerateKey(KeyKind::Pk) => "generate the Secure Boot platform key",
            Self::GenerateKey(KeyKind::Kek) => "generate the Secure Boot key exchange key",
            Self::GenerateKey(KeyKind::Db) => "generate the Secure Boot signing key",
            Self::GenerateKey(KeyKind::Pcr) => "generate the PCR policy key",
            Self::SealSnapshot => "capture etc-base and seal snapshot 1",
            Self::BuildUki => "build and sign the unified kernel image",
            Self::InstallBootloader => "sign and install systemd-boot",
            Self::WriteLoaderConf => "write the boot loader configuration",
            Self::VerifySignatures => "verify every signature",
            Self::WriteRecord => "write the record of snapshot 1",
            Self::EnrollKeys => "enroll the Secure Boot keys in the firmware",
            Self::TrustCertificate => "trust the imported certificate in the db",
            Self::SecureBootOff => {
                "firmware is not in Setup Mode: keys are kept, Secure Boot stays off"
            }
            Self::CreateBootEntry => "create the Tetsu boot entry",
            Self::SetBootOrder => "put Tetsu first in the boot order",
            Self::Finish => "flush, unmount and close the encrypted volume",
        }
        .to_owned()
    }
}

/// The ordered actions of one installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallPlan {
    actions: Vec<Action>,
}

impl InstallPlan {
    /// Build the plan from the confirmed answers.
    pub fn build(answers: &Answers, target: &InstallTarget) -> Self {
        let mut actions = stage_disk(answers);
        actions.extend(stage_boot(answers));
        actions.extend(stage_firmware(target));
        Self { actions }
    }

    #[cfg(test)]
    pub(crate) fn from_actions(actions: Vec<Action>) -> Self {
        Self { actions }
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    /// Every external program the plan runs, sorted and without repeats.
    pub fn programs(&self) -> Vec<&'static str> {
        let all: std::collections::BTreeSet<&'static str> = self
            .actions
            .iter()
            .flat_map(|a| a.programs().iter().copied())
            .collect();
        all.into_iter().collect()
    }

    /// The actions of one stage, in order.
    pub fn stage(&self, stage: Stage) -> impl Iterator<Item = Action> + '_ {
        self.actions
            .iter()
            .copied()
            .filter(move |a| a.stage() == stage)
    }
}

fn stage_disk(answers: &Answers) -> Vec<Action> {
    use Action::{
        BuildInitrd, ExtractPayload, FormatEsp, FormatRoot, HashPassword, InitLayout, LockRoot,
        LuksAddRecoveryKey, LuksFormat, LuksOpen, MountEsp, MountTop, Partition, SettleDevices,
        StageApiKey, WipeSignatures, WriteAgentBackend, WriteCmdline, WriteFstab, WriteHostname,
        WriteLocale, WriteUnlockLevel, WriteUser,
    };
    let mut actions = vec![
        WipeSignatures,
        Partition,
        SettleDevices,
        FormatEsp,
        MountEsp,
        LuksFormat,
        LuksAddRecoveryKey,
        LuksOpen,
        FormatRoot,
        MountTop,
        InitLayout,
        ExtractPayload,
    ];
    if matches!(answers.user.password, Password::Plain(_)) {
        actions.push(HashPassword);
    }
    actions.extend([
        WriteUser,
        LockRoot,
        WriteLocale,
        WriteHostname,
        WriteFstab,
        WriteCmdline,
    ]);
    if let Some(backend) = &answers.agent {
        actions.push(WriteAgentBackend);
        if matches!(backend, AgentBackend::Api { .. }) {
            actions.push(StageApiKey);
        }
    }
    actions.extend([WriteUnlockLevel, BuildInitrd]);
    actions
}

fn stage_boot(answers: &Answers) -> Vec<Action> {
    let mut actions = vec![
        Action::GenerateKey(KeyKind::Pk),
        Action::GenerateKey(KeyKind::Kek),
        Action::GenerateKey(KeyKind::Db),
    ];
    if answers.unlock.level.needs_tpm() {
        actions.push(Action::GenerateKey(KeyKind::Pcr));
    }
    actions.extend([
        Action::SealSnapshot,
        Action::BuildUki,
        Action::InstallBootloader,
        Action::WriteLoaderConf,
        Action::VerifySignatures,
        Action::WriteRecord,
    ]);
    actions
}

fn stage_firmware(target: &InstallTarget) -> Vec<Action> {
    let mut actions = if target.secure_boot == SecureBoot::SetupMode {
        vec![Action::EnrollKeys]
    } else {
        vec![Action::SecureBootOff]
    };
    // Only an enrollment can be followed by a trusted certificate.
    if target.trust_cert.is_some() && target.secure_boot == SecureBoot::SetupMode {
        actions.push(Action::TrustCertificate);
    }
    actions.extend([
        Action::CreateBootEntry,
        Action::SetBootOrder,
        Action::Finish,
    ]);
    actions
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Action::*;
    use super::*;
    use crate::agent::{ApiProvider, CliTool};
    use crate::install::fake::answers;
    use crate::secret::Secret;
    use crate::unlock::UnlockLevel;

    fn target(secure_boot: SecureBoot) -> InstallTarget {
        InstallTarget::new(
            Path::new("/dev/sda"),
            Path::new("/run/medium/payload.tar"),
            Path::new("/mnt/work"),
            secure_boot,
        )
    }

    const STAGE_ONE_HEAD: [Action; 12] = [
        WipeSignatures,
        Partition,
        SettleDevices,
        FormatEsp,
        MountEsp,
        LuksFormat,
        LuksAddRecoveryKey,
        LuksOpen,
        FormatRoot,
        MountTop,
        InitLayout,
        ExtractPayload,
    ];

    #[test]
    fn comfort_with_setup_mode_is_the_full_list() {
        let plan = InstallPlan::build(
            &answers(UnlockLevel::Comfort),
            &target(SecureBoot::SetupMode),
        );
        let mut want = STAGE_ONE_HEAD.to_vec();
        want.extend([
            HashPassword,
            WriteUser,
            LockRoot,
            WriteLocale,
            WriteHostname,
            WriteFstab,
            WriteCmdline,
            WriteUnlockLevel,
            BuildInitrd,
            GenerateKey(KeyKind::Pk),
            GenerateKey(KeyKind::Kek),
            GenerateKey(KeyKind::Db),
            GenerateKey(KeyKind::Pcr),
            SealSnapshot,
            BuildUki,
            InstallBootloader,
            WriteLoaderConf,
            VerifySignatures,
            WriteRecord,
            EnrollKeys,
            CreateBootEntry,
            SetBootOrder,
            Finish,
        ]);
        assert_eq!(plan.actions(), want);
    }

    #[test]
    fn balanced_plans_the_same_as_comfort() {
        let t = target(SecureBoot::SetupMode);
        assert_eq!(
            InstallPlan::build(&answers(UnlockLevel::Balanced), &t),
            InstallPlan::build(&answers(UnlockLevel::Comfort), &t)
        );
    }

    #[test]
    fn paranoid_has_no_pcr_key() {
        let plan = InstallPlan::build(
            &answers(UnlockLevel::Paranoid),
            &target(SecureBoot::SetupMode),
        );
        assert!(!plan.actions().contains(&GenerateKey(KeyKind::Pcr)));
        let keys: Vec<_> = plan
            .actions()
            .iter()
            .filter(|a| matches!(a, GenerateKey(_)))
            .collect();
        assert_eq!(keys.len(), 3);
    }

    #[test]
    fn off_path_keeps_the_keys_and_skips_enrollment() {
        for sb in [SecureBoot::Off, SecureBoot::Enforcing] {
            let plan = InstallPlan::build(&answers(UnlockLevel::Comfort), &target(sb));
            let tail: Vec<_> = plan.stage(Stage::Firmware).collect();
            assert_eq!(tail, [SecureBootOff, CreateBootEntry, SetBootOrder, Finish]);
            assert!(plan.actions().contains(&GenerateKey(KeyKind::Db)));
            assert!(!plan.actions().contains(&EnrollKeys));
        }
    }

    #[test]
    fn a_given_hash_needs_no_hashing() {
        let mut a = answers(UnlockLevel::Paranoid);
        a.user.password = Password::from_hash("$6$salt$abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefab").unwrap();
        let plan = InstallPlan::build(&a, &target(SecureBoot::Off));
        assert!(!plan.actions().contains(&HashPassword));
    }

    #[test]
    fn agent_backend_actions_follow_the_choice() {
        let mut a = answers(UnlockLevel::Paranoid);
        let t = target(SecureBoot::Off);
        let has = |a: &Answers, x: Action| InstallPlan::build(a, &t).actions().contains(&x);
        assert!(!has(&a, WriteAgentBackend) && !has(&a, StageApiKey));
        a.agent = Some(AgentBackend::Cli(CliTool::Codex));
        assert!(has(&a, WriteAgentBackend) && !has(&a, StageApiKey));
        a.agent = Some(AgentBackend::Api {
            provider: ApiProvider::Anthropic,
            key: Secret::new("sk-ant-abcdefgh"),
        });
        assert!(has(&a, WriteAgentBackend) && has(&a, StageApiKey));
    }

    #[test]
    fn actions_never_go_back_a_stage() {
        for level in UnlockLevel::ALL {
            for sb in [SecureBoot::Off, SecureBoot::SetupMode] {
                let plan = InstallPlan::build(&answers(level), &target(sb));
                let stages: Vec<Stage> = plan.actions().iter().map(|a| a.stage()).collect();
                assert!(stages.is_sorted(), "{stages:?}");
            }
        }
    }

    #[test]
    fn only_the_firmware_actions_are_soft_and_descriptions_are_one_line() {
        let plan = InstallPlan::build(
            &answers(UnlockLevel::Comfort),
            &target(SecureBoot::SetupMode),
        );
        for a in plan.actions() {
            assert_eq!(
                a.is_soft(),
                matches!(a, EnrollKeys | CreateBootEntry | SetBootOrder)
            );
            let d = a.describe();
            assert!(!d.is_empty() && !d.contains('\n'), "{a:?}");
        }
    }
}
