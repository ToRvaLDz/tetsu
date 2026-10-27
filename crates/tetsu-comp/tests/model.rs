//! The window and workspace model: operations in, contract-ordered events
//! and snapshots out.

mod common;

use tetsu_bus::output::{Mode, OutputEvent, OutputKind, Transform};
use tetsu_bus::window::{Placement, WindowEvent, WindowKind};
use tetsu_bus::workspace::{WorkspaceEvent, WorkspaceKind};
use tetsu_comp::layout::Rect;
use tetsu_comp::model::{Events, Model, OutputInfo, WindowInfo, WindowUpdate};

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
    common::check_outputs_list(&contract, &model.outputs_snapshot());
    let events = model.flush();
    for event in &events.output {
        common::check_output_event(&contract, event);
    }
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

#[test]
fn a_placement_set_before_the_first_flush_is_in_the_opened_event() {
    let (mut model, _) = started();
    let id = model.open(info("a", "one"));
    model.set_placement(&id, Placement::Floating);
    let events = check_all(&mut model);
    assert_eq!(kinds(&events.window), [WindowKind::Opened]);
    assert_eq!(
        events.window[0].window.as_ref().unwrap().placement,
        Placement::Floating
    );
}

// ---- outputs: hotplug, layout and the migration of workspaces ----

fn mode(w: i64, h: i64) -> Mode {
    Mode {
        width: w,
        height: h,
        refresh_mhz: 60000,
        preferred: true,
    }
}

fn output_info(name: &str, w: i64, h: i64) -> OutputInfo {
    OutputInfo {
        name: name.into(),
        make: Some("GSM".into()),
        model: Some("LG HDR 4K".into()),
        serial: None,
        modes: vec![mode(w, h)],
        current_mode: Some(0),
        scale: 1.0,
        transform: Transform::Normal,
    }
}

fn output_kinds(events: &[OutputEvent]) -> Vec<OutputKind> {
    events.iter().map(|e| e.kind).collect()
}

fn position(model: &Model, id: &str) -> (i64, i64) {
    let list = model.outputs_snapshot();
    let output = list.outputs.iter().find(|o| o.id == id).unwrap();
    (output.x, output.y)
}

fn ws_changed(events: &[WorkspaceEvent]) -> Vec<(String, Option<String>, i64)> {
    events
        .iter()
        .filter(|e| e.kind == WorkspaceKind::Changed)
        .map(|e| {
            let w = e.workspace.clone().unwrap();
            (w.id, w.output_id, w.index)
        })
        .collect()
}

fn no_workspace_removed(events: &Events) {
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Removed),
        "{:?}",
        events.workspace
    );
}

/// Two outputs. out-1 has ws-1 (active, focused, with win-1) and ws-3;
/// out-2 has ws-2 (with win-2).
fn two_outputs() -> Model {
    let mut model = Model::default();
    model.add_output_info(output_info("HEADLESS-1", 1920, 1080));
    model.add_output_info(output_info("HEADLESS-2", 1920, 1080));
    let third = model.workspace_at("out-1", 1).unwrap();
    assert_eq!(third, "ws-3");
    let one = model.open_on(info("a", "one"), "ws-1");
    model.focus(Some(&one));
    let two = model.open_on(info("b", "two"), "ws-2");
    assert_eq!((one.as_str(), two.as_str()), ("win-1", "win-2"));
    model.flush();
    model
}

#[test]
fn an_added_output_carries_its_state_and_a_packed_position() {
    let mut model = Model::default();
    let id = model.add_output_info(OutputInfo {
        make: Some("GSM".into()),
        model: Some("LG HDR 4K".into()),
        serial: Some("0x0004a1b2".into()),
        scale: 2.0,
        ..output_info("DP-1", 3840, 2160)
    });
    assert_eq!(id, "out-1");
    let second = model.add_output_info(output_info("DP-2", 1280, 720));
    assert_eq!(second, "out-2");
    let events = check_all(&mut model);
    assert_eq!(
        output_kinds(&events.output),
        [OutputKind::Added, OutputKind::Added]
    );
    let first = events.output[0].output.clone().unwrap();
    assert_eq!((first.id.as_str(), first.name.as_str()), ("out-1", "DP-1"));
    assert_eq!(first.make.as_deref(), Some("GSM"));
    assert_eq!(first.serial.as_deref(), Some("0x0004a1b2"));
    assert_eq!((first.x, first.y), (0, 0));
    assert_eq!(first.current_mode, Some(0));
    assert!(first.enabled);
    // The second sits right of the first's logical width (3840 / 2).
    let second = events.output[1].output.clone().unwrap();
    assert_eq!((second.x, second.y), (1920, 0));
    assert_eq!(
        model.output_layout(),
        [
            (
                "out-1".to_owned(),
                Rect {
                    x: 0,
                    y: 0,
                    w: 1920,
                    h: 1080
                }
            ),
            (
                "out-2".to_owned(),
                Rect {
                    x: 1920,
                    y: 0,
                    w: 1280,
                    h: 720
                }
            ),
        ]
    );
}

#[test]
fn add_output_is_a_one_mode_shortcut() {
    let (model, _) = started();
    let list = model.outputs_snapshot();
    assert_eq!(list.outputs.len(), 1);
    assert_eq!(list.outputs[0].modes, [mode(1920, 1080)]);
    assert_eq!(list.outputs[0].make, None);
}

#[test]
fn removing_the_left_output_removes_it_and_slides_the_next_one() {
    let mut model = two_outputs();
    assert_eq!(position(&model, "out-2"), (1920, 0));
    model.remove_output("out-1");
    let events = check_all(&mut model);
    assert_eq!(
        events.output.iter().map(|e| e.kind).collect::<Vec<_>>(),
        [OutputKind::Removed, OutputKind::Changed]
    );
    assert_eq!(events.output[0], OutputEvent::removed("out-1"));
    let slid = events.output[1].output.clone().unwrap();
    assert_eq!((slid.id.as_str(), slid.x), ("out-2", 0));
    assert_eq!(model.outputs_snapshot().outputs.len(), 1);
}

#[test]
fn an_unplugged_output_hands_its_workspaces_to_the_first_remaining_one() {
    let mut model = two_outputs();
    model.remove_output("out-1");
    let events = check_all(&mut model);
    no_workspace_removed(&events);
    assert_eq!(
        events.workspace,
        [
            WorkspaceEvent::changed(tetsu_bus::workspace::Workspace {
                id: "ws-1".into(),
                name: None,
                output_id: Some("out-2".into()),
                index: 1
            }),
            WorkspaceEvent::changed(tetsu_bus::workspace::Workspace {
                id: "ws-3".into(),
                name: None,
                output_id: Some("out-2".into()),
                index: 2
            }),
            WorkspaceEvent::activated(None, "out-1"),
            WorkspaceEvent::focused(Some("ws-2".into())),
        ]
    );
    // The focus follows to the top window of the target's workspace.
    assert_eq!(events.window, [WindowEvent::focused(Some("win-2".into()))]);
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.active.len(), 1);
    assert_eq!(snapshot.active["out-2"], "ws-2");
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-2"));
    // The moved workspace keeps its window.
    assert_eq!(model.windows_on("ws-1"), ["win-1"]);
    assert_eq!(model.output_of("ws-1"), Some("out-2"));
}

#[test]
fn removing_an_output_without_the_focus_leaves_the_focus_alone() {
    let mut model = two_outputs();
    model.remove_output("out-2");
    let events = check_all(&mut model);
    assert_eq!(
        ws_changed(&events.workspace),
        [("ws-2".to_owned(), Some("out-1".to_owned()), 2)]
    );
    assert!(
        events
            .workspace
            .contains(&WorkspaceEvent::activated(None, "out-2"))
    );
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Focused)
    );
    assert_eq!(events.window, []);
}

#[test]
fn an_output_with_the_same_name_gets_its_workspaces_back() {
    let mut model = two_outputs();
    model.remove_output("out-1");
    model.flush();
    let id = model.add_output_info(output_info("HEADLESS-1", 1920, 1080));
    assert_eq!(id, "out-3", "ids are never reused");
    let events = check_all(&mut model);
    no_workspace_removed(&events);
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Added),
        "{:?}",
        events.workspace
    );
    assert_eq!(
        ws_changed(&events.workspace),
        [
            ("ws-1".to_owned(), Some("out-3".to_owned()), 0),
            ("ws-3".to_owned(), Some("out-3".to_owned()), 1),
        ]
    );
    assert!(
        events
            .workspace
            .contains(&WorkspaceEvent::activated(Some("ws-1".into()), "out-3"))
    );
    // The focus stays where it went.
    assert_eq!(
        model.workspaces_snapshot().focused_id.as_deref(),
        Some("ws-2")
    );
    assert_eq!(model.workspaces_snapshot().active["out-3"], "ws-1");
    assert_eq!(model.active_workspace("out-2"), Some("ws-2"));
}

#[test]
fn a_returned_workspace_that_was_active_elsewhere_hands_the_active_back() {
    let mut model = two_outputs();
    model.remove_output("out-1");
    // The user shows the moved ws-1 on out-2, and focuses it.
    model.focus_workspace("ws-1");
    model.flush();
    model.add_output_info(output_info("HEADLESS-1", 1920, 1080));
    let events = check_all(&mut model);
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.active["out-3"], "ws-1");
    assert_eq!(snapshot.active["out-2"], "ws-2");
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-1"));
    assert!(
        events
            .workspace
            .contains(&WorkspaceEvent::activated(Some("ws-2".into()), "out-2"))
    );
}

#[test]
fn an_output_with_a_new_name_gets_a_fresh_workspace() {
    let mut model = two_outputs();
    model.remove_output("out-1");
    model.flush();
    model.add_output_info(output_info("HDMI-A-1", 1920, 1080));
    let events = check_all(&mut model);
    let kinds: Vec<_> = events.workspace.iter().map(|e| e.kind).collect();
    assert_eq!(kinds, [WorkspaceKind::Added, WorkspaceKind::Activated]);
    let added = events.workspace[0].workspace.clone().unwrap();
    assert_eq!(added.id, "ws-4");
    assert_eq!(added.index, 0);
}

#[test]
fn removing_the_last_output_orphans_the_workspaces_and_the_next_one_adopts_them() {
    let mut model = two_outputs();
    model.remove_output("out-1");
    model.focus_workspace("ws-1");
    model.flush();
    model.remove_output("out-2");
    let events = check_all(&mut model);
    no_workspace_removed(&events);
    let changed = ws_changed(&events.workspace);
    assert_eq!(changed.len(), 3);
    assert!(changed.iter().all(|(_, output, _)| output.is_none()));
    assert!(events.workspace.contains(&WorkspaceEvent::focused(None)));
    assert_eq!(events.window, [WindowEvent::focused(None)]);
    let snapshot = model.workspaces_snapshot();
    assert_eq!(snapshot.workspaces.len(), 3);
    assert!(snapshot.active.is_empty());
    assert_eq!(snapshot.focused_id, None);
    assert!(model.outputs_snapshot().outputs.is_empty());

    // A new output adopts them; the one that was active is active again.
    let id = model.add_output_info(output_info("DP-1", 1920, 1080));
    let events = check_all(&mut model);
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Added)
    );
    let snapshot = model.workspaces_snapshot();
    assert!(
        snapshot
            .workspaces
            .iter()
            .all(|w| w.output_id.as_deref() == Some(id.as_str()))
    );
    let mut indices: Vec<_> = snapshot.workspaces.iter().map(|w| w.index).collect();
    indices.sort();
    assert_eq!(indices, [0, 1, 2]);
    assert_eq!(snapshot.active[&id], "ws-1");
    assert_eq!(snapshot.focused_id.as_deref(), Some("ws-1"));
    assert_eq!(
        model.windows_snapshot().focused_id.as_deref(),
        Some("win-1")
    );
}

#[test]
fn unknown_ids_are_ignored_and_a_second_removal_is_a_no_op() {
    let mut model = two_outputs();
    model.remove_output("out-9");
    assert!(model.flush().is_empty());
    model.remove_output("out-1");
    model.flush();
    model.remove_output("out-1");
    assert!(model.flush().is_empty());
}

#[test]
fn a_mode_change_emits_changed_and_slides_the_outputs_to_its_right() {
    let mut model = two_outputs();
    model.set_output_mode("out-1", vec![mode(1280, 720), mode(1920, 1080)], Some(0));
    let events = check_all(&mut model);
    let ids: Vec<_> = events
        .output
        .iter()
        .map(|e| e.output.clone().unwrap().id)
        .collect();
    assert_eq!(ids, ["out-1", "out-2"]);
    assert_eq!(position(&model, "out-2"), (1280, 0));
    // The same mode again changes nothing.
    model.set_output_mode("out-1", vec![mode(1280, 720), mode(1920, 1080)], Some(0));
    assert!(model.flush().is_empty());
    model.set_output_mode("out-9", vec![], None);
    assert!(model.flush().is_empty());
}

#[test]
fn a_window_opened_with_no_output_lands_on_the_workspace_of_the_next_one() {
    let mut model = Model::default();
    let first = model.add_output("DP-1");
    model.remove_output(&first);
    model.flush();
    let win = model.open(info("a", "orphan"));
    model.flush();
    assert_eq!(model.workspace_of(&win), None);

    model.add_output("DP-2");
    let events = check_all(&mut model);
    let focused = model.focused_workspace().map(str::to_owned);
    assert!(focused.is_some());
    assert_eq!(model.workspace_of(&win), focused.as_deref());
    assert_eq!(model.windows_on(focused.as_deref().unwrap()), [win.clone()]);
    // The window event carries the workspace.
    let on_ws = events
        .window
        .iter()
        .filter_map(|e| e.window.as_ref())
        .any(|w| w.id == win && w.workspace_id == focused);
    assert!(on_ws);
}
