//! The notification state machine, without I/O: time comes in as an
//! `Instant`, everything the outside must do goes out as an [`Effect`].
//!
//! Rules (#23): an action answers only its caller and only from a click in
//! notifyd's own surfaces; nothing is answerable while the session is
//! locked or before the buttons arm; a critical notification never expires
//! and stays pending when it moves into the panel; a caller that goes away
//! withdraws its notification.

mod note;
mod types;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use tetsu_bus::notify::{DismissReason, NotifyReply, ShowParams, Urgency};

use note::{Note, Origin};
pub use types::*;

/// The state of every notification.
#[derive(Debug, Default)]
pub struct Core {
    notes: BTreeMap<NoteId, Note>,
    last_id: u32,
    locked: bool,
    history: VecDeque<HistoryView>,
}

impl Core {
    /// An empty, unlocked state.
    pub fn new() -> Self {
        Self::default()
    }

    /// `Show` from a Varlink caller.
    pub fn show_tetsu(
        &mut self,
        caller: &Caller,
        params: ShowParams,
        oneway: bool,
        now: Instant,
    ) -> Result<(NoteId, Vec<Effect>), ShowError> {
        let actions = params.actions.unwrap_or_default();
        let urgency = params.urgency.unwrap_or(Urgency::Normal);
        validate(&params.title, &actions, urgency, params.timeout_ms)?;
        if caller.agent && !actions.is_empty() {
            return Err(ShowError::PermissionDenied);
        }
        if oneway && !actions.is_empty() {
            return Err(ShowError::InvalidParameter("actions"));
        }
        if oneway && urgency == Urgency::Critical {
            return Err(ShowError::InvalidParameter("urgency"));
        }
        let lifetime = match (urgency, params.timeout_ms) {
            (Urgency::Critical, _) => None,
            (_, Some(ms)) => Some(Duration::from_millis(ms.unsigned_abs())),
            (urgency, None) => Some(default_lifetime(urgency)),
        };
        let id = self.next_id();
        let note = Note {
            id,
            origin: Origin::Tetsu {
                pid: caller.pid,
                sender: caller.sender.clone(),
                oneway,
            },
            title: params.title,
            body: params.body.unwrap_or_default(),
            urgency,
            actions,
            expires_at: lifetime.map(|l| now + l),
            place: Place::Popup,
            popped_at: now,
            armed_from: now,
            icon: None,
            category: None,
        };
        let effects = self.tracking(now, |core| {
            core.notes.insert(id, note);
            core.enforce_popup_limit();
            core.reply(id, NotifyReply::shown(&tetsu_id(id)), false)
        });
        Ok((id, effects))
    }

    /// freedesktop `Notify`. Returns the id the client gets back.
    pub fn show_fdo(&mut self, show: FdoShow, now: Instant) -> (u32, Vec<Effect>) {
        let replaced = NoteId(show.replaces_id);
        let id = match self.notes.get(&replaced) {
            Some(note) if matches!(note.origin, Origin::Fdo { .. }) => replaced,
            _ => self.next_id(),
        };
        let urgency = match show.urgency {
            Urgency::Unknown => Urgency::Normal,
            other => other,
        };
        let lifetime = match (urgency, show.expire_timeout) {
            (Urgency::Critical, _) | (_, 0) => None,
            (_, ms) if ms > 0 => Some(Duration::from_millis(ms.unsigned_abs().into())),
            (urgency, _) => Some(default_lifetime(urgency)),
        };
        let note = Note {
            id,
            origin: Origin::Fdo {
                app_name: show.app_name,
                client: show.client,
            },
            title: show.summary,
            body: show.body,
            urgency,
            actions: note::pair_actions(&show.actions),
            expires_at: lifetime.map(|l| now + l),
            place: Place::Popup,
            popped_at: now,
            armed_from: now,
            icon: show.icon,
            category: show.category,
        };
        let effects = self.tracking(now, |core| {
            core.notes.insert(id, note);
            core.enforce_popup_limit();
            Vec::new()
        });
        (id.0, effects)
    }

    /// The Varlink id of a note.
    pub fn tetsu_id(&self, note: NoteId) -> String {
        tetsu_id(note)
    }

    /// The note behind a freedesktop id, while it is open.
    pub fn fdo_note(&self, id: u32) -> Option<NoteId> {
        self.notes
            .get(&NoteId(id))
            .filter(|n| matches!(n.origin, Origin::Fdo { .. }))
            .map(|n| n.id)
    }

    /// Whether a note is still open.
    pub fn is_open(&self, note: NoteId) -> bool {
        self.notes.contains_key(&note)
    }

    /// Where a note is drawn.
    pub fn place(&self, note: NoteId) -> Option<Place> {
        self.notes.get(&note).map(|n| n.place)
    }

    /// Notifications with actions from bus callers, waiting for an answer.
    pub fn pending_count(&self) -> i64 {
        self.notes.values().filter(|n| n.is_pending()).count() as i64
    }

    /// The user clicked an action in notifyd's own surface.
    pub fn click(
        &mut self,
        note: NoteId,
        action: &str,
        now: Instant,
    ) -> Result<Vec<Effect>, Refused> {
        let n = self.answerable(note)?;
        if now < n.armed_from + ARM_DELAY {
            return Err(Refused::NotArmed);
        }
        let label = n
            .actions
            .iter()
            .find(|a| a.id == action)
            .map(|a| a.label.clone())
            .ok_or(Refused::NoSuchAction)?;
        let action = action.to_owned();
        Ok(self.tracking(now, |core| {
            let mut effects = match &core.notes[&note].origin {
                Origin::Fdo { client, .. } => vec![Effect::FdoActionInvoked {
                    id: note.0,
                    action: action.clone(),
                    client: client.clone(),
                }],
                Origin::Tetsu { .. } => Vec::new(),
            };
            effects.extend(core.end(
                note,
                NotifyReply::invoked(&action),
                2,
                Outcome::Invoked(label),
            ));
            effects
        }))
    }

    /// The user dismissed a note: for an approval, that is Deny.
    pub fn dismiss(&mut self, note: NoteId, now: Instant) -> Result<Vec<Effect>, Refused> {
        self.answerable(note)?;
        Ok(self.tracking(now, |core| {
            core.end(
                note,
                NotifyReply::dismissed(DismissReason::User),
                2,
                Outcome::Dismissed,
            )
        }))
    }

    /// `Close` from the process that showed the note.
    pub fn close_tetsu(
        &mut self,
        pid: i32,
        id: &str,
        now: Instant,
    ) -> Result<Vec<Effect>, ShowError> {
        let note = parse_tetsu_id(id)
            .filter(|note| {
                self.notes
                    .get(note)
                    .is_some_and(|n| matches!(n.origin, Origin::Tetsu { pid: p, .. } if p == pid))
            })
            .ok_or(ShowError::NotFound)?;
        Ok(self.tracking(now, |core| {
            core.end(
                note,
                NotifyReply::dismissed(DismissReason::Closed),
                3,
                Outcome::Closed,
            )
        }))
    }

    /// freedesktop `CloseNotification`.
    pub fn close_fdo(&mut self, id: u32, now: Instant) -> Vec<Effect> {
        match self.fdo_note(id) {
            Some(note) => self.tracking(now, |core| {
                core.end(
                    note,
                    NotifyReply::dismissed(DismissReason::Closed),
                    3,
                    Outcome::Closed,
                )
            }),
            None => Vec::new(),
        }
    }

    /// The caller of a streaming `Show` disconnected: withdraw, no reply.
    pub fn caller_gone(&mut self, note: NoteId, now: Instant) -> Vec<Effect> {
        let streaming = self
            .notes
            .get(&note)
            .is_some_and(|n| matches!(n.origin, Origin::Tetsu { oneway: false, .. }));
        if !streaming {
            return Vec::new();
        }
        self.tracking(now, |core| {
            if let Some(n) = core.notes.remove(&note) {
                core.remember(&n, Outcome::Withdrawn);
            }
            Vec::new()
        })
    }

    /// Moves a popup into the panel; it stays open, and pending if it was.
    pub fn collapse(&mut self, note: NoteId, now: Instant) -> Vec<Effect> {
        self.tracking(now, |core| {
            if let Some(n) = core.notes.get_mut(&note) {
                n.place = Place::Panel;
            }
            Vec::new()
        })
    }

    /// The panel opened: everything it shows arms from now.
    pub fn panel_opened(&mut self, now: Instant) {
        for n in self
            .notes
            .values_mut()
            .filter(|n| n.place == Place::Panel || n.is_pending())
        {
            n.armed_from = now;
        }
    }

    /// The session lock changed. On unlock, pending approvals pop up again
    /// and everything arms from now, since nothing was on screen.
    pub fn set_locked(&mut self, locked: bool, now: Instant) -> Vec<Effect> {
        let unlocking = self.locked && !locked;
        self.locked = locked;
        if !unlocking {
            return Vec::new();
        }
        self.tracking(now, |core| {
            for n in core.notes.values_mut() {
                n.armed_from = now;
                if n.is_pending() {
                    n.place = Place::Popup;
                    n.popped_at = now;
                }
            }
            core.enforce_popup_limit();
            Vec::new()
        })
    }

    /// Time passed: expire what has a lifetime, collapse what has none.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let expired: Vec<NoteId> = self
            .notes
            .values()
            .filter(|n| n.expires_at.is_some_and(|at| now >= at))
            .map(|n| n.id)
            .collect();
        self.tracking(now, |core| {
            for n in core.notes.values_mut() {
                if n.place == Place::Popup
                    && n.expires_at.is_none()
                    && now >= n.popped_at + COLLAPSE_AFTER
                {
                    n.place = Place::Panel;
                }
            }
            expired
                .into_iter()
                .flat_map(|note| {
                    core.end(
                        note,
                        NotifyReply::dismissed(DismissReason::Expired),
                        1,
                        Outcome::Expired,
                    )
                })
                .collect()
        })
    }

    /// What the surfaces draw now.
    pub fn view(&self, _now: Instant) -> View {
        let history = self.history.iter().rev().cloned().collect();
        if self.locked {
            return View {
                locked: true,
                history,
                ..View::default()
            };
        }
        let mut popups: Vec<&Note> = self
            .notes
            .values()
            .filter(|n| n.place == Place::Popup)
            .collect();
        popups.sort_by_key(|n| (n.popped_at, n.id));
        View {
            locked: false,
            popups: popups.into_iter().map(Note::view).collect(),
            pending: self
                .notes
                .values()
                .filter(|n| n.is_pending())
                .map(Note::view)
                .collect(),
            open: self
                .notes
                .values()
                .filter(|n| n.place == Place::Panel && !n.is_pending())
                .map(Note::view)
                .collect(),
            history,
        }
    }

    fn next_id(&mut self) -> NoteId {
        loop {
            self.last_id = self.last_id.checked_add(1).unwrap_or(1);
            let id = NoteId(self.last_id);
            if !self.notes.contains_key(&id) {
                return id;
            }
        }
    }

    fn answerable(&self, note: NoteId) -> Result<&Note, Refused> {
        if self.locked {
            return Err(Refused::Locked);
        }
        self.notes.get(&note).ok_or(Refused::Gone)
    }

    /// Runs `f`, then re-arms every note whose card moved (so an armed
    /// button never slides under the pointer) and adds a `PendingChanged`
    /// when the count moved.
    fn tracking(&mut self, now: Instant, f: impl FnOnce(&mut Self) -> Vec<Effect>) -> Vec<Effect> {
        let before = self.pending_count();
        let slots = self.slots();
        let mut effects = f(self);
        let moved: Vec<NoteId> = self
            .slots()
            .into_iter()
            .filter(|slot| !slots.contains(slot))
            .map(|(_, _, id)| id)
            .collect();
        for id in moved {
            if let Some(n) = self.notes.get_mut(&id) {
                n.armed_from = now;
            }
        }
        let after = self.pending_count();
        if after != before {
            effects.push(Effect::PendingChanged(after));
        }
        effects
    }

    /// Where each card is drawn: (popup stack or panel, index, note).
    fn slots(&self) -> Vec<(bool, usize, NoteId)> {
        let view = self.view(Instant::now());
        let popups = view.popups.iter().enumerate().map(|(i, n)| (true, i, n.id));
        let panel = view
            .pending
            .iter()
            .chain(&view.open)
            .enumerate()
            .map(|(i, n)| (false, i, n.id));
        popups.chain(panel).collect()
    }

    /// Removes a note, answering its caller: `reply` on a Varlink stream,
    /// `fdo_reason` in `NotificationClosed`.
    fn end(
        &mut self,
        note: NoteId,
        reply: NotifyReply,
        fdo_reason: u32,
        outcome: Outcome,
    ) -> Vec<Effect> {
        let effects = match self.notes.get(&note).map(|n| &n.origin) {
            Some(Origin::Tetsu { .. }) => self.reply(note, reply, true),
            Some(Origin::Fdo { client, .. }) => vec![Effect::FdoClosed {
                id: note.0,
                reason: fdo_reason,
                client: client.clone(),
            }],
            None => return Vec::new(),
        };
        if let Some(n) = self.notes.remove(&note) {
            self.remember(&n, outcome);
        }
        effects
    }

    /// A reply on a note's stream; nothing for a oneway note.
    fn reply(&self, note: NoteId, reply: NotifyReply, last: bool) -> Vec<Effect> {
        match self.notes.get(&note).map(|n| &n.origin) {
            Some(Origin::Tetsu { oneway: false, .. }) => vec![Effect::Reply { note, reply, last }],
            _ => Vec::new(),
        }
    }

    fn remember(&mut self, note: &Note, outcome: Outcome) {
        self.history.push_back(note.history(outcome));
        while self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
        }
    }

    fn enforce_popup_limit(&mut self) {
        let mut popups: Vec<(Instant, NoteId)> = self
            .notes
            .values()
            .filter(|n| n.place == Place::Popup)
            .map(|n| (n.popped_at, n.id))
            .collect();
        popups.sort();
        let excess = popups.len().saturating_sub(POPUP_LIMIT);
        for (_, id) in popups.into_iter().take(excess) {
            if let Some(n) = self.notes.get_mut(&id) {
                n.place = Place::Panel;
            }
        }
    }
}

fn validate(
    title: &str,
    actions: &[tetsu_bus::notify::Action],
    urgency: Urgency,
    timeout_ms: Option<i64>,
) -> Result<(), ShowError> {
    if title.trim().is_empty() {
        return Err(ShowError::InvalidParameter("title"));
    }
    if urgency == Urgency::Unknown {
        return Err(ShowError::InvalidParameter("urgency"));
    }
    if timeout_ms.is_some_and(|ms| ms <= 0) {
        return Err(ShowError::InvalidParameter("timeout_ms"));
    }
    let mut ids: Vec<&str> = actions.iter().map(|a| a.id.as_str()).collect();
    ids.sort_unstable();
    let duplicated = ids.windows(2).any(|w| w[0] == w[1]);
    if duplicated || actions.iter().any(|a| a.id.is_empty()) {
        return Err(ShowError::InvalidParameter("actions"));
    }
    Ok(())
}

fn default_lifetime(urgency: Urgency) -> Duration {
    match urgency {
        Urgency::Low => LOW_TIMEOUT,
        _ => NORMAL_TIMEOUT,
    }
}

fn tetsu_id(note: NoteId) -> String {
    format!("n-{}", note.0)
}

fn parse_tetsu_id(id: &str) -> Option<NoteId> {
    id.strip_prefix("n-")?.parse().ok().map(NoteId)
}
