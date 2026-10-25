//! The bless command: it freezes the current IDL only when the suite passes
//! against the previous baseline, and otherwise changes nothing. Runs on a
//! scratch copy of the IDL directory.

use std::fs;
use std::path::{Path, PathBuf};

use tetsu_bus_contract::{BlessError, Kind, bless};

/// A scratch copy of `idl/` whose baseline is the copied IDL itself, so a
/// test starts from a blessed tree whatever the real baseline lags behind.
/// Removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tetsu-bless-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        copy_dir(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl"),
            &root,
        );
        let baseline = root.join("baseline");
        fs::remove_dir_all(&baseline).unwrap();
        fs::create_dir(&baseline).unwrap();
        for entry in fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                fs::copy(&path, baseline.join(path.file_name().unwrap())).unwrap();
            }
        }
        Scratch(root)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn baseline(&self, name: &str) -> PathBuf {
        self.0.join("baseline").join(name)
    }

    /// Replaces the first `from` in an IDL file by `to`.
    fn edit(&self, name: &str, from: &str, to: &str) {
        let path = self.file(name);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "{from:?} not in {name}");
        fs::write(&path, text.replacen(from, to, 1)).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

const WINDOW: &str = "org.tetsu.comp.window.v1.varlink";

#[test]
fn bless_of_an_unchanged_idl_changes_nothing() {
    let s = Scratch::new("unchanged");
    let changed = bless(&s.0).expect("current tree blesses");
    assert!(changed.is_empty(), "{changed:?}");
}

#[test]
fn bless_copies_an_additive_change() {
    let s = Scratch::new("additive");
    s.edit(
        WINDOW,
        "xwayland: bool\n)",
        "xwayland: bool,\n  # Added later.\n  sticky: ?bool\n)",
    );
    let changed = bless(&s.0).expect("additive change blesses");
    assert_eq!(changed, [WINDOW]);
    assert_eq!(
        fs::read_to_string(s.file(WINDOW)).unwrap(),
        fs::read_to_string(s.baseline(WINDOW)).unwrap()
    );
}

#[test]
fn bless_refuses_a_breaking_change_and_touches_nothing() {
    let s = Scratch::new("breaking");
    let before = fs::read_to_string(s.baseline(WINDOW)).unwrap();
    s.edit(WINDOW, "fullscreen: bool,", "fullscreen: int,");
    match bless(&s.0) {
        Err(BlessError::Violations(v)) => {
            assert!(v.iter().any(|x| x.kind == Kind::Retyped), "{v:#?}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(s.baseline(WINDOW)).unwrap(), before);
}

#[test]
fn bless_refuses_a_rule_violation_and_touches_nothing() {
    let s = Scratch::new("rule");
    let before = fs::read_to_string(s.baseline(WINDOW)).unwrap();
    s.edit(
        WINDOW,
        "# Lists all windows and the focused one.\n",
        "# Lists all windows and the focused one.\n# @bogus\n",
    );
    match bless(&s.0) {
        Err(BlessError::Violations(v)) => {
            assert!(v.iter().any(|x| x.kind == Kind::UnknownTag), "{v:#?}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(s.baseline(WINDOW)).unwrap(), before);
}

#[test]
fn bless_that_fails_midway_restores_the_baseline() {
    let s = Scratch::new("midway");
    let before = fs::read_to_string(s.baseline(WINDOW)).unwrap();
    // An additive change that is written first ...
    s.edit(
        WINDOW,
        "xwayland: bool\n)",
        "xwayland: bool,\n  # Added later.\n  sticky: ?bool\n)",
    );
    // ... and a new interface whose baseline slot cannot be written: a
    // directory sits where the file must go.
    let late = "org.tetsu.zzz.v1.varlink";
    fs::write(
        s.file(late),
        "interface org.tetsu.zzz.v1\n\n# Pings.\n# @capability test.ping\nmethod Ping() -> ()\n\n# The caller may not use this interface.\n# @permanent\nerror PermissionDenied ()\n",
    )
    .unwrap();
    fs::create_dir(s.baseline(late)).unwrap();

    match bless(&s.0) {
        Err(BlessError::Io(_)) => {}
        other => panic!("expected an I/O failure, got {other:?}"),
    }
    assert_eq!(fs::read_to_string(s.baseline(WINDOW)).unwrap(), before);
    let leftovers: Vec<String> = fs::read_dir(s.0.join("baseline"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
