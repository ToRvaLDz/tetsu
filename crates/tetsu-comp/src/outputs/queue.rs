//! The output changes owed to the tty backend. While the backend is taken
//! out of the state (a hotplug rescan is running), changes made by a nested
//! reapply wait here and are applied once it is back.

use super::glue::{OutputChange, Refused};

/// Where the changes end up: the tty backend, or a recorder in a test.
pub trait ChangeSink {
    /// Applies the changes, already in a safe order. An error is the first
    /// refusal; the other changes were still tried.
    fn apply(&mut self, changes: &[OutputChange]) -> Result<(), Refused>;
}

/// Changes that wait for a sink.
#[derive(Debug, Default)]
pub struct ChangeQueue {
    pending: Vec<OutputChange>,
}

fn id_of(change: &OutputChange) -> &str {
    match change {
        OutputChange::Mode(id) | OutputChange::Disabled(id) | OutputChange::Enabled(id) => id,
    }
}

/// Disabled first (it frees CRTCs), then Mode, then Enabled. Per output only
/// the last light or dark counts, and a mode of an output left dark is moot.
pub fn order(changes: &[OutputChange]) -> Vec<OutputChange> {
    let last_light = |id: &str| {
        changes
            .iter()
            .rposition(|c| id_of(c) == id && !matches!(c, OutputChange::Mode(_)))
    };
    let rank = |c: &OutputChange| match c {
        OutputChange::Disabled(_) => 0,
        OutputChange::Mode(_) => 1,
        OutputChange::Enabled(_) => 2,
    };
    let mut kept: Vec<OutputChange> = changes
        .iter()
        .enumerate()
        .filter(|(at, change)| match change {
            OutputChange::Mode(id) => !matches!(
                last_light(id).map(|i| &changes[i]),
                Some(OutputChange::Disabled(_))
            ),
            _ => last_light(id_of(change)) == Some(*at),
        })
        .map(|(_, change)| change.clone())
        .collect();
    kept.sort_by_key(rank);
    kept
}

impl ChangeQueue {
    /// What waits.
    pub fn pending(&self) -> &[OutputChange] {
        &self.pending
    }

    /// Whether nothing waits.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Puts what `later` holds after what this holds.
    pub fn absorb(&mut self, later: ChangeQueue) {
        self.pending.extend(later.pending);
    }

    /// With a sink: applies what waited plus `changes`. Without: keeps them.
    pub fn submit(
        &mut self,
        changes: &[OutputChange],
        sink: Option<&mut dyn ChangeSink>,
    ) -> Result<(), Refused> {
        self.pending.extend_from_slice(changes);
        let Some(sink) = sink else {
            return Ok(());
        };
        let batch = order(&std::mem::take(&mut self.pending));
        if batch.is_empty() {
            return Ok(());
        }
        sink.apply(&batch)
    }
}
