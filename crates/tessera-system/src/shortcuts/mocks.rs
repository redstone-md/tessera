// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! In-memory control seam: never installs hooks, registers keys or reads input.
//! Explicit results/events may be stale to exercise consumer generation gates.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::{
    ShortcutCompletion, ShortcutConfig, ShortcutError, ShortcutErrorKind, ShortcutEvent,
    ShortcutHost, ShortcutSink, ShortcutSnapshot, ShortcutSubscription,
};

const CAPACITY: usize = 16;
const HISTORY_CAPACITY: usize = 128;

#[derive(Clone, Default)]
pub struct RecordingShortcutHost {
    inner: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    closed: bool,
    generation: u64,
    configurations: VecDeque<ShortcutConfig>,
    pending: VecDeque<PendingShortcutCompletion>,
    reject_next: Option<ShortcutError>,
    inline_next: Option<Result<ShortcutSnapshot, ShortcutError>>,
    listeners: Vec<Arc<Listener>>,
}

struct Listener {
    enabled: AtomicBool,
    sink: ShortcutSink,
}

struct Guard(Arc<Listener>);

impl ShortcutSubscription for Guard {}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.enabled.store(false, Ordering::Release);
    }
}

/// Owns one accepted callback. A taken callback can finish after host close.
/// Dropping it without explicit delivery reports cancellation exactly once.
pub struct PendingShortcutCompletion {
    config: ShortcutConfig,
    generation: u64,
    completion: Option<ShortcutCompletion>,
}

impl PendingShortcutCompletion {
    pub fn config(&self) -> &ShortcutConfig {
        &self.config
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn complete(mut self, result: Result<ShortcutSnapshot, ShortcutError>) {
        if let Some(completion) = self.completion.take() {
            deliver(completion, result);
        }
    }

    /// Explicitly simulates successful registration of this accepted request.
    pub fn complete_registered(self) {
        let snapshot = ShortcutSnapshot::new(&self.config, self.generation, Ok(()), Ok(()));
        self.complete(Ok(snapshot));
    }
}

impl Drop for PendingShortcutCompletion {
    fn drop(&mut self) {
        if let Some(completion) = self.completion.take() {
            deliver(completion, Err(unavailable()));
        }
    }
}

fn deliver(completion: ShortcutCompletion, result: Result<ShortcutSnapshot, ShortcutError>) {
    // One panicking test consumer must not abandon other accepted completions.
    let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
}

fn unavailable() -> ShortcutError {
    ShortcutError::new(
        ShortcutErrorKind::Busy,
        None,
        "The recording host is closed or full",
    )
}

impl RecordingShortcutHost {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn reject_next(&self, error: ShortcutError) {
        self.state().reject_next = Some(error);
    }

    /// The next accepted configuration completes before configure returns.
    /// A rejected request does not consume this armed inline response.
    pub fn inline_next(&self, result: Result<ShortcutSnapshot, ShortcutError>) {
        self.state().inline_next = Some(result);
    }

    /// Bounded history of accepted typed configurations, oldest first.
    pub fn configurations(&self) -> Vec<ShortcutConfig> {
        self.state().configurations.iter().cloned().collect()
    }

    pub fn pending_count(&self) -> usize {
        self.state().pending.len()
    }

    pub fn take_next_completion(&self) -> Option<PendingShortcutCompletion> {
        self.state().pending.pop_front()
    }

    pub fn complete_next(&self, result: Result<ShortcutSnapshot, ShortcutError>) -> bool {
        let Some(pending) = self.take_next_completion() else {
            return false;
        };
        pending.complete(result);
        true
    }

    /// Simulates delivery, including intentionally late/stale native events.
    /// Disabled guards cannot admit new callbacks; admitted callbacks may finish.
    pub fn emit(&self, event: ShortcutEvent) {
        let listeners = {
            let state = self.state();
            if state.closed {
                return;
            }
            state
                .listeners
                .iter()
                .filter(|listener| listener.enabled.load(Ordering::Acquire))
                .cloned()
                .collect::<Vec<_>>()
        };
        for listener in listeners {
            if listener.enabled.load(Ordering::Acquire) {
                let _ = catch_unwind(AssertUnwindSafe(|| (listener.sink)(event.clone())));
            }
        }
    }

    /// Disables event admission first, then cancels queued accepted requests.
    /// Transferred pending handles deliberately remain deliverable afterwards.
    pub fn close(&self) {
        let (listeners, pending) = {
            let mut state = self.state();
            if state.closed {
                return;
            }
            state.closed = true;
            for listener in &state.listeners {
                listener.enabled.store(false, Ordering::Release);
            }
            state.inline_next = None;
            state.reject_next = None;
            (
                std::mem::take(&mut state.listeners),
                std::mem::take(&mut state.pending),
            )
        };
        // Callback/sink destruction and callback delivery are outside the lock.
        drop(listeners);
        drop(pending);
    }
}

impl ShortcutHost for RecordingShortcutHost {
    fn configure(
        &self,
        config: ShortcutConfig,
        completion: ShortcutCompletion,
    ) -> Result<(), ShortcutError> {
        config
            .effective_chord(super::ShortcutAction::OpenSettings)
            .validate()?;
        let inline = {
            let mut state = self.state();
            if state.closed {
                return Err(unavailable());
            }
            if let Some(error) = state.reject_next.take() {
                return Err(error);
            }
            if state.pending.len() >= CAPACITY && state.inline_next.is_none() {
                return Err(unavailable());
            }
            let Some(generation) = state.generation.checked_add(1) else {
                return Err(unavailable());
            };
            state.generation = generation;
            if state.configurations.len() == HISTORY_CAPACITY {
                state.configurations.pop_front();
            }
            state.configurations.push_back(config.clone());
            let pending = PendingShortcutCompletion {
                config,
                generation,
                completion: Some(completion),
            };
            if let Some(result) = state.inline_next.take() {
                Some((pending, result))
            } else {
                state.pending.push_back(pending);
                None
            }
        };
        if let Some((pending, result)) = inline {
            pending.complete(result);
        }
        Ok(())
    }

    fn subscribe(
        &self,
        sink: ShortcutSink,
    ) -> Result<Box<dyn ShortcutSubscription>, ShortcutError> {
        let mut state = self.state();
        if state.closed {
            return Err(unavailable());
        }
        let (live, retired): (Vec<_>, Vec<_>) = std::mem::take(&mut state.listeners)
            .into_iter()
            .partition(|listener| listener.enabled.load(Ordering::Acquire));
        state.listeners = live;
        if state.listeners.len() >= CAPACITY {
            drop(state);
            drop(retired);
            return Err(unavailable());
        }
        let listener = Arc::new(Listener {
            enabled: AtomicBool::new(true),
            sink,
        });
        state.listeners.push(listener.clone());
        drop(state);
        drop(retired);
        Ok(Box::new(Guard(listener)))
    }
}

impl Drop for RecordingShortcutHost {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_exhaustion_rejects_without_wrapping_or_completion() {
        let host = RecordingShortcutHost::new();
        host.state().generation = u64::MAX;
        let called = Arc::new(AtomicBool::new(false));
        let callback_called = called.clone();
        assert!(
            host.configure(
                ShortcutConfig::default(),
                Box::new(move |_| {
                    callback_called.store(true, Ordering::Release);
                })
            )
            .is_err()
        );
        assert!(!called.load(Ordering::Acquire));
        assert!(host.configurations().is_empty());
        assert_eq!(host.state().generation, u64::MAX);
    }
}
