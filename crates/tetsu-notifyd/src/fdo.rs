//! `org.freedesktop.Notifications` (spec 1.3) on the session bus, for
//! applications. Capabilities `actions`, `body`, `icon-static`,
//! `persistence`; no `body-markup`, so text is stripped to plain text.

use std::collections::HashMap;

use tetsu_bus::notify::Urgency;
use tokio::sync::{mpsc, oneshot};
use zbus::OwnedVariant;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;

use crate::core::{Effect, FdoShow};
use crate::daemon::Command;
use crate::icon;
use crate::markup::plain_text;

/// The well-known name.
pub const BUS_NAME: &str = "org.freedesktop.Notifications";
/// The object path.
pub const PATH: &str = "/org/freedesktop/Notifications";
const CAPABILITIES: [&str; 4] = ["actions", "body", "icon-static", "persistence"];

/// The D-Bus object.
#[derive(Debug)]
pub struct Notifications {
    commands: mpsc::UnboundedSender<Command>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        #[zbus(header)] header: Header<'_>,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let show = FdoShow {
            client: header.sender().map(|s| s.to_string()).unwrap_or_default(),
            app_name: plain_text(&app_name),
            replaces_id,
            summary: plain_text(&summary),
            body: plain_text(&body),
            // Keys are opaque ids sent back as they came; labels are text.
            actions: actions
                .iter()
                .enumerate()
                .map(|(i, a)| if i % 2 == 0 { a.clone() } else { plain_text(a) })
                .collect(),
            urgency: urgency(&hints),
            category: hint_string(&hints, "category"),
            expire_timeout,
            icon: icon::from_hints_bounded(hints, app_icon).await,
        };
        let (done, id) = oneshot::channel();
        self.commands
            .send(Command::ShowFdo { show, done })
            .map_err(|_| unavailable())?;
        id.await.map_err(|_| unavailable())
    }

    fn close_notification(&self, id: u32) -> zbus::fdo::Result<()> {
        self.commands
            .send(Command::CloseFdo(id))
            .map_err(|_| unavailable())
    }

    fn get_capabilities(&self) -> Vec<String> {
        CAPABILITIES.iter().map(|c| (*c).to_owned()).collect()
    }

    #[zbus(out_args("name", "vendor", "version", "spec_version"))]
    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "tetsu-notifyd".into(),
            "Tetsu".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.3".into(),
        )
    }

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}

/// Serves the interface on the bus `builder` connects to, takes the
/// well-known name (`replace` takes it from a running server), and emits
/// the signals the daemon sends on `signals`.
pub async fn serve(
    builder: zbus::connection::Builder<'_>,
    commands: mpsc::UnboundedSender<Command>,
    mut signals: mpsc::UnboundedReceiver<Effect>,
    replace: bool,
) -> zbus::Result<zbus::Connection> {
    let connection = builder
        .serve_at(PATH, Notifications { commands })
        .name(BUS_NAME)
        .replace_existing_names(replace)
        .build()
        .await?;
    let emitting = connection.clone();
    tokio::task::spawn_local(async move {
        let Ok(emitter) = SignalEmitter::new(&emitting, PATH) else {
            return;
        };
        while let Some(effect) = signals.recv().await {
            let sent = match effect {
                Effect::FdoActionInvoked { id, action, .. } => {
                    Notifications::action_invoked(&emitter, id, &action).await
                }
                Effect::FdoClosed { id, reason, .. } => {
                    Notifications::notification_closed(&emitter, id, reason).await
                }
                _ => Ok(()),
            };
            if let Err(e) = sent {
                eprintln!("tetsu-notifyd: cannot emit a freedesktop signal: {e}");
            }
        }
    });
    Ok(connection)
}

fn unavailable() -> zbus::fdo::Error {
    zbus::fdo::Error::Failed("tetsu-notifyd is shutting down".into())
}

fn urgency(hints: &HashMap<String, OwnedVariant>) -> Urgency {
    match hints.get("urgency").and_then(|v| u8::try_from(v).ok()) {
        Some(0) => Urgency::Low,
        Some(2) => Urgency::Critical,
        _ => Urgency::Normal,
    }
}

fn hint_string(hints: &HashMap<String, OwnedVariant>, key: &str) -> Option<String> {
    hints
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .map(plain_text)
}
