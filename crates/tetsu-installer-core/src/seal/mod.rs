//! First-boot TPM seal, run by `tetsu-unlock-seal` in the initrd.
//!
//! The installer leaves Comfort and Balanced systems with `state =
//! "pending"`: the TPM key slot needs the signed PCR 11 policy and Secure
//! Boot on with our keys, which only exist after the first boot. The
//! service runs after the root is mounted at `/sysroot` and before the
//! switch to it.
//!
//! Decisions, in order:
//!
//! * **Where the state lives.** `/sysroot/etc/tetsu/unlock-level.toml`. The
//!   kernel command line says `rw`, so the snapshot is mounted read-write
//!   and the file is rewritten in place. The seal goes first and the state
//!   second; if the state cannot be written the next boot seals again, and
//!   `--wipe-slot=tpm2` makes that replace the old slot instead of adding
//!   one. The file lives inside the snapshot, so a rollback to a snapshot
//!   taken before the seal brings back `pending` while the LUKS header
//!   already holds the TPM2 slot. The header is the source of truth: when
//!   the file says `pending`, `systemd-cryptenroll <device>` lists a `tpm2`
//!   slot and `cryptsetup luksDump --dump-json-metadata <device>` shows a
//!   systemd-tpm2 token with Tetsu's policy (a `tpm2_pubkey` whose
//!   `tpm2_pubkey_pcrs` holds 11, `tpm2-pcrs` holding 7, and a `tpm2-pin`
//!   that is true for Balanced and missing or false for Comfort), the file
//!   is rewritten to `sealed` with no prompt and no `@var` mount. A tpm2
//!   slot without that policy (added by hand, stale, or forged in the
//!   unauthenticated header) does not count: the normal seal runs and
//!   `--wipe-slot=tpm2` replaces it. A failing listing or dump changes
//!   nothing and the seal goes on.
//! * **Where the keys live.** `/var` is the `@var` subvolume, mounted only
//!   after the switch. The service mounts that subvolume from the opened
//!   root mapper under `<work>/var` and unmounts it before it returns. It
//!   is mounted read-write on purpose: Btrfs refuses a second mount of a
//!   filesystem with a different read-only state than the first.
//! * **The passphrase.** `systemd-cryptenroll` needs a current credential.
//!   The cached boot passphrase is not reachable in a way that works across
//!   systemd versions, so it is asked once more through
//!   `systemd-ask-password` and handed over as a 0600 `--unlock-key-file`
//!   in the private work directory (a tmpfs in the initrd), removed right
//!   after.
//! * **The PIN** (Balanced) is asked twice and reaches the tool as the
//!   `NEWPIN` environment variable. No secret is ever in argv.
//!
//! The seal never blocks the boot: every error is printed by the binary and
//! the exit status stays 0; the level stays `pending` and the next boot
//! tries again.

pub mod cmdline;
#[cfg(test)]
mod dist;
pub mod runner;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tetsu_tx::boot::tool::Runner;

use crate::efi;
use crate::error::InstallError;
use crate::install::argv;
use crate::install::rundir::RunDir;
use crate::preflight::{SecureBoot, has_tpm2, secure_boot_state};
use crate::secret::Secret;
use crate::unlock::UnlockLevel;
use crate::unlockfile::{SealState, UnlockFile};

pub use runner::SealRunner;

/// The shared subvolume that holds `lib/tetsu/keys`.
pub const VAR_SUBVOLUME: &str = "@var";
/// TPM policy: the signed PCR 11 (the UKI) plus PCR 7 (Secure Boot keys).
/// Kept as numbers: the token check compares them with the integers
/// systemd writes, and the CLI form is built from them.
const PUBLIC_KEY_PCR_LIST: &[u64] = &[11];
const BOUND_PCR_LIST: &[u64] = &[7];
/// Seconds a prompt waits before the seal gives up and the boot goes on.
const ASK_TIMEOUT: &str = "--timeout=120";
const PIN_ATTEMPTS: usize = 3;

/// Every location the seal reads or writes, so tests use a tempdir.
#[derive(Clone, Debug)]
pub struct SealPaths {
    /// Root of `proc` and `sys` (firmware variables, TPM).
    pub root: PathBuf,
    /// The mounted new root, `/sysroot`.
    pub sysroot: PathBuf,
    /// Private work directory; `var/` below it is the `@var` mount point.
    pub work_dir: PathBuf,
}

impl SealPaths {
    pub fn system() -> Self {
        Self {
            root: PathBuf::from("/"),
            sysroot: PathBuf::from("/sysroot"),
            work_dir: PathBuf::from("/run/tetsu-seal"),
        }
    }

    fn level_file(&self) -> PathBuf {
        self.sysroot.join("etc/tetsu/unlock-level.toml")
    }

    fn var_mount(&self) -> PathBuf {
        self.work_dir.join("var")
    }
}

/// What the run did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to do: Paranoid, already sealed, or no level file.
    NotNeeded,
    /// Sealed this boot.
    Sealed,
    /// The header already held a TPM2 slot (a rollback brought `pending`
    /// back); the file now says `sealed` again.
    AlreadySealed,
    /// Still pending, with the reason for the one status line.
    Waiting(String),
}

fn waiting(why: &str) -> Result<Outcome, InstallError> {
    Ok(Outcome::Waiting(why.to_owned()))
}

/// The level file, or `None` when the system has none.
fn read_level(path: &Path) -> Result<Option<UnlockFile>, InstallError> {
    match fs::metadata(path) {
        Ok(_) => UnlockFile::read(path).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(InstallError::io(path, e)),
    }
}

/// Seal when the level, the state and the firmware allow it.
pub fn run(paths: &SealPaths, runner: &dyn SealRunner) -> Result<Outcome, InstallError> {
    let Some(file) = read_level(&paths.level_file())? else {
        return Ok(Outcome::NotNeeded);
    };
    if !file.level.needs_tpm() || file.state == SealState::Sealed {
        return Ok(Outcome::NotNeeded);
    }
    if !has_tpm2(&paths.root) {
        return waiting("no TPM 2.0 found");
    }
    match secure_boot_state(&paths.root) {
        SecureBoot::Enforcing => {}
        SecureBoot::SetupMode => return waiting("the firmware is in Setup Mode"),
        SecureBoot::Off => return waiting("Secure Boot is off"),
    }
    let cmdline_path = paths.root.join("proc/cmdline");
    let cmdline =
        fs::read_to_string(&cmdline_path).map_err(|e| InstallError::io(&cmdline_path, e))?;
    let luks = cmdline::luks_root(&cmdline)?;
    if header_has_tetsu_seal(runner, &luks.device, file.level) {
        UnlockFile {
            state: SealState::Sealed,
            ..file
        }
        .write(&paths.level_file())?;
        return Ok(Outcome::AlreadySealed);
    }

    let run_dir = RunDir::create(&paths.work_dir)?;
    let var = paths.var_mount();
    fs::create_dir_all(&var).map_err(|e| InstallError::io(&var, e))?;
    runner.run(
        "mount",
        &argv(&[
            &"-t",
            &"btrfs",
            &"-o",
            &format!("subvol={VAR_SUBVOLUME},noatime"),
            &luks.mapper,
            &var,
        ]),
    )?;
    let sealed = seal_with_keys(paths, runner, &run_dir, &luks.device, &var, file);
    let unmounted = runner.run("umount", &argv(&[&var]));
    let _ = fs::remove_dir_all(&paths.work_dir);
    match (sealed, unmounted) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(e.into()),
    }
}

/// Whether the header already holds Tetsu's seal for `level`. The slot
/// listing needs no unlock; a tpm2 row is then checked against its token
/// (`cryptsetup luksDump --dump-json-metadata`), so a hand-added or stale
/// token does not stand in for the seal. Any failure means "unknown" and
/// counts as no.
fn header_has_tetsu_seal(runner: &dyn Runner, device: &Path, level: UnlockLevel) -> bool {
    let listed = runner
        .output("systemd-cryptenroll", &argv(&[&device]))
        .is_ok_and(|listing| has_tpm2_slot(&listing));
    listed
        && runner
            .output(
                "cryptsetup",
                &argv(&[&"luksDump", &"--dump-json-metadata", &device]),
            )
            .is_ok_and(|json| has_tetsu_token(&json, level))
}

/// Whether a `systemd-cryptenroll` slot listing has a `tpm2` row. Rows are
/// `<slot> <type>`; the header row has the type `TYPE`.
fn has_tpm2_slot(listing: &str) -> bool {
    listing
        .lines()
        .filter_map(|row| row.split_whitespace().nth(1))
        .any(|kind| kind == "tpm2")
}

/// Whether the LUKS JSON metadata has a systemd-tpm2 token with Tetsu's
/// policy: a public key that signs PCR 11, a binding to PCR 7, and a PIN
/// exactly when the level is Balanced. Anything unparsable is no.
fn has_tetsu_token(json: &str, level: UnlockLevel) -> bool {
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    let Some(tokens) = metadata.get("tokens").and_then(|t| t.as_object()) else {
        return false;
    };
    let wants_pin = level == UnlockLevel::Balanced;
    tokens.values().any(|token| {
        token.get("type").and_then(|t| t.as_str()) == Some("systemd-tpm2")
            && token
                .get("tpm2_pubkey")
                .and_then(|k| k.as_str())
                .is_some_and(|k| !k.is_empty())
            && has_pcrs(token, "tpm2_pubkey_pcrs", PUBLIC_KEY_PCR_LIST)
            && has_pcrs(token, "tpm2-pcrs", BOUND_PCR_LIST)
            && token
                .get("tpm2-pin")
                .and_then(|p| p.as_bool())
                .unwrap_or(false)
                == wants_pin
    })
}

/// Whether the token's `key` array holds every PCR of `wanted`.
fn has_pcrs(token: &serde_json::Value, key: &str, wanted: &[u64]) -> bool {
    token
        .get(key)
        .and_then(|p| p.as_array())
        .is_some_and(|pcrs| {
            wanted
                .iter()
                .all(|w| pcrs.iter().any(|p| p.as_u64() == Some(*w)))
        })
}

/// A PCR list in `systemd-cryptenroll` syntax, e.g. `7+11`.
fn pcr_arg(pcrs: &[u64]) -> String {
    pcrs.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join("+")
}

/// The part that needs the keys mounted.
fn seal_with_keys(
    paths: &SealPaths,
    runner: &dyn SealRunner,
    run_dir: &RunDir,
    device: &Path,
    var: &Path,
    file: UnlockFile,
) -> Result<Outcome, InstallError> {
    let keys = var.join("lib/tetsu/keys");
    let pk = keys.join("PK.crt");
    if !pk.is_file() || !efi::platform_key_is(&paths.root, &pk)? {
        return waiting("Tetsu's Secure Boot keys are not enrolled");
    }
    let pcr_public = keys.join("pcr.pub");
    if !pcr_public.is_file() {
        return Err(InstallError::Failed(format!(
            "{} is missing",
            pcr_public.display()
        )));
    }

    let passphrase = ask(runner, "passphrase", "Disk passphrase (to seal the TPM):")?;
    let pin = match file.level {
        UnlockLevel::Balanced => Some(ask_pin(runner)?),
        _ => None,
    };
    let key_file = run_dir.secret("unlock-key", passphrase.expose())?;
    let args = enroll_args(key_file.path(), &pcr_public, pin.is_some(), device);
    match &pin {
        Some(pin) => runner.run_env("systemd-cryptenroll", &args, &[("NEWPIN", pin.expose())])?,
        None => runner.run("systemd-cryptenroll", &args)?,
    }
    key_file.remove()?;
    UnlockFile {
        state: SealState::Sealed,
        ..file
    }
    .write(&paths.level_file())?;
    Ok(Outcome::Sealed)
}

/// Arguments of the TPM2 enrollment; a new slot replaces the old one.
pub fn enroll_args(key_file: &Path, pcr_public: &Path, pin: bool, device: &Path) -> Vec<OsString> {
    let mut args = argv(&[
        &format!("--unlock-key-file={}", key_file.display()),
        &"--tpm2-device=auto",
        &format!("--tpm2-public-key={}", pcr_public.display()),
        &format!("--tpm2-public-key-pcrs={}", pcr_arg(PUBLIC_KEY_PCR_LIST)),
        &format!("--tpm2-pcrs={}", pcr_arg(BOUND_PCR_LIST)),
    ]);
    if pin {
        args.push("--tpm2-with-pin=yes".into());
    }
    args.push("--wipe-slot=tpm2".into());
    args.push(device.into());
    args
}

/// Ask one secret through the password agents; echo is off by default.
fn ask(runner: &dyn Runner, id: &str, prompt: &str) -> Result<Secret, InstallError> {
    let out = runner.output(
        "systemd-ask-password",
        &argv(&[&ASK_TIMEOUT, &format!("--id=tetsu-seal:{id}"), &prompt]),
    )?;
    let value = out.strip_suffix('\n').unwrap_or(&out);
    if value.is_empty() {
        return Err(InstallError::Failed("an empty answer was given".into()));
    }
    Ok(Secret::new(value))
}

/// Ask the PIN twice; a mismatch asks again.
fn ask_pin(runner: &dyn Runner) -> Result<Secret, InstallError> {
    for _ in 0..PIN_ATTEMPTS {
        let first = ask(runner, "pin", "New PIN for the TPM unlock:")?;
        if ask(runner, "pin-repeat", "Repeat the PIN:")? == first {
            return Ok(first);
        }
        eprintln!("The PINs do not match.");
    }
    Err(InstallError::Failed("the PINs did not match".into()))
}
