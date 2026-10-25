//! Other systems on the machine and the empty db. Enrolling Tetsu's keys
//! into an empty db leaves only Tetsu and Microsoft trusted, so a system
//! signed by another authority stops booting under Secure Boot. Before the
//! enrollment the installer warns about it and offers to import that
//! system's certificate; `tetsu secureboot trust` does the same later.

use std::path::{Path, PathBuf};

use crate::disk::{Disk, DiskChoice};
use crate::efi;
use crate::error::InstallError;
use crate::frontend::{Frontend, ask_valid};

use super::trust::Kind;

/// Shown when nothing is asked or the question is skipped.
const LATER: &str = "Add its certificate later with `tetsu secureboot trust <file>`.";

/// The systems found on the disks that stay as they are, for example
/// `Windows (sdb)`.
pub fn other_systems(disks: &[Disk], chosen: &DiskChoice) -> Vec<String> {
    disks
        .iter()
        .filter(|d| d.name != chosen.name)
        .filter_map(|d| d.os.as_ref().map(|os| format!("{os} ({})", d.name)))
        .collect()
}

/// A certificate file the user can name: it exists and its extension says
/// which kind it is. Empty skips.
fn check(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    if !Path::new(path).is_file() {
        return Err(format!("{path}: no such file (empty skips)"));
    }
    Kind::of(Path::new(path))
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Warn when other systems exist and the db is empty; ask for a certificate
/// to trust unless `scripted` (an answer file is in use: nobody to ask).
/// Returns the certificate to trust after the enrollment.
pub fn offer(
    fe: &mut dyn Frontend,
    root: &Path,
    others: &[String],
    scripted: bool,
) -> Result<Option<PathBuf>, InstallError> {
    if others.is_empty() || !efi::db_is_empty(root) {
        return Ok(None);
    }
    fe.message(&format!(
        "Warning: other systems were found: {}. The Secure Boot db is empty, so after \
         the enrollment only Tetsu and Microsoft are trusted: a system signed by \
         another authority will not boot with Secure Boot on.",
        others.join(", ")
    ));
    if scripted {
        fe.message(&format!("Nothing is asked with an answer file. {LATER}"));
        return Ok(None);
    }
    let path = ask_valid(
        fe,
        "Certificate to trust (.der, .crt, .pem or .auth file), empty to skip",
        &check,
    )?;
    if path.is_empty() {
        fe.message(LATER);
        return Ok(None);
    }
    Ok(Some(PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::disk::tests_support::named;
    use crate::testing::Script;

    fn root(db: Option<&[u8]>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("sys/firmware/efi/efivars");
        fs::create_dir_all(&dir).unwrap();
        if let Some(bytes) = db {
            fs::write(dir.join("db-d719b2cb-3d3a-4596-a3bc-dad00e67656f"), bytes).unwrap();
        }
        t
    }

    fn others() -> Vec<String> {
        vec!["Windows (sdb)".to_owned()]
    }

    #[test]
    fn only_the_disks_that_stay_count() {
        let disks = [
            named("sda", Some("Windows")),
            named("sdb", Some("Fedora Linux 42")),
            named("sdc", None),
        ];
        let chosen = DiskChoice::from(&disks[0]);
        assert_eq!(other_systems(&disks, &chosen), ["Fedora Linux 42 (sdb)"]);
        let blank = DiskChoice::from(&disks[2]);
        assert_eq!(
            other_systems(&disks, &blank),
            ["Windows (sda)", "Fedora Linux 42 (sdb)"]
        );
    }

    #[test]
    fn no_other_system_or_a_filled_db_is_silent() {
        let empty = root(None);
        let mut fe = Script::new();
        assert_eq!(offer(&mut fe, empty.path(), &[], false).unwrap(), None);
        let filled = root(Some(&[7, 0, 0, 0, 1, 2, 3]));
        assert_eq!(
            offer(&mut fe, filled.path(), &others(), false).unwrap(),
            None
        );
        assert!(fe.messages.is_empty());
        fe.assert_done();
    }

    #[test]
    fn an_empty_db_with_another_system_warns_and_takes_a_certificate() {
        for db in [None, Some(&[7u8, 0, 0, 0][..])] {
            let t = root(db);
            let cert = t.path().join("vendor.der");
            fs::write(&cert, "x").unwrap();
            let mut fe = Script::new().text(cert.to_str().unwrap());
            let got = offer(&mut fe, t.path(), &others(), false).unwrap();
            assert_eq!(got, Some(cert));
            assert!(
                fe.messages[0].contains("Windows (sdb)"),
                "{:?}",
                fe.messages
            );
            assert!(fe.messages[0].starts_with("Warning:"));
            fe.assert_done();
        }
    }

    #[test]
    fn the_certificate_is_skippable() {
        let t = root(None);
        let mut fe = Script::new().text("");
        assert_eq!(offer(&mut fe, t.path(), &others(), false).unwrap(), None);
        assert!(
            fe.messages
                .iter()
                .any(|m| m.contains("tetsu secureboot trust"))
        );
        fe.assert_done();
    }

    #[test]
    fn a_file_that_cannot_be_trusted_is_refused_by_the_check() {
        let t = tempfile::tempdir().unwrap();
        let ok = t.path().join("a.pem");
        let txt = t.path().join("a.txt");
        fs::write(&ok, "x").unwrap();
        fs::write(&txt, "x").unwrap();
        assert!(check("").is_ok());
        assert!(check(ok.to_str().unwrap()).is_ok());
        assert!(
            check(txt.to_str().unwrap())
                .unwrap_err()
                .contains("expected a .der")
        );
        assert!(
            check("/no/such/a.der")
                .unwrap_err()
                .contains("no such file")
        );
    }

    #[test]
    fn with_an_answer_file_it_only_warns() {
        let t = root(None);
        // A script with no answers: asking anything would panic.
        let mut fe = Script::new();
        assert_eq!(offer(&mut fe, t.path(), &others(), true).unwrap(), None);
        assert_eq!(fe.messages.len(), 2);
        assert!(fe.messages[0].contains("Windows (sdb)"));
        assert!(fe.messages[1].contains("tetsu secureboot trust"));
        assert!(fe.prompts.is_empty());
    }
}
