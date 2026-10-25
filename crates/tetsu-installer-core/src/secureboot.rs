//! Secure Boot key enrollment with efitools, as a pure sequence of steps.
//! The installer runs it in stage 3; `tetsu secureboot enroll` reuses it.
//!
//! The firmware must be in Setup Mode. The Microsoft certificates are
//! appended to the KEK and db lists so third-party firmware drivers and
//! option ROMs keep working, and existing db entries are kept by writing
//! with `efi-updatevar -a`. The order is db, KEK, then PK last: writing the
//! PK ends Setup Mode.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tetsu_tx::boot::tool::Runner;

use crate::error::InstallError;

pub mod trust;

/// Owner GUID efitools conventions use for Microsoft's certificates.
pub const MICROSOFT_OWNER_GUID: &str = "77fa9abd-0359-4d32-bd60-28f4e78f784b";

/// A random (version 4) GUID from 16 random bytes.
pub fn guid(mut b: [u8; 16]) -> String {
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

/// Microsoft certificates (DER or PEM) for the Secure Boot lists. The KEK
/// list takes the key-exchange CAs, the db list the signing CAs; the
/// caller decides which file goes where.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MicrosoftCerts {
    pub kek: Vec<PathBuf>,
    pub db: Vec<PathBuf>,
}

impl MicrosoftCerts {
    /// The certificates of an image directory: the sorted `*.der` files of
    /// `kek/` and `db/`. A missing directory gives no certificates.
    pub fn from_dir(base: &Path) -> Self {
        Self {
            kek: der_files(&base.join("kek")),
            db: der_files(&base.join("db")),
        }
    }
}

impl MicrosoftCerts {
    /// No certificate at all.
    pub fn is_empty(&self) -> bool {
        self.kek.is_empty() && self.db.is_empty()
    }
}

fn der_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_der(p) && p.is_file())
        .collect();
    files.sort();
    files
}

/// What to enroll and where the files live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Enrollment {
    /// Holds `PK|KEK|db` `.key` and `.crt` (PEM).
    pub keys_dir: PathBuf,
    /// Scratch directory for the signature lists and `.auth` files.
    pub work_dir: PathBuf,
    /// Owner GUID of our entries.
    pub owner_guid: String,
    pub microsoft: MicrosoftCerts,
    /// Enroll Tetsu's keys alone, on purpose. Without it an empty
    /// `microsoft` is refused: the db would lack the Microsoft UEFI CA.
    pub without_microsoft: bool,
}

/// One external command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub program: &'static str,
    pub args: Vec<OsString>,
}

/// One step of the sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Run(Command),
    /// Concatenate signature lists (efitools lists are plain byte
    /// concatenations).
    Concat {
        inputs: Vec<PathBuf>,
        output: PathBuf,
    },
}

fn cmd(program: &'static str, args: &[&dyn AsRef<std::ffi::OsStr>]) -> Step {
    Step::Run(Command {
        program,
        args: crate::install::argv(args),
    })
}

fn is_der(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("der"))
}

/// Convert each certificate to PEM when needed, then turn it into a
/// signature list. Returns the lists in order.
fn microsoft_lists(
    tag: &str,
    certs: &[PathBuf],
    work: &Path,
    steps: &mut Vec<Step>,
) -> Vec<PathBuf> {
    let mut lists = Vec::new();
    for (i, cert) in certs.iter().enumerate() {
        let pem = if is_der(cert) {
            let pem = work.join(format!("ms-{tag}-{i}.pem"));
            steps.push(cmd(
                "openssl",
                &[&"x509", &"-inform", &"DER", &"-in", cert, &"-out", &pem],
            ));
            pem
        } else {
            cert.clone()
        };
        let esl = work.join(format!("ms-{tag}-{i}.esl"));
        steps.push(cmd(
            "cert-to-efi-sig-list",
            &[&"-g", &MICROSOFT_OWNER_GUID, &pem, &esl],
        ));
        lists.push(esl);
    }
    lists
}

/// The full argv sequence of an enrollment. Refuses to enroll without the
/// Microsoft certificates unless `without_microsoft` says so.
pub fn enrollment_steps(e: &Enrollment) -> Result<Vec<Step>, InstallError> {
    if e.microsoft.is_empty() && !e.without_microsoft {
        return Err(InstallError::NoMicrosoftCerts);
    }
    let key = |n: &str| e.keys_dir.join(format!("{n}.key"));
    let crt = |n: &str| e.keys_dir.join(format!("{n}.crt"));
    let work = |n: &str| e.work_dir.join(n);
    let mut steps = Vec::new();

    // Our own certificates as signature lists.
    for name in ["PK", "KEK", "db"] {
        steps.push(cmd(
            "cert-to-efi-sig-list",
            &[
                &"-g",
                &e.owner_guid,
                &crt(name),
                &work(&format!("{name}.own.esl")),
            ],
        ));
    }
    // Microsoft's, appended to KEK and db.
    let kek_ms = microsoft_lists("kek", &e.microsoft.kek, &e.work_dir, &mut steps);
    let db_ms = microsoft_lists("db", &e.microsoft.db, &e.work_dir, &mut steps);
    for (name, ms) in [("KEK", kek_ms), ("db", db_ms)] {
        let mut inputs = vec![work(&format!("{name}.own.esl"))];
        inputs.extend(ms);
        steps.push(Step::Concat {
            inputs,
            output: work(&format!("{name}.esl")),
        });
    }

    // PK signs itself and the KEK list, the KEK signs the db list. db and
    // KEK are appended, so they are signed for APPEND_WRITE.
    steps.push(cmd(
        "sign-efi-sig-list",
        &[
            &"-k",
            &key("PK"),
            &"-c",
            &crt("PK"),
            &"PK",
            &work("PK.own.esl"),
            &work("PK.auth"),
        ],
    ));
    steps.push(cmd(
        "sign-efi-sig-list",
        &[
            &"-a",
            &"-k",
            &key("PK"),
            &"-c",
            &crt("PK"),
            &"KEK",
            &work("KEK.esl"),
            &work("KEK.auth"),
        ],
    ));
    steps.push(cmd(
        "sign-efi-sig-list",
        &[
            &"-a",
            &"-k",
            &key("KEK"),
            &"-c",
            &crt("KEK"),
            &"db",
            &work("db.esl"),
            &work("db.auth"),
        ],
    ));

    // db, KEK, then PK last.
    steps.push(cmd(
        "efi-updatevar",
        &[&"-a", &"-f", &work("db.auth"), &"db"],
    ));
    steps.push(cmd(
        "efi-updatevar",
        &[&"-a", &"-f", &work("KEK.auth"), &"KEK"],
    ));
    steps.push(cmd("efi-updatevar", &[&"-f", &work("PK.auth"), &"PK"]));
    Ok(steps)
}

/// Run `steps` in order, stopping at the first failure.
pub fn run_steps(runner: &dyn Runner, steps: &[Step]) -> Result<(), InstallError> {
    for step in steps {
        match step {
            Step::Run(c) => runner.run(c.program, &c.args)?,
            Step::Concat { inputs, output } => {
                let mut bytes = Vec::new();
                for input in inputs {
                    bytes.extend(fs::read(input).map_err(|e| InstallError::io(input, e))?);
                }
                fs::write(output, bytes).map_err(|e| InstallError::io(output, e))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enrollment(microsoft: MicrosoftCerts) -> Enrollment {
        Enrollment {
            keys_dir: "/k".into(),
            work_dir: "/w".into(),
            owner_guid: "11111111-2222-4333-8444-555555555555".into(),
            microsoft,
            without_microsoft: true,
        }
    }

    fn some_microsoft() -> MicrosoftCerts {
        MicrosoftCerts {
            kek: vec!["/ms/kek.der".into()],
            db: vec!["/ms/uefi.der".into()],
        }
    }

    /// Steps as shell-like lines; a concatenation reads `cat a b > out`.
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
                Step::Concat { inputs, output } => {
                    let ins: Vec<String> = inputs.iter().map(|p| p.display().to_string()).collect();
                    format!("cat {} > {}", ins.join(" "), output.display())
                }
            })
            .collect()
    }

    #[test]
    fn guid_is_a_v4_guid() {
        let g = guid([0xff; 16]);
        assert_eq!(g, "ffffffff-ffff-4fff-bfff-ffffffffffff");
        assert_eq!(guid([0; 16]), "00000000-0000-4000-8000-000000000000");
    }

    #[test]
    fn own_keys_only() {
        let got = lines(&enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap());
        let guid = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            got,
            [
                format!("cert-to-efi-sig-list -g {guid} /k/PK.crt /w/PK.own.esl"),
                format!("cert-to-efi-sig-list -g {guid} /k/KEK.crt /w/KEK.own.esl"),
                format!("cert-to-efi-sig-list -g {guid} /k/db.crt /w/db.own.esl"),
                "cat /w/KEK.own.esl > /w/KEK.esl".to_owned(),
                "cat /w/db.own.esl > /w/db.esl".to_owned(),
                "sign-efi-sig-list -k /k/PK.key -c /k/PK.crt PK /w/PK.own.esl /w/PK.auth"
                    .to_owned(),
                "sign-efi-sig-list -a -k /k/PK.key -c /k/PK.crt KEK /w/KEK.esl /w/KEK.auth"
                    .to_owned(),
                "sign-efi-sig-list -a -k /k/KEK.key -c /k/KEK.crt db /w/db.esl /w/db.auth"
                    .to_owned(),
                "efi-updatevar -a -f /w/db.auth db".to_owned(),
                "efi-updatevar -a -f /w/KEK.auth KEK".to_owned(),
                "efi-updatevar -f /w/PK.auth PK".to_owned(),
            ]
        );
    }

    #[test]
    fn no_microsoft_certificates_is_refused_unless_asked_for() {
        let mut e = enrollment(MicrosoftCerts::default());
        e.without_microsoft = false;
        assert!(matches!(
            enrollment_steps(&e),
            Err(InstallError::NoMicrosoftCerts)
        ));
        e.without_microsoft = true;
        assert!(enrollment_steps(&e).is_ok());
        e.without_microsoft = false;
        e.microsoft = some_microsoft();
        assert!(enrollment_steps(&e).is_ok());
    }

    #[test]
    fn microsoft_certificates_are_converted_and_appended() {
        let ms = MicrosoftCerts {
            kek: vec!["/ms/kek.der".into()],
            db: vec!["/ms/win.der".into(), "/ms/uefi.pem".into()],
        };
        let got = lines(&enrollment_steps(&enrollment(ms)).unwrap());
        let g = |s: &str| {
            got.iter()
                .position(|l| l == s)
                .unwrap_or_else(|| panic!("{s}\n{got:#?}"))
        };
        let ms_guid = MICROSOFT_OWNER_GUID;
        g("openssl x509 -inform DER -in /ms/kek.der -out /w/ms-kek-0.pem");
        g(&format!(
            "cert-to-efi-sig-list -g {ms_guid} /w/ms-kek-0.pem /w/ms-kek-0.esl"
        ));
        g("openssl x509 -inform DER -in /ms/win.der -out /w/ms-db-0.pem");
        g(&format!(
            "cert-to-efi-sig-list -g {ms_guid} /w/ms-db-0.pem /w/ms-db-0.esl"
        ));
        // A PEM is used as it is.
        g(&format!(
            "cert-to-efi-sig-list -g {ms_guid} /ms/uefi.pem /w/ms-db-1.esl"
        ));
        assert!(!got.iter().any(|l| l.contains("-in /ms/uefi.pem")));
        g("cat /w/KEK.own.esl /w/ms-kek-0.esl > /w/KEK.esl");
        g("cat /w/db.own.esl /w/ms-db-0.esl /w/ms-db-1.esl > /w/db.esl");
    }

    #[test]
    fn the_pk_is_written_last_and_the_others_are_appended() {
        let got = lines(&enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap());
        let writes: Vec<&String> = got
            .iter()
            .filter(|l| l.starts_with("efi-updatevar"))
            .collect();
        assert_eq!(writes.len(), 3);
        assert!(writes[0].ends_with(" db") && writes[0].contains(" -a "));
        assert!(writes[1].ends_with(" KEK") && writes[1].contains(" -a "));
        assert!(writes[2].ends_with(" PK") && !writes[2].contains(" -a "));
        assert_eq!(got.last(), Some(writes[2]));
    }

    #[test]
    fn run_steps_concatenates_and_stops_at_the_first_failure() {
        use std::cell::RefCell;
        struct Fail(RefCell<Vec<String>>);
        impl Runner for Fail {
            fn run(&self, program: &str, _: &[OsString]) -> Result<(), tetsu_tx::boot::BootError> {
                self.0.borrow_mut().push(program.to_owned());
                Err(tetsu_tx::boot::BootError::ToolMissing {
                    tool: program.to_owned(),
                })
            }
            fn output(&self, p: &str, a: &[OsString]) -> Result<String, tetsu_tx::boot::BootError> {
                self.run(p, a).map(|()| String::new())
            }
        }
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("a"), b"AA").unwrap();
        fs::write(t.path().join("b"), b"BB").unwrap();
        let out = t.path().join("out");
        let concat = Step::Concat {
            inputs: vec![t.path().join("a"), t.path().join("b")],
            output: out.clone(),
        };
        let runner = Fail(RefCell::default());
        run_steps(&runner, std::slice::from_ref(&concat)).unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"AABB");
        let steps = [cmd("one", &[]), cmd("two", &[])];
        assert!(run_steps(&runner, &steps).is_err());
        assert_eq!(*runner.0.borrow(), ["one"]);
        let missing = Step::Concat {
            inputs: vec![t.path().join("zz")],
            output: out,
        };
        assert!(run_steps(&runner, &[missing]).is_err());
    }
}
