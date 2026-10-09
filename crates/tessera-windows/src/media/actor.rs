// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Internal owner-thread seam, shared by WinRT and recording adapters.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

use tessera_system::media::{
    MediaCommandCompletion, MediaError, MediaErrorKind, MediaEvent, MediaReadCompletion,
    MediaRequest, MediaSnapshot,
};

use crate::single_flight::{Flight, FlightGate};

use super::MediaService;

const QUEUE_CAPACITY: usize = 16;
const MAX_SUBSCRIPTIONS: usize = 16;

type WatchCallback = Arc<dyn Fn(MediaEvent) + Send + Sync>;
type Completion<T> = Box<dyn FnOnce(Result<T, MediaError>) + Send + 'static>;

/// Native values never cross the worker seam and need not be `Send`.
/// `execute` must revalidate live identity and capability immediately before the
/// native effect. Every failed watch registration/rebind must be rollback-safe;
/// the actor additionally calls idempotent `stop_watch` on any failure.
pub(crate) trait Driver: 'static {
    fn read(&mut self) -> Result<MediaSnapshot, MediaError>;
    fn execute(&mut self, command: MediaRequest) -> Result<(), MediaError>;
    fn start_watch(&mut self, dirty: WatchCallback) -> Result<(), MediaError>;
    fn refresh_watch(&mut self) -> Result<(), MediaError>;
    /// Drains independent watch retirement discovered during a read, including
    /// when usable session facts were returned. Does not retry registration.
    fn take_watch_failure(&mut self) -> Option<MediaError> {
        None
    }
    fn stop_watch(&mut self);
}

struct QueueState {
    shutdown: AtomicBool,
    subscriptions: AtomicUsize,
}

pub(super) struct QueueLifetime {
    sender: SyncSender<Message>,
    state: Arc<QueueState>,
    flight: FlightGate,
}

impl QueueLifetime {
    pub(super) fn read(&self, completion: MediaReadCompletion) -> Result<(), MediaError> {
        let flight = self.flight.try_enter().ok_or_else(busy)?;
        self.send(Message::Read(Request::new(completion, flight)))
    }

    pub(super) fn execute(
        &self,
        command: MediaRequest,
        completion: MediaCommandCompletion,
    ) -> Result<(), MediaError> {
        let flight = self.flight.try_enter().ok_or_else(busy)?;
        self.send(Message::Execute(command, Request::new(completion, flight)))
    }

    fn send(&self, message: Message) -> Result<(), MediaError> {
        if self.state.shutdown.load(Ordering::Acquire) {
            message.reject();
            return Err(stopped());
        }
        match self.sender.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(message)) => {
                message.reject();
                Err(MediaError::new(
                    MediaErrorKind::Unavailable,
                    "Media worker queue is full",
                ))
            }
            Err(TrySendError::Disconnected(message)) => {
                message.reject();
                Err(stopped())
            }
        }
    }

    pub(super) fn subscribe(
        self: &Arc<Self>,
        callback: WatchCallback,
    ) -> Result<Option<Box<dyn Send>>, MediaError> {
        let mut count = self.state.subscriptions.load(Ordering::Acquire);
        loop {
            let next = count
                .checked_add(1)
                .filter(|_| count < MAX_SUBSCRIPTIONS)
                .ok_or_else(|| {
                    MediaError::new(
                        MediaErrorKind::WatchUnavailable,
                        "Too many media subscriptions",
                    )
                })?;
            match self.state.subscriptions.compare_exchange_weak(
                count,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => count = observed,
            }
        }
        let subscription = Arc::new(Subscription {
            active: AtomicBool::new(true),
            callback,
        });
        if let Err(error) = self.send(Message::Subscribe(Arc::clone(&subscription))) {
            subscription.active.store(false, Ordering::Release);
            self.state.subscriptions.fetch_sub(1, Ordering::AcqRel);
            return Err(error);
        }
        Ok(Some(Box::new(Guard {
            subscription,
            queue: Arc::clone(self),
        })))
    }

    fn wake(&self) {
        // Full means an earlier message will wake the owner. Shutdown, inactive
        // guards and dirty state live outside the bounded queue, so no control
        // transition is lost when a native call is stalled and the queue fills.
        let _ = self.sender.try_send(Message::Wake);
    }
}

impl Drop for QueueLifetime {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Release);
        self.wake();
    }
}

struct Guard {
    subscription: Arc<Subscription>,
    queue: Arc<QueueLifetime>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.subscription.active.store(false, Ordering::Release);
        self.queue
            .state
            .subscriptions
            .fetch_sub(1, Ordering::AcqRel);
        self.queue.wake();
    }
}

struct Subscription {
    active: AtomicBool,
    callback: WatchCallback,
}

impl Subscription {
    fn deliver(&self, event: MediaEvent) {
        if self.active.load(Ordering::Acquire) {
            let _ = catch_unwind(AssertUnwindSafe(|| (self.callback)(event)));
        }
    }
}

struct DirtyState {
    enabled: AtomicBool,
    pending: AtomicBool,
    seek_invalidated: AtomicBool,
}

impl DirtyState {
    fn callback(self: &Arc<Self>, sender: SyncSender<Message>) -> WatchCallback {
        let state = Arc::clone(self);
        Arc::new(move |event| {
            if state.enabled.load(Ordering::Acquire) {
                if event == MediaEvent::SeekInvalidated {
                    state.seek_invalidated.store(true, Ordering::Release);
                }
                if !state.pending.swap(true, Ordering::AcqRel) {
                    // The ABI only records invalidation and wakes the same owner.
                    let _ = sender.try_send(Message::Wake);
                }
            }
        })
    }

    fn disable(&self) {
        self.enabled.store(false, Ordering::Release);
        self.pending.store(false, Ordering::Release);
        self.seek_invalidated.store(false, Ordering::Release);
    }
}

/// Accepted requests own completion and admission until retirement. Unwinding
/// or receiver teardown still completes once. Immediate rejection disarms this
/// owner before dropping it, so rejected callbacks never run.
struct Request<T> {
    completion: Option<Completion<T>>,
    flight: Option<Flight>,
}

impl<T> Request<T> {
    fn new(completion: Completion<T>, flight: Flight) -> Self {
        Self {
            completion: Some(completion),
            flight: Some(flight),
        }
    }

    fn finish(&mut self, result: Result<T, MediaError>) {
        drop(self.flight.take());
        if let Some(completion) = self.completion.take() {
            // Release shared admission before consumer reentry or panic.
            let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
        }
    }

    fn reject(&mut self) {
        drop(self.completion.take());
        drop(self.flight.take());
    }
}

impl<T> Drop for Request<T> {
    fn drop(&mut self) {
        self.finish(Err(stopped()));
    }
}

enum Message {
    Read(Request<MediaSnapshot>),
    Execute(MediaRequest, Request<()>),
    Subscribe(Arc<Subscription>),
    Wake,
}

impl Message {
    fn reject(mut self) {
        match &mut self {
            Self::Read(request) => request.reject(),
            Self::Execute(_, request) => request.reject(),
            Self::Subscribe(_) | Self::Wake => {}
        }
    }
}

pub(super) fn spawn<D, F>(initialize: F) -> Result<MediaService, MediaError>
where
    D: Driver,
    F: FnMut() -> Result<D, MediaError> + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
    let state = Arc::new(QueueState {
        shutdown: AtomicBool::new(false),
        subscriptions: AtomicUsize::new(0),
    });
    let worker_sender = sender.clone();
    let worker_state = Arc::clone(&state);
    std::thread::Builder::new()
        .name("tessera-media".into())
        .spawn(move || Actor::new(initialize, worker_sender, worker_state).run(receiver))
        .map_err(|error| {
            MediaError::new(
                MediaErrorKind::Other,
                format!("Cannot start media worker: {}", error.kind()),
            )
        })?;
    Ok(MediaService {
        queue: Arc::new(QueueLifetime {
            sender,
            state,
            flight: FlightGate::default(),
        }),
    })
}

struct Actor<D: Driver, F: FnMut() -> Result<D, MediaError>> {
    driver: Option<D>,
    initialization_error: Option<MediaError>,
    initialize: F,
    sender: SyncSender<Message>,
    state: Arc<QueueState>,
    subscriptions: Vec<Arc<Subscription>>,
    dirty: Option<Arc<DirtyState>>,
    watch_status: Option<Result<(), MediaError>>,
}

impl<D: Driver, F: FnMut() -> Result<D, MediaError>> Actor<D, F> {
    fn new(mut initialize: F, sender: SyncSender<Message>, state: Arc<QueueState>) -> Self {
        let (driver, initialization_error) = match initialize_driver(&mut initialize) {
            Ok(driver) => (Some(driver), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            driver,
            initialization_error,
            initialize,
            sender,
            state,
            subscriptions: Vec::new(),
            dirty: None,
            watch_status: None,
        }
    }

    fn run(mut self, receiver: Receiver<Message>) {
        loop {
            self.prune();
            if self.state.shutdown.load(Ordering::Acquire) {
                break;
            }
            let Ok(message) = receiver.recv() else { break };
            if self.state.shutdown.load(Ordering::Acquire) {
                drop(message);
                break;
            }
            // Revoke UI intent before handling a previously queued request.
            self.publish_seek_invalidation();
            match message {
                Message::Read(mut request) => {
                    self.retry_initialization();
                    self.ensure_watch();
                    let result = self.call_driver("read", Driver::read).map(|mut snapshot| {
                        snapshot.current = snapshot.current.map(|session| session.bounded());
                        snapshot
                    });
                    self.reconcile_read_watch();
                    request.finish(result);
                }
                Message::Execute(command, mut request) => {
                    // Never initialize/retry/replay an accepted media intent.
                    let result =
                        self.call_driver("media request", |driver| driver.execute(command));
                    request.finish(result);
                }
                Message::Subscribe(subscription) => self.subscribe(subscription),
                Message::Wake => {}
            }
            self.prune();
            if self.state.shutdown.load(Ordering::Acquire) {
                break;
            }
            self.refresh_dirty();
        }
        self.state.shutdown.store(true, Ordering::Release);
        self.retire_watch();
        // Every accepted residual request has a panic-safe completion owner.
        for message in receiver.try_iter() {
            drop(message);
        }
    }

    fn retry_initialization(&mut self) {
        if self.driver.is_none() {
            match initialize_driver(&mut self.initialize) {
                Ok(driver) => {
                    self.driver = Some(driver);
                    self.initialization_error = None;
                }
                Err(error) => self.initialization_error = Some(error),
            }
        }
    }

    fn call_driver<T>(
        &mut self,
        operation: &str,
        call: impl FnOnce(&mut D) -> Result<T, MediaError>,
    ) -> Result<T, MediaError> {
        let Some(driver) = self.driver.as_mut() else {
            return Err(self.initialization_error.clone().unwrap_or_else(stopped));
        };
        match catch_unwind(AssertUnwindSafe(|| call(driver))) {
            Ok(result) => result,
            Err(_) => {
                let error = panicked(operation);
                self.retire_watch();
                self.release_driver();
                self.initialization_error = Some(error.clone());
                self.set_watch_status(Err(error.clone()));
                Err(error)
            }
        }
    }

    fn subscribe(&mut self, subscription: Arc<Subscription>) {
        if !subscription.active.load(Ordering::Acquire) {
            return;
        }
        let first = self.subscriptions.is_empty();
        self.subscriptions.push(Arc::clone(&subscription));
        if first {
            self.ensure_watch();
        } else if let Some(status) = &self.watch_status {
            subscription.deliver(watch_event(status));
        }
    }

    fn ensure_watch(&mut self) {
        if self.subscriptions.is_empty() || self.dirty.is_some() {
            return;
        }
        if self.driver.is_none() {
            let error = self.initialization_error.clone().unwrap_or_else(stopped);
            self.set_watch_status(Err(error));
            return;
        }
        let dirty = Arc::new(DirtyState {
            enabled: AtomicBool::new(true),
            pending: AtomicBool::new(false),
            seek_invalidated: AtomicBool::new(false),
        });
        let callback = dirty.callback(self.sender.clone());
        self.dirty = Some(dirty);
        let status = self.call_driver("watch registration", |driver| driver.start_watch(callback));
        if status.is_err() {
            // Also handles adapters that acquired their first token before
            // returning an error. Disable callback admission before retirement.
            self.retire_watch();
        }
        self.set_watch_status(status);
    }

    fn reconcile_read_watch(&mut self) {
        if self.dirty.is_none() {
            return;
        }
        let status = self.call_driver("read watch status", |driver| {
            driver.take_watch_failure().map_or(Ok(()), Err)
        });
        if status.is_err() {
            // Native read retirement must also release actor watch admission.
            // Only a subsequent explicit read attempts fresh registration.
            self.retire_watch();
            self.set_watch_status(status);
        }
    }

    fn publish_seek_invalidation(&self) {
        let seek_invalidated = self.dirty.as_ref().is_some_and(|dirty| {
            dirty.enabled.load(Ordering::Acquire)
                && dirty.seek_invalidated.swap(false, Ordering::AcqRel)
        });
        if seek_invalidated {
            self.deliver(MediaEvent::SeekInvalidated);
        }
    }

    fn refresh_dirty(&mut self) {
        let pending = self.dirty.as_ref().is_some_and(|dirty| {
            dirty.enabled.load(Ordering::Acquire) && dirty.pending.swap(false, Ordering::AcqRel)
        });
        // Take pending before publishing the reason: a coalesced callback
        // between these operations cannot strand its reason without a wake.
        self.publish_seek_invalidation();
        if !pending || self.subscriptions.is_empty() {
            return;
        }
        // Current-session rebinding precedes publication of refreshed facts.
        let status = self.call_driver("watch refresh", Driver::refresh_watch);
        if status.is_err() {
            self.retire_watch();
        }
        self.set_watch_status(status);
        self.deliver(MediaEvent::Changed);
    }

    fn prune(&mut self) {
        self.subscriptions
            .retain(|subscription| subscription.active.load(Ordering::Acquire));
        if self.subscriptions.is_empty() {
            self.retire_watch();
            self.watch_status = None;
        }
    }

    fn set_watch_status(&mut self, status: Result<(), MediaError>) {
        if self.watch_status.as_ref() != Some(&status) {
            self.watch_status = Some(status.clone());
            self.deliver(watch_event(&status));
        }
    }

    fn deliver(&self, event: MediaEvent) {
        for subscription in &self.subscriptions {
            subscription.deliver(event.clone());
        }
    }

    fn retire_watch(&mut self) {
        if let Some(dirty) = self.dirty.take() {
            dirty.disable();
            if let Some(driver) = self.driver.as_mut()
                && catch_unwind(AssertUnwindSafe(|| driver.stop_watch())).is_err()
            {
                self.release_driver();
                self.initialization_error = Some(panicked("watch retirement"));
            }
        }
    }

    fn release_driver(&mut self) {
        let driver = self.driver.take();
        // Native Drop and all resources it owns stay on this owner thread.
        let _ = catch_unwind(AssertUnwindSafe(|| drop(driver)));
    }
}

impl<D: Driver, F: FnMut() -> Result<D, MediaError>> Drop for Actor<D, F> {
    fn drop(&mut self) {
        self.state.shutdown.store(true, Ordering::Release);
        self.retire_watch();
        self.release_driver();
    }
}

fn initialize_driver<D: Driver>(
    initialize: &mut impl FnMut() -> Result<D, MediaError>,
) -> Result<D, MediaError> {
    catch_unwind(AssertUnwindSafe(initialize)).unwrap_or_else(|_| Err(panicked("initialization")))
}

fn watch_event(status: &Result<(), MediaError>) -> MediaEvent {
    match status {
        Ok(()) => MediaEvent::WatchReady,
        Err(error) => MediaEvent::WatchUnavailable(error.clone()),
    }
}

fn busy() -> MediaError {
    MediaError::new(
        MediaErrorKind::Unavailable,
        "Another media read or command is in flight",
    )
}

fn stopped() -> MediaError {
    MediaError::new(
        MediaErrorKind::Unavailable,
        "Media worker stopped before completing the request",
    )
}

fn panicked(operation: &str) -> MediaError {
    MediaError::new(
        MediaErrorKind::Other,
        format!("Media driver panicked during {operation}"),
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
