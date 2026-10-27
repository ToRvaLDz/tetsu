//! The model applying a target state to its outputs: contract-ordered
//! output and workspace events, disable and enable as unplug and plug.

mod common;

use tetsu_bus::output::{Mode, Output, OutputEvent, OutputKind, Transform};
use tetsu_bus::window::WindowEvent;
use tetsu_bus::workspace::{Workspace, WorkspaceEvent, WorkspaceKind};
use tetsu_comp::model::{Events, Model, OutputInfo, WindowInfo};
use tetsu_comp::outputs::configure::Target;

fn mode(w: i64, h: i64, preferred: bool) -> Mode {
    Mode {
        width: w,
        height: h,
        refresh_mhz: 60000,
        preferred,
    }
}

fn info(name: &str) -> OutputInfo {
    OutputInfo {
        modes: vec![mode(1920, 1080, true), mode(1280, 720, false)],
        ..OutputInfo::named(name)
    }
}

fn window(title: &str) -> WindowInfo {
    WindowInfo {
        app_id: Some("app".into()),
        title: Some(title.into()),
        pid: Some(100),
    }
}

/// out-1 has ws-1 (active, focused, win-1) and ws-3; out-2 has ws-2 (win-2).
fn two() -> Model {
    let mut model = Model::default();
    model.add_output_info(info("HEADLESS-1"));
    model.add_output_info(info("HEADLESS-2"));
    assert_eq!(model.workspace_at("out-1", 1).as_deref(), Some("ws-3"));
    let one = model.open_on(window("one"), "ws-1");
    model.focus(Some(&one));
    model.open_on(window("two"), "ws-2");
    model.flush();
    model
}

fn check_all(model: &mut Model) -> Events {
    let contract = common::contract();
    common::check_outputs_list(&contract, &model.outputs_snapshot());
    common::check_workspaces_list(&contract, &model.workspaces_snapshot());
    let events = model.flush();
    for event in &events.output {
        common::check_output_event(&contract, event);
    }
    for event in &events.workspace {
        common::check_workspace_event(&contract, event);
    }
    for event in &events.window {
        common::check_window_event(&contract, event);
    }
    events
}

fn target(id: &str) -> Target {
    Target {
        id: id.into(),
        enabled: true,
        mode: Some(0),
        scale: 1.0,
        transform: Transform::Normal,
        pin: None,
    }
}

fn output(model: &Model, id: &str) -> Output {
    let list = model.outputs_snapshot();
    list.outputs.into_iter().find(|o| o.id == id).unwrap()
}

fn changed<'a>(events: &'a [OutputEvent], id: &str) -> Vec<&'a Output> {
    events
        .iter()
        .filter(|e| e.kind == OutputKind::Changed)
        .filter_map(|e| e.output.as_ref())
        .filter(|o| o.id == id)
        .collect()
}

#[test]
fn a_scale_and_a_pin_give_one_changed_per_touched_output_with_its_full_state() {
    let mut model = two();
    model.configure_outputs(&[
        target("out-1"),
        Target {
            scale: 2.0,
            pin: Some((0, 1080)),
            ..target("out-2")
        },
    ]);
    let events = check_all(&mut model);
    let second = changed(&events.output, "out-2");
    assert_eq!(second.len(), 1);
    assert_eq!((second[0].scale, second[0].x, second[0].y), (2.0, 0, 1080));
    // The automatic output moved to the right of the pinned one (960 wide).
    let first = changed(&events.output, "out-1");
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].x, first[0].y), (960, 0));
    assert_eq!(events.output.len(), 2);
    assert_eq!(model.output_layout().len(), 2);
}

#[test]
fn an_untouched_output_gives_no_event() {
    let mut model = two();
    model.configure_outputs(&[target("out-1"), target("out-2")]);
    let events = check_all(&mut model);
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_mode_change_resizes_the_output() {
    let mut model = two();
    model.configure_outputs(&[
        Target {
            mode: Some(1),
            ..target("out-1")
        },
        target("out-2"),
    ]);
    let events = check_all(&mut model);
    let first = changed(&events.output, "out-1");
    assert_eq!(first[0].current_mode, Some(1));
    // out-2 slid left: out-1 is 1280 wide now.
    assert_eq!(output(&model, "out-2").x, 1280);
}

#[test]
fn disabling_an_output_unplugs_it_but_keeps_it_listed() {
    let mut model = two();
    model.configure_outputs(&[
        Target {
            enabled: false,
            ..target("out-1")
        },
        target("out-2"),
    ]);
    let events = check_all(&mut model);
    let first = changed(&events.output, "out-1");
    assert_eq!(first.len(), 1);
    assert!(!first[0].enabled);
    assert_eq!(first[0].current_mode, None);
    assert_eq!(model.outputs_snapshot().outputs.len(), 2);
    assert!(events.output.iter().all(|e| e.kind == OutputKind::Changed));
    let ws = |id: &str, index| {
        WorkspaceEvent::changed(Workspace {
            id: id.into(),
            name: None,
            output_id: Some("out-2".into()),
            index,
        })
    };
    assert_eq!(
        events.workspace,
        [
            ws("ws-1", 1),
            ws("ws-3", 2),
            WorkspaceEvent::activated(None, "out-1"),
            WorkspaceEvent::focused(Some("ws-2".into())),
        ]
    );
    assert_eq!(events.window, [WindowEvent::focused(Some("win-2".into()))]);
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Removed)
    );
    assert_eq!(model.output_layout().len(), 1);
    assert_eq!(model.output_of("ws-1"), Some("out-2"));
}

#[test]
fn enabling_it_again_brings_back_the_mode_and_the_workspaces() {
    let mut model = two();
    model.configure_outputs(&[
        Target {
            mode: Some(1),
            ..target("out-1")
        },
        target("out-2"),
    ]);
    model.configure_outputs(&[
        Target {
            enabled: false,
            ..target("out-1")
        },
        target("out-2"),
    ]);
    model.flush();
    model.configure_outputs(&[
        Target {
            mode: None,
            ..target("out-1")
        },
        target("out-2"),
    ]);
    let events = check_all(&mut model);
    let first = changed(&events.output, "out-1");
    assert!(first[0].enabled);
    assert_eq!(first[0].current_mode, Some(1), "the remembered mode");
    assert_eq!(model.output_of("ws-1"), Some("out-1"));
    assert_eq!(model.workspace_index("ws-1"), Some(0));
    assert_eq!(model.output_of("ws-3"), Some("out-1"));
    assert_eq!(model.workspace_index("ws-3"), Some(1));
    assert_eq!(model.active_workspace("out-1"), Some("ws-1"));
    assert!(
        events
            .workspace
            .contains(&WorkspaceEvent::activated(Some("ws-1".into()), "out-1"))
    );
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Removed)
    );
}

#[test]
fn unplugging_the_last_enabled_output_enables_a_disabled_one() {
    let mut model = two();
    model.configure_outputs(&[
        target("out-1"),
        Target {
            enabled: false,
            ..target("out-2")
        },
    ]);
    model.flush();
    model.remove_output("out-1");
    let events = check_all(&mut model);
    let second = output(&model, "out-2");
    assert!(second.enabled);
    assert!(second.current_mode.is_some());
    assert_eq!(model.output_of("ws-1"), Some("out-2"));
    assert_eq!(model.output_of("ws-2"), Some("out-2"));
    assert!(
        events
            .workspace
            .iter()
            .all(|e| e.kind != WorkspaceKind::Removed)
    );
    assert_eq!(model.output_layout().len(), 1);
}

#[test]
fn a_disabled_or_pinned_output_is_honoured_when_it_is_added() {
    let mut model = Model::default();
    model.add_output_info(info("HEADLESS-1"));
    let id = model.add_output_info(OutputInfo {
        enabled: false,
        ..info("HEADLESS-2")
    });
    let dark = output(&model, &id);
    assert!(!dark.enabled);
    assert_eq!(dark.current_mode, None);
    assert!(model.workspaces_of(&id).is_empty());
    let id = model.add_output_info(OutputInfo {
        position: Some((100, 50)),
        ..info("HEADLESS-3")
    });
    let pinned = output(&model, &id);
    assert_eq!((pinned.x, pinned.y), (100, 50));
    check_all(&mut model);
}

#[test]
fn the_only_output_is_enabled_whatever_it_says() {
    let mut model = Model::default();
    let id = model.add_output_info(OutputInfo {
        enabled: false,
        ..info("HEADLESS-1")
    });
    assert!(output(&model, &id).enabled);
    check_all(&mut model);
}

#[test]
fn unknown_ids_in_the_targets_are_ignored() {
    let mut model = two();
    model.configure_outputs(&[target("out-9"), target("out-1"), target("out-2")]);
    assert!(check_all(&mut model).is_empty());
}
