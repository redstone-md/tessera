// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use parking_lot::Mutex;

use tessera_system::audio::{
    AudioCommand, AudioEndpoint, AudioError, AudioErrorKind, AudioEvent, AudioFlow, AudioHost,
    AudioSnapshot, EndpointId, EndpointState, Volume,
};

use super::AudioService;
use super::actor::{self, Driver, Signal, SignalSender};

const WAIT: Duration = Duration::from_secs(3);

#[derive(Debug, PartialEq)]
enum Write {
    Volume(EndpointId, Volume),
    Muted(EndpointId, bool),
}

struct Recording {
    endpoints: [Result<Option<AudioEndpoint>, AudioError>; 2],
    drift: Option<(AudioFlow, Option<AudioEndpoint>)>,
    writes: Vec<Write>,
    watch_error: Option<AudioError>,
    signals: Option<SignalSender>,
    starts: usize,
    rebinds: usize,
    stops: usize,
    fail_readback: bool,
}

impl Recording {
    fn new() -> Self {
        Self {
            endpoints: [
                Ok(Some(endpoint("output", 0.4, false))),
                Ok(Some(endpoint("input", 0.6, true))),
            ],
            drift: None,
            writes: Vec::new(),
            watch_error: None,
            signals: None,
            starts: 0,
            rebinds: 0,
            stops: 0,
            fail_readback: false,
        }
    }
}

/// Intentionally !Send like native COM. Only the factory crosses threads.
struct RecordingDriver {
    recording: Arc<Mutex<Recording>>,
    finished: Sender<()>,
    registration: Option<(Sender<()>, Receiver<()>)>,
    _thread_only: Rc<()>,
}

impl Driver for RecordingDriver {
    type Endpoint = AudioEndpoint;

    fn default_endpoint(&mut self, flow: AudioFlow) -> Result<Option<Self::Endpoint>, AudioError> {
        self.recording.lock().endpoints[index(flow)].clone()
    }

    fn endpoint_id(endpoint: &Self::Endpoint) -> &EndpointId {
        &endpoint.id
    }

    fn read_endpoint(&mut self, endpoint: &Self::Endpoint) -> Result<AudioEndpoint, AudioError> {
        self.recording
            .lock()
            .endpoints
            .iter()
            .filter_map(|current| current.as_ref().ok().and_then(Option::as_ref))
            .find(|current| current.id == endpoint.id)
            .cloned()
            .ok_or_else(|| error(AudioErrorKind::DeviceChanged))
    }

    fn current_id(&mut self, flow: AudioFlow) -> Result<Option<EndpointId>, AudioError> {
        let mut recording = self.recording.lock();
        if let Some((flow, endpoint)) = recording.drift.take() {
            recording.endpoints[index(flow)] = Ok(endpoint);
        }
        recording.endpoints[index(flow)]
            .clone()
            .map(|endpoint| endpoint.map(|value| value.id))
    }

    fn set_volume(&mut self, endpoint: &Self::Endpoint, volume: Volume) -> Result<(), AudioError> {
        let mut recording = self.recording.lock();
        recording
            .writes
            .push(Write::Volume(endpoint.id.clone(), volume));
        for current in recording
            .endpoints
            .iter_mut()
            .filter_map(|value| value.as_mut().ok())
            .flatten()
        {
            if current.id == endpoint.id {
                current.volume = volume;
            }
        }
        if recording.fail_readback {
            recording.endpoints[0] = Err(error(AudioErrorKind::AccessDenied));
        }
        Ok(())
    }

    fn set_muted(&mut self, endpoint: &Self::Endpoint, muted: bool) -> Result<(), AudioError> {
        let mut recording = self.recording.lock();
        recording
            .writes
            .push(Write::Muted(endpoint.id.clone(), muted));
        for current in recording
            .endpoints
            .iter_mut()
            .filter_map(|value| value.as_mut().ok())
            .flatten()
        {
            if current.id == endpoint.id {
                current.muted = muted;
            }
        }
        Ok(())
    }

    fn start_watch(&mut self, signals: SignalSender) -> Result<(), AudioError> {
        if let Some((entered, release)) = self.registration.take() {
            entered.send(()).unwrap();
            release.recv_timeout(WAIT).unwrap();
        }
        let mut recording = self.recording.lock();
        recording.starts += 1;
        recording.signals = Some(signals);
        recording.watch_error.clone().map_or(Ok(()), Err)
    }

    fn rebind_watch(&mut self) -> Result<(), AudioError> {
        let mut recording = self.recording.lock();
        recording.rebinds += 1;
        recording.watch_error.clone().map_or(Ok(()), Err)
    }

    fn stop_watch(&mut self) {
        let mut recording = self.recording.lock();
        recording.signals = None;
        recording.stops += 1;
    }
}

impl Drop for RecordingDriver {
    fn drop(&mut self) {
        let _ = self.finished.send(());
    }
}

fn index(flow: AudioFlow) -> usize {
    match flow {
        AudioFlow::Output => 0,
        AudioFlow::Input => 1,
    }
}

fn endpoint(id: &str, scalar: f32, muted: bool) -> AudioEndpoint {
    AudioEndpoint {
        id: EndpointId::new(id.into()).unwrap(),
        volume: Volume::from_scalar(scalar).unwrap(),
        muted,
    }
}

fn error(kind: AudioErrorKind) -> AudioError {
    AudioError::new(kind, "recorded audio failure")
}

fn start(recording: Arc<Mutex<Recording>>) -> (AudioService, Receiver<()>) {
    start_with_registration(recording, None)
}

fn start_with_registration(
    recording: Arc<Mutex<Recording>>,
    registration: Option<(Sender<()>, Receiver<()>)>,
) -> (AudioService, Receiver<()>) {
    let (finished, receiver) = mpsc::channel();
    let service = actor::spawn(move || {
        Ok(RecordingDriver {
            recording,
            finished,
            registration,
            _thread_only: Rc::new(()),
        })
    })
    .unwrap();
    (service, receiver)
}

fn read(service: &AudioService) -> AudioSnapshot {
    let (sender, receiver) = mpsc::channel();
    service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    receiver.recv_timeout(WAIT).unwrap().unwrap()
}

fn execute(service: &AudioService, command: AudioCommand) -> Result<AudioSnapshot, AudioError> {
    let (sender, receiver) = mpsc::channel();
    service
        .execute(
            command,
            Box::new(move |result| sender.send(result).unwrap()),
        )
        .unwrap();
    let result = receiver.recv_timeout(WAIT).unwrap();
    assert!(
        receiver.recv_timeout(WAIT).is_err(),
        "completion was delivered twice"
    );
    result
}

fn volume_command(expected_id: &str) -> AudioCommand {
    AudioCommand::SetVolume {
        flow: AudioFlow::Output,
        expected_id: EndpointId::new(expected_id.into()).unwrap(),
        volume: Volume::from_scalar(0.8).unwrap(),
    }
}

#[test]
fn same_id_absolute_commands_return_independent_confirmed_readback() {
    let recording = Arc::new(Mutex::new(Recording::new()));
    let (service, finished) = start(Arc::clone(&recording));
    let snapshot = execute(&service, volume_command("output")).unwrap();
    assert_eq!(
        snapshot.output,
        EndpointState::Ready(endpoint("output", 0.8, false))
    );
    assert_eq!(
        snapshot.input,
        EndpointState::Ready(endpoint("input", 0.6, true))
    );
    let snapshot = execute(
        &service,
        AudioCommand::SetMuted {
            flow: AudioFlow::Input,
            expected_id: EndpointId::new("input".into()).unwrap(),
            muted: false,
        },
    )
    .unwrap();
    assert_eq!(
        snapshot.input,
        EndpointState::Ready(endpoint("input", 0.6, false))
    );
    assert_eq!(
        recording.lock().writes,
        vec![
            Write::Volume(
                EndpointId::new("output".into()).unwrap(),
                Volume::from_scalar(0.8).unwrap()
            ),
            Write::Muted(EndpointId::new("input".into()).unwrap(), false),
        ]
    );
    drop(service);
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn changed_absent_cross_flow_and_fabricated_ids_never_mutate() {
    for (current, expected, drift) in [
        (Some(endpoint("replacement", 0.5, false)), "output", None),
        (None, "output", None),
        (Some(endpoint("output", 0.4, false)), "input", None),
        (Some(endpoint("output", 0.4, false)), "fabricated", None),
        (
            Some(endpoint("output", 0.4, false)),
            "output",
            Some(Some(endpoint("replacement", 0.5, false))),
        ),
        (Some(endpoint("output", 0.4, false)), "output", Some(None)),
    ] {
        let mut state = Recording::new();
        state.endpoints[0] = Ok(current);
        state.drift = drift.map(|endpoint| (AudioFlow::Output, endpoint));
        let recording = Arc::new(Mutex::new(state));
        let (service, finished) = start(Arc::clone(&recording));
        assert_eq!(
            execute(&service, volume_command(expected))
                .unwrap_err()
                .kind,
            AudioErrorKind::DeviceChanged
        );
        assert!(recording.lock().writes.is_empty());
        drop(service);
        finished.recv_timeout(WAIT).unwrap();
    }
}

#[test]
fn read_failures_are_not_absence_and_failed_readback_is_not_success() {
    let mut state = Recording::new();
    state.endpoints[1] = Err(error(AudioErrorKind::AccessDenied));
    state.fail_readback = true;
    let recording = Arc::new(Mutex::new(state));
    let (service, finished) = start(recording);
    assert_eq!(
        read(&service).input,
        EndpointState::Unavailable(error(AudioErrorKind::AccessDenied))
    );
    assert_eq!(
        execute(&service, volume_command("output"))
            .unwrap_err()
            .kind,
        AudioErrorKind::AccessDenied
    );
    drop(service);
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn accepted_requests_finish_once_before_asynchronous_shutdown() {
    let recording = Arc::new(Mutex::new(Recording::new()));
    let (release, initialize) = mpsc::channel();
    let (finished, shutdown) = mpsc::channel();
    let service = actor::spawn(move || {
        initialize.recv_timeout(WAIT).unwrap();
        Ok(RecordingDriver {
            recording,
            finished,
            registration: None,
            _thread_only: Rc::new(()),
        })
    })
    .unwrap();
    let (sender, receiver) = mpsc::channel();
    service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    drop(service); // Neither construction, read nor drop waits for initialization.
    release.send(()).unwrap();
    assert!(receiver.recv_timeout(WAIT).unwrap().is_ok());
    assert!(receiver.recv_timeout(WAIT).is_err());
    shutdown.recv_timeout(WAIT).unwrap();
}

#[test]
fn initialization_failure_completes_requests_and_reports_unavailable_watch() {
    let service =
        actor::spawn::<RecordingDriver, _>(|| Err(error(AudioErrorKind::AccessDenied))).unwrap();
    let (sender, receiver) = mpsc::channel();
    service
        .read(Box::new(move |result| sender.send(result).unwrap()))
        .unwrap();
    assert_eq!(
        receiver.recv_timeout(WAIT).unwrap().unwrap_err().kind,
        AudioErrorKind::AccessDenied
    );
    assert!(receiver.recv_timeout(WAIT).is_err());
    let (sender, events) = mpsc::channel();
    let guard = service
        .subscribe(Arc::new(move |event| sender.send(event).unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        AudioEvent::WatchUnavailable(error(AudioErrorKind::AccessDenied))
    );
    drop(guard);
    drop(service);
}

#[test]
fn readiness_waits_for_registration_drop_suppresses_signals_and_unregisters() {
    let recording = Arc::new(Mutex::new(Recording::new()));
    let (entered, entering) = mpsc::channel();
    let (release, registration) = mpsc::channel();
    let (service, finished) =
        start_with_registration(Arc::clone(&recording), Some((entered, registration)));
    let (sender, events) = mpsc::channel();
    let guard = service
        .subscribe(Arc::new(move |event| sender.send(event).unwrap()))
        .unwrap()
        .unwrap();
    entering.recv_timeout(WAIT).unwrap();
    assert!(
        events.try_recv().is_err(),
        "guard creation fabricated watch readiness"
    );
    release.send(()).unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), AudioEvent::WatchReady);
    let signals = recording.lock().signals.clone().unwrap();
    drop(guard);
    recording.lock().endpoints[0] = Ok(Some(endpoint("output", 0.9, false)));
    signals.notify(Signal::Volume);
    read(&service); // Actor barrier after drop and queued invalidation.
    assert!(events.try_recv().is_err());
    assert_eq!(recording.lock().stops, 1);
    drop(service);
    finished.recv_timeout(WAIT).unwrap();
}

#[test]
fn watcher_failure_recovers_on_device_signal_and_only_real_changes_invalidate() {
    let mut state = Recording::new();
    state.watch_error = Some(error(AudioErrorKind::Other));
    state.endpoints[1] = Ok(None); // Absence still permits default-device watching.
    let recording = Arc::new(Mutex::new(state));
    let (service, finished) = start(Arc::clone(&recording));
    let (sender, events) = mpsc::channel();
    let guard = service
        .subscribe(Arc::new(move |event| sender.send(event).unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        AudioEvent::WatchUnavailable(error(AudioErrorKind::Other))
    );
    let signals = recording.lock().signals.clone().unwrap();
    recording.lock().watch_error = None;
    signals.notify(Signal::Devices);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), AudioEvent::WatchReady);
    read(&service);
    assert!(events.try_recv().is_err());
    recording.lock().endpoints[1] = Ok(Some(endpoint("new-input", 0.3, false)));
    signals.notify(Signal::Devices);
    assert_eq!(events.recv_timeout(WAIT).unwrap(), AudioEvent::Changed);
    signals.notify(Signal::Volume);
    read(&service);
    assert!(
        events.try_recv().is_err(),
        "unchanged readback emitted a false change"
    );
    drop(service);
    assert!(
        finished.try_recv().is_err(),
        "active guard did not retain worker lifetime"
    );
    drop(guard);
    finished.recv_timeout(WAIT).unwrap();
    assert_eq!(recording.lock().stops, 1);
}

#[test]
fn dropped_guard_during_setup_never_delivers_readiness() {
    let recording = Arc::new(Mutex::new(Recording::new()));
    let (entered, entering) = mpsc::channel();
    let (release, registration) = mpsc::channel();
    let (service, finished) = start_with_registration(recording, Some((entered, registration)));
    let (sender, events) = mpsc::channel();
    let guard = service
        .subscribe(Arc::new(move |event| sender.send(event).unwrap()))
        .unwrap()
        .unwrap();
    entering.recv_timeout(WAIT).unwrap();
    drop(guard);
    release.send(()).unwrap();
    read(&service);
    assert!(events.try_recv().is_err());
    drop(service);
    finished.recv_timeout(WAIT).unwrap();
}

#[cfg(not(windows))]
#[test]
fn production_constructor_is_honestly_unsupported_off_windows() {
    assert!(matches!(
        AudioService::new(),
        Err(AudioError {
            kind: AudioErrorKind::Unsupported,
            ..
        })
    ));
}
