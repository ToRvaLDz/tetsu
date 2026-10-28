//! The pure planning of `SetRenderDevice` and `SetExplicitSync`: the error
//! order, pending while locked, what is written, and the tty switch rule.

mod common;

use serde_json::json;
use tetsu_comp::gpu::control::{
    Change, DeviceFacts, Plan, RenderCtl, RenderError, Switch, Target, config_value, plan_device,
    plan_sync, tty_switch,
};
use tetsu_comp::gpu::sync::Support;
use tetsu_comp::settings::SyncMode;

fn tty(driver: &str, eventfd: bool, version: Option<(u32, u32, u32)>) -> RenderCtl {
    let mut ctl = RenderCtl::with_devices(
        vec![
            DeviceFacts::new("renderD128", "/dev/dri/renderD128", "i915"),
            DeviceFacts::new("renderD129", "/dev/dri/renderD129", driver),
        ],
        "renderD128",
    );
    ctl.support = Support {
        eventfd,
        driver: driver.to_owned(),
        version,
    };
    ctl.sync_live = false;
    ctl
}

fn headless() -> RenderCtl {
    RenderCtl::synthetic("headless-0", "headless")
}

fn user(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| (*k).to_owned()).collect()
}

fn device(id: &str) -> Target {
    Target::Device(id.to_owned())
}

#[test]
fn every_error_is_a_render_v1_error_that_passes_the_contract() {
    let contract = common::contract();
    let errors = [
        (
            RenderError::DeviceNotFound("x".into()),
            "DeviceNotFound",
            json!({ "id": "x" }),
        ),
        (
            RenderError::UnsupportedByHardware("nvidia < 555".into()),
            "UnsupportedByHardware",
            json!({ "reason": "nvidia < 555" }),
        ),
        (
            RenderError::SwitchFailed("boom".into()),
            "SwitchFailed",
            json!({ "reason": "boom" }),
        ),
        (
            RenderError::OverriddenByUser(vec!["render.device".into()]),
            "OverriddenByUser",
            json!({ "keys": ["render.device"] }),
        ),
        (
            RenderError::StateWriteFailed("disk".into()),
            "StateWriteFailed",
            json!({ "reason": "disk" }),
        ),
    ];
    for (error, name, parameters) in errors {
        let reply = error.reply();
        assert_eq!(reply.error, format!("org.tetsu.comp.render.v1.{name}"));
        assert_eq!(reply.parameters, parameters);
        let found = tetsu_bus_contract::check_error(&contract, &reply.error, &reply.parameters);
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}

#[test]
fn an_unknown_device_comes_before_the_user_file() {
    let ctl = headless();
    let both = plan_device(&ctl, &device("nope"), &user(&["render.device"]), true);
    assert_eq!(both, Err(RenderError::DeviceNotFound("nope".into())));
    let user_only = plan_device(&ctl, &device("headless-0"), &user(&["render.device"]), true);
    assert_eq!(
        user_only,
        Err(RenderError::OverriddenByUser(vec!["render.device".into()]))
    );
    let other_key = plan_device(
        &ctl,
        &device("headless-0"),
        &user(&["render.explicit_sync"]),
        false,
    );
    assert!(other_key.is_ok(), "{other_key:?}");
}

#[test]
fn on_is_refused_where_decide_says_unsupported() {
    for (ctl, reason) in [
        (headless(), "no syncobj_eventfd"),
        (tty("nvidia", true, Some((550, 1, 0))), "nvidia < 555"),
        (tty("i915", false, None), "no syncobj_eventfd"),
    ] {
        let plan = plan_sync(&ctl, SyncMode::On, &user(&["render.explicit_sync"]), true);
        assert_eq!(
            plan,
            Err(RenderError::UnsupportedByHardware(reason.into())),
            "unsupported comes first"
        );
    }
    let supported = tty("nvidia", true, Some((560, 35, 3)));
    let plan = plan_sync(&supported, SyncMode::On, &[], false);
    assert_eq!(
        plan,
        Ok(Plan::Apply {
            change: Change::Sync(SyncMode::On),
            key: "render.explicit_sync",
            value: "on".into(),
        })
    );
}

#[test]
fn auto_and_off_pass_on_any_device_unless_the_user_sets_the_key() {
    for mode in [SyncMode::Auto, SyncMode::Off] {
        assert!(plan_sync(&headless(), mode, &[], false).is_ok());
        assert_eq!(
            plan_sync(&headless(), mode, &user(&["render.explicit_sync"]), false),
            Err(RenderError::OverriddenByUser(vec![
                "render.explicit_sync".into()
            ]))
        );
    }
}

#[test]
fn locked_is_pending_and_writes_nothing() {
    let ctl = tty("i915", true, None);
    assert_eq!(
        plan_device(&ctl, &device("renderD129"), &[], true),
        Ok(Plan::Pending(Change::Device(device("renderD129"))))
    );
    assert_eq!(
        plan_sync(&ctl, SyncMode::Off, &[], true),
        Ok(Plan::Pending(Change::Sync(SyncMode::Off)))
    );
}

#[test]
fn unlocked_writes_the_node_path_or_auto() {
    let ctl = tty("i915", true, None);
    assert_eq!(
        plan_device(&ctl, &device("renderD129"), &[], false),
        Ok(Plan::Apply {
            change: Change::Device(device("renderD129")),
            key: "render.device",
            value: "/dev/dri/renderD129".into(),
        })
    );
    assert_eq!(
        plan_device(&ctl, &Target::Auto, &[], false),
        Ok(Plan::Apply {
            change: Change::Device(Target::Auto),
            key: "render.device",
            value: "auto".into(),
        })
    );
    assert_eq!(
        config_value(&ctl, &Change::Sync(SyncMode::Auto)),
        "auto".to_owned()
    );
    assert_eq!(Target::of(None), Target::Auto);
    assert_eq!(Target::of(Some("renderD129")), device("renderD129"));
}

#[test]
fn the_tty_applies_only_a_switch_to_the_device_rendering_now() {
    assert_eq!(
        tty_switch("renderD128", &device("renderD128"), "renderD129"),
        Switch::Applied
    );
    assert_eq!(
        tty_switch("renderD128", &Target::Auto, "renderD128"),
        Switch::Applied
    );
    assert_eq!(
        tty_switch("renderD128", &Target::Auto, "renderD129"),
        Switch::NextStart
    );
    assert_eq!(
        tty_switch("renderD128", &device("renderD129"), "renderD128"),
        Switch::NextStart
    );
}

#[test]
fn a_second_pending_call_replaces_the_first() {
    let mut ctl = tty("i915", true, None);
    ctl.hold(Change::Device(device("renderD129")));
    ctl.hold(Change::Sync(SyncMode::Off));
    ctl.hold(Change::Device(Target::Auto));
    assert_eq!(ctl.pending_device, Some(Target::Auto));
    assert_eq!(ctl.pending_sync, Some(SyncMode::Off));
    ctl.hold(Change::Sync(SyncMode::Auto));
    assert_eq!(ctl.pending_sync, Some(SyncMode::Auto));
}

#[test]
fn the_rendering_device_and_the_automatic_choice_cannot_be_removed() {
    let mut ctl = headless();
    assert!(ctl.add(DeviceFacts::new("gpu-1", "/dev/dri/gpu-1", "headless")));
    assert!(!ctl.add(DeviceFacts::new("gpu-1", "/dev/dri/gpu-1", "headless")));
    assert!(ctl.remove("headless-0").is_err());
    assert!(ctl.remove("nope").is_err());
    assert_eq!(ctl.remove("gpu-1"), Ok(()));
    assert_eq!(ctl.ids(), ["headless-0"]);
    ctl.failed("gpu-1");
    ctl.failed("gpu-1");
    assert_eq!(ctl.failures["gpu-1"], 2);
}
