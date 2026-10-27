//! The clock label and the wait to the next minute, from fixed instants.

use std::time::Duration;

use jiff::Zoned;
use tetsu_bar::clock::{label, until_next_minute};

fn at(text: &str) -> Zoned {
    text.parse().expect("a valid zoned timestamp")
}

#[test]
fn labels_are_zero_padded_hours_and_minutes() {
    assert_eq!(label(&at("2026-03-01T09:05:05[Europe/Rome]")), "09:05");
    assert_eq!(label(&at("2026-03-01T23:59:59[Europe/Rome]")), "23:59");
    assert_eq!(label(&at("2026-03-01T00:00:00[Europe/Rome]")), "00:00");
}

#[test]
fn waits_the_rest_of_the_minute() {
    let wait = until_next_minute(&at("2026-03-01T09:05:05[Europe/Rome]"));
    assert_eq!(wait, Duration::from_secs(55));
    let wait = until_next_minute(&at("2026-03-01T09:05:59.5[Europe/Rome]"));
    assert_eq!(wait, Duration::from_millis(500));
}

#[test]
fn on_the_boundary_it_waits_a_whole_minute_never_zero() {
    let wait = until_next_minute(&at("2026-03-01T09:05:00[Europe/Rome]"));
    assert_eq!(wait, Duration::from_secs(60));
}

#[test]
fn a_dst_transition_minute_stays_positive() {
    // 02:00 -> 03:00 in Rome on 2026-03-29.
    for text in [
        "2026-03-29T01:59:30[Europe/Rome]",
        "2026-03-29T03:00:00[Europe/Rome]",
        // 03:00 -> 02:00 on 2026-10-25, both passes over 02:59.
        "2026-10-25T02:59:30+02:00[Europe/Rome]",
        "2026-10-25T02:59:30+01:00[Europe/Rome]",
    ] {
        let wait = until_next_minute(&at(text));
        assert!(
            wait > Duration::ZERO && wait <= Duration::from_secs(60),
            "{text}"
        );
    }
}
