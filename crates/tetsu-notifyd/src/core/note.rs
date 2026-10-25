//! One live notification.

use std::time::Instant;

use tetsu_bus::notify::{Action, Urgency};

use super::types::{HistoryView, Icon, NoteId, NoteView, Outcome, Place, Source};

/// The freedesktop action invoked by clicking the body.
pub(super) const DEFAULT_ACTION: &str = "default";

#[derive(Debug, Clone)]
pub(super) enum Origin {
    /// A Varlink caller; `oneway` notes have no stream to answer.
    Tetsu {
        pid: i32,
        sender: String,
        oneway: bool,
    },
    /// A freedesktop client.
    Fdo { app_name: String, client: String },
}

#[derive(Debug, Clone)]
pub(super) struct Note {
    pub id: NoteId,
    pub origin: Origin,
    pub title: String,
    pub body: String,
    pub urgency: Urgency,
    pub actions: Vec<Action>,
    pub expires_at: Option<Instant>,
    pub place: Place,
    pub popped_at: Instant,
    pub armed_from: Instant,
    pub icon: Option<Icon>,
}

impl Note {
    /// Waits for an answer: a bus caller's notification with actions.
    pub fn is_pending(&self) -> bool {
        matches!(self.origin, Origin::Tetsu { .. }) && !self.actions.is_empty()
    }

    fn source(&self) -> Source {
        match &self.origin {
            Origin::Tetsu { sender, .. } => Source::Tetsu(sender.clone()),
            Origin::Fdo { app_name, .. } => Source::App(app_name.clone()),
        }
    }

    pub fn view(&self) -> NoteView {
        let fdo = matches!(self.origin, Origin::Fdo { .. });
        NoteView {
            id: self.id,
            source: self.source(),
            title: self.title.clone(),
            body: self.body.clone(),
            urgency: self.urgency,
            buttons: self
                .actions
                .iter()
                .filter(|a| !(fdo && a.id == DEFAULT_ACTION))
                .cloned()
                .collect(),
            default_action: fdo && self.actions.iter().any(|a| a.id == DEFAULT_ACTION),
            armed_at: self.armed_from + super::ARM_DELAY,
            icon: self.icon.clone(),
        }
    }

    pub fn history(&self, outcome: Outcome) -> HistoryView {
        HistoryView {
            source: self.source(),
            title: self.title.clone(),
            body: self.body.clone(),
            outcome,
        }
    }
}

/// Pairs a freedesktop action list (key, label, ...); a trailing key without
/// a label is dropped, and so is a repeated key.
pub(super) fn pair_actions(flat: &[String]) -> Vec<Action> {
    flat.as_chunks::<2>()
        .0
        .iter()
        .fold(Vec::new(), |mut actions: Vec<Action>, pair| {
            let [key, label] = pair;
            if !key.is_empty() && !actions.iter().any(|a| &a.id == key) {
                actions.push(Action::new(key, label));
            }
            actions
        })
}
