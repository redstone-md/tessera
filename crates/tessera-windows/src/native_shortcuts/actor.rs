// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::{Backend, BackendEvent, Gate, RawTrigger, Wake, unavailable};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use tessera_system::shortcuts::{
    ShortcutCompletion, ShortcutConfig, ShortcutError, ShortcutErrorKind, ShortcutEvent,
    ShortcutHost, ShortcutSink, ShortcutSnapshot, ShortcutSubscription, ShortcutTrigger,
};

const COMMAND_CAPACITY: usize = 16;
const EVENT_CAPACITY: usize = 32;
// Accepted completions, admitted triggers and one terminal owner fault each
// reserve disjoint slots. A saturated trigger queue cannot hide an owner fault.
const RELAY_CAPACITY: usize = COMMAND_CAPACITY + EVENT_CAPACITY + 1;

/// Reserve one bounded relay slot, returning the previous count. The checked
/// CAS loop supports the project's MSRV without version-specific atomic APIs.
pub(super) fn reserve_event_slot(events: &AtomicUsize) -> Result<usize, usize> {
    let mut current = events.load(Ordering::Acquire);
    loop {
        let Some(next) = current
            .checked_add(1)
            .filter(|next| *next <= EVENT_CAPACITY)
        else {
            return Err(current);
        };
        match events.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(previous) => return Ok(previous),
            Err(actual) => current = actual,
        }
    }
}

struct Configure {
    config: ShortcutConfig,
    generation: u64,
    completion: ShortcutCompletion,
}

enum Command {
    Configure(Configure),
    Wake,
}

struct Listener {
    alive: AtomicBool,
    sink: ShortcutSink,
}

struct Shared {
    gate: Arc<Gate>,
    admission: Mutex<u64>,
    pending: AtomicUsize,
    fault_queued: AtomicBool,
    commands: SyncSender<Command>,
    delivery: SyncSender<Delivery>,
    wake: Mutex<Option<Wake>>,
    listener: Mutex<Option<Arc<Listener>>>,
}

impl Shared {
    fn wake(&self) {
        // Rust waiters (lazy or faulted) need a queue wake too; a full command
        // queue already guarantees work. Native waiters use the event below.
        let _ = self.commands.try_send(Command::Wake);
        let wake = self.wake.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    fn current(&self, generation: u64) -> bool {
        generation != 0
            && !self.gate.closed.load(Ordering::Acquire)
            && self.gate.listening.load(Ordering::Acquire)
            && self.gate.generation.load(Ordering::Acquire) == generation
    }

    fn complete(&self, command: Configure, result: Result<ShortcutSnapshot, ShortcutError>) {
        // Capacity is reserved at admission, separately from event reservations.
        // No callbacks run here and the native owner never waits on a consumer.
        let delivery = Delivery::Completed(command.completion, result);
        // Every queued item owns a distinct reservation. Relay capacity exceeds
        // all reservations; this send cannot wait for callback progress. The
        // relay contains user panics and retires only when all senders retire.
        let _ = self.delivery.send(delivery);
    }

    fn trigger(&self, backend: &mut impl Backend, trigger: RawTrigger) {
        if !self.current(trigger.generation) {
            if trigger.reserved {
                self.gate.events.fetch_sub(1, Ordering::AcqRel);
            }
            return;
        }
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(listener) = listener.filter(|listener| listener.alive.load(Ordering::Acquire))
        else {
            if trigger.reserved {
                self.gate.events.fetch_sub(1, Ordering::AcqRel);
            }
            return;
        };
        if !trigger.reserved && reserve_event_slot(&self.gate.events).is_err() {
            return;
        }
        // One explicit cursor query per admitted trigger, outside the hook.
        let event = ShortcutEvent::Triggered(ShortcutTrigger {
            generation: trigger.generation,
            action: trigger.action,
            cursor: backend.cursor(),
        });
        if self
            .delivery
            .try_send(Delivery::Event(listener, event))
            .is_err()
        {
            self.gate.events.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn fault(&self, error: ShortcutError, failed_generation: Option<u64>) {
        if let Some(generation) = failed_generation {
            let _ = self.gate.generation.compare_exchange(
                generation,
                0,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        } else {
            self.gate.generation.store(0, Ordering::Release);
        }
        let listener = self
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(listener) = listener
            && !self.fault_queued.swap(true, Ordering::AcqRel)
        {
            let _ = self.delivery.send(Delivery::Fault(listener, error));
        }
    }
}

enum Delivery {
    Completed(ShortcutCompletion, Result<ShortcutSnapshot, ShortcutError>),
    Event(Arc<Listener>, ShortcutEvent),
    Fault(Arc<Listener>, ShortcutError),
}

struct Host {
    shared: Arc<Shared>,
}

impl ShortcutHost for Host {
    fn configure(
        &self,
        config: ShortcutConfig,
        completion: ShortcutCompletion,
    ) -> Result<(), ShortcutError> {
        // Typed enum variants are public; validate again at the native ingress.
        config
            .effective_chord(tessera_system::shortcuts::ShortcutAction::OpenSettings)
            .validate()?;
        let mut generation = self
            .shared
            .admission
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.shared.gate.closed.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        if self.shared.pending.load(Ordering::Acquire) >= COMMAND_CAPACITY {
            return Err(ShortcutError::new(
                ShortcutErrorKind::Busy,
                None,
                "Shortcut configuration queue is full",
            ));
        }
        let Some(next) = generation.checked_add(1) else {
            self.shared.gate.closed.store(true, Ordering::Release);
            self.shared.gate.generation.store(0, Ordering::Release);
            self.shared.wake();
            return Err(ShortcutError::new(
                ShortcutErrorKind::Other,
                None,
                "Shortcut generation exhausted",
            ));
        };
        // Invalidation precedes queue publication. Even immediate callbacks from
        // the retiring registration cannot be attributed to the new config.
        *generation = next;
        self.shared.gate.generation.store(next, Ordering::Release);
        self.shared.pending.fetch_add(1, Ordering::AcqRel);
        let command = Configure {
            config,
            generation: next,
            completion,
        };
        if self
            .shared
            .commands
            .try_send(Command::Configure(command))
            .is_err()
        {
            self.shared.pending.fetch_sub(1, Ordering::AcqRel);
            self.shared.gate.generation.store(0, Ordering::Release);
            return Err(ShortcutError::new(
                ShortcutErrorKind::Busy,
                None,
                "Shortcut owner cannot accept configuration",
            ));
        }
        self.shared.wake();
        Ok(())
    }

    fn subscribe(
        &self,
        sink: ShortcutSink,
    ) -> Result<Box<dyn ShortcutSubscription>, ShortcutError> {
        let mut slot = self
            .shared
            .listener
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.shared.gate.closed.load(Ordering::Acquire) {
            return Err(unavailable());
        }
        // A dead-but-occupied slot is a destructor still retiring its authority.
        // Never replace it before revoke/watermark publication and slot removal.
        if slot.is_some() {
            return Err(ShortcutError::new(
                ShortcutErrorKind::Busy,
                None,
                "Shortcut listener already exists",
            ));
        }
        let listener = Arc::new(Listener {
            alive: AtomicBool::new(true),
            sink,
        });
        *slot = Some(listener.clone());
        self.shared.gate.listening.store(true, Ordering::Release);
        Ok(Box::new(Subscription {
            shared: Arc::downgrade(&self.shared),
            listener,
        }))
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.shared.gate.closed.store(true, Ordering::Release);
        self.shared.gate.generation.store(0, Ordering::Release);
        self.shared.gate.listening.store(false, Ordering::Release);
        self.shared.wake();
    }
}

struct Subscription {
    shared: Weak<Shared>,
    listener: Arc<Listener>,
}

impl ShortcutSubscription for Subscription {}

impl Drop for Subscription {
    fn drop(&mut self) {
        // Disable the hook gate before publishing listener death, without a
        // callback lock. Occupied slots cannot be replaced during retirement.
        let shared = self.shared.upgrade();
        if let Some(shared) = &shared {
            shared.gate.listening.store(false, Ordering::Release);
        }
        self.listener.alive.store(false, Ordering::Release);
        if let Some(shared) = shared {
            let mut slot = shared.listener.lock().unwrap_or_else(|e| e.into_inner());
            if slot
                .as_ref()
                .is_some_and(|listener| Arc::ptr_eq(listener, &self.listener))
            {
                // Admission and retirement share one ordering lock. A newer
                // listener may configure before this guard's wake runs, but its
                // resources must never be cleaned by the old retirement.
                let generation = shared.admission.lock().unwrap_or_else(|e| e.into_inner());
                *slot = None;
                shared.gate.generation.store(0, Ordering::Release);
                shared
                    .gate
                    .retirement
                    .fetch_max(*generation, Ordering::AcqRel);
                drop(generation);
                drop(slot);
                shared.wake();
            }
        }
    }
}

type ChannelState = (Arc<Shared>, Receiver<Command>, Receiver<Delivery>);

fn channel_state() -> ChannelState {
    let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY * 2);
    let (delivery, deliveries) = mpsc::sync_channel(RELAY_CAPACITY);
    let shared = Arc::new(Shared {
        gate: Arc::new(Gate {
            generation: AtomicU64::new(0),
            listening: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            retirement: AtomicU64::new(0),
            events: AtomicUsize::new(0),
        }),
        admission: Mutex::new(0),
        pending: AtomicUsize::new(0),
        fault_queued: AtomicBool::new(false),
        commands,
        delivery,
        wake: Mutex::new(None),
        listener: Mutex::new(None),
    });
    (shared, receiver, deliveries)
}

pub(super) fn create<B: Backend, F>(factory: F) -> Result<Arc<dyn ShortcutHost>, ShortcutError>
where
    F: FnOnce(Arc<Gate>) -> Result<B, ShortcutError> + Send + 'static,
{
    let (shared, receiver, deliveries) = channel_state();
    let weak = Arc::downgrade(&shared);
    std::thread::Builder::new()
        .name("tessera-shortcut-delivery".into())
        .spawn(move || relay(weak, deliveries))
        .map_err(|_| unavailable())?;
    let owner_shared = shared.clone();
    if std::thread::Builder::new()
        .name("tessera-shortcut-owner".into())
        .spawn(move || owner(owner_shared, receiver, factory))
        .is_err()
    {
        shared.gate.closed.store(true, Ordering::Release);
        return Err(unavailable());
    }
    Ok(Arc::new(Host { shared }))
}

fn relay(shared: Weak<Shared>, receiver: Receiver<Delivery>) {
    while let Ok(delivery) = receiver.recv() {
        match delivery {
            Delivery::Completed(completion, result) => {
                if let Some(shared) = shared.upgrade() {
                    shared.pending.fetch_sub(1, Ordering::AcqRel);
                }
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            }
            Delivery::Event(listener, event) => {
                let Some(shared) = shared.upgrade() else {
                    continue;
                };
                shared.gate.events.fetch_sub(1, Ordering::AcqRel);
                let current = match &event {
                    ShortcutEvent::Triggered(trigger) => shared.current(trigger.generation),
                    ShortcutEvent::Unavailable(_) => !shared.gate.closed.load(Ordering::Acquire),
                };
                if current && listener.alive.load(Ordering::Acquire) {
                    let _ = catch_unwind(AssertUnwindSafe(|| (listener.sink)(event)));
                }
            }
            Delivery::Fault(listener, error) => {
                let Some(shared) = shared.upgrade() else {
                    continue;
                };
                shared.fault_queued.store(false, Ordering::Release);
                if !shared.gate.closed.load(Ordering::Acquire)
                    && listener.alive.load(Ordering::Acquire)
                {
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        (listener.sink)(ShortcutEvent::Unavailable(error))
                    }));
                }
            }
        }
    }
}

fn retirement_due(owned_generation: u64, retired_generation: u64) -> bool {
    owned_generation != 0 && owned_generation <= retired_generation
}

fn owner<B: Backend, F>(shared: Arc<Shared>, receiver: Receiver<Command>, factory: F)
where
    F: FnOnce(Arc<Gate>) -> Result<B, ShortcutError>,
{
    let mut factory = Some(factory);
    let mut backend: Option<B> = None;
    let mut terminal_error: Option<ShortcutError> = None;
    let mut owned_generation = 0;
    loop {
        if shared.gate.closed.load(Ordering::Acquire) {
            if let Some(backend) = &mut backend {
                let _ = backend.cleanup();
            }
            while let Ok(command) = receiver.try_recv() {
                if let Command::Configure(command) = command {
                    shared.complete(command, Err(unavailable()));
                }
            }
            break;
        }
        let retired = shared.gate.retirement.swap(0, Ordering::AcqRel);
        if retirement_due(owned_generation, retired)
            && let Some(backend) = &mut backend
        {
            match backend.cleanup() {
                Ok(()) => owned_generation = 0,
                Err(error) => {
                    terminal_error = Some(error.clone());
                    shared.fault(error, None);
                }
            }
        }
        let command = if backend.is_none() || terminal_error.is_some() {
            // Lazy/faulted owners wait only for explicit commands, never scan.
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        } else {
            receiver.try_recv().ok()
        };
        if let Some(Command::Configure(command)) = command {
            let result = catch_unwind(AssertUnwindSafe(|| {
                if shared.gate.closed.load(Ordering::Acquire) {
                    return Err(unavailable());
                }
                if let Some(error) = &terminal_error {
                    return Err(error.clone());
                }
                if command.generation != shared.gate.generation.load(Ordering::Acquire) {
                    return Err(ShortcutError::new(
                        ShortcutErrorKind::Busy,
                        None,
                        "Shortcut configuration superseded",
                    ));
                }
                if let Some(backend) = &mut backend {
                    backend.cleanup()?;
                }
                owned_generation = 0;
                if !command.config.enabled() {
                    return Ok(ShortcutSnapshot::new(
                        &command.config,
                        command.generation,
                        Ok(()),
                        Ok(()),
                    ));
                }
                if backend.is_none() {
                    let Some(factory) = factory.take() else {
                        return Err(unavailable());
                    };
                    let initialized = factory(shared.gate.clone())?;
                    *shared.wake.lock().unwrap_or_else(|e| e.into_inner()) =
                        Some(initialized.wake());
                    backend = Some(initialized);
                }
                let Some(backend) = &mut backend else {
                    return Err(unavailable());
                };
                owned_generation = command.generation;
                let launcher = backend.install_launcher(command.generation);
                let settings = backend.register_settings(
                    &command
                        .config
                        .effective_chord(tessera_system::shortcuts::ShortcutAction::OpenSettings),
                    command.generation,
                );
                if command.generation != shared.gate.generation.load(Ordering::Acquire)
                    || shared.gate.closed.load(Ordering::Acquire)
                {
                    backend.cleanup()?;
                    return Err(ShortcutError::new(
                        ShortcutErrorKind::Busy,
                        None,
                        "Shortcut configuration superseded",
                    ));
                }
                Ok(ShortcutSnapshot::new(
                    &command.config,
                    command.generation,
                    launcher,
                    settings,
                ))
            }))
            .unwrap_or_else(|_| Err(unavailable()));
            if result.is_err() {
                shared
                    .gate
                    .generation
                    .compare_exchange(command.generation, 0, Ordering::AcqRel, Ordering::Acquire)
                    .ok();
                if let Some(backend) = &mut backend {
                    match backend.cleanup() {
                        Ok(()) => owned_generation = 0,
                        Err(error) => terminal_error = Some(error),
                    }
                }
            }
            shared.complete(command, result);
            continue;
        }
        if matches!(command, Some(Command::Wake)) {
            continue;
        }
        let Some(backend) = &mut backend else {
            continue;
        };
        let event = catch_unwind(AssertUnwindSafe(|| backend.wait()))
            .unwrap_or_else(|_| Err(unavailable()));
        match event {
            Ok(Some(BackendEvent::Triggered(trigger))) => shared.trigger(backend, trigger),
            Ok(Some(BackendEvent::RearmRequired)) => {
                let generation = owned_generation;
                let _ = shared.gate.generation.compare_exchange(
                    generation,
                    0,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                match backend.cleanup() {
                    Ok(()) => {
                        owned_generation = 0;
                        shared.fault(unavailable(), Some(generation));
                    }
                    Err(error) => {
                        terminal_error = Some(error.clone());
                        shared.fault(error, None);
                    }
                }
                // No retry, input replay, query or timer is scheduled here.
                // The same host can accept an explicit fresh configuration.
            }
            Ok(None) => {}
            Err(error) => {
                terminal_error = Some(error.clone());
                shared.fault(error, None);
                let _ = backend.cleanup();
                // Faulted owners remain available to complete accepted commands
                // honestly, without spinning or creating replacement hooks.
            }
        }
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::retirement_due;

    #[test]
    fn retired_generation_never_removes_newer_replacement_or_paused_resources() {
        for (owned, retired, expected) in [
            (0, 0, false),
            (0, 5, false),
            (1, 0, false),
            (5, 5, true),
            (5, 6, true),
            (6, 5, false),
            (u64::MAX, u64::MAX - 1, false),
            (u64::MAX, u64::MAX, true),
            (0, u64::MAX, false),
        ] {
            assert_eq!(retirement_due(owned, retired), expected);
        }
    }
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod admission_tests;

#[cfg(test)]
mod event_reservation_tests {
    use super::*;

    #[test]
    fn checked_reservation_preserves_previous_count_and_rejects_full_or_overflow() {
        let events = AtomicUsize::new(EVENT_CAPACITY - 1);
        assert_eq!(reserve_event_slot(&events), Ok(EVENT_CAPACITY - 1));
        assert_eq!(events.load(Ordering::Acquire), EVENT_CAPACITY);
        assert_eq!(reserve_event_slot(&events), Err(EVENT_CAPACITY));
        assert_eq!(events.load(Ordering::Acquire), EVENT_CAPACITY);
        events.store(usize::MAX, Ordering::Release);
        assert_eq!(reserve_event_slot(&events), Err(usize::MAX));
        assert_eq!(events.load(Ordering::Acquire), usize::MAX);
    }
}
