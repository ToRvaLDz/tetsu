//! Account file merge: the new snapshot's `/etc` account files keep the
//! system users and groups its packages added, the running system's keep
//! the accounts the user made. Pure text; the first-boot sync reuses it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::boot::BootError;
use crate::tx::TxError;

/// The account files that are merged, relative to `/etc`.
pub const ACCOUNT_FILES: [&str; 4] = ["passwd", "group", "shadow", "gshadow"];

/// Union of two account files keyed on the first `:` field. A key in both
/// takes the `running` line, in the `target` position; `target` order is
/// kept and keys only `running` has are appended in its order. Blank lines
/// are dropped and a repeated key keeps its first line.
pub fn merge(target: &str, running: &str) -> String {
    let key = |line: &str| line.split(':').next().unwrap_or(line).to_owned();
    let mut running_by_key: BTreeMap<String, &str> = BTreeMap::new();
    for line in running.lines().filter(|l| !l.trim().is_empty()) {
        running_by_key.entry(key(line)).or_insert(line);
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out = String::new();
    for line in target.lines().filter(|l| !l.trim().is_empty()) {
        let k = key(line);
        if !seen.insert(k.clone()) {
            continue;
        }
        out.push_str(running_by_key.get(&k).copied().unwrap_or(line));
        out.push('\n');
    }
    for line in running.lines().filter(|l| !l.trim().is_empty()) {
        if seen.insert(key(line)) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// A regular file's text; `None` when missing, a failure for a symlink or
/// anything that is not a regular file.
pub fn read_regular(path: &Path) -> Result<Option<String>, TxError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_file() => {}
        Ok(_) => {
            return Err(TxError::failed(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(BootError::io(path, e).into()),
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|e| BootError::io(path, e).into())
}

/// Carry one account file over: merge `running` into `target`, the running
/// side wins on a shared key. Nothing happens when `running` is missing; a
/// missing `target` is copied whole, mode included.
pub fn merge_file(running: &Path, target: &Path) -> Result<(), TxError> {
    let Some(running_text) = read_regular(running)? else {
        return Ok(());
    };
    // Writing in place keeps the mode of the target.
    let written = match read_regular(target)? {
        Some(target_text) => fs::write(target, merge(&target_text, &running_text)),
        None => fs::copy(running, target).map(drop),
    };
    written.map_err(|e| BootError::io(target, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_wins_on_a_shared_key_in_the_target_position() {
        let target = "root:x:0:0::/root:/bin/bash\ndaemon:x:1:1::/:/usr/bin/nologin\n";
        let running = "daemon:x:1:1:changed:/:/bin/sh\nroot:x:0:0::/root:/bin/zsh\n";
        assert_eq!(
            merge(target, running),
            "root:x:0:0::/root:/bin/zsh\ndaemon:x:1:1:changed:/:/bin/sh\n"
        );
    }

    #[test]
    fn target_only_keys_stay_and_running_only_keys_are_appended() {
        let target = "root:x:0:0\nsystemd-resolve:x:2:2\n";
        let running = "root:x:0:0\nalice:x:1000:1000\nbob:x:1001:1001\n";
        assert_eq!(
            merge(target, running),
            "root:x:0:0\nsystemd-resolve:x:2:2\nalice:x:1000:1000\nbob:x:1001:1001\n"
        );
    }

    #[test]
    fn shadow_and_group_lines_key_on_the_name() {
        let target = "root:!:19000::::::\nnobody:!:19000::::::\n";
        let running = "root:$6$hash:19500::::::\n";
        assert_eq!(
            merge(target, running),
            "root:$6$hash:19500::::::\nnobody:!:19000::::::\n"
        );
        assert_eq!(
            merge("wheel:x:10:\n", "wheel:x:10:alice\n"),
            "wheel:x:10:alice\n"
        );
    }

    #[test]
    fn empty_sides_and_blank_lines() {
        assert_eq!(merge("", ""), "");
        assert_eq!(merge("a:x\n", ""), "a:x\n");
        assert_eq!(merge("", "b:x\n"), "b:x\n");
        assert_eq!(merge("a:x\n\n\nc:x", "b:x\n"), "a:x\nc:x\nb:x\n");
    }

    #[test]
    fn a_repeated_key_keeps_its_first_line() {
        assert_eq!(merge("a:1\na:2\n", ""), "a:1\n");
        assert_eq!(merge("a:1\n", "a:2\na:3\n"), "a:2\n");
    }

    #[test]
    fn merging_is_idempotent() {
        let target = "root:x:0\nsys:x:2\n";
        let running = "root:x:0\nalice:x:1000\n";
        let once = merge(target, running);
        assert_eq!(merge(&once, running), once);
    }

    #[test]
    fn the_four_account_files_are_listed() {
        assert_eq!(ACCOUNT_FILES, ["passwd", "group", "shadow", "gshadow"]);
    }
}
