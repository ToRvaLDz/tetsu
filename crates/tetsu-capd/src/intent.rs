//! Why the agent is about to make a call: the reason an `Intent` gave,
//! kept for a few seconds so the next ask of the same process can show it.

use std::time::{Duration, SystemTime};

/// How long an intent can be matched.
pub const MAX_AGE: Duration = Duration::from_secs(5);
/// Intents kept at once.
pub const MAX_ENTRIES: usize = 64;
/// Bytes of a reason kept.
pub const MAX_REASON: usize = 1024;

#[derive(Debug, Clone)]
struct Entry {
    pid: i64,
    method: String,
    reason: String,
    at: SystemTime,
}

/// The table of recent intents.
#[derive(Debug, Default)]
pub struct Intents {
    entries: Vec<Entry>,
}

impl Intents {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many intents are held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether none is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn fresh(entry: &Entry, now: SystemTime) -> bool {
        now.duration_since(entry.at).is_ok_and(|age| age <= MAX_AGE)
    }

    /// Records an intent of process `pid` for `method`. The reason is cut
    /// to 1 KiB. A newer intent of the same process and method replaces
    /// the older one; expired ones are dropped, and at 64 the oldest goes.
    pub fn record(&mut self, pid: i64, method: &str, reason: &str, now: SystemTime) {
        let _ = (pid, method, reason, now);
        unimplemented!()
    }

    /// The reason for `pid` and `method` if one was recorded at most 5 s
    /// ago. Using it consumes it.
    pub fn take(&mut self, pid: i64, method: &str, now: SystemTime) -> Option<String> {
        let _ = (pid, method, now);
        unimplemented!()
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
pub fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    const M: &str = "org.tetsu.tx.v1.Install";

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn an_intent_matches_within_five_seconds() {
        let mut intents = Intents::new();
        intents.record(7, M, "to build the project", at(100));
        assert_eq!(
            intents.take(7, M, at(105)).as_deref(),
            Some("to build the project")
        );
    }

    #[test]
    fn an_intent_older_than_five_seconds_does_not_match() {
        let mut intents = Intents::new();
        intents.record(7, M, "late", at(100));
        assert_eq!(intents.take(7, M, at(106)), None);
    }

    #[test]
    fn another_pid_or_method_does_not_match() {
        let mut intents = Intents::new();
        intents.record(7, M, "mine", at(100));
        assert_eq!(intents.take(8, M, at(101)), None);
        assert_eq!(intents.take(7, "org.tetsu.tx.v1.Remove", at(101)), None);
        assert_eq!(intents.take(7, M, at(101)).as_deref(), Some("mine"));
    }

    #[test]
    fn an_intent_is_consumed_by_the_first_ask() {
        let mut intents = Intents::new();
        intents.record(7, M, "once", at(100));
        assert!(intents.take(7, M, at(101)).is_some());
        assert_eq!(intents.take(7, M, at(101)), None);
    }

    #[test]
    fn a_newer_intent_replaces_the_older_of_the_same_peer_and_method() {
        let mut intents = Intents::new();
        intents.record(7, M, "first", at(100));
        intents.record(7, M, "second", at(101));
        assert_eq!(intents.len(), 1);
        assert_eq!(intents.take(7, M, at(102)).as_deref(), Some("second"));
    }

    #[test]
    fn the_table_is_capped_and_expired_entries_are_pruned_on_insert() {
        let mut intents = Intents::new();
        for pid in 0..100 {
            intents.record(pid, M, "r", at(100));
        }
        assert_eq!(intents.len(), MAX_ENTRIES);
        // The oldest went first.
        assert_eq!(intents.take(0, M, at(101)), None);
        assert!(intents.take(99, M, at(101)).is_some());
        // Ten seconds on, an insert drops every expired entry.
        intents.record(500, M, "fresh", at(110));
        assert_eq!(intents.len(), 1);
    }

    #[test]
    fn a_reason_is_cut_to_a_kibibyte_at_a_character_boundary() {
        let mut intents = Intents::new();
        intents.record(1, M, &"x".repeat(5000), at(100));
        assert_eq!(intents.take(1, M, at(100)).unwrap().len(), MAX_REASON);
        // 'é' is two bytes: 1023 of them would end mid-character at 1024.
        let reason = format!("a{}", "é".repeat(600));
        intents.record(1, M, &reason, at(100));
        let kept = intents.take(1, M, at(100)).unwrap();
        assert!(kept.len() <= MAX_REASON && reason.starts_with(&kept));
        assert_eq!(kept.len(), 1 + 2 * 511);
    }
}
