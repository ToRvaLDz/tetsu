//! The workspace mirror: events in, the strip out.

use std::collections::BTreeMap;

use tetsu_bar::model::workspaces::{Mirror, WsCell};
use tetsu_bus::workspace::{Workspace, WorkspaceEvent, WorkspaceKind};

fn ws(id: &str, name: Option<&str>, output: &str, index: i64) -> Workspace {
    Workspace {
        id: id.into(),
        name: name.map(str::to_owned),
        output_id: Some(output.into()),
        index,
    }
}

fn cell(label: &str, active: bool, focused: bool) -> WsCell {
    WsCell {
        label: label.into(),
        active,
        focused,
    }
}

fn initial(
    workspaces: Vec<Workspace>,
    active: &[(&str, &str)],
    focused: Option<&str>,
) -> WorkspaceEvent {
    let active: BTreeMap<String, String> = active
        .iter()
        .map(|(o, w)| ((*o).to_owned(), (*w).to_owned()))
        .collect();
    WorkspaceEvent::initial(workspaces, active, focused.map(str::to_owned))
}

fn after(events: &[WorkspaceEvent]) -> Mirror {
    events
        .iter()
        .fold(Mirror::default(), |mirror, event| mirror.apply(event))
}

fn two_outputs() -> WorkspaceEvent {
    initial(
        vec![
            ws("b2", None, "o2", 1),
            ws("a2", Some("web"), "o1", 1),
            ws("a1", None, "o1", 0),
            ws("b1", None, "o2", 0),
        ],
        &[("o1", "a1"), ("o2", "b2")],
        Some("a1"),
    )
}

#[test]
fn initial_builds_the_strip_ordered_by_output_then_index() {
    let strip = after(&[two_outputs()]).strip();
    assert_eq!(
        strip,
        vec![
            cell("1", true, true),
            cell("web", false, false),
            cell("1", false, false),
            cell("2", true, false),
        ]
    );
}

#[test]
fn an_empty_mirror_has_an_empty_strip() {
    assert_eq!(Mirror::default().strip(), vec![]);
}

#[test]
fn added_inserts_in_order() {
    let mirror = after(&[
        two_outputs(),
        WorkspaceEvent::added(ws("a3", None, "o1", 2)),
    ]);
    let labels: Vec<_> = mirror.strip().into_iter().map(|c| c.label).collect();
    assert_eq!(labels, ["1", "web", "3", "1", "2"]);
}

#[test]
fn changed_replaces_name_and_index() {
    let mirror = after(&[
        two_outputs(),
        WorkspaceEvent::changed(ws("a2", Some("mail"), "o1", 1)),
        WorkspaceEvent::changed(ws("a1", None, "o1", 5)),
    ]);
    let strip = mirror.strip();
    assert_eq!(strip[0], cell("mail", false, false));
    assert_eq!(strip[1], cell("6", true, true));
}

#[test]
fn removed_drops_the_workspace() {
    let mirror = after(&[two_outputs(), WorkspaceEvent::removed("a2")]);
    assert_eq!(mirror.strip().len(), 3);
}

#[test]
fn activated_moves_the_active_mark() {
    let mirror = after(&[
        two_outputs(),
        WorkspaceEvent::activated(Some("a2".into()), "o1"),
    ]);
    let strip = mirror.strip();
    assert_eq!(strip[0], cell("1", false, true));
    assert_eq!(strip[1], cell("web", true, false));
}

#[test]
fn activated_with_null_drops_the_outputs_entry() {
    let mirror = after(&[two_outputs(), WorkspaceEvent::activated(None, "o2")]);
    let strip = mirror.strip();
    assert_eq!(strip[3], cell("2", false, false));
    assert_eq!(strip[0], cell("1", true, true));
}

#[test]
fn focused_moves_and_clears_the_focus() {
    let moved = after(&[two_outputs(), WorkspaceEvent::focused(Some("b2".into()))]);
    assert_eq!(moved.strip()[3], cell("2", true, true));
    assert_eq!(moved.strip()[0], cell("1", true, false));
    let cleared = after(&[two_outputs(), WorkspaceEvent::focused(None)]);
    assert!(cleared.strip().iter().all(|c| !c.focused));
}

#[test]
fn an_unknown_kind_changes_nothing() {
    let before = after(&[two_outputs()]);
    let unknown = WorkspaceEvent {
        kind: WorkspaceKind::Unknown,
        workspaces: None,
        active: None,
        focused_id: None,
        workspace: None,
        id: None,
        output_id: None,
    };
    assert_eq!(before.apply(&unknown), before);
}

#[test]
fn an_event_before_initial_is_ignored() {
    let mirror = after(&[
        WorkspaceEvent::added(ws("a1", None, "o1", 0)),
        WorkspaceEvent::focused(Some("a1".into())),
    ]);
    assert_eq!(mirror.strip(), vec![]);
    assert_eq!(mirror, Mirror::default());
}

#[test]
fn a_second_initial_resets_the_mirror() {
    let mirror = after(&[
        two_outputs(),
        WorkspaceEvent::added(ws("a3", None, "o1", 2)),
        initial(vec![ws("c1", None, "o9", 0)], &[("o9", "c1")], None),
    ]);
    assert_eq!(mirror.strip(), vec![cell("1", true, false)]);
}

#[test]
fn applying_returns_a_new_value_and_leaves_the_old_one() {
    let first = after(&[two_outputs()]);
    let second = first.apply(&WorkspaceEvent::focused(None));
    assert!(first.strip()[0].focused);
    assert!(!second.strip()[0].focused);
}
