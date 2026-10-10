// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One lazy WLAN owner with bounded reads, scoped commands and native notification wakeups.

use std::sync::Arc;
use tessera_system::network::{NetworkError, NetworkHost};

/// Creates a facade without reading discovery, opening WLAN, or changing OS state.
/// Native initialization occurs only after an explicit read or subscription.
pub fn native_network_host() -> Result<Arc<dyn NetworkHost>, NetworkError> {
    #[cfg(windows)]
    {
        worker::start(crate::native_network::NativeOwner::new())
    }
    #[cfg(not(windows))]
    {
        Err(NetworkError::new(
            tessera_system::network::NetworkErrorKind::Unsupported,
            "Native network observations are available only on Windows.",
        ))
    }
}

#[cfg(any(windows, test))]
pub(crate) mod worker {
    use crate::single_flight::{Flight, FlightGate};
    use std::collections::BTreeMap;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};
    use tessera_system::network::{
        NetworkActionCompletion, NetworkCommand, NetworkCommandAccepted, NetworkCommandCompletion,
        NetworkCommandOutcome, NetworkCompletion, NetworkError, NetworkErrorKind, NetworkEvent,
        NetworkHost, NetworkSnapshot, NetworkView, NetworkViewCompletion,
    };

    pub(crate) const MAX_SUBSCRIBERS: usize = 16;

    /// Private real-native/recorded-owner seam. All methods and Drop run on worker.
    pub(crate) trait NetworkOwner: Send + 'static {
        fn read(&mut self) -> Result<NetworkSnapshot, NetworkError>;
        fn read_view(&mut self) -> Result<NetworkView, NetworkError> {
            self.read().map(|snapshot| NetworkView {
                snapshot,
                controls: None,
            })
        }
        fn command(&mut self, _command: NetworkCommand) -> Result<(), NetworkError> {
            Err(NetworkError::new(
                NetworkErrorKind::Unsupported,
                "Wi-Fi controls are unavailable.",
            ))
        }
        fn command_readback(&mut self) -> Result<Option<NetworkCommandOutcome>, NetworkError> {
            Ok(Some(NetworkCommandOutcome::AcceptedUnconfirmed))
        }
        fn command_retired(&mut self) {}
        fn open_settings(&mut self) -> Result<(), NetworkError>;
        fn register(&mut self, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), NetworkError>;
        fn unregister(&mut self) -> Result<(), NetworkError>;
    }

    struct Read {
        completion: NetworkViewCompletion,
        flight: Flight,
    }
    struct Action {
        completion: NetworkActionCompletion,
        flight: Flight,
    }
    struct ControlRequest {
        command: Option<NetworkCommand>,
        completion: NetworkCommandCompletion,
        accepted: Option<NetworkCommandAccepted>,
        flight: Flight,
    }
    struct Subscriber {
        admitted: AtomicBool,
        ready_sent: AtomicBool,
        events: Arc<dyn Fn(NetworkEvent) + Send + Sync>,
    }

    #[derive(Default)]
    struct State {
        stopped: bool,
        read: Option<Read>,
        action: Option<Action>,
        command: Option<ControlRequest>,
        subscribers: BTreeMap<u64, Arc<Subscriber>>,
        next_id: u64,
        watch_changed: bool,
        retirement_error: Option<NetworkError>,
    }

    struct Shared {
        state: Mutex<State>,
        sender: SyncSender<()>,
        dirty: AtomicBool,
        read_gate: FlightGate,
        action_gate: FlightGate,
    }

    impl Shared {
        fn state(&self) -> MutexGuard<'_, State> {
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
        fn wake(&self) -> Result<(), NetworkError> {
            match self.sender.try_send(()) {
                Ok(()) | Err(TrySendError::Full(())) => Ok(()),
                Err(TrySendError::Disconnected(())) => Err(stopped()),
            }
        }
        fn dirty(&self) {
            if !self.dirty.swap(true, Ordering::AcqRel) {
                let _ = self.wake();
            }
        }
    }

    struct Host {
        shared: Arc<Shared>,
    }

    impl NetworkHost for Host {
        fn read(&self, completion: NetworkCompletion) -> Result<(), NetworkError> {
            self.read_view(Box::new(move |result| {
                completion(result.map(|view| view.snapshot))
            }))
        }
        fn read_view(&self, completion: NetworkViewCompletion) -> Result<(), NetworkError> {
            let flight = self.shared.read_gate.try_enter().ok_or_else(busy)?;
            let mut state = self.shared.state();
            if state.stopped {
                return Err(stopped());
            }
            state.read = Some(Read { completion, flight });
            if let Err(error) = self.shared.wake() {
                state.read.take();
                state.stopped = true;
                return Err(error);
            }
            Ok(())
        }
        fn open_settings(&self, completion: NetworkActionCompletion) -> Result<(), NetworkError> {
            let flight = self.shared.action_gate.try_enter().ok_or_else(busy)?;
            let mut state = self.shared.state();
            if state.stopped {
                return Err(stopped());
            }
            state.action = Some(Action { completion, flight });
            if let Err(error) = self.shared.wake() {
                state.action.take();
                state.stopped = true;
                return Err(error);
            }
            Ok(())
        }
        fn command(
            &self,
            command: NetworkCommand,
            completion: NetworkCommandCompletion,
        ) -> Result<(), NetworkError> {
            self.command_with_acceptance(command, Box::new(|| {}), completion)
        }
        fn command_with_acceptance(
            &self,
            command: NetworkCommand,
            accepted: NetworkCommandAccepted,
            completion: NetworkCommandCompletion,
        ) -> Result<(), NetworkError> {
            let flight = self.shared.action_gate.try_enter().ok_or_else(busy)?;
            let mut state = self.shared.state();
            if state.stopped {
                return Err(stopped());
            }
            state.command = Some(ControlRequest {
                command: Some(command),
                accepted: Some(accepted),
                completion,
                flight,
            });
            if let Err(error) = self.shared.wake() {
                state.command.take();
                state.stopped = true;
                return Err(error);
            }
            Ok(())
        }
        fn subscribe(
            &self,
            events: Arc<dyn Fn(NetworkEvent) + Send + Sync>,
        ) -> Result<Option<Box<dyn Send>>, NetworkError> {
            let mut state = self.shared.state();
            if state.stopped {
                return Err(stopped());
            }
            if state.subscribers.len() >= MAX_SUBSCRIBERS {
                return Err(busy());
            }
            let id = loop {
                state.next_id = state.next_id.wrapping_add(1);
                if !state.subscribers.contains_key(&state.next_id) {
                    break state.next_id;
                }
            };
            let subscriber = Arc::new(Subscriber {
                admitted: AtomicBool::new(true),
                ready_sent: AtomicBool::new(false),
                events,
            });
            state.subscribers.insert(id, Arc::clone(&subscriber));
            state.watch_changed = true;
            if let Err(error) = self.shared.wake() {
                state.subscribers.remove(&id);
                subscriber.admitted.store(false, Ordering::Release);
                state.stopped = true;
                return Err(error);
            }
            Ok(Some(Box::new(Guard {
                shared: Arc::clone(&self.shared),
                id,
                subscriber,
            })))
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let mut state = self.shared.state();
            state.stopped = true;
            for subscriber in state.subscribers.values() {
                subscriber.admitted.store(false, Ordering::Release);
            }
            state.subscribers.clear();
            state.watch_changed = true;
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
            // Already admitted callbacks may complete; new admission closes now.
            self.subscriber.admitted.store(false, Ordering::Release);
            let mut state = self.shared.state();
            state.subscribers.remove(&self.id);
            state.watch_changed = true;
            let _ = self.shared.wake();
        }
    }

    enum Watch {
        Dormant,
        Installed,
        Failed(NetworkError),
    }

    fn busy() -> NetworkError {
        NetworkError::new(
            NetworkErrorKind::Busy,
            "The network operation is already in progress.",
        )
    }
    fn stopped() -> NetworkError {
        NetworkError::new(NetworkErrorKind::Stopped, "The network worker has stopped.")
    }
    fn panicked() -> NetworkError {
        NetworkError::new(
            NetworkErrorKind::Other,
            "The network worker could not complete the operation.",
        )
    }
    fn event(subscriber: &Subscriber, notification: NetworkEvent) {
        if subscriber.admitted.load(Ordering::Acquire) {
            let _ = catch_unwind(AssertUnwindSafe(|| (subscriber.events)(notification)));
        }
    }
    fn read_complete(request: Read, result: Result<NetworkView, NetworkError>) {
        let Read { completion, flight } = request;
        drop(flight);
        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
    }
    fn action_complete(request: Action, result: Result<(), NetworkError>) {
        let Action { completion, flight } = request;
        drop(flight);
        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
    }
    fn control_complete(
        owner: &mut impl NetworkOwner,
        request: ControlRequest,
        result: Result<NetworkCommandOutcome, NetworkError>,
    ) {
        let _ = catch_unwind(AssertUnwindSafe(|| owner.command_retired()));
        let ControlRequest {
            completion, flight, ..
        } = request;
        drop(flight);
        let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
    }

    pub(crate) fn start<O: NetworkOwner>(
        mut owner: O,
    ) -> Result<Arc<dyn NetworkHost>, NetworkError> {
        let (sender, receiver) = sync_channel(1);
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            sender,
            dirty: AtomicBool::new(false),
            read_gate: FlightGate::default(),
            action_gate: FlightGate::default(),
        });
        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("tessera-network".into())
            .spawn(move || {
                let shared = worker_shared;
                let callback_shared = Arc::clone(&shared);
                let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || callback_shared.dirty());
                let mut pending: Option<(ControlRequest, Instant)> = None;
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    let mut watch = Watch::Dormant;
                    loop {
                        let received = match pending.as_ref() {
                            Some((_, deadline)) => receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                            None => receiver.recv().map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
                        };
                        match received {
                            Ok(()) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                                if let Some((request, _)) = pending.take() {
                                    let result = catch_unwind(AssertUnwindSafe(|| owner.command_readback()))
                                        .unwrap_or_else(|_| Err(panicked()))
                                        .map(|outcome| outcome.unwrap_or(NetworkCommandOutcome::AcceptedUnconfirmed));
                                    control_complete(&mut owner, request, result);
                                    shared.state().watch_changed = true;
                                }
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                        loop {
                            let (stop, reconcile, subscribers, read, action, command) = {
                                let mut state = shared.state();
                                let reconcile = std::mem::take(&mut state.watch_changed);
                                (
                                    state.stopped,
                                    reconcile,
                                    state.subscribers.values().cloned().collect::<Vec<_>>(),
                                    state.read.take(),
                                    state.action.take(),
                                    state.command.take(),
                                )
                            };
                            if reconcile || stop {
                                if (subscribers.is_empty() && pending.is_none() && command.is_none()) || stop {
                                    if !matches!(watch, Watch::Dormant) {
                                        let result =
                                            catch_unwind(AssertUnwindSafe(|| owner.unregister()))
                                                .unwrap_or_else(|_| Err(panicked()));
                                        if let Err(error) = result {
                                            shared.state().retirement_error = Some(error);
                                        }
                                        watch = Watch::Dormant;
                                    }
                                } else {
                                    if matches!(watch, Watch::Dormant) {
                                        watch = match catch_unwind(AssertUnwindSafe(|| {
                                            owner.register(Arc::clone(&wake))
                                        }))
                                        .unwrap_or_else(|_| Err(panicked()))
                                        {
                                            Ok(()) => Watch::Installed,
                                            Err(error) => Watch::Failed(error),
                                        };
                                    }
                                    for subscriber in &subscribers {
                                        if !subscriber.ready_sent.swap(true, Ordering::AcqRel) {
                                            match &watch {
                                                Watch::Installed => {
                                                    event(subscriber, NetworkEvent::WatchReady)
                                                }
                                                Watch::Failed(error) => event(
                                                    subscriber,
                                                    NetworkEvent::WatchUnavailable(error.clone()),
                                                ),
                                                Watch::Dormant => {}
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some(request) = read {
                                let result = if stop {
                                    Err(stopped())
                                } else {
                                    catch_unwind(AssertUnwindSafe(|| owner.read_view()))
                                        .unwrap_or_else(|_| Err(panicked()))
                                };
                                read_complete(request, result);
                            }
                            if let Some(request) = action {
                                let result = if stop {
                                    Err(stopped())
                                } else {
                                    catch_unwind(AssertUnwindSafe(|| owner.open_settings()))
                                        .unwrap_or_else(|_| Err(panicked()))
                                };
                                action_complete(request, result);
                            }
                            if let Some(mut request) = command {
                                let result = if stop {
                                    Err(stopped())
                                } else if !matches!(watch, Watch::Installed) {
                                    Err(NetworkError::new(NetworkErrorKind::Unsupported, "A working WLAN notification watch is required for connection controls."))
                                } else {
                                    catch_unwind(AssertUnwindSafe(|| owner.command(request.command.take().expect("admitted WLAN command"))))
                                        .unwrap_or_else(|_| Err(panicked()))
                                };
                                match result {
                                    Ok(()) => {
                                        let accepted = request.accepted.take();
                                        pending = Some((request, Instant::now() + Duration::from_secs(20)));
                                        if let Some(accepted) = accepted {
                                            let _ = catch_unwind(AssertUnwindSafe(accepted));
                                        }
                                    }
                                    Err(error) => control_complete(&mut owner, request, Err(error)),
                                }
                            }
                            if pending.is_some() {
                                let result = if stop {
                                    Ok(Some(NetworkCommandOutcome::AcceptedUnconfirmed))
                                } else {
                                    catch_unwind(AssertUnwindSafe(|| owner.command_readback()))
                                        .unwrap_or_else(|_| Err(panicked()))
                                };
                                if !matches!(result, Ok(None))
                                    && let Some((request, _)) = pending.take()
                                {
                                    control_complete(&mut owner, request, result.map(|outcome| outcome.unwrap_or(NetworkCommandOutcome::AcceptedUnconfirmed)));
                                    shared.state().watch_changed = true;
                                }
                            }
                            if shared.dirty.swap(false, Ordering::AcqRel)
                                && matches!(watch, Watch::Installed)
                                && !stop
                            {
                                for subscriber in &subscribers {
                                    event(subscriber, NetworkEvent::Changed);
                                }
                            }
                            if stop {
                                return;
                            }
                            let state = shared.state();
                            if state.read.is_none()
                                && state.action.is_none()
                                && state.command.is_none()
                                && !state.watch_changed
                                && !shared.dirty.load(Ordering::Acquire)
                            {
                                break;
                            }
                        }
                    }
                }));
                // Unexpected unwinding or receiver retirement cannot strand queued work.
                // NativeOwner Drop performs its safe native barrier on this worker only.
                let (read, action, command) = {
                    let mut state = shared.state();
                    state.stopped = true;
                    for subscriber in state.subscribers.values() {
                        subscriber.admitted.store(false, Ordering::Release);
                    }
                    state.subscribers.clear();
                    (state.read.take(), state.action.take(), state.command.take())
                };
                if let Some(request) = read {
                    read_complete(request, Err(stopped()));
                }
                if let Some(request) = action {
                    action_complete(request, Err(stopped()));
                }
                if let Some(request) = command {
                    control_complete(&mut owner, request, Err(stopped()));
                }
                if let Some((request, _)) = pending.take() {
                    control_complete(&mut owner, request, Ok(NetworkCommandOutcome::AcceptedUnconfirmed));
                }
            })
            .map_err(|_| {
                NetworkError::new(
                    NetworkErrorKind::Other,
                    "The network worker could not start.",
                )
            })?;
        Ok(Arc::new(Host { shared }))
    }

    #[cfg(test)]
    mod tests;
}
