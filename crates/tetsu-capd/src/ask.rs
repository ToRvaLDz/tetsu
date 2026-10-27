//! The ask: what the user sees, and the round-trip with the notification
//! service.

use std::io;
use std::time::Duration;

use serde_json::Value;
use tetsu_bus::notify::{Action, DismissReason, NotifyKind, NotifyReply, ShowParams, Urgency};

pub use crate::decide::Answer;
use crate::decide::{ALLOW_ONCE, ALWAYS_ALLOW, APPROVE, Buttons, DENY};

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
    let mut lines = vec![
        format!("Method: {}", request.method),
        format!("Class: {}", request.class),
    ];
    lines.extend(argument_lines(request.args));
    if let Some(reason) = request.reason {
        lines.push(format!(
            "Reason given by the agent (unverified): {}",
            fence(&Value::String(reason.to_owned()))
        ));
    }
    let actions = match request.buttons {
        Buttons::Three => vec![
            Action::new(ALLOW_ONCE, "Allow once"),
            Action::new(ALWAYS_ALLOW, "Always allow"),
            Action::new(DENY, "Deny"),
        ],
        Buttons::Two => vec![Action::new(APPROVE, "Approve"), Action::new(DENY, "Deny")],
    };
    ShowParams {
        title: format!("The agent wants to run {}", request.method),
        body: Some(lines.join("\n")),
        urgency: Some(Urgency::Critical),
        actions: Some(actions),
        timeout_ms: None,
    }
}

/// The `Arguments:` block: one line per top-level argument.
fn argument_lines(args: &Value) -> Vec<String> {
    let map = match args {
        Value::Object(map) if !map.is_empty() => map,
        Value::Null => return vec!["Arguments: none".to_owned()],
        Value::Object(_) => return vec!["Arguments: none".to_owned()],
        other => {
            return vec!["Arguments:".to_owned(), format!("value: {}", fence(other))];
        }
    };
    let mut lines = vec!["Arguments:".to_owned()];
    for (name, value) in map.iter().take(MAX_ARGS) {
        let name = cut(&clean(name));
        lines.push(format!("{name}: {}", fence(value)));
    }
    if map.len() > MAX_ARGS {
        lines.push("…".to_owned());
    }
    lines
}

/// A value as one safe line: JSON-rendered, so a string is quoted and its
/// newlines are escapes, then stripped of control characters and cut.
fn fence(value: &Value) -> String {
    cut(&clean(&value.to_string()))
}

/// Drops control characters, line and paragraph separators and the bidi
/// overrides: nothing that can start a line or reorder the text.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}

/// At most `MAX_VALUE` characters, then `…`.
fn cut(text: &str) -> String {
    match text.char_indices().nth(MAX_VALUE) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
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
    for attempt in 0..=backoff.len() {
        if attempt > 0 {
            tokio::time::sleep(backoff[attempt - 1]).await;
        }
        observer(attempt);
        let Ok(mut events) = channel.show(params.clone()).await else {
            continue;
        };
        // An error is the end of the stream: show again.
        while let Ok(reply) = events.next().await {
            match reply.kind {
                NotifyKind::Invoked => {
                    return Answer::Invoked(reply.action_id.unwrap_or_default());
                }
                NotifyKind::Dismissed => {
                    let reason = reply.reason.unwrap_or(DismissReason::Unknown);
                    return Answer::Dismissed(reason);
                }
                NotifyKind::Shown | NotifyKind::Unknown => {}
            }
        }
    }
    Answer::Unavailable
}
