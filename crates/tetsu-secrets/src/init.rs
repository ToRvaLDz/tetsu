//! First login: create the vault key and seal it with `systemd-creds`.
//!
//! The key is 32 random bytes as hex. It goes to `systemd-creds` on stdin
//! and is written, encrypted, to the user credstore; it is never in argv,
//! in a file in the clear or in the environment.

use std::ffi::OsString;
use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::Runner;

use zeroize::Zeroizing;

use crate::paths::Paths;
use crate::unlock::CREDENTIAL_NAME;

/// The key length in bytes; the credential holds it as hex.
const KEY_BYTES: usize = 32;

/// What the key is sealed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seal {
    /// The host key and, when there is one, the TPM.
    Auto,
    /// The host key only.
    Host,
}

impl Seal {
    /// The `--with-key=` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Host => "host",
        }
    }
}

/// Why no key was created.
#[derive(Debug, thiserror::Error)]
pub enum InitError {
    /// A sealed key is there already; it is never replaced.
    #[error("{0}: a vault key exists already")]
    CredentialExists(PathBuf),
    /// A vault with content is there; a new key would lock it for good.
    #[error("{0}: a vault with content exists and has no key")]
    VaultExists(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// `systemd-creds` failed.
    #[error(transparent)]
    Tool(#[from] BootError),
}

/// Creates and seals the vault key; returns the credential path.
/// `rng` fills its argument with random bytes.
pub fn init(
    paths: &Paths,
    seal: Seal,
    runner: &dyn Runner,
    rng: &mut dyn FnMut(&mut [u8]),
) -> Result<PathBuf, InitError> {
    let credential = paths.credential();
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| InitError::Io { path, source }
    };
    if credential.symlink_metadata().is_ok() {
        return Err(InitError::CredentialExists(credential));
    }
    let vault = paths.vault();
    match fs::metadata(&vault) {
        Ok(m) if m.len() > 0 => return Err(InitError::VaultExists(vault)),
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io(&vault)(e)),
    }
    let store = paths.credstore();
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&store)
        .map_err(io(&store))?;
    let mut raw = Zeroizing::new([0u8; KEY_BYTES]);
    rng(raw.as_mut_slice());
    let mut hex = Zeroizing::new(String::with_capacity(KEY_BYTES * 2));
    for byte in raw.iter() {
        hex.push_str(&format!("{byte:02x}"));
    }
    let args: Vec<OsString> = vec![
        "encrypt".into(),
        "--user".into(),
        format!("--name={CREDENTIAL_NAME}").into(),
        format!("--with-key={}", seal.as_str()).into(),
        "-".into(),
        credential.clone().into_os_string(),
    ];
    runner.output_stdin("systemd-creds", &args, hex.as_bytes())?;
    Ok(credential)
}

/// Fills `buf` from the kernel's random source.
pub fn system_random(buf: &mut [u8]) {
    use rustix::rand::{GetRandomFlags, getrandom};
    let mut filled = 0;
    while filled < buf.len() {
        match getrandom(&mut buf[filled..], GetRandomFlags::empty()) {
            Ok(n) => filled += n,
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => panic!("getrandom failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    type Call = (String, Vec<String>, Vec<u8>);

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<Call>>,
        fail: bool,
    }

    impl Runner for Fake {
        fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
            unreachable!("init only pipes a key")
        }

        fn output(&self, _: &str, _: &[OsString]) -> Result<String, BootError> {
            unreachable!("init only pipes a key")
        }

        fn output_stdin(
            &self,
            program: &str,
            args: &[OsString],
            stdin: &[u8],
        ) -> Result<String, BootError> {
            let args = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            self.calls
                .borrow_mut()
                .push((program.to_owned(), args, stdin.to_vec()));
            if self.fail {
                return Err(BootError::ToolMissing {
                    tool: program.to_owned(),
                });
            }
            Ok(String::new())
        }
    }

    fn counting(byte: u8) -> impl FnMut(&mut [u8]) {
        move |buf: &mut [u8]| buf.fill(byte)
    }

    #[test]
    fn the_key_goes_to_systemd_creds_on_stdin() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        let runner = Fake::default();
        let out = init(&paths, Seal::Auto, &runner, &mut counting(0xab)).unwrap();
        assert_eq!(out, paths.credential());
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (program, args, stdin) = &calls[0];
        assert_eq!(program, "systemd-creds");
        let expected = [
            "encrypt".to_owned(),
            "--user".to_owned(),
            "--name=tetsu-secrets.vault-key".to_owned(),
            "--with-key=auto".to_owned(),
            "-".to_owned(),
            paths.credential().to_string_lossy().into_owned(),
        ];
        assert_eq!(args, &expected);
        assert_eq!(stdin, "ab".repeat(32).as_bytes());
        assert!(args.iter().all(|a| !a.contains("abab")));
    }

    #[test]
    fn host_sealing_is_passed_through() {
        let home = tempfile::tempdir().unwrap();
        let runner = Fake::default();
        init(
            &Paths::new(home.path()),
            Seal::Host,
            &runner,
            &mut counting(1),
        )
        .unwrap();
        assert!(
            runner.calls.borrow()[0]
                .1
                .contains(&"--with-key=host".to_owned())
        );
    }

    #[test]
    fn the_credstore_is_private() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        init(&paths, Seal::Auto, &Fake::default(), &mut counting(1)).unwrap();
        let mode = fs::metadata(paths.credstore())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn an_existing_credential_is_never_replaced() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        fs::create_dir_all(paths.credstore()).unwrap();
        fs::write(paths.credential(), b"sealed").unwrap();
        let runner = Fake::default();
        let e = init(&paths, Seal::Auto, &runner, &mut counting(1)).unwrap_err();
        assert!(matches!(e, InitError::CredentialExists(_)), "{e}");
        assert!(runner.calls.borrow().is_empty());
        assert_eq!(fs::read(paths.credential()).unwrap(), b"sealed");
    }

    #[test]
    fn a_vault_with_content_blocks_a_new_key() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        fs::create_dir_all(paths.vault().parent().unwrap()).unwrap();
        fs::write(paths.vault(), b"old vault").unwrap();
        let runner = Fake::default();
        let e = init(&paths, Seal::Auto, &runner, &mut counting(1)).unwrap_err();
        assert!(matches!(e, InitError::VaultExists(_)), "{e}");
        assert!(runner.calls.borrow().is_empty());
        assert!(!paths.credstore().exists());
        assert_eq!(fs::read(paths.vault()).unwrap(), b"old vault");
    }

    #[test]
    fn an_empty_vault_file_does_not_block() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::new(home.path());
        fs::create_dir_all(paths.vault().parent().unwrap()).unwrap();
        fs::write(paths.vault(), b"").unwrap();
        init(&paths, Seal::Auto, &Fake::default(), &mut counting(1)).unwrap();
    }

    #[test]
    fn a_tool_failure_propagates() {
        let home = tempfile::tempdir().unwrap();
        let runner = Fake {
            fail: true,
            ..Fake::default()
        };
        let e = init(
            &Paths::new(home.path()),
            Seal::Auto,
            &runner,
            &mut counting(1),
        )
        .unwrap_err();
        assert!(matches!(e, InitError::Tool(_)), "{e}");
    }
}
