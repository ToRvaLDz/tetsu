//! Transaction records: one JSON file per committed transaction in
//! `/var/lib/tetsu/transactions/<n>.json`. The ESP stays the single source
//! of truth for what boots; a lost record is rebuilt from it.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::StoreError;
use crate::boot::entry::{BootState, canonical};
use crate::boot::esp::EspState;
use crate::tx::PackageChange;

/// Default directory of the records.
pub const RECORDS_DIR: &str = "/var/lib/tetsu/transactions";

const EXT: &str = ".json";
const TMP_EXT: &str = ".json.tmp";

/// What a transaction did (the kinds of the `org.tetsu.tx.v1` contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxKind {
    Update,
    Install,
    InstallAur,
    Remove,
    Rollback,
}

/// The boot verdict recorded at bless time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootVerdict {
    Good,
    Bad,
}

/// One committed transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub counter: u64,
    /// `None` when rebuilt from the ESP.
    pub kind: Option<TxKind>,
    /// Snapshot the transaction started from; `None` for transaction zero
    /// and for rebuilt records.
    pub source: Option<u64>,
    /// `None` until the boot is blessed or marked bad.
    pub boot: Option<BootVerdict>,
    /// Rebuilt from the ESP rather than written by the transaction.
    #[serde(default)]
    pub reconstructed: bool,
    /// The packages the user asked to install or remove.
    #[serde(default)]
    pub packages: Vec<String>,
    /// Every package whose version the transaction changed.
    #[serde(default)]
    pub changes: Vec<PackageChange>,
    /// Unix seconds; `None` for records written before they were kept.
    #[serde(default)]
    pub started_at: Option<u64>,
    #[serde(default)]
    pub finished_at: Option<u64>,
    /// For a rollback, the snapshot that was running when it was asked for.
    #[serde(default)]
    pub rolled_back_from: Option<u64>,
    /// Notes worth keeping: the `/var` report, DKMS owners, tool output.
    #[serde(default)]
    pub log: Vec<String>,
}

/// The records directory.
#[derive(Clone, Debug)]
pub struct Records {
    dir: PathBuf,
}

impl Records {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, counter: u64) -> PathBuf {
        self.dir.join(format!("{counter}{EXT}"))
    }

    /// Every record, keyed by counter. A missing directory is empty;
    /// foreign files and staged `*.json.tmp` are ignored.
    pub fn load(&self) -> Result<BTreeMap<u64, Record>, StoreError> {
        let read = match fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(e) => return Err(StoreError::io(&self.dir, e)),
        };
        let mut all = BTreeMap::new();
        for item in read {
            let item = item.map_err(|e| StoreError::io(&self.dir, e))?;
            let name = item.file_name();
            let Some(counter) = name
                .to_str()
                .filter(|n| !n.ends_with(TMP_EXT))
                .and_then(|n| n.strip_suffix(EXT))
                .and_then(canonical)
            else {
                continue;
            };
            let record = read_record(&item.path())?;
            if record.counter != counter {
                return Err(StoreError::CorruptRecord {
                    path: item.path(),
                    reason: format!("holds counter {}", record.counter),
                });
            }
            all.insert(counter, record);
        }
        Ok(all)
    }

    /// Write `record` atomically: stage, fsync, rename, fsync the directory.
    pub fn write(&self, record: &Record) -> Result<(), StoreError> {
        fs::create_dir_all(&self.dir).map_err(|e| StoreError::io(&self.dir, e))?;
        let json = serde_json::to_vec_pretty(record).expect("a record always serialises");
        let tmp = self.dir.join(format!("{}{TMP_EXT}", record.counter));
        let dest = self.path(record.counter);
        fs::write(&tmp, json).map_err(|e| StoreError::io(&tmp, e))?;
        sync_file(&tmp)?;
        fs::rename(&tmp, &dest).map_err(|e| StoreError::io(&tmp, e))?;
        sync_file(&self.dir)
    }

    /// Record the bless-time verdict of `counter`.
    pub fn set_boot(&self, counter: u64, verdict: BootVerdict) -> Result<(), StoreError> {
        let path = self.path(counter);
        if !path.is_file() {
            return Err(StoreError::NotFound(counter));
        }
        let mut record = read_record(&path)?;
        record.boot = Some(verdict);
        self.write(&record)
    }

    /// [`Records::load`], first writing a record for every committed ESP
    /// entry that has none: a lost `/var/lib/tetsu`, or a cut between the
    /// commit and the record. The verdict comes from the entry's boot
    /// counting: blessed is good, exhausted is bad, anything else unknown.
    pub fn load_or_reconstruct(&self, esp: &EspState) -> Result<BTreeMap<u64, Record>, StoreError> {
        let mut all = self.load()?;
        for record in missing_records(&all, esp) {
            self.write(&record)?;
            all.insert(record.counter, record);
        }
        Ok(all)
    }

    /// [`Records::load`] with the records of ESP entries that have none
    /// derived in memory, as [`Records::load_or_reconstruct`] would write
    /// them. Writes nothing, so it is safe without the transaction lock.
    pub fn load_derived(&self, esp: &EspState) -> Result<BTreeMap<u64, Record>, StoreError> {
        let mut all = self.load()?;
        for record in missing_records(&all, esp) {
            all.insert(record.counter, record);
        }
        Ok(all)
    }
}

/// The records of ESP entries not in `have`, reconstructed from the entry's
/// boot counting.
fn missing_records(have: &BTreeMap<u64, Record>, esp: &EspState) -> Vec<Record> {
    esp.entries
        .iter()
        .filter(|entry| !have.contains_key(&entry.name.counter))
        .map(|entry| Record {
            counter: entry.name.counter,
            kind: None,
            source: None,
            boot: match entry.state() {
                BootState::Good => Some(BootVerdict::Good),
                BootState::Bad => Some(BootVerdict::Bad),
                BootState::Untried | BootState::Trying => None,
            },
            reconstructed: true,
            ..Record::default()
        })
        .collect()
}

fn read_record(path: &Path) -> Result<Record, StoreError> {
    let bytes = fs::read(path).map_err(|e| StoreError::io(path, e))?;
    serde_json::from_slice(&bytes).map_err(|e| StoreError::CorruptRecord {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })
}

/// fsync a file or directory, as a store error.
fn sync_file(path: &Path) -> Result<(), StoreError> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| StoreError::io(path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::EspEntry;

    fn records() -> (tempfile::TempDir, Records) {
        let dir = tempfile::tempdir().unwrap();
        let r = Records::new(&dir.path().join("var/lib/tetsu/transactions"));
        (dir, r)
    }

    fn rec(counter: u64) -> Record {
        Record {
            counter,
            kind: Some(TxKind::Update),
            source: Some(counter - 1),
            boot: None,
            ..Record::default()
        }
    }

    fn esp(names: &[&str]) -> EspState {
        let mut st = EspState::default();
        for n in names {
            let Some(FileKind::Entry(name)) = parse_file_name(n) else {
                panic!("{n}")
            };
            st.entries.push(EspEntry {
                name,
                path: PathBuf::from(n),
                size: 1,
            });
        }
        st
    }

    #[test]
    fn a_record_written_before_the_transaction_details_still_loads() {
        let (_d, r) = records();
        fs::create_dir_all(r.dir()).unwrap();
        let old = r#"{"counter":3,"kind":"update","source":2,"boot":"good"}"#;
        fs::write(r.path(3), old).unwrap();
        let got = &r.load().unwrap()[&3];
        assert_eq!(got.kind, Some(TxKind::Update));
        assert_eq!(got.boot, Some(BootVerdict::Good));
        assert!(!got.reconstructed);
        assert!(got.packages.is_empty() && got.changes.is_empty() && got.log.is_empty());
        assert_eq!(
            (got.started_at, got.finished_at, got.rolled_back_from),
            (None, None, None)
        );
    }

    #[test]
    fn transaction_details_round_trip() {
        let (_d, r) = records();
        let full = Record {
            packages: vec!["vim".into()],
            changes: vec![PackageChange {
                name: "vim".into(),
                from: None,
                to: Some("9.1-1".into()),
            }],
            started_at: Some(1_700_000_000),
            finished_at: Some(1_700_000_042),
            rolled_back_from: Some(5),
            log: vec!["/var/lib/foo".into()],
            ..rec(6)
        };
        r.write(&full).unwrap();
        assert_eq!(r.load().unwrap()[&6], full);
    }

    #[test]
    fn missing_dir_is_empty() {
        let (_d, r) = records();
        assert!(r.load().unwrap().is_empty());
    }

    #[test]
    fn write_then_load_round_trips() {
        let (_d, r) = records();
        r.write(&rec(3)).unwrap();
        r.write(&rec(5)).unwrap();
        let all = r.load().unwrap();
        assert_eq!(all.keys().copied().collect::<Vec<_>>(), vec![3, 5]);
        assert_eq!(all[&5], rec(5));
        assert!(r.path(5).is_file());
        assert!(!r.dir().join("5.json.tmp").exists());
    }

    #[test]
    fn json_uses_contract_names() {
        let (_d, r) = records();
        let mut x = rec(2);
        x.kind = Some(TxKind::InstallAur);
        x.boot = Some(BootVerdict::Good);
        r.write(&x).unwrap();
        let text = fs::read_to_string(r.path(2)).unwrap();
        assert!(text.contains("\"install_aur\""), "{text}");
        assert!(text.contains("\"good\""), "{text}");
    }

    #[test]
    fn foreign_and_staged_files_are_ignored() {
        let (_d, r) = records();
        r.write(&rec(1)).unwrap();
        for name in ["2.json.tmp", "notes.txt", "x.json", "03.json"] {
            fs::write(r.dir().join(name), b"{").unwrap();
        }
        assert_eq!(r.load().unwrap().len(), 1);
    }

    #[test]
    fn corrupt_record_is_reported() {
        let (_d, r) = records();
        fs::create_dir_all(r.dir()).unwrap();
        fs::write(r.path(4), b"{not json").unwrap();
        assert!(matches!(r.load(), Err(StoreError::CorruptRecord { .. })));
    }

    #[test]
    fn counter_must_match_the_file_name() {
        let (_d, r) = records();
        r.write(&rec(4)).unwrap();
        fs::rename(r.path(4), r.path(6)).unwrap();
        assert!(matches!(r.load(), Err(StoreError::CorruptRecord { .. })));
    }

    #[test]
    fn set_boot_records_the_verdict() {
        let (_d, r) = records();
        r.write(&rec(2)).unwrap();
        r.set_boot(2, BootVerdict::Bad).unwrap();
        assert_eq!(r.load().unwrap()[&2].boot, Some(BootVerdict::Bad));
        assert!(matches!(
            r.set_boot(9, BootVerdict::Good),
            Err(StoreError::NotFound(9))
        ));
    }

    #[test]
    fn reconstruct_from_the_esp_when_records_are_lost() {
        let (_d, r) = records();
        let st = esp(&[
            "tetsu-2.efi",
            "tetsu-3+0-3.efi",
            "tetsu-4+2-1.efi",
            "tetsu-5+3.efi",
        ]);
        let all = r.load_or_reconstruct(&st).unwrap();
        let verdicts: Vec<_> = all.values().map(|x| (x.counter, x.boot)).collect();
        assert_eq!(
            verdicts,
            [
                (2, Some(BootVerdict::Good)),
                (3, Some(BootVerdict::Bad)),
                (4, None),
                (5, None),
            ]
        );
        assert!(all.values().all(|x| x.reconstructed && x.kind.is_none()));
        // Persisted: a plain load sees them.
        assert_eq!(r.load().unwrap(), all);
    }

    #[test]
    fn load_derived_reconstructs_in_memory_and_writes_nothing() {
        let (_d, r) = records();
        let st = esp(&["tetsu-2.efi", "tetsu-3+0-3.efi", "tetsu-5+3.efi"]);
        let all = r.load_derived(&st).unwrap();
        let verdicts: Vec<_> = all.values().map(|x| (x.counter, x.boot)).collect();
        assert_eq!(
            verdicts,
            [
                (2, Some(BootVerdict::Good)),
                (3, Some(BootVerdict::Bad)),
                (5, None)
            ]
        );
        assert!(all.values().all(|x| x.reconstructed));
        assert!(r.load().unwrap().is_empty());
        assert!(!r.dir().exists());
    }

    #[test]
    fn reconstruct_only_fills_missing_records() {
        let (_d, r) = records();
        r.write(&rec(4)).unwrap();
        let all = r
            .load_or_reconstruct(&esp(&["tetsu-4.efi", "tetsu-5+3.efi"]))
            .unwrap();
        // The written record stays as it is, even where the ESP says Good.
        assert_eq!(all[&4], rec(4));
        assert!(all[&5].reconstructed);
    }
}
