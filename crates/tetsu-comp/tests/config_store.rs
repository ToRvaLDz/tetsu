//! The store: the three files read, watched by polling, and the managed
//! file written. Every path is under a temporary directory.

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tetsu_bus::config::{ConfigEvent, ConfigKind, Origin};
use tetsu_comp::config::paths::Paths;
use tetsu_comp::config::store::Store;

struct Dirs {
    root: tempfile::TempDir,
    paths: Paths,
}

fn dirs() -> Dirs {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths {
        defaults: root.path().join("defaults.toml"),
        managed: Some(root.path().join("state/tetsu/comp.toml")),
        user: Some(root.path().join("config/tetsu/comp.toml")),
    };
    Dirs { root, paths }
}

/// Writes a file, creating its directory, a moment after the last write so
/// the modification time moves.
fn put(path: &Path, text: &str) {
    std::thread::sleep(Duration::from_millis(20));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn user(d: &Dirs) -> PathBuf {
    d.paths.user.clone().unwrap()
}

fn managed(d: &Dirs) -> PathBuf {
    d.paths.managed.clone().unwrap()
}

fn kinds(events: &[ConfigEvent]) -> Vec<ConfigKind> {
    events.iter().map(|e| e.kind).collect()
}

fn outer(store: &Store) -> (Origin, serde_json::Value) {
    let entry = store.view().entry("layout.gaps_outer").unwrap();
    (entry.origin, entry.value.clone())
}

#[test]
fn a_poll_sees_create_edit_rename_and_delete_of_the_user_file() {
    let d = dirs();
    let mut store = Store::new(d.paths.clone());
    assert!(store.poll().is_empty(), "nothing changed yet");

    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    let events = store.poll();
    assert_eq!(
        kinds(&events),
        [ConfigKind::Changed, ConfigKind::Diagnostics]
    );
    assert_eq!(outer(&store), (Origin::User, json!(5)));

    put(&user(&d), "[layout]\ngaps_outer = 7\n");
    store.poll();
    assert_eq!(outer(&store), (Origin::User, json!(7)));

    // An editor that saves by rename.
    let tmp = user(&d).with_extension("swp");
    put(&tmp, "[layout]\ngaps_outer = 9\n");
    fs::rename(&tmp, user(&d)).unwrap();
    store.poll();
    assert_eq!(outer(&store), (Origin::User, json!(9)));

    fs::remove_file(user(&d)).unwrap();
    // A deletion is adopted on the second poll (see the debounce tests).
    assert!(store.poll().is_empty());
    let events = store.poll();
    assert_eq!(
        kinds(&events),
        [ConfigKind::Changed, ConfigKind::Diagnostics]
    );
    assert_eq!(outer(&store), (Origin::Default, json!(0)));

    assert!(store.poll().is_empty());
}

#[test]
fn a_syntax_error_keeps_the_last_good_user_layer() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    let mut store = Store::new(d.paths.clone());
    assert_eq!(outer(&store), (Origin::User, json!(5)));

    put(&user(&d), "[layout]\ngaps_outer =\n");
    let events = store.poll();
    assert_eq!(kinds(&events), [ConfigKind::Diagnostics]);
    assert_eq!(outer(&store), (Origin::User, json!(5)));
    let found = store.diagnostics();
    assert_eq!(found.len(), 1);
    assert_eq!((found[0].line, found[0].key.as_deref()), (Some(2), None));

    put(&user(&d), "[layout]\ngaps_outer = 6\n");
    store.poll();
    assert_eq!(outer(&store), (Origin::User, json!(6)));
    assert!(store.diagnostics().is_empty());
}

#[test]
fn a_bad_value_costs_only_its_key_and_is_published_with_its_line() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_inner = 3\nmaster_ratio = 5.0\n");
    let store = Store::new(d.paths.clone());
    assert_eq!(
        store.view().entry("layout.gaps_inner").unwrap().origin,
        Origin::User
    );
    let found = store.diagnostics();
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].line, found[0].key.as_deref()),
        (Some(3), Some("layout.master_ratio"))
    );
}

#[test]
fn only_the_keys_that_changed_get_a_changed_event() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    let mut store = Store::new(d.paths.clone());
    // A comment: the file changed, no key did.
    put(&user(&d), "# hello\n[layout]\ngaps_outer = 5\n");
    let events = store.poll();
    assert_eq!(kinds(&events), [ConfigKind::Diagnostics]);
    assert_eq!(events[0].diagnostics, Some(vec![]));
}

#[test]
fn an_edit_of_the_managed_file_is_followed_without_a_diagnostics_event() {
    let d = dirs();
    let mut store = Store::new(d.paths.clone());
    put(&managed(&d), "[layout]\ngaps_outer = 4\n");
    let events = store.poll();
    assert_eq!(kinds(&events), [ConfigKind::Changed]);
    assert_eq!(outer(&store), (Origin::Managed, json!(4)));
}

#[test]
fn the_defaults_file_is_a_default_origin_and_a_managed_command_key_is_ignored() {
    let d = dirs();
    put(&d.paths.defaults, "[layout]\ngaps_outer = 2\n");
    put(&managed(&d), "[lock]\ncommand = [\"evil\"]\n");
    let store = Store::new(d.paths.clone());
    assert_eq!(outer(&store), (Origin::Default, json!(2)));
    let lock = store.view().entry("lock.command").unwrap();
    assert_eq!(lock.origin, Origin::Default);
    assert_eq!(lock.managed_value, None);
}

#[test]
fn write_managed_makes_a_private_file_in_a_private_directory_and_round_trips() {
    let d = dirs();
    let mut store = Store::new(d.paths.clone());
    let values: BTreeMap<String, toml::Value> = [
        ("layout.gaps_outer".to_owned(), toml::Value::Integer(10)),
        ("layout.master_ratio".to_owned(), toml::Value::Float(0.7)),
        (
            "window_rule".to_owned(),
            toml::Value::Array(vec![toml::Value::Table(
                [("app_id".to_owned(), toml::Value::String("foot".into()))]
                    .into_iter()
                    .collect(),
            )]),
        ),
    ]
    .into();
    let events = store.write_managed(&values).unwrap();
    assert!(events.iter().all(|e| e.kind == ConfigKind::Changed));
    assert_eq!(events.len(), 3);

    let file = managed(&d);
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(file.parent().unwrap()), 0o700);
    let leftovers: Vec<_> = fs::read_dir(file.parent().unwrap()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file stays");

    let parsed = tetsu_comp::config::layer::parse(&fs::read_to_string(&file).unwrap());
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(parsed.values, values);
    assert_eq!(outer(&store), (Origin::Managed, json!(10)));
    // The write is already seen: the next poll has nothing to say.
    assert!(store.poll().is_empty());
}

#[test]
fn a_malformed_managed_file_is_not_overwritten() {
    let d = dirs();
    put(&managed(&d), "[layout\nbroken");
    let mut store = Store::new(d.paths.clone());
    let values = [("layout.gaps_outer".to_owned(), toml::Value::Integer(1))].into();
    let err = store.write_managed(&values).unwrap_err();
    assert!(!err.to_string().is_empty());
    assert_eq!(fs::read_to_string(managed(&d)).unwrap(), "[layout\nbroken");
}

#[test]
fn an_unwritable_state_directory_fails_and_leaves_no_file() {
    let d = dirs();
    // The state directory's parent is a regular file.
    let blocker = d.root.path().join("state");
    fs::write(&blocker, "not a directory").unwrap();
    let mut store = Store::new(d.paths.clone());
    let values = [("layout.gaps_outer".to_owned(), toml::Value::Integer(1))].into();
    assert!(store.write_managed(&values).is_err());
    assert_eq!(fs::read_to_string(&blocker).unwrap(), "not a directory");
    assert_eq!(outer(&store), (Origin::Default, json!(0)));
}

#[test]
fn with_no_home_there_is_nowhere_to_write() {
    let mut store = Store::new(Paths {
        defaults: PathBuf::from("/nonexistent/defaults.toml"),
        managed: None,
        user: None,
    });
    assert!(store.poll().is_empty());
    assert!(store.write_managed(&BTreeMap::new()).is_err());
}

#[test]
fn a_file_emptied_in_place_is_not_adopted_until_it_stays_so() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    let mut store = Store::new(d.paths.clone());

    // A save that truncates, then writes: the poll lands in between.
    put(&user(&d), "");
    assert!(store.poll().is_empty(), "the empty file waits one poll");
    assert_eq!(outer(&store), (Origin::User, json!(5)));

    put(&user(&d), "[layout]\ngaps_outer = 6\n");
    store.poll();
    assert_eq!(outer(&store), (Origin::User, json!(6)));

    // A file that stays empty is adopted on the second poll.
    put(&user(&d), "");
    assert!(store.poll().is_empty());
    let events = store.poll();
    assert_eq!(
        kinds(&events),
        [ConfigKind::Changed, ConfigKind::Diagnostics]
    );
    assert_eq!(outer(&store), (Origin::Default, json!(0)));
}

#[test]
fn a_deleted_file_is_dropped_on_the_second_poll() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    let mut store = Store::new(d.paths.clone());
    fs::remove_file(user(&d)).unwrap();
    assert!(store.poll().is_empty());
    assert_eq!(outer(&store), (Origin::User, json!(5)));
    assert!(!store.poll().is_empty());
    assert_eq!(outer(&store), (Origin::Default, json!(0)));
}

#[test]
fn an_unreadable_file_is_read_again_once_it_can_be() {
    let d = dirs();
    put(&user(&d), "[layout]\ngaps_outer = 5\n");
    fs::set_permissions(user(&d), fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_to_string(user(&d)).is_ok() {
        return; // root reads anything
    }
    let mut store = Store::new(d.paths.clone());
    assert_eq!(store.diagnostics().len(), 1, "the file is not readable");

    // Only the ctime moves: the fingerprint does not.
    fs::set_permissions(user(&d), fs::Permissions::from_mode(0o644)).unwrap();
    store.poll();
    assert_eq!(outer(&store), (Origin::User, json!(5)));
    assert!(store.diagnostics().is_empty());
}

const USER_OUTPUT: &str = "[output.DP-1]\nscale = 1.5\nenabled = true\n";

fn mirror_of(store: &Store) -> tetsu_comp::bus::ConfigMirror {
    tetsu_comp::bus::ConfigMirror::new(tetsu_comp::config::glue::config_reply(Some(store)))
}

fn mirror_keys(mirror: &tetsu_comp::bus::ConfigMirror) -> Vec<String> {
    let mut keys: Vec<_> = mirror.reply().entries.into_iter().map(|e| e.key).collect();
    keys.sort();
    keys
}

#[test]
fn deleting_an_output_table_removes_its_keys_and_the_mirror_drops_them() {
    let d = dirs();
    put(&user(&d), USER_OUTPUT);
    let mut store = Store::new(d.paths.clone());
    let mut mirror = mirror_of(&store);
    assert!(mirror_keys(&mirror).contains(&"output.DP-1.scale".to_owned()));
    put(&user(&d), "");
    assert!(store.poll().is_empty(), "an emptied file waits one poll");
    let events = store.poll();
    let removed: Vec<_> = events
        .iter()
        .filter(|e| e.kind == ConfigKind::Removed)
        .filter_map(|e| e.key.as_deref())
        .collect();
    assert_eq!(removed, ["output.DP-1.enabled", "output.DP-1.scale"]);
    for event in &events {
        common::check_config_event(&common::contract(), event);
        mirror.apply(event);
    }
    assert!(
        !mirror_keys(&mirror)
            .iter()
            .any(|k| k.starts_with("output."))
    );
}

#[test]
fn a_key_the_managed_layer_still_sets_is_changed_not_removed() {
    let d = dirs();
    put(&managed(&d), "[output.DP-1]\nscale = 2.0\n");
    put(&user(&d), USER_OUTPUT);
    let mut store = Store::new(d.paths.clone());
    put(&user(&d), "# nothing\n");
    let events = store.poll();
    let scale = events
        .iter()
        .find(|e| {
            e.entry
                .as_ref()
                .is_some_and(|x| x.key == "output.DP-1.scale")
        })
        .expect("scale is reported");
    assert_eq!(scale.kind, ConfigKind::Changed);
    assert_eq!(scale.entry.as_ref().unwrap().origin, Origin::Managed);
    let gone: Vec<_> = events
        .iter()
        .filter(|e| e.kind == ConfigKind::Removed)
        .filter_map(|e| e.key.as_deref())
        .collect();
    assert_eq!(gone, ["output.DP-1.enabled"]);
}

#[test]
fn write_managed_nests_the_connector_table_and_parses_back() {
    let d = dirs();
    let mut store = Store::new(d.paths.clone());
    let at = toml::Value::Table(
        [
            ("x".to_owned(), toml::Value::Integer(-1920)),
            ("y".to_owned(), toml::Value::Integer(0)),
        ]
        .into_iter()
        .collect(),
    );
    let values: BTreeMap<String, toml::Value> = [
        ("output.DP-1.position".to_owned(), at),
        ("output.DP-1.scale".to_owned(), toml::Value::Float(1.25)),
        ("layout.gaps_outer".to_owned(), toml::Value::Integer(4)),
    ]
    .into();
    let events = store.write_managed(&values).unwrap();
    for event in &events {
        common::check_config_event(&common::contract(), event);
    }
    let text = fs::read_to_string(managed(&d)).unwrap();
    assert!(text.contains("[output.DP-1]"), "{text}");
    assert!(!text.contains("\"output.DP-1"), "{text}");
    let parsed = tetsu_comp::config::layer::parse(&text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    assert_eq!(parsed.values, values);
    assert!(store.poll().is_empty());
}
