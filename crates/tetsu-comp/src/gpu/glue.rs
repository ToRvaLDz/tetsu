//! The render control on the loop (#107): the Set calls from the bus, the
//! switch on each backend, the managed write, the changes kept for unlock,
//! and the `changed` the bus gets when the reported state moved.
//!
//! A call switches first and writes after: a failed switch writes
//! nothing, and a failed write puts the previous device back. While a
//! call or the unlock runs, nothing is published, so the watchers see one
//! `changed` with the outcome.

use std::collections::BTreeMap;

use tetsu_bus::render::{Render, RenderEvent};

use super::control::{
    Change, DEVICE_KEY, Plan, RenderCtl, RenderError, SYNC_KEY, Switch, Target, config_value,
    plan_device, plan_sync, tty_switch,
};
use super::pending::resolve;
use super::report::{self, diff};
use crate::backend::headless_gpu;
use crate::bus::{SetExplicitSync, SetRenderDevice};
use crate::config::store::Store;
use crate::settings::{self, SyncMode};
use crate::state::State;

/// The effective `render.explicit_sync` of the config.
fn configured_mode(store: &Store) -> Option<SyncMode> {
    let entry = store.view().entry(SYNC_KEY)?;
    settings::sync_mode(entry.value.as_str()?).ok()
}

impl State {
    /// Runs a `SetRenderDevice` from the bus.
    pub fn handle_render_device(&mut self, call: SetRenderDevice) {
        let target = Target::of(call.target.as_deref());
        let answer = self.render_call(|ctl, user, locked| plan_device(ctl, &target, user, locked));
        let _ = call.reply.send(answer.map_err(|e| e.reply()));
    }

    /// Runs a `SetExplicitSync` from the bus.
    pub fn handle_explicit_sync(&mut self, call: SetExplicitSync) {
        let mode = call.mode;
        let answer = self.render_call(|ctl, user, locked| plan_sync(ctl, mode, user, locked));
        let _ = call.reply.send(answer.map_err(|e| e.reply()));
    }

    /// Plans and runs one call; `Ok(true)` when it waits for unlock. The
    /// files are read again first, so an edit saved a moment ago counts.
    fn render_call(
        &mut self,
        plan: impl FnOnce(&RenderCtl, &[String], bool) -> Result<Plan, RenderError>,
    ) -> Result<bool, RenderError> {
        self.render.batch = true;
        self.config_tick();
        let user = self.render_user_keys();
        let answer = match plan(&self.render, &user, self.lock.is_locked()) {
            Ok(Plan::Pending(change)) => {
                self.render.hold(change);
                Ok(true)
            }
            Ok(Plan::Apply { change, key, value }) => {
                self.apply_render(&change, key, &value).map(|()| false)
            }
            Err(e) => Err(e),
        };
        self.render.batch = false;
        self.render_publish();
        answer
    }

    fn render_user_keys(&self) -> Vec<String> {
        self.config
            .as_ref()
            .map(|store| store.view().user_keys().to_vec())
            .unwrap_or_default()
    }

    /// Switches (for a device change), then writes the managed key. A
    /// failed write puts the previous device back.
    fn apply_render(&mut self, change: &Change, key: &str, value: &str) -> Result<(), RenderError> {
        let before = (self.render.current.clone(), self.render.automatic);
        if let Change::Device(target) = change {
            match self.switch_render_device(target) {
                Switch::Applied => {
                    self.render.current = self.render.resolve(target);
                    self.render.automatic = *target == Target::Auto;
                }
                Switch::NextStart => {
                    println!("tetsu-comp: render device {value} applies at the next start");
                }
                Switch::Failed(reason) => {
                    let id = self.render.resolve(target);
                    self.render.failed(&id);
                    eprintln!("tetsu-comp: the switch to render device {id} failed: {reason}");
                    return Err(RenderError::SwitchFailed(reason));
                }
            }
        }
        if let Err(e) = self.write_render(key, value) {
            (self.render.current, self.render.automatic) = before;
            return Err(e);
        }
        Ok(())
    }

    /// The switch, as the running backend takes it.
    fn switch_render_device(&self, target: &Target) -> Switch {
        if self.tty.is_some() {
            tty_switch(&self.render.current, target, &self.render.auto_id)
        } else {
            headless_gpu::switch(&self.render, target)
        }
    }

    /// Sets one managed key and applies the config events it makes.
    fn write_render(&mut self, key: &str, value: &str) -> Result<(), RenderError> {
        let Some(store) = self.config.as_mut() else {
            return Err(RenderError::StateWriteFailed(
                "the config is not loaded".to_owned(),
            ));
        };
        let mut events = store.poll();
        let mut next = store.managed_values().clone();
        next.insert(key.to_owned(), toml::Value::String(value.to_owned()));
        let written = match store.write_managed(&next) {
            Ok(more) => {
                events.extend(more);
                Ok(())
            }
            Err(e) => Err(RenderError::StateWriteFailed(e.0)),
        };
        if let Err(refused) = self.apply_config(events) {
            eprintln!("tetsu-comp: {refused}");
        }
        written
    }

    /// The session unlocked: the changes kept for it are applied, or
    /// dropped with a `pending_dropped` each. One `changed` follows.
    pub fn render_unlocked(&mut self) {
        if self.render.pending_device.is_none() && self.render.pending_sync.is_none() {
            return;
        }
        self.render.batch = true;
        self.config_tick();
        let resolution = resolve(&self.render, &self.render.ids(), &self.render_user_keys());
        self.render.pending_device = None;
        self.render.pending_sync = None;
        let changes = [
            resolution.device.map(|t| (Change::Device(t), DEVICE_KEY)),
            resolution.sync.map(|m| (Change::Sync(m), SYNC_KEY)),
        ];
        for (change, key) in changes.into_iter().flatten() {
            let value = config_value(&self.render, &change);
            if let Err(e) = self.apply_render(&change, key, &value) {
                eprintln!(
                    "tetsu-comp: the {key} change kept for unlock failed: {}",
                    e.reply().parameters
                );
            }
        }
        self.render.batch = false;
        let dropped = resolution
            .dropped
            .into_iter()
            .map(RenderEvent::pending_dropped)
            .collect();
        self.render_publish_with(dropped);
    }

    /// Sends a `changed` when the reported state moved. Runs after every
    /// change and on the publish tick, which catches outputs coming and
    /// going.
    pub fn render_publish(&mut self) {
        self.render_publish_with(Vec::new());
    }

    /// Like `render_publish`, with `pending_dropped` events first; a
    /// `changed` always follows them.
    fn render_publish_with(&mut self, mut events: Vec<RenderEvent>) {
        // A tty taken out of the state has no heads to report.
        if self.render.batch || self.tty_taken {
            return;
        }
        if let Some(mode) = self.config.as_ref().and_then(configured_mode) {
            self.render.mode = mode;
        }
        let now = self.render_reply();
        let owed = match &self.render.published {
            Some(_) if !events.is_empty() => Some(RenderEvent::changed(now.clone())),
            Some(last) => diff(last, &now),
            // The first state goes into the bus thread's snapshot.
            None => None,
        };
        events.extend(owed);
        self.render.published = Some(now);
        if let Some(publisher) = &self.publisher {
            publisher.publish_render(events);
        }
    }

    /// The render state as `GetRender` reports it now.
    pub fn render_reply(&self) -> Render {
        report::render(&self.render, &self.render_outputs())
    }

    /// The outputs wired to each device: the tty's heads, else every output
    /// on the synthetic device.
    fn render_outputs(&self) -> BTreeMap<String, Vec<String>> {
        if let Some(tty) = &self.tty {
            return crate::backend::udev::render::output_ids(tty, &self.outputs);
        }
        let ids = self.outputs.iter().map(|o| o.id.clone()).collect();
        self.render
            .devices
            .first()
            .map(|device| BTreeMap::from([(device.id.clone(), ids)]))
            .unwrap_or_default()
    }
}
