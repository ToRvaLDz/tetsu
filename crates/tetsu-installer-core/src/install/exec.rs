//! The executor: runs a plan action by action through the `Runner`, reports
//! progress to the frontend and the log, and stops at the first failure.
//! It checks for cancellation between actions, so the binary can honour
//! Ctrl+C at a safe point.

use std::ffi::{OsStr, OsString};

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::space::SpaceProbe;
use tetsu_tx::boot::tool::Runner;

use crate::answers::Answers;
use crate::entropy::Entropy;
use crate::error::InstallError;
use crate::frontend::Frontend;

use super::argv;
use super::log::InstallLog;
use super::plan::{Action, InstallPlan, Stage};
use super::rundir::RunDir;
use super::target::InstallTarget;

/// Everything the executor reaches the machine through.
pub struct Deps<'a> {
    pub runner: &'a dyn Runner,
    /// Free-space probe for the ESP.
    pub probe: &'a dyn SpaceProbe,
    pub entropy: &'a dyn Entropy,
    /// Polled between actions; `true` stops the install with
    /// [`InstallError::Cancelled`].
    pub cancelled: &'a dyn Fn() -> bool,
}

/// What earlier actions found out and later ones need.
#[derive(Default)]
pub(super) struct State {
    pub luks_uuid: Option<String>,
    pub password_hash: Option<String>,
    pub kernel_version: Option<String>,
    /// The committed UKI on the ESP.
    pub uki_path: Option<std::path::PathBuf>,
    /// The `BootOrder` to set once the Tetsu entry exists.
    pub boot_order: Option<String>,
    /// The base kernel command line, without the per-snapshot rootflags.
    pub cmdline: Option<String>,
}

/// Runs one install plan.
pub struct Executor<'a> {
    pub(super) deps: Deps<'a>,
    pub(super) answers: &'a Answers,
    pub(super) target: &'a InstallTarget,
    pub(super) fe: &'a mut dyn Frontend,
    pub(super) run: RunDir,
    pub(super) log: InstallLog,
    pub(super) state: State,
    /// Actions of the plan already performed; `run` continues after them.
    done: usize,
    /// An action was started: the machine may have been changed.
    touched: bool,
    /// The keys were written to the firmware.
    keys_enrolled: bool,
}

/// Lets a `&dyn Runner` be handed to APIs that take a `Runner` by value.
pub(super) struct Shared<'a>(pub &'a dyn Runner);

impl Runner for Shared<'_> {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.0.run(program, args)
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.0.output(program, args)
    }

    fn check_tool(&self, program: &str) -> Result<(), BootError> {
        self.0.check_tool(program)
    }
}

impl<'a> Executor<'a> {
    /// Prepare the private run directory of `target`.
    pub fn new(
        deps: Deps<'a>,
        answers: &'a Answers,
        target: &'a InstallTarget,
        fe: &'a mut dyn Frontend,
    ) -> Result<Self, InstallError> {
        Ok(Self {
            deps,
            answers,
            target,
            fe,
            run: RunDir::create(&target.run_dir)?,
            log: InstallLog::new(),
            state: State::default(),
            done: 0,
            touched: false,
            keys_enrolled: false,
        })
    }

    /// The log so far (no secrets).
    pub fn log(&self) -> &InstallLog {
        &self.log
    }

    /// Whether any action was started, so the disk may have been changed.
    pub fn touched(&self) -> bool {
        self.touched
    }

    /// Whether the Secure Boot keys are in the firmware. From then on the
    /// firmware trusts the keys on the disk, so the disk must be kept.
    pub fn keys_enrolled(&self) -> bool {
        self.keys_enrolled
    }

    /// Run every action of `plan`. A failed soft action is logged and
    /// skipped; any other failure stops the install. It starts at the first
    /// action not yet performed, so after `Cancelled` a second call carries
    /// on where the run stopped.
    pub fn run(&mut self, plan: &InstallPlan) -> Result<(), InstallError> {
        for &action in plan.actions().iter().skip(self.done) {
            if (self.deps.cancelled)() {
                self.say(action.stage(), "cancelled before the next action");
                return Err(InstallError::Cancelled);
            }
            self.say(action.stage(), &action.describe());
            self.touched = true;
            match self.perform(action) {
                Ok(()) => {
                    self.done += 1;
                    self.keys_enrolled |= action == Action::EnrollKeys;
                }
                Err(e) if action.is_soft() => {
                    self.done += 1;
                    self.warn(&format!("{}: {e}", action.describe()));
                    self.warn(action.soft_consequence());
                }
                Err(e) => {
                    self.say(action.stage(), &format!("failed: {e}"));
                    return Err(InstallError::ActionFailed {
                        action: action.describe(),
                        source: Box::new(e),
                    });
                }
            }
        }
        self.say(Stage::Firmware, "installation complete");
        Ok(())
    }

    fn perform(&mut self, action: Action) -> Result<(), InstallError> {
        match action.stage() {
            Stage::Disk => self.disk_action(action),
            Stage::Boot => self.boot_action(action),
            Stage::Firmware => self.firmware_action(action),
        }
    }

    /// Show a progress line and log it.
    pub(super) fn say(&mut self, stage: Stage, msg: &str) {
        self.log.record(&format!("[{}] {msg}", stage.name()));
        self.fe.progress(stage.name(), msg);
    }

    /// Show a warning and log it.
    pub(super) fn warn(&mut self, msg: &str) {
        self.log.record(&format!("[warning] {msg}"));
        self.fe.message(&format!("Warning: {msg}"));
    }

    /// Run an external tool.
    pub(super) fn tool(
        &self,
        program: &str,
        args: &[&dyn AsRef<OsStr>],
    ) -> Result<(), InstallError> {
        Ok(self.deps.runner.run(program, &argv(args))?)
    }

    /// Run an external tool and return its stdout.
    pub(super) fn tool_output(
        &self,
        program: &str,
        args: &[&dyn AsRef<OsStr>],
    ) -> Result<String, InstallError> {
        Ok(self.deps.runner.output(program, &argv(args))?)
    }
}
