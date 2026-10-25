//! `tetsu secureboot enroll` and `tetsu secureboot trust <cert>`. The key
//! enrollment is the installer's code path (`enrollment_steps`).

use std::fs;
use std::path::{Path, PathBuf};

use tetsu_installer_core::efi;
use tetsu_installer_core::entropy::Entropy;
use tetsu_installer_core::install::rundir::RunDir;
use tetsu_installer_core::preflight::{SecureBoot, is_uefi, secure_boot_state};
use tetsu_installer_core::secureboot::trust::{Trust, trust_steps};
use tetsu_installer_core::secureboot::{
    Enrollment, MicrosoftCerts, Step, enrollment_steps, guid, run_steps,
};
use tetsu_installer_core::unlock::UnlockLevel;
use tetsu_installer_core::unlockfile::UnlockFile;
use tetsu_tx::boot::tool::Runner;

const NOT_SETUP_MODE: &str = "the firmware is not in Setup Mode: put the firmware in Setup Mode \
(clear the Secure Boot keys) and run this again";

/// Every location the commands touch, so tests use a tempdir.
#[derive(Clone, Debug)]
pub struct Paths {
    /// Root the firmware variables are read under.
    pub root: PathBuf,
    pub keys_dir: PathBuf,
    pub level_file: PathBuf,
    /// Image directory holding `kek/` and `db/` with `*.der`.
    pub microsoft_dir: PathBuf,
    /// Private scratch directory for the lists and `.auth` files.
    pub work_dir: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        Self {
            root: PathBuf::from("/"),
            keys_dir: PathBuf::from(tetsu_tx::boot::keys::DEFAULT_KEYS_DIR),
            level_file: PathBuf::from("/etc/tetsu/unlock-level.toml"),
            microsoft_dir: PathBuf::from("/usr/share/tetsu/secureboot/microsoft"),
            work_dir: PathBuf::from("/run/tetsu-secureboot"),
        }
    }
}

pub struct Context<'a> {
    pub paths: Paths,
    pub is_root: bool,
    pub runner: &'a dyn Runner,
    pub entropy: &'a dyn Entropy,
}

fn require_root(ctx: &Context<'_>, what: &str) -> Result<(), String> {
    if ctx.is_root {
        Ok(())
    } else {
        Err(format!("{what} needs root: run it with sudo"))
    }
}

fn require_uefi(ctx: &Context<'_>) -> Result<(), String> {
    if is_uefi(&ctx.paths.root) {
        Ok(())
    } else {
        Err("the machine did not boot in UEFI mode".to_owned())
    }
}

fn require_keys(dir: &Path, names: &[&str]) -> Result<(), String> {
    for name in names {
        let path = dir.join(name);
        if !path.is_file() {
            return Err(format!(
                "{} is missing: the Secure Boot keys are not there",
                path.display()
            ));
        }
    }
    Ok(())
}

/// Run `steps` in a private scratch directory that is removed afterwards.
fn run_in_scratch(
    ctx: &Context<'_>,
    build: impl FnOnce(&Path) -> Result<Vec<Step>, String>,
) -> Result<(), String> {
    let run = RunDir::create(&ctx.paths.work_dir).map_err(|e| e.to_string())?;
    let result = build(run.path())
        .and_then(|steps| run_steps(ctx.runner, &steps).map_err(|e| e.to_string()));
    let _ = fs::remove_dir_all(run.path());
    result
}

fn random_guid(ctx: &Context<'_>) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    ctx.entropy.fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(guid(bytes))
}

/// Enroll the keys of the keys directory; on success mark the unlock level
/// file so the next boot seals the TPM.
pub fn enroll(ctx: &Context<'_>) -> Result<String, String> {
    require_root(ctx, "tetsu secureboot enroll")?;
    require_uefi(ctx)?;
    if secure_boot_state(&ctx.paths.root) != SecureBoot::SetupMode {
        return Err(NOT_SETUP_MODE.to_owned());
    }
    let keys: Vec<String> = ["PK", "KEK", "db"]
        .iter()
        .flat_map(|n| [format!("{n}.key"), format!("{n}.crt")])
        .collect();
    require_keys(
        &ctx.paths.keys_dir,
        &keys.iter().map(String::as_str).collect::<Vec<_>>(),
    )?;
    let owner_guid = random_guid(ctx)?;
    run_in_scratch(ctx, |work| {
        let enrollment = Enrollment {
            keys_dir: ctx.paths.keys_dir.clone(),
            work_dir: work.to_path_buf(),
            owner_guid,
            microsoft: MicrosoftCerts::from_dir(&ctx.paths.microsoft_dir),
        };
        Ok(enrollment_steps(&enrollment))
    })?;
    Ok(mark_enrolled(&ctx.paths.level_file))
}

/// Set `secure_boot = "enrolled"` for the levels that seal at the next
/// boot. The keys are in the firmware already, so a problem with the file is
/// a note, not a failure.
fn mark_enrolled(path: &Path) -> String {
    let done = "Secure Boot keys enrolled. Reboot and turn Secure Boot on in the firmware";
    let file = match UnlockFile::read(path) {
        Ok(f) => f,
        Err(e) => return format!("{done}.\nNote: the unlock level was not updated: {e}"),
    };
    if file.level == UnlockLevel::Paranoid {
        return format!("{done}.");
    }
    let marked = UnlockFile {
        secure_boot_enrolled: true,
        ..file
    };
    match marked.write(path) {
        Ok(()) => format!("{done}; the TPM seal is done at the next boot."),
        Err(e) => format!("{done}.\nNote: the unlock level was not updated: {e}"),
    }
}

/// Append a certificate (or a signed `.auth`) to the db.
pub fn trust(ctx: &Context<'_>, cert: &Path) -> Result<String, String> {
    require_root(ctx, "tetsu secureboot trust")?;
    require_uefi(ctx)?;
    if !cert.is_file() {
        return Err(format!("{}: no such file", cert.display()));
    }
    if secure_boot_state(&ctx.paths.root) != SecureBoot::SetupMode {
        let ours = efi::platform_key_is(&ctx.paths.root, &ctx.paths.keys_dir.join("PK.crt"))
            .map_err(|e| e.to_string())?;
        if !ours {
            return Err(
                "the Secure Boot keys are not enrolled under Tetsu's PK: run \
                        `tetsu secureboot enroll` first (the firmware must be in Setup Mode)"
                    .to_owned(),
            );
        }
    }
    require_keys(&ctx.paths.keys_dir, &["KEK.key", "KEK.crt"])?;
    let owner_guid = random_guid(ctx)?;
    run_in_scratch(ctx, |work| {
        trust_steps(&Trust {
            cert: cert.to_path_buf(),
            keys_dir: ctx.paths.keys_dir.clone(),
            work_dir: work.to_path_buf(),
            owner_guid,
        })
        .map_err(|e| e.to_string())
    })?;
    Ok(format!("{} is now trusted in the db.", cert.display()))
}
