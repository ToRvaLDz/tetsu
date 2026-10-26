//! The per-path decision of the three-way `/etc` sync. Pure: it looks at
//! three [`Node`]s and says what to do.

use serde::{Deserialize, Serialize};

use super::node::{Kind, Node, same};

/// Why a path is reported as a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// Both sides changed it, differently.
    BothChanged,
    /// The user deleted it and the transaction changed it.
    UserDeleted,
    /// The transaction deleted it and the user changed it.
    TxDeleted,
}

/// What to do with one path of the new tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Leave the new tree as it is.
    Keep,
    /// Replace the path in the new tree with the old one.
    TakeOld,
    /// Delete the path from the new tree.
    DeleteNew,
    /// Write the merge of the account files.
    MergeAccounts,
    /// The new tree wins; `keep_old` says the user's copy is kept aside.
    Conflict { kind: ConflictKind, keep_old: bool },
    /// Both are directories: decide each child. `take_old_attrs` copies the
    /// user's mode and owner onto the new directory.
    Recurse { take_old_attrs: bool },
    /// Not synced: a socket, FIFO or device.
    Skip,
}

/// Decide for one path. `account` is true for the four account files in the
/// top of `/etc`.
pub fn decide(
    base: Option<&Node>,
    old: Option<&Node>,
    new: Option<&Node>,
    account: bool,
) -> Action {
    if [base, old, new].into_iter().flatten().any(Node::is_special) {
        return Action::Skip;
    }
    if let (Some(o), Some(n)) = (old, new)
        && o.is_dir()
        && n.is_dir()
    {
        let take_old_attrs =
            base.is_some_and(|b| b.is_dir() && !same(Some(o), Some(b)) && same(Some(n), Some(b)));
        return Action::Recurse { take_old_attrs };
    }
    let (user, tx) = (!same(old, base), !same(new, base));
    match (user, tx) {
        (false, _) => Action::Keep,
        (true, false) => match old {
            Some(_) => Action::TakeOld,
            None => Action::DeleteNew,
        },
        (true, true) if same(old, new) => Action::Keep,
        (true, true) if account && both_regular(old, new) => Action::MergeAccounts,
        (true, true) => {
            let kind = match (old, new) {
                (None, _) => ConflictKind::UserDeleted,
                (_, None) => ConflictKind::TxDeleted,
                _ => ConflictKind::BothChanged,
            };
            Action::Conflict {
                kind,
                keep_old: old.is_some(),
            }
        }
    }
}

fn both_regular(old: Option<&Node>, new: Option<&Node>) -> bool {
    let regular = |n: Option<&Node>| n.is_some_and(|n| matches!(n.kind, Kind::File(_)));
    regular(old) && regular(new)
}

#[cfg(test)]
mod tests {
    use super::super::node::fixtures::*;
    use super::ConflictKind::*;
    use super::*;

    const KEEP: Action = Action::Keep;

    fn d(b: Option<Node>, o: Option<Node>, n: Option<Node>) -> Action {
        decide(b.as_ref(), o.as_ref(), n.as_ref(), false)
    }

    fn conflict(kind: super::ConflictKind, keep_old: bool) -> Action {
        Action::Conflict { kind, keep_old }
    }

    #[test]
    fn nothing_changed_keeps_the_new_path() {
        assert_eq!(d(None, None, None), KEEP);
        let f = file("a", 0o644);
        assert_eq!(d(Some(f.clone()), Some(f.clone()), Some(f)), KEEP);
    }

    #[test]
    fn only_the_user_changed_it() {
        let (b, o) = (file("a", 0o644), file("b", 0o644));
        assert_eq!(
            d(Some(b.clone()), Some(o), Some(b.clone())),
            Action::TakeOld
        );
        // Added by the user.
        assert_eq!(d(None, Some(file("b", 0o644)), None), Action::TakeOld);
        // Deleted by the user.
        assert_eq!(d(Some(b.clone()), None, Some(b)), Action::DeleteNew);
    }

    #[test]
    fn only_the_transaction_changed_it() {
        let (b, n) = (file("a", 0o644), file("b", 0o644));
        assert_eq!(d(Some(b.clone()), Some(b.clone()), Some(n)), KEEP);
        // Added by the transaction.
        assert_eq!(d(None, None, Some(file("n", 0o644))), KEEP);
        // Deleted by the transaction: stays deleted.
        assert_eq!(d(Some(b.clone()), Some(b), None), KEEP);
    }

    #[test]
    fn both_changed_it_the_same_way_is_a_no_op() {
        let (b, x) = (file("a", 0o644), file("x", 0o644));
        assert_eq!(d(Some(b.clone()), Some(x.clone()), Some(x.clone())), KEEP);
        assert_eq!(d(None, Some(x.clone()), Some(x)), KEEP);
        // Both deleted it.
        assert_eq!(d(Some(b), None, None), KEEP);
    }

    #[test]
    fn both_changed_it_differently_the_new_one_wins() {
        let b = file("a", 0o644);
        assert_eq!(
            d(
                Some(b.clone()),
                Some(file("o", 0o644)),
                Some(file("n", 0o644))
            ),
            conflict(BothChanged, true)
        );
        // Both added it, differently.
        assert_eq!(
            d(None, Some(file("o", 0o644)), Some(file("n", 0o644))),
            conflict(BothChanged, true)
        );
        // The user deleted it, the transaction edited it.
        assert_eq!(
            d(Some(b.clone()), None, Some(file("n", 0o644))),
            conflict(UserDeleted, false)
        );
        // The transaction deleted it, the user edited it.
        assert_eq!(
            d(Some(b), Some(file("o", 0o644)), None),
            conflict(TxDeleted, true)
        );
    }

    #[test]
    fn a_mode_only_or_owner_only_change_counts() {
        let b = file("a", 0o644);
        assert_eq!(
            d(Some(b.clone()), Some(file("a", 0o600)), Some(b.clone())),
            Action::TakeOld
        );
        let mut chowned = b.clone();
        chowned.uid = 1000;
        assert_eq!(
            d(Some(b.clone()), Some(chowned.clone()), Some(b.clone())),
            Action::TakeOld
        );
        assert_eq!(d(Some(b.clone()), Some(b.clone()), Some(chowned)), KEEP);
        // The user changed the mode, the transaction the content.
        assert_eq!(
            d(Some(b), Some(file("a", 0o600)), Some(file("n", 0o644))),
            conflict(BothChanged, true)
        );
    }

    #[test]
    fn a_type_change_is_a_change() {
        let b = dir(0o755);
        // The user replaced a directory with a file.
        assert_eq!(
            d(Some(b.clone()), Some(file("x", 0o644)), Some(b.clone())),
            Action::TakeOld
        );
        // The user replaced a file with a symlink.
        let f = file("a", 0o644);
        assert_eq!(
            d(Some(f.clone()), Some(link("t")), Some(f.clone())),
            Action::TakeOld
        );
        // The transaction replaced a file with a directory.
        assert_eq!(d(Some(f.clone()), Some(f), Some(dir(0o755))), KEEP);
        // The user added a directory where the transaction added a file.
        assert_eq!(
            d(None, Some(dir(0o755)), Some(file("n", 0o644))),
            conflict(BothChanged, true)
        );
    }

    #[test]
    fn symlinks_compare_by_target() {
        let b = link("a");
        assert_eq!(
            d(Some(b.clone()), Some(link("o")), Some(b.clone())),
            Action::TakeOld
        );
        assert_eq!(
            d(Some(b.clone()), Some(link("o")), Some(link("n"))),
            conflict(BothChanged, true)
        );
        assert_eq!(d(Some(b.clone()), Some(link("o")), Some(link("o"))), KEEP);
    }

    #[test]
    fn two_directories_are_recursed_into() {
        let b = dir(0o755);
        let rec = |take_old_attrs| Action::Recurse { take_old_attrs };
        assert_eq!(
            d(Some(b.clone()), Some(b.clone()), Some(b.clone())),
            rec(false)
        );
        assert_eq!(d(None, Some(b.clone()), Some(b.clone())), rec(false));
        // Only the user changed the directory's mode.
        assert_eq!(
            d(Some(b.clone()), Some(dir(0o700)), Some(b.clone())),
            rec(true)
        );
        // Only the transaction did, or both did.
        assert_eq!(
            d(Some(b.clone()), Some(b.clone()), Some(dir(0o700))),
            rec(false)
        );
        assert_eq!(d(Some(b), Some(dir(0o700)), Some(dir(0o750))), rec(false));
    }

    #[test]
    fn a_directory_added_or_removed_on_one_side() {
        let b = dir(0o755);
        assert_eq!(d(None, Some(b.clone()), None), Action::TakeOld);
        assert_eq!(d(None, None, Some(b.clone())), KEEP);
        assert_eq!(d(Some(b.clone()), None, Some(b.clone())), Action::DeleteNew);
        assert_eq!(d(Some(b.clone()), Some(b), None), KEEP);
    }

    #[test]
    fn the_account_files_merge_when_both_sides_changed_them() {
        let acc = |b, o, n| decide(Some(&b), Some(&o), Some(&n), true);
        let (b, o, n) = (file("a\n", 0o644), file("b\n", 0o644), file("c\n", 0o644));
        assert_eq!(acc(b.clone(), o.clone(), n.clone()), Action::MergeAccounts);
        // Only the user changed it.
        assert_eq!(acc(b.clone(), o.clone(), b.clone()), Action::TakeOld);
        // Only the transaction did.
        assert_eq!(acc(b.clone(), b.clone(), n.clone()), KEEP);
        // Both the same way.
        assert_eq!(acc(b, n.clone(), n), KEEP);
    }

    #[test]
    fn an_account_file_that_is_not_a_regular_file_follows_the_plain_rules() {
        let b = file("a\n", 0o644);
        assert_eq!(
            decide(Some(&b), Some(&link("x")), Some(&file("n\n", 0o644)), true),
            conflict(BothChanged, true)
        );
        assert_eq!(
            decide(Some(&b), None, Some(&file("n\n", 0o644)), true),
            conflict(UserDeleted, false)
        );
    }

    #[test]
    fn special_files_are_skipped() {
        let f = file("a", 0o644);
        assert_eq!(d(None, Some(special()), None), Action::Skip);
        assert_eq!(d(None, None, Some(special())), Action::Skip);
        assert_eq!(d(Some(f.clone()), Some(f), Some(special())), Action::Skip);
        assert_eq!(d(Some(special()), None, None), Action::Skip);
    }
}
