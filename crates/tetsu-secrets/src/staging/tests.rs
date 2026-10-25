use std::os::unix::fs::symlink;

use tetsu_installer_core::install::config::staged_api_key;

use super::*;

#[derive(Default)]
struct FakeVault {
    stored: Vec<(String, String)>,
    fail: bool,
}

impl Vault for FakeVault {
    fn store_api_key(&mut self, provider: &str, key: &Secret) -> Result<(), VaultError> {
        if self.fail {
            return Err(VaultError("vault is locked".into()));
        }
        self.stored
            .push((provider.to_owned(), key.expose().to_owned()));
        Ok(())
    }
}

fn me() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Stage `text` the way the installer does: a 0700 directory, a 0600 file.
fn stage(home: &Path, text: &str, mode: u32) -> PathBuf {
    let path = staged_path(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn the_installer_output_is_imported_and_deleted() {
    let home = tempfile::tempdir().unwrap();
    let path = stage(home.path(), &staged_api_key("openai", "sk-a\"b\\c"), 0o600);
    let mut vault = FakeVault::default();
    let got = import_staged(home.path(), &mut vault).unwrap();
    assert_eq!(
        got,
        Some(Imported {
            provider: "openai".into()
        })
    );
    assert_eq!(
        vault.stored,
        [("openai".to_owned(), "sk-a\"b\\c".to_owned())]
    );
    assert!(!path.exists());
    assert_eq!(import_staged(home.path(), &mut vault).unwrap(), None);
    assert_eq!(vault.stored.len(), 1);
}

#[test]
fn nothing_staged_is_not_an_error() {
    let home = tempfile::tempdir().unwrap();
    let mut vault = FakeVault::default();
    assert_eq!(import_staged(home.path(), &mut vault).unwrap(), None);
}

#[test]
fn a_file_with_the_wrong_mode_is_reported_and_kept() {
    for mode in [0o644, 0o640, 0o400, 0o700] {
        let home = tempfile::tempdir().unwrap();
        let path = stage(home.path(), &staged_api_key("openai", "sk"), mode);
        let mut vault = FakeVault::default();
        let err = import_staged(home.path(), &mut vault).unwrap_err();
        assert!(matches!(err, StagingError::Refused { .. }), "{mode:o}");
        assert!(err.to_string().contains("0600"));
        assert!(vault.stored.is_empty());
        assert!(path.exists());
    }
}

#[test]
fn a_file_of_another_user_is_reported_and_kept() {
    let home = tempfile::tempdir().unwrap();
    let path = stage(home.path(), &staged_api_key("openai", "sk"), 0o600);
    let mut vault = FakeVault::default();
    let err = import_staged_as(home.path(), me() + 1, &mut vault).unwrap_err();
    assert!(err.to_string().contains("not owned"), "{err}");
    assert!(vault.stored.is_empty());
    assert!(path.exists());
}

#[test]
fn a_symlink_or_a_directory_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let path = staged_path(home.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let target = home.path().join("elsewhere");
    fs::write(&target, staged_api_key("openai", "sk")).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&target, &path).unwrap();
    let mut vault = FakeVault::default();
    assert!(matches!(
        import_staged(home.path(), &mut vault),
        Err(StagingError::Refused { .. })
    ));
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        import_staged(home.path(), &mut vault),
        Err(StagingError::Refused { .. }) | Err(StagingError::Io { .. })
    ));
    assert!(vault.stored.is_empty());
    assert!(target.exists());
}

#[test]
fn a_malformed_file_is_kept_and_never_echoed() {
    for text in [
        "not toml [",
        "provider = \"openai\"\n",
        "provider = \"\"\nkey = \"sk-secret\"\n",
        "provider = \"openai\"\nkey = \"\"\n",
    ] {
        let home = tempfile::tempdir().unwrap();
        let path = stage(home.path(), text, 0o600);
        let mut vault = FakeVault::default();
        let err = import_staged(home.path(), &mut vault).unwrap_err();
        assert!(matches!(err, StagingError::Invalid { .. }), "{text}");
        assert!(!err.to_string().contains("sk-secret"));
        assert!(path.exists());
        assert!(vault.stored.is_empty());
    }
}

#[test]
fn a_vault_failure_keeps_the_staged_file() {
    let home = tempfile::tempdir().unwrap();
    let path = stage(home.path(), &staged_api_key("openai", "sk"), 0o600);
    let mut vault = FakeVault {
        fail: true,
        ..FakeVault::default()
    };
    let err = import_staged(home.path(), &mut vault).unwrap_err();
    assert_eq!(err.to_string(), "vault is locked");
    assert!(path.exists());
}

#[test]
fn the_content_is_zeroed_before_the_unlink() {
    // A second hard link keeps the inode readable after the unlink.
    let home = tempfile::tempdir().unwrap();
    let path = stage(home.path(), &staged_api_key("openai", "sk-secret"), 0o600);
    let keep = home.path().join("keep");
    fs::hard_link(&path, &keep).unwrap();
    let mut vault = FakeVault::default();
    import_staged(home.path(), &mut vault).unwrap();
    assert!(!path.exists());
    let left = fs::read(&keep).unwrap();
    assert!(!left.is_empty() && left.iter().all(|b| *b == 0));
}
