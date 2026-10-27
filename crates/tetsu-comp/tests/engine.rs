//! The layout engine: operations and actions in, contract-ordered events
//! and arrangements out. Every event goes through the contract checks.

mod common;

use tetsu_bus::window::{Placement, WindowEvent, WindowKind};
use tetsu_bus::workspace::{WorkspaceEvent, WorkspaceKind};
use tetsu_comp::bus::Snapshot;
use tetsu_comp::engine::{Action, Engine};
use tetsu_comp::layout::dwindle::dwindle;
use tetsu_comp::layout::floating::{MIN_VISIBLE, centered, clamp_into, from_tile};
use tetsu_comp::layout::master::master;
use tetsu_comp::layout::{Gaps, LayoutKind, Rect};
use tetsu_comp::model::{Events, WindowInfo};
use tetsu_comp::settings::{Settings, parse};

const VIEW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

type Views = Vec<(String, Rect)>;

fn info(app_id: &str) -> WindowInfo {
    WindowInfo {
        app_id: Some(app_id.into()),
        title: Some("t".into()),
        pid: Some(1),
    }
}

fn settings(layout: LayoutKind) -> Settings {
    Settings {
        layout,
        ..Settings::default()
    }
}

fn with_rules(text: &str) -> Settings {
    parse(text).unwrap()
}

/// An engine with one output, flushed.
fn started(settings: Settings) -> (Engine, String, Views) {
    let mut engine = Engine::new(settings);
    let out = engine.add_output("HEADLESS-1");
    engine.flush();
    let views = vec![(out.clone(), VIEW)];
    (engine, out, views)
}

/// Flushes the engine and checks every event and snapshot.
fn checked(engine: &mut Engine) -> Events {
    let contract = common::contract();
    common::check_windows_list(&contract, &engine.model().windows_snapshot());
    common::check_workspaces_list(&contract, &engine.model().workspaces_snapshot());
    let events = engine.flush();
    for event in &events.window {
        common::check_window_event(&contract, event);
    }
    for event in &events.workspace {
        common::check_workspace_event(&contract, event);
    }
    events
}

fn open_n(engine: &mut Engine, views: &Views, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| engine.open(info(&format!("app{i}")), None, views))
        .collect()
}

fn ids(list: &[(String, Rect)]) -> Vec<&str> {
    list.iter().map(|(id, _)| id.as_str()).collect()
}

fn kinds(events: &[WindowEvent]) -> Vec<WindowKind> {
    events.iter().map(|e| e.kind).collect()
}

fn changed_ids(events: &[WindowEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == WindowKind::Changed)
        .map(|e| e.window.clone().unwrap().id)
        .collect()
}

#[test]
fn dwindle_and_master_arrange_like_their_functions() {
    for gaps in [
        Gaps::NONE,
        Gaps {
            outer: 10,
            inner: 20,
        },
    ] {
        for n in 1..=3 {
            let (mut engine, out, views) = started(Settings {
                gaps,
                ..settings(LayoutKind::Dwindle)
            });
            let opened = open_n(&mut engine, &views, n);
            let arrangement = engine.arrange(&out, VIEW);
            let expected = dwindle(VIEW, gaps, n);
            assert_eq!(
                arrangement.tiled,
                opened.into_iter().zip(expected).collect::<Vec<_>>()
            );
            assert!(arrangement.floating.is_empty() && arrangement.hidden.is_empty());

            let (mut engine, out, views) = started(Settings {
                gaps,
                ..settings(LayoutKind::Master)
            });
            let opened = open_n(&mut engine, &views, n);
            let expected = master(VIEW, gaps, n, 0.55);
            assert_eq!(
                engine.arrange(&out, VIEW).tiled,
                opened.into_iter().zip(expected).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn the_floating_layout_floats_every_window_and_publishes_it() {
    let (mut engine, out, views) = started(settings(LayoutKind::Floating));
    let one = engine.open(info("a"), Some((300, 200)), &views);
    let two = engine.open(info("b"), Some((400, 300)), &views);
    let arrangement = engine.arrange(&out, VIEW);
    assert!(arrangement.tiled.is_empty());
    assert_eq!(
        arrangement.floating,
        [
            (one.clone(), centered(VIEW, (300, 200))),
            (two, centered(VIEW, (400, 300)))
        ]
    );
    let events = checked(&mut engine);
    // The placement is in `opened`: no tiled-then-floating flap.
    assert_eq!(
        kinds(&events.window),
        [
            WindowKind::Opened,
            WindowKind::Focused,
            WindowKind::Opened,
            WindowKind::Focused
        ]
    );
    for event in events
        .window
        .iter()
        .filter(|e| e.kind == WindowKind::Opened)
    {
        assert_eq!(
            event.window.as_ref().unwrap().placement,
            Placement::Floating
        );
    }
}

#[test]
fn cycling_the_layout_changes_only_the_windows_whose_placement_flips() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 3);
    // w3 floats on its own.
    engine.apply(Action::ToggleFloating, &views);
    checked(&mut engine);

    // dwindle -> master: nobody flips.
    let effect = engine.apply(Action::CycleLayout, &views);
    assert!(effect.relayout);
    assert_eq!(checked(&mut engine).window, []);
    assert_eq!(
        engine.arrange(&out, VIEW).tiled,
        [
            (w[0].clone(), master(VIEW, Gaps::NONE, 2, 0.55)[0]),
            (w[1].clone(), master(VIEW, Gaps::NONE, 2, 0.55)[1])
        ]
    );

    // master -> floating: w1 and w2 flip, w3 already floats.
    engine.apply(Action::CycleLayout, &views);
    let events = checked(&mut engine);
    assert_eq!(changed_ids(&events.window), [w[0].clone(), w[1].clone()]);
    assert_eq!(events.window.len(), 2);
    assert!(engine.arrange(&out, VIEW).tiled.is_empty());
    assert_eq!(engine.arrange(&out, VIEW).floating.len(), 3);

    // floating -> dwindle: the two flip back.
    engine.apply(Action::CycleLayout, &views);
    let events = checked(&mut engine);
    assert_eq!(changed_ids(&events.window), [w[0].clone(), w[1].clone()]);
    for event in &events.window {
        assert_eq!(event.window.as_ref().unwrap().placement, Placement::Tiled);
    }
    assert_eq!(
        engine.arrange(&out, VIEW).tiled,
        [
            (w[0].clone(), dwindle(VIEW, Gaps::NONE, 2)[0]),
            (w[1].clone(), dwindle(VIEW, Gaps::NONE, 2)[1])
        ]
    );
}

#[test]
fn the_layout_is_per_workspace() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    open_n(&mut engine, &views, 1);
    engine.apply(Action::CycleLayout, &views);
    engine.apply(Action::FocusWorkspace(2), &views);
    let id = engine.open(info("b"), None, &views);
    // Workspace 2 is a dwindle workspace: the window is tiled.
    assert_eq!(engine.arrange(&out, VIEW).tiled, [(id, VIEW)]);
}

#[test]
fn toggling_floating_takes_a_window_out_of_the_tiles_and_back() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 3);
    checked(&mut engine);

    let effect = engine.apply(Action::ToggleFloating, &views);
    assert!(effect.relayout);
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(
        arrangement.tiled,
        [
            (w[0].clone(), dwindle(VIEW, Gaps::NONE, 2)[0]),
            (w[1].clone(), dwindle(VIEW, Gaps::NONE, 2)[1])
        ]
    );
    // It was a 960x540 tile: it floats at that size, centred.
    let tile = dwindle(VIEW, Gaps::NONE, 3)[2];
    assert_eq!(
        arrangement.floating,
        [(w[2].clone(), from_tile(VIEW, tile))]
    );
    let events = checked(&mut engine);
    assert_eq!(changed_ids(&events.window), [w[2].clone()]);
    assert_eq!(
        events.window[0].window.as_ref().unwrap().placement,
        Placement::Floating
    );

    engine.apply(Action::ToggleFloating, &views);
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(
        ids(&arrangement.tiled),
        [w[0].as_str(), w[1].as_str(), w[2].as_str()]
    );
    assert!(arrangement.floating.is_empty());
    let events = checked(&mut engine);
    assert_eq!(
        events.window[0].window.as_ref().unwrap().placement,
        Placement::Tiled
    );
}

#[test]
fn swapping_and_promoting_reorder_the_windows() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 3);
    let order = |engine: &Engine| {
        engine
            .arrange(&out, VIEW)
            .tiled
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&engine), w);

    engine.apply(Action::SwapPrev, &views);
    assert_eq!(order(&engine), [w[0].clone(), w[2].clone(), w[1].clone()]);
    // The focus stays on the moved window.
    assert_eq!(engine.model().focused_window(), Some(w[2].as_str()));
    engine.apply(Action::SwapNext, &views);
    assert_eq!(order(&engine), w);

    let effect = engine.apply(Action::PromoteMaster, &views);
    assert!(effect.relayout);
    assert_eq!(order(&engine), [w[2].clone(), w[0].clone(), w[1].clone()]);
    // Already the master: nothing to do.
    assert!(!engine.apply(Action::PromoteMaster, &views).relayout);
}

#[test]
fn focus_moves_through_the_layout_order_and_wraps() {
    let (mut engine, _out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 3);
    checked(&mut engine);
    engine.apply(Action::FocusNext, &views);
    assert_eq!(engine.model().focused_window(), Some(w[0].as_str()));
    engine.apply(Action::FocusPrev, &views);
    assert_eq!(engine.model().focused_window(), Some(w[2].as_str()));
    engine.apply(Action::FocusPrev, &views);
    assert_eq!(engine.model().focused_window(), Some(w[1].as_str()));
    let events = checked(&mut engine);
    assert_eq!(
        events.window,
        [
            WindowEvent::focused(Some(w[0].clone())),
            WindowEvent::focused(Some(w[2].clone())),
            WindowEvent::focused(Some(w[1].clone())),
        ]
    );
}

#[test]
fn the_master_ratio_steps_and_is_clamped_in_master_only() {
    let (mut engine, out, views) = started(settings(LayoutKind::Master));
    open_n(&mut engine, &views, 2);
    let master_w = |engine: &Engine| engine.arrange(&out, VIEW).tiled[0].1.w;
    assert_eq!(master_w(&engine), 1056);
    assert!(engine.apply(Action::RatioDelta(1), &views).relayout);
    assert_eq!(master_w(&engine), 1152);
    for _ in 0..20 {
        engine.apply(Action::RatioDelta(1), &views);
    }
    assert_eq!(master_w(&engine), 1728);
    assert!(!engine.apply(Action::RatioDelta(1), &views).relayout);
    for _ in 0..40 {
        engine.apply(Action::RatioDelta(-1), &views);
    }
    assert_eq!(master_w(&engine), 192);

    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    open_n(&mut engine, &views, 2);
    let before = engine.arrange(&out, VIEW);
    assert!(!engine.apply(Action::RatioDelta(1), &views).relayout);
    assert_eq!(engine.arrange(&out, VIEW), before);
}

#[test]
fn moving_a_window_to_another_workspace_hides_it_and_retiles_the_source() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 2);
    checked(&mut engine);

    let effect = engine.apply(Action::MoveToWorkspace(2), &views);
    assert!(effect.relayout);
    let events = checked(&mut engine);
    assert_eq!(events.workspace.len(), 1);
    assert_eq!(events.workspace[0].kind, WorkspaceKind::Added);
    let added = events.workspace[0].workspace.clone().unwrap();
    assert_eq!((added.id.as_str(), added.index), ("ws-2", 1));
    assert_eq!(events.window.len(), 2);
    assert_eq!(events.window[0], WindowEvent::focused(Some(w[0].clone())));
    assert_eq!(events.window[1].kind, WindowKind::Changed);
    let moved = events.window[1].window.clone().unwrap();
    assert_eq!(
        (moved.id.as_str(), moved.workspace_id.as_deref()),
        (w[1].as_str(), Some("ws-2"))
    );

    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(arrangement.tiled, [(w[0].clone(), VIEW)]);
    assert_eq!(arrangement.hidden, [w[1].clone()]);

    // Switching shows it and focuses it.
    engine.apply(Action::FocusWorkspace(2), &views);
    let events = checked(&mut engine);
    assert_eq!(
        events.workspace,
        [
            WorkspaceEvent::activated(Some("ws-2".into()), "out-1"),
            WorkspaceEvent::focused(Some("ws-2".into())),
        ]
    );
    assert_eq!(events.window, [WindowEvent::focused(Some(w[1].clone()))]);
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(arrangement.tiled, [(w[1].clone(), VIEW)]);
    assert_eq!(arrangement.hidden, [w[0].clone()]);
}

#[test]
fn moving_to_the_current_workspace_or_with_no_window_does_nothing() {
    let (mut engine, _out, views) = started(settings(LayoutKind::Dwindle));
    assert!(!engine.apply(Action::MoveToWorkspace(2), &views).relayout);
    open_n(&mut engine, &views, 1);
    checked(&mut engine);
    assert!(!engine.apply(Action::MoveToWorkspace(1), &views).relayout);
    assert_eq!(engine.flush(), Events::default());
}

#[test]
fn close_asks_for_the_focused_window_and_forgets_closed_ones() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 2);
    let effect = engine.apply(Action::Close, &views);
    assert_eq!(effect.close.as_deref(), Some(w[1].as_str()));
    // The glue closes when the client confirms.
    engine.close(&w[1]);
    assert_eq!(engine.arrange(&out, VIEW).tiled, [(w[0].clone(), VIEW)]);
    let events = checked(&mut engine);
    assert_eq!(events.window.last().unwrap(), &WindowEvent::closed(&w[1]));
    engine.close(&w[0]);
    assert_eq!(engine.apply(Action::Close, &views).close, None);
}

#[test]
fn a_floating_rule_floats_the_window_at_the_rule_size() {
    let (mut engine, out, views) = started(with_rules(
        "[[window_rule]]\napp_id = \"^pavucontrol$\"\nfloating = true\nsize = [640, 480]\n",
    ));
    let tiled = engine.open(info("foot"), None, &views);
    let plan = engine.plan(Some("pavucontrol"), Some("Volume"), &views);
    assert!(plan.floating);
    assert_eq!(plan.configure_size, Some((640, 480)));
    assert_eq!(plan.output_id, out);
    assert_eq!(plan.workspace_index, 0);

    let id = engine.open(info("pavucontrol"), Some((640, 480)), &views);
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(arrangement.tiled, [(tiled, VIEW)]);
    assert_eq!(arrangement.floating, [(id, centered(VIEW, (640, 480)))]);
    let events = checked(&mut engine);
    let opened = events
        .window
        .iter()
        .rfind(|e| e.kind == WindowKind::Opened)
        .unwrap();
    assert_eq!(
        opened.window.as_ref().unwrap().placement,
        Placement::Floating
    );
}

#[test]
fn a_floating_rule_without_a_size_leaves_it_to_the_client() {
    let (engine, _out, views) = started(with_rules(
        "[[window_rule]]\ntitle = \"Settings\"\nfloating = true\n",
    ));
    let plan = engine.plan(Some("x"), Some("Settings"), &views);
    assert!(plan.floating);
    assert_eq!(plan.configure_size, None);
}

#[test]
fn the_plan_of_a_tiled_window_is_the_tile_it_would_get_appended() {
    let (mut engine, _out, views) = started(settings(LayoutKind::Dwindle));
    open_n(&mut engine, &views, 2);
    let plan = engine.plan(Some("x"), None, &views);
    assert!(!plan.floating);
    let tile = dwindle(VIEW, Gaps::NONE, 3)[2];
    assert_eq!(plan.configure_size, Some((tile.w, tile.h)));
    // In the floating layout the client chooses.
    let (engine, _out, views) = started(settings(LayoutKind::Floating));
    let plan = engine.plan(Some("x"), None, &views);
    assert!(plan.floating);
    assert_eq!(plan.configure_size, None);
}

#[test]
fn planning_changes_nothing() {
    let (mut engine, _out, views) = started(with_rules(
        "[[window_rule]]\napp_id = \"x\"\nworkspace = 4\n",
    ));
    open_n(&mut engine, &views, 1);
    engine.flush();
    let before = Snapshot::of(engine.model());
    let plan = engine.plan(Some("x"), Some("t"), &views);
    assert_eq!(plan.workspace_index, 3);
    assert_eq!(Snapshot::of(engine.model()), before);
    assert_eq!(engine.flush(), Events::default());
}

#[test]
fn a_workspace_rule_opens_the_window_there_without_taking_the_focus() {
    let (mut engine, out, views) = started(with_rules(
        "[[window_rule]]\napp_id = \"^music$\"\nworkspace = 3\n",
    ));
    let first = engine.open(info("foot"), None, &views);
    checked(&mut engine);

    let id = engine.open(info("music"), None, &views);
    let events = checked(&mut engine);
    assert_eq!(events.workspace.len(), 1);
    let added = events.workspace[0].workspace.clone().unwrap();
    assert_eq!((added.id.as_str(), added.index), ("ws-2", 2));
    assert_eq!(kinds(&events.window), [WindowKind::Opened]);
    assert_eq!(
        events.window[0]
            .window
            .as_ref()
            .unwrap()
            .workspace_id
            .as_deref(),
        Some("ws-2")
    );
    assert_eq!(engine.model().focused_window(), Some(first.as_str()));
    assert_eq!(engine.model().focused_workspace(), Some("ws-1"));
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(arrangement.hidden, [id]);
    assert_eq!(arrangement.tiled, [(first, VIEW)]);
}

#[test]
fn an_output_rule_opens_on_the_active_workspace_of_that_output() {
    let mut engine = Engine::new(with_rules(
        "[[window_rule]]\napp_id = \"^side$\"\noutput = \"HEADLESS-2\"\n",
    ));
    let first = engine.add_output("HEADLESS-1");
    let second = engine.add_output("HEADLESS-2");
    engine.flush();
    let views = vec![
        (first.clone(), VIEW),
        (second.clone(), Rect { x: 1920, ..VIEW }),
    ];
    let id = engine.open(info("side"), None, &views);
    assert_eq!(
        engine.model().workspace_of(&id),
        engine.model().active_workspace(&second)
    );
    assert_eq!(engine.model().focused_window(), None);
    let arrangement = engine.arrange(&second, views[1].1);
    assert_eq!(arrangement.tiled, [(id, views[1].1)]);
    checked(&mut engine);
}

#[test]
fn an_unknown_output_falls_back_to_the_focused_one() {
    let (mut engine, out, views) = started(with_rules(
        "[[window_rule]]\napp_id = \"x\"\noutput = \"NOWHERE-9\"\n",
    ));
    let plan = engine.plan(Some("x"), None, &views);
    assert_eq!(plan.output_id, out);
    let id = engine.open(info("x"), None, &views);
    assert_eq!(engine.model().focused_window(), Some(id.as_str()));
}

#[test]
fn moving_a_floating_window_clamps_it_onto_the_screen() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 2);
    assert_eq!(engine.move_floating(&w[0], (10, 10), VIEW), None);
    engine.apply(Action::ToggleFloating, &views);
    let size = engine.arrange(&out, VIEW).floating[0].1;

    let moved = engine.move_floating(&w[1], (100, 50), VIEW).unwrap();
    assert_eq!(
        (moved.x, moved.y, moved.w, moved.h),
        (100, 50, size.w, size.h)
    );
    assert_eq!(engine.arrange(&out, VIEW).floating, [(w[1].clone(), moved)]);

    let moved = engine.move_floating(&w[1], (50_000, 50_000), VIEW).unwrap();
    assert_eq!((moved.x, moved.y), (1920 - MIN_VISIBLE, 1080 - MIN_VISIBLE));
}

#[test]
fn a_floating_window_keeps_its_place_when_it_tiles_and_floats_again() {
    let (mut engine, out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 2);
    engine.apply(Action::ToggleFloating, &views);
    engine.move_floating(&w[1], (123, 77), VIEW);
    engine.apply(Action::ToggleFloating, &views);
    engine.apply(Action::ToggleFloating, &views);
    let rect = engine.arrange(&out, VIEW).floating[0].1;
    assert_eq!((rect.x, rect.y), (123, 77));
}

#[test]
fn floating_rects_follow_their_viewport() {
    let mut engine = Engine::new(settings(LayoutKind::Floating));
    let first = engine.add_output("A");
    let second = engine.add_output("B");
    let at = Rect { x: 1920, ..VIEW };
    let views = vec![(first, VIEW), (second.clone(), at)];
    engine.apply(Action::FocusWorkspace(1), &views);
    let id = engine.open(info("a"), Some((200, 100)), &views);
    let first_id = engine.model().output_name("out-1").map(str::to_owned);
    assert_eq!(first_id.as_deref(), Some("A"));
    assert_eq!(
        engine.arrange("out-1", VIEW).floating,
        [(id, centered(VIEW, (200, 100)))]
    );
    assert!(engine.arrange(&second, at).floating.is_empty());
}

#[test]
fn a_window_a_bind_moved_away_can_no_longer_be_moved() {
    let (mut engine, _out, views) = started(settings(LayoutKind::Dwindle));
    let w = open_n(&mut engine, &views, 2);
    engine.apply(Action::ToggleFloating, &views);
    assert!(engine.move_floating(&w[1], (100, 50), VIEW).is_some());
    engine.apply(Action::MoveToWorkspace(2), &views);
    assert_eq!(engine.move_floating(&w[1], (200, 80), VIEW), None);
}

#[test]
fn toggling_floating_in_the_floating_layout_changes_nothing() {
    let (mut engine, out, views) = started(settings(LayoutKind::Floating));
    let w = open_n(&mut engine, &views, 1);
    let effect = engine.apply(Action::ToggleFloating, &views);
    assert!(!effect.relayout);
    // Back in dwindle the window tiles: no hidden floated flag was set.
    engine.apply(Action::CycleLayout, &views);
    let arrangement = engine.arrange(&out, VIEW);
    assert_eq!(ids(&arrangement.tiled), [w[0].as_str()]);
    assert!(arrangement.floating.is_empty());
}

#[test]
fn floating_rects_are_clamped_into_a_shrunk_viewport() {
    let (mut engine, out, views) = started(settings(LayoutKind::Floating));
    let id = engine.open(info("a"), Some((400, 300)), &views);
    engine.move_floating(&id, (1500, 800), VIEW).unwrap();
    let small = Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    };
    let floating = engine.arrange(&out, small).floating;
    let stored = engine.arrange(&out, VIEW).floating[0].1;
    assert_eq!(floating, [(id, clamp_into(small, stored))]);
    assert_eq!(floating[0].1.x, 800 - MIN_VISIBLE);
    assert_eq!(floating[0].1.y, 600 - MIN_VISIBLE);
}

// ---- unplugging an output ----

/// Two outputs of 1920x1080 side by side, flushed.
fn two_outputs(settings: Settings) -> (Engine, String, String, Views) {
    let mut engine = Engine::new(settings);
    let first = engine.add_output("HEADLESS-1");
    let second = engine.add_output("HEADLESS-2");
    engine.flush();
    let views = vec![
        (first.clone(), VIEW),
        (second.clone(), Rect { x: 1920, ..VIEW }),
    ];
    (engine, first, second, views)
}

#[test]
fn after_an_unplug_the_moved_windows_are_hidden_and_the_target_keeps_its_tiles() {
    let (mut engine, first, second, views) = two_outputs(settings(LayoutKind::Dwindle));
    let on_first = engine.open(info("a"), None, &views);
    engine.focus(Some(&on_first));
    let on_second = engine.open(info("b"), None, &views);
    let before = engine.arrange(&second, views[1].1);
    assert_eq!(ids(&before.tiled), [on_second.as_str()]);

    engine.remove_output(&first);
    let view = Rect { x: 0, ..views[1].1 };
    let after = engine.arrange(&second, view);
    assert_eq!(ids(&after.tiled), [on_second.as_str()]);
    assert_eq!(after.hidden, [on_first.clone()]);
    assert_eq!(after.tiled[0].1, view);
    checked(&mut engine);
}

#[test]
fn showing_a_moved_workspace_tiles_its_windows_in_the_target_viewport() {
    let (mut engine, first, second, views) = two_outputs(settings(LayoutKind::Dwindle));
    let moved = engine.open(info("a"), None, &views);
    engine.remove_output(&first);
    let small = Rect {
        x: 0,
        y: 0,
        w: 1280,
        h: 720,
    };
    let views = vec![(second.clone(), small)];
    // The moved workspace has index 1 on the target: Super+2.
    let effect = engine.apply(Action::FocusWorkspace(2), &views);
    assert!(effect.relayout);
    let arrangement = engine.arrange(&second, small);
    assert_eq!(arrangement.tiled, [(moved, small)]);
    checked(&mut engine);
}

#[test]
fn a_floating_window_from_a_larger_output_is_clamped_into_a_smaller_target() {
    let mut engine = Engine::new(settings(LayoutKind::Floating));
    let big = engine.add_output("DP-1");
    engine.add_output("eDP-1");
    engine.flush();
    let small = Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    };
    let views = vec![(big.clone(), VIEW)];
    let id = engine.open(info("a"), Some((400, 300)), &views);
    engine.move_floating(&id, (1500, 800), VIEW).unwrap();

    engine.remove_output(&big);
    let target = engine.model().output_named("eDP-1").unwrap();
    let views = vec![(target.clone(), small)];
    engine.apply(Action::FocusWorkspace(2), &views);
    let floating = engine.arrange(&target, small).floating;
    assert_eq!(floating.len(), 1);
    let rect = floating[0].1;
    assert!(rect.x < small.w && rect.y < small.h, "{rect:?}");
    assert_eq!(rect, clamp_into(small, rect));
}

#[test]
fn a_rule_naming_a_removed_output_falls_back_to_the_focused_one() {
    let (mut engine, first, second, views) = two_outputs(with_rules(
        "[[window_rule]]\napp_id = \"^side$\"\noutput = \"HEADLESS-1\"\n",
    ));
    let plan = engine.plan(Some("side"), None, &views);
    assert_eq!(plan.output_id, first);
    engine.remove_output(&first);
    let views = vec![(second.clone(), Rect { x: 0, ..VIEW })];
    let plan = engine.plan(Some("side"), None, &views);
    assert_eq!(plan.output_id, second);
    let id = engine.open(info("side"), None, &views);
    assert_eq!(
        engine.model().workspace_of(&id),
        engine.model().active_workspace(&second)
    );
    assert_eq!(engine.model().focused_window(), Some(id.as_str()));
}
