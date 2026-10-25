//! Finding the answer file.
//!
//! The file lives on a filesystem labelled `TETSU-ANSWERS` (a small image or
//! USB stick); a FAT filesystem, whose label holds 11 characters, is labelled
//! `TETSU-ANS`. The kernel command line may name it:
//!
//! ```text
//! tetsu.answers=/path/in/that/filesystem.toml
//! ```
//!
//! The path is relative to the root of the labelled filesystem. Without the
//! parameter, a labelled filesystem is still used, with `/answers.toml`.
//! The parameter without a labelled filesystem is an error, so a mistyped
//! unattended install never falls back to asking questions on a machine
//! nobody watches. The filesystem is mounted read-only just long enough to
//! read the file.

use std::ffi::OsString;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use tetsu_installer_core::error::InstallError;
use tetsu_installer_core::install::rundir::RunDir;
use tetsu_tx::boot::tool::Runner;

/// Filesystem label of the volume holding the answer file.
pub const LABEL: &str = "TETSU-ANSWERS";
/// The same volume as a FAT filesystem, whose labels hold 11 characters.
pub const FAT_LABEL: &str = "TETSU-ANS";
/// File read when the command line gives no path.
pub const DEFAULT_PATH: &str = "/answers.toml";
const CMDLINE_KEY: &str = "tetsu.answers=";
/// Larger files are not answer files.
const MAX_BYTES: u64 = 64 * 1024;

/// The path given by `tetsu.answers=`; the last occurrence wins.
pub fn cmdline_path(cmdline: &str) -> Option<&str> {
    cmdline
        .split_whitespace()
        .rev()
        .find_map(|w| w.strip_prefix(CMDLINE_KEY))
}

/// `path` relative to the volume root, refusing anything that climbs out.
fn relative(path: &str) -> Result<PathBuf, InstallError> {
    let rel = Path::new(path.trim_start_matches('/'));
    let clean = !path.is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)));
    if clean && rel.components().next().is_some() {
        Ok(rel.to_path_buf())
    } else {
        Err(InstallError::Failed(format!(
            "tetsu.answers={path}: use a plain path inside the {LABEL} volume"
        )))
    }
}

/// Where the machine looks for the volume and which command line it read.
pub struct Source<'a> {
    /// Root under which `/dev/disk/by-label` is looked up (`/` in production).
    pub root: &'a Path,
    pub cmdline: &'a str,
    pub run_dir: &'a Path,
}

/// The text of the answer file, or `None` when the machine offers none.
pub fn load(src: &Source<'_>, runner: &dyn Runner) -> Result<Option<String>, InstallError> {
    let requested = cmdline_path(src.cmdline);
    let device = [LABEL, FAT_LABEL]
        .iter()
        .map(|l| Path::new("/dev/disk/by-label").join(l))
        .find(|d| src.root.join(d.strip_prefix("/").unwrap_or(d)).exists());
    let Some(device) = device else {
        return match requested {
            Some(p) => Err(InstallError::Failed(format!(
                "tetsu.answers={p}: no filesystem labelled {LABEL} found \
                 ({FAT_LABEL} for a FAT volume)"
            ))),
            None => Ok(None),
        };
    };
    let rel = relative(requested.unwrap_or(DEFAULT_PATH))?;
    let mount = RunDir::create(src.run_dir)?.subdir("answers")?;
    let mount_args: [OsString; 4] = [
        "-o".into(),
        "ro,nodev,noexec,nosuid".into(),
        device.into(),
        mount.clone().into(),
    ];
    runner.run("mount", &mount_args)?;
    let text = read_limited(&mount.join(&rel));
    // Unmount whatever the read gave, and report a failed unmount only when
    // the read itself worked.
    let unmounted = runner.run("umount", &[mount.into()]);
    let text = text?;
    unmounted?;
    Ok(Some(text))
}

fn io_error(path: &Path, source: std::io::Error) -> InstallError {
    InstallError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn read_limited(path: &Path) -> Result<String, InstallError> {
    let file = fs::File::open(path).map_err(|e| io_error(path, e))?;
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| io_error(path, e))?;
    if text.len() as u64 > MAX_BYTES {
        return Err(InstallError::Failed(format!(
            "{}: too large for an answer file",
            path.display()
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use tetsu_tx::boot::BootError;

    use super::*;

    /// Serves `files` as the content of the labelled volume on `mount`.
    struct Volume {
        files: Vec<(&'static str, &'static str)>,
        calls: RefCell<Vec<String>>,
    }

    impl Volume {
        fn new(files: Vec<(&'static str, &'static str)>) -> Self {
            Self {
                files,
                calls: RefCell::default(),
            }
        }
    }

    impl Runner for Volume {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            self.calls
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            if program == "mount" {
                for (rel, text) in &self.files {
                    let path = Path::new(args.last().unwrap()).join(rel);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, text).unwrap();
                }
            }
            Ok(())
        }

        fn output(&self, _: &str, _: &[OsString]) -> Result<String, BootError> {
            unreachable!()
        }
    }

    fn machine(labelled: bool) -> tempfile::TempDir {
        machine_labelled(labelled.then_some(LABEL))
    }

    fn machine_labelled(label: Option<&str>) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        if let Some(label) = label {
            let dir = t.path().join("dev/disk/by-label");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(label), "").unwrap();
        }
        t
    }

    fn load_from(
        t: &tempfile::TempDir,
        cmdline: &str,
        vol: &Volume,
    ) -> Result<Option<String>, InstallError> {
        let run = t.path().join("run");
        load(
            &Source {
                root: t.path(),
                cmdline,
                run_dir: &run,
            },
            vol,
        )
    }

    #[test]
    fn the_last_command_line_parameter_wins() {
        assert_eq!(cmdline_path("quiet tetsu.answers=/a.toml"), Some("/a.toml"));
        assert_eq!(
            cmdline_path("tetsu.answers=/a tetsu.answers=/b"),
            Some("/b")
        );
        assert_eq!(cmdline_path("quiet xtetsu.answers=/a"), None);
        assert_eq!(cmdline_path(""), None);
    }

    #[test]
    fn no_volume_and_no_parameter_means_interactive() {
        let t = machine(false);
        let vol = Volume::new(vec![]);
        assert_eq!(load_from(&t, "quiet", &vol).unwrap(), None);
        assert!(vol.calls.borrow().is_empty());
    }

    #[test]
    fn a_parameter_without_a_volume_is_an_error() {
        let t = machine(false);
        let err = load_from(&t, "tetsu.answers=/a.toml", &Volume::new(vec![])).unwrap_err();
        assert!(
            err.to_string()
                .contains("no filesystem labelled TETSU-ANSWERS")
        );
    }

    #[test]
    fn the_volume_alone_gives_the_default_file_mounted_read_only() {
        let t = machine(true);
        let vol = Volume::new(vec![("answers.toml", "[install]\nconfirm = true\n")]);
        let text = load_from(&t, "quiet", &vol).unwrap().unwrap();
        assert_eq!(text, "[install]\nconfirm = true\n");
        let calls = vol.calls.borrow();
        assert!(
            calls[0]
                .starts_with("mount -o ro,nodev,noexec,nosuid /dev/disk/by-label/TETSU-ANSWERS ")
        );
        assert!(calls[1].starts_with("umount "));
    }

    #[test]
    fn a_fat_volume_with_the_short_label_is_found_too() {
        let t = machine_labelled(Some(FAT_LABEL));
        let vol = Volume::new(vec![("answers.toml", "x = 1\n")]);
        let text = load_from(&t, "quiet", &vol).unwrap();
        assert_eq!(text.as_deref(), Some("x = 1\n"));
        assert!(vol.calls.borrow()[0].contains("/dev/disk/by-label/TETSU-ANS "));
    }

    #[test]
    fn the_parameter_names_a_path_inside_the_volume() {
        let t = machine(true);
        let vol = Volume::new(vec![
            ("tests/paranoid.toml", "x = 1\n"),
            ("answers.toml", "no"),
        ]);
        let text = load_from(&t, "tetsu.answers=/tests/paranoid.toml", &vol).unwrap();
        assert_eq!(text.as_deref(), Some("x = 1\n"));
    }

    #[test]
    fn paths_that_climb_out_are_refused_before_any_mount() {
        let t = machine(true);
        for bad in ["../x", "/a/../../x", "/"] {
            let vol = Volume::new(vec![]);
            let cmdline = format!("tetsu.answers={bad}");
            assert!(load_from(&t, &cmdline, &vol).is_err(), "{bad}");
            assert!(vol.calls.borrow().is_empty(), "{bad}");
        }
    }

    #[test]
    fn the_volume_is_unmounted_even_when_the_file_is_missing() {
        let t = machine(true);
        let vol = Volume::new(vec![]);
        assert!(load_from(&t, "quiet", &vol).is_err());
        assert!(vol.calls.borrow()[1].starts_with("umount "));
    }
}
