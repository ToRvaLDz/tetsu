//! UKI assembly (`ukify`) and signing (`sbsign`, `sbverify`).

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::BootError;
use super::keys::SigningKeys;
use super::tool::Runner;

/// Everything `ukify` needs for one snapshot.
#[derive(Clone, Debug)]
pub struct UkiSpec {
    pub counter: u64,
    /// Root of the snapshot the UKI will boot.
    pub snapshot_root: PathBuf,
    pub kernel_version: String,
    pub base_cmdline: String,
    pub keys: SigningKeys,
}

impl UkiSpec {
    /// Reject inputs that cannot name a build: bad keys or a kernel version
    /// that is not a single normal path component.
    pub fn validate(&self) -> Result<(), BootError> {
        self.keys.validate()?;
        let v = self.kernel_version.as_str();
        if v.is_empty() || v == "." || v == ".." || v.contains(['/', '\0']) {
            return Err(BootError::InvalidKernelVersion(v.to_owned()));
        }
        Ok(())
    }

    /// Every input must be a regular file reached without any symlink
    /// component below the snapshot root, so a planted link cannot pull a
    /// host file into the signed image.
    pub fn check_inputs(&self) -> Result<(), BootError> {
        for path in [
            self.kernel_path(),
            self.initrd_path(),
            self.os_release_path(),
        ] {
            let rel = path
                .strip_prefix(&self.snapshot_root)
                .expect("inputs are joined onto the snapshot root");
            let mut cur = self.snapshot_root.clone();
            for part in rel.components() {
                cur.push(part);
                let meta = fs::symlink_metadata(&cur).map_err(|e| BootError::io(&cur, e))?;
                if meta.file_type().is_symlink() {
                    return Err(BootError::InputEscapesSnapshot { path: cur });
                }
            }
            if !fs::symlink_metadata(&cur)
                .map_err(|e| BootError::io(&cur, e))?
                .is_file()
            {
                let not_file = io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
                return Err(BootError::io(&cur, not_file));
            }
        }
        Ok(())
    }

    pub fn kernel_path(&self) -> PathBuf {
        self.snapshot_root
            .join("usr/lib/modules")
            .join(&self.kernel_version)
            .join("vmlinuz")
    }

    pub fn initrd_path(&self) -> PathBuf {
        self.snapshot_root.join("usr/lib/tetsu/initrd.img")
    }

    pub fn os_release_path(&self) -> PathBuf {
        self.snapshot_root.join("usr/lib/os-release")
    }

    /// Base command line plus the snapshot root subvolume.
    pub fn cmdline(&self) -> String {
        let root = format!("rootflags=subvol=@snapshots/{}/root", self.counter);
        match self.base_cmdline.trim() {
            "" => root,
            base => format!("{base} {root}"),
        }
    }
}

/// Read a text file, refusing a symlink at the last component (guards the
/// window between [`UkiSpec::check_inputs`] and the read).
pub(crate) fn read_no_follow(path: &Path) -> Result<String, BootError> {
    let mut text = String::new();
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)
        .and_then(|mut f| f.read_to_string(&mut text))
        .map_err(|e| BootError::io(path, e))?;
    Ok(text)
}

/// `os_release` with `IMAGE_VERSION=<counter>` set, replacing any existing one.
pub fn with_image_version(os_release: &str, counter: u64) -> String {
    let mut out: String = os_release
        .lines()
        .filter(|l| !l.trim_start().starts_with("IMAGE_VERSION="))
        .flat_map(|l| [l, "\n"])
        .collect();
    out.push_str(&format!("IMAGE_VERSION={counter}\n"));
    out
}

/// The `ukify build` argument vector. Every value is glued to its flag
/// (`--opt=value`) so a value with a leading dash is never read as an option.
pub fn ukify_args(spec: &UkiSpec, os_release: &str, out: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["build".into()];
    let mut flag = |name: &str, value: &OsStr| {
        let mut arg = OsString::from(name);
        arg.push("=");
        arg.push(value);
        args.push(arg);
    };
    flag("--linux", spec.kernel_path().as_os_str());
    flag("--initrd", spec.initrd_path().as_os_str());
    flag("--cmdline", spec.cmdline().as_ref());
    // ukify takes literal text when the value does not start with `@`.
    flag("--os-release", os_release.as_ref());
    flag("--uname", spec.kernel_version.as_ref());
    flag(
        "--secureboot-private-key",
        spec.keys.db_key.as_path().as_os_str(),
    );
    flag("--secureboot-certificate", spec.keys.db_cert.as_os_str());
    if let Some(pcr_key) = &spec.keys.pcr_key {
        flag("--pcr-private-key", pcr_key.as_path().as_os_str());
        if let Some(public) = &spec.keys.pcr_public {
            flag("--pcr-public-key", public.as_os_str());
        }
    }
    flag("--output", out.as_os_str());
    args
}

/// Cheap checks that must pass before anything on the ESP is changed:
/// valid spec, `ukify` installed, confined and readable inputs.
pub fn preflight(runner: &dyn Runner, spec: &UkiSpec) -> Result<(), BootError> {
    spec.validate()?;
    runner.check_tool("ukify")?;
    spec.check_inputs()?;
    for path in [
        spec.kernel_path(),
        spec.initrd_path(),
        spec.os_release_path(),
    ] {
        fs::File::open(&path).map_err(|e| BootError::io(&path, e))?;
    }
    Ok(())
}

/// Build and sign the UKI into `out`.
pub fn build_uki(runner: &dyn Runner, spec: &UkiSpec, out: &Path) -> Result<(), BootError> {
    spec.validate()?;
    spec.check_inputs()?;
    let path = spec.os_release_path();
    let text = read_no_follow(&path)?;
    let os_release = with_image_version(&text, spec.counter);
    runner.run("ukify", &ukify_args(spec, &os_release, out))
}

/// Sign an EFI binary (systemd-boot) with the db key.
pub fn sign_efi(
    runner: &dyn Runner,
    input: &Path,
    output: &Path,
    keys: &SigningKeys,
) -> Result<(), BootError> {
    keys.validate()?;
    let glued = |name: &str, value: &OsStr| {
        let mut arg = OsString::from(name);
        arg.push("=");
        arg.push(value);
        arg
    };
    let args: Vec<OsString> = vec![
        glued("--key", keys.db_key.as_path().as_os_str()),
        glued("--cert", keys.db_cert.as_os_str()),
        glued("--output", output.as_os_str()),
        "--".into(),
        input.into(),
    ];
    runner.run("sbsign", &args)
}

/// `sbverify --cert=<cert> -- <path>`.
pub fn verify_signature(runner: &dyn Runner, path: &Path, cert: &Path) -> Result<(), BootError> {
    let mut cert_arg = OsString::from("--cert=");
    cert_arg.push(cert);
    runner.run("sbverify", &[cert_arg, "--".into(), path.into()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::keys::KeyRef;
    use std::cell::RefCell;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<(String, Vec<String>)>>);

    impl Runner for Recorder {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let args = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            self.0.borrow_mut().push((program.to_owned(), args));
            Ok(())
        }
    }

    fn keys(pcr: bool) -> SigningKeys {
        SigningKeys {
            db_key: KeyRef::File("/k/db.key".into()),
            db_cert: "/k/db.crt".into(),
            pcr_key: pcr.then(|| KeyRef::File("/k/pcr.key".into())),
            pcr_public: pcr.then(|| "/k/pcr.pub".into()),
        }
    }

    fn spec(root: &Path, pcr: bool) -> UkiSpec {
        UkiSpec {
            counter: 7,
            snapshot_root: root.to_path_buf(),
            kernel_version: "6.1.0".into(),
            base_cmdline: "quiet rw".into(),
            keys: keys(pcr),
        }
    }

    fn snapshot(os_release: &str) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let lib = t.path().join("usr/lib");
        fs::create_dir_all(lib.join("modules/6.1.0")).unwrap();
        fs::create_dir_all(lib.join("tetsu")).unwrap();
        fs::write(lib.join("modules/6.1.0/vmlinuz"), b"k").unwrap();
        fs::write(lib.join("tetsu/initrd.img"), b"i").unwrap();
        fs::write(lib.join("os-release"), os_release).unwrap();
        t
    }

    /// Value of an `--name=value` argument.
    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .find_map(|a| a.strip_prefix(name)?.strip_prefix('='))
    }

    #[test]
    fn inputs_live_inside_the_snapshot() {
        let s = spec(Path::new("/snap"), false);
        assert_eq!(
            s.kernel_path(),
            Path::new("/snap/usr/lib/modules/6.1.0/vmlinuz")
        );
        assert_eq!(s.initrd_path(), Path::new("/snap/usr/lib/tetsu/initrd.img"));
        assert_eq!(s.os_release_path(), Path::new("/snap/usr/lib/os-release"));
    }

    #[test]
    fn cmdline_appends_rootflags() {
        let s = spec(Path::new("/s"), false);
        assert_eq!(s.cmdline(), "quiet rw rootflags=subvol=@snapshots/7/root");
        let mut s = s;
        s.base_cmdline = String::new();
        assert_eq!(s.cmdline(), "rootflags=subvol=@snapshots/7/root");
    }

    #[test]
    fn image_version_is_appended() {
        let out = with_image_version("NAME=Tetsu\nID=tetsu\n", 7);
        assert_eq!(out, "NAME=Tetsu\nID=tetsu\nIMAGE_VERSION=7\n");
        let out = with_image_version("NAME=Tetsu", 7);
        assert_eq!(out, "NAME=Tetsu\nIMAGE_VERSION=7\n");
    }

    #[test]
    fn image_version_replaces_existing() {
        let out = with_image_version("A=1\nIMAGE_VERSION=\"old\"\nB=2\n", 9);
        assert_eq!(out.matches("IMAGE_VERSION").count(), 1);
        assert!(out.contains("IMAGE_VERSION=9\n"));
        assert!(out.contains("A=1\n") && out.contains("B=2\n"));
        assert!(!out.contains("old"));
    }

    #[test]
    fn ukify_argv_without_pcr() {
        let snap = snapshot("NAME=Tetsu\n");
        let rec = Recorder::default();
        build_uki(&rec, &spec(snap.path(), false), Path::new("/esp/x.tmp")).unwrap();
        let calls = rec.0.borrow();
        assert_eq!(calls.len(), 1);
        let (prog, args) = &calls[0];
        assert_eq!(prog, "ukify");
        assert_eq!(args[0], "build");
        let root = snap.path().display();
        assert_eq!(
            flag(args, "--linux"),
            Some(format!("{root}/usr/lib/modules/6.1.0/vmlinuz").as_str())
        );
        assert_eq!(
            flag(args, "--initrd"),
            Some(format!("{root}/usr/lib/tetsu/initrd.img").as_str())
        );
        assert_eq!(
            flag(args, "--cmdline"),
            Some("quiet rw rootflags=subvol=@snapshots/7/root")
        );
        assert_eq!(flag(args, "--uname"), Some("6.1.0"));
        assert_eq!(flag(args, "--secureboot-private-key"), Some("/k/db.key"));
        assert_eq!(flag(args, "--secureboot-certificate"), Some("/k/db.crt"));
        assert_eq!(flag(args, "--output"), Some("/esp/x.tmp"));
        assert!(!args.iter().any(|a| a.starts_with("--pcr")));
    }

    #[test]
    fn ukify_os_release_carries_image_version() {
        let snap = snapshot("NAME=Tetsu\nIMAGE_VERSION=old\n");
        let rec = Recorder::default();
        build_uki(&rec, &spec(snap.path(), false), Path::new("/o")).unwrap();
        let calls = rec.0.borrow();
        let osrel = flag(&calls[0].1, "--os-release").unwrap();
        assert!(osrel.contains("NAME=Tetsu"));
        assert!(osrel.contains("IMAGE_VERSION=7"));
        assert!(!osrel.contains("old"));
    }

    #[test]
    fn pcr_flags_only_with_a_pcr_key() {
        let snap = snapshot("NAME=Tetsu\n");
        let rec = Recorder::default();
        build_uki(&rec, &spec(snap.path(), true), Path::new("/o")).unwrap();
        let calls = rec.0.borrow();
        let args = &calls[0].1;
        assert_eq!(flag(args, "--pcr-private-key"), Some("/k/pcr.key"));
        assert_eq!(flag(args, "--pcr-public-key"), Some("/k/pcr.pub"));
    }

    #[test]
    fn missing_os_release_is_an_io_error() {
        let t = tempfile::tempdir().unwrap();
        let rec = Recorder::default();
        let err = build_uki(&rec, &spec(t.path(), false), Path::new("/o")).unwrap_err();
        assert!(matches!(err, BootError::Io { .. }));
        assert!(rec.0.borrow().is_empty());
    }

    #[test]
    fn kernel_version_must_be_one_normal_component() {
        let snap = snapshot("NAME=T\n");
        for bad in ["", ".", "..", "../x", "a/b", "/abs", "6.1/", "a\0b"] {
            let mut s = spec(snap.path(), false);
            s.kernel_version = bad.into();
            let rec = Recorder::default();
            let err = build_uki(&rec, &s, Path::new("/o")).unwrap_err();
            assert!(
                matches!(err, BootError::InvalidKernelVersion(_)),
                "{bad:?}: {err:?}"
            );
            assert!(rec.0.borrow().is_empty());
        }
    }

    fn assert_refused(snap: &TempDir) {
        let rec = Recorder::default();
        let err = build_uki(&rec, &spec(snap.path(), false), Path::new("/o")).unwrap_err();
        assert!(
            matches!(err, BootError::InputEscapesSnapshot { .. }),
            "{err:?}"
        );
        assert!(rec.0.borrow().is_empty(), "ukify must not run");
    }

    #[test]
    fn symlinked_initrd_is_refused() {
        let snap = snapshot("NAME=T\n");
        let secret = snap.path().join("../secret.key");
        fs::write(&secret, b"secret").unwrap();
        let initrd = snap.path().join("usr/lib/tetsu/initrd.img");
        fs::remove_file(&initrd).unwrap();
        symlink(&secret, &initrd).unwrap();
        assert_refused(&snap);
        fs::remove_file(secret).unwrap();
    }

    #[test]
    fn symlinked_kernel_and_os_release_are_refused() {
        for rel in ["usr/lib/modules/6.1.0/vmlinuz", "usr/lib/os-release"] {
            let snap = snapshot("NAME=T\n");
            let p = snap.path().join(rel);
            let target = snap.path().join("inside");
            fs::write(&target, b"x").unwrap();
            fs::remove_file(&p).unwrap();
            symlink(&target, &p).unwrap();
            assert_refused(&snap);
        }
    }

    #[test]
    fn symlinked_directory_component_is_refused() {
        let snap = snapshot("NAME=T\n");
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("initrd.img"), b"host").unwrap();
        let dir = snap.path().join("usr/lib/tetsu");
        fs::remove_dir_all(&dir).unwrap();
        symlink(outside.path(), &dir).unwrap();
        assert_refused(&snap);
    }

    #[test]
    fn missing_input_is_an_io_error() {
        let snap = snapshot("NAME=T\n");
        fs::remove_file(snap.path().join("usr/lib/tetsu/initrd.img")).unwrap();
        let rec = Recorder::default();
        let err = build_uki(&rec, &spec(snap.path(), false), Path::new("/o")).unwrap_err();
        assert!(matches!(err, BootError::Io { .. }), "{err:?}");
        assert!(rec.0.borrow().is_empty());
    }

    #[test]
    fn sbsign_argv() {
        let rec = Recorder::default();
        sign_efi(
            &rec,
            Path::new("/in/boot.efi"),
            Path::new("/out/boot.efi"),
            &keys(true),
        )
        .unwrap();
        let calls = rec.0.borrow();
        let (prog, args) = &calls[0];
        assert_eq!(prog, "sbsign");
        assert_eq!(flag(args, "--key"), Some("/k/db.key"));
        assert_eq!(flag(args, "--cert"), Some("/k/db.crt"));
        assert_eq!(flag(args, "--output"), Some("/out/boot.efi"));
        let n = args.len();
        assert_eq!(&args[n - 2..], ["--", "/in/boot.efi"]);
    }

    #[test]
    fn sbverify_argv() {
        let rec = Recorder::default();
        verify_signature(&rec, Path::new("/e/a.efi"), Path::new("/k/db.crt")).unwrap();
        let calls = rec.0.borrow();
        let (prog, args) = &calls[0];
        assert_eq!(prog, "sbverify");
        assert_eq!(args, &["--cert=/k/db.crt", "--", "/e/a.efi"]);
    }

    #[test]
    fn leading_dash_values_stay_values() {
        let snap = snapshot("NAME=T\n");
        let mut s = spec(snap.path(), false);
        s.keys.db_key = KeyRef::File("-db.key".into());
        s.base_cmdline = "-x".into();
        let rec = Recorder::default();
        build_uki(&rec, &s, Path::new("-out.efi")).unwrap();
        let calls = rec.0.borrow();
        let args = &calls[0].1;
        assert_eq!(flag(args, "--secureboot-private-key"), Some("-db.key"));
        assert_eq!(flag(args, "--output"), Some("-out.efi"));
        assert_eq!(
            flag(args, "--cmdline"),
            Some("-x rootflags=subvol=@snapshots/7/root")
        );
        // Every value is glued to its flag: no bare argument but the verb.
        assert!(args[1..].iter().all(|a| a.starts_with("--")), "{args:?}");
    }

    #[test]
    fn pcr_public_without_pcr_key_is_refused() {
        let snap = snapshot("NAME=T\n");
        let mut s = spec(snap.path(), false);
        s.keys.pcr_public = Some("/k/pcr.pub".into());
        let rec = Recorder::default();
        let err = build_uki(&rec, &s, Path::new("/o")).unwrap_err();
        assert!(matches!(err, BootError::InvalidKeys(_)), "{err:?}");
        assert!(rec.0.borrow().is_empty());
    }
}
