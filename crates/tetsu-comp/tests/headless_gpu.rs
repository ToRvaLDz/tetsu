//! The `gpu` lines of the headless hotplug stream, and the hot switch of
//! the backends without a renderer.

use tetsu_comp::backend::headless_gpu::{GpuCommand, parse, switch};
use tetsu_comp::gpu::control::{DeviceFacts, RenderCtl, Switch, Target};

#[test]
fn add_add_fail_and_remove_parse() {
    assert_eq!(
        parse("gpu add headless-1"),
        Ok(GpuCommand::Add {
            name: "headless-1".into(),
            fail: false,
        })
    );
    assert_eq!(
        parse("gpu add gpu2 fail\n"),
        Ok(GpuCommand::Add {
            name: "gpu2".into(),
            fail: true,
        })
    );
    assert_eq!(
        parse("gpu remove headless-1"),
        Ok(GpuCommand::Remove {
            name: "headless-1".into()
        })
    );
}

#[test]
fn bad_names_extra_words_and_unknown_verbs_are_refused() {
    let long = format!("gpu add {}", "a".repeat(33));
    let at_limit = format!("gpu add {}", "a".repeat(32));
    assert!(parse(&at_limit).is_ok());
    for line in [
        long.as_str(),
        "gpu add Headless",
        "gpu add a/b",
        "gpu add ../x",
        "gpu add a_b",
        "gpu add",
        "gpu add n fail now",
        "gpu add n maybe",
        "gpu remove n m",
        "gpu swap n",
        "gpu",
        "",
    ] {
        assert!(parse(line).is_err(), "{line:?} parsed");
    }
}

#[test]
fn a_switch_is_a_relabel_unless_the_device_fails() {
    let mut ctl = RenderCtl::synthetic("headless-0", "headless");
    ctl.add(DeviceFacts::new("gpu-1", "/dev/dri/gpu-1", "headless"));
    ctl.add(DeviceFacts {
        fails: true,
        ..DeviceFacts::new("gpu-2", "/dev/dri/gpu-2", "headless")
    });
    assert_eq!(
        switch(&ctl, &Target::Device("gpu-1".into())),
        Switch::Applied
    );
    assert_eq!(switch(&ctl, &Target::Auto), Switch::Applied);
    assert!(matches!(
        switch(&ctl, &Target::Device("gpu-2".into())),
        Switch::Failed(_)
    ));
}
