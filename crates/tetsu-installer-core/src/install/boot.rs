//! Stage 2: Secure Boot keys, the signed UKI and systemd-boot, signature
//! checks, snapshot 1 sealed and recorded. Nothing here touches firmware.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tetsu_tx::boot::esp::Esp;
use tetsu_tx::boot::install::write_uki;
use tetsu_tx::boot::keys::SigningKeys;
use tetsu_tx::boot::uki::{UkiSpec, sign_efi, verify_signature};
use tetsu_tx::store::btrfs::CliBtrfs;
use tetsu_tx::store::record::{Record, Records};
use tetsu_tx::store::snapshot::{FIRST_COUNTER, capture_etc_base, mark_committed, seal};

use crate::error::InstallError;

use super::exec::{Executor, RunnerRef};
use super::plan::{Action, KeyKind};
use super::rundir::private_dir;
use super::system::write_file;

/// Room added to the kernel and initrd sizes for the UKI's own sections.
const ESTIMATE_SLACK: u64 = 16 * 1024 * 1024;
/// Where systemd-boot ships its signed-less binary inside the image.
const SYSTEMD_BOOT_SRC: &str = "usr/lib/systemd/boot/efi/systemd-bootx64.efi";
const SYSTEMD_BOOT_DEST: &str = "EFI/systemd/systemd-bootx64.efi";
const FALLBACK_DEST: &str = "EFI/BOOT/BOOTX64.EFI";

impl Executor<'_> {
    fn keys(&self) -> SigningKeys {
        SigningKeys::in_dir(
            &self.target.keys_dir(),
            self.answers.unlock.level.needs_tpm(),
        )
    }

    fn esp_file(&self, rel: &str) -> PathBuf {
        self.target.esp().join(rel)
    }

    pub(super) fn boot_action(&mut self, action: Action) -> Result<(), InstallError> {
        match action {
            Action::GenerateKey(kind) => self.generate_key(kind),
            Action::SealSnapshot => self.seal_snapshot(),
            Action::BuildUki => self.build_uki(),
            Action::InstallBootloader => self.install_bootloader(),
            Action::WriteLoaderConf => {
                let conf = self.esp_file("loader/loader.conf");
                write_file(&conf, super::config::loader_conf(), 0o644)
            }
            Action::VerifySignatures => self.verify_signatures(),
            Action::WriteRecord => self.write_record(),
            other => Err(InstallError::Failed(format!(
                "{other:?} is not a boot action"
            ))),
        }
    }

    /// A key file is created 0600 before `openssl` fills it, so the private
    /// key is never readable by others, not even for a moment.
    fn private_file(&self, path: &Path) -> Result<(), InstallError> {
        write_file(path, "", 0o600)
    }

    fn generate_key(&mut self, kind: KeyKind) -> Result<(), InstallError> {
        let dir = self.target.keys_dir();
        private_dir(&dir)?;
        let stem = kind.stem();
        let key = dir.join(format!("{stem}.key"));
        self.private_file(&key)?;
        if kind == KeyKind::Pcr {
            self.tool(
                "openssl",
                &[
                    &"genpkey",
                    &"-algorithm",
                    &"RSA",
                    &"-pkeyopt",
                    &"rsa_keygen_bits:2048",
                    &"-out",
                    &key,
                ],
            )?;
            return self.tool(
                "openssl",
                &[
                    &"rsa",
                    &"-pubout",
                    &"-in",
                    &key,
                    &"-out",
                    &dir.join("pcr.pub"),
                ],
            );
        }
        let crt = dir.join(format!("{stem}.crt"));
        let subject = format!("/CN=Tetsu {stem} {}/", self.answers.hostname);
        self.tool(
            "openssl",
            &[
                &"req",
                &"-new",
                &"-x509",
                &"-newkey",
                &"rsa:2048",
                &"-nodes",
                &"-days",
                &"3650",
                &"-subj",
                &subject,
                &"-keyout",
                &key,
                &"-out",
                &crt,
            ],
        )?;
        self.tool(
            "openssl",
            &[
                &"x509",
                &"-outform",
                &"DER",
                &"-in",
                &crt,
                &"-out",
                &dir.join(format!("{stem}.der")),
            ],
        )
    }

    fn btrfs(&self) -> CliBtrfs<RunnerRef<'_>> {
        CliBtrfs::new(RunnerRef(self.deps.runner))
    }

    /// The configuration is complete: take the read-only `etc-base` from
    /// the snapshot's own `etc`, then make the root read-only.
    fn seal_snapshot(&mut self) -> Result<(), InstallError> {
        let (layout, btrfs) = (self.target.layout(), self.btrfs());
        capture_etc_base(&layout, &btrfs, FIRST_COUNTER)?;
        seal(&layout, &btrfs, FIRST_COUNTER)?;
        Ok(())
    }

    fn build_uki(&mut self) -> Result<(), InstallError> {
        let version = self
            .state
            .kernel_version
            .clone()
            .ok_or_else(|| InstallError::Failed("the kernel version is not known".into()))?;
        let base_cmdline =
            self.state.cmdline.clone().ok_or_else(|| {
                InstallError::Failed("the kernel command line is not known".into())
            })?;
        let spec = UkiSpec {
            counter: FIRST_COUNTER,
            snapshot_root: self.target.layout().root(FIRST_COUNTER),
            kernel_version: version,
            base_cmdline,
            keys: self.keys(),
        };
        let size = |p: PathBuf| {
            fs::metadata(&p)
                .map(|m| m.len())
                .map_err(|e| InstallError::io(p, e))
        };
        let estimate = size(spec.kernel_path())? + size(spec.initrd_path())? + ESTIMATE_SLACK;
        let esp = Esp::open(&self.target.esp());
        let committed = write_uki(
            &esp,
            self.deps.probe,
            self.deps.runner,
            &spec,
            None,
            estimate,
        )?;
        if let Some(e) = committed.gc_error {
            self.warn(&format!("cleaning the ESP: {e}"));
        }
        self.state.uki_path = Some(committed.path);
        Ok(())
    }

    fn install_bootloader(&mut self) -> Result<(), InstallError> {
        let src = self
            .target
            .layout()
            .root(FIRST_COUNTER)
            .join(SYSTEMD_BOOT_SRC);
        let dest = self.esp_file(SYSTEMD_BOOT_DEST);
        let fallback = self.esp_file(FALLBACK_DEST);
        for file in [&dest, &fallback] {
            let dir = file.parent().expect("a file below the ESP root");
            fs::create_dir_all(dir).map_err(|e| InstallError::io(dir, e))?;
        }
        sign_efi(self.deps.runner, &src, &dest, &self.keys())?;
        fs::copy(&dest, &fallback).map_err(|e| InstallError::io(&fallback, e))?;
        Ok(())
    }

    fn verify_signatures(&mut self) -> Result<(), InstallError> {
        let uki = self
            .state
            .uki_path
            .clone()
            .ok_or_else(|| InstallError::Failed("no UKI was written".into()))?;
        let cert = self.keys().db_cert;
        let paths = [
            uki,
            self.esp_file(SYSTEMD_BOOT_DEST),
            self.esp_file(FALLBACK_DEST),
        ];
        for path in &paths {
            verify_signature(self.deps.runner, path, &cert)?;
        }
        Ok(())
    }

    /// The marker and the record of snapshot 1, as a first transaction
    /// would leave them.
    fn write_record(&mut self) -> Result<(), InstallError> {
        mark_committed(&self.target.layout(), FIRST_COUNTER)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let record = Record {
            counter: FIRST_COUNTER,
            finished_at: Some(now),
            log: vec!["created by the installer".to_owned()],
            ..Record::default()
        };
        Records::new(&self.target.records_dir()).write(&record)?;
        Ok(())
    }
}
