//! Trusting one more certificate in the db, for `tetsu secureboot trust`.
//! A certificate is turned into a signature list under a fresh GUID, signed
//! with Tetsu's KEK and appended; a pre-signed `.auth` is appended as is.

use std::path::{Path, PathBuf};

use super::{Step, cmd};
use crate::error::InstallError;

/// What the file given on the command line is, from its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Der,
    Pem,
    /// A signed authenticated variable, ready for `efi-updatevar`.
    Auth,
}

impl Kind {
    pub fn of(path: &Path) -> Result<Self, InstallError> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        match ext.as_deref() {
            Some("der") => Ok(Self::Der),
            Some("crt" | "pem") => Ok(Self::Pem),
            Some("auth") => Ok(Self::Auth),
            _ => Err(InstallError::Failed(format!(
                "{}: expected a .der, .crt, .pem or .auth file",
                path.display()
            ))),
        }
    }
}

/// What to trust and where the KEK lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trust {
    pub cert: PathBuf,
    /// Holds `KEK.key` and `KEK.crt`.
    pub keys_dir: PathBuf,
    /// Scratch directory for the list and the `.auth` file.
    pub work_dir: PathBuf,
    /// Owner GUID of the new entry.
    pub owner_guid: String,
}

/// The sequence that appends the certificate to the db.
pub fn trust_steps(t: &Trust) -> Result<Vec<Step>, InstallError> {
    let kind = Kind::of(&t.cert)?;
    if kind == Kind::Auth {
        return Ok(vec![cmd("efi-updatevar", &[&"-a", &"-f", &t.cert, &"db"])]);
    }
    let mut steps = Vec::new();
    let pem = if kind == Kind::Der {
        let pem = t.work_dir.join("trust.pem");
        steps.push(cmd(
            "openssl",
            &[&"x509", &"-inform", &"DER", &"-in", &t.cert, &"-out", &pem],
        ));
        pem
    } else {
        t.cert.clone()
    };
    let esl = t.work_dir.join("trust.esl");
    let auth = t.work_dir.join("trust.auth");
    steps.push(cmd(
        "cert-to-efi-sig-list",
        &[&"-g", &t.owner_guid, &pem, &esl],
    ));
    steps.push(cmd(
        "sign-efi-sig-list",
        &[
            &"-a",
            &"-k",
            &t.keys_dir.join("KEK.key"),
            &"-c",
            &t.keys_dir.join("KEK.crt"),
            &"db",
            &esl,
            &auth,
        ],
    ));
    steps.push(cmd("efi-updatevar", &[&"-a", &"-f", &auth, &"db"]));
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust(cert: &str) -> Trust {
        Trust {
            cert: cert.into(),
            keys_dir: "/k".into(),
            work_dir: "/w".into(),
            owner_guid: "g-1".into(),
        }
    }

    fn lines(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|s| match s {
                Step::Run(c) => {
                    let args: Vec<String> = c
                        .args
                        .iter()
                        .map(|a| a.to_string_lossy().into_owned())
                        .collect();
                    format!("{} {}", c.program, args.join(" "))
                }
                Step::Concat { .. } => panic!("no concatenation expected"),
            })
            .collect()
    }

    #[test]
    fn extensions_pick_the_kind() {
        assert_eq!(Kind::of(Path::new("a.DER")).unwrap(), Kind::Der);
        assert_eq!(Kind::of(Path::new("a.crt")).unwrap(), Kind::Pem);
        assert_eq!(Kind::of(Path::new("a.pem")).unwrap(), Kind::Pem);
        assert_eq!(Kind::of(Path::new("a.auth")).unwrap(), Kind::Auth);
        assert!(Kind::of(Path::new("a.txt")).is_err());
        assert!(Kind::of(Path::new("noext")).is_err());
    }

    #[test]
    fn a_pem_is_listed_signed_with_the_kek_and_appended() {
        let got = lines(&trust_steps(&trust("/c/vendor.crt")).unwrap());
        assert_eq!(
            got,
            [
                "cert-to-efi-sig-list -g g-1 /c/vendor.crt /w/trust.esl",
                "sign-efi-sig-list -a -k /k/KEK.key -c /k/KEK.crt db /w/trust.esl /w/trust.auth",
                "efi-updatevar -a -f /w/trust.auth db",
            ]
        );
    }

    #[test]
    fn a_der_is_converted_first() {
        let got = lines(&trust_steps(&trust("/c/vendor.der")).unwrap());
        assert_eq!(
            got[0],
            "openssl x509 -inform DER -in /c/vendor.der -out /w/trust.pem"
        );
        assert_eq!(
            got[1],
            "cert-to-efi-sig-list -g g-1 /w/trust.pem /w/trust.esl"
        );
        assert_eq!(got.len(), 4);
    }

    #[test]
    fn an_auth_file_is_appended_as_it_is() {
        let got = lines(&trust_steps(&trust("/c/db.auth")).unwrap());
        assert_eq!(got, ["efi-updatevar -a -f /c/db.auth db"]);
    }

    #[test]
    fn an_unknown_file_type_is_refused() {
        assert!(trust_steps(&trust("/c/vendor.txt")).is_err());
    }
}
