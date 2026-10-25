//! Card layout and hit testing: what a pointer press at a point means.

use std::time::Instant;

use tetsu_bus::notify::{Action, Urgency};
use tetsu_notifyd::core::{NoteId, NoteView, Source};
use tetsu_notifyd::ui::layout::{CARD_WIDTH, Hit, MAX_BUTTONS, Measure, panel, stack};

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
