//! Unlock step: security level, disk passphrase and recovery key. The
//! Balanced PIN is not asked here; it is asked in the initrd at first boot.

pub mod recovery;
pub mod strength;

use crate::entropy::Entropy;
use crate::error::InstallError;
use crate::frontend::{Frontend, ask_valid, choose_from};
use crate::secret::Secret;

/// How the disk is unlocked at boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockLevel {
    /// Passphrase at every boot, no TPM involved.
    Paranoid,
    /// The TPM unlocks the disk by itself.
    Comfort,
    /// The TPM unlocks the disk after a PIN.
    Balanced,
}

impl UnlockLevel {
    pub const ALL: [Self; 3] = [Self::Paranoid, Self::Comfort, Self::Balanced];

    pub fn label(self) -> &'static str {
        match self {
            Self::Paranoid => "Paranoid: full passphrase at every boot",
            Self::Comfort => "Comfort: automatic unlock through the TPM",
            Self::Balanced => "Balanced: TPM unlock with a PIN (asked at first boot)",
        }
    }

    /// Stable identifier written to configuration files.
    pub fn id(self) -> &'static str {
        match self {
            Self::Paranoid => "paranoid",
            Self::Comfort => "comfort",
            Self::Balanced => "balanced",
        }
    }

    pub fn needs_tpm(self) -> bool {
        self != Self::Paranoid
    }
}

/// Unlock answers. Secrets live in memory only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unlock {
    pub level: UnlockLevel,
    pub passphrase: Secret,
    pub recovery_key: Secret,
}

/// Ask the unlock level; only Paranoid exists without a TPM.
pub fn ask_level(fe: &mut dyn Frontend, tpm2: bool) -> Result<UnlockLevel, InstallError> {
    if !tpm2 {
        fe.message("No TPM 2.0 found: only the Paranoid level is available.");
        return Ok(UnlockLevel::Paranoid);
    }
    let labels: Vec<&str> = UnlockLevel::ALL.iter().map(|l| l.label()).collect();
    Ok(UnlockLevel::ALL[choose_from(fe, "Unlock level", &labels)?])
}

/// Ask a strong disk passphrase, twice.
pub fn ask_passphrase(fe: &mut dyn Frontend) -> Result<Secret, InstallError> {
    loop {
        let first = fe.ask_secret("Disk passphrase")?;
        if let Err(why) = strength::check(&first) {
            fe.message(&why);
            continue;
        }
        if fe.ask_secret("Repeat the disk passphrase")? == first {
            return Ok(Secret::new(first));
        }
        fe.message("The passphrases do not match.");
    }
}

/// Show the key as QR and text.
pub fn show_key(fe: &mut dyn Frontend, key: &Secret) -> Result<(), InstallError> {
    fe.message("Recovery key. Write it down or scan it: it is shown once and never stored.\n");
    fe.show_qr(key.expose())?;
    fe.message(key.expose());
    Ok(())
}

/// Show the key, then require it to be retyped.
pub fn show_and_confirm_key(fe: &mut dyn Frontend, key: &Secret) -> Result<(), InstallError> {
    show_key(fe, key)?;
    let wanted = recovery::normalise(key.expose());
    let check = move |s: &str| {
        if recovery::normalise(s) == wanted {
            Ok(())
        } else {
            Err("That is not the recovery key; check what you wrote down.".to_owned())
        }
    };
    ask_valid(fe, "Retype the recovery key", &check)?;
    Ok(())
}

/// Ask level, passphrase and recovery key confirmation.
pub fn run(
    fe: &mut dyn Frontend,
    tpm2: bool,
    entropy: &dyn Entropy,
) -> Result<Unlock, InstallError> {
    let level = ask_level(fe, tpm2)?;
    let passphrase = ask_passphrase(fe)?;
    let recovery_key = recovery::generate(entropy)?;
    show_and_confirm_key(fe, &recovery_key)?;
    Ok(Unlock {
        level,
        passphrase,
        recovery_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entropy::fake::Counter;
    use crate::testing::Script;

    fn key() -> String {
        recovery::generate(&Counter::default())
            .unwrap()
            .expose()
            .to_owned()
    }

    #[test]
    fn without_tpm_only_paranoid_and_no_question() {
        let k = key();
        let mut fe = Script::new()
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&k);
        let u = run(&mut fe, false, &Counter::default()).unwrap();
        assert_eq!(u.level, UnlockLevel::Paranoid);
        assert!(fe.messages[0].contains("only the Paranoid"));
        assert!(fe.options.is_empty());
        fe.assert_done();
    }

    #[test]
    fn with_tpm_the_level_is_chosen() {
        let k = key();
        let mut fe = Script::new()
            .pick(2)
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&k);
        let u = run(&mut fe, true, &Counter::default()).unwrap();
        assert_eq!(u.level, UnlockLevel::Balanced);
        assert_eq!(fe.options[0].len(), 3);
        assert!(!fe.prompts.iter().any(|p| p.to_lowercase().contains("pin")));
    }

    #[test]
    fn weak_and_mismatched_passphrases_are_asked_again() {
        let k = key();
        let mut fe = Script::new()
            .secret("short")
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-8")
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&k);
        let u = run(&mut fe, false, &Counter::default()).unwrap();
        assert_eq!(u.passphrase.expose(), "Correct-Horse-9");
        assert!(fe.messages.iter().any(|m| m.contains("too short")));
        assert!(fe.messages.iter().any(|m| m.contains("do not match")));
        fe.assert_done();
    }

    #[test]
    fn recovery_key_is_shown_as_qr_and_text_and_must_be_retyped() {
        let k = key();
        let mut fe = Script::new()
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text("cccc-wrong")
            .text(&k.to_uppercase().replace('-', " "));
        let u = run(&mut fe, false, &Counter::default()).unwrap();
        assert_eq!(fe.qrs.len(), 1);
        assert_eq!(fe.qrs[0], k);
        assert!(fe.messages.contains(&k));
        assert_eq!(u.recovery_key.expose(), k);
        assert!(
            fe.messages
                .iter()
                .any(|m| m.contains("not the recovery key"))
        );
        fe.assert_done();
    }

    #[test]
    fn debug_output_leaks_no_secret() {
        let k = key();
        let mut fe = Script::new()
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&k);
        let u = run(&mut fe, false, &Counter::default()).unwrap();
        let dbg = format!("{u:?}");
        assert!(!dbg.contains("Correct-Horse") && !dbg.contains(&k));
    }
}
