//! Scripted fake frontend for unit tests.

use std::collections::VecDeque;

use crate::error::InstallError;
use crate::frontend::{Frontend, Validate};

#[derive(Debug)]
enum Event {
    Text(String),
    Secret(String),
    Pick(usize),
    Yes(bool),
}

/// Replays queued answers in order and records what the core showed.
/// Running out of answers, or answering the wrong kind of prompt, is a
/// test failure.
#[derive(Default)]
pub struct Script {
    queue: VecDeque<Event>,
    pub messages: Vec<String>,
    pub qrs: Vec<String>,
    pub progress: Vec<(String, String)>,
    pub prompts: Vec<String>,
    /// Options offered by every `choose`, in order.
    pub options: Vec<Vec<String>>,
}

impl Script {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, s: &str) -> Self {
        self.queue.push_back(Event::Text(s.to_owned()));
        self
    }

    pub fn secret(mut self, s: &str) -> Self {
        self.queue.push_back(Event::Secret(s.to_owned()));
        self
    }

    pub fn pick(mut self, i: usize) -> Self {
        self.queue.push_back(Event::Pick(i));
        self
    }

    pub fn yes(mut self, b: bool) -> Self {
        self.queue.push_back(Event::Yes(b));
        self
    }

    pub fn assert_done(&self) {
        assert!(self.queue.is_empty(), "unused answers: {:?}", self.queue);
    }

    fn next(&mut self, prompt: &str) -> Event {
        self.prompts.push(prompt.to_owned());
        self.queue
            .pop_front()
            .unwrap_or_else(|| panic!("script exhausted at prompt {prompt:?}"))
    }
}

impl Frontend for Script {
    fn ask(&mut self, prompt: &str, _validate: Validate<'_>) -> Result<String, InstallError> {
        match self.next(prompt) {
            Event::Text(s) => Ok(s),
            e => panic!("prompt {prompt:?} got {e:?}"),
        }
    }

    fn ask_secret(&mut self, prompt: &str) -> Result<String, InstallError> {
        match self.next(prompt) {
            Event::Secret(s) => Ok(s),
            e => panic!("secret prompt {prompt:?} got {e:?}"),
        }
    }

    fn choose(&mut self, prompt: &str, options: &[String]) -> Result<usize, InstallError> {
        self.options.push(options.to_vec());
        match self.next(prompt) {
            Event::Pick(i) => Ok(i),
            e => panic!("choose {prompt:?} got {e:?}"),
        }
    }

    fn confirm(&mut self, prompt: &str) -> Result<bool, InstallError> {
        match self.next(prompt) {
            Event::Yes(b) => Ok(b),
            e => panic!("confirm {prompt:?} got {e:?}"),
        }
    }

    fn progress(&mut self, step: &str, msg: &str) {
        self.progress.push((step.to_owned(), msg.to_owned()));
    }

    fn message(&mut self, text: &str) {
        self.messages.push(text.to_owned());
    }

    fn show_qr(&mut self, payload: &str) -> Result<(), InstallError> {
        self.qrs.push(payload.to_owned());
        Ok(())
    }
}
