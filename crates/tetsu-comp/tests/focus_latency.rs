//! Focus-to-bar latency, end to end on the headless compositor.
//!
//! - T0: just before the test writes `focus-next` to the compositor's stdin
//!   hook, which runs the same `run_action` path as Super+J.
//! - T1: `tetsu_bus::window::watch(..).next()` returns the `focused` event
//!   naming the window just focused (the bar's bus-thread receive path).
//!
//! Both are `Instant`s of this process. The bar's redraw is left out: the
//! measurement is about the bus, not the renderer. The test asserts p95 <
//! 50 ms (max < 100 ms as CI tolerance) and prints the report line on
//! every run.
//!
//! ```text
//! cargo test -p tetsu-comp --test focus_latency -- --nocapture
//! # the long run, as a tool (1..=10000 trials, default 60):
//! TETSU_FOCUS_LATENCY_TRIALS=1000 cargo test -p tetsu-comp --release \
//!     --test focus_latency -- --nocapture
//! ```

mod common;
#[path = "common/latency.rs"]
mod latency;
#[path = "common/wayland.rs"]
mod wayland;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::comp::Comp;
use latency::{Budget, Summary, phase_offset, trials_from};
use tetsu_bus::window::{self, WindowKind};
use wayland::WlClient;

const BUDGET: Budget = Budget {
    p95: Duration::from_millis(50),
    max: Duration::from_millis(100),
};
const WARMUP: usize = 3;
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// The bar's receive path: a current-thread runtime following
/// `WatchWindows`. Sends `(focused_id, receive time)` per `focused` event,
/// and a unit once `initial` was seen.
fn subscribe(bus: std::path::PathBuf) -> (mpsc::Receiver<(String, Instant)>, mpsc::Receiver<()>) {
    let (focused_tx, focused_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let mut watch = window::watch(&bus).await.unwrap();
            while let Ok(event) = watch.next().await {
                let at = Instant::now();
                match event.kind {
                    WindowKind::Initial => {
                        let _ = ready_tx.send(());
                    }
                    WindowKind::Focused => {
                        if let Some(Some(id)) = event.focused_id
                            && focused_tx.send((id, at)).is_err()
                        {
                            return;
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    (focused_rx, ready_rx)
}

fn next_focused(rx: &mpsc::Receiver<(String, Instant)>, what: &str) -> (String, Instant) {
    rx.recv_timeout(RECV_TIMEOUT)
        .unwrap_or_else(|e| panic!("no focused event for {what}: {e}"))
}

#[test]
fn focus_reaches_the_bus_client_within_the_budget() {
    let trials = trials_from(std::env::var("TETSU_FOCUS_LATENCY_TRIALS").ok().as_deref())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut comp = Comp::start_hotplug(&[], None);
    let (focused, ready) = subscribe(comp.bus.clone());
    ready.recv_timeout(RECV_TIMEOUT).expect("initial event");

    let mut wl = WlClient::connect(&comp.wayland_socket());
    let _a = wl.map("latency-a", "a");
    let (id_a, _) = next_focused(&focused, "latency-a");
    let _b = wl.map("latency-b", "b");
    let (id_b, _) = next_focused(&focused, "latency-b");
    assert_ne!(id_a, id_b);

    // B holds the focus now; focus-next alternates A, B, A, ...
    let mut samples = Vec::with_capacity(trials);
    for i in 0..WARMUP + trials {
        std::thread::sleep(phase_offset(i));
        wl.sync();
        let t0 = Instant::now();
        comp.control("focus-next");
        let (id, t1) = next_focused(&focused, "focus-next");
        let expected = if i % 2 == 0 { &id_a } else { &id_b };
        assert_eq!(&id, expected, "trial {i}: not the event this trial caused");
        if i >= WARMUP {
            samples.push(t1 - t0);
        }
    }

    let summary = Summary::of(&samples).expect("at least one trial");
    eprintln!("{summary}");
    if let Err(e) = BUDGET.check(&summary) {
        panic!("{e}");
    }
}
