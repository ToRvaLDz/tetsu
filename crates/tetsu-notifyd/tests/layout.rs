//! Card layout and hit testing: what a pointer press at a point means.

use std::time::Instant;

use tetsu_bus::notify::{Action, Urgency};
use tetsu_notifyd::core::{NoteId, NoteView, Source};
use tetsu_notifyd::ui::draw::Painter;
use tetsu_notifyd::ui::layout::{
    BODY_LINE, CARD_WIDTH, Hit, MAX_ASK_POPUP_LINES, MAX_BODY_LINES, MAX_BUTTONS, Measure,
    OPEN_PANEL_LABEL, OPEN_PANEL_WIDTH, PADDING, SENDER_LINE, TITLE_LINE, panel, stack,
};

/// One body line per 40 characters.
struct Fixed;

impl Measure for Fixed {
    fn lines(&mut self, text: &str, _width: f32) -> usize {
        text.len().div_ceil(40)
    }
}

fn note(id: u32, body: &str, buttons: &[(&str, &str)]) -> NoteView {
    NoteView {
        id: NoteId(id),
        source: Source::Tetsu("tetsu-agent.service".into()),
        title: format!("Note {id}"),
        body: body.into(),
        urgency: Urgency::Critical,
        buttons: buttons.iter().map(|(i, l)| Action::new(i, l)).collect(),
        default_action: false,
        armed_at: Instant::now(),
        icon: None,
        category: None,
    }
}

#[test]
fn cards_stack_downwards_without_overlap() {
    let notes = [
        note(1, "", &[]),
        note(
            2,
            &"x".repeat(100),
            &[("approve", "Approve"), ("deny", "Deny")],
        ),
    ];
    let (cards, height) = stack(&notes, &mut Fixed);
    assert_eq!(cards.len(), 2);
    assert!(cards[0].rect.y + cards[0].rect.h <= cards[1].rect.y);
    assert!(
        cards[1].rect.h > cards[0].rect.h,
        "body and buttons take room"
    );
    assert!(height >= cards[1].rect.y + cards[1].rect.h);
    assert!(cards.iter().all(|c| c.rect.w <= CARD_WIDTH));
}

#[test]
fn a_press_on_a_button_is_that_action_of_that_note() {
    let notes = [
        note(1, "", &[("a", "A")]),
        note(2, "", &[("approve", "Approve"), ("deny", "Deny")]),
    ];
    let (cards, _) = stack(&notes, &mut Fixed);
    for card in &cards {
        for (rect, action) in &card.buttons {
            let (x, y) = rect.center();
            assert_eq!(
                cards.iter().find_map(|c| c.hit(x, y)),
                Some(Hit::Action(card.note.id, action.id.clone()))
            );
        }
    }
}

#[test]
fn the_close_box_dismisses_and_the_body_is_inert_for_tetsu_notes() {
    let (cards, _) = stack(&[note(7, "body", &[("a", "A")])], &mut Fixed);
    let (x, y) = cards[0].close.center();
    assert_eq!(cards[0].hit(x, y), Some(Hit::Dismiss(NoteId(7))));
    let (x, y) = cards[0].body.center();
    assert_eq!(cards[0].hit(x, y), None);
}

#[test]
fn the_body_invokes_the_default_action_when_there_is_one() {
    let mut fdo = note(3, "body", &[]);
    fdo.source = Source::App("chat".into());
    fdo.default_action = true;
    let (cards, _) = stack(&[fdo], &mut Fixed);
    let (x, y) = cards[0].body.center();
    assert_eq!(cards[0].hit(x, y), Some(Hit::Default(NoteId(3))));
}

#[test]
fn presses_outside_every_card_hit_nothing() {
    let (cards, height) = stack(&[note(1, "", &[("a", "A")])], &mut Fixed);
    assert_eq!(cards[0].hit(-1.0, 5.0), None);
    assert_eq!(cards[0].hit(5.0, height + 50.0), None);
}

#[test]
fn buttons_beyond_the_limit_are_not_drawn() {
    let many: Vec<(String, String)> = (0..6).map(|i| (format!("a{i}"), format!("A{i}"))).collect();
    let refs: Vec<(&str, &str)> = many.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let (cards, _) = stack(&[note(1, "", &refs)], &mut Fixed);
    assert_eq!(cards[0].buttons.len(), MAX_BUTTONS);
}

#[test]
fn the_panel_puts_pending_first_and_has_a_close_box() {
    let pending = [note(1, "", &[("approve", "Approve")])];
    let open = [note(2, "", &[])];
    let layout = panel(&pending, &open, 3, 800.0, 0.0, &mut Fixed);
    assert!(layout.cards[0].note.id == NoteId(1));
    assert!(layout.cards[1].rect.y > layout.cards[0].rect.y);
    let (x, y) = layout.close.center();
    assert_eq!(layout.hit(x, y), Some(Hit::ClosePanel));
    assert!(layout.history_top > layout.cards[1].rect.y);
}

#[test]
fn scrolling_moves_the_panel_content_but_not_its_close_box() {
    let pending = [note(1, "", &[("approve", "Approve")])];
    let still = panel(&pending, &[], 0, 800.0, 0.0, &mut Fixed);
    let scrolled = panel(&pending, &[], 0, 800.0, 30.0, &mut Fixed);
    assert_eq!(scrolled.cards[0].rect.y, still.cards[0].rect.y - 30.0);
    assert_eq!(scrolled.close, still.close);
}

const THREE: [(&str, &str); 3] = [
    ("allow_once", "Allow once"),
    ("always_allow", "Always allow"),
    ("deny", "Deny"),
];

/// A body of `n` lines under the `Fixed` measurer.
fn lines(n: usize) -> String {
    "x".repeat(40 * n)
}

fn ids(card: &tetsu_notifyd::ui::layout::Card) -> Vec<&str> {
    card.buttons.iter().map(|(_, a)| a.id.as_str()).collect()
}

#[test]
fn a_tall_ask_in_the_popup_shows_every_line_with_its_buttons() {
    let (cards, _) = stack(&[note(1, &lines(14), &THREE)], &mut Fixed);
    let card = &cards[0];
    assert_eq!(card.body_lines, 14);
    assert_eq!(card.hidden_lines, 0);
    assert_eq!(card.open_panel, None);
    assert_eq!(ids(card), ["allow_once", "always_allow", "deny"]);
}

#[test]
fn an_ask_at_the_popup_limit_shows_every_line_with_its_buttons() {
    let (cards, _) = stack(&[note(1, &lines(MAX_ASK_POPUP_LINES), &THREE)], &mut Fixed);
    let card = &cards[0];
    assert_eq!(card.body_lines, MAX_ASK_POPUP_LINES);
    assert_eq!(card.hidden_lines, 0);
    assert_eq!(card.open_panel, None);
    assert_eq!(card.buttons.len(), 3);
}

#[test]
fn an_ask_over_the_popup_limit_is_cut_and_offers_only_the_panel() {
    let measured = MAX_ASK_POPUP_LINES + 1;
    let (cards, _) = stack(&[note(1, &lines(measured), &THREE)], &mut Fixed);
    let card = &cards[0];
    // The statement takes one of the limit's lines.
    assert_eq!(card.body_lines, MAX_ASK_POPUP_LINES - 1);
    assert_eq!(card.hidden_lines, measured - card.body_lines);
    assert!(card.buttons.is_empty(), "no answer button is drawn");
    let open = card.open_panel.expect("the open-panel button");
    assert!(open.x >= card.rect.x && open.x + open.w <= card.rect.x + card.rect.w);
    assert!(open.y + open.h <= card.rect.y + card.rect.h);
    let (x, y) = open.center();
    assert_eq!(card.hit(x, y), Some(Hit::OpenPanel));
    // Where the three buttons would be, no answer is hit.
    for i in 0..3 {
        let bx = card.rect.x + card.rect.w - PADDING - (i as f32 + 0.5) * 60.0;
        assert!(!matches!(card.hit(bx, y), Some(Hit::Action(..))));
    }
    // The close box stays.
    let (x, y) = card.close.center();
    assert_eq!(card.hit(x, y), Some(Hit::Dismiss(NoteId(1))));
}

#[test]
fn the_cut_ask_counts_the_hidden_lines_exactly() {
    for measured in [MAX_ASK_POPUP_LINES + 1, 30, 139] {
        let (cards, _) = stack(&[note(1, &lines(measured), &THREE)], &mut Fixed);
        let card = &cards[0];
        // Lines drawn: the text, then the statement line.
        assert_eq!(card.body_lines + 1, MAX_ASK_POPUP_LINES);
        assert_eq!(card.body_lines + card.hidden_lines, measured);
        // The text block holds the statement line too.
        assert_eq!(
            card.body.h,
            SENDER_LINE + TITLE_LINE + MAX_ASK_POPUP_LINES as f32 * BODY_LINE
        );
    }
}

#[test]
fn a_cut_ask_card_is_as_tall_as_its_drawn_lines() {
    let (full, _) = stack(&[note(1, &lines(MAX_ASK_POPUP_LINES), &THREE)], &mut Fixed);
    let (cut, _) = stack(&[note(1, &lines(50), &THREE)], &mut Fixed);
    // Both draw the limit's lines; the cut one swaps the buttons for the
    // wider open-panel button, same height.
    assert_eq!(cut[0].rect.h, full[0].rect.h);
}

#[test]
fn the_panel_never_cuts_an_ask() {
    let ask = note(1, &lines(139), &THREE);
    let layout = panel(&[ask], &[], 0, 800.0, 0.0, &mut Fixed);
    let card = &layout.cards[0];
    assert_eq!(card.body_lines, 139);
    assert_eq!(card.hidden_lines, 0);
    assert_eq!(card.open_panel, None);
    assert_eq!(ids(card), ["allow_once", "always_allow", "deny"]);
    let text_bottom = card.body.y + card.body.h;
    assert!(card.buttons.iter().all(|(r, _)| r.y >= text_bottom));
    assert!(layout.content_height >= card.rect.y + card.rect.h);
}

#[test]
fn the_buttons_of_a_tall_panel_ask_are_reachable_by_scrolling() {
    let view = 800.0;
    let ask = [note(1, &lines(139), &THREE)];
    let content = panel(&ask, &[], 0, view, 0.0, &mut Fixed).content_height;
    assert!(content > view, "the ask needs scrolling");
    // The scroll clamp of the UI: content minus the surface height.
    let layout = panel(&ask, &[], 0, view, content - view, &mut Fixed);
    for (rect, action) in &layout.cards[0].buttons {
        assert!(rect.y + rect.h <= view, "{} is in view", action.id);
        let (x, y) = rect.center();
        assert_eq!(
            layout.hit(x, y),
            Some(Hit::Action(NoteId(1), action.id.clone()))
        );
    }
}

#[test]
fn a_freedesktop_note_with_buttons_stays_at_four_lines_everywhere() {
    let mut fdo = note(5, &lines(30), &THREE);
    fdo.source = Source::App("chat".into());
    let (cards, _) = stack(std::slice::from_ref(&fdo), &mut Fixed);
    let card = &cards[0];
    assert_eq!(fdo.urgency, Urgency::Critical);
    assert_eq!(card.body_lines, MAX_BODY_LINES);
    assert_eq!(card.hidden_lines, 0);
    assert_eq!(card.open_panel, None);
    assert_eq!(card.buttons.len(), 3);
    let layout = panel(&[], &[fdo], 0, 800.0, 0.0, &mut Fixed);
    let card = &layout.cards[0];
    assert_eq!(card.body_lines, MAX_BODY_LINES);
    assert_eq!(card.open_panel, None);
    assert_eq!(card.buttons.len(), 3);
}

#[test]
fn a_tetsu_note_without_buttons_stays_at_four_lines() {
    let (cards, _) = stack(&[note(1, &lines(30), &[])], &mut Fixed);
    assert_eq!(cards[0].body_lines, MAX_BODY_LINES);
    assert_eq!(cards[0].open_panel, None);
    let layout = panel(&[], &[note(1, &lines(30), &[])], 0, 800.0, 0.0, &mut Fixed);
    assert_eq!(layout.cards[0].body_lines, MAX_BODY_LINES);
}

#[test]
fn the_open_panel_label_fits_its_button_with_the_real_font() {
    let mut painter = Painter::new();
    let inner = OPEN_PANEL_WIDTH - 16.0;
    assert_eq!(painter.lines(OPEN_PANEL_LABEL, inner), 1);
    // A body of 14 short lines measures as 14 and is drawn whole.
    let body = (0..14)
        .map(|i| format!("  arg{i}: \"v\""))
        .collect::<Vec<_>>()
        .join("\n");
    let (cards, _) = stack(&[note(1, &body, &THREE)], &mut painter);
    assert_eq!(cards[0].body_lines, 14);
    assert_eq!(cards[0].body.h, 16.0 + 22.0 + 14.0 * BODY_LINE);
}
