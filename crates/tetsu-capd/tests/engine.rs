//! The engine over a tempdir store, a fixed clock, a scripted notify
//! channel and a recording audit sink.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::pending;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tetsu_bus::cap::{
    Capability, CapabilityEvent, CheckParams, CheckPeer, CheckReply, DeniedBy, EventKind, Grant,
    IntentParams, Layer, SetParams, State,
};
use tetsu_bus::notify::{DismissReason, NotifyReply, ShowParams, Urgency};
use tetsu_capd::ask::{NotifyChannel, ShowEvents};
use tetsu_capd::audit::{AuditEntry, Event, RecordingSink};
use tetsu_capd::catalog;
use tetsu_capd::clock::{FixedClock, rfc3339};
use tetsu_capd::engine::{CapError, Engine};
use tetsu_capd::store::{Layers, Store};
use tokio::sync::oneshot;
use tokio::task::LocalSet;

const INSTALL: &str = "org.tetsu.tx.v1.Install";
const REMOVE: &str = "org.tetsu.tx.v1.Remove";
const REVIEW: &str = "org.tetsu.tx.v1.ReviewAurPackages";
const UPDATE: &str = "org.tetsu.tx.v1.Update";
const HOUR: u64 = 3600;

enum Script {
    /// `shown`, then `invoked` with this action.
    Answer(&'static str),
    /// `shown`, then `dismissed`.
    Dismiss(DismissReason),
    /// `shown`, then the stream ends.
    EndsEarly,
    /// The connect fails.
    ConnectFails,
    /// `shown`, then nothing, ever.
    Hang,
    /// `shown`, then whatever the test sends.
    Gate(oneshot::Receiver<NotifyReply>),
}

enum Next {
    Reply(NotifyReply),
    Hang,
    Gate(oneshot::Receiver<NotifyReply>),
}

struct FakeEvents {
    queue: VecDeque<Next>,
    dropped: Rc<Cell<usize>>,
}

impl Drop for FakeEvents {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

impl ShowEvents for FakeEvents {
    async fn next(&mut self) -> io::Result<NotifyReply> {
        match self.queue.pop_front() {
            Some(Next::Reply(reply)) => Ok(reply),
            Some(Next::Hang) => pending().await,
            Some(Next::Gate(rx)) => rx
                .await
                .map_err(|_| io::Error::from(io::ErrorKind::UnexpectedEof)),
            None => Err(io::ErrorKind::UnexpectedEof.into()),
        }
    }
}

#[derive(Default)]
struct Inner {
    scripts: RefCell<VecDeque<Script>>,
    shows: RefCell<Vec<ShowParams>>,
    notices: RefCell<Vec<ShowParams>>,
    dropped: Rc<Cell<usize>>,
}

#[derive(Clone, Default)]
struct Fake(Rc<Inner>);

impl Fake {
    fn script(&self, script: Script) {
        self.0.scripts.borrow_mut().push_back(script);
    }

    fn shows(&self) -> usize {
        self.0.shows.borrow().len()
    }

    fn last_show(&self) -> ShowParams {
        self.0.shows.borrow().last().cloned().expect("a show")
    }
}

impl NotifyChannel for Fake {
    type Events = FakeEvents;

    async fn show(&self, params: ShowParams) -> io::Result<FakeEvents> {
        self.0.shows.borrow_mut().push(params);
        let script = self.0.scripts.borrow_mut().pop_front();
        let shown = || Next::Reply(NotifyReply::shown("n1"));
        let queue: VecDeque<Next> = match script {
            None | Some(Script::ConnectFails) => {
                return Err(io::ErrorKind::ConnectionRefused.into());
            }
            Some(Script::Answer(id)) => [shown(), Next::Reply(NotifyReply::invoked(id))].into(),
            Some(Script::Dismiss(reason)) => {
                [shown(), Next::Reply(NotifyReply::dismissed(reason))].into()
            }
            Some(Script::EndsEarly) => [shown()].into(),
            Some(Script::Hang) => [shown(), Next::Hang].into(),
            Some(Script::Gate(rx)) => [shown(), Next::Gate(rx)].into(),
        };
        Ok(FakeEvents {
            queue,
            dropped: self.0.dropped.clone(),
        })
    }

    async fn notice(&self, params: ShowParams) -> io::Result<()> {
        self.0.notices.borrow_mut().push(params);
        Ok(())
    }
}

type TestEngine = Engine<Fake, FixedClock, Rc<RecordingSink>>;

struct Setup {
    dir: TempDir,
    engine: Rc<TestEngine>,
    notify: Fake,
    clock: FixedClock,
    sink: Rc<RecordingSink>,
}

impl Setup {
    /// An engine whose layers hold these texts (image, managed, user).
    fn with(image: &str, managed: &str, user: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let layers = Layers {
            image: dir.path().join("image/capabilities.toml"),
            managed: dir.path().join("state/capabilities.toml"),
            user: dir.path().join("config/capabilities.toml"),
        };
        for (path, text) in [
            (&layers.image, image),
            (&layers.managed, managed),
            (&layers.user, user),
        ] {
            if !text.is_empty() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        }
        let notify = Fake::default();
        let clock = FixedClock::at(1_000_000);
        let sink = Rc::new(RecordingSink::default());
        let engine = Engine::new(
            notify.clone(),
            clock.clone(),
            sink.clone(),
            Store::new(layers),
            vec![Duration::ZERO; 3],
        );
        Self {
            dir,
            engine: Rc::new(engine),
            notify,
            clock,
            sink,
        }
    }

    fn new() -> Self {
        Self::with("", "", "")
    }

    fn managed_path(&self) -> std::path::PathBuf {
        self.dir.path().join("state/capabilities.toml")
    }

    fn user_path(&self) -> std::path::PathBuf {
        self.dir.path().join("config/capabilities.toml")
    }

    async fn check(&self, method: &str, args: Value) -> Result<CheckReply, CapError> {
        self.engine
            .check("tetsu-tx.service", &params(method, args))
            .await
    }

    fn events(&self) -> Vec<&'static str> {
        self.sink
            .entries
            .borrow()
            .iter()
            .map(|e| e.event.name())
            .collect()
    }

    fn entries(&self) -> Vec<AuditEntry> {
        self.sink.entries.borrow().clone()
    }

    fn cap(&self, class: &str) -> Capability {
        self.engine
            .list()
            .into_iter()
            .find(|c| c.class == class && c.scope.is_none())
            .unwrap_or_else(|| panic!("no capability {class}"))
    }
}

fn params(method: &str, args: Value) -> CheckParams {
    CheckParams {
        peer: CheckPeer {
            pid: 4242,
            uid: 1000,
            unit: "tetsu-agent.service".into(),
        },
        method: method.into(),
        args,
    }
}

fn run<F: Future>(test: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, test)
}

fn allow() -> CheckReply {
    CheckReply::allow()
}

fn deny(by: DeniedBy) -> CheckReply {
    CheckReply::deny(by)
}

fn packages() -> Value {
    json!({ "packages": ["htop"] })
}

fn ids(show: &ShowParams) -> Vec<String> {
    show.actions
        .as_ref()
        .map(|a| a.iter().map(|a| a.id.clone()).collect())
        .unwrap_or_default()
}

// 1. every grant state

#[test]
fn unset_asks_with_three_buttons() {
    run(async {
        let s = Setup::new();
        s.notify.script(Script::Answer("allow_once"));
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 1);
        let show = s.notify.last_show();
        assert_eq!(ids(&show), ["allow_once", "always_allow", "deny"]);
        assert_eq!(show.urgency, Some(Urgency::Critical));
        assert!(show.body.unwrap().contains("packages: [\"htop\"]"));
    });
}

#[test]
fn automatic_allows_without_a_show() {
    run(async {
        let s = Setup::with("[class]\n\"packages.install\" = \"automatic\"\n", "", "");
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 0);
        assert_eq!(s.events(), ["check"]);
    });
}

#[test]
fn notify_allows_and_tells_the_user_once_after_the_window() {
    run(async {
        let s = Setup::with("", "[class]\n\"packages.remove\" = \"notify\"\n", "");
        for _ in 0..2 {
            assert_eq!(s.check(REMOVE, packages()).await.unwrap(), allow());
        }
        assert_eq!(s.notify.shows(), 0);
        s.engine.flush_notices().await;
        assert!(
            s.notify.0.notices.borrow().is_empty(),
            "still inside the window"
        );
        s.clock.advance(Duration::from_secs(10));
        s.engine.flush_notices().await;
        let notices = s.notify.0.notices.borrow().clone();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].title, "The agent used packages.remove 2 times");
        assert_eq!(notices[0].urgency, Some(Urgency::Low));
        assert_eq!(notices[0].actions, None);
        s.engine.flush_notices().await;
        assert_eq!(s.notify.0.notices.borrow().len(), 1);
    });
}

#[test]
fn ask_asks_with_two_buttons() {
    run(async {
        let s = Setup::with("[class]\n\"aur.review\" = \"ask\"\n", "", "");
        s.notify.script(Script::Answer("approve"));
        assert_eq!(
            s.check(REVIEW, json!({ "names": ["x"] })).await.unwrap(),
            allow()
        );
        assert_eq!(ids(&s.notify.last_show()), ["approve", "deny"]);
    });
}

#[test]
fn off_denies_without_a_show() {
    run(async {
        let s = Setup::with("", "", "[class]\n\"packages.install\" = \"off\"\n");
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Off)
        );
        assert_eq!(s.notify.shows(), 0);
        assert_eq!(s.events(), ["check"]);
    });
}

#[test]
fn a_destructive_class_in_automatic_still_asks_and_in_off_refuses() {
    run(async {
        let s = Setup::with("[class]\n\"system.update\" = \"automatic\"\n", "", "");
        s.notify.script(Script::Answer("approve"));
        assert_eq!(s.check(UPDATE, json!({})).await.unwrap(), allow());
        assert_eq!(ids(&s.notify.last_show()), ["approve", "deny"]);

        let s = Setup::with("[class]\n\"system.update\" = \"off\"\n", "", "");
        assert_eq!(
            s.check(UPDATE, json!({})).await.unwrap(),
            deny(DeniedBy::Off)
        );
        assert_eq!(s.notify.shows(), 0);
    });
}

#[test]
fn the_layer_of_a_state_is_reported() {
    run(async {
        let text = "[class]\n\"packages.install\" = \"ask\"\n";
        for (setup, layer) in [
            (Setup::with(text, "", ""), Layer::Image),
            (Setup::with("", text, ""), Layer::Managed),
            (Setup::with("", "", text), Layer::User),
        ] {
            let cap = setup.cap("packages.install");
            assert_eq!((cap.state, cap.layer), (State::Ask, Some(layer)));
        }
        let s = Setup::new();
        let cap = s.cap("packages.install");
        assert_eq!((cap.state, cap.layer), (State::Unset, None));
    });
}

// 2. the ask round-trip

#[test]
fn approve_allows_and_counts_an_approval() {
    run(async {
        let s = Setup::with("[class]\n\"aur.review\" = \"ask\"\n", "", "");
        s.notify.script(Script::Answer("approve"));
        s.check(REVIEW, json!({})).await.unwrap();
        let cap = s.cap("aur.review");
        assert_eq!((cap.asks, cap.approvals, cap.denials), (1, 1, 0));
        assert_eq!(s.events(), ["ask", "approved", "check"]);
    });
}

#[test]
fn deny_suppresses_asks_for_a_day() {
    run(async {
        let s = Setup::new();
        s.notify.script(Script::Answer("deny"));
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::User)
        );
        let cap = s.cap("packages.install");
        assert_eq!((cap.asks, cap.denials), (1, 1));
        let until = s.clock_plus(24 * HOUR);
        assert_eq!(cap.suppressed_until_at.as_deref(), Some(until.as_str()));

        // A second call inside the day is denied without a show.
        s.clock.advance(Duration::from_secs(HOUR));
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Suppressed)
        );
        assert_eq!(s.notify.shows(), 1);
        assert_eq!(s.cap("packages.install").suppressions, 1);
        assert_eq!(
            s.events(),
            ["ask", "denied", "check", "suppressed", "check"]
        );

        // After the day it asks again.
        s.clock.advance(Duration::from_secs(24 * HOUR));
        s.notify.script(Script::Answer("allow_once"));
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 2);
        assert_eq!(s.cap("packages.install").suppressed_until_at, None);
    });
}

impl Setup {
    fn clock_plus(&self, secs: u64) -> String {
        use tetsu_capd::clock::Clock;
        rfc3339(self.clock.now() + Duration::from_secs(secs))
    }
}

#[test]
fn dismissing_by_hand_or_by_expiry_is_a_deny() {
    run(async {
        for reason in [DismissReason::User, DismissReason::Expired] {
            let s = Setup::new();
            s.notify.script(Script::Dismiss(reason));
            assert_eq!(
                s.check(INSTALL, packages()).await.unwrap(),
                deny(DeniedBy::User)
            );
            assert!(s.cap("packages.install").suppressed_until_at.is_some());
        }
    });
}

#[test]
fn a_suppression_does_not_touch_an_automatic_class() {
    run(async {
        let s = Setup::with("[class]\n\"packages.remove\" = \"automatic\"\n", "", "");
        s.notify.script(Script::Answer("deny"));
        s.check(INSTALL, packages()).await.unwrap();
        assert_eq!(s.check(REMOVE, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 1);
    });
}

#[test]
fn always_allow_writes_notify_to_the_managed_layer() {
    run(async {
        let s = Setup::new();
        s.notify.script(Script::Answer("always_allow"));
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        let file = std::fs::read_to_string(s.managed_path()).unwrap();
        assert!(file.contains("\"packages.install\" = \"notify\""), "{file}");
        assert_eq!(s.events(), ["ask", "approved", "set", "check"]);
        let cap = s.cap("packages.install");
        assert_eq!(
            (cap.state, cap.layer),
            (State::Notify, Some(Layer::Managed))
        );
        // The next call allows without asking.
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 1);
    });
}

#[test]
fn a_stream_that_ends_is_shown_again_up_to_three_times() {
    run(async {
        let s = Setup::new();
        for _ in 0..3 {
            s.notify.script(Script::EndsEarly);
        }
        s.notify.script(Script::Answer("allow_once"));
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        assert_eq!(s.notify.shows(), 4);
        assert_eq!(s.cap("packages.install").asks, 1, "one ask, four shows");
    });
}

#[test]
fn four_failures_deny_unavailable_without_a_suppression() {
    run(async {
        let s = Setup::new();
        for _ in 0..4 {
            s.notify.script(Script::EndsEarly);
        }
        s.notify.script(Script::Answer("allow_once"));
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Unavailable)
        );
        assert_eq!(s.notify.shows(), 4);
        let cap = s.cap("packages.install");
        assert_eq!((cap.denials, cap.suppressed_until_at), (0, None));
        assert_eq!(s.events(), ["ask", "denied", "check"]);
        // The user did not deny, so the next call asks again.
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
    });
}

#[test]
fn an_intent_shows_its_reason_until_it_is_five_seconds_old() {
    run(async {
        let s = Setup::new();
        let intent = IntentParams {
            method: INSTALL.into(),
            reason: "to run the tests".into(),
        };
        s.engine.intent(4242, &intent).unwrap();
        s.clock.advance(Duration::from_secs(3));
        s.notify.script(Script::Answer("allow_once"));
        s.check(INSTALL, packages()).await.unwrap();
        let body = s.notify.last_show().body.unwrap();
        assert!(
            body.contains("Reason given by the agent (unverified): \"to run the tests\""),
            "{body}"
        );
        assert_eq!(
            s.entries().iter().find_map(|e| e.reason.clone()).as_deref(),
            Some("to run the tests"),
            "the audit trail keeps it"
        );

        s.engine.intent(4242, &intent).unwrap();
        s.clock.advance(Duration::from_secs(6));
        s.notify.script(Script::Answer("allow_once"));
        s.check(INSTALL, packages()).await.unwrap();
        assert!(!s.notify.last_show().body.unwrap().contains("Reason"));
    });
}

#[test]
fn an_intent_for_an_unknown_method_is_refused() {
    let s = Setup::new();
    let intent = IntentParams {
        method: "org.tetsu.tx.v1.Nope".into(),
        reason: "x".into(),
    };
    assert!(matches!(
        s.engine.intent(1, &intent),
        Err(CapError::UnknownMethod(m)) if m == "org.tetsu.tx.v1.Nope"
    ));
}

#[test]
fn two_open_asks_each_get_their_own_answer() {
    run(async {
        let s = Setup::with("[class]\n\"aur.review\" = \"ask\"\n", "", "");
        let (first_tx, first_rx) = oneshot::channel();
        let (second_tx, second_rx) = oneshot::channel();
        s.notify.script(Script::Gate(first_rx));
        s.notify.script(Script::Gate(second_rx));
        let engine = s.engine.clone();
        let first = tokio::task::spawn_local(async move {
            engine
                .check("tetsu-tx.service", &params(INSTALL, packages()))
                .await
        });
        tokio::task::yield_now().await;
        let engine = s.engine.clone();
        let second = tokio::task::spawn_local(async move {
            engine
                .check("tetsu-tx.service", &params(REVIEW, json!({})))
                .await
        });
        while s.notify.shows() < 2 {
            tokio::task::yield_now().await;
        }
        // Answered in reverse order.
        second_tx.send(NotifyReply::invoked("approve")).unwrap();
        first_tx.send(NotifyReply::invoked("deny")).unwrap();
        assert_eq!(second.await.unwrap().unwrap(), allow());
        assert_eq!(first.await.unwrap().unwrap(), deny(DeniedBy::User));
    });
}

#[test]
fn dropping_a_pending_check_withdraws_the_ask_and_audits_it() {
    run(async {
        let s = Setup::new();
        s.notify.script(Script::Hang);
        let engine = s.engine.clone();
        let pending = tokio::task::spawn_local(async move {
            engine
                .check("tetsu-tx.service", &params(INSTALL, packages()))
                .await
        });
        while s.notify.shows() < 1 {
            tokio::task::yield_now().await;
        }
        assert_eq!(s.notify.0.dropped.get(), 0);
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        assert_eq!(s.notify.0.dropped.get(), 1, "the show stream was dropped");
        assert_eq!(s.events(), ["ask", "denied", "check"]);
        let entries = s.entries();
        assert!(entries[1].cancelled);
        assert_eq!(entries[1].denied_by.as_deref(), Some("unavailable"));
        assert_eq!(s.cap("packages.install").suppressed_until_at, None);
    });
}

// 3. the audit trail

#[test]
fn a_check_entry_carries_the_decision_and_who_asked() {
    run(async {
        let s = Setup::with("[class]\n\"packages.install\" = \"automatic\"\n", "", "");
        s.check(INSTALL, packages()).await.unwrap();
        let entries = s.entries();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.event, Event::Check);
        assert_eq!(e.method.as_deref(), Some(INSTALL));
        assert_eq!(e.class.as_deref(), Some("packages.install"));
        assert_eq!(e.state.as_deref(), Some("automatic"));
        assert_eq!(e.layer.as_deref(), Some("image"));
        assert_eq!(e.decision.as_deref(), Some("allow"));
        assert_eq!(e.denied_by, None);
        assert_eq!(e.peer_pid, Some(4242));
        assert_eq!(e.peer_unit.as_deref(), Some("tetsu-agent.service"));
        assert_eq!(e.caller_unit.as_deref(), Some("tetsu-tx.service"));
        assert_eq!(e.args.as_deref(), Some("{\"packages\":[\"htop\"]}"));
    });
}

#[test]
fn a_failing_journal_turns_an_allow_into_a_deny() {
    run(async {
        let s = Setup::with("[class]\n\"packages.install\" = \"automatic\"\n", "", "");
        s.sink.fail.set(true);
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Unavailable)
        );
        // An ask is not even shown when its audit entry cannot be written.
        let s = Setup::new();
        s.sink.fail.set(true);
        s.notify.script(Script::Answer("allow_once"));
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Unavailable)
        );
        assert_eq!(s.notify.shows(), 0);
    });
}

#[test]
fn set_writes_a_set_entry_and_a_failing_journal_refuses_it() {
    let s = Setup::new();
    let set = SetParams {
        class: "packages.install".into(),
        scope: None,
        state: Some(Grant::Ask),
    };
    s.engine.set("sh", &set).unwrap();
    let e = &s.entries()[0];
    assert_eq!(e.event, Event::Set);
    assert_eq!(e.class.as_deref(), Some("packages.install"));
    assert_eq!(e.state.as_deref(), Some("ask"));

    let s = Setup::new();
    s.sink.fail.set(true);
    assert!(matches!(
        s.engine.set("sh", &set),
        Err(CapError::Unavailable)
    ));
    assert!(!s.managed_path().exists());
}

// 4. counters and List

#[test]
fn list_has_every_class_with_its_methods_and_counters() {
    run(async {
        let s = Setup::with("[class]\n\"packages.install\" = \"automatic\"\n", "", "");
        let list = s.engine.list();
        let classes = catalog::classes();
        assert_eq!(list.len(), classes.len());
        let update = list.iter().find(|c| c.class == "system.update").unwrap();
        assert!(update.destructive);
        assert_eq!(update.methods, [UPDATE]);
        assert_eq!(update.last_used_at, None);

        s.check(INSTALL, packages()).await.unwrap();
        let cap = s.cap("packages.install");
        assert_eq!(cap.last_used_at.as_deref(), Some(rfc3339(s.now()).as_str()));
    });
}

impl Setup {
    fn now(&self) -> std::time::SystemTime {
        use tetsu_capd::clock::Clock;
        self.clock.now()
    }
}

#[test]
fn last_used_is_set_on_allow_only() {
    run(async {
        let s = Setup::with("", "", "[class]\n\"packages.install\" = \"off\"\n");
        s.check(INSTALL, packages()).await.unwrap();
        assert_eq!(s.cap("packages.install").last_used_at, None);
    });
}

// 5. Watch

#[test]
fn watch_sends_initial_then_a_changed_after_a_set_and_after_each_check() {
    run(async {
        let s = Setup::with("[class]\n\"packages.install\" = \"automatic\"\n", "", "");
        let (initial, mut rx) = s.engine.watch();
        assert_eq!(initial.kind, EventKind::Initial);
        assert_eq!(
            initial.capabilities.as_ref().unwrap().len(),
            catalog::classes().len()
        );
        assert!(rx.try_recv().is_err());

        let set = SetParams {
            class: "aur.review".into(),
            scope: None,
            state: Some(Grant::Off),
        };
        s.engine.set("sh", &set).unwrap();
        let changed: CapabilityEvent = rx.try_recv().unwrap();
        assert_eq!(changed.kind, EventKind::Changed);
        let cap = changed.capability.unwrap();
        assert_eq!((cap.class.as_str(), cap.state), ("aur.review", State::Off));

        s.check(INSTALL, packages()).await.unwrap();
        let changed = rx.try_recv().unwrap();
        let cap = changed.capability.unwrap();
        assert_eq!(cap.class, "packages.install");
        assert!(cap.last_used_at.is_some());
    });
}

// 6. a malformed layer fails closed

#[test]
fn a_malformed_layer_denies_checks_and_refuses_sets_but_lists() {
    run(async {
        let s = Setup::with("", "", "[class]\n\"packages.install\" = \"automatic\"\n");
        assert_eq!(s.check(INSTALL, packages()).await.unwrap(), allow());
        std::fs::write(s.user_path(), "[class]\n\"packages.install\" = \"nope\"\n").unwrap();
        assert_eq!(
            s.check(INSTALL, packages()).await.unwrap(),
            deny(DeniedBy::Unavailable)
        );
        assert_eq!(s.notify.shows(), 0);
        let set = SetParams {
            class: "aur.review".into(),
            scope: None,
            state: Some(Grant::Off),
        };
        assert!(matches!(
            s.engine.set("sh", &set),
            Err(CapError::InvalidParameter(_))
        ));
        assert!(!s.managed_path().exists());
        // List answers from the last good parse.
        assert_eq!(s.cap("packages.install").state, State::Automatic);
    });
}

// 7. which methods are gated

#[test]
fn an_unknown_method_is_refused_and_an_ungated_one_is_allowed() {
    run(async {
        let s = Setup::new();
        assert!(matches!(
            s.check("org.tetsu.tx.v1.Nope", json!({})).await,
            Err(CapError::UnknownMethod(_))
        ));
        assert!(s.entries().is_empty());
        for method in ["org.tetsu.tx.v1.ListTransactions", "org.tetsu.cap.v1.Set"] {
            assert_eq!(s.check(method, json!({})).await.unwrap(), allow());
        }
        assert_eq!(s.notify.shows(), 0);
        let e = &s.entries()[0];
        assert_eq!((e.event, e.class.as_deref()), (Event::Check, None));
        assert_eq!(e.decision.as_deref(), Some("allow"));
    });
}

// Set validation

#[test]
fn set_validates_the_class_the_scope_and_the_user_file() {
    let s = Setup::with("", "", "[class]\n\"aur.review\" = \"ask\"\n");
    let set = |class: &str, scope: Option<&str>, state| SetParams {
        class: class.into(),
        scope: scope.map(str::to_owned),
        state,
    };
    assert!(matches!(
        s.engine.set("sh", &set("made.up", None, Some(Grant::Ask))),
        Err(CapError::UnknownClass(c)) if c == "made.up"
    ));
    assert!(matches!(
        s.engine
            .set("sh", &set("packages.install", Some("x"), Some(Grant::Ask))),
        Err(CapError::InvalidParameter("scope"))
    ));
    assert!(matches!(
        s.engine.set("sh", &set("aur.review", None, Some(Grant::Off))),
        Err(CapError::OverriddenByUser(c)) if c == "aur.review"
    ));
    s.engine
        .set("sh", &set("packages.install", None, Some(Grant::Notify)))
        .unwrap();
    s.engine
        .set("sh", &set("packages.install", None, None))
        .unwrap();
    assert_eq!(s.cap("packages.install").state, State::Unset);
}
