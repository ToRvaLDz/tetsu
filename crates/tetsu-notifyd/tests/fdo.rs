//! `org.freedesktop.Notifications` on a private D-Bus daemon. Skipped with
//! a note when `dbus-daemon` is not installed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command as Process, Stdio};
use std::time::Duration;

use futures_util::StreamExt;
use tetsu_bus::notify::Urgency;
use tetsu_notifyd::core::{ARM_DELAY, Source, View};
use tetsu_notifyd::daemon::{Command, Daemon, UiMsg};
use tetsu_notifyd::fdo;
use tokio::sync::mpsc;
use tokio::task::LocalSet;
use zbus::Variant as Value;

const TIMEOUT: Duration = Duration::from_secs(5);

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, &Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;
    fn close_notification(&self, id: u32) -> zbus::Result<()>;
    fn get_capabilities(&self) -> zbus::Result<Vec<String>>;
    fn get_server_information(&self) -> zbus::Result<(String, String, String, String)>;
    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: String) -> zbus::Result<()>;
    #[zbus(signal)]
    fn notification_closed(&self, id: u32, reason: u32) -> zbus::Result<()>;
}

struct Bus(Child, String);

impl Bus {
    fn start() -> Option<Self> {
        let mut child = Process::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut line = String::new();
        BufReader::new(child.stdout.take()?)
            .read_line(&mut line)
            .ok()?;
        Some(Self(child, line.trim().to_owned()))
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn until(rx: &mut mpsc::UnboundedReceiver<UiMsg>, pred: impl Fn(&View) -> bool) -> View {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .expect("the view in time")
            .unwrap();
        if let UiMsg::View(view) = msg
            && pred(&view)
        {
            return view;
        }
    }
}

#[test]
fn notify_send_style_calls_show_and_answer() {
    let Some(bus) = Bus::start() else {
        eprintln!("note: dbus-daemon not installed, skipping");
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
        let (signals_tx, signals_rx) = mpsc::unbounded_channel();
        let (daemon, commands) = Daemon::new(ui_tx);
        tokio::task::spawn_local(daemon.with_fdo_signals(signals_tx).run());
        commands.send(Command::Lock(false)).unwrap();
        let builder = zbus::connection::Builder::address(bus.1.as_str());
        let _server = fdo::serve(builder, commands.clone(), signals_rx, false)
            .await
            .unwrap();

        let client = zbus::connection::Builder::address(bus.1.as_str())
            .build()
            .await
            .unwrap();
        let proxy = NotificationsProxy::new(&client).await.unwrap();

        let capabilities = proxy.get_capabilities().await.unwrap();
        assert_eq!(
            capabilities,
            ["actions", "body", "icon-static", "persistence"]
        );
        let (name, _vendor, _version, spec) = proxy.get_server_information().await.unwrap();
        assert_eq!((name.as_str(), spec.as_str()), ("tetsu-notifyd", "1.3"));

        let mut invoked = proxy.receive_action_invoked().await.unwrap();
        let mut closed = proxy.receive_notification_closed().await.unwrap();
        let critical = Value::from(2u8);
        let category = Value::from("im.received");
        let hints = HashMap::from([("urgency", &critical), ("category", &category)]);
        let id = proxy
            .notify(
                "chat",
                0,
                "",
                "<b>Ana</b>",
                "hi &amp; <i>bye</i>",
                &["default", "Open", "re&amp;ply", "<b>Reply</b>"],
                hints,
                -1,
            )
            .await
            .unwrap();
        assert_ne!(id, 0);

        let view = until(&mut ui_rx, |v| !v.popups.is_empty()).await;
        let note = &view.popups[0];
        assert_eq!(note.source, Source::App("chat".into()));
        assert_eq!(note.title, "Ana");
        assert_eq!(note.body, "hi & bye");
        assert_eq!(note.urgency, Urgency::Critical);
        assert!(note.default_action);
        assert_eq!(note.buttons.len(), 1);
        assert_eq!(note.buttons[0].label, "Reply");
        assert_eq!(note.category.as_deref(), Some("im.received"));

        tokio::time::sleep(ARM_DELAY + Duration::from_millis(100)).await;
        commands
            .send(Command::Click {
                note: note.id,
                action: "re&amp;ply".into(),
            })
            .unwrap();
        let signal = tokio::time::timeout(TIMEOUT, invoked.next())
            .await
            .unwrap()
            .unwrap();
        let args = signal.args().unwrap();
        // Keys come back exactly as sent; only labels are stripped.
        assert_eq!((args.id, args.action_key.as_str()), (id, "re&amp;ply"));
        let signal = tokio::time::timeout(TIMEOUT, closed.next())
            .await
            .unwrap()
            .unwrap();
        let args = signal.args().unwrap();
        assert_eq!((args.id, args.reason), (id, 2));

        let other = proxy
            .notify("x", 0, "", "Short", "", &[], HashMap::new(), 0)
            .await
            .unwrap();
        proxy.close_notification(other).await.unwrap();
        let signal = tokio::time::timeout(TIMEOUT, closed.next())
            .await
            .unwrap()
            .unwrap();
        let args = signal.args().unwrap();
        assert_eq!((args.id, args.reason), (other, 3));
    });
}
