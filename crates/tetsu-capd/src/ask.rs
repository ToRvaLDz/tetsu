//! The ask: what the user sees, and the round-trip with the notification
//! service.

use std::io;
use std::time::Duration;

use serde_json::Value;
use tetsu_bus::notify::{NotifyReply, ShowParams};

pub use crate::decide::Answer;
use crate::decide::Buttons;

/// The pauses before the 2nd, 3rd and 4th show.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Longest rendered value or name, in characters.
pub const MAX_VALUE: usize = 200;
/// Arguments shown; more are summarised by `…`.
pub const MAX_ARGS: usize = 10;

/// What an ask is about.
#[derive(Debug, Clone, Copy)]
pub struct AskRequest<'a> {
    /// The fully qualified method.
    pub method: &'a str,
    /// Its class.
    pub class: &'a str,
    /// The arguments, untrusted.
    pub args: &'a Value,
    /// The reason an `Intent` gave, untrusted.
    pub reason: Option<&'a str>,
    /// The buttons.
    pub buttons: Buttons,
}

/// The notification of an ask.
pub fn render(request: &AskRequest<'_>) -> ShowParams {
    let _ = request;
    unimplemented!()
}

/// The replies of one `Show`.
pub trait ShowEvents {
    /// The next reply; an error once the stream ended.
    fn next(&mut self) -> impl Future<Output = io::Result<NotifyReply>>;
}

/// The notification service, as the ask needs it.
pub trait NotifyChannel {
    /// The replies of one show. Dropping them withdraws the notification.
    type Events: ShowEvents;

    /// Shows a notification and follows its replies.
    fn show(&self, params: ShowParams) -> impl Future<Output = io::Result<Self::Events>>;

    /// Shows a notification nobody answers (oneway).
    fn notice(&self, params: ShowParams) -> impl Future<Output = io::Result<()>>;
}

/// Shows the ask and waits for the answer, with no timeout. When the
/// stream ends or the connect fails before an answer, shows it again after
/// each pause of `backoff`; once those are used up the answer is
/// `Unavailable`. `observer` is told the index of every show attempt
/// before it is made. Dropping the future withdraws the notification.
pub async fn run_ask<C: NotifyChannel>(
    channel: &C,
    params: &ShowParams,
    backoff: &[Duration],
    observer: &mut dyn FnMut(usize),
) -> Answer {
    let _ = (channel, params, backoff, observer);
    unimplemented!()
}
