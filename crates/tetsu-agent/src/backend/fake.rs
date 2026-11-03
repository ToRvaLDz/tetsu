//! A scripted backend for tests of the core and of the adapters (#79/#80):
//! it answers each `send` with the next list of events and records what it
//! was given. It calls no provider.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::{Backend, BackendError, Event, Flags, Input, SessionConfig, ToolSpec};

/// What a [`FakeBackend`] was given.
#[derive(Debug, Default)]
pub struct Recorded {
    /// Every `send`, in order.
    pub inputs: Vec<Input>,
    /// The config and tools of the last `start`.
    pub started: Option<(SessionConfig, Vec<ToolSpec>)>,
    /// How many times `start` ran.
    pub starts: usize,
    /// How many times `cancel` ran.
    pub cancels: usize,
}

/// The scripted backend.
#[derive(Debug)]
pub struct FakeBackend {
    flags: Flags,
    script: VecDeque<Vec<Event>>,
    current: VecDeque<Event>,
    recorded: Rc<RefCell<Recorded>>,
}

impl FakeBackend {
    /// A backend that answers the n-th `send` with the n-th list of events.
    pub fn new(script: Vec<Vec<Event>>) -> Self {
        Self {
            flags: Flags::default(),
            script: script.into(),
            current: VecDeque::new(),
            recorded: Rc::default(),
        }
    }

    /// The same backend with other flags.
    pub fn with_flags(self, flags: Flags) -> Self {
        Self { flags, ..self }
    }

    /// A handle to what the backend is given, readable after the backend
    /// moved into an agent.
    pub fn recorder(&self) -> Rc<RefCell<Recorded>> {
        self.recorded.clone()
    }
}

impl Backend for FakeBackend {
    fn flags(&self) -> Flags {
        self.flags
    }

    async fn start(
        &mut self,
        config: &SessionConfig,
        tools: &[ToolSpec],
    ) -> Result<(), BackendError> {
        let mut recorded = self.recorded.borrow_mut();
        recorded.starts += 1;
        recorded.started = Some((config.clone(), tools.to_vec()));
        Ok(())
    }

    async fn send(&mut self, input: Input) -> Result<(), BackendError> {
        self.recorded.borrow_mut().inputs.push(input);
        let events = self.script.pop_front().ok_or_else(|| BackendError {
            message: "the script has no answer for this send".to_owned(),
            retryable: false,
        })?;
        self.current = events.into();
        Ok(())
    }

    async fn next_event(&mut self) -> Option<Event> {
        self.current.pop_front()
    }

    async fn cancel(&mut self) {
        self.recorded.borrow_mut().cancels += 1;
        self.current.clear();
    }
}
