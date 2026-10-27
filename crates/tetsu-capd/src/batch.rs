//! Grouping for `notify` mode: allowed calls of a class are counted over a
//! window and told to the user once, not one by one.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use tetsu_bus::notify::{ShowParams, Urgency};

/// How long calls of a class are gathered before the user is told.
pub const WINDOW: Duration = Duration::from_secs(10);

/// What to tell the user about one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The class.
    pub class: String,
    /// How many calls were allowed.
    pub count: u32,
    /// The methods called, sorted, once each.
    pub methods: Vec<String>,
}

#[derive(Debug)]
struct Group {
    since: SystemTime,
    count: u32,
    methods: BTreeSet<String>,
}

/// Open groups, one per class.
#[derive(Debug, Default)]
pub struct Batcher {
    open: BTreeMap<String, Group>,
}

impl Batcher {
    /// No group open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts one allowed call; the first of a class opens its window.
    pub fn record(&mut self, class: &str, method: &str, now: SystemTime) {
        let _ = (class, method, now);
        unimplemented!()
    }

    /// Takes the groups whose window has ended.
    pub fn due(&mut self, now: SystemTime) -> Vec<Notice> {
        let _ = now;
        unimplemented!()
    }
}

/// The informational notification of a group: low urgency, no actions.
pub fn show_params(notice: &Notice) -> ShowParams {
    let _ = Urgency::Low;
    let _ = notice;
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use super::*;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn nothing_is_due_inside_the_window() {
        let mut batcher = Batcher::new();
        batcher.record("a.b", "x.y.M", at(100));
        assert!(batcher.due(at(109)).is_empty());
    }

    #[test]
    fn a_group_is_due_when_the_window_ends_and_only_once() {
        let mut batcher = Batcher::new();
        batcher.record("a.b", "x.y.M", at(100));
        batcher.record("a.b", "x.y.N", at(104));
        batcher.record("a.b", "x.y.M", at(108));
        let due = batcher.due(at(110));
        assert_eq!(
            due,
            vec![Notice {
                class: "a.b".into(),
                count: 3,
                methods: vec!["x.y.M".into(), "x.y.N".into()],
            }]
        );
        assert!(batcher.due(at(200)).is_empty());
    }

    #[test]
    fn classes_are_grouped_apart_and_a_new_call_opens_a_new_window() {
        let mut batcher = Batcher::new();
        batcher.record("a.b", "x.y.M", at(100));
        batcher.record("c.d", "x.y.Q", at(105));
        let due = batcher.due(at(110));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].class, "a.b");
        batcher.record("a.b", "x.y.M", at(111));
        assert!(batcher.due(at(115)).is_empty());
        assert_eq!(batcher.due(at(121)).len(), 2);
    }

    #[test]
    fn the_notification_is_low_and_has_no_buttons() {
        let notice = Notice {
            class: "packages.install".into(),
            count: 3,
            methods: vec!["org.tetsu.tx.v1.Install".into()],
        };
        let params = show_params(&notice);
        assert_eq!(params.title, "The agent used packages.install 3 times");
        assert_eq!(params.urgency, Some(Urgency::Low));
        assert_eq!(params.actions, None);
        assert!(params.body.unwrap().contains("org.tetsu.tx.v1.Install"));
        let once = Notice { count: 1, ..notice };
        assert_eq!(
            show_params(&once).title,
            "The agent used packages.install 1 time"
        );
    }
}
