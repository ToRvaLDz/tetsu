//! The notification state machine: every rule of #23 that does not need a
//! socket or a screen.

use std::time::{Duration, Instant};

use tetsu_bus::notify::{Action, DismissReason, NotifyReply, ShowParams, Urgency};
use tetsu_notifyd::core::{
    ARM_DELAY, COLLAPSE_AFTER, Caller, Core, Effect, FdoShow, HISTORY_LIMIT, NoteId, Place,
    Refused, ShowError, Source,
};

fn caller(pid: i32) -> Caller {
    Caller {
        pid,
        sender: format!("app-{pid}.service"),
        agent: false,
    }
}

fn agent() -> Caller {
    Caller {
        pid: 900,
        sender: "tetsu-agent.service".into(),
        agent: true,
    }
}

fn approval() -> ShowParams {
    ShowParams {
        title: "Install firefox?".into(),
        body: Some("Requested by the agent".into()),
        urgency: Some(Urgency::Critical),
        actions: Some(vec![
            Action::new("approve", "Approve"),
            Action::new("deny", "Deny"),
        ]),
        timeout_ms: None,
    }
}

fn info(timeout_ms: Option<i64>) -> ShowParams {
    ShowParams {
        title: "Backup done".into(),
        body: None,
        urgency: None,
        actions: None,
        timeout_ms,
    }
}

fn replies_for(effects: &[Effect], note: NoteId) -> Vec<(NotifyReply, bool)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Reply {
                note: n,
                reply,
                last,
            } if *n == note => Some((reply.clone(), *last)),
            _ => None,
        })
        .collect()
}

fn pending_changes(effects: &[Effect]) -> Vec<i64> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::PendingChanged(n) => Some(*n),
            _ => None,
        })
        .collect()
}

struct Clock(Instant);

impl Clock {
    fn new() -> Self {
        Self(Instant::now())
    }
    fn at(&self, ms: u64) -> Instant {
        self.0 + Duration::from_millis(ms)
    }
}

fn armed(clock: &Clock) -> Instant {
    clock.at(ARM_DELAY.as_millis() as u64)
}

// --- Show ---------------------------------------------------------------

#[test]
fn show_replies_shown_with_its_id() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, effects) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let replies = replies_for(&effects, note);
    assert_eq!(
        replies,
        vec![(NotifyReply::shown(&core.tetsu_id(note)), false)]
    );
}

#[test]
fn an_agent_caller_may_not_send_actions() {
    let mut core = Core::new();
    let result = core.show_tetsu(&agent(), approval(), false, Instant::now());
    assert_eq!(result.unwrap_err(), ShowError::PermissionDenied);
}

#[test]
fn an_agent_caller_may_send_an_informational_notification() {
    let mut core = Core::new();
    assert!(
        core.show_tetsu(&agent(), info(None), false, Instant::now())
            .is_ok()
    );
}

#[test]
fn oneway_rejects_actions_and_critical() {
    let mut core = Core::new();
    let now = Instant::now();
    assert_eq!(
        core.show_tetsu(&caller(1), approval(), true, now)
            .unwrap_err(),
        ShowError::InvalidParameter("actions")
    );
    let critical = ShowParams {
        urgency: Some(Urgency::Critical),
        ..info(None)
    };
    assert_eq!(
        core.show_tetsu(&caller(1), critical, true, now)
            .unwrap_err(),
        ShowError::InvalidParameter("urgency")
    );
}

#[test]
fn invalid_parameters_are_refused() {
    let mut core = Core::new();
    let now = Instant::now();
    let duplicate = ShowParams {
        actions: Some(vec![Action::new("a", "A"), Action::new("a", "B")]),
        ..info(None)
    };
    assert_eq!(
        core.show_tetsu(&caller(1), duplicate, false, now)
            .unwrap_err(),
        ShowError::InvalidParameter("actions")
    );
    assert_eq!(
        core.show_tetsu(&caller(1), info(Some(0)), false, now)
            .unwrap_err(),
        ShowError::InvalidParameter("timeout_ms")
    );
    let blank = ShowParams {
        title: "  ".into(),
        ..info(None)
    };
    assert_eq!(
        core.show_tetsu(&caller(1), blank, false, now).unwrap_err(),
        ShowError::InvalidParameter("title")
    );
    let unknown = ShowParams {
        urgency: Some(Urgency::Unknown),
        ..info(None)
    };
    assert_eq!(
        core.show_tetsu(&caller(1), unknown, false, now)
            .unwrap_err(),
        ShowError::InvalidParameter("urgency")
    );
}

// --- Answering ------------------------------------------------------------

#[test]
fn a_click_answers_only_the_owner_and_ends_its_stream() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (mine, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let (theirs, _) = core
        .show_tetsu(&caller(11), approval(), false, clock.at(0))
        .unwrap();
    let effects = core.click(mine, "approve", armed(&clock)).unwrap();
    assert_eq!(
        replies_for(&effects, mine),
        vec![(NotifyReply::invoked("approve"), true)]
    );
    assert!(replies_for(&effects, theirs).is_empty());
    assert!(core.is_open(theirs));
    assert!(!core.is_open(mine));
}

#[test]
fn a_click_before_arming_is_refused() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let early = clock.at(ARM_DELAY.as_millis() as u64 - 1);
    assert_eq!(core.click(note, "approve", early), Err(Refused::NotArmed));
    assert!(core.is_open(note));
}

#[test]
fn an_unknown_action_is_refused() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    assert_eq!(
        core.click(note, "format-disk", armed(&clock)),
        Err(Refused::NoSuchAction)
    );
}

#[test]
fn nothing_is_invocable_while_locked() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    core.set_locked(true, clock.at(10));
    assert_eq!(
        core.click(note, "approve", clock.at(5_000)),
        Err(Refused::Locked)
    );
    assert_eq!(core.dismiss(note, clock.at(5_000)), Err(Refused::Locked));
    assert!(core.is_open(note));
}

#[test]
fn unlock_re_pops_pending_approvals_and_re_arms_them() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    core.collapse(note, clock.at(0));
    core.set_locked(true, clock.at(1_000));
    core.set_locked(false, clock.at(60_000));
    assert_eq!(core.place(note), Some(Place::Popup));
    assert_eq!(
        core.click(note, "approve", clock.at(60_000 + 10)),
        Err(Refused::NotArmed)
    );
    assert!(
        core.click(note, "approve", clock.at(60_000) + ARM_DELAY)
            .is_ok()
    );
}

#[test]
fn a_user_dismissal_answers_dismissed_user() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let effects = core.dismiss(note, clock.at(1)).unwrap();
    assert_eq!(
        replies_for(&effects, note),
        vec![(NotifyReply::dismissed(DismissReason::User), true)]
    );
}

// --- Close and withdrawal ---------------------------------------------------

#[test]
fn the_owner_closes_its_notification_from_any_connection() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let id = core.tetsu_id(note);
    let effects = core.close_tetsu(10, &id, Instant::now()).unwrap();
    assert_eq!(
        replies_for(&effects, note),
        vec![(NotifyReply::dismissed(DismissReason::Closed), true)]
    );
    assert!(!core.is_open(note));
}

#[test]
fn another_process_cannot_close_or_learn_of_it() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let id = core.tetsu_id(note);
    assert_eq!(
        core.close_tetsu(11, &id, Instant::now()),
        Err(ShowError::NotFound)
    );
    assert_eq!(
        core.close_tetsu(10, "n-999", Instant::now()),
        Err(ShowError::NotFound)
    );
    assert_eq!(
        core.close_tetsu(10, "garbage", Instant::now()),
        Err(ShowError::NotFound)
    );
    assert!(core.is_open(note));
}

#[test]
fn a_caller_disconnecting_withdraws_without_a_reply() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let effects = core.caller_gone(note, Instant::now());
    assert!(replies_for(&effects, note).is_empty());
    assert_eq!(pending_changes(&effects), vec![0]);
    assert!(!core.is_open(note));
}

#[test]
fn a_oneway_notification_has_no_caller_to_lose() {
    let mut core = Core::new();
    let (note, effects) = core
        .show_tetsu(&caller(10), info(None), true, Instant::now())
        .unwrap();
    assert!(replies_for(&effects, note).is_empty());
    core.caller_gone(note, Instant::now());
    assert!(core.is_open(note));
}

// --- Expiry, collapse and the panel ------------------------------------------

#[test]
fn a_normal_notification_expires_on_its_timeout() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), info(Some(2_000)), false, clock.at(0))
        .unwrap();
    assert!(replies_for(&core.tick(clock.at(1_999)), note).is_empty());
    let effects = core.tick(clock.at(2_000));
    assert_eq!(
        replies_for(&effects, note),
        vec![(NotifyReply::dismissed(DismissReason::Expired), true)]
    );
}

#[test]
fn a_critical_notification_never_expires_and_collapses_into_the_panel() {
    let clock = Clock::new();
    let mut core = Core::new();
    let params = ShowParams {
        timeout_ms: Some(1),
        ..approval()
    };
    let (note, _) = core
        .show_tetsu(&caller(10), params, false, clock.at(0))
        .unwrap();
    let later = clock.at(COLLAPSE_AFTER.as_millis() as u64 + 3_600_000);
    let effects = core.tick(later);
    assert!(replies_for(&effects, note).is_empty());
    assert_eq!(core.place(note), Some(Place::Panel));
    assert_eq!(core.pending_count(), 1);
    // Still answerable from the panel, with a fresh arming when it opens.
    core.panel_opened(later);
    assert_eq!(core.click(note, "approve", later), Err(Refused::NotArmed));
    let effects = core.click(note, "approve", later + ARM_DELAY).unwrap();
    assert_eq!(
        replies_for(&effects, note),
        vec![(NotifyReply::invoked("approve"), true)]
    );
}

#[test]
fn a_fourth_popup_pushes_the_oldest_into_the_panel() {
    let clock = Clock::new();
    let mut core = Core::new();
    let notes: Vec<NoteId> = (0..4)
        .map(|i| {
            core.show_tetsu(&caller(10 + i), approval(), false, clock.at(i as u64))
                .unwrap()
                .0
        })
        .collect();
    assert_eq!(core.place(notes[0]), Some(Place::Panel));
    for note in &notes[1..] {
        assert_eq!(core.place(*note), Some(Place::Popup));
    }
    assert_eq!(core.pending_count(), 4);
}

// --- Pending count ------------------------------------------------------------

#[test]
fn the_pending_count_covers_every_caller_with_actions() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (_, e1) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let (_, e2) = core
        .show_tetsu(&caller(11), approval(), false, clock.at(0))
        .unwrap();
    let (_, e3) = core
        .show_tetsu(&caller(12), info(None), false, clock.at(0))
        .unwrap();
    assert_eq!(pending_changes(&e1), vec![1]);
    assert_eq!(pending_changes(&e2), vec![2]);
    assert!(pending_changes(&e3).is_empty());
    assert_eq!(core.pending_count(), 2);
}

// --- History --------------------------------------------------------------------

#[test]
fn history_keeps_the_most_recent_entries_only() {
    let clock = Clock::new();
    let mut core = Core::new();
    for i in 0..(HISTORY_LIMIT as u64 + 5) {
        let (note, _) = core
            .show_tetsu(&caller(10), info(None), false, clock.at(i))
            .unwrap();
        core.dismiss(note, clock.at(i)).unwrap();
    }
    let view = core.view(clock.at(10_000));
    assert_eq!(view.history.len(), HISTORY_LIMIT);
}

// --- freedesktop -----------------------------------------------------------------

fn fdo(replaces_id: u32, expire_timeout: i32) -> FdoShow {
    FdoShow {
        app_name: "firefox".into(),
        client: ":1.42".into(),
        replaces_id,
        summary: "Download finished".into(),
        body: "file.iso".into(),
        actions: vec![
            "default".into(),
            "Open".into(),
            "show".into(),
            "Show".into(),
        ],
        urgency: Urgency::Normal,
        category: Some("transfer.complete".into()),
        expire_timeout,
        icon: None,
    }
}

fn fdo_closed(effects: &[Effect]) -> Vec<(u32, u32)> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::FdoClosed { id, reason, .. } => Some((*id, *reason)),
            _ => None,
        })
        .collect()
}

#[test]
fn fdo_ids_are_non_zero_and_replaces_id_keeps_the_id() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (id, _) = core.show_fdo(fdo(0, -1), clock.at(0));
    assert_ne!(id, 0);
    let (again, effects) = core.show_fdo(fdo(id, -1), clock.at(10));
    assert_eq!(again, id);
    assert!(fdo_closed(&effects).is_empty());
    let (unknown, _) = core.show_fdo(fdo(4_000_000, -1), clock.at(10));
    assert_ne!(unknown, 4_000_000);
}

#[test]
fn fdo_actions_and_closing() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (id, _) = core.show_fdo(fdo(0, -1), clock.at(0));
    let note = core.fdo_note(id).unwrap();
    let effects = core.click(note, "show", armed(&clock)).unwrap();
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::FdoActionInvoked { id: i, action, .. } if *i == id && action == "show"
    )));
    assert_eq!(fdo_closed(&effects), vec![(id, 2)]);

    let (id, _) = core.show_fdo(fdo(0, -1), clock.at(0));
    assert_eq!(
        fdo_closed(&core.close_fdo(id, Instant::now())),
        vec![(id, 3)]
    );
    assert!(core.close_fdo(id, Instant::now()).is_empty());
}

#[test]
fn fdo_expiry_follows_expire_timeout() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (short, _) = core.show_fdo(fdo(0, 1_000), clock.at(0));
    let (never, _) = core.show_fdo(fdo(0, 0), clock.at(0));
    let effects = core.tick(clock.at(1_000));
    assert_eq!(fdo_closed(&effects), vec![(short, 1)]);
    assert!(core.fdo_note(never).is_some());
    assert_eq!(core.pending_count(), 0, "fdo actions are not approvals");
}

#[test]
fn the_view_marks_tetsu_senders_by_their_credentials() {
    let clock = Clock::new();
    let mut core = Core::new();
    core.show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    core.show_fdo(fdo(0, -1), clock.at(0));
    let view = core.view(clock.at(0));
    let sources: Vec<&Source> = view.popups.iter().map(|n| &n.source).collect();
    assert!(sources.contains(&&Source::Tetsu("app-10.service".into())));
    assert!(sources.contains(&&Source::App("firefox".into())));
    // A freedesktop notification never gets the Tetsu frame, whatever it claims.
    let mut lying = fdo(0, -1);
    lying.app_name = "app-10.service".into();
    core.show_fdo(lying, clock.at(0));
    let view = core.view(clock.at(0));
    assert_eq!(
        view.popups
            .iter()
            .filter(|n| matches!(n.source, Source::Tetsu(_)))
            .count(),
        1
    );
}

#[test]
fn the_view_hides_everything_while_locked() {
    let clock = Clock::new();
    let mut core = Core::new();
    core.show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    core.set_locked(true, clock.at(1));
    let view = core.view(clock.at(2));
    assert!(view.locked);
    assert!(view.popups.is_empty());
}

// --- Arming follows what the user sees ------------------------------------

#[test]
fn opening_the_panel_re_arms_pending_approvals_still_in_popups() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (note, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    core.panel_opened(clock.at(1_000));
    assert_eq!(
        core.click(note, "approve", clock.at(1_000)),
        Err(Refused::NotArmed)
    );
    assert!(
        core.click(note, "approve", clock.at(1_000) + ARM_DELAY)
            .is_ok()
    );
}

#[test]
fn a_popup_that_moves_up_re_arms() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (top, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let (below, _) = core
        .show_tetsu(&caller(11), approval(), false, clock.at(0))
        .unwrap();
    // The card above goes away: the one below slides into its place.
    core.caller_gone(top, clock.at(2_000));
    assert_eq!(
        core.click(below, "approve", clock.at(2_000)),
        Err(Refused::NotArmed)
    );
    assert!(
        core.click(below, "approve", clock.at(2_000) + ARM_DELAY)
            .is_ok()
    );
}

#[test]
fn a_popup_that_stays_put_keeps_its_arming() {
    let clock = Clock::new();
    let mut core = Core::new();
    let (top, _) = core
        .show_tetsu(&caller(10), approval(), false, clock.at(0))
        .unwrap();
    let (below, _) = core
        .show_tetsu(&caller(11), approval(), false, clock.at(0))
        .unwrap();
    core.caller_gone(below, clock.at(2_000));
    assert!(core.click(top, "approve", clock.at(2_000)).is_ok());
}

#[test]
fn everything_shown_under_the_lock_arms_from_the_unlock() {
    let clock = Clock::new();
    let mut core = Core::new();
    core.set_locked(true, clock.at(0));
    let (id, _) = core.show_fdo(fdo(0, 0), clock.at(10));
    let note = core.fdo_note(id).unwrap();
    core.set_locked(false, clock.at(5_000));
    assert_eq!(
        core.click(note, "show", clock.at(5_000)),
        Err(Refused::NotArmed)
    );
}

#[test]
fn the_category_reaches_the_view() {
    let mut core = Core::new();
    core.show_fdo(fdo(0, -1), Instant::now());
    let view = core.view(Instant::now());
    assert_eq!(
        view.popups[0].category.as_deref(),
        Some("transfer.complete")
    );
}
