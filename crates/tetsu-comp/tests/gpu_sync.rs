//! Explicit sync decision and the commit gate.

use tetsu_comp::gpu::sync::{BufferKind, ExplicitSync, Gate, Support, commit_gate, decide, parse_driver_version};
use tetsu_comp::settings::SyncMode;

fn support(eventfd: bool, driver: &str, version: Option<(u32, u32, u32)>) -> Support {
    Support {
        eventfd,
        driver: driver.to_owned(),
        version,
    }
}

fn run(mode: SyncMode, s: Support) -> ExplicitSync {
    decide(mode, s)
}

#[test]
fn versions_parse_with_two_or_three_parts_and_whitespace() {
    assert_eq!(parse_driver_version("560.35.03\n"), Some((560, 35, 3)));
    assert_eq!(parse_driver_version("555.58"), Some((555, 58, 0)));
}

#[test]
fn garbage_versions_are_none() {
    for bad in ["", "\n", "abc", "560.x.1", "1.2.3.4", "-1.2.3"] {
        assert_eq!(parse_driver_version(bad), None, "{bad:?}");
    }
}

#[test]
fn auto_is_on_when_supported() {
    for s in [
        support(true, "i915", None),
        support(true, "nvidia", Some((560, 35, 3))),
        support(true, "nvidia", Some((555, 0, 0))),
    ] {
        let r = run(SyncMode::Auto, s);
        assert!(r.supported && r.enabled, "{r:?}");
        assert_eq!(r.reason, None);
        assert_eq!(r.mode, SyncMode::Auto);
    }
}

#[test]
fn on_is_also_enabled_where_supported() {
    let r = run(SyncMode::On, support(true, "nvidia", Some((560, 35, 3))));
    assert!(r.supported && r.enabled);
}

#[test]
fn nvidia_below_555_is_unsupported_for_auto_and_on() {
    for mode in [SyncMode::Auto, SyncMode::On] {
        let r = run(mode, support(true, "nvidia", Some((550, 1, 0))));
        assert!(!r.supported && !r.enabled);
        assert_eq!(r.reason.as_deref(), Some("nvidia < 555"));
    }
}

#[test]
fn nvidia_with_an_unreadable_version_is_unsupported() {
    let r = run(SyncMode::Auto, support(true, "nvidia", None));
    assert!(!r.supported && !r.enabled);
    assert_eq!(r.reason.as_deref(), Some("nvidia < 555"));
}

#[test]
fn no_eventfd_is_unsupported_and_on_cannot_force_it() {
    for mode in [SyncMode::Auto, SyncMode::On] {
        let r = run(mode, support(false, "i915", None));
        assert!(!r.supported && !r.enabled);
        assert_eq!(r.reason.as_deref(), Some("no syncobj_eventfd"));
    }
}

#[test]
fn off_disables_but_still_reports_support() {
    let r = run(SyncMode::Off, support(true, "i915", None));
    assert!(r.supported);
    assert!(!r.enabled);
    assert_eq!(r.reason.as_deref(), Some("off in config"));
    let r = run(SyncMode::Off, support(false, "i915", None));
    assert!(!r.supported && !r.enabled);
    assert_eq!(r.reason.as_deref(), Some("off in config"));
}

#[test]
fn the_gate_follows_the_acquire_point_then_the_dmabuf_fence() {
    use BufferKind::{Dmabuf, None as NoBuffer, Shm};
    use Gate::{AcquirePoint, DmabufFence, Immediate};
    assert_eq!(commit_gate(true, true, Dmabuf), AcquirePoint);
    assert_eq!(commit_gate(true, true, Shm), AcquirePoint);
    assert_eq!(commit_gate(true, false, Dmabuf), DmabufFence);
    assert_eq!(commit_gate(true, false, Shm), Immediate);
    assert_eq!(commit_gate(true, false, NoBuffer), Immediate);
    assert_eq!(commit_gate(true, true, NoBuffer), Immediate);
    // Syncobj off: the implicit dmabuf fence still applies.
    assert_eq!(commit_gate(false, true, Dmabuf), DmabufFence);
    assert_eq!(commit_gate(false, false, Dmabuf), DmabufFence);
    assert_eq!(commit_gate(false, false, Shm), Immediate);
}
