use std::fs;
use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::model::{Host, KindSpec, ToolId};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VALUE: &str = "S3NT1NEL-SENTINEL";

fn key() -> Secret {
    Secret::new(KEY)
}

fn meta(id: &str) -> SecretMeta {
    SecretMeta {
        id: SecretId::parse(id).unwrap(),
        kind: KindSpec::parse("header:x-api-key").unwrap(),
        host: Host::parse("api.example.com").unwrap(),
        allowed_tools: vec![
            ToolId::parse("web-fetch").unwrap(),
            ToolId::parse("git").unwrap(),
        ],
        created_at: 1_700_000_000,
        rotated_at: None,
    }
}

fn vault_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("secrets").join("vault.keyring")
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn a_new_vault_is_empty_and_private() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    assert!(store.list().await.unwrap().is_empty());
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[tokio::test]
async fn a_secret_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    drop(store);
    let again = Store::open(&path, &key()).await.unwrap();
    assert_eq!(again.list().await.unwrap(), [meta("demo")]);
    let id = SecretId::parse("demo").unwrap();
    assert_eq!(again.value(&id).await.unwrap().expose(), VALUE);
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(path.parent().unwrap()), 0o700);
}

#[tokio::test]
async fn the_value_is_not_in_the_file_in_the_clear() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    let bytes = fs::read(&path).unwrap();
    let needle = VALUE.as_bytes();
    assert!(!bytes.windows(needle.len()).any(|w| w == needle));
}

#[tokio::test]
async fn a_wrong_key_does_not_open_a_vault_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    drop(store);
    let wrong = Secret::new("f".repeat(64));
    let e = Store::open(&path, &wrong).await.unwrap_err();
    assert!(matches!(e, StoreError::Open { .. }), "{e}");
    assert!(!e.to_string().contains(VALUE));
}

#[tokio::test]
async fn a_duplicate_add_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&vault_path(&dir), &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    let e = store
        .add(&meta("demo"), &Secret::new("other"))
        .await
        .unwrap_err();
    assert!(matches!(e, StoreError::Exists(_)), "{e}");
    let id = SecretId::parse("demo").unwrap();
    assert_eq!(store.value(&id).await.unwrap().expose(), VALUE);
    assert_eq!(store.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn rotate_replaces_the_value_and_keeps_the_binding() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    let id = SecretId::parse("demo").unwrap();
    let rotated = store
        .rotate(&id, &Secret::new("second-value"), 1_700_000_500)
        .await
        .unwrap();
    assert_eq!(rotated.rotated_at, Some(1_700_000_500));
    assert_eq!(rotated.host, meta("demo").host);
    assert_eq!(rotated.allowed_tools, meta("demo").allowed_tools);
    assert_eq!(store.value(&id).await.unwrap().expose(), "second-value");
    assert_eq!(store.list().await.unwrap(), std::slice::from_ref(&rotated));
    drop(store);
    let again = Store::open(&path, &key()).await.unwrap();
    assert_eq!(again.list().await.unwrap(), [rotated]);
    assert_eq!(again.value(&id).await.unwrap().expose(), "second-value");
}

#[tokio::test]
async fn an_unknown_id_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&vault_path(&dir), &key()).await.unwrap();
    let id = SecretId::parse("ghost").unwrap();
    assert!(matches!(
        store.value(&id).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.rotate(&id, &Secret::new("x"), 1).await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        store.remove(&id).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn remove_deletes_one_secret_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("one"), &Secret::new("v1")).await.unwrap();
    store.add(&meta("two"), &Secret::new("v2")).await.unwrap();
    store
        .remove(&SecretId::parse("one").unwrap())
        .await
        .unwrap();
    assert_eq!(store.list().await.unwrap(), [meta("two")]);
    assert_eq!(mode(&path), 0o600);
    drop(store);
    let again = Store::open(&path, &key()).await.unwrap();
    assert_eq!(again.list().await.unwrap(), [meta("two")]);
}

#[tokio::test]
async fn a_loose_file_mode_is_tightened_at_the_next_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = vault_path(&dir);
    let store = Store::open(&path, &key()).await.unwrap();
    store.add(&meta("one"), &Secret::new("v1")).await.unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    store.add(&meta("two"), &Secret::new("v2")).await.unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[tokio::test]
async fn debug_never_shows_a_value() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&vault_path(&dir), &key()).await.unwrap();
    store.add(&meta("demo"), &Secret::new(VALUE)).await.unwrap();
    let text = format!("{store:?} {:?}", store.list().await.unwrap());
    assert!(!text.contains(VALUE));
}
