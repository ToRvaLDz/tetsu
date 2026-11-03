//! `ext-idle-notify-v1` on the whole compositor, headless: the global is
//! advertised, notifications go idle after their timeout, activity (the
//! hotplug `activity` command) resumes and re-arms them, for both kinds
//! and for every client.

mod common;
#[path = "common/idle.rs"]
mod idle;
#[path = "common/wayland.rs"]
mod wayland;

use std::time::Duration;

use common::comp::Comp;
use idle::{IdleClient, Seen};

const SHORT_MS: u32 = 200;

#[test]
fn the_compositor_advertises_the_idle_notifier() {
    let comp = Comp::start();
    let globals = wayland::globals(&comp.wayland_socket());
    assert!(
        globals.iter().any(|g| g == "ext_idle_notifier_v1"),
        "{globals:?}"
    );
}

#[test]
fn a_notification_idles_resumes_on_activity_and_idles_again() {
    let mut comp = Comp::start_hotplug(&[], None);
    let mut client = IdleClient::connect(&comp.wayland_socket());
    let note = client.notification(SHORT_MS);
    client.wait_for(note, &[Seen::Idled]);
    comp.control("activity");
    client.wait_for(note, &[Seen::Idled, Seen::Resumed]);
    client.wait_for(note, &[Seen::Idled, Seen::Resumed, Seen::Idled]);
}

#[test]
fn an_input_notification_idles_resumes_on_activity_and_idles_again() {
    let mut comp = Comp::start_hotplug(&[], None);
    let mut client = IdleClient::connect(&comp.wayland_socket());
    let note = client.input_notification(SHORT_MS);
    client.wait_for(note, &[Seen::Idled]);
    comp.control("activity");
    client.wait_for(note, &[Seen::Idled, Seen::Resumed]);
    client.wait_for(note, &[Seen::Idled, Seen::Resumed, Seen::Idled]);
}

#[test]
fn a_long_notification_stays_quiet_across_activity() {
    let mut comp = Comp::start_hotplug(&[], None);
    let mut client = IdleClient::connect(&comp.wayland_socket());
    let note = client.notification(60_000);
    client.settle(Duration::from_millis(250));
    comp.control("activity");
    client.settle(Duration::from_millis(250));
    assert_eq!(client.events(note), []);
}

#[test]
fn every_client_is_notified() {
    let mut comp = Comp::start_hotplug(&[], None);
    let mut first = IdleClient::connect(&comp.wayland_socket());
    let mut second = IdleClient::connect(&comp.wayland_socket());
    let a = first.notification(SHORT_MS);
    let b = second.notification(SHORT_MS);
    first.wait_for(a, &[Seen::Idled]);
    second.wait_for(b, &[Seen::Idled]);
    comp.control("activity");
    first.wait_for(a, &[Seen::Idled, Seen::Resumed]);
    second.wait_for(b, &[Seen::Idled, Seen::Resumed]);
}
