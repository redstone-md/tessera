// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{FolderError, MessageCalls, PeekOptions, WaitOptions, Wake};
use parking_lot::{Condvar, Mutex};
use std::collections::VecDeque;
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::folders) enum Event {
    Signal,
    Disconnected,
    Wait(WaitOptions),
    Peek(PeekOptions),
    Translate(u32, bool),
    Dispatch(u32, isize),
    Initialized(ThreadId),
    Resolve(ThreadId),
    TargetDrop(ThreadId),
    Uninitialize(ThreadId),
    Completion,
    Close,
}

#[derive(Clone, Default)]
pub(in crate::folders) struct Trace(Arc<(Mutex<Vec<Event>>, Condvar)>);

impl Trace {
    pub(in crate::folders) fn push(&self, event: Event) {
        self.0.0.lock().push(event);
        self.0.1.notify_all();
    }
    pub(in crate::folders) fn events(&self) -> Vec<Event> {
        self.0.0.lock().clone()
    }
    pub(in crate::folders) fn until(&self, predicate: impl Fn(&[Event]) -> bool) {
        let mut events = self.0.0.lock();
        let timeout = self
            .0
            .1
            .wait_while_for(&mut events, |events| !predicate(events), DEADLINE);
        assert!(
            !timeout.timed_out() || predicate(&events),
            "recording trace deadline"
        );
    }
}

#[derive(Default)]
struct State {
    signaled: bool,
    waiting: bool,
    hold: bool,
    safety_timeouts: usize,
    messages: VecDeque<u32>,
    wait_result: Option<Result<u32, FolderError>>,
    signal_failure: Option<FolderError>,
    before_wait: Option<Box<dyn FnOnce() + Send>>,
    during_peek: Option<Box<dyn FnOnce() + Send>>,
    during_signal: Option<Box<dyn FnOnce() + Send>>,
}

#[derive(Default)]
pub(in crate::folders) struct Recording {
    pub(in crate::folders) trace: Trace,
    state: Mutex<State>,
    changed: Condvar,
}

impl Recording {
    pub(in crate::folders) fn hold(&self) {
        self.state.lock().hold = true;
    }
    pub(in crate::folders) fn resume(&self) {
        self.state.lock().hold = false;
        self.changed.notify_all();
    }
    pub(in crate::folders) fn parked(&self) {
        let mut state = self.state.lock();
        let timeout = self
            .changed
            .wait_while_for(&mut state, |state| !state.waiting, DEADLINE);
        assert!(
            !timeout.timed_out() || state.waiting,
            "recording idle deadline"
        );
    }
    pub(in crate::folders) fn safety_timeout(&self) {
        self.state.lock().safety_timeouts += 1;
        self.changed.notify_all();
    }
    pub(in crate::folders) fn fail_signal(&self, error: FolderError) {
        self.state.lock().signal_failure = Some(error);
    }
    pub(in crate::folders) fn wait_result(&self, result: Result<u32, FolderError>) {
        self.state.lock().wait_result = Some(result);
        self.changed.notify_all();
    }
    pub(in crate::folders) fn messages(&self, messages: impl IntoIterator<Item = u32>) {
        self.state.lock().messages.extend(messages);
        self.changed.notify_all();
    }
    pub(in crate::folders) fn before_wait(&self, hook: impl FnOnce() + Send + 'static) {
        self.state.lock().before_wait = Some(Box::new(hook));
    }
    pub(in crate::folders) fn during_peek(&self, hook: impl FnOnce() + Send + 'static) {
        self.state.lock().during_peek = Some(Box::new(hook));
    }
    pub(in crate::folders) fn during_signal(&self, hook: impl FnOnce() + Send + 'static) {
        self.state.lock().during_signal = Some(Box::new(hook));
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.trace.push(Event::Close);
    }
}

impl Wake for Recording {
    fn signal(&self) -> Result<(), FolderError> {
        let hook = self.state.lock().during_signal.take();
        if let Some(hook) = hook {
            hook();
        }
        self.trace.push(Event::Signal);
        let mut state = self.state.lock();
        if let Some(error) = &state.signal_failure {
            return Err(error.clone());
        }
        // AUTO-reset persistence/coalescing, not a counting semaphore.
        state.signaled = true;
        self.changed.notify_all();
        Ok(())
    }
}

pub(in crate::folders) struct Calls {
    recording: Arc<Recording>,
}

impl Calls {
    pub(in crate::folders) fn new(recording: Arc<Recording>) -> Self {
        Self { recording }
    }
}

impl MessageCalls for Calls {
    type Message = u32;

    fn wait(&mut self, options: WaitOptions) -> Result<u32, FolderError> {
        let recording = &self.recording;
        recording.trace.push(Event::Wait(options));
        let hook = recording.state.lock().before_wait.take();
        if let Some(hook) = hook {
            hook();
        }
        let mut state = recording.state.lock();
        while state.hold
            || (!state.signaled
                && state.messages.is_empty()
                && state.wait_result.is_none()
                && state.safety_timeouts == 0)
        {
            state.waiting = true;
            recording.changed.notify_all();
            let timeout = recording.changed.wait_for(&mut state, DEADLINE);
            assert!(!timeout.timed_out(), "recording wait must be released");
        }
        state.waiting = false;
        if let Some(result) = state.wait_result.take() {
            return result;
        }
        if !state.messages.is_empty() {
            return Ok(1);
        }
        if state.safety_timeouts > 0 {
            state.safety_timeouts -= 1;
            return Ok(258);
        }
        assert!(std::mem::take(&mut state.signaled));
        Ok(0)
    }

    fn peek(&mut self, options: PeekOptions) -> Option<u32> {
        self.recording.trace.push(Event::Peek(options));
        let hook = self.recording.state.lock().during_peek.take();
        // Models nonqueued reentry even when Peek returns no posted message.
        // No recording lock is held while Rust callbacks run.
        if let Some(hook) = hook {
            hook();
        }
        self.recording.state.lock().messages.pop_front()
    }

    fn message_id(message: &u32) -> u32 {
        *message
    }
    fn translate(&mut self, message: &u32) -> bool {
        self.recording.trace.push(Event::Translate(*message, false));
        false
    }
    fn dispatch(&mut self, message: &u32) -> isize {
        self.recording.trace.push(Event::Dispatch(*message, 0));
        0
    }
}
