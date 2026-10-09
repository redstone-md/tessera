// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Typed draft editing and asynchronous native shortcut authority. The parent
//! alone saves its complete preferences and then calls `apply_saved`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use parking_lot::Mutex;
use slint::language::KeyEvent;
use tessera_system::shortcuts::{
    KeyChord, ShortcutAction, ShortcutBindingState, ShortcutConfig, ShortcutError,
    ShortcutErrorKind, ShortcutEvent, ShortcutHost, ShortcutSnapshot, ShortcutSubscription,
    ShortcutTrigger,
};

mod capture;
mod mailbox;
#[cfg(test)]
mod reentry_tests;
#[cfg(test)]
mod tests;
use capture::{Capture, CaptureResult};
use mailbox::{Mailbox, Wake};

type Factory = Rc<dyn Fn() -> Result<Option<Arc<dyn ShortcutHost>>, ShortcutError>>;

/// Callbacks must capture weak parent/controller handles. `wake` queues an
/// event-loop callback (never calls `process_pending` inline). `is_current`
/// checks the parent's weak Root and active application scope, not popup visibility.
/// `deliver` routes the closed action enum into the existing launcher/settings.
pub(crate) struct ShortcutsBindings {
    pub factory: Factory,
    pub is_current: Rc<dyn Fn() -> bool>,
    pub project: Rc<dyn Fn(ShortcutsView)>,
    pub deliver: Rc<dyn Fn(ShortcutTrigger)>,
    pub wake: Wake,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutsView {
    pub enabled: bool,
    pub settings_label: String,
    pub capturing: bool,
    pub dirty: bool,
    pub pause_applied: bool,
    pub applying: bool,
    pub launcher_status: String,
    pub settings_status: String,
    pub message: String,
}

struct Flight {
    token: u64,
    revision: u64,
    config: ShortcutConfig,
}

struct State {
    saved: ShortcutConfig,
    draft: ShortcutConfig,
    capture: Capture,
    paused: bool,
    active: bool,
    revision: u64,
    next_token: u64,
    last_generation: u64,
    generation_provider: Option<std::sync::Weak<dyn ShortcutHost>>,
    factory_busy: bool,
    provider: Option<Arc<dyn ShortcutHost>>,
    subscription: Option<Box<dyn ShortcutSubscription>>,
    pending: Option<ShortcutConfig>,
    flight: Option<Flight>,
    snapshot: Option<ShortcutSnapshot>,
    message: String,
}

pub(crate) struct ShortcutsController {
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    bindings: ShortcutsBindings,
    checking_current: Cell<bool>,
    projecting: Cell<bool>,
}

impl ShortcutsController {
    /// No factory, subscription or native installation during construction.
    pub fn new(config: ShortcutConfig, bindings: ShortcutsBindings) -> Self {
        Self {
            state: RefCell::new(State {
                saved: config.clone(),
                draft: config,
                capture: Capture::default(),
                paused: false,
                active: false,
                revision: 0,
                next_token: 0,
                last_generation: 0,
                factory_busy: false,
                provider: None,
                generation_provider: None,
                subscription: None,
                pending: None,
                flight: None,
                snapshot: None,
                message: String::new(),
            }),
            mailbox: Arc::new(Mutex::new(Mailbox {
                alive: true,
                ..Mailbox::default()
            })),
            bindings,
            checking_current: Cell::new(false),
            projecting: Cell::new(false),
        }
    }

    pub fn draft_config(&self) -> ShortcutConfig {
        self.state.borrow().draft.clone()
    }

    pub fn view(&self) -> ShortcutsView {
        let state = self.state.borrow();
        let status = |action| {
            state.snapshot.as_ref().map_or_else(
                || "Not applied".into(),
                |snapshot| {
                    let binding = snapshot.binding(action);
                    format!(
                        "{} ({})",
                        binding_state(binding.state),
                        binding.chord.label()
                    )
                },
            )
        };
        ShortcutsView {
            enabled: state.draft.enabled(),
            settings_label: state
                .draft
                .effective_chord(ShortcutAction::OpenSettings)
                .label(),
            capturing: state.capture.active(),
            dirty: state.draft != state.saved,
            pause_applied: state.paused
                && state.flight.is_none()
                && state.pending.is_none()
                && state
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot.enabled),
            applying: state.flight.is_some() || state.pending.is_some() || state.factory_busy,
            launcher_status: status(ShortcutAction::ToggleLauncher),
            settings_status: status(ShortcutAction::OpenSettings),
            message: state.message.clone(),
        }
    }

    pub fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = CallbackGuard(&self.projecting);
        let revision = self.state.borrow().revision;
        if self.current() && self.state.borrow().revision == revision {
            (self.bindings.project)(self.view());
        }
    }

    /// Preferences-open/Cancel only. Does not change applied/native authority.
    pub fn reset_draft(&self, config: ShortcutConfig) {
        let mut state = self.state.borrow_mut();
        state.draft = config;
        state.capture.cancel();
        state.message.clear();
        drop(state);
        self.project();
    }

    pub fn set_draft_enabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.draft = state.draft.clone().with_enabled(enabled);
        if !enabled {
            state.capture.cancel();
        }
        state.message.clear();
        drop(state);
        self.project();
    }

    pub fn set_settings_override(&self, chord: Option<KeyChord>) -> Result<(), ShortcutError> {
        let mut state = self.state.borrow_mut();
        state.draft = state.draft.clone().with_settings_override(chord)?;
        state.capture.cancel();
        state.message.clear();
        drop(state);
        self.project();
        Ok(())
    }

    pub fn reset_settings(&self) {
        // Reset the source default, not a new display-derived configuration.
        let _ = self.set_settings_override(None);
    }

    pub fn begin_capture(&self) {
        let mut state = self.state.borrow_mut();
        if !state.draft.enabled() {
            return;
        }
        state.capture.begin();
        state.message.clear();
        drop(state);
        self.project();
    }

    pub fn cancel_capture(&self) {
        self.state.borrow_mut().capture.cancel();
        self.project();
    }

    /// True means the focused capture handled the event; Slint owns EventResult.
    pub fn key_pressed(&self, event: KeyEvent) -> bool {
        let result = self.state.borrow_mut().capture.pressed(&event);
        self.capture_result(result)
    }

    pub fn key_released(&self, event: KeyEvent) -> bool {
        let result = self.state.borrow_mut().capture.released(&event);
        self.capture_result(result)
    }

    fn capture_result(&self, result: CaptureResult) -> bool {
        if matches!(result, CaptureResult::Ignored) {
            return false;
        }
        match result {
            CaptureResult::Chord(chord) => {
                if let Err(error) = self.set_settings_override(Some(chord)) {
                    self.state.borrow_mut().message = error_message(&error).into();
                }
            }
            CaptureResult::Invalid(error) => {
                self.state.borrow_mut().message = error_message(&error).into();
            }
            _ => {}
        }
        self.project();
        true
    }

    /// Invoke only after the parent's *complete* Save transaction succeeds (or
    /// once at admitted non-diagnostic startup). Draft editing never gets here.
    /// Finishes capture and unpauses atomically with the NEW saved authority:
    /// callers must not issue a separate unpause before or after a complete Save.
    pub fn apply_saved(&self, config: ShortcutConfig) {
        {
            let mut state = self.state.borrow_mut();
            state.saved = config.clone();
            state.draft = config;
            state.capture.cancel();
            state.paused = false;
            state.active = true;
        }
        self.request_configuration();
    }

    /// Explicit temporary native pause, independent of the draft enabled toggle.
    /// Parent may use this around focused capture; never persists pause state.
    pub fn set_paused(&self, paused: bool) {
        {
            let mut state = self.state.borrow_mut();
            if state.paused == paused {
                return;
            }
            state.paused = paused;
        }
        self.request_configuration();
    }

    fn request_configuration(&self) {
        // Revoke the UI delivery guard BEFORE any factory/subscribe/configure
        // callback, including synchronous/reentrant provider implementations.
        self.mailbox.lock().retire_delivery();
        let mut state = self.state.borrow_mut();
        state.snapshot = None;
        state.message.clear();
        let Some(revision) = state.revision.checked_add(1) else {
            state.active = false;
            state.pending = None;
            state.message = "Shortcut scope is exhausted.".into();
            return;
        };
        state.revision = revision;
        if state.active {
            state.pending = Some(
                state
                    .saved
                    .clone()
                    .with_enabled(state.saved.enabled() && !state.paused),
            );
        }
        drop(state);
        mailbox::notify(&self.mailbox, &self.bindings.wake);
        self.pump();
        self.project();
    }

    fn current(&self) -> bool {
        if self.checking_current.replace(true) {
            return false;
        }
        let _guard = CallbackGuard(&self.checking_current);
        (self.bindings.is_current)()
    }

    fn admitted(&self, revision: u64) -> bool {
        // Evaluate external predicate without any RefCell borrow; it can reenter.
        let current = self.current();
        let state = self.state.borrow();
        current && state.active && state.revision == revision
    }

    fn pump(&self) {
        let (revision, provider) = {
            let state = self.state.borrow();
            if !state.active
                || state.factory_busy
                || state.flight.is_some()
                || state.pending.is_none()
            {
                return;
            }
            (state.revision, state.provider.clone())
        };
        if !self.admitted(revision) {
            return;
        }
        let provider = match provider {
            Some(provider) => provider,
            None => {
                self.state.borrow_mut().factory_busy = true;
                let result = (self.bindings.factory)();
                self.state.borrow_mut().factory_busy = false;
                if !self.admitted(revision) {
                    // Reentry cannot install a provider for an abandoned scope.
                    mailbox::notify(&self.mailbox, &self.bindings.wake);
                    return;
                }
                match result {
                    Ok(Some(provider)) => provider,
                    Ok(None) => {
                        self.fail_pending("Shortcuts are unavailable on this host.");
                        return;
                    }
                    Err(error) => {
                        self.fail_pending(error_message(&error));
                        return;
                    }
                }
            }
        };
        if self.state.borrow().subscription.is_none() {
            let source = {
                let mut mailbox = self.mailbox.lock();
                let Some(source) = mailbox.source.checked_add(1) else {
                    drop(mailbox);
                    self.cancel_current();
                    return;
                };
                mailbox.source = source;
                source
            };
            self.state.borrow_mut().factory_busy = true;
            let mailbox = self.mailbox.clone();
            let wake = self.bindings.wake.clone();
            let guard = provider.subscribe(Arc::new(move |event| {
                mailbox::event(&mailbox, &wake, source, event);
            }));
            self.state.borrow_mut().factory_busy = false;
            if !self.admitted(revision) {
                drop(guard); // delivery already retired before native cleanup
                mailbox::notify(&self.mailbox, &self.bindings.wake);
                return;
            }
            match guard {
                Ok(guard) => self.state.borrow_mut().subscription = Some(guard),
                Err(error) => {
                    self.fail_pending(error_message(&error));
                    return;
                }
            }
        }
        if !self.admitted(revision) {
            return;
        }
        let (token, config) = {
            let mut state = self.state.borrow_mut();
            let Some(token) = state.next_token.checked_add(1) else {
                drop(state);
                self.cancel_current();
                return;
            };
            let Some(config) = state.pending.take() else {
                return;
            };
            state.next_token = token;
            let identity = Arc::downgrade(&provider);
            if !state
                .generation_provider
                .as_ref()
                .is_some_and(|old| old.ptr_eq(&identity))
            {
                state.last_generation = 0;
                state.generation_provider = Some(identity);
            }
            state.provider = Some(provider.clone());
            state.flight = Some(Flight {
                token,
                revision,
                config: config.clone(),
            });
            (token, config)
        };
        self.mailbox.lock().expected = Some(token);
        let mailbox = self.mailbox.clone();
        let wake = self.bindings.wake.clone();
        // Persistent flight is installed first: inline completion and reentry are
        // exactly the same as a delayed provider completion. No UI thread join.
        let result = provider.configure(
            config,
            Box::new(move |result| {
                mailbox::complete(&mailbox, &wake, token, result);
            }),
        );
        if let Err(error) = result {
            mailbox::complete(&self.mailbox, &self.bindings.wake, token, Err(error));
        }
    }

    fn fail_pending(&self, message: &str) {
        let mut state = self.state.borrow_mut();
        state.pending = None;
        state.message = message.into();
    }

    /// Parent's weak event-loop wake calls this on the UI thread. At most one
    /// completion and eight events are consumed; stale events are never replayed.
    pub fn process_pending(&self) {
        let delivery_revision = self.state.borrow().revision;
        let (completion, events) = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            (
                mailbox.completion.take(),
                std::mem::take(&mut mailbox.events),
            )
        };
        if let Some((token, result)) = completion {
            let flight = {
                let mut state = self.state.borrow_mut();
                if state.flight.as_ref().map(|flight| flight.token) == Some(token) {
                    state.flight.take()
                } else {
                    None
                }
            };
            if let Some(flight) = flight {
                self.mailbox.lock().expected = None;
                if self.admitted(flight.revision) {
                    match result {
                        Ok(snapshot)
                            if valid_snapshot(
                                &snapshot,
                                &flight.config,
                                self.state.borrow().last_generation,
                            ) =>
                        {
                            let generation = snapshot.generation;
                            let enabled = snapshot.enabled;
                            {
                                let mut state = self.state.borrow_mut();
                                state.last_generation = generation;
                                state.snapshot = Some(snapshot);
                            }
                            if enabled {
                                self.mailbox.lock().generation = Some(generation);
                            }
                        }
                        Ok(_) => self.fail_pending("Shortcut provider returned invalid state."),
                        Err(error) => self.fail_pending(error_message(&error)),
                    }
                }
            }
        }
        for event in events {
            match event {
                ShortcutEvent::Triggered(trigger) => {
                    if self.accepts_trigger_at(&trigger, delivery_revision) {
                        (self.bindings.deliver)(trigger);
                    }
                }
                ShortcutEvent::Unavailable(error) => {
                    if !self.admitted(delivery_revision) {
                        continue;
                    }
                    self.mailbox.lock().retire_delivery();
                    let mut state = self.state.borrow_mut();
                    state.snapshot = None;
                    state.message = error_message(&error).into();
                }
            }
        }
        self.pump();
        self.project();
    }

    /// Revalidate an admitted intent after asynchronous parent display lookup.
    /// No RefCell borrow spans the callback-capable current-scope predicate.
    pub fn accepts_trigger(&self, trigger: &ShortcutTrigger) -> bool {
        let revision = self.state.borrow().revision;
        self.accepts_trigger_at(trigger, revision)
    }

    fn accepts_trigger_at(&self, trigger: &ShortcutTrigger, revision: u64) -> bool {
        if !self.admitted(revision) {
            return false;
        }
        let registered = self
            .state
            .borrow()
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.enabled
                    && snapshot.generation == trigger.generation
                    && snapshot.binding(trigger.action).state == ShortcutBindingState::Registered
            });
        let mailbox = self.mailbox.lock();
        registered && mailbox.alive && mailbox.generation == Some(trigger.generation)
    }

    /// Synchronously revoke delivery before dropping any native listener/host.
    /// An accepted configuration still owns its one flight until completion;
    /// reopen/apply cannot issue a second configure while that flight is pending.
    pub fn cancel_current(&self) {
        self.mailbox.lock().retire_delivery();
        let (guard, provider) = {
            let mut state = self.state.borrow_mut();
            state.active = false;
            state.pending = None;
            state.capture.cancel();
            state.snapshot = None;
            (state.subscription.take(), state.provider.take())
        };
        drop(guard);
        drop(provider);
    }
}

impl Drop for ShortcutsController {
    fn drop(&mut self) {
        self.mailbox.lock().close();
        let state = self.state.get_mut();
        drop(state.subscription.take());
        drop(state.provider.take());
    }
}

struct CallbackGuard<'a>(&'a Cell<bool>);

impl Drop for CallbackGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

fn valid_snapshot(snapshot: &ShortcutSnapshot, config: &ShortcutConfig, last: u64) -> bool {
    snapshot.generation > last
        && snapshot.enabled == config.enabled()
        && snapshot.bindings[0].action != snapshot.bindings[1].action
        && [ShortcutAction::ToggleLauncher, ShortcutAction::OpenSettings]
            .into_iter()
            .all(|action| {
                let binding = snapshot.binding(action);
                binding.chord == config.effective_chord(action)
                    && (config.enabled() || binding.state == ShortcutBindingState::Paused)
                    && (binding.state != ShortcutBindingState::Registered
                        || binding.error.is_none())
            })
}

fn binding_state(state: ShortcutBindingState) -> &'static str {
    match state {
        ShortcutBindingState::Registered => "Registered",
        ShortcutBindingState::Paused => "Paused",
        ShortcutBindingState::Conflict => "Conflict",
        ShortcutBindingState::Denied => "Access denied",
        ShortcutBindingState::Unavailable => "Unavailable",
    }
}

fn error_message(error: &ShortcutError) -> &'static str {
    match error.kind() {
        ShortcutErrorKind::Unsupported => "Shortcuts are unavailable on this host.",
        ShortcutErrorKind::AccessDenied => "Shortcut registration was denied.",
        ShortcutErrorKind::Conflict => "This shortcut conflicts with another binding.",
        ShortcutErrorKind::InvalidData => "This key combination is not supported.",
        ShortcutErrorKind::Busy => {
            "Shortcut registration is busy. Try again after applying finishes."
        }
        ShortcutErrorKind::Other => "Shortcut registration failed.",
    }
}
