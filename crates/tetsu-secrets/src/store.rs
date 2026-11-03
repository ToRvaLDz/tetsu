//! The vault file: an oo7 v1 keyring, opened only by the broker.
//!
//! One item per secret. Its label is `tetsu-secret:<id>`, its attributes
//! hold the binding (`tetsu.id`, `tetsu.kind`, `tetsu.header`,
//! `tetsu.host`, `tetsu.allow`, `tetsu.created_at`, `tetsu.rotated_at`)
//! and its secret is the value. The file is 0600 in a 0700 directory.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use oo7::Secret as OoSecret;
use oo7::file::{UnlockedItem, UnlockedKeyring};
use tetsu_bus::secrets::SecretKind;

use crate::model::{Host, KindSpec, SecretId, SecretMeta, ToolId};
use crate::vault::Secret;

/// Why the vault refused.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No secret has this id.
    #[error("no secret `{0}`")]
    NotFound(SecretId),
    /// A secret has this id already.
    #[error("a secret `{0}` exists already")]
    Exists(SecretId),
    /// The file cannot be opened with this key, or is not a keyring.
    #[error("{path}: cannot open the vault: {reason}")]
    Open { path: PathBuf, reason: String },
    /// The file or an item in it is damaged.
    #[error("the vault is damaged: {0}")]
    Corrupt(String),
    /// Reading or writing the file failed.
    #[error("vault i/o: {0}")]
    Io(String),
}

/// An open vault.
pub struct Store {
    path: PathBuf,
    keyring: UnlockedKeyring,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

const ATTR_ID: &str = "tetsu.id";
const ATTR_KIND: &str = "tetsu.kind";
const ATTR_HEADER: &str = "tetsu.header";
const ATTR_HOST: &str = "tetsu.host";
const ATTR_ALLOW: &str = "tetsu.allow";
const ATTR_CREATED: &str = "tetsu.created_at";
const ATTR_ROTATED: &str = "tetsu.rotated_at";
const LABEL_PREFIX: &str = "tetsu-secret:";

fn id_attrs(id: &SecretId) -> HashMap<String, String> {
    HashMap::from([(ATTR_ID.to_owned(), id.as_str().to_owned())])
}

fn attrs_of(meta: &SecretMeta) -> HashMap<String, String> {
    let (kind, header) = meta.kind.to_wire();
    let kind = match kind {
        SecretKind::Header => "header",
        SecretKind::Basic => "basic",
        _ => "bearer",
    };
    let allow: Vec<&str> = meta.allowed_tools.iter().map(ToolId::as_str).collect();
    let mut attrs = id_attrs(&meta.id);
    attrs.insert(ATTR_KIND.to_owned(), kind.to_owned());
    if let Some(header) = header {
        attrs.insert(ATTR_HEADER.to_owned(), header);
    }
    attrs.insert(ATTR_HOST.to_owned(), meta.host.as_str().to_owned());
    attrs.insert(ATTR_ALLOW.to_owned(), allow.join(","));
    attrs.insert(ATTR_CREATED.to_owned(), meta.created_at.to_string());
    if let Some(at) = meta.rotated_at {
        attrs.insert(ATTR_ROTATED.to_owned(), at.to_string());
    }
    attrs
}

fn corrupt(label: &str, why: impl fmt::Display) -> StoreError {
    StoreError::Corrupt(format!("item `{label}`: {why}"))
}

fn meta_of(label: &str, attrs: &HashMap<String, String>) -> Result<SecretMeta, StoreError> {
    let get = |name: &str| attrs.get(name).map(String::as_str);
    let need = |name: &str| get(name).ok_or_else(|| corrupt(label, format!("no {name}")));
    let number = |name: &str, text: &str| {
        text.parse::<u64>()
            .map_err(|_| corrupt(label, format!("{name} is not a number")))
    };
    let kind = match need(ATTR_KIND)? {
        "bearer" => SecretKind::Bearer,
        "header" => SecretKind::Header,
        "basic" => SecretKind::Basic,
        _ => return Err(corrupt(label, "unknown kind")),
    };
    let allowed_tools = match need(ATTR_ALLOW)? {
        "" => Vec::new(),
        list => list
            .split(',')
            .map(ToolId::parse)
            .collect::<Result<_, _>>()
            .map_err(|e| corrupt(label, e))?,
    };
    Ok(SecretMeta {
        id: SecretId::parse(need(ATTR_ID)?).map_err(|e| corrupt(label, e))?,
        kind: KindSpec::from_wire(kind, get(ATTR_HEADER)).map_err(|e| corrupt(label, e))?,
        host: Host::parse(need(ATTR_HOST)?).map_err(|e| corrupt(label, e))?,
        allowed_tools,
        created_at: number(ATTR_CREATED, need(ATTR_CREATED)?)?,
        rotated_at: get(ATTR_ROTATED)
            .map(|text| number(ATTR_ROTATED, text))
            .transpose()?,
    })
}

fn io_error(e: impl fmt::Display) -> StoreError {
    StoreError::Io(e.to_string())
}

/// The directory 0700 (created when missing).
fn private_dir(dir: &Path) -> Result<(), StoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(io_error)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(io_error)
}

impl Store {
    /// Opens the vault at `path` with `key`, creating its directory 0700.
    /// A missing file is an empty vault, written on the first change.
    pub async fn open(path: &Path, key: &Secret) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            private_dir(dir)?;
        }
        let keyring = UnlockedKeyring::load(path, OoSecret::text(key.expose()))
            .await
            .map_err(|e| StoreError::Open {
                path: path.to_owned(),
                reason: e.to_string(),
            })?;
        Ok(Self {
            path: path.to_owned(),
            keyring,
        })
    }

    /// Every secret's binding; values stay in the file.
    pub async fn list(&self) -> Result<Vec<SecretMeta>, StoreError> {
        let items = self.keyring.items().await.map_err(io_error)?;
        let mut metas = Vec::with_capacity(items.len());
        for item in items {
            let item = item.map_err(|e| StoreError::Corrupt(e.to_string()))?;
            let item = item.as_unlocked();
            metas.push(meta_of(item.label(), item.attributes())?);
        }
        metas.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(metas)
    }

    async fn find(&self, id: &SecretId) -> Result<Option<UnlockedItem>, StoreError> {
        let found = self
            .keyring
            .lookup_item(&id_attrs(id))
            .await
            .map_err(io_error)?;
        Ok(found.map(|item| item.as_unlocked().clone()))
    }

    /// Whether a secret has this id.
    pub async fn contains(&self, id: &SecretId) -> Result<bool, StoreError> {
        Ok(self.find(id).await?.is_some())
    }

    /// The value of one secret.
    pub async fn value(&self, id: &SecretId) -> Result<Secret, StoreError> {
        let item = self
            .find(id)
            .await?
            .ok_or_else(|| StoreError::NotFound(id.clone()))?;
        let value = item.secret();
        let text = std::str::from_utf8(value.as_bytes())
            .map_err(|_| corrupt(item.label(), "the value is not text"))?;
        Ok(Secret::new(text))
    }

    /// Stores a new secret; the id must be free.
    pub async fn add(&self, meta: &SecretMeta, value: &Secret) -> Result<(), StoreError> {
        if self.find(&meta.id).await?.is_some() {
            return Err(StoreError::Exists(meta.id.clone()));
        }
        let label = format!("{LABEL_PREFIX}{}", meta.id);
        self.keyring
            .create_item(
                &label,
                &attrs_of(meta),
                OoSecret::text(value.expose()),
                false,
            )
            .await
            .map_err(io_error)?;
        self.tighten()
    }

    /// Replaces the value; the binding stays, `rotated_at` becomes `at`.
    pub async fn rotate(
        &self,
        id: &SecretId,
        value: &Secret,
        at: u64,
    ) -> Result<SecretMeta, StoreError> {
        let not_found = || StoreError::NotFound(id.clone());
        let index = self
            .keyring
            .lookup_item_index(&id_attrs(id))
            .await
            .map_err(io_error)?
            .ok_or_else(not_found)?;
        let mut item = self.find(id).await?.ok_or_else(not_found)?;
        let mut meta = meta_of(item.label(), item.attributes())?;
        meta.rotated_at = Some(at);
        item.set_attributes(&attrs_of(&meta));
        item.set_secret(OoSecret::text(value.expose()));
        self.keyring
            .replace_item_index(index, &item)
            .await
            .map_err(io_error)?;
        self.tighten()?;
        Ok(meta)
    }

    /// Deletes a secret and its value.
    pub async fn remove(&self, id: &SecretId) -> Result<(), StoreError> {
        if self.find(id).await?.is_none() {
            return Err(StoreError::NotFound(id.clone()));
        }
        self.keyring.delete(&id_attrs(id)).await.map_err(io_error)?;
        self.tighten()
    }

    /// The vault file is 0600 whatever the library wrote.
    fn tighten(&self) -> Result<(), StoreError> {
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)).map_err(io_error)
    }
}

#[cfg(test)]
mod tests;
