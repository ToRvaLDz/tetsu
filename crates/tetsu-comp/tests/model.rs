//! The window and workspace model: operations in, contract-ordered events
//! and snapshots out.

mod common;

use tetsu_bus::window::{Placement, WindowEvent, WindowKind};
use tetsu_bus::workspace::{WorkspaceEvent, WorkspaceKind};
use tetsu_comp::model::{Events, Model, WindowInfo, WindowUpdate};

fn info(app_id: &str, title: &str) -> WindowInfo {
    WindowInfo {
        app_id: Some(app_id.into()),
        title: Some(title.into()),
        pid: Some(100),
    }
}

fn kinds(events: &[WindowEvent]) -> Vec<WindowKind> {
    events.iter().map(|e| e.kind).collect()
}

/// A model with one output, flushed.
fn started() -> (Model, String) {
    let mut model = Model::default();
    let output = model.add_output("HEADLESS-1");
    model.flush();
    (model, output)
}

fn check_all(model: &mut Model) -> Events {
    let contract = common::contract();
    common::check_windows_list(&contract, &model.windows_snapshot());
    common::check_workspaces_list(&contract, &model.workspaces_snapshot());
    let events = model.flush();
    for event in &events.window {
        common::check_window_event(&contract, event);
    }
    for event in &events.workspace {
        common::check_workspace_event(&contract, event);
    }
    events
}

#[test]
fn an_output_gets_an_active_focused_workspace_added_first() {
    let mut model = Model::default();
    let output = model.add_output("HEADLESS-1");
    assert_eq!(output, "out-1");
    let events = check_all(&mut model);
    let workspace_kinds: Vec<_> = events.workspace.iter().map(|e| e.kind).collect();
    assert_eq!(
        workspace_kinds,
        [
            WorkspaceKind::Added,
            WorkspaceKind::Activated,
            WorkspaceKind::Focused
        ]
    );
    let added = events.workspace[0].workspace.clone().unwrap();
    assert_eq!(added.id, "ws-1");
    assert_eq!(added.name, None);
    assert_eq!(added.index, 0);
    assert_eq!(added.output_id.as_deref(), Some("out-1"));
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.active["out-1"], "ws-1");
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-1"));
    assert_eq!(
        events.workspace[1],
        WorkspaceEvent::activated(Some("ws-1".into()), "out-1")
    );
}

#[test]
fn a_second_output_does_not_take_the_focus() {
    let (mut model, _) = started();
    assert_eq!(model.add_output("HEADLESS-2"), "out-2");
    let events = check_all(&mut model);
    let workspace_kinds: Vec<_> = events.workspace.iter().map(|e| e.kind).collect();
    assert_eq!(
        workspace_kinds,
        [WorkspaceKind::Added, WorkspaceKind::Activated]
    );
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-1"));
    assert_eq!(snapshot.active.len(), 2);
}

#[test]
fn a_new_window_is_tiled_on_the_focused_workspace_and_opens_before_it_is_focused() {
    let (mut model, _) = started();
    let id = model.open(info("foot", "one"));
    model.focus(Some(&id));
    let events = check_all(&mut model);
    assert_eq!(
        kinds(&events.window),
        [WindowKind::Opened, WindowKind::Focused]
    );
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.id, id);
    assert_eq!(window.placement, Placement::Tiled);
    assert!(!window.fullscreen && !window.xwayland);
    assert_eq!(window.pid, Some(100));
    assert_eq!(window.workspace_id.as_deref(), Some("ws-1"));
    assert_eq!(events.window[1], WindowEvent::focused(Some(id.clone())));
    let list = model.windows_snapshot();
    assert_eq!(list.windows.len(), 1);
    assert_eq!(list.focused_id, Some(id));
}

#[test]
fn ids_are_never_reused() {
    let (mut model, _) = started();
    let first = model.open(info("a", "a"));
    model.close(&first);
    let second = model.open(info("a", "a"));
    assert_ne!(first, second);
    assert_eq!(first, "win-1");
    assert_eq!(second, "win-2");
}

#[test]
fn closing_the_focused_window_focuses_the_next_in_the_stack_before_closed() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    model.focus(Some(&one));
    let two = model.open(info("a", "two"));
    model.focus(Some(&two));
    let three = model.open(info("a", "three"));
    model.focus(Some(&three));
    model.flush();

    model.close(&three);
    let events = check_all(&mut model);
    assert_eq!(
        events.window,
        [
            WindowEvent::focused(Some(two.clone())),
            WindowEvent::closed(&three)
        ]
    );

    model.close(&two);
    model.close(&one);
    let events = check_all(&mut model);
    assert_eq!(
        events.window,
        [
            WindowEvent::focused(Some(one.clone())),
            WindowEvent::closed(&two),
            WindowEvent::focused(None),
            WindowEvent::closed(&one),
        ]
    );
    assert_eq!(model.windows_snapshot().focused_id, None);
}

#[test]
fn closing_an_unfocused_window_leaves_the_focus_alone() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    let two = model.open(info("a", "two"));
    model.focus(Some(&two));
    model.flush();
    model.close(&one);
    let events = check_all(&mut model);
    assert_eq!(events.window, [WindowEvent::closed(&one)]);
}

#[test]
fn focusing_the_focused_window_again_emits_nothing() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    model.focus(Some(&one));
    model.flush();
    model.focus(Some(&one));
    assert_eq!(model.flush().window, []);
}

#[test]
fn updates_coalesce_into_one_changed_per_window() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    let two = model.open(info("b", "two"));
    model.flush();

    model.update(&one, WindowUpdate::title("1"));
    model.update(&one, WindowUpdate::title("11"));
    model.update(&two, WindowUpdate::app_id("c"));
    model.update(&one, WindowUpdate::app_id("z"));
    let events = check_all(&mut model);
    assert_eq!(kinds(&events.window), [WindowKind::Changed; 2]);
    let first = events.window[0].window.clone().unwrap();
    assert_eq!(first.id, one);
    assert_eq!(first.title.as_deref(), Some("11"));
    assert_eq!(first.app_id.as_deref(), Some("z"));
    let second = events.window[1].window.clone().unwrap();
    assert_eq!(second.id, two);
    assert_eq!(second.app_id.as_deref(), Some("c"));
    assert_eq!(model.flush().window, []);
}

#[test]
fn an_update_that_changes_nothing_emits_nothing() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    model.flush();
    model.update(&one, WindowUpdate::title("one"));
    assert_eq!(model.flush().window, []);
}

#[test]
fn a_window_closed_before_the_flush_gets_no_changed() {
    let (mut model, _) = started();
    let one = model.open(info("a", "one"));
    model.flush();
    model.update(&one, WindowUpdate::title("late"));
    model.close(&one);
    let events = check_all(&mut model);
    assert_eq!(events.window, [WindowEvent::closed(&one)]);
}

#[test]
fn unknown_windows_are_ignored() {
    let (mut model, _) = started();
    model.update("win-9", WindowUpdate::title("x"));
    model.close("win-9");
    model.focus(Some("win-9"));
    let events = model.flush();
    assert_eq!(events.window, []);
    assert_eq!(events.workspace, []);
}

#[test]
fn a_window_can_be_opened_with_unknown_fields() {
    let (mut model, _) = started();
    let id = model.open(WindowInfo {
        app_id: None,
        title: None,
        pid: None,
    });
    let events = check_all(&mut model);
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.id, id);
    assert_eq!(
        (window.app_id, window.title, window.pid),
        (None, None, None)
    );
}

#[test]
fn without_an_output_a_window_has_no_workspace() {
    let mut model = Model::default();
    let id = model.open(info("a", "one"));
    let events = check_all(&mut model);
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.id, id);
    assert_eq!(window.workspace_id, None);
}

#[test]
fn focusing_a_window_on_another_output_focuses_its_workspace() {
    let (mut model, _) = started();
    let first = model.open(info("a", "one"));
    model.focus(Some(&first));
    model.add_output("HEADLESS-2");
    model.focus_workspace("ws-2");
    let second = model.open(info("b", "two"));
    model.focus(Some(&second));
    model.flush();
    model.focus(Some(&first));
    let events = check_all(&mut model);
    assert_eq!(
        events.workspace,
        [WorkspaceEvent::focused(Some("ws-1".into()))]
    );
    assert_eq!(events.window, [WindowEvent::focused(Some(first))]);
}

/// Two windows on ws-1, the second focused; ws-2 exists. Flushed.
fn two_windows_and_a_spare_workspace() -> (Model, String, String, String) {
    let (mut model, output) = started();
    let one = model.open(info("a", "one"));
    model.focus(Some(&one));
    let two = model.open(info("a", "two"));
    model.focus(Some(&two));
    let spare = model.workspace_at(&output, 1).unwrap();
    model.flush();
    (model, one, two, spare)
}

#[test]
fn a_workspace_is_created_on_demand_with_added_only() {
    let (mut model, output) = started();
    let ws = model.workspace_at(&output, 1).unwrap();
    assert_eq!(ws, "ws-2");
    let events = check_all(&mut model);
    assert_eq!(events.workspace.len(), 1);
    assert_eq!(events.workspace[0].kind, WorkspaceKind::Added);
    let added = events.workspace[0].workspace.clone().unwrap();
    assert_eq!(
        (added.id.as_str(), added.index, added.output_id.as_deref()),
        ("ws-2", 1, Some("out-1"))
    );
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.active["out-1"], "ws-1");
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-1"));
}

#[test]
fn asking_for_a_workspace_again_is_idempotent() {
    let (mut model, output) = started();
    let first = model.workspace_at(&output, 2).unwrap();
    model.flush();
    assert_eq!(model.workspace_at(&output, 2).as_deref(), Some(&*first));
    assert_eq!(model.workspace_at(&output, 0).as_deref(), Some("ws-1"));
    assert_eq!(model.flush().workspace, []);
    assert_eq!(model.workspace_at("out-9", 1), None);
}

#[test]
fn added_precedes_activated_when_the_new_workspace_is_focused() {
    let (mut model, output) = started();
    let ws = model.workspace_at(&output, 1).unwrap();
    model.focus_workspace(&ws);
    let events = check_all(&mut model);
    let kinds: Vec<_> = events.workspace.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            WorkspaceKind::Added,
            WorkspaceKind::Activated,
            WorkspaceKind::Focused
        ]
    );
}

#[test]
fn a_window_can_open_on_another_workspace() {
    let (mut model, output) = started();
    let ws = model.workspace_at(&output, 1).unwrap();
    let id = model.open_on(info("a", "one"), &ws);
    let events = check_all(&mut model);
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.id, id);
    assert_eq!(window.workspace_id.as_deref(), Some(&*ws));
    assert_eq!(model.windows_on(&ws), [id]);
}

#[test]
fn moving_a_window_emits_one_changed_with_the_new_workspace() {
    let (mut model, one, _two, spare) = two_windows_and_a_spare_workspace();
    model.move_to_workspace(&one, &spare);
    let events = check_all(&mut model);
    assert_eq!(kinds(&events.window), [WindowKind::Changed]);
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.id, one);
    assert_eq!(window.workspace_id.as_deref(), Some(&*spare));
    assert_eq!(model.workspace_of(&one), Some(&*spare));
}

#[test]
fn moving_the_focused_window_focuses_the_next_one_on_the_old_workspace() {
    let (mut model, one, two, spare) = two_windows_and_a_spare_workspace();
    model.move_to_workspace(&two, &spare);
    let events = check_all(&mut model);
    assert_eq!(events.window.len(), 2);
    assert_eq!(events.window[0], WindowEvent::focused(Some(one.clone())));
    assert_eq!(events.window[1].kind, WindowKind::Changed);
    assert_eq!(model.focused_window(), Some(&*one));

    // The last window on a workspace leaves: nothing is focused.
    model.move_to_workspace(&one, &spare);
    let events = check_all(&mut model);
    assert_eq!(events.window[0], WindowEvent::focused(None));
    assert_eq!(model.focused_window(), None);
}

#[test]
fn moving_a_window_where_it_already_is_emits_nothing() {
    let (mut model, one, _two, _spare) = two_windows_and_a_spare_workspace();
    model.move_to_workspace(&one, "ws-1");
    model.move_to_workspace(&one, "ws-9");
    model.move_to_workspace("win-9", "ws-1");
    assert_eq!(model.flush().window, []);
}

#[test]
fn a_placement_change_emits_one_changed_and_a_no_op_nothing() {
    let (mut model, one, _two, _spare) = two_windows_and_a_spare_workspace();
    model.set_placement(&one, Placement::Tiled);
    assert_eq!(model.flush().window, []);
    model.set_placement(&one, Placement::Floating);
    model.update(&one, WindowUpdate::title("late"));
    let events = check_all(&mut model);
    assert_eq!(kinds(&events.window), [WindowKind::Changed]);
    let window = events.window[0].window.clone().unwrap();
    assert_eq!(window.placement, Placement::Floating);
    assert_eq!(window.title.as_deref(), Some("late"));
}

#[test]
fn switching_workspace_activates_focuses_it_and_the_window_on_top() {
    let (mut model, one, two, spare) = two_windows_and_a_spare_workspace();
    model.move_to_workspace(&two, &spare);
    model.flush();

    model.focus_workspace(&spare);
    let events = check_all(&mut model);
    assert_eq!(
        events.workspace,
        [
            WorkspaceEvent::activated(Some(spare.clone()), "out-1"),
            WorkspaceEvent::focused(Some(spare.clone())),
        ]
    );
    assert_eq!(events.window, [WindowEvent::focused(Some(two.clone()))]);

    // Back: the most recently focused window of ws-1 gets the focus.
    model.focus_workspace("ws-1");
    let events = check_all(&mut model);
    assert_eq!(events.window, [WindowEvent::focused(Some(one))]);
}

#[test]
fn switching_to_an_empty_workspace_focuses_no_window() {
    let (mut model, _one, two, spare) = two_windows_and_a_spare_workspace();
    assert_eq!(model.focused_window(), Some(&*two));
    model.focus_workspace(&spare);
    let events = check_all(&mut model);
    assert_eq!(events.window, [WindowEvent::focused(None)]);
    assert_eq!(model.windows_snapshot().focused_id, None);
}

#[test]
fn switching_to_the_focused_workspace_again_emits_nothing() {
    let (mut model, _one, _two, _spare) = two_windows_and_a_spare_workspace();
    model.focus_workspace("ws-1");
    let events = model.flush();
    assert_eq!(events.window, []);
    assert_eq!(events.workspace, []);
}
