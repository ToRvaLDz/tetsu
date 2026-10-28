//! The config on the loop: the poll tick, applying a new view, and the
//! `SetManaged` commands from the bus thread.

use std::time::Duration;

use tetsu_bus::config::{ConfigEvent, ConfigKind, GetConfigReply};

use super::set::{self, plan};
use super::store::Store;
use crate::bus::SetManaged;
use crate::outputs::glue::Refused;
use crate::settings::Settings;
use crate::state::State;

/// How often the three files are looked at.
pub const RELOAD_POLL: Duration = Duration::from_millis(500);

/// The config as `GetConfig` shows it.
pub fn config_reply(store: Option<&Store>) -> GetConfigReply {
    store.map_or_else(GetConfigReply::default, |store| GetConfigReply {
        entries: store.view().entries().to_vec(),
        diagnostics: store.diagnostics().to_vec(),
    })
}

impl State {
    /// Looks at the files and applies what changed.
    pub fn config_tick(&mut self) {
        let Some(store) = self.config.as_mut() else {
            return;
        };
        let events = store.poll();
        if let Err(refused) = self.apply_config(events) {
            eprintln!("tetsu-comp: {refused}");
        }
    }

    /// Publishes the events and, when a key changed, runs with the new
    /// settings. The render and trust keys apply at the next start.
    pub(crate) fn apply_config(&mut self, events: Vec<ConfigEvent>) -> Result<(), Refused> {
        if events.is_empty() {
            return Ok(());
        }
        let changed = events.iter().any(|e| e.kind == ConfigKind::Changed);
        let outputs = events.iter().any(|e| {
            e.kind != ConfigKind::Diagnostics
                && (e.key.as_deref())
                    .or(e.entry.as_ref().map(|x| x.key.as_str()))
                    .is_some_and(|key| key.starts_with("output."))
        });
        let render = events.iter().any(|e| {
            (e.entry.as_ref().map(|x| x.key.as_str())).is_some_and(|key| key.starts_with("render."))
        });
        if let Some(publisher) = &self.publisher {
            publisher.publish_config(events);
        }
        // A render key edited in a file applies at the next start; only
        // the explicit sync mode shows at once.
        if render {
            self.render_publish();
        }
        let Some(store) = &self.config else {
            return Ok(());
        };
        if changed {
            let settings = Settings::from_values(&store.view().values());
            self.lock_glue.set_command(settings.lock.command.clone());
            self.idle.set_timeout_s(settings.lock.idle_timeout_s);
            self.engine.set_settings(settings);
            self.relayout();
        }
        if outputs {
            return self.reapply_outputs();
        }
        Ok(())
    }

    /// Runs a `SetManaged` from the bus: reads the files again first, so
    /// an edit saved a moment ago counts, then plans, writes and applies.
    pub fn handle_config_command(&mut self, command: SetManaged) {
        let SetManaged {
            values,
            remove,
            reply,
        } = command;
        let Some(store) = self.config.as_mut() else {
            let _ = reply.send(Err(set::state_write_failed("the config is not loaded")));
            return;
        };
        let mut events = store.poll();
        let answer = match plan(store.view(), store.managed_values(), &values, &remove) {
            Err(error) => Err(error.reply()),
            Ok(next) => match store.write_managed(&next) {
                Ok(written) => {
                    events.extend(written);
                    Ok(())
                }
                Err(e) => Err(set::state_write_failed(&e.0)),
            },
        };
        if let Err(refused) = self.apply_config(events) {
            eprintln!("tetsu-comp: {refused}");
        }
        let _ = reply.send(answer);
    }
}
