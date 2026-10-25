//! Pure selection logic of decision #20: the transaction source, orphans of
//! an interrupted transaction, the next counter, the last good snapshot and
//! snapshot retention. It plans; it never touches the disk.

use std::collections::BTreeMap;

use super::record::{BootVerdict, Record};
use crate::boot::entry::BootState;
use crate::boot::esp::EspState;

/// Root snapshots kept by retention, besides the protected ones.
pub const KEEP_SNAPSHOTS: usize = 10;

/// Where a transaction starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub counter: u64,
    /// The source is a pending default: the new snapshot inherits its
    /// `etc-base`, so the first-boot sync still sees every change since the
    /// first pending transaction started.
    pub inherit_etc_base: bool,
}

/// The pending default (committed, untried) if there is one, else the
/// booted snapshot.
pub fn select_source(esp: &EspState, booted: u64) -> Source {
    match esp.default_entry() {
        Some(e) if e.name.counter != booted && is_untried(e.state()) => Source {
            counter: e.name.counter,
            inherit_etc_base: true,
        },
        _ => Source {
            counter: booted,
            inherit_etc_base: false,
        },
    }
}

/// Snapshots left by an interrupted transaction: no record and newer than
/// every committed ESP entry, so never committed. An older snapshot whose
/// UKI was collected keeps its record and is not an orphan; the booted one
/// never is.
pub fn orphans(
    snapshots: &[u64],
    records: &BTreeMap<u64, Record>,
    esp: &EspState,
    booted: Option<u64>,
) -> Vec<u64> {
    let newest = esp.newest_counter();
    snapshots
        .iter()
        .copied()
        .filter(|&n| Some(n) != booted && !records.contains_key(&n))
        .filter(|&n| newest.is_none_or(|m| n > m))
        .collect()
}

/// One above every counter in use: snapshots, records, ESP entries and
/// staged files.
pub fn next_counter(snapshots: &[u64], records: &BTreeMap<u64, Record>, esp: &EspState) -> u64 {
    let entries = esp.entries.iter().map(|e| e.name.counter);
    let tmps = esp.orphan_tmps.iter().map(|t| t.counter);
    snapshots
        .iter()
        .copied()
        .chain(records.keys().copied())
        .chain(entries)
        .chain(tmps)
        .max()
        .map_or(1, |m| m + 1)
}

/// The implicit rollback target: the newest existing snapshot older than
/// the booted one whose boot was blessed good.
pub fn last_good(snapshots: &[u64], records: &BTreeMap<u64, Record>, booted: u64) -> Option<u64> {
    snapshots
        .iter()
        .rev()
        .copied()
        .find(|&n| n < booted && is_good(records, n))
}

/// The newest existing snapshot whose boot was blessed good.
pub fn newest_good(snapshots: &[u64], records: &BTreeMap<u64, Record>) -> Option<u64> {
    snapshots
        .iter()
        .rev()
        .copied()
        .find(|&n| is_good(records, n))
}

/// Snapshots retention deletes: everything outside the newest `keep`,
/// except the `protected` ones (the booted, the default, the newest good).
pub fn plan_retention(snapshots: &[u64], protected: &[u64], keep: usize) -> Vec<u64> {
    let mut sorted = snapshots.to_vec();
    sorted.sort_unstable();
    let cut = sorted.len().saturating_sub(keep);
    sorted[..cut]
        .iter()
        .copied()
        .filter(|n| !protected.contains(n))
        .collect()
}

/// Counter of the entry systemd-boot picks, if any.
pub fn default_counter(esp: &EspState) -> Option<u64> {
    esp.default_entry().map(|e| e.name.counter)
}

fn is_untried(state: BootState) -> bool {
    state == BootState::Untried
}

fn is_good(records: &BTreeMap<u64, Record>, counter: u64) -> bool {
    records
        .get(&counter)
        .is_some_and(|r| r.boot == Some(BootVerdict::Good))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::{EspEntry, OrphanTmp};
    use std::path::PathBuf;

    fn esp(names: &[&str], tmps: &[u64]) -> EspState {
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
        for &counter in tmps {
            st.orphan_tmps.push(OrphanTmp {
                counter,
                path: PathBuf::new(),
                size: 1,
            });
        }
        st
    }

    fn records(list: &[(u64, Option<BootVerdict>)]) -> BTreeMap<u64, Record> {
        list.iter()
            .map(|&(counter, boot)| {
                (
                    counter,
                    Record {
                        counter,
                        kind: None,
                        source: None,
                        boot,
                        reconstructed: false,
                    },
                )
            })
            .collect()
    }

    const G: Option<BootVerdict> = Some(BootVerdict::Good);
    const B: Option<BootVerdict> = Some(BootVerdict::Bad);

    #[test]
    fn source_is_the_booted_one_without_a_pending_default() {
        let st = esp(&["tetsu-4.efi", "tetsu-5.efi"], &[]);
        let s = select_source(&st, 5);
        assert_eq!(
            s,
            Source {
                counter: 5,
                inherit_etc_base: false
            }
        );
    }

    #[test]
    fn source_is_the_pending_untried_default() {
        let st = esp(&["tetsu-5.efi", "tetsu-6+3.efi"], &[]);
        let s = select_source(&st, 5);
        assert_eq!(
            s,
            Source {
                counter: 6,
                inherit_etc_base: true
            }
        );
    }

    #[test]
    fn a_bad_or_trying_newest_entry_is_not_pending() {
        let st = esp(&["tetsu-5.efi", "tetsu-6+0-3.efi"], &[]);
        assert_eq!(select_source(&st, 5).counter, 5);
        let st = esp(&["tetsu-5.efi", "tetsu-6+2-1.efi"], &[]);
        assert_eq!(select_source(&st, 5).counter, 5);
    }

    #[test]
    fn booted_untried_entry_is_not_pending() {
        // Booted for the first time, not blessed yet: it is the source.
        let st = esp(&["tetsu-5.efi", "tetsu-6+3.efi"], &[]);
        let s = select_source(&st, 6);
        assert_eq!(
            s,
            Source {
                counter: 6,
                inherit_etc_base: false
            }
        );
    }

    #[test]
    fn orphans_are_uncommitted_and_unrecorded() {
        let st = esp(&["tetsu-3.efi", "tetsu-5+3.efi"], &[]);
        // 1 lost its UKI but has a record; 6 and 7 never committed.
        let recs = records(&[(1, G), (3, G), (5, None)]);
        assert_eq!(orphans(&[1, 3, 5, 6, 7], &recs, &st, Some(3)), vec![6, 7]);
    }

    #[test]
    fn a_collected_snapshot_without_record_is_not_an_orphan() {
        // Records lost and the UKI collected: older than the newest entry,
        // so it was committed once.
        let st = esp(&["tetsu-5.efi"], &[]);
        assert_eq!(
            orphans(&[2, 5], &records(&[]), &st, Some(5)),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn without_esp_entries_every_unrecorded_snapshot_but_booted_is_orphan() {
        let st = esp(&[], &[]);
        assert_eq!(orphans(&[1, 2], &records(&[]), &st, Some(1)), vec![2]);
        assert_eq!(orphans(&[1, 2], &records(&[]), &st, None), vec![1, 2]);
    }

    #[test]
    fn next_counter_is_above_everything_in_use() {
        let st = esp(&["tetsu-4.efi"], &[9]);
        assert_eq!(next_counter(&[1, 4], &records(&[(4, G)]), &st), 10);
        let st = esp(&["tetsu-4.efi"], &[]);
        assert_eq!(next_counter(&[1, 7], &records(&[(8, G)]), &st), 9);
        assert_eq!(next_counter(&[], &records(&[]), &esp(&[], &[])), 1);
    }

    #[test]
    fn last_good_is_the_newest_blessed_older_than_booted() {
        let recs = records(&[(1, G), (2, G), (3, B), (4, None), (5, G), (6, G)]);
        assert_eq!(last_good(&[1, 2, 3, 4, 5, 6], &recs, 5), Some(2));
        // Deleted snapshots are not targets.
        assert_eq!(last_good(&[1, 3, 4, 5, 6], &recs, 5), Some(1));
        assert_eq!(last_good(&[3, 4, 5], &recs, 5), None);
    }

    #[test]
    fn newest_good_existing_snapshot() {
        let recs = records(&[(1, G), (2, B), (3, G), (4, None)]);
        assert_eq!(newest_good(&[1, 2, 3, 4], &recs), Some(3));
        assert_eq!(newest_good(&[1, 2, 4], &recs), Some(1));
        assert_eq!(newest_good(&[2, 4], &recs), None);
    }

    #[test]
    fn retention_keeps_the_newest_ten() {
        let snaps: Vec<u64> = (1..=13).collect();
        assert_eq!(plan_retention(&snaps, &[], KEEP_SNAPSHOTS), vec![1, 2, 3]);
        assert!(plan_retention(&snaps[..10], &[], KEEP_SNAPSHOTS).is_empty());
    }

    #[test]
    fn retention_never_deletes_protected_snapshots() {
        let snaps: Vec<u64> = (1..=14).collect();
        assert_eq!(plan_retention(&snaps, &[2, 3], KEEP_SNAPSHOTS), vec![1, 4]);
    }

    #[test]
    fn default_counter_skips_bad_entries() {
        let st = esp(&["tetsu-4.efi", "tetsu-5+0-3.efi"], &[]);
        assert_eq!(default_counter(&st), Some(4));
        assert_eq!(default_counter(&esp(&[], &[])), None);
    }
}
