//! The decision for one call: a pure function of the grant, the method's
//! destructive flag, a standing suppression and the time.

use std::time::SystemTime;

use tetsu_bus::cap::{DeniedBy, State};
use tetsu_bus::notify::DismissReason;

/// The action id of *Approve*.
pub const APPROVE: &str = "approve";
/// The action id of *Allow once*.
pub const ALLOW_ONCE: &str = "allow_once";
/// The action id of *Always allow*.
pub const ALWAYS_ALLOW: &str = "always_allow";
/// The action id of *Deny*.
pub const DENY: &str = "deny";

/// Which buttons an ask carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    /// *Allow once*, *Always allow*, *Deny*: the class was never chosen.
    Three,
    /// *Approve*, *Deny*.
    Two,
}

/// What `decide` says to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Let the call run; with `notify` the user is told afterwards.
    Allow {
        /// Whether to tell the user.
        notify: bool,
    },
    /// Refuse without asking.
    Deny(DeniedBy),
    /// Ask the user.
    Ask(Buttons),
}

/// Decides one call.
pub fn decide(
    state: State,
    destructive: bool,
    suppressed_until: Option<SystemTime>,
    now: SystemTime,
) -> Verdict {
    let needs_ask = match state {
        State::Off | State::Unknown => return Verdict::Deny(DeniedBy::Off),
        State::Unset | State::Ask => true,
        State::Automatic | State::Notify => destructive,
    };
    if needs_ask {
        if suppressed_until.is_some_and(|until| now < until) {
            return Verdict::Deny(DeniedBy::Suppressed);
        }
        let buttons = if state == State::Unset && !destructive {
            Buttons::Three
        } else {
            Buttons::Two
        };
        return Verdict::Ask(buttons);
    }
    Verdict::Allow {
        notify: state == State::Notify,
    }
}

/// How an ask ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The user pressed a button.
    Invoked(String),
    /// The notification went away unanswered.
    Dismissed(DismissReason),
    /// The notification service could not be reached, or kept failing.
    Unavailable,
}

/// What to do with an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Allow this call.
    Allow,
    /// Allow this call and write `notify` to the managed layer.
    AllowAndAlways,
    /// Deny, and suppress asks for 24 hours.
    DenyAndSuppress,
    /// Deny `unavailable`, with no suppression: the user did not deny.
    DenyUnavailable,
}

/// Maps an answer to what happens. Anything not understood denies.
pub fn answer_effect(buttons: Buttons, answer: &Answer) -> Outcome {
    match (buttons, answer) {
        (Buttons::Two, Answer::Invoked(id)) if id == APPROVE => Outcome::Allow,
        (Buttons::Three, Answer::Invoked(id)) if id == ALLOW_ONCE => Outcome::Allow,
        (Buttons::Three, Answer::Invoked(id)) if id == ALWAYS_ALLOW => Outcome::AllowAndAlways,
        (_, Answer::Invoked(id)) if id == DENY => Outcome::DenyAndSuppress,
        (_, Answer::Dismissed(DismissReason::User | DismissReason::Expired)) => {
            Outcome::DenyAndSuppress
        }
        _ => Outcome::DenyUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_000_000)
    }

    fn later() -> Option<SystemTime> {
        Some(now() + Duration::from_secs(60))
    }

    fn earlier() -> Option<SystemTime> {
        Some(now() - Duration::from_secs(60))
    }

    #[test]
    fn off_denies_first_even_for_a_destructive_class_and_a_suppression() {
        for destructive in [false, true] {
            for suppressed in [None, later()] {
                assert_eq!(
                    decide(State::Off, destructive, suppressed, now()),
                    Verdict::Deny(DeniedBy::Off)
                );
            }
        }
    }

    #[test]
    fn automatic_and_notify_allow_unless_destructive() {
        assert_eq!(
            decide(State::Automatic, false, None, now()),
            Verdict::Allow { notify: false }
        );
        assert_eq!(
            decide(State::Notify, false, None, now()),
            Verdict::Allow { notify: true }
        );
        for state in [State::Automatic, State::Notify] {
            assert_eq!(
                decide(state, true, None, now()),
                Verdict::Ask(Buttons::Two),
                "{state:?}"
            );
        }
    }

    #[test]
    fn unset_asks_with_three_buttons_and_ask_with_two() {
        assert_eq!(
            decide(State::Unset, false, None, now()),
            Verdict::Ask(Buttons::Three)
        );
        assert_eq!(
            decide(State::Ask, false, None, now()),
            Verdict::Ask(Buttons::Two)
        );
        // A destructive class never offers a standing grant.
        assert_eq!(
            decide(State::Unset, true, None, now()),
            Verdict::Ask(Buttons::Two)
        );
    }

    #[test]
    fn a_standing_suppression_denies_what_would_ask() {
        for (state, destructive) in [
            (State::Unset, false),
            (State::Ask, false),
            (State::Automatic, true),
            (State::Notify, true),
        ] {
            assert_eq!(
                decide(state, destructive, later(), now()),
                Verdict::Deny(DeniedBy::Suppressed),
                "{state:?} {destructive}"
            );
            // Expired, or expiring exactly now: asks again.
            for until in [earlier(), Some(now())] {
                assert!(matches!(
                    decide(state, destructive, until, now()),
                    Verdict::Ask(_)
                ));
            }
        }
    }

    #[test]
    fn a_suppression_does_not_touch_what_would_not_ask() {
        assert_eq!(
            decide(State::Automatic, false, later(), now()),
            Verdict::Allow { notify: false }
        );
        assert_eq!(
            decide(State::Notify, false, later(), now()),
            Verdict::Allow { notify: true }
        );
    }

    #[test]
    fn an_unknown_state_fails_closed() {
        assert_eq!(
            decide(State::Unknown, false, None, now()),
            Verdict::Deny(DeniedBy::Off)
        );
    }

    fn invoked(id: &str) -> Answer {
        Answer::Invoked(id.to_owned())
    }

    #[test]
    fn answers_map_to_outcomes() {
        use Buttons::{Three, Two};
        assert_eq!(answer_effect(Two, &invoked(APPROVE)), Outcome::Allow);
        assert_eq!(answer_effect(Three, &invoked(ALLOW_ONCE)), Outcome::Allow);
        assert_eq!(
            answer_effect(Three, &invoked(ALWAYS_ALLOW)),
            Outcome::AllowAndAlways
        );
        for buttons in [Two, Three] {
            assert_eq!(
                answer_effect(buttons, &invoked(DENY)),
                Outcome::DenyAndSuppress
            );
            for reason in [DismissReason::User, DismissReason::Expired] {
                assert_eq!(
                    answer_effect(buttons, &Answer::Dismissed(reason)),
                    Outcome::DenyAndSuppress
                );
            }
            for answer in [
                Answer::Dismissed(DismissReason::Closed),
                Answer::Dismissed(DismissReason::Unknown),
                Answer::Unavailable,
            ] {
                assert_eq!(answer_effect(buttons, &answer), Outcome::DenyUnavailable);
            }
        }
    }

    #[test]
    fn a_button_the_ask_did_not_have_denies() {
        assert_eq!(
            answer_effect(Buttons::Two, &invoked(ALWAYS_ALLOW)),
            Outcome::DenyUnavailable
        );
        assert_eq!(
            answer_effect(Buttons::Two, &invoked(ALLOW_ONCE)),
            Outcome::DenyUnavailable
        );
        assert_eq!(
            answer_effect(Buttons::Three, &invoked(APPROVE)),
            Outcome::DenyUnavailable
        );
        assert_eq!(
            answer_effect(Buttons::Two, &invoked("made_up")),
            Outcome::DenyUnavailable
        );
    }
}
