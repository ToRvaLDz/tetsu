//! Where each card, button and close box sits, in surface coordinates
//! (logical pixels), and what a pointer press at a point means. No drawing
//! here: text is measured through [`Measure`].

use tetsu_bus::notify::Action;

use crate::core::{NoteId, NoteView, Source};

/// Width of a card and of the popup surface.
pub const CARD_WIDTH: f32 = 380.0;
/// Width of the panel surface.
pub const PANEL_WIDTH: f32 = CARD_WIDTH + 2.0 * MARGIN;
/// Space around and between cards.
pub const MARGIN: f32 = 10.0;
/// Inner padding of a card.
pub const PADDING: f32 = 12.0;
/// The sender line.
pub const SENDER_LINE: f32 = 16.0;
/// The title line.
pub const TITLE_LINE: f32 = 22.0;
/// One body line.
pub const BODY_LINE: f32 = 18.0;
/// Body lines shown at most, but for an ask.
pub const MAX_BODY_LINES: usize = 4;
/// Body lines an ask shows at most in the popup; the panel shows it whole.
pub const MAX_ASK_POPUP_LINES: usize = 20;
/// Label of the button that opens the panel on a cut ask.
pub const OPEN_PANEL_LABEL: &str = "Read all in the panel";
/// Width of that button: the label plus the text padding, with margin.
pub const OPEN_PANEL_WIDTH: f32 = 190.0;
/// Button height.
pub const BUTTON_HEIGHT: f32 = 30.0;
/// Button width.
pub const BUTTON_WIDTH: f32 = 104.0;
/// Buttons drawn at most; the rest of a freedesktop list is not shown.
pub const MAX_BUTTONS: usize = 3;
/// The close box side.
pub const CLOSE_SIZE: f32 = 22.0;
/// Panel header height.
pub const HEADER: f32 = 44.0;
/// One history row.
pub const HISTORY_ROW: f32 = 40.0;

/// The statement drawn under a cut ask, for `hidden` lines not shown.
pub fn hidden_statement(hidden: usize) -> String {
    format!("… {hidden} more lines not shown")
}

/// Measures wrapped text.
pub trait Measure {
    /// How many lines `text` wraps to at `width`.
    fn lines(&mut self, text: &str, width: f32) -> usize;
}

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// Whether the point is inside.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// The middle point.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    fn shifted(self, dy: f32) -> Self {
        Self {
            y: self.y + dy,
            ..self
        }
    }
}

/// What a press means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// An action button.
    Action(NoteId, String),
    /// The close box: dismiss.
    Dismiss(NoteId),
    /// The body of a note with a freedesktop `default` action.
    Default(NoteId),
    /// The panel's close box.
    ClosePanel,
    /// The button of a cut ask: open the panel, which shows it whole.
    OpenPanel,
}

/// One laid out card.
#[derive(Debug, Clone)]
pub struct Card {
    /// The note drawn.
    pub note: NoteView,
    /// The whole card.
    pub rect: Rect,
    /// The close box.
    pub close: Rect,
    /// The text block (sender, title, body).
    pub body: Rect,
    /// Body lines shown.
    pub body_lines: usize,
    /// The buttons drawn, left to right.
    pub buttons: Vec<(Rect, Action)>,
    /// Body lines cut off, counted after the line the statement takes; the
    /// statement is drawn below the body when this is not zero.
    pub hidden_lines: usize,
    /// The button that opens the panel, on a cut ask.
    pub open_panel: Option<Rect>,
}

impl Card {
    /// What a press at the point means on this card.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if !self.rect.contains(x, y) {
            return None;
        }
        if self.close.contains(x, y) {
            return Some(Hit::Dismiss(self.note.id));
        }
        if let Some((_, action)) = self.buttons.iter().find(|(r, _)| r.contains(x, y)) {
            return Some(Hit::Action(self.note.id, action.id.clone()));
        }
        if self.open_panel.is_some_and(|r| r.contains(x, y)) {
            return Some(Hit::OpenPanel);
        }
        (self.note.default_action && self.body.contains(x, y)).then_some(Hit::Default(self.note.id))
    }

    /// Whether this card carries the Tetsu frame.
    pub fn framed(&self) -> bool {
        matches!(self.note.source, Source::Tetsu(_))
    }

    fn shifted(mut self, dy: f32) -> Self {
        self.rect = self.rect.shifted(dy);
        self.close = self.close.shifted(dy);
        self.body = self.body.shifted(dy);
        self.buttons = self
            .buttons
            .into_iter()
            .map(|(r, a)| (r.shifted(dy), a))
            .collect();
        self.open_panel = self.open_panel.map(|r| r.shifted(dy));
        self
    }
}

/// Where a card is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The popup surface: a tall ask is cut.
    Popup,
    /// The panel, which scrolls: an ask is shown whole.
    Panel,
}

/// How many body lines are drawn and how many are cut off, for `measured`
/// lines of body. Only a pending ask (as `Note::is_pending`: a Tetsu note
/// with buttons) is shown past [`MAX_BODY_LINES`]; urgency says nothing.
fn body_split(note: &NoteView, measured: usize, mode: Mode) -> (usize, usize) {
    let ask = matches!(note.source, Source::Tetsu(_)) && !note.buttons.is_empty();
    match (ask, mode) {
        (false, _) => (measured.clamp(1, MAX_BODY_LINES), 0),
        (true, Mode::Panel) => (measured, 0),
        (true, Mode::Popup) if measured <= MAX_ASK_POPUP_LINES => (measured, 0),
        // The statement takes the last of the allowed lines.
        (true, Mode::Popup) => {
            let drawn = MAX_ASK_POPUP_LINES - 1;
            (drawn, measured - drawn)
        }
    }
}

/// Lays out one card with its top-left corner at (`x`, `y`).
fn card(note: &NoteView, x: f32, y: f32, mode: Mode, measure: &mut dyn Measure) -> Card {
    let text_width = CARD_WIDTH - 2.0 * PADDING - CLOSE_SIZE;
    let (body_lines, hidden_lines) = if note.body.is_empty() {
        (0, 0)
    } else {
        body_split(note, measure.lines(&note.body, text_width).max(1), mode)
    };
    let cut = hidden_lines > 0;
    // A cut ask offers no answer: only the way to the panel.
    let shown: Vec<&Action> = if cut {
        Vec::new()
    } else {
        note.buttons.iter().take(MAX_BUTTONS).collect()
    };
    let statement_lines = usize::from(cut);
    let text_height = SENDER_LINE + TITLE_LINE + (body_lines + statement_lines) as f32 * BODY_LINE;
    let buttons_height = if shown.is_empty() && !cut {
        0.0
    } else {
        MARGIN + BUTTON_HEIGHT
    };
    let height = PADDING * 2.0 + text_height + buttons_height;
    let button_y = y + PADDING + text_height + MARGIN;
    let right = x + CARD_WIDTH - PADDING;
    let open_panel = cut.then_some(Rect {
        x: right - OPEN_PANEL_WIDTH,
        y: button_y,
        w: OPEN_PANEL_WIDTH,
        h: BUTTON_HEIGHT,
    });
    let count = shown.len() as f32;
    let buttons = shown
        .into_iter()
        .enumerate()
        .map(|(i, action)| {
            let from_right = count - i as f32;
            let bx = right - from_right * BUTTON_WIDTH - (from_right - 1.0) * MARGIN;
            let rect = Rect {
                x: bx,
                y: button_y,
                w: BUTTON_WIDTH,
                h: BUTTON_HEIGHT,
            };
            (rect, action.clone())
        })
        .collect();
    Card {
        note: note.clone(),
        rect: Rect {
            x,
            y,
            w: CARD_WIDTH,
            h: height,
        },
        close: Rect {
            x: x + CARD_WIDTH - PADDING / 2.0 - CLOSE_SIZE,
            y: y + PADDING / 2.0,
            w: CLOSE_SIZE,
            h: CLOSE_SIZE,
        },
        body: Rect {
            x: x + PADDING,
            y: y + PADDING,
            w: text_width,
            h: text_height,
        },
        body_lines,
        buttons,
        hidden_lines,
        open_panel,
    }
}

/// Stacks cards from the top of the popup surface. Returns the cards and
/// the height the surface needs.
pub fn stack(notes: &[NoteView], measure: &mut dyn Measure) -> (Vec<Card>, f32) {
    stack_in(notes, Mode::Popup, measure)
}

fn stack_in(notes: &[NoteView], mode: Mode, measure: &mut dyn Measure) -> (Vec<Card>, f32) {
    notes
        .iter()
        .fold((Vec::new(), 0.0), |(mut cards, y), note| {
            let laid = card(note, 0.0, y, mode, measure);
            let next = y + laid.rect.h + MARGIN;
            cards.push(laid);
            (cards, next)
        })
}

/// The laid out panel.
#[derive(Debug, Clone)]
pub struct PanelLayout {
    /// The panel's close box, fixed while scrolling.
    pub close: Rect,
    /// Pending approvals, then other open notes.
    pub cards: Vec<Card>,
    /// Where the history rows start.
    pub history_top: f32,
    /// Content height before scrolling.
    pub content_height: f32,
}

impl PanelLayout {
    /// What a press at the point means; content under the header is hidden.
    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.close.contains(x, y) {
            return Some(Hit::ClosePanel);
        }
        if y < HEADER {
            return None;
        }
        self.cards.iter().find_map(|c| c.hit(x, y))
    }
}

/// Lays out the panel: pending approvals on top, then other open notes,
/// then `history` rows; everything below the header scrolls by `scroll`.
pub fn panel(
    pending: &[NoteView],
    open: &[NoteView],
    history: usize,
    _height: f32,
    scroll: f32,
    measure: &mut dyn Measure,
) -> PanelLayout {
    let (stacked, stacked_height) = stack_in(&[pending, open].concat(), Mode::Panel, measure);
    let top = HEADER;
    let cards = stacked
        .into_iter()
        .map(|c| c.shifted(top - scroll).shifted_x(MARGIN))
        .collect();
    let history_top = top + stacked_height + MARGIN - scroll;
    PanelLayout {
        close: Rect {
            x: PANEL_WIDTH - MARGIN - CLOSE_SIZE,
            y: (HEADER - CLOSE_SIZE) / 2.0,
            w: CLOSE_SIZE,
            h: CLOSE_SIZE,
        },
        cards,
        history_top,
        content_height: top + stacked_height + MARGIN + history as f32 * HISTORY_ROW,
    }
}

impl Card {
    fn shifted_x(mut self, dx: f32) -> Self {
        let move_x = |r: Rect| Rect { x: r.x + dx, ..r };
        self.rect = move_x(self.rect);
        self.close = move_x(self.close);
        self.body = move_x(self.body);
        self.buttons = self
            .buttons
            .into_iter()
            .map(|(r, a)| (move_x(r), a))
            .collect();
        self.open_panel = self.open_panel.map(move_x);
        self
    }
}
