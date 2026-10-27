use std::cell::{Cell, RefCell};
use std::io;

use serde_json::Value;
use tetsu_bus::secrets::{SecretEvent, SecretEventKind};

use super::*;
use crate::audit::{Action, AuditEntry, Outcome};

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const VALUE: &str = "S3NT1NEL-SENTINEL";

#[derive(Default)]
struct Recording {
    entries: RefCell<Vec<AuditEntry>>,
    fail: Cell<bool>,
}

impl AuditSink for Recording {
    fn record(&self, entry: &AuditEntry) -> io::Result<()> {
        if self.fail.get() {
            return Err(io::Error::other("journal down"));
        }
        self.entries.borrow_mut().push(entry.clone());
        Ok(())
    }
}

struct Fixed(Cell<u64>);

impl Clock for Fixed {
    fn now_secs(&self) -> u64 {
        self.0.get()
    }
}

struct Rig {
    broker: Broker,
    sink: Rc<Recording>,
    clock: Rc<Fixed>,
    _dir: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("v/vault.keyring"), &Secret::new(KEY))
            .await
            .unwrap();
        let sink = Rc::new(Recording::default());
        let clock = Rc::new(Fixed(Cell::new(1_700_000_000)));
        let broker = Broker::new(store, sink.clone(), clock.clone());
        Self {
            broker,
            sink,
            clock,
            _dir: dir,
        }
    }

    fn entries(&self) -> Vec<AuditEntry> {
        self.sink.entries.borrow().clone()
    }

    fn last(&self) -> AuditEntry {
        self.entries().pop().expect("an audit entry")
    }
}

fn user() -> Caller {
    Caller {
        unit: Some("tetsu-cli.scope".into()),
        pid: 100,
        exe: Some("/usr/bin/tetsu".into()),
        comm: "tetsu".into(),
        agent: false,
    }
}

fn agent() -> Caller {
    Caller {
        unit: Some("tetsu-agent.service".into()),
        pid: 200,
        exe: None,
        comm: "tetsu-agent".into(),
        agent: true,
    }
}

fn new_secret(id: &str) -> NewSecret {
    NewSecret {
        id: SecretId::parse(id).unwrap(),
        kind: KindSpec::parse("bearer").unwrap(),
        host: Host::parse("api.example.com").unwrap(),
        allowed_tools: vec![ToolId::parse("web-fetch").unwrap()],
        value: Secret::new(VALUE),
    }
}

fn id(text: &str) -> SecretId {
    SecretId::parse(text).unwrap()
}

fn proxy(host: &str) -> Use {
    Use::Proxy {
        upstream: host.into(),
    }
}

fn event(item: StreamItem) -> SecretEvent {
    let StreamItem::Reply(value) = item else {
        panic!("not a reply: {item:?}");
    };
    let event: Value = value["event"].clone();
    serde_json::from_value(event).unwrap()
}

#[tokio::test]
async fn add_stores_and_audits() {
    let rig = Rig::new().await;
    let info = rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    assert_eq!(info.id, "demo");
    assert_eq!(info.host, "api.example.com");
    assert_eq!(info.allowed_tools, ["web-fetch"]);
    assert_eq!(info.created_at, "2023-11-14T22:13:20.000Z");
    assert_eq!(info.use_count, 0);
    let entry = rig.last();
    assert_eq!(entry.action, Action::Add);
    assert_eq!(entry.outcome, Outcome::Ok);
    assert_eq!(entry.secret_id.as_deref(), Some("demo"));
    assert_eq!(entry.caller, user());
    let listed = rig.broker.list(&user()).await.unwrap();
    assert_eq!(listed, [info]);
}

#[tokio::test]
async fn listing_is_audited() {
    let rig = Rig::new().await;
    rig.broker.list(&user()).await.unwrap();
    assert_eq!(rig.last().action, Action::List);
    assert_eq!(rig.last().outcome, Outcome::Ok);
}

#[tokio::test]
async fn the_agent_cannot_write() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    let denied = Err(BrokerError::Denied("the agent unit may not change secrets"));
    assert_eq!(rig.broker.add(&agent(), new_secret("other")).await, denied);
    assert_eq!(rig.last().action, Action::Denied);
    assert_eq!(rig.last().outcome, Outcome::Denied);
    assert_eq!(rig.last().caller, agent());
    let value = Secret::new("changed");
    assert_eq!(
        rig.broker.rotate(&agent(), &id("demo"), value).await,
        denied
    );
    assert_eq!(
        rig.broker.remove(&agent(), &id("demo")).await,
        Err(denied.clone().unwrap_err())
    );
    let listed = rig.broker.list(&agent()).await.unwrap();
    assert_eq!(listed.len(), 1);
    let got = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(got.unwrap().expose(), VALUE);
}

#[tokio::test]
async fn a_duplicate_add_changes_nothing() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    let mut again = new_secret("demo");
    again.value = Secret::new("other");
    let e = rig.broker.add(&user(), again).await.unwrap_err();
    assert_eq!(e, BrokerError::Exists(id("demo")));
    assert_eq!(rig.last().outcome, Outcome::Failed);
    let got = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(got.unwrap().expose(), VALUE);
}

#[tokio::test]
async fn rotate_keeps_the_binding_and_audits() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    rig.clock.0.set(1_700_000_100);
    let info = rig
        .broker
        .rotate(&user(), &id("demo"), Secret::new("second"))
        .await
        .unwrap();
    assert_eq!(info.rotated_at.as_deref(), Some("2023-11-14T22:15:00.000Z"));
    assert_eq!(info.host, "api.example.com");
    assert_eq!(rig.last().action, Action::Rotate);
    let got = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(got.unwrap().expose(), "second");
}

#[tokio::test]
async fn remove_and_unknown_ids() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    rig.broker.remove(&user(), &id("demo")).await.unwrap();
    assert_eq!(rig.last().action, Action::Remove);
    assert!(rig.broker.list(&user()).await.unwrap().is_empty());
    let missing = BrokerError::NotFound(id("demo"));
    assert_eq!(
        rig.broker.remove(&user(), &id("demo")).await,
        Err(missing.clone())
    );
    let rotated = rig
        .broker
        .rotate(&user(), &id("demo"), Secret::new("x"))
        .await;
    assert_eq!(rotated, Err(missing.clone()));
    let used = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(used.unwrap_err(), missing);
    assert_eq!(rig.last().outcome, Outcome::Failed);
}

#[tokio::test]
async fn a_proxy_use_counts_and_is_audited() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    rig.clock.0.set(1_700_000_200);
    let got = rig
        .broker
        .use_secret(&agent(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(got.unwrap().expose(), VALUE);
    let entry = rig
        .entries()
        .into_iter()
        .rev()
        .find(|e| e.action == Action::Use)
        .unwrap();
    assert_eq!(entry.outcome, Outcome::Ok);
    assert_eq!(entry.upstream.as_deref(), Some("api.example.com"));
    assert_eq!(entry.secret_id.as_deref(), Some("demo"));
    assert_eq!(entry.caller, agent());
    rig.broker
        .use_secret(&agent(), &id("demo"), proxy("api.example.com"))
        .await
        .unwrap();
    let info = rig.broker.list(&user()).await.unwrap().remove(0);
    assert_eq!(info.use_count, 2);
    assert_eq!(
        info.last_used_at.as_deref(),
        Some("2023-11-14T22:16:40.000Z")
    );
}

#[tokio::test]
async fn a_host_mismatch_is_denied_and_audited() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    for host in [
        "evil.example.net",
        "api.example.com:8443",
        "sub.api.example.com",
        "",
    ] {
        let got = rig
            .broker
            .use_secret(&agent(), &id("demo"), proxy(host))
            .await;
        assert!(matches!(got, Err(BrokerError::Denied(_))), "{host}");
        let entry = rig.last();
        assert_eq!(entry.action, Action::Denied);
        assert_eq!(entry.outcome, Outcome::Denied);
        assert_eq!(entry.upstream.as_deref(), Some(host));
    }
    let info = rig.broker.list(&user()).await.unwrap().remove(0);
    assert_eq!(info.use_count, 0);
}

#[tokio::test]
async fn a_spawn_needs_an_allowed_tool() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    let tool = |t: &str| Use::Spawn { tool_id: t.into() };
    let got = rig
        .broker
        .use_secret(&agent(), &id("demo"), tool("web-fetch"))
        .await;
    assert_eq!(got.unwrap().expose(), VALUE);
    let entry = rig.last();
    assert_eq!(entry.action, Action::Spawn);
    assert_eq!(entry.tool_id.as_deref(), Some("web-fetch"));
    let got = rig
        .broker
        .use_secret(&agent(), &id("demo"), tool("rogue"))
        .await;
    assert!(matches!(got, Err(BrokerError::Denied(_))));
    assert_eq!(rig.last().action, Action::Denied);
    assert_eq!(rig.last().tool_id.as_deref(), Some("rogue"));
}

#[tokio::test]
async fn a_failing_journal_refuses_and_changes_nothing() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    rig.sink.fail.set(true);
    assert_eq!(
        rig.broker.add(&user(), new_secret("two")).await,
        Err(BrokerError::Unavailable)
    );
    let rotated = rig
        .broker
        .rotate(&user(), &id("demo"), Secret::new("changed"))
        .await;
    assert_eq!(rotated, Err(BrokerError::Unavailable));
    assert_eq!(
        rig.broker.remove(&user(), &id("demo")).await,
        Err(BrokerError::Unavailable)
    );
    let used = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(used.unwrap_err(), BrokerError::Unavailable);
    assert_eq!(
        rig.broker.list(&user()).await,
        Err(BrokerError::Unavailable)
    );
    rig.sink.fail.set(false);
    let listed = rig.broker.list(&user()).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].use_count, 0);
    assert_eq!(listed[0].rotated_at, None);
    let got = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("api.example.com"))
        .await;
    assert_eq!(got.unwrap().expose(), VALUE);
}

#[tokio::test]
async fn watchers_see_initial_then_each_change() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    let mut watch = rig.broker.watch(&user()).await.unwrap();
    let initial = event(watch.recv().await.unwrap());
    assert_eq!(initial.kind, SecretEventKind::Initial);
    assert_eq!(initial.secrets.unwrap().len(), 1);
    rig.broker.add(&user(), new_secret("two")).await.unwrap();
    assert_eq!(
        event(watch.recv().await.unwrap()).kind,
        SecretEventKind::Added
    );
    rig.broker
        .use_secret(&user(), &id("two"), proxy("api.example.com"))
        .await
        .unwrap();
    let changed = event(watch.recv().await.unwrap());
    assert_eq!(changed.kind, SecretEventKind::Changed);
    assert_eq!(changed.secret.unwrap().use_count, 1);
    rig.broker
        .rotate(&user(), &id("two"), Secret::new("v2"))
        .await
        .unwrap();
    assert_eq!(
        event(watch.recv().await.unwrap()).kind,
        SecretEventKind::Changed
    );
    rig.broker.remove(&user(), &id("two")).await.unwrap();
    let removed = event(watch.recv().await.unwrap());
    assert_eq!(removed.kind, SecretEventKind::Removed);
    assert_eq!(removed.id.as_deref(), Some("two"));
    assert!(watch.try_recv().is_err());
}

#[tokio::test]
async fn a_dropped_watcher_is_forgotten() {
    let rig = Rig::new().await;
    let watch = rig.broker.watch(&user()).await.unwrap();
    drop(watch);
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
}

#[tokio::test]
async fn no_entry_and_no_error_holds_the_value() {
    let rig = Rig::new().await;
    rig.broker.add(&user(), new_secret("demo")).await.unwrap();
    let _ = rig.broker.add(&user(), new_secret("demo")).await;
    let _ = rig.broker.add(&agent(), new_secret("x")).await;
    let _ = rig
        .broker
        .use_secret(&user(), &id("demo"), proxy("nope.example.net"))
        .await;
    rig.broker
        .rotate(&user(), &id("demo"), Secret::new("S3NT1NEL-SECOND"))
        .await
        .unwrap();
    rig.sink.fail.set(true);
    let failed = rig.broker.add(&user(), new_secret("y")).await.unwrap_err();
    let shown = format!("{:?} {failed} {failed:?}", rig.entries());
    assert!(!shown.contains("S3NT1NEL"), "{shown}");
    for entry in rig.entries() {
        let bytes = crate::audit::encode(&entry);
        assert!(!bytes.windows(8).any(|w| w == b"S3NT1NEL"));
    }
}
