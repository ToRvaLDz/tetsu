//! The private run directory: secrets reach external tools through 0600
//! files in here (`--key-file`), never through argv, and are deleted right
//! after use.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::InstallError;

/// A directory with mode 0700.
#[derive(Debug)]
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    /// Create `path` (and its parents) with mode 0700 and force the mode
    /// when it already exists. A symlink or a file in its place is refused.
    pub fn create(path: &Path) -> Result<Self, InstallError> {
        private_dir(path)?;
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A private subdirectory, created on first use.
    pub fn subdir(&self, name: &str) -> Result<PathBuf, InstallError> {
        let path = self.path.join(name);
        private_dir(&path)?;
        Ok(path)
    }

    /// Write `contents` to a new 0600 file `name`. The file is deleted when
    /// the returned guard is dropped.
    pub fn secret(&self, name: &str, contents: &str) -> Result<SecretFile, InstallError> {
        let path = self.path.join(name);
        remove_if_present(&path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| InstallError::io(&path, e))?;
        // Created with 0600 already; the guard exists before the content
        // lands so a failed write still deletes the file.
        let guard = SecretFile { path };
        file.write_all(contents.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| InstallError::io(&guard.path, e))?;
        Ok(guard)
    }
}

/// A secret file; removed on drop.
#[derive(Debug)]
pub struct SecretFile {
    path: PathBuf,
}

impl SecretFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Delete now and report a failure (drop only does it best effort).
    pub fn remove(self) -> Result<(), InstallError> {
        remove_if_present(&self.path)
    }
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn private_dir(path: &Path) -> Result<(), InstallError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            let err = std::io::Error::new(ErrorKind::AlreadyExists, "not a directory");
            return Err(InstallError::io(path, err));
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .map_err(|e| InstallError::io(path, e))?;
        }
        Err(e) => return Err(InstallError::io(path, e)),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| InstallError::io(path, e))
}

fn remove_if_present(path: &Path) -> Result<(), InstallError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(InstallError::io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn creates_a_private_directory_and_fixes_an_open_one() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("a/run");
        let run = RunDir::create(&dir).unwrap();
        assert_eq!(mode(run.path()), 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        RunDir::create(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&run.subdir("scratch").unwrap()), 0o700);
    }

    #[test]
    fn refuses_a_symlink_or_a_file() {
        let t = tempfile::tempdir().unwrap();
        symlink(t.path(), t.path().join("link")).unwrap();
        assert!(RunDir::create(&t.path().join("link")).is_err());
        fs::write(t.path().join("file"), b"").unwrap();
        assert!(RunDir::create(&t.path().join("file")).is_err());
    }

    #[test]
    fn secret_is_0600_and_removed_on_drop() {
        let t = tempfile::tempdir().unwrap();
        let run = RunDir::create(&t.path().join("run")).unwrap();
        let secret = run.secret("pass", "s3cret").unwrap();
        let path = secret.path().to_path_buf();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(fs::read_to_string(&path).unwrap(), "s3cret");
        drop(secret);
        assert!(!path.exists());
    }

    #[test]
    fn secret_replaces_a_leftover_and_removes_explicitly() {
        let t = tempfile::tempdir().unwrap();
        let run = RunDir::create(&t.path().join("run")).unwrap();
        fs::write(run.path().join("pass"), "old").unwrap();
        let secret = run.secret("pass", "new").unwrap();
        assert_eq!(fs::read_to_string(secret.path()).unwrap(), "new");
        let path = secret.path().to_path_buf();
        secret.remove().unwrap();
        assert!(!path.exists());
    }
}
