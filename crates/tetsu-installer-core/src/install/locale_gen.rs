//! Generate the owner's language on the target when the payload lacks it.

use tetsu_tx::tx::nspawn::Network;

use crate::error::InstallError;
use crate::locales::{self, Supported};

use super::exec::Executor;
use super::system::{read_or_empty, write_file};

impl Executor<'_> {
    /// Add the language's line to the target's `etc/locale.gen` and run its
    /// own `locale-gen` offline, so the first boot finds the locale. The
    /// line stays in `locale.gen`: it is captured into etc-base, so glibc's
    /// upgrade hook regenerates the locale in later snapshots.
    pub(super) fn generate_locale(&mut self) -> Result<(), InstallError> {
        let language = self.answers.locale.language.clone();
        let line = Supported::load(&self.snapshot_root())
            .ok()
            .and_then(|supported| supported.gen_line(&language))
            .ok_or_else(|| {
                InstallError::Failed(format!(
                    "the target's glibc does not support the chosen locale ({})",
                    locales::SUPPORTED
                ))
            })?;

        let path = self.etc().join("locale.gen");
        let text = locales::with_locale_gen_entry(&read_or_empty(&path)?, &line);
        write_file(&path, &text, 0o644)?;

        let ns = self.offline_container("locale-var")?;
        let generate = ns.args(Network::Offline, &["locale-gen"]);
        self.deps.runner.run("systemd-nspawn", &generate)?;
        let list = ns.args(Network::Offline, &["localedef", "--list-archive"]);
        let archive = self.deps.runner.output("systemd-nspawn", &list)?;
        if !locales::archive_lists(&archive, &language) {
            return Err(InstallError::Failed(format!(
                "locale-gen did not generate {}",
                locales::archive_name(&language)
            )));
        }
        Ok(())
    }
}
