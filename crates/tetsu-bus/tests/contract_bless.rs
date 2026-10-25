//! The bless command: it freezes the current IDL only when the suite passes
//! against the previous baseline, and otherwise changes nothing. Runs on a
//! scratch copy of the IDL directory.

use std::fs;
use std::path::{Path, PathBuf};

use tetsu_bus_contract::{BlessError, Kind, bless};

/// A scratch copy of `idl/`, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("tetsu-bless-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        copy_dir(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl"),
            &root,
        );
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
