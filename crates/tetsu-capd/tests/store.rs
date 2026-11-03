//! The three-layer store over files in a tempdir.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use tempfile::TempDir;
use tetsu_bus::cap::{Grant, Layer, State};
use tetsu_capd::store::{Layers, Store, StoreError};

struct Fixture {
    dir: TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let layers = Layers {
            image: dir.path().join("image/capabilities.toml"),
            managed: dir.path().join("state/tetsu/capabilities.toml"),
            user: dir.path().join("config/tetsu/capabilities.toml"),
        };
        Self {
            dir,
            store: Store::new(layers),
        }
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.dir.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn state(&self, class: &str, scope: Option<&str>) -> (State, Option<Layer>) {
        let e = self.store.effective(class, scope);
        (e.state, e.layer)
    }
}

const IMAGE: &str = "image/capabilities.toml";
const MANAGED: &str = "state/tetsu/capabilities.toml";
const USER: &str = "config/tetsu/capabilities.toml";

#[test]
fn nothing_set_is_unset_and_missing_files_are_empty_layers() {
    let mut f = Fixture::new();
    f.store.refresh().unwrap();
    assert_eq!(f.state("packages.install", None), (State::Unset, None));
}

#[test]
fn each_layer_alone_reports_itself() {
    for (file, layer) in [
        (IMAGE, Layer::Image),
        (MANAGED, Layer::Managed),
        (USER, Layer::User),
    ] {
        let mut f = Fixture::new();
        f.write(file, "[class]\n\"packages.install\" = \"notify\"\n");
        f.store.refresh().unwrap();
        assert_eq!(
            f.state("packages.install", None),
            (State::Notify, Some(layer)),
            "{file}"
        );
    }
}

#[test]
fn the_nearest_layer_wins_key_by_key() {
    let mut f = Fixture::new();
    f.write(
        IMAGE,
        "[class]\n\"packages.install\" = \"automatic\"\n\"packages.remove\" = \"ask\"\n\"aur.review\" = \"off\"\n",
    );
    f.write(
        MANAGED,
        "[class]\n\"packages.install\" = \"notify\"\n\"packages.remove\" = \"notify\"\n",
    );
    f.write(USER, "[class]\n\"packages.install\" = \"off\"\n");
    f.store.refresh().unwrap();
    assert_eq!(
        f.state("packages.install", None),
        (State::Off, Some(Layer::User))
    );
    assert_eq!(
        f.state("packages.remove", None),
        (State::Notify, Some(Layer::Managed))
    );
    assert_eq!(
        f.state("aur.review", None),
        (State::Off, Some(Layer::Image))
    );
}

#[test]
fn a_scoped_key_wins_over_the_class_key() {
    let mut f = Fixture::new();
    f.write(
        MANAGED,
        "[class]\n\"packages.install\" = \"automatic\"\n[scope.\"packages.install\"]\nwork = \"ask\"\n",
    );
    f.write(USER, "[scope.\"packages.install\"]\nhome = \"off\"\n");
    f.store.refresh().unwrap();
    assert_eq!(
        f.state("packages.install", Some("work")),
        (State::Ask, Some(Layer::Managed))
    );
    assert_eq!(
        f.state("packages.install", Some("home")),
        (State::Off, Some(Layer::User))
    );
    // A scope nobody set falls back to the class key.
    assert_eq!(
        f.state("packages.install", Some("other")),
        (State::Automatic, Some(Layer::Managed))
    );
    assert_eq!(
        f.state("packages.install", None),
        (State::Automatic, Some(Layer::Managed))
    );
    let mut keys = f.store.scoped_keys();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            ("packages.install".to_owned(), "home".to_owned()),
            ("packages.install".to_owned(), "work".to_owned()),
        ]
    );
}

#[test]
fn classes_the_catalog_does_not_have_are_ignored() {
    let mut f = Fixture::new();
    f.write(
        IMAGE,
        "[class]\n\"made.up\" = \"off\"\n\"packages.install\" = \"ask\"\n",
    );
    f.store.refresh().unwrap();
    assert_eq!(f.state("made.up", None), (State::Unset, None));
    assert_eq!(
        f.state("packages.install", None),
        (State::Ask, Some(Layer::Image))
    );
}

#[test]
fn a_malformed_layer_names_the_file_and_line_and_keeps_the_last_good_parse() {
    let mut f = Fixture::new();
    f.write(USER, "[class]\n\"packages.install\" = \"off\"\n");
    f.store.refresh().unwrap();
    f.write(
        USER,
        "[class]\n\"packages.install\" = \"off\"\n\"aur.review\" = \"sometimes\"\n",
    );
    match f.store.refresh() {
        Err(StoreError::Malformed { path, line, .. }) => {
            assert!(path.ends_with("config/tetsu/capabilities.toml"));
            assert_eq!(line, 3);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        f.state("packages.install", None),
        (State::Off, Some(Layer::User)),
        "the last good parse is kept"
    );
    f.write(USER, "not toml [");
    assert!(matches!(
        f.store.refresh(),
        Err(StoreError::Malformed { .. })
    ));
}

#[test]
fn a_hand_edit_is_picked_up_by_the_next_refresh() {
    let mut f = Fixture::new();
    f.store.refresh().unwrap();
    assert_eq!(f.state("aur.review", None).0, State::Unset);
    f.write(USER, "[class]\n\"aur.review\" = \"ask\"\n");
    f.store.refresh().unwrap();
    assert_eq!(f.state("aur.review", None).0, State::Ask);
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn set_managed_writes_atomically_and_keeps_other_keys() {
    let mut f = Fixture::new();
    f.store
        .set_managed("packages.install", None, Some(Grant::Notify))
        .unwrap();
    f.store
        .set_managed("packages.install", Some("work"), Some(Grant::Ask))
        .unwrap();
    f.store
        .set_managed("aur.review", None, Some(Grant::Off))
        .unwrap();
    let path = f.dir.path().join(MANAGED);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
    assert!(
        !path.with_extension("toml.tmp").exists(),
        "no temp file left"
    );

    // A fresh store reads the same grants back.
    let mut again = Store::new(f.store.layers().clone());
    again.refresh().unwrap();
    for (class, scope, state) in [
        ("packages.install", None, State::Notify),
        ("packages.install", Some("work"), State::Ask),
        ("aur.review", None, State::Off),
    ] {
        let e = again.effective(class, scope);
        assert_eq!((e.state, e.layer), (state, Some(Layer::Managed)));
    }

    // Removing one key leaves the others.
    f.store.set_managed("packages.install", None, None).unwrap();
    let mut again = Store::new(f.store.layers().clone());
    again.refresh().unwrap();
    assert_eq!(
        again.effective("packages.install", None).state,
        State::Unset
    );
    assert_eq!(again.effective("aur.review", None).state, State::Off);
    assert_eq!(
        again.effective("packages.install", Some("work")).state,
        State::Ask
    );
    // The in-memory view follows the write.
    assert_eq!(f.state("packages.install", None).0, State::Unset);
}

#[test]
fn set_managed_is_refused_when_the_user_file_sets_the_key() {
    let mut f = Fixture::new();
    f.write(USER, "[class]\n\"aur.review\" = \"ask\"\n");
    let error = f
        .store
        .set_managed("aur.review", None, Some(Grant::Automatic))
        .unwrap_err();
    assert!(matches!(error, StoreError::OverriddenByUser), "{error:?}");
    assert!(!f.dir.path().join(MANAGED).exists(), "nothing was written");
    // Removing is refused as well; a different key is not.
    assert!(matches!(
        f.store.set_managed("aur.review", None, None),
        Err(StoreError::OverriddenByUser)
    ));
    f.store
        .set_managed("aur.review", Some("x"), Some(Grant::Off))
        .unwrap();
}

#[test]
fn set_managed_is_refused_while_a_layer_is_malformed() {
    let mut f = Fixture::new();
    f.write(IMAGE, "[class\n");
    let error = f
        .store
        .set_managed("aur.review", None, Some(Grant::Off))
        .unwrap_err();
    assert!(matches!(error, StoreError::Malformed { .. }), "{error:?}");
    assert!(!f.dir.path().join(MANAGED).exists());
}
