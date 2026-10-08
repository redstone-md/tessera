// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Aggregate state, fixed open and explicit confirmed mutation, independent of desktop observation.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_system::dock_utilities::DockUtilityErrorKind;
use tessera_system::recycle_bin::{
    RecycleBinHost, RecycleBinInfo, RecycleBinWatchEvent, RecycleBinWatchGuard,
};
use tessera_system::recycle_bin_mutation::{RecycleBinEmptyOutcome, RecycleBinMutationHost};

use crate::DesktopHost;
use crate::generated::{Dock, DockRecycleAction, DockRecycleState};

const CHANGE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Token(u64);

struct Flight {
    token: Token,
    dispatched: bool,
}

#[derive(Default)]
struct State {
    sequence: u64,
    visible: bool,
    provider: Option<Arc<dyn RecycleBinHost>>,
    acquiring: Option<Token>,
    acquire_needed: bool,
    mutation_provider: Option<Arc<dyn RecycleBinMutationHost>>,
    mutation_acquiring: Option<Token>,
    watch_needed: bool,
    watch: Option<Token>,
    ready_pending: bool,
    watch_live: bool,
    watch_guard: Option<Box<dyn RecycleBinWatchGuard>>,
    read: Option<Token>,
    open: Option<Flight>,
    empty: Option<Flight>,
    empty_result: Option<Result<RecycleBinEmptyOutcome, DockUtilityErrorKind>>,
    // A pre-return read retires normally, but cannot satisfy a mutation's readback.
    obsolete_read: Option<Token>,
    readback_needed: bool,
    read_needed: bool,
    dirty: bool,
    // A failed read retains its invalidation, but never becomes an idle retry loop.
    automatic_read_blocked: bool,
    timer: Option<Token>,
    throttle_elapsed: bool,
    snapshot: Option<RecycleBinInfo>,
    read_error: Option<DockUtilityErrorKind>,
    watch_error: Option<DockUtilityErrorKind>,
    open_error: Option<DockUtilityErrorKind>,
    open_succeeded: bool,
}

impl State {
    fn next_token(&mut self) -> Option<Token> {
        self.sequence = self.sequence.checked_add(1)?;
        Some(Token(self.sequence))
    }
}

type ResultSlot<T> = Option<(Token, Result<T, DockUtilityErrorKind>)>;

struct EmptyReturn {
    token: Token,
    result: Result<RecycleBinEmptyOutcome, DockUtilityErrorKind>,
    read_cutoff: Option<Token>,
}

/// Slots are bounded independently; notifications carry only a dirty bit and one safe kind.
#[derive(Default)]
struct Mailbox {
    closed: bool,
    watch: Option<Token>,
    ready_received: bool,
    ready: ResultSlot<()>,
    unavailable: Option<(Token, DockUtilityErrorKind)>,
    dirty: bool,
    expected_read: Option<Token>,
    read: ResultSlot<RecycleBinInfo>,
    latest_read: Option<Token>,
    expected_open: Option<Token>,
    open: ResultSlot<()>,
    expected_empty: Option<Token>,
    empty: Option<EmptyReturn>,
    wake_queued: bool,
}

impl Mailbox {
    fn pending(&self) -> bool {
        self.ready.is_some()
            || self.unavailable.is_some()
            || self.dirty
            || self.read.is_some()
            || self.open.is_some()
            || self.empty.is_some()
    }
}

fn publish(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    update: impl FnOnce(&mut Mailbox) -> bool,
) {
    {
        let mut mailbox = mailbox.lock();
        if mailbox.closed || !update(&mut mailbox) || mailbox.wake_queued {
            return;
        }
        mailbox.wake_queued = true;
    }
    let weak_mailbox = Arc::downgrade(mailbox);
    if dock
        .upgrade_in_event_loop(move |dock| {
            let Some(mailbox) = weak_mailbox.upgrade() else {
                return;
            };
            let current = {
                let mut mailbox = mailbox.lock();
                let current = !mailbox.closed && mailbox.wake_queued && mailbox.pending();
                mailbox.wake_queued = false;
                current
            };
            if current {
                dock.invoke_recycle_event_ready();
            }
        })
        .is_err()
    {
        // The no-event-loop backend drains this same root callback explicitly.
        mailbox.lock().wake_queued = false;
    }
}

fn ready(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    result: Result<(), DockUtilityErrorKind>,
) {
    publish(mailbox, dock, |mailbox| {
        if mailbox.watch != Some(token) || mailbox.ready_received {
            return false;
        }
        mailbox.ready_received = true;
        mailbox.ready = Some((token, result));
        true
    });
}

fn read_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    result: Result<RecycleBinInfo, DockUtilityErrorKind>,
) {
    publish(mailbox, dock, |mailbox| {
        if mailbox.expected_read != Some(token) || mailbox.read.is_some() {
            return false;
        }
        mailbox.read = Some((token, result));
        true
    });
}

fn open_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    result: Result<(), DockUtilityErrorKind>,
) {
    publish(mailbox, dock, |mailbox| {
        if mailbox.expected_open != Some(token) || mailbox.open.is_some() {
            return false;
        }
        mailbox.open = Some((token, result));
        true
    });
}

fn empty_complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    result: Result<RecycleBinEmptyOutcome, DockUtilityErrorKind>,
) {
    publish(mailbox, dock, |mailbox| {
        if mailbox.expected_empty != Some(token) || mailbox.empty.is_some() {
            return false;
        }
        // Keep the cutoff even if the old read's slot is drained before this return.
        mailbox.empty = Some(EmptyReturn {
            token,
            result,
            read_cutoff: mailbox.latest_read,
        });
        true
    });
}

fn watch_event(
    mailbox: &Arc<Mutex<Mailbox>>,
    dock: &slint::Weak<Dock>,
    token: Token,
    event: RecycleBinWatchEvent,
) {
    publish(mailbox, dock, |mailbox| {
        if mailbox.watch != Some(token) || mailbox.unavailable.is_some() {
            return false;
        }
        match event {
            RecycleBinWatchEvent::Invalidated => mailbox.dirty = true,
            RecycleBinWatchEvent::Unavailable(kind) => {
                mailbox.unavailable = Some((token, kind));
            }
        }
        true
    });
}

struct Driving<'a>(&'a Cell<bool>);

impl Drop for Driving<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

enum Effect {
    Acquire(Token),
    AcquireMutation(Token),
    Watch(Token, Arc<dyn RecycleBinHost>),
    Read(Token, Arc<dyn RecycleBinHost>),
    Open(Token, Arc<dyn RecycleBinHost>),
    Empty(Token, Arc<dyn RecycleBinMutationHost>),
    Timer(Token),
}

pub(crate) struct RecycleBinController {
    dock: slint::Weak<Dock>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    throttle: slint::Timer,
    driving: Cell<bool>,
    closed: Cell<bool>,
}

impl RecycleBinController {
    /// Construction preserves root callbacks and does not acquire, subscribe, read or show.
    pub(crate) fn new(host: Arc<dyn DesktopHost>, dock: &Dock) -> Rc<Self> {
        Rc::new(Self {
            dock: dock.as_weak(),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            throttle: slint::Timer::default(),
            driving: Cell::new(false),
            closed: Cell::new(false),
        })
    }

    /// Only the root's successful real placement starts a shown session.
    pub(crate) fn shown(self: &Rc<Self>) {
        if self.closed.get() || !self.dock_visible() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.visible {
                return;
            }
            state.visible = true;
            state.read_needed = state.read_needed
                || (state.read.is_none()
                    && (state.dirty || state.snapshot.is_none() || state.read_error.is_some()));
            state.automatic_read_blocked = false;
            state.acquire_needed = state.provider.is_none() && state.acquiring.is_none();
        }
        self.drive();
    }

    /// Passive watch and accepted effects survive hiding; undispatched intents never replay.
    pub(crate) fn hidden(self: &Rc<Self>) {
        if self.closed.get() {
            return;
        }
        let canceled_open = {
            let mut state = self.state.borrow_mut();
            state.visible = false;
            state.timer = None;
            state.throttle_elapsed = false;
            if state.open.as_ref().is_some_and(|flight| !flight.dispatched) {
                state.open.take().map(|flight| flight.token)
            } else {
                None
            }
        };
        if let Some(token) = canceled_open {
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected_open == Some(token) {
                mailbox.expected_open = None;
                mailbox.open = None;
            }
        }
        self.cancel_undispatched_empty();
        self.throttle.stop();
        if let Some(dock) = self.dock.upgrade() {
            dock.set_recycle_empty_enabled(false);
        }
    }

    fn cancel_undispatched_empty(&self) {
        let canceled = {
            let mut state = self.state.borrow_mut();
            state.mutation_acquiring = None;
            if state
                .empty
                .as_ref()
                .is_some_and(|flight| !flight.dispatched)
            {
                state.empty.take().map(|flight| flight.token)
            } else {
                None
            }
        };
        if let Some(token) = canceled {
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected_empty == Some(token) {
                mailbox.expected_empty = None;
                mailbox.empty = None;
            }
            drop(mailbox);
            if let Some(dock) = self.dock.upgrade() {
                dock.set_recycle_empty_busy(false);
                dock.set_recycle_empty_enabled(false);
            }
        }
    }

    pub(crate) fn request(self: &Rc<Self>, action: DockRecycleAction) {
        if self.closed.get() || !self.dock_visible() || !self.state.borrow().visible {
            return;
        }
        match action {
            DockRecycleAction::Open => {
                let token = {
                    let mut state = self.state.borrow_mut();
                    if state.open.is_some() {
                        return;
                    }
                    let Some(token) = state.next_token() else {
                        return;
                    };
                    state.open = Some(Flight {
                        token,
                        dispatched: false,
                    });
                    state.open_error = None;
                    state.open_succeeded = false;
                    state.acquire_needed = state.acquire_needed
                        || (state.provider.is_none() && state.acquiring.is_none());
                    token
                };
                let mut mailbox = self.mailbox.lock();
                mailbox.expected_open = Some(token);
                mailbox.open = None;
            }
            DockRecycleAction::Empty => {
                let token = {
                    let mut state = self.state.borrow_mut();
                    if state.empty.is_some() {
                        return;
                    }
                    let Some(token) = state.next_token() else {
                        return;
                    };
                    state.empty = Some(Flight {
                        token,
                        dispatched: false,
                    });
                    state.empty_result = None;
                    token
                };
                let mut mailbox = self.mailbox.lock();
                mailbox.expected_empty = Some(token);
                mailbox.empty = None;
            }
            DockRecycleAction::Retry => {
                let mut state = self.state.borrow_mut();
                state.acquire_needed =
                    state.acquire_needed || (state.provider.is_none() && state.acquiring.is_none());
                state.watch_needed = state.watch_needed || state.watch.is_none();
                state.read_needed = true;
                state.automatic_read_blocked = false;
            }
        }
        self.drive();
    }

    /// Read/watch/Empty render safe notices; only an explicit open returns root feedback.
    pub(crate) fn process_events(self: &Rc<Self>) -> Option<Result<(), String>> {
        if self.closed.get() {
            return None;
        }
        let (ready, unavailable, dirty, read, open, empty) = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            let read = mailbox.read.take();
            if read.is_some() {
                mailbox.expected_read = None;
            }
            let open = mailbox.open.take();
            if open.is_some() {
                mailbox.expected_open = None;
            }
            let empty = mailbox.empty.take();
            if empty.is_some() {
                mailbox.expected_empty = None;
            }
            (
                mailbox.ready.take(),
                mailbox.unavailable.take(),
                std::mem::take(&mut mailbox.dirty),
                read,
                open,
                empty,
            )
        };
        let mut retired_guard = None;
        let mut retired_watch = None;
        let mut open_result = None;
        let mut retire_readback_timer = false;
        {
            let mut state = self.state.borrow_mut();
            // Apply the return barrier before any read in the same drained batch.
            if let Some(returned) = empty
                && state
                    .empty
                    .as_ref()
                    .is_some_and(|flight| flight.token == returned.token)
            {
                state.empty = None;
                state.empty_result = Some(returned.result);
                state.obsolete_read = state.read.filter(|token| {
                    returned
                        .read_cutoff
                        .is_some_and(|cutoff| token.0 <= cutoff.0)
                });
                state.readback_needed = state.read.is_none() || state.obsolete_read.is_some();
                state.read_needed = state.read_needed || state.readback_needed;
                state.automatic_read_blocked = false;
                state.acquire_needed =
                    state.acquire_needed || (state.provider.is_none() && state.acquiring.is_none());
                if state.readback_needed {
                    state.dirty = true;
                    state.timer = None;
                    state.throttle_elapsed = false;
                    retire_readback_timer = true;
                }
            }
            if let Some((token, result)) = ready
                && state.watch == Some(token)
                && state.ready_pending
            {
                state.ready_pending = false;
                match result {
                    Ok(()) => {
                        state.watch_live = true;
                        state.watch_error = None;
                    }
                    Err(kind) => {
                        state.watch_live = false;
                        state.watch_error = Some(kind);
                        retired_watch = state.watch.take();
                        retired_guard = state.watch_guard.take();
                    }
                }
            }
            if let Some((token, kind)) = unavailable
                && state.watch == Some(token)
            {
                state.ready_pending = false;
                state.watch_live = false;
                state.watch_error = Some(kind);
                retired_watch = state.watch.take();
                retired_guard = state.watch_guard.take();
            }
            if dirty {
                state.dirty = true;
                if state.automatic_read_blocked {
                    state.throttle_elapsed = false;
                }
                state.automatic_read_blocked = false;
            }
            if let Some((token, result)) = read
                && state.read == Some(token)
            {
                state.read = None;
                if state.obsolete_read == Some(token) {
                    // Retire exactly once without projecting old info or blocking fresh readback.
                    state.obsolete_read = None;
                } else {
                    match result {
                        Ok(info) => {
                            state.snapshot = Some(info);
                            state.read_error = None;
                            state.automatic_read_blocked = false;
                        }
                        Err(kind) => {
                            state.read_error = Some(kind);
                            state.automatic_read_blocked = !state.dirty;
                            state.dirty = true;
                        }
                    }
                }
            }
            if let Some((token, result)) = open
                && state
                    .open
                    .as_ref()
                    .is_some_and(|flight| flight.token == token)
            {
                state.open = None;
                state.open_error = result.as_ref().err().copied();
                state.open_succeeded = result.is_ok();
                open_result = Some(result.map_err(|kind| notice("Open", kind)));
            }
        }
        if let Some(token) = retired_watch {
            let mut mailbox = self.mailbox.lock();
            if mailbox.watch == Some(token) {
                mailbox.watch = None;
                mailbox.ready = None;
                mailbox.unavailable = None;
                mailbox.dirty = false;
            }
        }
        if retire_readback_timer {
            self.throttle.stop();
        }
        // Guard drop is callback-capable. Retire its authority before releasing ownership.
        drop(retired_guard);
        self.drive();
        open_result
    }

    /// Retire all authority before timer/guard/provider destruction; never join native work.
    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        let (guard, provider, mutation_provider) = {
            let mut state = self.state.borrow_mut();
            state.visible = false;
            state.acquiring = None;
            state.mutation_acquiring = None;
            state.watch = None;
            state.ready_pending = false;
            state.watch_live = false;
            state.read = None;
            state.open = None;
            state.empty = None;
            state.empty_result = None;
            state.obsolete_read = None;
            state.readback_needed = false;
            state.read_needed = false;
            state.acquire_needed = false;
            state.timer = None;
            (
                state.watch_guard.take(),
                state.provider.take(),
                state.mutation_provider.take(),
            )
        };
        *self.mailbox.lock() = Mailbox {
            closed: true,
            ..Mailbox::default()
        };
        self.throttle.stop();
        if let Some(dock) = self.dock.upgrade() {
            dock.set_recycle_read_busy(false);
            dock.set_recycle_open_busy(false);
            dock.set_recycle_empty_busy(false);
            dock.set_recycle_empty_enabled(false);
        }
        drop(guard);
        drop(provider);
        drop(mutation_provider);
    }

    fn dock_visible(&self) -> bool {
        self.dock
            .upgrade()
            .is_some_and(|dock| dock.window().is_visible())
    }

    /// Callback reentry records demand; the outer driver dispatches it after callbacks return.
    fn drive(self: &Rc<Self>) {
        if self.closed.get() || self.driving.replace(true) {
            return;
        }
        let _driving = Driving(&self.driving);
        loop {
            if self.closed.get() || !self.dock_visible() {
                break;
            }
            let effect = {
                let mut state = self.state.borrow_mut();
                if !state.visible {
                    break;
                }
                self.next_effect(&mut state)
            };
            self.render();
            match effect {
                Some(Effect::Acquire(token)) => self.acquire(token),
                Some(Effect::AcquireMutation(token)) => self.acquire_mutation(token),
                Some(Effect::Watch(token, provider)) => self.start_watch(token, provider),
                Some(Effect::Read(token, provider)) => self.start_read(token, provider),
                Some(Effect::Open(token, provider)) => self.start_open(token, provider),
                Some(Effect::Empty(token, provider)) => self.start_empty(token, provider),
                Some(Effect::Timer(token)) => {
                    let weak = Rc::downgrade(self);
                    self.throttle
                        .start(slint::TimerMode::SingleShot, CHANGE_INTERVAL, move || {
                            let Some(controller) = weak.upgrade() else {
                                return;
                            };
                            if controller.closed.get() {
                                return;
                            }
                            {
                                let mut state = controller.state.borrow_mut();
                                if state.timer != Some(token) {
                                    return;
                                }
                                state.timer = None;
                                state.throttle_elapsed = true;
                            }
                            controller.drive();
                        });
                }
                None => break,
            }
        }
        self.render();
    }

    fn next_effect(&self, state: &mut State) -> Option<Effect> {
        // Mutation demand is independent of read/watch/Open capability and readiness.
        if let Some(flight) = state.empty.as_ref()
            && !flight.dispatched
        {
            if let Some(provider) = state.mutation_provider.as_ref() {
                return Some(Effect::Empty(flight.token, provider.clone()));
            }
            if state.mutation_acquiring.is_none() {
                let token = state.next_token()?;
                state.mutation_acquiring = Some(token);
                return Some(Effect::AcquireMutation(token));
            }
        }
        if state.provider.is_none() {
            if !state.acquire_needed || state.acquiring.is_some() {
                return None;
            }
            let token = state.next_token()?;
            state.acquiring = Some(token);
            state.acquire_needed = false;
            return Some(Effect::Acquire(token));
        }
        let provider = state.provider.as_ref()?.clone();
        if state.watch_needed && state.watch.is_none() {
            let token = state.next_token()?;
            state.watch_needed = false;
            state.watch = Some(token);
            state.ready_pending = true;
            state.watch_live = false;
            let mut mailbox = self.mailbox.lock();
            mailbox.watch = Some(token);
            mailbox.ready_received = false;
            mailbox.ready = None;
            mailbox.unavailable = None;
            mailbox.dirty = false;
            return Some(Effect::Watch(token, provider));
        }
        if let Some(flight) = state.open.as_mut()
            && !flight.dispatched
        {
            flight.dispatched = true;
            return Some(Effect::Open(flight.token, provider));
        }
        if (!state.ready_pending || state.readback_needed)
            && state.read.is_none()
            && (state.read_needed
                || state.readback_needed
                || (state.dirty && state.throttle_elapsed && !state.automatic_read_blocked))
        {
            let token = state.next_token()?;
            state.read = Some(token);
            state.read_needed = false;
            state.dirty = false;
            state.timer = None;
            state.throttle_elapsed = false;
            let mut mailbox = self.mailbox.lock();
            mailbox.expected_read = Some(token);
            mailbox.read = None;
            mailbox.dirty = false;
            return Some(Effect::Read(token, provider));
        }
        if state.readback_needed {
            // The obsolete accepted read must retire; no timer polls it or queues another.
            return None;
        }
        if state.dirty
            && !state.automatic_read_blocked
            && !state.throttle_elapsed
            && state.timer.is_none()
        {
            let token = state.next_token()?;
            state.timer = Some(token);
            return Some(Effect::Timer(token));
        }
        None
    }

    fn acquire(&self, token: Token) {
        let result = self
            .host
            .recycle_bin_host()
            .map_err(|error| error.kind)
            .and_then(|provider| provider.ok_or(DockUtilityErrorKind::Unsupported));
        if self.closed.get() || self.state.borrow().acquiring != Some(token) {
            return;
        }
        let failed_open = {
            let mut state = self.state.borrow_mut();
            state.acquiring = None;
            match result {
                Ok(provider) => {
                    state.provider = Some(provider);
                    state.watch_needed = true;
                    state.read_needed = true;
                    None
                }
                Err(kind) => {
                    state.read_needed = false;
                    state.readback_needed = false;
                    state.read_error = Some(kind);
                    state.watch_error = Some(kind);
                    state.automatic_read_blocked = true;
                    state.open.as_mut().map(|flight| {
                        flight.dispatched = true;
                        (flight.token, kind)
                    })
                }
            }
        };
        if let Some((token, kind)) = failed_open {
            open_complete(&self.mailbox, &self.dock, token, Err(kind));
        }
    }

    fn acquire_mutation(&self, token: Token) {
        if self.closed.get() || self.state.borrow().mutation_acquiring != Some(token) {
            return;
        }
        if !self.dock_visible() || !self.state.borrow().visible {
            self.cancel_undispatched_empty();
            return;
        }
        let result = self
            .host
            .recycle_bin_mutation_host()
            .map_err(|error| error.kind)
            .and_then(|provider| provider.ok_or(DockUtilityErrorKind::Unsupported));
        if self.closed.get() || self.state.borrow().mutation_acquiring != Some(token) {
            return;
        }
        if !self.dock_visible() || !self.state.borrow().visible {
            self.cancel_undispatched_empty();
            return;
        }
        let failed_empty = {
            let mut state = self.state.borrow_mut();
            state.mutation_acquiring = None;
            match result {
                Ok(provider) => {
                    state.mutation_provider = Some(provider);
                    None
                }
                Err(kind) => state.empty.as_mut().map(|flight| {
                    flight.dispatched = true;
                    (flight.token, kind)
                }),
            }
        };
        if let Some((token, kind)) = failed_empty {
            empty_complete(&self.mailbox, &self.dock, token, Err(kind));
        }
    }

    fn start_watch(&self, token: Token, provider: Arc<dyn RecycleBinHost>) {
        let events = self.mailbox.clone();
        let event_dock = self.dock.clone();
        let completions = self.mailbox.clone();
        let ready_dock = self.dock.clone();
        let result = provider.watch(
            Arc::new(move |event| watch_event(&events, &event_dock, token, event)),
            Box::new(move |result| {
                ready(
                    &completions,
                    &ready_dock,
                    token,
                    result.map_err(|error| error.kind),
                );
            }),
        );
        match result {
            Ok(guard) => {
                let mut guard = Some(guard);
                if !self.closed.get() {
                    let mut state = self.state.borrow_mut();
                    if state.watch == Some(token) {
                        state.watch_guard = guard.take();
                    }
                }
                // Inline readiness/reentry may already have retired this returned guard.
                drop(guard);
            }
            Err(error) => ready(&self.mailbox, &self.dock, token, Err(error.kind)),
        }
    }

    fn start_read(&self, token: Token, provider: Arc<dyn RecycleBinHost>) {
        if self.closed.get() {
            return;
        }
        if !self.dock_visible() || !self.state.borrow().visible {
            {
                let mut state = self.state.borrow_mut();
                if state.read != Some(token) {
                    return;
                }
                state.read = None;
                state.read_needed = true;
                state.dirty = state.dirty || state.readback_needed;
            }
            let mut mailbox = self.mailbox.lock();
            if mailbox.expected_read == Some(token) {
                mailbox.expected_read = None;
                mailbox.read = None;
            }
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.read != Some(token) {
                return;
            }
            state.readback_needed = false;
        }
        // Stamp native issuance, not render-time reservation, before callback-capable dispatch.
        self.mailbox.lock().latest_read = Some(token);
        self.throttle.stop();
        let mailbox = self.mailbox.clone();
        let dock = self.dock.clone();
        if let Err(error) = provider.read(Box::new(move |result| {
            read_complete(&mailbox, &dock, token, result.map_err(|error| error.kind));
        })) {
            read_complete(&self.mailbox, &self.dock, token, Err(error.kind));
        }
    }

    fn start_open(&self, token: Token, provider: Arc<dyn RecycleBinHost>) {
        let mailbox = self.mailbox.clone();
        let dock = self.dock.clone();
        if let Err(error) = provider.open(Box::new(move |result| {
            open_complete(&mailbox, &dock, token, result.map_err(|error| error.kind));
        })) {
            open_complete(&self.mailbox, &self.dock, token, Err(error.kind));
        }
    }

    fn start_empty(&self, token: Token, provider: Arc<dyn RecycleBinMutationHost>) {
        if self.closed.get() {
            return;
        }
        if !self.dock_visible() || !self.state.borrow().visible {
            self.cancel_undispatched_empty();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let Some(flight) = state.empty.as_mut() else {
                return;
            };
            if flight.token != token || flight.dispatched {
                return;
            }
            // Only this point enters the provider; rendering/acquisition may have canceled demand.
            flight.dispatched = true;
        }
        let mailbox = self.mailbox.clone();
        let dock = self.dock.clone();
        if let Err(error) = provider.empty(Box::new(move |result| {
            empty_complete(&mailbox, &dock, token, result.map_err(|error| error.kind));
        })) {
            empty_complete(&self.mailbox, &self.dock, token, Err(error.kind));
        }
    }

    fn render(&self) {
        if self.closed.get() {
            return;
        }
        let (
            visible,
            recycle_state,
            count,
            read_busy,
            open_busy,
            empty_busy,
            stale,
            read,
            watch,
            open,
            empty,
        ) = {
            let state = self.state.borrow();
            let recycle_state = match state.snapshot.as_ref() {
                None => DockRecycleState::Unknown,
                Some(info) if info.is_empty() => DockRecycleState::Empty,
                Some(_) => DockRecycleState::Full,
            };
            let read_busy =
                state.read.is_some() || state.acquiring.is_some() || state.ready_pending;
            let count = match state.snapshot.as_ref() {
                Some(info) => format!("{} items", info.item_count),
                None if read_busy => "Loading…".to_owned(),
                None if state.read_error.is_some() => "Unavailable".to_owned(),
                None => "Unknown".to_owned(),
            };
            let read = state
                .read_error
                .map_or_else(String::new, |kind| notice("Read", kind));
            let watch = state.watch_error.map_or_else(
                || {
                    if state.ready_pending {
                        "Live updates starting.".to_owned()
                    } else {
                        String::new()
                    }
                },
                |kind| notice("Live updates", kind),
            );
            let open = state.open_error.map_or_else(
                || {
                    if state.open.is_some() {
                        "Open request pending.".to_owned()
                    } else if state.open_succeeded {
                        "Recycle Bin open requested.".to_owned()
                    } else {
                        String::new()
                    }
                },
                |kind| notice("Open", kind),
            );
            let empty = if state.empty.is_some() {
                "Recycle Bin Empty request pending.".to_owned()
            } else {
                state.empty_result.map_or_else(String::new, empty_notice)
            };
            (
                state.visible,
                recycle_state,
                count,
                read_busy,
                state.open.is_some(),
                state.empty.is_some(),
                state.snapshot.is_some()
                    && (state.dirty
                        || state.read.is_some()
                        || state.empty.is_some()
                        || !state.watch_live
                        || state.read_error.is_some()
                        || state.watch_error.is_some()),
                read,
                watch,
                open,
                empty,
            )
        };
        let Some(dock) = self.dock.upgrade() else {
            return;
        };
        if !visible || !dock.window().is_visible() {
            return;
        }
        dock.set_recycle_state(recycle_state);
        dock.set_recycle_item_count_label(count.into());
        dock.set_recycle_read_busy(read_busy);
        dock.set_recycle_open_busy(open_busy);
        dock.set_recycle_stale(stale);
        dock.set_recycle_read_notice(read.into());
        dock.set_recycle_watch_notice(watch.into());
        dock.set_recycle_open_notice(open.into());
        dock.set_recycle_empty_busy(empty_busy);
        dock.set_recycle_empty_notice(empty.into());
        // Generated changed notification is pure menu projection; no actor borrow crosses it.
        dock.set_recycle_empty_enabled(!empty_busy);
    }
}

impl Drop for RecycleBinController {
    fn drop(&mut self) {
        self.close();
    }
}

fn notice(operation: &str, kind: DockUtilityErrorKind) -> String {
    let reason = match kind {
        DockUtilityErrorKind::Unsupported => "is not supported by this host",
        DockUtilityErrorKind::AccessDenied => "was denied",
        DockUtilityErrorKind::Unavailable => "is unavailable",
        DockUtilityErrorKind::Busy => "provider is busy",
        DockUtilityErrorKind::Stopped => "provider has stopped",
        DockUtilityErrorKind::Other => "could not be completed",
    };
    let next = match operation {
        "Open" => "Try opening again.",
        "Empty" => "State refresh requested.",
        _ => "Retry explicitly.",
    };
    format!("Recycle Bin {operation}: {reason}. {next}")
}

fn empty_notice(result: Result<RecycleBinEmptyOutcome, DockUtilityErrorKind>) -> String {
    match result {
        Ok(outcome) => {
            let status = outcome.native_hresult as u32;
            let qualifier = if outcome.native_hresult < 0 {
                "failure status "
            } else if outcome.native_hresult == 0 {
                ""
            } else {
                "status "
            };
            format!(
                "Recycle Bin operation returned {qualifier}0x{status:08X}. State refresh requested."
            )
        }
        Err(kind) => notice("Empty", kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PanelPreferences, PanelSnapshot, SystemAction};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tessera_system::dock_utilities::DockUtilityError;
    use tessera_system::recycle_bin::{
        RecycleBinCompletion, RecycleBinReadCompletion, RecycleBinWatchCallback,
        RecycleBinWatchCompletion,
    };
    use tessera_system::recycle_bin_mutation::RecycleBinEmptyCompletion;

    type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
    thread_local! {
        static BACKEND_INITIALIZED: Cell<bool> = const { Cell::new(false) };
        static FACTORY_HOOK: Hook = RefCell::default();
        static MUTATION_FACTORY_HOOK: Hook = RefCell::default();
        static EMPTY_HOOK: Hook = RefCell::default();
        static WATCH_HOOK: Hook = RefCell::default();
        static READ_HOOK: Hook = RefCell::default();
        static OPEN_HOOK: Hook = RefCell::default();
        static DROP_HOOK: Hook = RefCell::default();
    }

    fn run_hook(hook: &'static std::thread::LocalKey<Hook>) {
        let hook = hook.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }

    fn error(kind: DockUtilityErrorKind) -> DockUtilityError {
        DockUtilityError::new(
            kind,
            "PRIVATE-provider-detail\nC:\\user\\secret\u{202e}<script>",
        )
    }

    fn info(item_count: u64) -> RecycleBinInfo {
        RecycleBinInfo {
            item_count,
            size_in_bytes: 0,
        }
    }

    enum Reply<T> {
        Pending,
        Inline(Result<T, DockUtilityError>),
        Reject(DockUtilityError),
    }

    struct WatchCall {
        callback: RecycleBinWatchCallback,
        ready: Option<RecycleBinWatchCompletion>,
    }

    #[derive(Default)]
    struct RecordingBin {
        reads: AtomicUsize,
        opens: AtomicUsize,
        guards_dropped: Arc<AtomicUsize>,
        events: Mutex<Vec<&'static str>>,
        watch_calls: Mutex<Vec<WatchCall>>,
        watch_replies: Mutex<VecDeque<Reply<()>>>,
        read_replies: Mutex<VecDeque<Reply<RecycleBinInfo>>>,
        open_replies: Mutex<VecDeque<Reply<()>>>,
        pending_read: Mutex<Option<RecycleBinReadCompletion>>,
        pending_open: Mutex<Option<RecycleBinCompletion>>,
    }

    struct WatchGuard(Arc<AtomicUsize>);

    impl RecycleBinWatchGuard for WatchGuard {}

    impl Drop for WatchGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
            run_hook(&DROP_HOOK);
        }
    }

    impl RecordingBin {
        fn finish_ready(&self, index: usize, result: Result<(), DockUtilityError>) {
            let ready = self.watch_calls.lock()[index]
                .ready
                .take()
                .expect("accepted watch readiness");
            ready(result);
        }

        fn emit(&self, index: usize, event: RecycleBinWatchEvent) {
            let callback = self.watch_calls.lock()[index].callback.clone();
            callback(event);
        }

        fn finish_read(&self, result: Result<RecycleBinInfo, DockUtilityError>) {
            let completion = self.pending_read.lock().take().expect("accepted read");
            completion(result);
        }

        fn finish_open(&self, result: Result<(), DockUtilityError>) {
            let completion = self.pending_open.lock().take().expect("accepted open");
            completion(result);
        }

        fn reads(&self) -> usize {
            self.reads.load(Ordering::Relaxed)
        }

        fn opens(&self) -> usize {
            self.opens.load(Ordering::Relaxed)
        }

        fn watches(&self) -> usize {
            self.watch_calls.lock().len()
        }
    }

    impl RecycleBinHost for RecordingBin {
        fn watch(
            &self,
            callback: RecycleBinWatchCallback,
            completion: RecycleBinWatchCompletion,
        ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
            self.events.lock().push("watch");
            let reply = self
                .watch_replies
                .lock()
                .pop_front()
                .unwrap_or(Reply::Pending);
            match reply {
                Reply::Pending => self.watch_calls.lock().push(WatchCall {
                    callback,
                    ready: Some(completion),
                }),
                Reply::Inline(result) => {
                    self.watch_calls.lock().push(WatchCall {
                        callback,
                        ready: None,
                    });
                    completion(result);
                }
                Reply::Reject(error) => {
                    run_hook(&WATCH_HOOK);
                    return Err(error);
                }
            }
            run_hook(&WATCH_HOOK);
            Ok(Box::new(WatchGuard(self.guards_dropped.clone())))
        }

        fn read(&self, completion: RecycleBinReadCompletion) -> Result<(), DockUtilityError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.events.lock().push("read");
            assert!(self.pending_read.lock().is_none());
            let reply = self
                .read_replies
                .lock()
                .pop_front()
                .unwrap_or(Reply::Pending);
            match reply {
                Reply::Pending => *self.pending_read.lock() = Some(completion),
                Reply::Inline(result) => completion(result),
                Reply::Reject(error) => {
                    run_hook(&READ_HOOK);
                    return Err(error);
                }
            }
            run_hook(&READ_HOOK);
            Ok(())
        }

        fn open(&self, completion: RecycleBinCompletion) -> Result<(), DockUtilityError> {
            self.opens.fetch_add(1, Ordering::Relaxed);
            self.events.lock().push("open");
            assert!(self.pending_open.lock().is_none());
            let reply = self
                .open_replies
                .lock()
                .pop_front()
                .unwrap_or(Reply::Pending);
            match reply {
                Reply::Pending => *self.pending_open.lock() = Some(completion),
                Reply::Inline(result) => completion(result),
                Reply::Reject(error) => {
                    run_hook(&OPEN_HOOK);
                    return Err(error);
                }
            }
            run_hook(&OPEN_HOOK);
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingMutation {
        empties: AtomicUsize,
        replies: Mutex<VecDeque<Reply<RecycleBinEmptyOutcome>>>,
        pending: Mutex<Option<RecycleBinEmptyCompletion>>,
    }

    impl RecordingMutation {
        fn empties(&self) -> usize {
            self.empties.load(Ordering::Relaxed)
        }

        fn finish(&self, result: Result<RecycleBinEmptyOutcome, DockUtilityError>) {
            let completion = self.pending.lock().take().expect("accepted Empty");
            completion(result);
        }
    }

    impl RecycleBinMutationHost for RecordingMutation {
        fn empty(&self, completion: RecycleBinEmptyCompletion) -> Result<(), DockUtilityError> {
            self.empties.fetch_add(1, Ordering::Relaxed);
            assert!(self.pending.lock().is_none(), "one mutation flight");
            let reply = self.replies.lock().pop_front().unwrap_or(Reply::Pending);
            match reply {
                Reply::Pending => *self.pending.lock() = Some(completion),
                Reply::Inline(result) => completion(result),
                Reply::Reject(error) => {
                    run_hook(&EMPTY_HOOK);
                    return Err(error);
                }
            }
            run_hook(&EMPTY_HOOK);
            Ok(())
        }
    }

    fn outcome(native_hresult: i32) -> RecycleBinEmptyOutcome {
        RecycleBinEmptyOutcome { native_hresult }
    }

    struct RecordingDesktop {
        provider: Mutex<Result<Option<Arc<dyn RecycleBinHost>>, DockUtilityError>>,
        factory_calls: AtomicUsize,
        mutation_provider: Mutex<Result<Option<Arc<dyn RecycleBinMutationHost>>, DockUtilityError>>,
        mutation_factory_calls: AtomicUsize,
    }

    impl DesktopHost for RecordingDesktop {
        fn observe(&self) -> Result<PanelSnapshot, String> {
            panic!("Recycle Bin cannot observe applications")
        }

        fn activate(&self, _: &str) -> Result<(), String> {
            panic!("Recycle Bin cannot activate application windows")
        }

        fn launch(&self, _: &str) -> Result<(), String> {
            panic!("Recycle Bin cannot launch applications")
        }

        fn system_action(&self, _: SystemAction) -> Result<(), String> {
            panic!("Recycle Bin cannot dispatch recovery or desktop actions")
        }

        fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
            panic!("Recycle Bin cannot save preferences")
        }

        fn subscribe(
            &self,
            _: Arc<dyn Fn() + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, String> {
            panic!("Recycle Bin cannot subscribe to desktop observation")
        }

        fn request_ui_focus(&self, _: &slint::Window) -> Result<(), String> {
            panic!("Recycle Bin cannot request desktop focus")
        }

        fn recycle_bin_host(&self) -> Result<Option<Arc<dyn RecycleBinHost>>, DockUtilityError> {
            self.factory_calls.fetch_add(1, Ordering::Relaxed);
            run_hook(&FACTORY_HOOK);
            self.provider.lock().clone()
        }

        fn recycle_bin_mutation_host(
            &self,
        ) -> Result<Option<Arc<dyn RecycleBinMutationHost>>, DockUtilityError> {
            self.mutation_factory_calls.fetch_add(1, Ordering::Relaxed);
            run_hook(&MUTATION_FACTORY_HOOK);
            self.mutation_provider.lock().clone()
        }
    }

    struct Fixture {
        dock: Dock,
        bin: Arc<RecordingBin>,
        mutation: Arc<RecordingMutation>,
        host: Arc<RecordingDesktop>,
        presenter: Rc<RecycleBinController>,
        feedback: Rc<RefCell<Vec<Result<(), String>>>>,
        wakes: Rc<Cell<usize>>,
    }

    impl Fixture {
        fn new() -> Self {
            BACKEND_INITIALIZED.with(|initialized| {
                if !initialized.replace(true) {
                    i_slint_backend_testing::init_no_event_loop();
                }
            });
            let bin = Arc::new(RecordingBin::default());
            let mutation = Arc::new(RecordingMutation::default());
            let host = Arc::new(RecordingDesktop {
                provider: Mutex::new(Ok(Some(bin.clone()))),
                factory_calls: AtomicUsize::new(0),
                mutation_provider: Mutex::new(Ok(Some(mutation.clone()))),
                mutation_factory_calls: AtomicUsize::new(0),
            });
            let dock = Dock::new().unwrap();
            dock.window().set_size(slint::PhysicalSize::new(168, 72));
            let presenter = RecycleBinController::new(host.clone(), &dock);
            let feedback = Rc::new(RefCell::new(Vec::new()));
            let results = feedback.clone();
            let weak = Rc::downgrade(&presenter);
            let wakes = Rc::new(Cell::new(0));
            let wake_count = wakes.clone();
            dock.on_recycle_event_ready(move || {
                wake_count.set(wake_count.get() + 1);
                if let Some(presenter) = weak.upgrade()
                    && let Some(result) = presenter.process_events()
                {
                    results.borrow_mut().push(result);
                }
            });
            let weak = Rc::downgrade(&presenter);
            dock.on_recycle_action_requested(move |action| {
                if let Some(presenter) = weak.upgrade() {
                    presenter.request(action);
                }
            });
            Self {
                dock,
                bin,
                mutation,
                host,
                presenter,
                feedback,
                wakes,
            }
        }

        fn show(&self) {
            self.dock.show().unwrap();
            self.presenter.shown();
        }

        fn hide(&self) {
            self.presenter.hidden();
            self.dock.hide().unwrap();
        }

        fn drain(&self) {
            self.dock.invoke_recycle_event_ready();
        }

        fn open(&self) {
            self.dock
                .invoke_recycle_action_requested(DockRecycleAction::Open);
        }

        fn empty(&self) {
            self.dock
                .invoke_recycle_action_requested(DockRecycleAction::Empty);
        }

        fn retry(&self) {
            self.dock
                .invoke_recycle_action_requested(DockRecycleAction::Retry);
        }

        fn start_read(&self) {
            self.show();
            self.bin.finish_ready(0, Ok(()));
            self.drain();
        }

        fn snapshot(&self, count: u64) {
            self.start_read();
            self.bin.finish_read(Ok(info(count)));
            self.drain();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.presenter.close();
            // Slint's visible-window keepalive owns the component until hide.
            self.dock.hide().unwrap();
        }
    }

    fn advance(milliseconds: u64) {
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
        slint::platform::update_timers_and_animations();
    }

    #[test]
    fn construction_and_provisional_show_have_no_capability_or_input_authority() {
        let fixture = Fixture::new();
        fixture
            .dock
            .set_recycle_item_count_label("root-owned".into());
        let spare = RecycleBinController::new(fixture.host.clone(), &fixture.dock);
        fixture.drain();
        assert_eq!(
            fixture.wakes.get(),
            1,
            "constructor preserves root callback"
        );
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "root-owned");
        fixture.open();
        fixture.retry();
        fixture.presenter.shown();
        fixture.dock.show().unwrap();
        fixture.open();
        fixture.retry();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.bin.reads(), 0);
        assert_eq!(fixture.bin.opens(), 0);
        spare.close();
        fixture.presenter.close();
        fixture.presenter.shown();
        fixture.open();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn asynchronous_ready_is_required_and_subscription_precedes_genuine_read() {
        let fixture = Fixture::new();
        fixture.show();
        fixture.presenter.shown();
        assert_eq!(*fixture.bin.events.lock(), vec!["watch"]);
        assert_eq!(fixture.bin.reads(), 0);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "Loading…");
        fixture.bin.finish_ready(0, Ok(()));
        assert_eq!(
            fixture.bin.reads(),
            0,
            "ready is queued, not a worker-side read"
        );
        fixture.drain();
        assert_eq!(*fixture.bin.events.lock(), vec!["watch", "read"]);
        assert!(fixture.dock.get_recycle_read_busy());
        assert!(!fixture.dock.get_recycle_open_busy());
        fixture.bin.finish_read(Ok(info(u64::MAX)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
        assert_eq!(
            fixture.dock.get_recycle_item_count_label(),
            "18446744073709551615 items"
        );
        assert!(!fixture.dock.get_recycle_read_busy());
        assert!(!fixture.dock.get_recycle_stale());
        fixture.retry();
        fixture.bin.finish_read(Ok(info(0)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
        assert_eq!(fixture.bin.watches(), 1);
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        assert!(fixture.feedback.borrow().is_empty());
    }

    #[test]
    fn read_and_open_flights_are_independent_and_duplicates_do_not_queue_open() {
        let fixture = Fixture::new();
        fixture.show();
        fixture.open();
        fixture.open();
        assert_eq!(fixture.bin.opens(), 1, "open does not wait for readiness");
        assert!(fixture.dock.get_recycle_open_busy());
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        fixture.open();
        fixture.bin.finish_open(Ok(()));
        fixture.drain();
        assert!(!fixture.dock.get_recycle_open_busy());
        assert!(fixture.dock.get_recycle_read_busy());
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        fixture.open();
        assert_eq!(fixture.bin.opens(), 2);
        fixture.bin.finish_read(Ok(info(1)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "1 items");
        assert!(fixture.dock.get_recycle_open_busy());
        fixture
            .bin
            .finish_open(Err(error(DockUtilityErrorKind::AccessDenied)));
        fixture.drain();
        fixture.retry();
        fixture.drain();
        advance(1000);
        assert_eq!(fixture.bin.opens(), 2, "Retry and time never replay open");
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.feedback.borrow().len(), 2);
        assert!(
            fixture
                .dock
                .get_recycle_open_notice()
                .contains("was denied")
        );
    }

    #[test]
    fn watch_failure_still_reads_and_opens_and_retry_replaces_only_failed_watch() {
        let fixture = Fixture::new();
        fixture
            .bin
            .watch_replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Unavailable)));
        fixture.show();
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        fixture.bin.finish_read(Ok(info(7)));
        fixture.drain();
        assert!(fixture.dock.get_recycle_stale());
        assert!(
            fixture
                .dock
                .get_recycle_watch_notice()
                .contains("is unavailable")
        );
        fixture.open();
        fixture
            .bin
            .finish_open(Err(error(DockUtilityErrorKind::Other)));
        fixture.drain();
        fixture.retry();
        assert_eq!(fixture.bin.watches(), 1);
        assert_eq!(
            fixture.bin.reads(),
            1,
            "replacement readiness still precedes read"
        );
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        fixture.bin.finish_read(Ok(info(8)));
        fixture.drain();
        assert!(!fixture.dock.get_recycle_stale());
        assert!(fixture.dock.get_recycle_watch_notice().is_empty());
        fixture.retry();
        assert_eq!(
            fixture.bin.watches(),
            1,
            "healthy watch is retained on Retry"
        );
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn burst_uses_non_restarted_one_shot_and_dirty_during_read_has_one_followup() {
        let fixture = Fixture::new();
        fixture.snapshot(3);
        for _ in 0..1000 {
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        }
        assert!(fixture.presenter.mailbox.lock().dirty);
        fixture.drain();
        assert!(fixture.presenter.throttle.running());
        advance(99);
        assert_eq!(fixture.bin.reads(), 1);
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        advance(1);
        assert_eq!(
            fixture.bin.reads(),
            2,
            "late event must not restart the 100ms window"
        );
        for _ in 0..1000 {
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        }
        fixture.drain();
        advance(100);
        assert_eq!(
            fixture.bin.reads(),
            2,
            "one read in flight despite elapsed throttle"
        );
        assert!(
            !fixture.presenter.throttle.running(),
            "expired timer does not poll"
        );
        fixture.bin.finish_read(Ok(info(4)));
        fixture.drain();
        assert_eq!(
            fixture.bin.reads(),
            3,
            "retained invalidation has one bounded followup"
        );
        fixture.bin.finish_read(Ok(info(5)));
        fixture.drain();
        advance(100_000);
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 3, "idle has no automatic reads");
        assert!(!fixture.presenter.throttle.running());
    }

    #[test]
    fn sustained_changes_cannot_starve_or_run_faster_than_the_change_interval() {
        let fixture = Fixture::new();
        fixture.snapshot(1);
        for interval in 0..5 {
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
            fixture.drain();
            for _ in 0..9 {
                advance(10);
                fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
                fixture.drain();
            }
            assert_eq!(fixture.bin.reads(), interval + 1);
            advance(10);
            assert_eq!(fixture.bin.reads(), interval + 2);
            fixture.bin.finish_read(Ok(info(interval as u64 + 2)));
            fixture.drain();
        }
        assert_eq!(fixture.bin.reads(), 6);
    }

    #[test]
    fn hidden_retains_watch_and_accepted_results_without_showing_or_open_replay() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.open();
        fixture.hide();
        for _ in 0..100 {
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        }
        fixture.drain();
        fixture.bin.finish_read(Ok(info(9)));
        fixture
            .bin
            .finish_open(Err(error(DockUtilityErrorKind::Stopped)));
        fixture.drain();
        advance(1000);
        fixture.open();
        fixture.retry();
        assert!(!fixture.dock.window().is_visible());
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.bin.guards_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(fixture.presenter.state.borrow().snapshot, Some(info(9)));
        fixture.show();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "9 items");
        fixture.bin.finish_read(Ok(info(10)));
        fixture.drain();
        fixture.hide();
        fixture.show();
        assert_eq!(
            fixture.bin.reads(),
            2,
            "clean initialized resume is passive"
        );
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.bin.watches(), 1);
    }

    #[test]
    fn hidden_readiness_waits_for_resume_and_hidden_timer_cannot_dispatch() {
        let fixture = Fixture::new();
        fixture.show();
        fixture.hide();
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 0);
        fixture.show();
        fixture.bin.finish_read(Ok(info(2)));
        fixture.drain();
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        assert!(fixture.presenter.throttle.running());
        fixture.hide();
        advance(1000);
        assert_eq!(fixture.bin.reads(), 1);
        fixture.show();
        assert_eq!(fixture.bin.reads(), 2);
    }

    #[test]
    fn read_backpressure_keeps_last_good_and_dirty_without_an_idle_retry_loop() {
        let fixture = Fixture::new();
        fixture.snapshot(42);
        fixture
            .bin
            .read_replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Busy)));
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        advance(100);
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "42 items");
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
        assert!(fixture.dock.get_recycle_stale());
        assert!(fixture.presenter.state.borrow().dirty);
        advance(10_000);
        assert_eq!(fixture.bin.reads(), 2);
        fixture.hide();
        fixture.show();
        assert_eq!(fixture.bin.reads(), 3);
        fixture.bin.finish_read(Ok(info(43)));
        fixture.drain();
        assert!(!fixture.dock.get_recycle_stale());
    }

    #[test]
    fn unknown_failure_never_claims_empty_and_all_notices_are_kind_only() {
        let fixture = Fixture::new();
        fixture.show();
        fixture
            .bin
            .finish_ready(0, Err(error(DockUtilityErrorKind::Other)));
        fixture.drain();
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::AccessDenied)));
        fixture.drain();
        fixture.open();
        fixture
            .bin
            .finish_open(Err(error(DockUtilityErrorKind::Unavailable)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "Unavailable");
        for notice in [
            fixture.dock.get_recycle_read_notice(),
            fixture.dock.get_recycle_watch_notice(),
            fixture.dock.get_recycle_open_notice(),
        ] {
            assert!(!notice.is_empty());
            assert!(!notice.contains("PRIVATE"));
            assert!(!notice.contains("secret"));
            assert!(!notice.contains('\n'));
            assert!(notice.len() < 128);
        }
        for kind in [
            DockUtilityErrorKind::Unsupported,
            DockUtilityErrorKind::AccessDenied,
            DockUtilityErrorKind::Unavailable,
            DockUtilityErrorKind::Busy,
            DockUtilityErrorKind::Stopped,
            DockUtilityErrorKind::Other,
        ] {
            assert!(notice("Read", kind).len() < 128);
        }
    }

    #[test]
    fn failed_factory_is_not_cached_and_only_retry_or_resume_can_reacquire() {
        let fixture = Fixture::new();
        *fixture.host.provider.lock() = Err(error(DockUtilityErrorKind::Unavailable));
        fixture.show();
        fixture.drain();
        fixture.presenter.shown();
        advance(10_000);
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.bin.watches(), 0);
        assert_eq!(fixture.bin.reads(), 0);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        *fixture.host.provider.lock() = Ok(Some(fixture.bin.clone()));
        fixture.retry();
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        fixture.bin.finish_read(Ok(info(1)));
        fixture.drain();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 2);
        fixture.retry();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.bin.opens(), 0);
    }

    #[test]
    fn stale_watch_and_duplicate_completions_cannot_replace_current_generations() {
        let fixture = Fixture::new();
        fixture.snapshot(6);
        let old_watch = fixture.presenter.state.borrow().watch.unwrap();
        fixture.bin.emit(
            0,
            RecycleBinWatchEvent::Unavailable(DockUtilityErrorKind::Stopped),
        );
        fixture.drain();
        assert_eq!(fixture.bin.guards_dropped.load(Ordering::Relaxed), 1);
        fixture.retry();
        let new_watch = fixture.presenter.state.borrow().watch.unwrap();
        assert_ne!(old_watch, new_watch);
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        ready(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            old_watch,
            Ok(()),
        );
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        fixture.bin.finish_ready(1, Ok(()));
        fixture.drain();
        let read_token = fixture.presenter.state.borrow().read.unwrap();
        fixture.bin.finish_read(Ok(info(7)));
        read_complete(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            read_token,
            Ok(info(0)),
        );
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "7 items");
        read_complete(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            read_token,
            Ok(info(0)),
        );
        ready(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            new_watch,
            Err(DockUtilityErrorKind::Other),
        );
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "7 items");
        assert_eq!(fixture.presenter.state.borrow().watch, Some(new_watch));
        fixture.open();
        let open_token = fixture
            .presenter
            .state
            .borrow()
            .open
            .as_ref()
            .unwrap()
            .token;
        fixture.bin.finish_open(Ok(()));
        open_complete(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            open_token,
            Err(DockUtilityErrorKind::Other),
        );
        fixture.drain();
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
        open_complete(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            open_token,
            Ok(()),
        );
        fixture.drain();
        assert_eq!(fixture.feedback.borrow().len(), 1);
    }

    #[test]
    fn inline_ready_read_open_and_reentry_reserve_before_every_callback() {
        let fixture = Fixture::new();
        fixture
            .bin
            .watch_replies
            .lock()
            .push_back(Reply::Inline(Ok(())));
        fixture
            .bin
            .read_replies
            .lock()
            .push_back(Reply::Inline(Ok(info(11))));
        fixture
            .bin
            .open_replies
            .lock()
            .push_back(Reply::Inline(Ok(())));
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.shown();
                presenter.request(DockRecycleAction::Open);
                presenter.request(DockRecycleAction::Open);
            }))
        });
        WATCH_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                dock.upgrade().unwrap().invoke_recycle_event_ready();
            }))
        });
        let dock = fixture.dock.as_weak();
        READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                dock.upgrade().unwrap().invoke_recycle_event_ready();
            }))
        });
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        OPEN_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.request(DockRecycleAction::Open);
                dock.upgrade().unwrap().invoke_recycle_event_ready();
            }))
        });
        fixture.show();
        fixture.drain();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.bin.watches(), 1);
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "11 items");
        assert!(!fixture.dock.get_recycle_read_busy());
        assert!(!fixture.dock.get_recycle_open_busy());
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
    }

    #[test]
    fn inline_failed_ready_can_retire_before_watch_returns_its_guard() {
        let fixture = Fixture::new();
        fixture
            .bin
            .watch_replies
            .lock()
            .push_back(Reply::Inline(Err(error(
                DockUtilityErrorKind::AccessDenied,
            ))));
        let dock = fixture.dock.as_weak();
        WATCH_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                dock.upgrade().unwrap().invoke_recycle_event_ready();
            }))
        });
        fixture.show();
        assert_eq!(fixture.bin.guards_dropped.load(Ordering::Relaxed), 1);
        assert!(fixture.presenter.state.borrow().watch.is_none());
        assert_eq!(fixture.bin.reads(), 1);
        fixture.bin.finish_read(Ok(info(12)));
        fixture.drain();
        assert!(fixture.dock.get_recycle_stale());
    }

    #[test]
    fn close_during_factory_or_watch_return_never_installs_or_resurrects_authority() {
        {
            let fixture = Fixture::new();
            let presenter = fixture.presenter.clone();
            FACTORY_HOOK
                .with(|hook| *hook.borrow_mut() = Some(Box::new(move || presenter.close())));
            fixture.show();
            assert!(fixture.presenter.state.borrow().provider.is_none());
            assert_eq!(fixture.bin.watches(), 0);
            fixture.retry();
            fixture.presenter.shown();
            assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        }
        {
            let fixture = Fixture::new();
            let presenter = fixture.presenter.clone();
            WATCH_HOOK.with(|hook| *hook.borrow_mut() = Some(Box::new(move || presenter.close())));
            fixture.show();
            assert_eq!(fixture.bin.guards_dropped.load(Ordering::Relaxed), 1);
            fixture.bin.finish_ready(0, Ok(()));
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
            fixture.drain();
            advance(1000);
            assert_eq!(fixture.bin.reads(), 0);
            assert!(!fixture.presenter.mailbox.lock().pending());
        }
    }

    #[test]
    fn close_retires_before_guard_drop_and_accepted_late_completions_are_ignored() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.open();
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        let presenter = fixture.presenter.clone();
        let bin = fixture.bin.clone();
        DROP_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.closed.get());
                assert!(presenter.state.borrow().watch.is_none());
                bin.emit(0, RecycleBinWatchEvent::Invalidated);
                presenter.request(DockRecycleAction::Open);
                presenter.shown();
            }))
        });
        fixture.presenter.close();
        fixture.presenter.close();
        fixture.bin.finish_read(Ok(info(0)));
        fixture.bin.finish_open(Ok(()));
        fixture.bin.emit(
            0,
            RecycleBinWatchEvent::Unavailable(DockUtilityErrorKind::Other),
        );
        fixture.drain();
        advance(1000);
        assert_eq!(fixture.bin.guards_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.bin.opens(), 1);
        assert!(fixture.feedback.borrow().is_empty());
        assert!(!fixture.presenter.mailbox.lock().pending());
        assert!(fixture.presenter.state.borrow().provider.is_none());
        assert!(!fixture.presenter.throttle.running());
        assert!(!fixture.dock.get_recycle_read_busy());
        assert!(!fixture.dock.get_recycle_open_busy());
    }

    #[test]
    fn hiding_during_factory_cancels_undispatched_open_instead_of_replaying_on_resume() {
        let fixture = Fixture::new();
        let presenter = fixture.presenter.clone();
        FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.request(DockRecycleAction::Open);
                presenter.hidden();
            }))
        });
        fixture.show();
        assert_eq!(fixture.bin.opens(), 0);
        assert_eq!(fixture.bin.watches(), 0);
        assert!(fixture.presenter.state.borrow().open.is_none());
        fixture.show();
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.bin.opens(), 0);
    }

    #[test]
    fn resume_with_an_accepted_clean_read_does_not_queue_a_duplicate() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.hide();
        fixture.show();
        assert_eq!(fixture.bin.reads(), 1);
        fixture.bin.finish_read(Ok(info(15)));
        fixture.drain();
        advance(1000);
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "15 items");
    }

    #[test]
    fn invalidation_drained_during_read_survives_failure_with_one_bounded_followup() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        advance(99);
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::Busy)));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        assert!(fixture.presenter.state.borrow().dirty);
        advance(1);
        assert_eq!(fixture.bin.reads(), 2);
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::Busy)));
        fixture.drain();
        advance(10_000);
        assert_eq!(
            fixture.bin.reads(),
            2,
            "no fresh invalidation means no failure retry loop"
        );
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        advance(99);
        assert_eq!(fixture.bin.reads(), 2);
        advance(1);
        assert_eq!(fixture.bin.reads(), 3);
    }

    #[test]
    fn rejected_read_and_open_are_reserved_during_reentry_and_never_replayed() {
        let fixture = Fixture::new();
        fixture
            .bin
            .read_replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Busy)));
        let presenter = fixture.presenter.clone();
        READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.state.borrow().read.is_some());
                presenter.process_events();
            }))
        });
        fixture.start_read();
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        fixture
            .bin
            .open_replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Unavailable)));
        let presenter = fixture.presenter.clone();
        OPEN_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.state.borrow().open.is_some());
                presenter.request(DockRecycleAction::Open);
                presenter.process_events();
            }))
        });
        fixture.open();
        fixture.open();
        fixture.drain();
        advance(10_000);
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.feedback.borrow().len(), 1);
        assert!(
            fixture
                .dock
                .get_recycle_open_notice()
                .contains("Try opening again.")
        );
        fixture
            .bin
            .read_replies
            .lock()
            .push_back(Reply::Inline(Err(error(
                DockUtilityErrorKind::AccessDenied,
            ))));
        fixture.retry();
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
    }

    #[test]
    fn missing_factory_and_reentrant_open_complete_once_without_reacquisition_loop() {
        let fixture = Fixture::new();
        *fixture.host.provider.lock() = Ok(None);
        let presenter = fixture.presenter.clone();
        FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.request(DockRecycleAction::Open);
                presenter.request(DockRecycleAction::Open);
                presenter.request(DockRecycleAction::Retry);
            }))
        });
        fixture.show();
        fixture.drain();
        advance(1000);
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        assert_eq!(fixture.bin.opens(), 0);
        assert_eq!(fixture.feedback.borrow().len(), 1);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "Unavailable");
        *fixture.host.provider.lock() = Ok(Some(fixture.bin.clone()));
        fixture.retry();
        fixture.bin.finish_ready(0, Ok(()));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(
            fixture.bin.opens(),
            0,
            "successful capability retry cannot revive failed open"
        );
    }

    #[test]
    fn queued_wake_keeps_bounded_slots_and_worker_results_are_not_backpressured_away() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.open();
        // Model a successfully posted wake still waiting for the root to drain it.
        fixture.presenter.mailbox.lock().wake_queued = true;
        let bin = fixture.bin.clone();
        std::thread::spawn(move || {
            for _ in 0..10_000 {
                bin.emit(0, RecycleBinWatchEvent::Invalidated);
            }
            bin.finish_read(Ok(RecycleBinInfo {
                item_count: 2,
                size_in_bytes: u64::MAX,
            }));
            bin.finish_open(Ok(()));
        })
        .join()
        .unwrap();
        {
            let mailbox = fixture.presenter.mailbox.lock();
            assert!(mailbox.wake_queued);
            assert!(mailbox.dirty);
            assert!(mailbox.read.is_some());
            assert!(mailbox.open.is_some());
            assert!(mailbox.ready.is_none());
        }
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "2 items");
        assert_eq!(
            fixture
                .presenter
                .state
                .borrow()
                .snapshot
                .as_ref()
                .unwrap()
                .size_in_bytes,
            u64::MAX
        );
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
        assert_eq!(fixture.bin.reads(), 1);
        advance(99);
        assert_eq!(fixture.bin.reads(), 1);
        advance(1);
        assert_eq!(fixture.bin.reads(), 2);
    }

    #[test]
    fn owned_dock_hide_releases_sdk_keepalive_and_weak_late_callbacks_do_not_revive_it() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.open();
        let dock = fixture.dock.as_weak();
        let bin = fixture.bin.clone();
        let presenter = Rc::downgrade(&fixture.presenter);
        drop(fixture);
        assert!(presenter.upgrade().is_none());
        assert!(
            dock.upgrade().is_none(),
            "owned fixture must hide before dropping Dock"
        );
        bin.finish_read(Ok(info(0)));
        bin.finish_open(Ok(()));
        bin.emit(0, RecycleBinWatchEvent::Invalidated);
        advance(1000);
        assert!(dock.upgrade().is_none());
        assert_eq!(bin.reads(), 1);
        assert_eq!(bin.opens(), 1);
        assert_eq!(bin.guards_dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn empty_is_lazy_current_live_intent_and_never_count_disabled() {
        let fixture = Fixture::new();
        fixture.empty();
        fixture.retry();
        fixture.dock.show().unwrap();
        fixture.empty();
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            0
        );
        assert!(!fixture.dock.get_recycle_empty_enabled());
        fixture.presenter.shown();
        assert_eq!(
            fixture.bin.reads(),
            0,
            "first read still waits for watch readiness"
        );
        assert!(
            fixture.dock.get_recycle_empty_enabled(),
            "Unknown is not deletion authority"
        );
        fixture.open();
        fixture.empty();
        fixture.empty();
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert_eq!(
            fixture.mutation.empties(),
            1,
            "pending watch/Open never blocks Empty"
        );
        assert!(fixture.dock.get_recycle_empty_busy());
        assert!(!fixture.dock.get_recycle_empty_enabled());
        fixture.hide();
        fixture.empty();
        assert_eq!(fixture.mutation.empties(), 1);
        fixture.mutation.finish(Ok(outcome(0)));
        fixture.drain();
        fixture.show();
        assert_eq!(
            fixture.bin.reads(),
            1,
            "explicit post-return readback bypasses readiness"
        );
        fixture.bin.finish_read(Ok(info(0)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
        assert!(
            fixture.dock.get_recycle_empty_enabled(),
            "zero count does not disable Empty"
        );
        fixture.empty();
        assert_eq!(fixture.mutation.empties(), 2);
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        fixture.presenter.close();
        fixture.empty();
        fixture.presenter.shown();
        assert!(!fixture.dock.get_recycle_empty_enabled());
        assert!(!fixture.dock.get_recycle_empty_busy());
        assert_eq!(fixture.mutation.empties(), 2);
    }

    #[test]
    fn empty_read_open_and_watch_flights_are_independent_and_pending_snapshot_stays_stale() {
        let fixture = Fixture::new();
        fixture.snapshot(9);
        fixture.retry();
        fixture.open();
        fixture.empty();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.mutation.empties(), 1);
        assert!(fixture.dock.get_recycle_read_busy());
        assert!(fixture.dock.get_recycle_open_busy());
        assert!(fixture.dock.get_recycle_empty_busy());
        fixture.bin.finish_read(Ok(info(10)));
        fixture.bin.finish_open(Ok(()));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "10 items");
        assert!(
            fixture.dock.get_recycle_stale(),
            "a pending mutation keeps even updated count stale"
        );
        assert!(!fixture.dock.get_recycle_read_busy());
        assert!(!fixture.dock.get_recycle_open_busy());
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
        fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        fixture.drain();
        advance(100);
        assert_eq!(
            fixture.bin.reads(),
            3,
            "mutation does not suspend the existing watcher"
        );
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::Unavailable)));
        fixture.drain();
        fixture.mutation.finish(Ok(outcome(1)));
        assert_eq!(
            fixture.presenter.process_events(),
            None,
            "Empty never becomes Open feedback"
        );
        assert_eq!(fixture.bin.reads(), 4, "return clears failed-read blocking");
        fixture.bin.finish_read(Ok(info(11)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "11 items");
        assert!(!fixture.dock.get_recycle_stale());
        assert!(fixture.dock.get_recycle_empty_enabled());
        assert_eq!(fixture.feedback.borrow().len(), 1);
    }

    #[test]
    fn empty_mutation_factory_reentry_reserves_flight_before_callbacks() {
        let fixture = Fixture::new();
        fixture.snapshot(3);
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        MUTATION_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                let token = {
                    let state = presenter.state.borrow();
                    assert!(state.mutation_acquiring.is_some());
                    let flight = state.empty.as_ref().unwrap();
                    assert!(!flight.dispatched);
                    flight.token
                };
                assert_eq!(presenter.mailbox.lock().expected_empty, Some(token));
                assert!(!dock.upgrade().unwrap().get_recycle_empty_enabled());
                presenter.request(DockRecycleAction::Empty);
                presenter.request(DockRecycleAction::Empty);
                presenter.request(DockRecycleAction::Retry);
                presenter.request(DockRecycleAction::Open);
                presenter.process_events();
            }));
        });
        fixture.empty();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.bin.opens(), 1);
        fixture.mutation.finish(Ok(outcome(0)));
        fixture.drain();
        assert_eq!(
            fixture.bin.reads(),
            2,
            "old accepted read still owns its flight"
        );
        fixture.bin.finish_read(Ok(info(0)));
        fixture.bin.finish_open(Ok(()));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "3 items");
        assert_eq!(fixture.bin.reads(), 3);
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
    }

    #[test]
    fn empty_inline_return_and_reentry_complete_once_and_keep_success_only_cache() {
        let fixture = Fixture::new();
        fixture.snapshot(4);
        fixture
            .mutation
            .replies
            .lock()
            .push_back(Reply::Inline(Ok(outcome(0))));
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        EMPTY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.state.borrow().empty.as_ref().unwrap().dispatched);
                presenter.request(DockRecycleAction::Empty);
                presenter.request(DockRecycleAction::Empty);
                dock.upgrade().unwrap().invoke_recycle_event_ready();
            }));
        });
        fixture.empty();
        fixture.drain();
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.bin.reads(), 2);
        assert!(!fixture.dock.get_recycle_empty_busy());
        assert!(fixture.dock.get_recycle_empty_enabled());
        assert!(
            fixture
                .dock
                .get_recycle_empty_notice()
                .contains("0x00000000")
        );
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "4 items");
        assert!(fixture.feedback.borrow().is_empty());
        fixture.bin.finish_read(Ok(info(4)));
        fixture.drain();
        advance(100_000);
        assert_eq!(fixture.bin.reads(), 2);
        assert!(!fixture.presenter.throttle.running());
        fixture.empty();
        assert_eq!(
            fixture.mutation.empties(),
            2,
            "a new explicit intent is allowed"
        );
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn empty_immediate_rejection_reserves_during_reentry_and_never_auto_replays() {
        let fixture = Fixture::new();
        fixture.snapshot(6);
        fixture
            .mutation
            .replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Busy)));
        let presenter = fixture.presenter.clone();
        EMPTY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.state.borrow().empty.as_ref().unwrap().dispatched);
                presenter.request(DockRecycleAction::Empty);
                assert_eq!(presenter.process_events(), None);
                assert!(
                    presenter.state.borrow().empty.is_some(),
                    "immediate rejection has no callback"
                );
            }));
        });
        fixture.empty();
        fixture.empty();
        fixture.drain();
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "6 items");
        assert!(
            fixture
                .dock
                .get_recycle_empty_notice()
                .contains("provider is busy")
        );
        assert!(!fixture.dock.get_recycle_empty_notice().contains("PRIVATE"));
        fixture.bin.finish_read(Ok(info(7)));
        fixture.drain();
        fixture.retry();
        fixture.hide();
        fixture.show();
        advance(100_000);
        assert_eq!(
            fixture.mutation.empties(),
            1,
            "Retry/resume/time cannot repeat native mutation"
        );
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert!(fixture.feedback.borrow().is_empty());
    }

    #[test]
    fn empty_failed_or_missing_factory_is_safe_uncached_and_retried_only_by_new_empty() {
        for kind in [
            None,
            Some(DockUtilityErrorKind::Unsupported),
            Some(DockUtilityErrorKind::AccessDenied),
            Some(DockUtilityErrorKind::Unavailable),
            Some(DockUtilityErrorKind::Busy),
            Some(DockUtilityErrorKind::Stopped),
            Some(DockUtilityErrorKind::Other),
        ] {
            let fixture = Fixture::new();
            fixture.snapshot(8);
            *fixture.host.mutation_provider.lock() = match kind {
                None => Ok(None),
                Some(kind) => Err(error(kind)),
            };
            let presenter = fixture.presenter.clone();
            MUTATION_FACTORY_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    presenter.request(DockRecycleAction::Empty);
                    presenter.process_events();
                }));
            });
            fixture.empty();
            fixture.empty();
            fixture.drain();
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                1
            );
            assert!(fixture.presenter.state.borrow().mutation_provider.is_none());
            assert_eq!(fixture.mutation.empties(), 0);
            assert_eq!(
                fixture.bin.reads(),
                2,
                "factory terminal failure still requests readback"
            );
            let notice = fixture.dock.get_recycle_empty_notice();
            assert!(notice.contains("State refresh requested."));
            assert!(!notice.contains("PRIVATE"));
            assert!(!notice.contains("secret"));
            assert!(!notice.contains('\n'));
            assert!(notice.len() < 128);
            fixture.bin.finish_read(Ok(info(8)));
            fixture.drain();
            fixture.retry();
            fixture.bin.finish_read(Ok(info(8)));
            fixture.drain();
            fixture.hide();
            fixture.show();
            advance(100_000);
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                1
            );
            *fixture.host.mutation_provider.lock() = Ok(Some(fixture.mutation.clone()));
            fixture.empty();
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                2
            );
            assert_eq!(fixture.mutation.empties(), 1);
            assert!(fixture.feedback.borrow().is_empty());
        }
    }

    #[test]
    fn empty_is_independent_of_failed_read_factory_and_watch_setup() {
        let fixture = Fixture::new();
        *fixture.host.provider.lock() = Err(error(DockUtilityErrorKind::Unavailable));
        fixture.show();
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 0);
        assert!(fixture.dock.get_recycle_empty_enabled());
        fixture.empty();
        assert_eq!(fixture.mutation.empties(), 1);
        fixture.mutation.finish(Ok(outcome(0)));
        fixture.drain();
        assert_eq!(
            fixture.host.factory_calls.load(Ordering::Relaxed),
            2,
            "one readback reacquisition attempt"
        );
        advance(100_000);
        fixture.drain();
        assert_eq!(fixture.host.factory_calls.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        *fixture.host.provider.lock() = Ok(Some(fixture.bin.clone()));
        fixture
            .bin
            .watch_replies
            .lock()
            .push_back(Reply::Reject(error(DockUtilityErrorKind::Unavailable)));
        fixture.retry();
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 1);
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::Other)));
        fixture.drain();
        assert!(
            fixture.dock.get_recycle_empty_enabled(),
            "read/watch unavailability is not authority"
        );
        fixture.open();
        fixture.empty();
        assert_eq!(fixture.bin.opens(), 1);
        assert_eq!(fixture.mutation.empties(), 2);
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn empty_hide_during_factory_discards_success_and_cancels_without_resume_replay() {
        for raw_hide in [false, true] {
            let fixture = Fixture::new();
            fixture.snapshot(7);
            let presenter = fixture.presenter.clone();
            let dock = fixture.dock.as_weak();
            MUTATION_FACTORY_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    if !raw_hide {
                        presenter.hidden();
                    }
                    dock.upgrade().unwrap().hide().unwrap();
                }));
            });
            fixture.empty();
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                1
            );
            assert_eq!(fixture.mutation.empties(), 0);
            {
                let state = fixture.presenter.state.borrow();
                assert!(state.mutation_provider.is_none());
                assert!(state.mutation_acquiring.is_none());
                assert!(state.empty.is_none());
                assert!(
                    !state.readback_needed,
                    "a canceled undispatched intent has no native readback"
                );
            }
            assert!(fixture.presenter.mailbox.lock().expected_empty.is_none());
            assert!(!fixture.dock.get_recycle_empty_enabled());
            assert!(!fixture.dock.get_recycle_empty_busy());
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "7 items");
            fixture.presenter.hidden();
            fixture.show();
            fixture.retry();
            fixture.bin.finish_read(Ok(info(7)));
            fixture.drain();
            advance(100_000);
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                1
            );
            assert_eq!(fixture.mutation.empties(), 0);
            fixture.empty();
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                2
            );
            assert_eq!(fixture.mutation.empties(), 1);
        }
    }

    #[test]
    fn empty_hide_and_reshow_inside_factory_cannot_restore_canceled_intent() {
        let fixture = Fixture::new();
        fixture.snapshot(2);
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        MUTATION_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.hidden();
                let dock = dock.upgrade().unwrap();
                assert!(!dock.get_recycle_empty_enabled());
                dock.hide().unwrap();
                dock.show().unwrap();
                presenter.shown();
            }));
        });
        fixture.empty();
        assert!(fixture.dock.window().is_visible());
        assert!(fixture.dock.get_recycle_empty_enabled());
        assert!(fixture.presenter.state.borrow().empty.is_none());
        assert!(fixture.presenter.state.borrow().mutation_provider.is_none());
        assert_eq!(fixture.mutation.empties(), 0);
        assert_eq!(fixture.bin.reads(), 1);
        fixture.empty();
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            2
        );
        assert_eq!(fixture.mutation.empties(), 1);
    }

    #[test]
    fn empty_close_during_factory_retires_delivery_before_provider_return() {
        let fixture = Fixture::new();
        fixture.snapshot(4);
        let presenter = fixture.presenter.clone();
        MUTATION_FACTORY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                presenter.close();
                let state = presenter.state.borrow();
                assert!(state.empty.is_none());
                assert!(state.mutation_acquiring.is_none());
                assert!(state.mutation_provider.is_none());
                let mailbox = presenter.mailbox.lock();
                assert!(mailbox.closed);
                assert!(mailbox.expected_empty.is_none());
                assert!(!mailbox.pending());
            }));
        });
        fixture.empty();
        fixture.drain();
        fixture.presenter.shown();
        fixture.empty();
        fixture.retry();
        advance(100_000);
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert_eq!(fixture.mutation.empties(), 0);
        assert_eq!(fixture.bin.reads(), 1);
        assert!(fixture.presenter.state.borrow().mutation_provider.is_none());
        assert!(!fixture.dock.get_recycle_empty_enabled());
        assert!(!fixture.dock.get_recycle_empty_busy());
    }

    #[test]
    fn empty_hidden_accepted_return_caches_status_without_projection_and_defers_readback() {
        let fixture = Fixture::new();
        fixture.snapshot(12);
        fixture.empty();
        let pending_notice = fixture.dock.get_recycle_empty_notice();
        fixture.hide();
        assert!(
            !fixture.dock.get_recycle_empty_enabled(),
            "hidden authority is projected before Dock hide"
        );
        fixture.mutation.finish(Ok(outcome(1)));
        fixture.drain();
        fixture.empty();
        fixture.retry();
        advance(100_000);
        assert_eq!(fixture.bin.reads(), 1);
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "12 items");
        assert_eq!(fixture.dock.get_recycle_empty_notice(), pending_notice);
        assert!(!fixture.dock.window().is_visible());
        {
            let state = fixture.presenter.state.borrow();
            assert_eq!(state.empty_result, Some(Ok(outcome(1))));
            assert!(state.dirty);
            assert!(state.readback_needed);
            assert!(state.empty.is_none());
        }
        assert!(!fixture.presenter.throttle.running());
        fixture.show();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "12 items");
        assert!(
            fixture
                .dock
                .get_recycle_empty_notice()
                .contains("0x00000001")
        );
        assert!(fixture.dock.get_recycle_empty_enabled());
        assert!(fixture.dock.get_recycle_stale());
        fixture.bin.finish_read(Ok(info(13)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "13 items");
        assert!(!fixture.dock.get_recycle_stale());
        fixture.hide();
        fixture.show();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.mutation.empties(), 1);
    }

    #[test]
    fn empty_hidden_obsolete_read_retires_once_and_resume_starts_required_fresh_read() {
        let fixture = Fixture::new();
        fixture.snapshot(15);
        fixture.retry();
        fixture.empty();
        fixture.hide();
        fixture.mutation.finish(Ok(outcome(-1)));
        fixture.drain();
        assert!(fixture.presenter.state.borrow().obsolete_read.is_some());
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::AccessDenied)));
        fixture.drain();
        {
            let state = fixture.presenter.state.borrow();
            assert!(state.read.is_none());
            assert!(state.obsolete_read.is_none());
            assert!(
                state.read_error.is_none(),
                "obsolete failure is not a current failure"
            );
            assert!(state.readback_needed);
            assert!(!state.automatic_read_blocked);
            assert_eq!(state.snapshot, Some(info(15)));
        }
        advance(100_000);
        assert_eq!(fixture.bin.reads(), 2);
        assert!(!fixture.presenter.throttle.running());
        fixture.show();
        assert_eq!(fixture.bin.reads(), 3);
        fixture.bin.finish_read(Ok(info(16)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "16 items");
        assert_eq!(fixture.mutation.empties(), 1);
    }

    #[test]
    fn empty_close_retires_before_guard_drop_and_late_values_do_not_revive_dock() {
        let fixture = Fixture::new();
        fixture.snapshot(5);
        fixture.retry();
        fixture.open();
        fixture.empty();
        let presenter = fixture.presenter.clone();
        let dock = fixture.dock.as_weak();
        DROP_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                assert!(presenter.closed.get());
                let state = presenter.state.borrow();
                assert!(state.empty.is_none());
                assert!(state.mutation_acquiring.is_none());
                assert!(state.mutation_provider.is_none());
                assert!(!state.readback_needed);
                drop(state);
                assert!(presenter.mailbox.lock().closed);
                assert!(!presenter.mailbox.lock().pending());
                assert!(!dock.upgrade().unwrap().get_recycle_empty_enabled());
                presenter.request(DockRecycleAction::Empty);
                presenter.shown();
            }));
        });
        fixture.presenter.close();
        assert!(!fixture.dock.get_recycle_empty_busy());
        let dock = fixture.dock.as_weak();
        let presenter = Rc::downgrade(&fixture.presenter);
        let mutation = fixture.mutation.clone();
        let bin = fixture.bin.clone();
        let mailbox = fixture.presenter.mailbox.clone();
        drop(fixture);
        assert!(presenter.upgrade().is_none());
        assert!(dock.upgrade().is_none());
        std::thread::spawn(move || {
            mutation.finish(Ok(outcome(0)));
            bin.finish_read(Ok(info(0)));
            bin.finish_open(Ok(()));
            bin.emit(0, RecycleBinWatchEvent::Invalidated);
            assert_eq!(mutation.empties(), 1);
            assert_eq!(bin.reads(), 2);
            assert_eq!(bin.opens(), 1);
        })
        .join()
        .unwrap();
        advance(100_000);
        assert!(!mailbox.lock().pending());
        assert!(mailbox.lock().expected_empty.is_none());
        assert!(dock.upgrade().is_none());
    }

    #[test]
    fn empty_close_during_entered_dispatch_discards_inline_rejected_and_late_results() {
        for reply in [
            Reply::Inline(Ok(outcome(0))),
            Reply::Reject(error(DockUtilityErrorKind::Other)),
            Reply::Pending,
        ] {
            let fixture = Fixture::new();
            fixture.snapshot(6);
            fixture.mutation.replies.lock().push_back(reply);
            let presenter = fixture.presenter.clone();
            EMPTY_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert!(presenter.state.borrow().empty.as_ref().unwrap().dispatched);
                    presenter.close();
                    presenter.request(DockRecycleAction::Empty);
                }));
            });
            fixture.empty();
            let pending = fixture.mutation.pending.lock().is_some();
            if pending {
                fixture.mutation.finish(Ok(outcome(0)));
            }
            fixture.drain();
            fixture.presenter.shown();
            fixture.retry();
            advance(100_000);
            assert_eq!(fixture.mutation.empties(), 1);
            assert_eq!(
                fixture.bin.reads(),
                1,
                "close cancels undispatched UI readback demand"
            );
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "6 items");
            assert!(!fixture.dock.get_recycle_empty_enabled());
            assert!(!fixture.presenter.mailbox.lock().pending());
            assert!(fixture.presenter.state.borrow().empty_result.is_none());
            assert!(fixture.feedback.borrow().is_empty());
        }
    }

    #[test]
    fn empty_raw_zero_positive_and_negative_statuses_are_fixed_hex_without_outcome_inference() {
        for (status, hex, failed) in [
            (0, "0x00000000", false),
            (1, "0x00000001", false),
            (i32::MAX, "0x7FFFFFFF", false),
            (-2_147_467_259, "0x80004005", true),
            (i32::MIN, "0x80000000", true),
        ] {
            let fixture = Fixture::new();
            fixture.snapshot(7);
            fixture.empty();
            fixture.mutation.finish(Ok(outcome(status)));
            fixture.drain();
            assert_eq!(
                fixture.presenter.state.borrow().empty_result,
                Some(Ok(outcome(status)))
            );
            let notice = fixture.dock.get_recycle_empty_notice();
            assert!(notice.contains(hex), "{notice}");
            assert_eq!(notice.contains("failure status"), failed);
            assert!(notice.contains("returned"));
            assert!(notice.contains("State refresh requested."));
            assert!(!notice.contains("emptied"));
            assert!(!notice.contains("canceled"));
            assert!(!notice.contains("confirmed"));
            assert!(notice.len() < 128);
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "7 items");
            assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
            assert!(fixture.dock.get_recycle_stale());
            assert_eq!(fixture.bin.reads(), 2);
            fixture.bin.finish_read(Ok(info(8)));
            fixture.drain();
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "8 items");
            assert!(!fixture.dock.get_recycle_stale());
            assert_eq!(fixture.dock.get_recycle_empty_notice(), notice);
            assert!(fixture.feedback.borrow().is_empty());
            advance(100_000);
            assert_eq!(fixture.bin.reads(), 2);
            assert_eq!(fixture.mutation.empties(), 1);
        }
    }

    #[test]
    fn empty_obsolete_read_success_or_failure_retires_once_without_projection_before_fresh_success()
    {
        for old_result in [Ok(info(0)), Err(error(DockUtilityErrorKind::AccessDenied))] {
            let fixture = Fixture::new();
            fixture.snapshot(20);
            fixture.retry();
            let obsolete = fixture.presenter.state.borrow().read.unwrap();
            fixture.empty();
            fixture.mutation.finish(Ok(outcome(0)));
            fixture.drain();
            assert_eq!(fixture.presenter.state.borrow().read, Some(obsolete));
            assert_eq!(
                fixture.presenter.state.borrow().obsolete_read,
                Some(obsolete)
            );
            assert!(fixture.presenter.state.borrow().readback_needed);
            assert_eq!(
                fixture.bin.reads(),
                2,
                "do not create simultaneous aggregate reads"
            );
            assert!(!fixture.presenter.throttle.running());
            fixture.bin.finish_read(old_result);
            fixture.drain();
            let fresh = fixture.presenter.state.borrow().read.unwrap();
            assert_ne!(fresh, obsolete);
            assert!(fresh.0 > obsolete.0);
            assert!(fixture.presenter.state.borrow().obsolete_read.is_none());
            assert!(!fixture.presenter.state.borrow().readback_needed);
            assert!(fixture.presenter.state.borrow().read_error.is_none());
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "20 items");
            assert_eq!(fixture.bin.reads(), 3);
            read_complete(
                &fixture.presenter.mailbox,
                &fixture.presenter.dock,
                obsolete,
                Ok(info(0)),
            );
            fixture.drain();
            assert_eq!(fixture.presenter.state.borrow().read, Some(fresh));
            assert_eq!(
                fixture.bin.reads(),
                3,
                "late duplicate cannot dispatch another read"
            );
            fixture.bin.finish_read(Ok(info(21)));
            fixture.drain();
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "21 items");
            assert!(!fixture.dock.get_recycle_stale());
            assert!(!fixture.dock.get_recycle_read_busy());
            advance(100_000);
            assert_eq!(fixture.bin.reads(), 3);
            assert_eq!(fixture.mutation.empties(), 1);
        }
    }

    #[test]
    fn empty_required_fresh_failure_retains_last_good_and_blocks_idle_retry_until_real_event() {
        for reject_fresh in [false, true] {
            let fixture = Fixture::new();
            fixture.snapshot(22);
            fixture.retry();
            fixture.empty();
            fixture.mutation.finish(Ok(outcome(-1)));
            fixture.drain();
            if reject_fresh {
                fixture
                    .bin
                    .read_replies
                    .lock()
                    .push_back(Reply::Reject(error(DockUtilityErrorKind::Busy)));
            }
            fixture
                .bin
                .finish_read(Err(error(DockUtilityErrorKind::AccessDenied)));
            fixture.drain();
            assert_eq!(
                fixture.bin.reads(),
                3,
                "old failure must not suppress required fresh read"
            );
            if !reject_fresh {
                fixture
                    .bin
                    .finish_read(Err(error(DockUtilityErrorKind::Busy)));
            }
            fixture.drain();
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "22 items");
            assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Full);
            assert!(fixture.dock.get_recycle_stale());
            assert!(
                fixture
                    .dock
                    .get_recycle_read_notice()
                    .contains("provider is busy")
            );
            {
                let state = fixture.presenter.state.borrow();
                assert!(state.automatic_read_blocked);
                assert!(state.dirty);
                assert!(state.read.is_none());
                assert!(state.obsolete_read.is_none());
                assert!(!state.readback_needed);
            }
            advance(100_000);
            fixture.drain();
            assert_eq!(fixture.bin.reads(), 3);
            assert!(!fixture.presenter.throttle.running());
            assert_eq!(fixture.mutation.empties(), 1);
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
            fixture.drain();
            advance(99);
            assert_eq!(fixture.bin.reads(), 3);
            advance(1);
            assert_eq!(fixture.bin.reads(), 4);
            fixture.bin.finish_read(Ok(info(23)));
            fixture.drain();
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "23 items");
            assert_eq!(
                fixture.mutation.empties(),
                1,
                "watch recovery never repeats mutation"
            );
        }
    }

    #[test]
    fn empty_unknown_and_required_fresh_failure_never_publish_optimistic_zero() {
        let fixture = Fixture::new();
        fixture.start_read();
        fixture.empty();
        fixture.mutation.finish(Ok(outcome(0)));
        fixture.bin.finish_read(Ok(info(0)));
        fixture.drain();
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "Loading…");
        assert!(
            fixture.presenter.state.borrow().snapshot.is_none(),
            "obsolete zero is not a snapshot"
        );
        fixture
            .bin
            .finish_read(Err(error(DockUtilityErrorKind::Unavailable)));
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Unknown);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "Unavailable");
        assert!(!fixture.dock.get_recycle_empty_notice().contains("emptied"));
        assert!(fixture.dock.get_recycle_empty_enabled());
        advance(100_000);
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.mutation.empties(), 1);
    }

    #[test]
    fn empty_shared_mailbox_barrier_discards_pre_return_read_in_either_completion_order() {
        for read_first in [false, true] {
            let fixture = Fixture::new();
            fixture.snapshot(30);
            fixture.retry();
            fixture.open();
            fixture.empty();
            fixture.presenter.mailbox.lock().wake_queued = true;
            if read_first {
                fixture.bin.finish_read(Ok(info(0)));
                fixture.mutation.finish(Ok(outcome(0)));
            } else {
                fixture.mutation.finish(Ok(outcome(0)));
                fixture
                    .bin
                    .finish_read(Err(error(DockUtilityErrorKind::AccessDenied)));
            }
            fixture.bin.finish_open(Ok(()));
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
            {
                let mailbox = fixture.presenter.mailbox.lock();
                assert!(mailbox.wake_queued);
                assert!(mailbox.read.is_some());
                assert!(mailbox.empty.is_some());
                assert!(mailbox.open.is_some());
                assert!(mailbox.dirty);
            }
            fixture.drain();
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "30 items");
            assert!(fixture.presenter.state.borrow().read_error.is_none());
            assert_eq!(
                fixture.bin.reads(),
                3,
                "one required fresh read despite shared dirty/result batch"
            );
            assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
            assert!(!fixture.dock.get_recycle_empty_busy());
            fixture.bin.finish_read(Ok(info(31)));
            fixture.drain();
            advance(100_000);
            assert_eq!(fixture.dock.get_recycle_item_count_label(), "31 items");
            assert_eq!(fixture.bin.reads(), 3);
            assert_eq!(fixture.mutation.empties(), 1);
        }
    }

    #[test]
    fn empty_read_started_after_terminal_publication_can_satisfy_barrier_before_ui_drain() {
        let fixture = Fixture::new();
        fixture.snapshot(40);
        fixture.empty();
        fixture.mutation.finish(Ok(outcome(1)));
        fixture.retry();
        assert_eq!(fixture.bin.reads(), 2);
        let fresh = fixture.presenter.state.borrow().read.unwrap();
        let cutoff = fixture
            .presenter
            .mailbox
            .lock()
            .empty
            .as_ref()
            .unwrap()
            .read_cutoff
            .unwrap();
        assert!(
            fresh.0 > cutoff.0,
            "read was genuinely issued after the terminal publication"
        );
        fixture.bin.finish_read(Ok(info(0)));
        fixture.drain();
        assert_eq!(
            fixture.bin.reads(),
            2,
            "already post-return read satisfies the barrier exactly once"
        );
        assert_eq!(fixture.dock.get_recycle_state(), DockRecycleState::Empty);
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "0 items");
        assert!(!fixture.dock.get_recycle_stale());
        assert!(!fixture.presenter.state.borrow().readback_needed);
        assert!(fixture.presenter.state.borrow().obsolete_read.is_none());
        advance(100_000);
        assert_eq!(fixture.bin.reads(), 2);
        assert_eq!(fixture.mutation.empties(), 1);
        assert!(fixture.feedback.borrow().is_empty());
    }

    #[test]
    fn empty_worker_slot_is_bounded_preserves_first_terminal_and_watch_followup_without_idle_loop()
    {
        let fixture = Fixture::new();
        fixture.snapshot(50);
        fixture.retry();
        fixture.open();
        fixture.empty();
        let (empty_token, old_read) = {
            let state = fixture.presenter.state.borrow();
            (state.empty.as_ref().unwrap().token, state.read.unwrap())
        };
        fixture.presenter.mailbox.lock().wake_queued = true;
        let bin = fixture.bin.clone();
        let mutation = fixture.mutation.clone();
        let mailbox = fixture.presenter.mailbox.clone();
        let dock = fixture.presenter.dock.clone();
        std::thread::spawn(move || {
            for _ in 0..10_000 {
                bin.emit(0, RecycleBinWatchEvent::Invalidated);
            }
            mutation.finish(Err(error(DockUtilityErrorKind::Unavailable)));
            bin.finish_read(Ok(info(0)));
            bin.finish_open(Ok(()));
            for _ in 0..10_000 {
                empty_complete(&mailbox, &dock, empty_token, Ok(outcome(0)));
                read_complete(&mailbox, &dock, old_read, Err(DockUtilityErrorKind::Other));
            }
        })
        .join()
        .unwrap();
        {
            let mailbox = fixture.presenter.mailbox.lock();
            assert!(mailbox.wake_queued);
            assert!(mailbox.dirty);
            assert!(mailbox.read.is_some());
            assert!(mailbox.open.is_some());
            let returned = mailbox.empty.as_ref().unwrap();
            assert_eq!(returned.result, Err(DockUtilityErrorKind::Unavailable));
            assert_eq!(returned.read_cutoff, Some(old_read));
            assert_eq!(mailbox.expected_empty, Some(empty_token));
            assert!(mailbox.ready.is_none());
            assert!(mailbox.unavailable.is_none());
        }
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "50 items");
        assert_eq!(fixture.bin.reads(), 3);
        assert!(
            fixture
                .dock
                .get_recycle_empty_notice()
                .contains("is unavailable")
        );
        assert!(!fixture.dock.get_recycle_empty_notice().contains("PRIVATE"));
        assert_eq!(*fixture.feedback.borrow(), vec![Ok(())]);
        assert!(fixture.presenter.mailbox.lock().empty.is_none());
        assert!(fixture.presenter.mailbox.lock().expected_empty.is_none());
        for _ in 0..10_000 {
            fixture.bin.emit(0, RecycleBinWatchEvent::Invalidated);
        }
        fixture.drain();
        advance(100);
        assert_eq!(
            fixture.bin.reads(),
            3,
            "dirty fresh read cannot overlap another read"
        );
        fixture.bin.finish_read(Ok(info(51)));
        fixture.drain();
        assert_eq!(
            fixture.bin.reads(),
            4,
            "exactly one existing rate-bounded dirty followup"
        );
        fixture.bin.finish_read(Ok(info(52)));
        fixture.drain();
        empty_complete(
            &fixture.presenter.mailbox,
            &fixture.presenter.dock,
            empty_token,
            Ok(outcome(0)),
        );
        fixture.drain();
        advance(100_000);
        fixture.drain();
        assert_eq!(fixture.dock.get_recycle_item_count_label(), "52 items");
        assert_eq!(fixture.bin.reads(), 4);
        assert_eq!(fixture.mutation.empties(), 1);
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert_eq!(fixture.feedback.borrow().len(), 1);
        assert!(!fixture.presenter.throttle.running());
        assert!(!fixture.presenter.mailbox.lock().pending());
    }

    #[test]
    fn empty_enabled_projection_callback_is_pure_live_authority_and_preserves_root_callbacks() {
        let fixture = Fixture::new();
        let projections = Rc::new(RefCell::new(Vec::new()));
        let observed = projections.clone();
        let dock = fixture.dock.as_weak();
        let presenter = Rc::downgrade(&fixture.presenter);
        fixture.dock.on_recycle_projection_changed(move || {
            let dock = dock.upgrade().unwrap();
            let presenter = presenter.upgrade().unwrap();
            let state = presenter.state.borrow();
            let enabled = dock.get_recycle_empty_enabled();
            assert_eq!(
                enabled,
                !presenter.closed.get()
                    && state.visible
                    && dock.window().is_visible()
                    && state.empty.is_none()
            );
            observed.borrow_mut().push(enabled);
        });
        fixture.snapshot(60);
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&true));
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            0
        );
        fixture.hide();
        assert!(!fixture.dock.get_recycle_empty_enabled());
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&false));
        assert_eq!(fixture.bin.reads(), 1, "hidden projection starts no read");
        fixture.show();
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&true));
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            0
        );
        fixture.empty();
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&false));
        assert_eq!(fixture.mutation.empties(), 1);
        fixture.mutation.finish(Ok(outcome(0)));
        fixture.drain();
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&true));
        assert_eq!(fixture.bin.reads(), 2);
        fixture.presenter.close();
        assert!(!fixture.dock.get_recycle_empty_enabled());
        slint::platform::update_timers_and_animations();
        assert_eq!(projections.borrow().last(), Some(&false));
        assert_eq!(
            fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
            1
        );
        assert_eq!(fixture.mutation.empties(), 1);
        assert!(
            fixture.wakes.get() >= 3,
            "constructor/render preserve the root event callback"
        );
        assert!(fixture.feedback.borrow().is_empty());
    }

    #[test]
    fn empty_reserved_readback_rechecks_hidden_or_closed_before_native_read_entry() {
        for close in [false, true] {
            let fixture = Fixture::new();
            fixture.snapshot(70);
            fixture
                .mutation
                .replies
                .lock()
                .push_back(Reply::Inline(Ok(outcome(0))));
            let presenter = fixture.presenter.clone();
            EMPTY_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert_eq!(presenter.process_events(), None);
                    let effect = {
                        let mut state = presenter.state.borrow_mut();
                        assert!(state.readback_needed);
                        presenter.next_effect(&mut state)
                    };
                    let Some(Effect::Read(token, provider)) = effect else {
                        panic!("genuine terminal return must demand one aggregate read");
                    };
                    assert_ne!(presenter.mailbox.lock().latest_read, Some(token));
                    if close {
                        presenter.close();
                    } else {
                        presenter.hidden();
                    }
                    presenter.start_read(token, provider);
                }));
            });
            fixture.empty();
            assert_eq!(
                fixture.bin.reads(),
                1,
                "undispatched readback has no native entry after authority loss"
            );
            assert!(fixture.presenter.state.borrow().read.is_none());
            assert!(fixture.presenter.mailbox.lock().expected_read.is_none());
            assert!(!fixture.dock.get_recycle_empty_enabled());
            if !close {
                assert!(fixture.presenter.state.borrow().readback_needed);
                fixture.show();
                assert_eq!(fixture.bin.reads(), 2);
                fixture.bin.finish_read(Ok(info(71)));
                fixture.drain();
                assert_eq!(fixture.dock.get_recycle_item_count_label(), "71 items");
            } else {
                fixture.presenter.shown();
                assert!(!fixture.presenter.state.borrow().readback_needed);
                assert_eq!(fixture.bin.reads(), 1);
            }
            assert_eq!(fixture.mutation.empties(), 1);
        }
    }

    #[test]
    fn empty_cached_provider_rechecks_hidden_or_closed_before_entering_next_explicit_intent() {
        for close in [false, true] {
            let fixture = Fixture::new();
            fixture.snapshot(80);
            fixture
                .mutation
                .replies
                .lock()
                .push_back(Reply::Inline(Ok(outcome(0))));
            let presenter = fixture.presenter.clone();
            EMPTY_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    assert_eq!(presenter.process_events(), None);
                    presenter.request(DockRecycleAction::Empty);
                    let effect = {
                        let mut state = presenter.state.borrow_mut();
                        presenter.next_effect(&mut state)
                    };
                    let Some(Effect::Empty(token, provider)) = effect else {
                        panic!("new explicit current intent should use cached mutation capability");
                    };
                    assert!(!presenter.state.borrow().empty.as_ref().unwrap().dispatched);
                    if close {
                        presenter.close();
                    } else {
                        presenter.hidden();
                    }
                    presenter.start_empty(token, provider);
                }));
            });
            fixture.empty();
            assert_eq!(
                fixture.mutation.empties(),
                1,
                "selected effect is not accepted native work"
            );
            assert!(fixture.presenter.state.borrow().empty.is_none());
            assert!(fixture.presenter.mailbox.lock().expected_empty.is_none());
            assert_eq!(
                fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                1
            );
            if !close {
                fixture.show();
                assert_eq!(
                    fixture.mutation.empties(),
                    1,
                    "reshow reconciles only the genuine first return"
                );
                assert_eq!(fixture.bin.reads(), 2);
                fixture.bin.finish_read(Ok(info(81)));
                fixture.drain();
                fixture.empty();
                assert_eq!(
                    fixture.mutation.empties(),
                    2,
                    "only a new live explicit request enters again"
                );
                assert_eq!(
                    fixture.host.mutation_factory_calls.load(Ordering::Relaxed),
                    1
                );
            } else {
                fixture.presenter.shown();
                fixture.empty();
                assert_eq!(fixture.mutation.empties(), 1);
                assert_eq!(fixture.bin.reads(), 1);
            }
        }
    }
}
