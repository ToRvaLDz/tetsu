//! Output changes that arrive while the tty backend is taken out of the
//! state are kept and applied once it is back, in a safe order.

use tetsu_comp::outputs::glue::{OutputChange, Refused};
use tetsu_comp::outputs::queue::{ChangeQueue, ChangeSink};

use OutputChange::{Disabled, Enabled, Mode};

fn id(s: &str) -> String {
    s.to_owned()
}

/// Records what it was asked to apply.
#[derive(Default)]
struct Recorder {
    calls: Vec<Vec<OutputChange>>,
    refuse: bool,
}

impl ChangeSink for Recorder {
    fn apply(&mut self, changes: &[OutputChange]) -> Result<(), Refused> {
        self.calls.push(changes.to_vec());
        if self.refuse {
            return Err(Refused {
                id: id("a"),
                reason: id("no"),
            });
        }
        Ok(())
    }
}

#[test]
fn changes_made_while_the_sink_is_away_are_applied_once_it_is_back() {
    let mut queue = ChangeQueue::default();
    // The nested case: the tty is taken out, a reapply lights the panel.
    assert_eq!(queue.submit(&[Enabled(id("eDP-1"))], None), Ok(()));
    assert_eq!(queue.pending(), [Enabled(id("eDP-1"))]);

    let mut sink = Recorder::default();
    let outcome = queue.submit(&[], Some(&mut sink));
    assert_eq!(outcome, Ok(()));
    assert_eq!(sink.calls, vec![vec![Enabled(id("eDP-1"))]]);
    assert!(queue.pending().is_empty());

    // Applied once: a later submit does not replay it.
    queue.submit(&[], Some(&mut sink)).unwrap();
    assert_eq!(sink.calls.len(), 1);
}

#[test]
fn queued_changes_come_before_the_new_ones_of_the_same_kind() {
    let mut queue = ChangeQueue::default();
    queue.submit(&[Mode(id("a"))], None).unwrap();
    let mut sink = Recorder::default();
    queue.submit(&[Mode(id("b"))], Some(&mut sink)).unwrap();
    assert_eq!(sink.calls, vec![vec![Mode(id("a")), Mode(id("b"))]]);
}

#[test]
fn disabled_goes_first_then_mode_then_enabled() {
    let mut queue = ChangeQueue::default();
    let mut sink = Recorder::default();
    let changes = [
        Enabled(id("a")),
        Mode(id("b")),
        Disabled(id("c")),
        Disabled(id("d")),
    ];
    queue.submit(&changes, Some(&mut sink)).unwrap();
    assert_eq!(
        sink.calls,
        vec![vec![
            Disabled(id("c")),
            Disabled(id("d")),
            Mode(id("b")),
            Enabled(id("a")),
        ]]
    );
}

#[test]
fn the_last_word_on_an_output_wins() {
    let mut queue = ChangeQueue::default();
    let mut sink = Recorder::default();
    // Lit, then dark again: the display never needs lighting.
    let changes = [Enabled(id("a")), Mode(id("a")), Disabled(id("a"))];
    queue.submit(&changes, Some(&mut sink)).unwrap();
    assert_eq!(sink.calls, vec![vec![Disabled(id("a"))]]);
}

#[test]
fn nothing_to_apply_does_not_call_the_sink() {
    let mut queue = ChangeQueue::default();
    let mut sink = Recorder::default();
    queue.submit(&[], Some(&mut sink)).unwrap();
    assert!(sink.calls.is_empty());
}

#[test]
fn a_refusal_is_returned_and_the_queue_is_drained() {
    let mut queue = ChangeQueue::default();
    queue.submit(&[Enabled(id("a"))], None).unwrap();
    let mut sink = Recorder {
        refuse: true,
        ..Recorder::default()
    };
    assert!(queue.submit(&[], Some(&mut sink)).is_err());
    assert!(queue.pending().is_empty());
}

#[test]
fn absorb_keeps_the_order_of_arrival() {
    let mut first = ChangeQueue::default();
    first.submit(&[Mode(id("a"))], None).unwrap();
    let mut second = ChangeQueue::default();
    second.submit(&[Mode(id("b"))], None).unwrap();
    first.absorb(second);
    assert_eq!(first.pending(), [Mode(id("a")), Mode(id("b"))]);
}
