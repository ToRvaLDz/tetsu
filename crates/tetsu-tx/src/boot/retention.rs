//! Retention and orphan GC (decision #20). Pure: it plans, it never deletes.

use std::path::PathBuf;

use super::entry::BootState;
use super::esp::EspState;

/// Entries kept besides the incoming one: the booted entry and the two
/// newest Good entries older than it.
pub const KEEP_PREVIOUS_GOOD: usize = 2;

/// What GC would delete.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcPlan {
    pub remove: Vec<PathBuf>,
    pub freed_bytes: u64,
}

/// Plan the removal that makes room for one incoming entry.
///
/// Every Bad entry and every orphan `*.efi.tmp` goes. Of the rest only the
/// booted entry and the two newest Good entries older than it stay, so the
/// incoming entry makes four; anything else (an untried or trying entry
/// newer than the booted one included) is superseded by the incoming entry.
/// The booted entry is never removed, even when it looks Bad. Without a
/// booted counter the three newest non-Bad entries stay.
pub fn plan_gc(state: &EspState, booted: Option<u64>) -> GcPlan {
    let keep: Vec<u64> = match booted {
        Some(b) => {
            let previous = state
                .entries
                .iter()
                .rev()
                .filter(|e| e.name.counter < b && e.state() == BootState::Good)
                .take(KEEP_PREVIOUS_GOOD)
                .map(|e| e.name.counter);
            std::iter::once(b).chain(previous).collect()
        }
        None => state
            .entries
            .iter()
            .rev()
            .filter(|e| e.state() != BootState::Bad)
            .take(KEEP_PREVIOUS_GOOD + 1)
            .map(|e| e.name.counter)
            .collect(),
    };
    let mut plan = GcPlan::default();
    for e in state
        .entries
        .iter()
        .filter(|e| !keep.contains(&e.name.counter))
    {
        plan.remove.push(e.path.clone());
        plan.freed_bytes += e.size;
    }
    for t in &state.orphan_tmps {
        plan.remove.push(t.path.clone());
        plan.freed_bytes += t.size;
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::{EspEntry, OrphanTmp};

    fn state(names: &[&str], tmps: &[u64]) -> EspState {
        let mut st = EspState::default();
        for n in names {
            let Some(FileKind::Entry(name)) = parse_file_name(n) else {
                panic!("{n}")
            };
            st.entries.push(EspEntry {
                name,
                path: PathBuf::from(n),
                size: 100,
            });
        }
        st.entries.sort_by_key(|e| e.name.counter);
        for t in tmps {
            st.orphan_tmps.push(OrphanTmp {
                counter: *t,
                path: PathBuf::from(format!("tetsu-{t}.efi.tmp")),
                size: 7,
            });
        }
        st
    }

    fn removed(plan: &GcPlan) -> Vec<String> {
        let mut v: Vec<String> = plan
            .remove
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn fewer_than_four_removes_nothing() {
        let st = state(&["tetsu-1.efi", "tetsu-2.efi", "tetsu-3.efi"], &[]);
        let plan = plan_gc(&st, Some(3));
        assert!(plan.remove.is_empty());
        assert_eq!(plan.freed_bytes, 0);
    }

    #[test]
    fn keeps_booted_and_two_previous_good() {
        let st = state(
            &["tetsu-1.efi", "tetsu-2.efi", "tetsu-3.efi", "tetsu-4.efi"],
            &[],
        );
        let plan = plan_gc(&st, Some(4));
        assert_eq!(removed(&plan), vec!["tetsu-1.efi"]);
        assert_eq!(plan.freed_bytes, 100);
    }

    #[test]
    fn bad_entries_and_orphans_are_removed() {
        let st = state(&["tetsu-1+0-3.efi", "tetsu-2.efi", "tetsu-3+0.efi"], &[9]);
        let plan = plan_gc(&st, Some(2));
        assert_eq!(
            removed(&plan),
            vec!["tetsu-1+0-3.efi", "tetsu-3+0.efi", "tetsu-9.efi.tmp"]
        );
        assert_eq!(plan.freed_bytes, 207);
    }

    #[test]
    fn superseded_untried_pending_default_is_removed() {
        let st = state(&["tetsu-1.efi", "tetsu-2.efi", "tetsu-3+3.efi"], &[]);
        let plan = plan_gc(&st, Some(2));
        assert_eq!(removed(&plan), vec!["tetsu-3+3.efi"]);
    }

    #[test]
    fn trying_entry_newer_than_booted_is_removed() {
        let st = state(&["tetsu-1.efi", "tetsu-2+2-1.efi"], &[]);
        let plan = plan_gc(&st, Some(1));
        assert_eq!(removed(&plan), vec!["tetsu-2+2-1.efi"]);
    }

    #[test]
    fn non_good_entries_older_than_booted_are_removed() {
        let st = state(
            &[
                "tetsu-1+3.efi",
                "tetsu-2+1-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi",
            ],
            &[],
        );
        let plan = plan_gc(&st, Some(4));
        assert_eq!(removed(&plan), vec!["tetsu-1+3.efi", "tetsu-2+1-2.efi"]);
    }

    #[test]
    fn booted_is_never_removed_even_if_bad_looking() {
        let st = state(&["tetsu-1.efi", "tetsu-2+0-3.efi", "tetsu-3.efi"], &[]);
        let plan = plan_gc(&st, Some(2));
        // 3 is newer and superseded; the Bad booted 2 stays; 1 is the previous good.
        assert_eq!(removed(&plan), vec!["tetsu-3.efi"]);
    }

    #[test]
    fn booted_trying_entry_stays() {
        let st = state(&["tetsu-1.efi", "tetsu-2+2-1.efi"], &[]);
        assert!(plan_gc(&st, Some(2)).remove.is_empty());
    }

    #[test]
    fn orphans_go_even_when_nothing_else_does() {
        let st = state(&["tetsu-1.efi"], &[2, 3]);
        let plan = plan_gc(&st, Some(1));
        assert_eq!(removed(&plan), vec!["tetsu-2.efi.tmp", "tetsu-3.efi.tmp"]);
        assert_eq!(plan.freed_bytes, 14);
    }

    #[test]
    fn unknown_booted_keeps_three_newest_non_bad() {
        let st = state(
            &[
                "tetsu-1.efi",
                "tetsu-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi",
                "tetsu-5+0-3.efi",
            ],
            &[],
        );
        let plan = plan_gc(&st, None);
        assert_eq!(removed(&plan), vec!["tetsu-1.efi", "tetsu-5+0-3.efi"]);
    }

    #[test]
    fn empty_state_plans_nothing() {
        assert_eq!(plan_gc(&EspState::default(), None), GcPlan::default());
    }

    #[test]
    fn state_helper_sanity() {
        let st = state(&["tetsu-1+3.efi"], &[]);
        assert_eq!(st.entries[0].state(), BootState::Untried);
    }
}
