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

pub mod enroll;
pub mod foreign;
pub mod trust;

pub use enroll::{Enrolled, enroll};

/// Owner GUID efitools conventions use for Microsoft's certificates.
pub const MICROSOFT_OWNER_GUID: &str = "77fa9abd-0359-4d32-bd60-28f4e78f784b";

/// A GUID of `version` from 16 bytes, with the RFC 4122 variant bits.
fn format_guid(mut b: [u8; 16], version: u8) -> String {
    b[6] = (b[6] & 0x0f) | (version << 4);
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

/// A random (version 4) GUID from 16 random bytes.
pub fn guid(b: [u8; 16]) -> String {
    format_guid(b, 4)
}

/// A custom (version 8) GUID from the first 16 bytes of a digest.
pub fn derived_guid(digest: [u8; 16]) -> String {
    format_guid(digest, 8)
}

/// The 32 bytes of an `openssl x509 -fingerprint -sha256` line. OpenSSL 1.1
/// prints `SHA256 Fingerprint=`, 3.x `sha256 Fingerprint=`.
fn parse_fingerprint(output: &str) -> Option<[u8; 32]> {
    let (label, hex) = output.trim().split_once('=')?;
    if !label.to_ascii_lowercase().ends_with("fingerprint") {
        return None;
    }
    let bytes: Vec<u8> = hex
        .trim()
        .split(':')
        .map(|h| {
            (h.len() == 2)
                .then(|| u8::from_str_radix(h, 16).ok())
                .flatten()
        })
        .collect::<Option<_>>()?;
    bytes.try_into().ok()
}

/// The owner GUID of Tetsu's entries, derived from the SHA-256 fingerprint
/// of the PK certificate: the same keys always give the same GUID, so a
/// second enrollment appends the same entries and the firmware keeps one.
pub fn owner_guid(runner: &dyn Runner, pk_crt: &Path) -> Result<String, InstallError> {
    let out = runner.output(
        "openssl",
        &crate::install::argv(&[
            &"x509",
            &"-in",
            &pk_crt,
            &"-noout",
            &"-fingerprint",
            &"-sha256",
        ]),
    )?;
    let digest = parse_fingerprint(&out).ok_or_else(|| {
        InstallError::Failed(format!(
            "{}: cannot read the SHA-256 fingerprint from {:?}",
            pk_crt.display(),
            out.trim()
        ))
    })?;
    let mut head = [0u8; 16];
    head.copy_from_slice(&digest[..16]);
    Ok(derived_guid(head))
}

/// The clock in Unix seconds, for the timestamp of an enrollment.
pub fn now_unix() -> Result<u64, InstallError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| InstallError::Failed(format!("the clock is before 1970: {e}")))
}

/// `YYYY-MM-DD HH:MM:SS` (UTC) of Unix seconds, the form `sign-efi-sig-list -t` takes.
pub fn efi_time(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
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
    /// Unix seconds (UTC) the signed writes carry: the restores of a rollback
    /// carry the second after, which the firmware needs to take them.
    pub timestamp: u64,
}

/// A variable the enrollment writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    Db,
    Kek,
    Pk,
}

impl Var {
    /// The name `efi-updatevar` and `sign-efi-sig-list` take.
    pub fn name(self) -> &'static str {
        match self {
            Self::Db => "db",
            Self::Kek => "KEK",
            Self::Pk => "PK",
        }
    }

    /// The key that signs a write of this variable: the KEK signs the db, the
    /// PK the KEK and itself.
    pub fn signer(self) -> &'static str {
        match self {
            Self::Db => "KEK",
            Self::Kek | Self::Pk => "PK",
        }
    }
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

/// An enrollment as steps: the preparation (lists, concatenation, signing)
/// and the three writes, which the caller runs one by one to roll back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentSteps {
    pub prepare: Vec<Step>,
    /// db, KEK, then PK last.
    pub writes: Vec<(Var, Step)>,
}

/// Refuse to enroll without the Microsoft certificates unless
/// `without_microsoft` says so.
pub fn require_microsoft(
    microsoft: &MicrosoftCerts,
    without_microsoft: bool,
) -> Result<(), InstallError> {
    if microsoft.is_empty() && !without_microsoft {
        return Err(InstallError::NoMicrosoftCerts);
    }
    Ok(())
}

/// The full argv sequence of an enrollment. Refuses to enroll without the
/// Microsoft certificates unless `without_microsoft` says so.
pub fn enrollment_steps(e: &Enrollment) -> Result<EnrollmentSteps, InstallError> {
    require_microsoft(&e.microsoft, e.without_microsoft)?;
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
    let ts = efi_time(e.timestamp);
    steps.push(cmd(
        "sign-efi-sig-list",
        &[
            &"-t",
            &ts,
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
            &"-t",
            &ts,
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
            &"-t",
            &ts,
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
    let writes = vec![
        (
            Var::Db,
            cmd("efi-updatevar", &[&"-a", &"-f", &work("db.auth"), &"db"]),
        ),
        (
            Var::Kek,
            cmd("efi-updatevar", &[&"-a", &"-f", &work("KEK.auth"), &"KEK"]),
        ),
        (
            Var::Pk,
            cmd("efi-updatevar", &[&"-f", &work("PK.auth"), &"PK"]),
        ),
    ];
    Ok(EnrollmentSteps {
        prepare: steps,
        writes,
    })
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
            timestamp: 1_793_734_685,
        }
    }

    /// The preparation and the writes, in the order they run.
    fn flat(steps: EnrollmentSteps) -> Vec<Step> {
        let writes = steps.writes.into_iter().map(|(_, s)| s);
        steps.prepare.into_iter().chain(writes).collect()
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

    /// Answers `openssl` with a fixed line and records the call.
    struct Openssl(Result<String, ()>, std::cell::RefCell<Vec<String>>);

    impl Runner for Openssl {
        fn run(&self, _: &str, _: &[OsString]) -> Result<(), tetsu_tx::boot::BootError> {
            unreachable!()
        }
        fn output(&self, p: &str, a: &[OsString]) -> Result<String, tetsu_tx::boot::BootError> {
            let args: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
            self.1.borrow_mut().push(format!("{p} {}", args.join(" ")));
            self.0
                .clone()
                .map_err(|()| tetsu_tx::boot::BootError::ToolMissing { tool: p.to_owned() })
        }
    }

    fn fingerprint(label: &str) -> String {
        let hex: Vec<String> = (0u8..0x20)
            .map(|i| format!("{:02X}", if i < 16 { i * 0x11 } else { i }))
            .collect();
        format!("{label}={}\n", hex.join(":"))
    }

    #[test]
    fn the_derived_guid_is_a_v8_guid() {
        let mut b = [0u8; 16];
        for (i, x) in b.iter_mut().enumerate() {
            *x = u8::try_from(i).unwrap() * 0x11;
        }
        assert_eq!(derived_guid(b), "00112233-4455-8677-8899-aabbccddeeff");
        assert_eq!(
            derived_guid([0xff; 16]),
            "ffffffff-ffff-8fff-bfff-ffffffffffff"
        );
    }

    #[test]
    fn the_owner_guid_comes_from_the_pk_fingerprint() {
        for label in ["SHA256 Fingerprint", "sha256 Fingerprint"] {
            let r = Openssl(Ok(fingerprint(label)), Default::default());
            let g = owner_guid(&r, Path::new("/k/PK.crt")).unwrap();
            assert_eq!(g, "00112233-4455-8677-8899-aabbccddeeff");
            assert_eq!(
                *r.1.borrow(),
                ["openssl x509 -in /k/PK.crt -noout -fingerprint -sha256"]
            );
            let again = owner_guid(&r, Path::new("/k/PK.crt")).unwrap();
            assert_eq!(g, again);
        }
    }

    #[test]
    fn a_malformed_or_short_fingerprint_is_an_error() {
        let full = fingerprint("SHA256 Fingerprint");
        let short = full.trim_end().rsplit_once(':').unwrap().0.to_owned();
        for out in [
            String::new(),
            "no equals sign\n".to_owned(),
            short,
            full.replace("00:11", "zz:11"),
            format!("{}:AA\n", full.trim_end()),
            "SHA256 Fingerprint=\n".to_owned(),
        ] {
            let r = Openssl(Ok(out.clone()), Default::default());
            assert!(owner_guid(&r, Path::new("/k/PK.crt")).is_err(), "{out:?}");
        }
        let r = Openssl(Err(()), Default::default());
        assert!(owner_guid(&r, Path::new("/k/PK.crt")).is_err());
    }

    #[test]
    fn efi_time_formats_utc_seconds() {
        assert_eq!(efi_time(0), "1970-01-01 00:00:00");
        assert_eq!(efi_time(1_709_208_000), "2024-02-29 12:00:00");
        assert_eq!(efi_time(951_868_799), "2000-02-29 23:59:59");
        assert_eq!(efi_time(4_107_542_400), "2100-03-01 00:00:00");
        assert_eq!(efi_time(1_793_734_685), "2026-11-03 19:38:05");
    }

    #[test]
    fn own_keys_only() {
        let got = lines(&flat(
            enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap(),
        ));
        let guid = "11111111-2222-4333-8444-555555555555";
        assert_eq!(
            got,
            [
                format!("cert-to-efi-sig-list -g {guid} /k/PK.crt /w/PK.own.esl"),
                format!("cert-to-efi-sig-list -g {guid} /k/KEK.crt /w/KEK.own.esl"),
                format!("cert-to-efi-sig-list -g {guid} /k/db.crt /w/db.own.esl"),
                "cat /w/KEK.own.esl > /w/KEK.esl".to_owned(),
                "cat /w/db.own.esl > /w/db.esl".to_owned(),
                "sign-efi-sig-list -t 2026-11-03 19:38:05 -k /k/PK.key -c /k/PK.crt PK /w/PK.own.esl /w/PK.auth"
                    .to_owned(),
                "sign-efi-sig-list -a -t 2026-11-03 19:38:05 -k /k/PK.key -c /k/PK.crt KEK /w/KEK.esl /w/KEK.auth"
                    .to_owned(),
                "sign-efi-sig-list -a -t 2026-11-03 19:38:05 -k /k/KEK.key -c /k/KEK.crt db /w/db.esl /w/db.auth"
                    .to_owned(),
                "efi-updatevar -a -f /w/db.auth db".to_owned(),
                "efi-updatevar -a -f /w/KEK.auth KEK".to_owned(),
                "efi-updatevar -f /w/PK.auth PK".to_owned(),
            ]
        );
    }

    #[test]
    fn the_timestamp_is_one_argv_element_on_every_signature() {
        let steps = enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap();
        let signs: Vec<&Command> = steps
            .prepare
            .iter()
            .filter_map(|s| match s {
                Step::Run(c) if c.program == "sign-efi-sig-list" => Some(c),
                _ => None,
            })
            .collect();
        assert_eq!(signs.len(), 3);
        for c in signs {
            let at = c.args.iter().position(|a| a == "-t").unwrap();
            assert_eq!(c.args[at + 1], "2026-11-03 19:38:05");
        }
    }

    #[test]
    fn the_writes_are_tagged_by_variable() {
        let steps = enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap();
        let vars: Vec<Var> = steps.writes.iter().map(|(v, _)| *v).collect();
        assert_eq!(vars, [Var::Db, Var::Kek, Var::Pk]);
        assert!(
            steps
                .prepare
                .iter()
                .all(|s| !lines(std::slice::from_ref(s))[0].starts_with("efi-updatevar"))
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
        let got = lines(&flat(enrollment_steps(&enrollment(ms)).unwrap()));
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
        let got = lines(&flat(
            enrollment_steps(&enrollment(MicrosoftCerts::default())).unwrap(),
        ));
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
