// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use crate::native_battery::NativeOwner;
use crate::single_flight::{Flight, FlightGate};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tessera_system::battery::{
    BatteryCompletion, BatteryError, BatteryEvent, BatteryHost, BatterySettingsCompletion,
};

type Events = Arc<dyn Fn(BatteryEvent) + Send + Sync>;

#[derive(Default)]
pub(super) struct LazyHost {
    worker: OnceLock<Result<Arc<Host>, BatteryError>>,
}
impl LazyHost {
    fn worker(&self) -> Result<&Arc<Host>, BatteryError> {
        self.worker
            .get_or_init(start)
            .as_ref()
            .map_err(|error| *error)
    }
}
impl BatteryHost for LazyHost {
    fn read(&self, completion: BatteryCompletion) -> Result<(), BatteryError> {
        self.worker()?.read(completion)
    }
    fn subscribe(&self, events: Events) -> Result<Option<Box<dyn Send>>, BatteryError> {
        self.worker()?.subscribe(events)
    }
    fn open_settings(&self, completion: BatterySettingsCompletion) -> Result<(), BatteryError> {
        self.worker()?.open_settings(completion)
    }
}

struct Read {
    completion: BatteryCompletion,
    flight: Flight,
}
struct Settings {
    completion: BatterySettingsCompletion,
    flight: Flight,
}
struct Subscriber {
    admitted: AtomicBool,
    ready: AtomicBool,
    events: Events,
}
#[derive(Default)]
struct State {
    stopped: bool,
    read: Option<Read>,
    settings: Option<Settings>,
    subscribers: BTreeMap<u64, Arc<Subscriber>>,
    next_id: u64,
    reconcile: bool,
}
struct Shared {
    state: Mutex<State>,
    sender: SyncSender<()>,
    dirty: AtomicBool,
    reads: FlightGate,
    settings: FlightGate,
}
impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn wake(&self) -> Result<(), BatteryError> {
        match self.sender.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => Ok(()),
            Err(TrySendError::Disconnected(())) => Err(BatteryError::Stopped),
        }
    }
    fn changed(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            let _ = self.wake();
        }
    }
}
struct Host {
    shared: Arc<Shared>,
}
impl BatteryHost for Host {
    fn read(&self, completion: BatteryCompletion) -> Result<(), BatteryError> {
        let flight = self.shared.reads.try_enter().ok_or(BatteryError::Busy)?;
        let mut state = self.shared.state();
        if state.stopped {
            return Err(BatteryError::Stopped);
        }
        state.read = Some(Read { completion, flight });
        if let Err(error) = self.shared.wake() {
            state.read.take();
            state.stopped = true;
            return Err(error);
        }
        Ok(())
    }
    fn open_settings(&self, completion: BatterySettingsCompletion) -> Result<(), BatteryError> {
        let flight = self.shared.settings.try_enter().ok_or(BatteryError::Busy)?;
        let mut state = self.shared.state();
        if state.stopped {
            return Err(BatteryError::Stopped);
        }
        state.settings = Some(Settings { completion, flight });
        if let Err(error) = self.shared.wake() {
            state.settings.take();
            state.stopped = true;
            return Err(error);
        }
        Ok(())
    }
    fn subscribe(&self, events: Events) -> Result<Option<Box<dyn Send>>, BatteryError> {
        let subscriber = Arc::new(Subscriber {
            admitted: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
        });
        let id = {
            let mut state = self.shared.state();
            if state.stopped {
                return Err(BatteryError::Stopped);
            }
            if state.subscribers.len() >= 16 {
                return Err(BatteryError::Busy);
            }
            let id = state
                .next_id
                .checked_add(1)
                .ok_or(BatteryError::Exhausted)?;
            state.next_id = id;
            state.subscribers.insert(id, Arc::clone(&subscriber));
            state.reconcile = true;
            if let Err(error) = self.shared.wake() {
                state.subscribers.remove(&id);
                state.stopped = true;
                return Err(error);
            }
            id
        };
        Ok(Some(Box::new(Guard {
            shared: Arc::clone(&self.shared),
            id,
            subscriber,
        })))
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state();
            state.stopped = true;
            for subscriber in state.subscribers.values() {
                subscriber.admitted.store(false, Ordering::Release);
            }
            state.subscribers.clear();
            state.reconcile = true;
        }
        let _ = self.shared.wake();
    }
}
struct Guard {
    shared: Arc<Shared>,
    id: u64,
    subscriber: Arc<Subscriber>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.subscriber.admitted.store(false, Ordering::Release);
        {
            let mut state = self.shared.state();
            state.subscribers.remove(&self.id);
            state.reconcile = true;
        }
        let _ = self.shared.wake();
    }
}
fn event(subscriber: &Subscriber, value: BatteryEvent) {
    if subscriber.admitted.load(Ordering::Acquire) {
        let _ = catch_unwind(AssertUnwindSafe(|| (subscriber.events)(value)));
    }
}
fn owner(slot: &mut Option<NativeOwner>) -> Result<&mut NativeOwner, BatteryError> {
    if slot.is_none() {
        *slot = Some(NativeOwner::new()?);
    }
    slot.as_mut().ok_or(BatteryError::Unavailable)
}
fn start() -> Result<Arc<Host>, BatteryError> {
    let (sender, receiver) = sync_channel(1);
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        sender,
        dirty: AtomicBool::new(false),
        reads: FlightGate::default(),
        settings: FlightGate::default(),
    });
    let worker = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("tessera-battery".into())
        .spawn(move || {
            let shared = worker;
            // WinRT handlers retain only a weak wakeup. Failed revocation cannot
            // retain a worker/apartment or invoke UI through a dangling pointer.
            let weak = Arc::downgrade(&shared);
            let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
                if let Some(shared) = weak.upgrade() {
                    shared.changed();
                }
            });
            let mut native = None;
            let mut watching: Option<Result<(), BatteryError>> = None;
            let mut retirement_error = None;
            while receiver.recv().is_ok() {
                loop {
                    let (stop, reconcile, subscribers, read, settings) = {
                        let mut state = shared.state();
                        (
                            state.stopped,
                            std::mem::take(&mut state.reconcile),
                            state.subscribers.values().cloned().collect::<Vec<_>>(),
                            state.read.take(),
                            state.settings.take(),
                        )
                    };
                    if reconcile || stop {
                        if subscribers.is_empty() || stop {
                            if watching.take().is_some() {
                                let result = catch_unwind(AssertUnwindSafe(|| {
                                    owner(&mut native)?.unregister()
                                }))
                                .unwrap_or(Err(BatteryError::Unavailable));
                                if let Err(error) = result {
                                    retirement_error = Some(error);
                                }
                            }
                        } else {
                            let result = *watching.get_or_insert_with(|| {
                                if let Some(error) = retirement_error {
                                    return Err(error);
                                }
                                catch_unwind(AssertUnwindSafe(|| {
                                    owner(&mut native)?.register(Arc::clone(&wake))
                                }))
                                .unwrap_or(Err(BatteryError::Unavailable))
                            });
                            for subscriber in &subscribers {
                                if !subscriber.ready.swap(true, Ordering::AcqRel) {
                                    event(
                                        subscriber,
                                        match result {
                                            Ok(()) => BatteryEvent::WatchReady,
                                            Err(error) => BatteryEvent::WatchUnavailable(error),
                                        },
                                    );
                                }
                            }
                        }
                    }
                    // Host retirement stops new admission, never cancels or replays
                    // an already accepted request. All callbacks run without locks.
                    if let Some(Read { completion, flight }) = read {
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            owner(&mut native).map(|owner| owner.read())
                        }))
                        .unwrap_or(Err(BatteryError::Unavailable));
                        drop(flight);
                        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                    }
                    if let Some(Settings { completion, flight }) = settings {
                        let result =
                            catch_unwind(AssertUnwindSafe(|| owner(&mut native)?.open_settings()))
                                .unwrap_or(Err(BatteryError::Unavailable));
                        drop(flight);
                        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                    }
                    if shared.dirty.swap(false, Ordering::AcqRel)
                        && watching == Some(Ok(()))
                        && !stop
                    {
                        for subscriber in &subscribers {
                            event(subscriber, BatteryEvent::Changed);
                        }
                    }
                    if stop {
                        return;
                    }
                    let state = shared.state();
                    if state.read.is_none()
                        && state.settings.is_none()
                        && !state.reconcile
                        && !shared.dirty.load(Ordering::Acquire)
                    {
                        break;
                    }
                }
            }
        })
        .map_err(|_| BatteryError::Unavailable)?;
    Ok(Arc::new(Host { shared }))
}
