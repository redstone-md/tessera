// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Default-multimedia endpoint audio. The facade only owns a command queue;
//! native interfaces are created, used, unregistered and released by its actor.

use std::sync::Arc;

#[cfg(not(windows))]
use tessera_system::audio::AudioErrorKind;
use tessera_system::audio::{
    AudioCommand, AudioCompletion, AudioDeviceCommand, AudioDevicesCompletion, AudioError,
    AudioEvent, AudioHost,
};

#[cfg(any(windows, test))]
use actor::QueueLifetime;

/// Asynchronous Windows endpoint-volume host, independent of desktop observation.
///
/// Construction starts a worker without waiting for COM or a driver. Initialization
/// failures are delivered to accepted requests and subscriptions. The last facade
/// or subscription queues shutdown; dropping either never joins a native thread.
/// On other platforms construction returns an honest `Unsupported` error.
pub struct AudioService {
    #[cfg(any(windows, test))]
    queue: Arc<QueueLifetime>,
}

impl AudioService {
    pub fn new() -> Result<Self, AudioError> {
        #[cfg(windows)]
        {
            actor::spawn(crate::native_audio::NativeAudio::new)
        }
        #[cfg(not(windows))]
        {
            Err(unsupported())
        }
    }
}

#[cfg(not(windows))]
fn unsupported() -> AudioError {
    AudioError::new(AudioErrorKind::Unsupported, "Native audio requires Windows")
}

impl AudioHost for AudioService {
    fn read(&self, completion: AudioCompletion) -> Result<(), AudioError> {
        #[cfg(any(windows, test))]
        {
            self.queue.send(actor::Message::Read(completion))
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = completion;
            Err(unsupported())
        }
    }

    fn execute(
        &self,
        command: AudioCommand,
        completion: AudioCompletion,
    ) -> Result<(), AudioError> {
        #[cfg(any(windows, test))]
        {
            self.queue
                .send(actor::Message::Execute(command, completion))
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = (command, completion);
            Err(unsupported())
        }
    }

    fn supports_devices(&self) -> bool {
        cfg!(windows)
    }

    fn read_devices(&self, completion: AudioDevicesCompletion) -> Result<(), AudioError> {
        #[cfg(any(windows, test))]
        {
            self.queue.send(actor::Message::ReadDevices(completion))
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = completion;
            Err(unsupported())
        }
    }

    fn execute_device(
        &self,
        command: AudioDeviceCommand,
        completion: AudioDevicesCompletion,
    ) -> Result<(), AudioError> {
        #[cfg(any(windows, test))]
        {
            self.queue
                .send(actor::Message::ExecuteDevice(command, completion))
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = (command, completion);
            Err(unsupported())
        }
    }

    fn subscribe(
        &self,
        changed: Arc<dyn Fn(AudioEvent) + Send + Sync>,
    ) -> Result<Option<Box<dyn Send>>, AudioError> {
        #[cfg(any(windows, test))]
        {
            self.queue.subscribe(changed)
        }
        #[cfg(not(any(windows, test)))]
        {
            let _ = changed;
            Ok(None)
        }
    }
}

#[cfg(any(windows, test))]
pub(crate) mod actor {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{self, Receiver, Sender};

    use tessera_system::audio::{
        AudioCommand, AudioCompletion, AudioDeviceCommand, AudioDevicesCompletion,
        AudioDevicesResult, AudioEndpoint, AudioError, AudioErrorKind, AudioEvent, AudioFlow,
        AudioSnapshot, EndpointId, EndpointState, Volume,
    };

    use super::AudioService;

    /// A small recording/native seam; no native endpoint has to be `Send`.
    pub(crate) trait Driver: 'static {
        type Endpoint;

        fn default_endpoint(
            &mut self,
            flow: AudioFlow,
        ) -> Result<Option<Self::Endpoint>, AudioError>;
        fn endpoint_id(endpoint: &Self::Endpoint) -> &EndpointId;
        fn read_endpoint(&mut self, endpoint: &Self::Endpoint)
        -> Result<AudioEndpoint, AudioError>;
        /// Reacquires just the default identity, after any expensive activation.
        fn current_id(&mut self, flow: AudioFlow) -> Result<Option<EndpointId>, AudioError>;
        fn set_volume(
            &mut self,
            endpoint: &Self::Endpoint,
            volume: Volume,
        ) -> Result<(), AudioError>;
        fn set_muted(&mut self, endpoint: &Self::Endpoint, muted: bool) -> Result<(), AudioError>;
        fn read_devices(&mut self) -> Result<AudioDevicesResult, AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "Audio devices unavailable",
            ))
        }
        fn execute_device(
            &mut self,
            _command: AudioDeviceCommand,
        ) -> Result<AudioDevicesResult, AudioError> {
            Err(AudioError::new(
                AudioErrorKind::Unsupported,
                "Audio device controls unavailable",
            ))
        }
        fn start_watch(&mut self, signals: SignalSender) -> Result<(), AudioError>;
        fn rebind_watch(&mut self) -> Result<(), AudioError>;
        fn stop_watch(&mut self);
    }

    pub(super) struct QueueLifetime {
        sender: Sender<Message>,
    }

    impl QueueLifetime {
        pub(super) fn send(&self, message: Message) -> Result<(), AudioError> {
            // A failed send drops its completion without invoking it: immediate
            // rejection is distinct from an accepted request's completion.
            self.sender.send(message).map_err(|_| stopped())
        }

        pub(super) fn subscribe(
            self: &Arc<Self>,
            callback: Arc<dyn Fn(AudioEvent) + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, AudioError> {
            let subscription = Arc::new(Subscription {
                active: AtomicBool::new(true),
                callback,
            });
            self.send(Message::Subscribe(Arc::clone(&subscription)))?;
            Ok(Some(Box::new(Guard {
                subscription,
                queue: Arc::clone(self),
            })))
        }
    }

    impl Drop for QueueLifetime {
        fn drop(&mut self) {
            // Callback senders keep the channel connected, not the facade alive.
            // Explicit shutdown breaks that cycle without waiting for a driver.
            let _ = self.sender.send(Message::Shutdown);
        }
    }

    struct Guard {
        subscription: Arc<Subscription>,
        queue: Arc<QueueLifetime>,
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            self.subscription.active.store(false, Ordering::Release);
            let _ = self
                .queue
                .send(Message::Unsubscribe(Arc::clone(&self.subscription)));
        }
    }

    pub(super) struct Subscription {
        active: AtomicBool,
        callback: Arc<dyn Fn(AudioEvent) + Send + Sync>,
    }

    impl Subscription {
        fn deliver(&self, event: AudioEvent) {
            if self.active.load(Ordering::Acquire) {
                // A consumer panic must not abandon other accepted requests.
                let _ = catch_unwind(AssertUnwindSafe(|| (self.callback)(event)));
            }
        }
    }

    #[derive(Clone, Copy)]
    pub(crate) enum Signal {
        Devices,
        Volume,
    }

    struct SignalState {
        enabled: AtomicBool,
        devices_pending: AtomicBool,
        volume_pending: AtomicBool,
    }

    impl SignalState {
        fn pending(&self, signal: Signal) -> &AtomicBool {
            match signal {
                Signal::Devices => &self.devices_pending,
                Signal::Volume => &self.volume_pending,
            }
        }
    }

    /// Callback ABI only coalesces and queues an invalidation. It owns no COM,
    /// caller closures or facade lifetime and never waits for actor processing.
    #[derive(Clone)]
    pub(crate) struct SignalSender {
        sender: Sender<Message>,
        state: Arc<SignalState>,
    }

    impl SignalSender {
        fn new(sender: Sender<Message>) -> Self {
            Self {
                sender,
                state: Arc::new(SignalState {
                    enabled: AtomicBool::new(true),
                    devices_pending: AtomicBool::new(false),
                    volume_pending: AtomicBool::new(false),
                }),
            }
        }

        pub(crate) fn notify(&self, signal: Signal) {
            if self.state.enabled.load(Ordering::Acquire)
                && !self.state.pending(signal).swap(true, Ordering::AcqRel)
                && self
                    .sender
                    .send(Message::Signal(signal, self.clone()))
                    .is_err()
            {
                self.state.pending(signal).store(false, Ordering::Release);
            }
        }

        fn disable(&self) {
            self.state.enabled.store(false, Ordering::Release);
        }
    }

    pub(super) enum Message {
        Read(AudioCompletion),
        Execute(AudioCommand, AudioCompletion),
        ReadDevices(AudioDevicesCompletion),
        ExecuteDevice(AudioDeviceCommand, AudioDevicesCompletion),
        Subscribe(Arc<Subscription>),
        Unsubscribe(Arc<Subscription>),
        Signal(Signal, SignalSender),
        Shutdown,
    }

    pub(super) fn spawn<D, F>(initialize: F) -> Result<AudioService, AudioError>
    where
        D: Driver,
        F: FnOnce() -> Result<D, AudioError> + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let worker_sender = sender.clone();
        std::thread::Builder::new()
            .name("tessera-audio".into())
            .spawn(move || Actor::new(initialize(), worker_sender).run(receiver))
            .map_err(|error| {
                AudioError::new(
                    AudioErrorKind::Other,
                    format!("Cannot start audio worker: {}", error.kind()),
                )
            })?;
        Ok(AudioService {
            queue: Arc::new(QueueLifetime { sender }),
        })
    }

    struct Actor<D> {
        driver: Result<D, AudioError>,
        sender: Sender<Message>,
        subscriptions: Vec<Arc<Subscription>>,
        signals: Option<SignalSender>,
        watch_status: Option<Result<(), AudioError>>,
        last_snapshot: Option<AudioSnapshot>,
        inventory_active: bool,
    }

    impl<D: Driver> Actor<D> {
        fn new(driver: Result<D, AudioError>, sender: Sender<Message>) -> Self {
            Self {
                driver,
                sender,
                subscriptions: Vec::new(),
                signals: None,
                watch_status: None,
                last_snapshot: None,
                inventory_active: false,
            }
        }

        fn run(mut self, receiver: Receiver<Message>) {
            while let Ok(message) = receiver.recv() {
                match message {
                    Message::Read(completion) => {
                        let result = self
                            .driver
                            .as_mut()
                            .map(snapshot)
                            .map_err(|error| error.clone());
                        complete(completion, result);
                    }
                    Message::Execute(command, completion) => {
                        let result = self
                            .driver
                            .as_mut()
                            .map_err(|error| error.clone())
                            .and_then(|driver| execute(driver, command));
                        if let Ok(snapshot) = &result {
                            self.changed(snapshot.clone());
                        }
                        complete(completion, result);
                    }
                    Message::ReadDevices(completion) => {
                        self.inventory_active = true;
                        let result = self
                            .driver
                            .as_mut()
                            .map_err(|error| error.clone())
                            .and_then(Driver::read_devices);
                        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                    }
                    Message::ExecuteDevice(command, completion) => {
                        let result = self
                            .driver
                            .as_mut()
                            .map_err(|error| error.clone())
                            .and_then(|driver| driver.execute_device(command));
                        // The same watch invalidates both default routes and the
                        // inventory, including commands with partial role results.
                        self.deliver(AudioEvent::Changed);
                        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
                    }
                    Message::Subscribe(subscription) => self.subscribe(subscription),
                    Message::Unsubscribe(subscription) => {
                        self.subscriptions
                            .retain(|item| !Arc::ptr_eq(item, &subscription));
                        if self.subscriptions.is_empty() {
                            self.stop_watch();
                        }
                    }
                    Message::Signal(signal, source) => self.signal(signal, source),
                    Message::Shutdown => break,
                }
            }
            self.stop_watch();
            // Orderly shutdown processes all earlier requests. Also explicitly
            // fail any residual request instead of silently dropping callbacks.
            for message in receiver.try_iter() {
                match message {
                    Message::Read(completion) | Message::Execute(_, completion) => {
                        complete(completion, Err(stopped()));
                    }
                    Message::ReadDevices(completion) | Message::ExecuteDevice(_, completion) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| completion(Err(stopped()))));
                    }
                    _ => {}
                }
            }
        }

        fn subscribe(&mut self, subscription: Arc<Subscription>) {
            if !subscription.active.load(Ordering::Acquire) {
                return;
            }
            if self.subscriptions.is_empty() {
                let signals = SignalSender::new(self.sender.clone());
                let status = self
                    .driver
                    .as_mut()
                    .map_err(|error| error.clone())
                    .and_then(|driver| driver.start_watch(signals.clone()));
                self.signals = Some(signals);
                self.watch_status = Some(status);
                self.last_snapshot = self.driver.as_mut().ok().map(snapshot);
            }
            let event = watch_event(self.watch_status.as_ref().expect("watch setup recorded"));
            self.subscriptions.push(Arc::clone(&subscription));
            subscription.deliver(event);
        }

        fn signal(&mut self, signal: Signal, source: SignalSender) {
            source.state.pending(signal).store(false, Ordering::Release);
            if self.subscriptions.is_empty()
                || !self
                    .signals
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(&current.state, &source.state))
            {
                return;
            }
            if matches!(signal, Signal::Devices) {
                let status = self
                    .driver
                    .as_mut()
                    .map_err(|error| error.clone())
                    .and_then(Driver::rebind_watch);
                if self.watch_status.as_ref() != Some(&status) {
                    self.deliver(watch_event(&status));
                    self.watch_status = Some(status);
                }
            }
            if let Ok(driver) = &mut self.driver {
                let snapshot = snapshot(driver);
                let inventory_changed =
                    self.inventory_active && self.last_snapshot.as_ref() == Some(&snapshot);
                self.changed(snapshot);
                // Session/selected-device notifications may leave the legacy
                // default-route snapshot unchanged but still invalidate inventory.
                if inventory_changed {
                    self.deliver(AudioEvent::Changed);
                }
            }
        }

        fn changed(&mut self, snapshot: AudioSnapshot) {
            if !self.subscriptions.is_empty() && self.last_snapshot.as_ref() != Some(&snapshot) {
                self.last_snapshot = Some(snapshot);
                self.deliver(AudioEvent::Changed);
            }
        }

        fn deliver(&self, event: AudioEvent) {
            for subscription in &self.subscriptions {
                subscription.deliver(event.clone());
            }
        }

        fn stop_watch(&mut self) {
            if let Some(signals) = self.signals.take() {
                signals.disable();
                if let Ok(driver) = &mut self.driver {
                    driver.stop_watch();
                }
            }
            self.watch_status = None;
            self.last_snapshot = None;
        }
    }

    fn watch_event(status: &Result<(), AudioError>) -> AudioEvent {
        match status {
            Ok(()) => AudioEvent::WatchReady,
            Err(error) => AudioEvent::WatchUnavailable(error.clone()),
        }
    }

    fn complete(completion: AudioCompletion, result: Result<AudioSnapshot, AudioError>) {
        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
    }

    fn stopped() -> AudioError {
        AudioError::new(AudioErrorKind::Stopped, "Audio worker has stopped")
    }

    fn device_changed() -> AudioError {
        AudioError::new(
            AudioErrorKind::DeviceChanged,
            "The default multimedia audio endpoint changed",
        )
    }

    fn snapshot<D: Driver>(driver: &mut D) -> AudioSnapshot {
        AudioSnapshot {
            output: endpoint_state(driver, AudioFlow::Output),
            input: endpoint_state(driver, AudioFlow::Input),
        }
    }

    fn endpoint_state<D: Driver>(driver: &mut D, flow: AudioFlow) -> EndpointState {
        match driver.default_endpoint(flow) {
            Ok(Some(endpoint)) => match driver.read_endpoint(&endpoint) {
                Ok(endpoint) => EndpointState::Ready(endpoint),
                Err(error) => EndpointState::Unavailable(error),
            },
            Ok(None) => EndpointState::Absent,
            Err(error) => EndpointState::Unavailable(error),
        }
    }

    fn execute<D: Driver>(
        driver: &mut D,
        command: AudioCommand,
    ) -> Result<AudioSnapshot, AudioError> {
        let (flow, expected_id) = match &command {
            AudioCommand::SetVolume {
                flow, expected_id, ..
            }
            | AudioCommand::SetMuted {
                flow, expected_id, ..
            } => (*flow, expected_id),
        };
        let endpoint = driver.default_endpoint(flow)?.ok_or_else(device_changed)?;
        if D::endpoint_id(&endpoint) != expected_id {
            return Err(device_changed());
        }
        // Activation may perform RPC. Check the live default again immediately
        // before mutation; never open a caller-provided ID via GetDevice.
        if driver.current_id(flow)?.as_ref() != Some(expected_id) {
            return Err(device_changed());
        }
        match command {
            AudioCommand::SetVolume { volume, .. } => driver.set_volume(&endpoint, volume)?,
            AudioCommand::SetMuted { muted, .. } => driver.set_muted(&endpoint, muted)?,
        }
        let snapshot = snapshot(driver);
        let confirmed = match flow {
            AudioFlow::Output => &snapshot.output,
            AudioFlow::Input => &snapshot.input,
        };
        // Native scalar quantization is valid: return actual readback, not the
        // requested value. Failure to confirm the target is an explicit error.
        match confirmed {
            EndpointState::Ready(actual) if &actual.id == D::endpoint_id(&endpoint) => Ok(snapshot),
            EndpointState::Unavailable(error) => Err(error.clone()),
            EndpointState::Ready(_) | EndpointState::Absent => Err(device_changed()),
        }
    }

    #[cfg(test)]
    mod rejection_tests {
        use super::*;

        #[test]
        fn disconnected_queue_rejects_without_callback_or_watch_delivery() {
            let (sender, receiver) = mpsc::channel();
            drop(receiver);
            let queue = Arc::new(QueueLifetime { sender });
            let called = Arc::new(AtomicBool::new(false));
            let completion_called = Arc::clone(&called);
            let result = queue.send(Message::Read(Box::new(move |_| {
                completion_called.store(true, Ordering::Release);
            })));
            assert_eq!(result.unwrap_err().kind, AudioErrorKind::Stopped);
            assert!(!called.load(Ordering::Acquire));
            let event_called = Arc::clone(&called);
            let result = queue.subscribe(Arc::new(move |_| {
                event_called.store(true, Ordering::Release);
            }));
            assert!(matches!(
                result,
                Err(AudioError {
                    kind: AudioErrorKind::Stopped,
                    ..
                })
            ));
            assert!(!called.load(Ordering::Acquire));
        }
    }
}

#[cfg(test)]
#[path = "audio_tests.rs"]
mod tests;
