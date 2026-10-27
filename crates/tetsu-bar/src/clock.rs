//! The clock segment: the label and the wait to the next minute boundary.
//! The instant is a parameter, so tests pass fixed values.

use std::time::Duration;

use jiff::Zoned;

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const MINUTE: Duration = Duration::from_secs(60);

/// The local time now, in the system time zone.
pub fn now() -> Zoned {
    Zoned::now()
}

/// `HH:MM`.
pub fn label(now: &Zoned) -> String {
    format!("{:02}:{:02}", now.hour(), now.minute())
}

/// The time until the next minute starts. On the boundary itself it is a
/// whole minute, never zero, so a timer re-armed from its own tick does not
/// spin.
pub fn until_next_minute(now: &Zoned) -> Duration {
    let into_minute =
        i64::from(now.second()) * NANOS_PER_SECOND + i64::from(now.subsec_nanosecond());
    MINUTE.saturating_sub(Duration::from_nanos(into_minute.unsigned_abs()))
}
