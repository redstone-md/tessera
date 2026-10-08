// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Genuine aggregate state and fixed open intent, independent of desktop observation.

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
    watch_needed: bool,
    watch: Option<Token>,
    ready_pending: bool,
    watch_live: bool,
    watch_guard: Option<Box<dyn RecycleBinWatchGuard>>,
    read: Option<Token>,
    open: Option<Flight>,
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
    expected_open: Option<Token>,
    open: ResultSlot<()>,
    wake_queued: bool,
}

impl Mailbox {
    fn pending(&self) -> bool {
        self.ready.is_some()
            || self.unavailable.is_some()
            || self.dirty
            || self.read.is_some()
            || self.open.is_some()
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
    Watch(Token, Arc<dyn RecycleBinHost>),
    Read(Token, Arc<dyn RecycleBinHost>),
    Open(Token, Arc<dyn RecycleBinHost>),
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

    /// Passive watch and accepted effects survive hiding; undispatched open never replays.
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
        self.throttle.stop();
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

    /// Read/watch results render safe notices; only an explicit open returns root feedback.
    pub(crate) fn process_events(self: &Rc<Self>) -> Option<Result<(), String>> {
        if self.closed.get() {
            return None;
        }
        let (ready, unavailable, dirty, read, open) = {
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
            (
                mailbox.ready.take(),
                mailbox.unavailable.take(),
                std::mem::take(&mut mailbox.dirty),
                read,
                open,
            )
        };
        let mut retired_guard = None;
        let mut retired_watch = None;
        let mut open_result = None;
        {
            let mut state = self.state.borrow_mut();
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
        let (guard, provider) = {
            let mut state = self.state.borrow_mut();
            state.visible = false;
            state.acquiring = None;
            state.watch = None;
            state.ready_pending = false;
            state.watch_live = false;
            state.read = None;
            state.open = None;
            state.timer = None;
            (state.watch_guard.take(), state.provider.take())
        };
        *self.mailbox.lock() = Mailbox {
            closed: true,
            ..Mailbox::default()
        };
        self.throttle.stop();
        if let Some(dock) = self.dock.upgrade() {
            dock.set_recycle_read_busy(false);
            dock.set_recycle_open_busy(false);
        }
        drop(guard);
        drop(provider);
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
                Some(Effect::Watch(token, provider)) => self.start_watch(token, provider),
                Some(Effect::Read(token, provider)) => self.start_read(token, provider),
                Some(Effect::Open(token, provider)) => self.start_open(token, provider),
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
        if !state.ready_pending
            && state.read.is_none()
            && (state.read_needed
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

    fn render(&self) {
        if self.closed.get() {
            return;
        }
        let (visible, recycle_state, count, read_busy, open_busy, stale, read, watch, open) = {
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
            (
                state.visible,
                recycle_state,
                count,
                read_busy,
                state.open.is_some(),
                state.snapshot.is_some()
                    && (state.dirty
                        || state.read.is_some()
                        || !state.watch_live
                        || state.read_error.is_some()
                        || state.watch_error.is_some()),
                read,
                watch,
                open,
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
    let next = if operation == "Open" {
        "Try opening again."
    } else {
        "Retry explicitly."
    };
    format!("Recycle Bin {operation}: {reason}. {next}")
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

    type Hook = RefCell<Option<Box<dyn FnOnce()>>>;
    thread_local! {
        static BACKEND_INITIALIZED: Cell<bool> = const { Cell::new(false) };
        static FACTORY_HOOK: Hook = RefCell::default();
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

    struct RecordingDesktop {
        provider: Mutex<Result<Option<Arc<dyn RecycleBinHost>>, DockUtilityError>>,
        factory_calls: AtomicUsize,
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
    }

    struct Fixture {
        dock: Dock,
        bin: Arc<RecordingBin>,
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
            let host = Arc::new(RecordingDesktop {
                provider: Mutex::new(Ok(Some(bin.clone()))),
                factory_calls: AtomicUsize::new(0),
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
}
