// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::owner::{Calls, Owner, SNAPSHOT_LIMIT, TargetKey};
use super::*;
use parking_lot::Mutex;
use std::sync::mpsc;
use std::thread::{self, ThreadId};
use std::time::Duration;

#[derive(Clone, Copy, Default, PartialEq)]
enum Fault {
    #[default]
    None,
    Enter,
    Collect,
    Create,
    Association,
    Awareness,
    Dpi,
    Initialize,
    Activate,
    Text,
    Close,
    CloseOnce,
    Restore,
    RestoreOnce,
    TextPanic,
    Panic,
    ConfigSize,
    ConfigQuery,
    Manager,
    Targets,
    TargetCount,
    TargetProperty,
    StableId,
    ManagerPanic,
    TargetsPanic,
}
#[derive(Clone)]
struct Config {
    monitors: Vec<Monitor>,
    dpi: u32,
    text: f64,
    fault: Fault,
    paths: Option<Vec<RecordedPath>>,
    modes: Option<Vec<RecordedMode>>,
    targets: Option<Vec<RecordedTarget>>,
    faulty_monitor: Option<usize>,
    snapshot_sizes: Option<(u32, u32)>,
    size_steps: Vec<(u32, u32)>,
    query_statuses: Vec<u32>,
    returned_counts: Option<(u32, u32)>,
    restore_error: Option<DisplayContextError>,
}

#[derive(Clone, Default)]
struct RecordedPath {
    index: u32,
    key: TargetKey,
}
#[derive(Clone, Default)]
struct RecordedMode {
    source: bool,
    position: (i32, i32),
}
#[derive(Clone)]
struct RecordedTarget {
    key: Result<TargetKey, DisplayContextError>,
    id: Result<String, DisplayContextError>,
    relative_error: Option<DisplayContextError>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            monitors: vec![monitor(1, -1200, -100, 1200, 900, true)],
            dpi: 132,
            text: 1.125,
            fault: Fault::None,
            paths: None,
            modes: None,
            targets: None,
            faulty_monitor: None,
            snapshot_sizes: None,
            size_steps: Vec::new(),
            query_statuses: Vec::new(),
            returned_counts: None,
            restore_error: None,
        }
    }
}
type Events = Arc<Mutex<Vec<(&'static str, ThreadId)>>>;
fn record(events: &Events, name: &'static str) {
    events.lock().push((name, thread::current().id()));
}
fn monitor(identity: usize, x: i32, y: i32, width: u32, height: u32, primary: bool) -> Monitor {
    Monitor {
        identity,
        bounds: Rect::new(x, y, width, height).unwrap(),
        primary,
    }
}
fn native_error() -> DisplayContextError {
    DisplayContextError::Native { code: 701 }
}

/// Recorder substitutes raw operations/tokens only. Optional resource state,
/// cleanup, retry, disarm and unwind behavior belong to the production Owner.
struct RecordingCalls {
    config: Config,
    events: Events,
    close_calls: usize,
    restore_calls: usize,
    query_calls: usize,
    active_monitor: usize,
}
impl RecordingCalls {
    fn candidate_fault(&self, identity: usize) -> Fault {
        if self
            .config
            .faulty_monitor
            .is_none_or(|faulty| faulty == identity)
        {
            self.config.fault
        } else {
            Fault::None
        }
    }
    fn paths(&self) -> Vec<RecordedPath> {
        self.config.paths.clone().unwrap_or_else(|| {
            self.config
                .monitors
                .iter()
                .enumerate()
                .map(|(index, monitor)| RecordedPath {
                    index: index as u32,
                    key: TargetKey {
                        low: 17,
                        high: -3,
                        id: monitor.identity as u32,
                    },
                })
                .collect()
        })
    }
    fn modes(&self) -> Vec<RecordedMode> {
        self.config.modes.clone().unwrap_or_else(|| {
            self.config
                .monitors
                .iter()
                .map(|monitor| RecordedMode {
                    source: true,
                    position: (monitor.bounds.x(), monitor.bounds.y()),
                })
                .collect()
        })
    }
    fn targets(&self) -> Vec<RecordedTarget> {
        self.config.targets.clone().unwrap_or_else(|| {
            self.config
                .monitors
                .iter()
                .map(|monitor| RecordedTarget {
                    key: Ok(TargetKey {
                        low: 17,
                        high: -3,
                        id: monitor.identity as u32,
                    }),
                    id: Ok(format!("opaque-{:08}", monitor.identity)),
                    relative_error: None,
                })
                .collect()
        })
    }
}
impl Calls for RecordingCalls {
    type Dpi = ();
    type Window = usize;
    type Apartment = ();
    type Settings = ();
    type Manager = ();
    type Targets = Vec<RecordedTarget>;
    type Path = RecordedPath;
    type Mode = RecordedMode;

    fn enter_dpi(&mut self) -> Result<(), DisplayContextError> {
        record(&self.events, "enter");
        if self.config.fault == Fault::Enter {
            return Err(native_error());
        }
        Ok(())
    }
    fn restore_dpi(&mut self, _: &mut ()) -> Result<(), DisplayContextError> {
        record(&self.events, "restore");
        self.restore_calls += 1;
        if let Some(error) = self.config.restore_error {
            return Err(error);
        }
        if self.config.fault == Fault::Restore
            || (self.config.fault == Fault::RestoreOnce && self.restore_calls == 1)
        {
            return Err(native_error());
        }
        Ok(())
    }
    fn discard_dpi(&mut self, (): ()) {
        record(&self.events, "dpi-discard");
    }
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError> {
        record(&self.events, "collect");
        if self.config.fault == Fault::Panic {
            panic!("recorded driver panic");
        }
        if self.config.fault == Fault::Collect {
            return Err(native_error());
        }
        Ok(self.config.monitors.clone())
    }
    fn config_sizes(&mut self) -> Result<(u32, u32), DisplayContextError> {
        record(&self.events, "config-sizes");
        if self.config.fault == Fault::ConfigSize {
            return Err(native_error());
        }
        Ok(self
            .config
            .size_steps
            .get(self.query_calls)
            .copied()
            .or(self.config.snapshot_sizes)
            .unwrap_or((self.paths().len() as u32, self.modes().len() as u32)))
    }
    fn config_query(
        &mut self,
        paths: &mut [RecordedPath],
        modes: &mut [RecordedMode],
    ) -> Result<(u32, u32), DisplayContextError> {
        record(&self.events, "config-query");
        let status = self
            .config
            .query_statuses
            .get(self.query_calls)
            .copied()
            .unwrap_or(0);
        self.query_calls += 1;
        for (output, value) in paths.iter_mut().zip(self.paths()) {
            *output = value;
        }
        for (output, value) in modes.iter_mut().zip(self.modes()) {
            *output = value;
        }
        if self.config.fault == Fault::ConfigQuery {
            return Err(native_error());
        }
        if status != 0 {
            // Poisoned failed outputs must never participate in a target lookup.
            for path in paths {
                path.key.id = u32::MAX;
            }
            return Err(DisplayContextError::Native { code: status });
        }
        Ok(self
            .config
            .returned_counts
            .unwrap_or((self.paths().len() as u32, self.modes().len() as u32)))
    }
    fn source_index(path: &RecordedPath) -> u32 {
        path.index
    }
    fn source_position(mode: &RecordedMode) -> Option<(i32, i32)> {
        mode.source.then_some(mode.position)
    }
    fn path_target(path: &RecordedPath) -> TargetKey {
        path.key
    }
    fn activate_manager(&mut self) -> Result<(), DisplayContextError> {
        record(&self.events, "manager");
        if self.config.fault == Fault::ManagerPanic {
            panic!("recorded manager activation panic");
        }
        if self.config.fault == Fault::Manager {
            return Err(native_error());
        }
        Ok(())
    }
    fn current_targets(&mut self, _: &()) -> Result<Vec<RecordedTarget>, DisplayContextError> {
        record(&self.events, "targets");
        if self.config.fault == Fault::TargetsPanic {
            panic!("recorded targets acquisition panic");
        }
        if self.config.fault == Fault::Targets {
            return Err(native_error());
        }
        Ok(self.targets())
    }
    fn target_count(&mut self, targets: &Vec<RecordedTarget>) -> Result<u32, DisplayContextError> {
        record(&self.events, "target-count");
        if self.config.fault == Fault::TargetCount {
            return Err(native_error());
        }
        Ok(targets.len() as u32)
    }
    fn target_adapter(
        &mut self,
        targets: &Vec<RecordedTarget>,
        index: u32,
    ) -> Result<(u32, i32), DisplayContextError> {
        record(&self.events, "target-key");
        if self.config.fault == Fault::TargetProperty {
            return Err(native_error());
        }
        targets[index as usize].key.map(|key| (key.low, key.high))
    }
    fn target_id(
        &mut self,
        targets: &Vec<RecordedTarget>,
        index: u32,
    ) -> Result<u32, DisplayContextError> {
        record(&self.events, "target-id");
        let target = &targets[index as usize];
        if let Some(error) = target.relative_error {
            return Err(error);
        }
        target.key.map(|key| key.id)
    }
    fn stable_id(
        &mut self,
        targets: &Vec<RecordedTarget>,
        index: u32,
    ) -> Result<String, DisplayContextError> {
        record(&self.events, "stable-id");
        if self.config.fault == Fault::StableId {
            return Err(native_error());
        }
        targets[index as usize].id.clone()
    }
    fn release_targets(&mut self, _: Vec<RecordedTarget>) {
        record(&self.events, "targets-drop");
    }
    fn release_manager(&mut self, _: ()) {
        record(&self.events, "manager-drop");
    }
    fn create_window(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<usize, DisplayContextError> {
        record(&self.events, "create");
        assert!(x >= selected.bounds.x() && x < selected.bounds.right());
        assert!(y >= selected.bounds.y() && y < selected.bounds.bottom());
        self.active_monitor = selected.identity;
        if self.candidate_fault(selected.identity) == Fault::Create {
            return Err(native_error());
        }
        Ok(selected.identity)
    }
    fn probe_matches(
        &mut self,
        window: &usize,
        selected: Monitor,
    ) -> Result<bool, DisplayContextError> {
        assert_eq!(*window, selected.identity);
        record(&self.events, "association-awareness");
        Ok(!matches!(
            self.candidate_fault(selected.identity),
            Fault::Association | Fault::Awareness
        ))
    }
    fn dpi(&mut self, _: &usize) -> Result<u32, DisplayContextError> {
        record(&self.events, "dpi");
        if self.candidate_fault(self.active_monitor) == Fault::Dpi {
            return Err(native_error());
        }
        Ok(self.config.dpi)
    }
    fn destroy_window(&mut self, _: &usize) -> Result<(), DisplayContextError> {
        record(&self.events, "close");
        self.close_calls += 1;
        if self.config.fault == Fault::Close
            || (self.config.fault == Fault::CloseOnce && self.close_calls == 1)
        {
            return Err(native_error());
        }
        Ok(())
    }
    fn initialize(&mut self) -> Result<(), DisplayContextError> {
        record(&self.events, "initialize");
        if self.config.fault == Fault::Initialize {
            return Err(native_error());
        }
        Ok(())
    }
    fn uninitialize(&mut self, (): ()) {
        record(&self.events, "uninitialize");
    }
    fn activate_settings(&mut self) -> Result<(), DisplayContextError> {
        record(&self.events, "activate");
        if self.candidate_fault(self.active_monitor) == Fault::Activate {
            return Err(native_error());
        }
        Ok(())
    }
    fn text_scale(&mut self, _: &()) -> Result<f64, DisplayContextError> {
        record(&self.events, "text");
        if self.candidate_fault(self.active_monitor) == Fault::TextPanic {
            panic!("recorded settings query panic");
        }
        if self.candidate_fault(self.active_monitor) == Fault::Text {
            return Err(native_error());
        }
        Ok(self.config.text)
    }
    fn release_settings(&mut self, (): ()) {
        record(&self.events, "settings-drop");
    }
}
impl Drop for RecordingCalls {
    fn drop(&mut self) {
        record(&self.events, "driver-drop");
    }
}
fn host(config: Config, events: Events, inline: bool) -> NativeDisplayContextHost {
    NativeDisplayContextHost {
        gate: FlightGate::default(),
        factory: Arc::new(move || {
            record(&events, "factory");
            Ok(Box::new(Owner::new(RecordingCalls {
                config: config.clone(),
                events: events.clone(),
                close_calls: 0,
                restore_calls: 0,
                query_calls: 0,
                active_monitor: 0,
            })?))
        }),
        spawn: Arc::new(move |job| {
            if inline {
                job();
                Ok(())
            } else {
                thread::Builder::new().spawn(job).map(|_| ())
            }
        }),
    }
}
fn run(config: Config) -> (ReadResult, Vec<&'static str>) {
    let events = Events::default();
    let host = host(config, events.clone(), true);
    let (tx, rx) = mpsc::channel();
    let callback_events = events.clone();
    host.read(Box::new(move |result| {
        record(&callback_events, "callback");
        tx.send(result).unwrap();
    }))
    .unwrap();
    let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let names = events.lock().iter().map(|(name, _)| *name).collect();
    assert!(host.gate.try_enter().is_some());
    (result, names)
}

#[test]
fn zero_inclusive_union_and_explicit_selection() {
    assert!(geometry(&[]).unwrap().is_none());
    let first = monitor(1, 100, 200, 300, 400, false);
    let second = monitor(2, -1000, -300, 500, 700, false);
    let (bounds, selected, selection) = geometry(&[first]).unwrap().unwrap();
    assert_eq!(bounds, Rect::new(0, 0, 400, 600).unwrap());
    assert_eq!(selected.identity, 1);
    assert_eq!(selection, DisplaySelection::FirstFallback);
    let (bounds, selected, selection) = geometry(&[first, second]).unwrap().unwrap();
    assert_eq!(bounds, Rect::new(-1000, -300, 1400, 900).unwrap());
    assert_eq!(selected.identity, 1);
    assert_eq!(selection, DisplaySelection::FirstFallback);
    let mut primary = second;
    primary.primary = true;
    let (_, selected, selection) = geometry(&[first, primary]).unwrap().unwrap();
    assert_eq!(selected.identity, 2);
    assert_eq!(selection, DisplaySelection::Primary);
    let mut other_primary = first;
    other_primary.primary = true;
    let (_, selected, selection) = geometry(&[other_primary, primary]).unwrap().unwrap();
    assert_eq!(selected.identity, 1);
    assert_eq!(selection, DisplaySelection::Primary);
    let negative = monitor(3, -500, -400, 100, 100, false);
    assert_eq!(
        geometry(&[negative]).unwrap().unwrap().0,
        Rect::new(-500, -400, 500, 400).unwrap()
    );
    let duplicate = monitor(1, -1, -1, 1, 1, false);
    assert!(matches!(
        geometry(&[first, duplicate]),
        Err(DisplayContextError::InvalidData)
    ));
    assert!(matches!(
        geometry(&[monitor(0, 0, 0, 1, 1, false)]),
        Err(DisplayContextError::InvalidData)
    ));
}

#[test]
fn probe_center_handles_tiny_signed_and_extreme_rects() {
    for rect in [
        Rect::new(-1, -1, 1, 1).unwrap(),
        Rect::new(i32::MIN, i32::MIN, u32::MAX, u32::MAX).unwrap(),
        Rect::new(i32::MAX - 1, 0, 1, 1).unwrap(),
    ] {
        let (x, y) = probe_center(rect).unwrap();
        assert!(x >= rect.x() && x < rect.right());
        assert!(y >= rect.y() && y < rect.bottom());
    }
}

#[test]
fn admitted_duplicate_handles_are_rejected_after_projection() {
    let (result, events) = run(Config {
        monitors: vec![
            monitor(1, 0, 0, 100, 100, true),
            monitor(1, 100, 0, 100, 100, true),
        ],
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::InvalidData));
    assert_eq!(events.iter().filter(|name| **name == "create").count(), 2);
    assert_eq!(events.iter().filter(|name| **name == "close").count(), 2);
    assert_eq!(&events[events.len() - 2..], &["driver-drop", "callback"]);
}

#[test]
fn fractional_scale_and_retirement_order() {
    let (result, events) = run(Config::default());
    assert_eq!(result.unwrap().unwrap().presentation_scale(), 1.546875);
    assert_eq!(
        events,
        [
            "factory",
            "enter",
            "collect",
            "config-sizes",
            "config-query",
            "initialize",
            "manager",
            "targets",
            "target-count",
            "target-key",
            "target-id",
            "stable-id",
            "create",
            "association-awareness",
            "dpi",
            "activate",
            "text",
            "close",
            "settings-drop",
            "targets-drop",
            "manager-drop",
            "uninitialize",
            "restore",
            "dpi-discard",
            "driver-drop",
            "callback"
        ]
    );
    let (result, events) = run(Config {
        monitors: vec![],
        ..Config::default()
    });
    assert_eq!(result, Ok(None));
    assert_eq!(
        events,
        [
            "factory",
            "enter",
            "collect",
            "config-sizes",
            "config-query",
            "initialize",
            "manager",
            "targets",
            "target-count",
            "targets-drop",
            "manager-drop",
            "uninitialize",
            "restore",
            "dpi-discard",
            "driver-drop",
            "callback"
        ]
    );
}

#[test]
fn failures_and_invalid_scales_never_default_or_skip_retirement() {
    for fault in [
        Fault::Enter,
        Fault::Collect,
        Fault::Create,
        Fault::Association,
        Fault::Awareness,
        Fault::Dpi,
        Fault::Initialize,
        Fault::Activate,
        Fault::Text,
        Fault::Close,
        Fault::CloseOnce,
        Fault::Restore,
        Fault::RestoreOnce,
        Fault::Panic,
        Fault::TextPanic,
    ] {
        let (result, events) = run(Config {
            fault,
            ..Config::default()
        });
        let expected = match fault {
            Fault::Create
            | Fault::Association
            | Fault::Awareness
            | Fault::Dpi
            | Fault::Activate
            | Fault::Text => Ok(None),
            Fault::Panic | Fault::TextPanic => Err(DisplayContextError::Unavailable),
            _ => Err(native_error()),
        };
        assert_eq!(result, expected);
        assert_eq!(&events[events.len() - 2..], &["driver-drop", "callback"]);
        assert_eq!(events.contains(&"restore"), fault != Fault::Enter);
        assert_eq!(events.contains(&"dpi-discard"), fault != Fault::Enter);
        assert_eq!(
            events.contains(&"uninitialize"),
            !matches!(
                fault,
                Fault::Enter | Fault::Collect | Fault::Initialize | Fault::Panic
            )
        );
        if matches!(
            fault,
            Fault::Association
                | Fault::Awareness
                | Fault::Dpi
                | Fault::Activate
                | Fault::Text
                | Fault::TextPanic
                | Fault::Close
                | Fault::CloseOnce
        ) {
            assert!(events.contains(&"close"));
        }
        if fault == Fault::Close {
            assert_eq!(events.iter().filter(|name| **name == "close").count(), 3);
        }
        if fault == Fault::CloseOnce {
            assert_eq!(events.iter().filter(|name| **name == "close").count(), 2);
        }
        if matches!(fault, Fault::Restore | Fault::RestoreOnce) {
            assert_eq!(events.iter().filter(|name| **name == "restore").count(), 2);
        }
        if events.contains(&"text") {
            assert!(
                events
                    .iter()
                    .position(|name| *name == "settings-drop")
                    .unwrap()
                    < events
                        .iter()
                        .position(|name| *name == "uninitialize")
                        .unwrap()
            );
        }
    }
    for text in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        let (result, _) = run(Config {
            text,
            dpi: 192,
            ..Config::default()
        });
        assert_eq!(result, Ok(None));
    }
    let (result, events) = run(Config {
        dpi: 0,
        ..Config::default()
    });
    assert_eq!(result, Ok(None));
    assert!(events.contains(&"initialize"));
    let (result, _) = run(Config {
        dpi: 1,
        text: f64::from_bits(1),
        ..Config::default()
    });
    assert_eq!(result, Ok(None));
}

#[test]
fn spawn_failure_and_panic_accept_zero_callbacks_and_release_flight() {
    for panic in [false, true] {
        let events = Events::default();
        let mut host = host(Config::default(), events.clone(), true);
        host.spawn = Arc::new(move |_job| {
            if panic {
                panic!("recorded spawn panic");
            }
            Err(std::io::Error::other("recorded spawn failure"))
        });
        let (tx, rx) = mpsc::channel();
        assert_eq!(
            host.read(Box::new(move |_| {
                tx.send(()).unwrap();
            })),
            Err(DisplayContextError::Unavailable)
        );
        assert!(rx.try_recv().is_err());
        assert!(events.lock().is_empty());
        assert!(host.gate.try_enter().is_some());
    }
}

#[test]
fn factory_failure_and_panic_complete_once_after_gate_release() {
    for panic in [false, true] {
        let mut host = host(Config::default(), Events::default(), true);
        host.factory = Arc::new(move || {
            if panic {
                panic!("recorded factory panic");
            }
            Err(native_error())
        });
        let clone = host.clone();
        let (tx, rx) = mpsc::channel();
        host.read(Box::new(move |result| {
            assert!(clone.gate.try_enter().is_some());
            tx.send(result).unwrap();
        }))
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(if panic {
                DisplayContextError::Unavailable
            } else {
                native_error()
            })
        );
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn clone_busy_rejects_without_callback_then_owner_retires_before_callback() {
    let events = Events::default();
    let mut host = host(Config::default(), events.clone(), false);
    let factory = host.factory.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    host.factory = Arc::new(move || {
        entered_tx.send(thread::current().id()).unwrap();
        release_rx
            .lock()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        factory()
    });
    let clone = host.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let callback_events = events.clone();
    let callback_clone = clone.clone();
    host.read(Box::new(move |result| {
        assert!(callback_clone.gate.try_enter().is_some());
        record(&callback_events, "callback");
        done_tx.send(result).unwrap();
    }))
    .unwrap();
    let owner = entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_ne!(owner, thread::current().id());
    let (rejected_tx, rejected_rx) = mpsc::channel();
    assert_eq!(
        clone.read(Box::new(move |_| {
            rejected_tx.send(()).unwrap();
        })),
        Err(DisplayContextError::Busy)
    );
    assert!(rejected_rx.try_recv().is_err());
    release_tx.send(()).unwrap();
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_ok()
    );
    let events = events.lock();
    assert!(events.iter().all(|(_, thread)| *thread == owner));
    assert_eq!(
        &events[events.len() - 2..]
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>(),
        &["driver-drop", "callback"]
    );
}

#[test]
fn consumer_panic_is_contained_and_reentrant_next_read_is_allowed() {
    let events = Events::default();
    let host = host(Config::default(), events.clone(), true);
    host.read(Box::new(|_| panic!("recorded consumer panic")))
        .unwrap();
    let clone = host.clone();
    let (tx, rx) = mpsc::channel();
    host.read(Box::new(move |first| {
        assert!(first.is_ok());
        clone
            .read(Box::new(move |second| {
                tx.send(second).unwrap();
            }))
            .unwrap();
    }))
    .unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap().is_ok());
    assert_eq!(
        events
            .lock()
            .iter()
            .filter(|(name, _)| *name == "factory")
            .count(),
        3
    );
}

#[test]
fn shared_owner_disarms_success_and_finish_is_idempotent() {
    let events = Events::default();
    let config = Config::default();
    let selected = config.monitors[0];
    let mut owner = Owner::new(RecordingCalls {
        config,
        events: events.clone(),
        close_calls: 0,
        restore_calls: 0,
        query_calls: 0,
        active_monitor: 0,
    })
    .unwrap();
    let (x, y) = probe_center(selected.bounds).unwrap();
    owner.create_probe(selected, x, y).unwrap();
    owner.text_scale().unwrap();
    owner.close_probe().unwrap();
    owner.finish().unwrap();
    owner.finish().unwrap();
    drop(owner);
    let names: Vec<_> = events.lock().iter().map(|(name, _)| *name).collect();
    assert_eq!(
        names,
        [
            "enter",
            "create",
            "initialize",
            "activate",
            "text",
            "close",
            "settings-drop",
            "uninitialize",
            "restore",
            "dpi-discard",
            "driver-drop"
        ]
    );
}

#[test]
fn shared_owner_unwinds_with_live_settings_before_uninitialization_and_dpi() {
    let (result, events) = run(Config {
        fault: Fault::TextPanic,
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::Unavailable));
    assert_eq!(
        events,
        [
            "factory",
            "enter",
            "collect",
            "config-sizes",
            "config-query",
            "initialize",
            "manager",
            "targets",
            "target-count",
            "target-key",
            "target-id",
            "stable-id",
            "create",
            "association-awareness",
            "dpi",
            "activate",
            "text",
            "close",
            "settings-drop",
            "targets-drop",
            "manager-drop",
            "uninitialize",
            "restore",
            "dpi-discard",
            "driver-drop",
            "callback"
        ]
    );
}

fn key(id: usize) -> TargetKey {
    TargetKey {
        low: 17,
        high: -3,
        id: id as u32,
    }
}
fn target(key: TargetKey, id: &str) -> RecordedTarget {
    RecordedTarget {
        key: Ok(key),
        id: Ok(id.into()),
        relative_error: None,
    }
}
fn count(events: &[&str], name: &str) -> usize {
    events.iter().filter(|event| **event == name).count()
}

#[test]
fn filtered_primary_uses_first_stable_survivor_not_raw_first() {
    let first = monitor(1, 0, 0, 100, 100, false);
    let second = monitor(2, 100, 0, 100, 100, false);
    let filtered_primary = monitor(3, -100, 0, 100, 100, true);
    for monitors in [
        vec![first, second, filtered_primary],
        vec![filtered_primary, second, first],
    ] {
        let layout = run(Config {
            monitors,
            targets: Some(vec![target(key(1), "z-opaque"), target(key(2), "a-opaque")]),
            ..Config::default()
        })
        .0
        .unwrap()
        .unwrap();
        assert_eq!(layout.selected_bounds(), second.bounds);
        assert_eq!(layout.selection(), DisplaySelection::FirstFallback);
        assert_eq!(layout.desktop_bounds(), Rect::new(0, 0, 200, 100).unwrap());
    }
}

#[test]
fn empty_and_duplicate_opaque_ids_preserve_raw_ties_and_first_sorted_primary() {
    let first = monitor(1, 0, 0, 100, 100, false);
    let second = monitor(2, 100, 0, 100, 100, false);
    for id in ["", "same opaque ID: not a device path"] {
        for monitors in [vec![first, second], vec![second, first]] {
            let expected = monitors[0].bounds;
            let layout = run(Config {
                monitors,
                targets: Some(vec![target(key(1), id), target(key(2), id)]),
                ..Config::default()
            })
            .0
            .unwrap()
            .unwrap();
            assert_eq!(layout.selected_bounds(), expected);
            assert_eq!(layout.desktop_bounds(), Rect::new(0, 0, 200, 100).unwrap());
        }
    }
    let mut first_primary = first;
    first_primary.primary = true;
    let mut second_primary = second;
    second_primary.primary = true;
    let layout = run(Config {
        monitors: vec![first_primary, second_primary],
        targets: Some(vec![target(key(1), "z"), target(key(2), "a")]),
        ..Config::default()
    })
    .0
    .unwrap()
    .unwrap();
    assert_eq!(layout.selected_bounds(), second.bounds);
    assert_eq!(layout.selection(), DisplaySelection::Primary);
}

#[test]
fn ccd_priority_tries_next_path_after_property_failure_then_stops_at_success() {
    let mut failed = target(key(1), "unusable");
    failed.id = Err(native_error());
    let config = Config {
        paths: Some(vec![
            RecordedPath {
                index: 0,
                key: key(1),
            },
            RecordedPath {
                index: 0,
                key: key(2),
            },
            RecordedPath {
                index: 0,
                key: key(3),
            },
        ]),
        targets: Some(vec![
            failed,
            target(key(2), "successful"),
            target(key(3), "later"),
        ]),
        ..Config::default()
    };
    let (result, events) = run(config);
    assert!(result.unwrap().is_some());
    assert_eq!(count(&events, "stable-id"), 2);
    assert_eq!(count(&events, "manager"), 1);
    assert_eq!(count(&events, "targets"), 1);
}

#[test]
fn adapter_luid_both_parts_and_relative_target_id_are_required() {
    let expected = key(7);
    let wrong_high = TargetKey {
        high: 9,
        ..expected
    };
    let wrong_low = TargetKey { low: 9, ..expected };
    let wrong_id = TargetKey { id: 8, ..expected };
    let mut unrelated = target(wrong_high, "wrong-high");
    unrelated.relative_error = Some(native_error());
    let (result, events) = run(Config {
        paths: Some(vec![RecordedPath {
            index: 0,
            key: expected,
        }]),
        targets: Some(vec![
            unrelated,
            target(wrong_low, "wrong-low"),
            target(wrong_id, "wrong-target"),
            target(expected, "exact"),
        ]),
        ..Config::default()
    });
    assert!(result.unwrap().is_some());
    assert_eq!(count(&events, "target-key"), 4);
    assert_eq!(count(&events, "target-id"), 2);
    assert_eq!(count(&events, "stable-id"), 1);
}

#[test]
fn target_property_errors_fail_only_the_candidate_lookup_without_global_filtering() {
    // Source stops lookup on the first property error, even when a later target matches.
    let mut failed = target(key(9), "bad");
    failed.key = Err(native_error());
    let (result, events) = run(Config {
        targets: Some(vec![failed, target(key(1), "would-match")]),
        ..Config::default()
    });
    assert_eq!(result, Ok(None));
    assert_eq!(count(&events, "target-key"), 1);
    assert_eq!(count(&events, "create"), 0);
    for fault in [Fault::TargetProperty, Fault::StableId] {
        assert_eq!(
            run(Config {
                fault,
                ..Config::default()
            })
            .0,
            Ok(None)
        );
    }
    let mut relative_failed = target(key(1), "bad");
    relative_failed.relative_error = Some(native_error());
    assert_eq!(
        run(Config {
            targets: Some(vec![relative_failed]),
            ..Config::default()
        })
        .0,
        Ok(None)
    );
}

#[test]
fn invalid_or_non_source_ccd_indices_and_position_mismatch_exclude_only_projection() {
    for path in [
        RecordedPath {
            index: u32::MAX,
            key: key(1),
        },
        RecordedPath {
            index: 1,
            key: key(1),
        },
    ] {
        assert_eq!(
            run(Config {
                paths: Some(vec![path]),
                ..Config::default()
            })
            .0,
            Ok(None)
        );
    }
    for mode in [
        RecordedMode {
            source: false,
            position: (-1200, -100),
        },
        RecordedMode {
            source: true,
            position: (-1199, -100),
        },
    ] {
        assert_eq!(
            run(Config {
                modes: Some(vec![mode]),
                ..Config::default()
            })
            .0,
            Ok(None)
        );
    }
    assert_eq!(
        run(Config {
            targets: Some(vec![]),
            ..Config::default()
        })
        .0,
        Ok(None)
    );
    for fault in [
        Fault::Collect,
        Fault::ConfigSize,
        Fault::ConfigQuery,
        Fault::Initialize,
        Fault::Manager,
        Fault::Targets,
        Fault::TargetCount,
    ] {
        let (result, events) = run(Config {
            fault,
            ..Config::default()
        });
        assert_eq!(result, Err(native_error()));
        assert_eq!(count(&events, "callback"), 1);
        assert_eq!(
            count(&events, "uninitialize"),
            usize::from(matches!(
                fault,
                Fault::Manager | Fault::Targets | Fault::TargetCount
            ))
        );
        assert_eq!(
            count(&events, "manager-drop"),
            usize::from(matches!(fault, Fault::Targets | Fault::TargetCount))
        );
        assert_eq!(
            count(&events, "targets-drop"),
            usize::from(fault == Fault::TargetCount)
        );
    }
    // Empty raw enumeration must not hide failure of the global query stack.
    assert_eq!(
        run(Config {
            monitors: vec![],
            fault: Fault::Manager,
            ..Config::default()
        })
        .0,
        Err(native_error())
    );
}

#[test]
fn excluded_malformed_handles_do_not_poison_survivors() {
    let accepted = monitor(1, 0, 0, 100, 100, false);
    let excluded_duplicate = monitor(1, 100, 0, 100, 100, true);
    let excluded_zero = monitor(0, -100, 0, 100, 100, true);
    let layout = run(Config {
        monitors: vec![excluded_zero, excluded_duplicate, accepted],
        paths: Some(vec![RecordedPath {
            index: 2,
            key: key(1),
        }]),
        ..Config::default()
    })
    .0
    .unwrap()
    .unwrap();
    assert_eq!(layout.selected_bounds(), accepted.bounds);
    assert_eq!(layout.desktop_bounds(), accepted.bounds);
    assert_eq!(layout.selection(), DisplaySelection::FirstFallback);
    assert_eq!(
        run(Config {
            monitors: vec![monitor(0, 0, 0, 100, 100, true)],
            ..Config::default()
        })
        .0,
        Err(DisplayContextError::InvalidData)
    );
}

#[test]
fn all_candidates_need_scale_and_failed_selected_association_uses_surviving_fallback() {
    let first = monitor(1, 0, 0, 100, 100, true);
    let second = monitor(2, 100, 0, 100, 100, false);
    for fault in [
        Fault::Dpi,
        Fault::Text,
        Fault::Association,
        Fault::Awareness,
        Fault::Create,
    ] {
        let (result, events) = run(Config {
            monitors: vec![first, second],
            fault,
            faulty_monitor: Some(2),
            ..Config::default()
        });
        let layout = result.unwrap().unwrap();
        assert_eq!(layout.desktop_bounds(), first.bounds);
        assert_eq!(layout.selected_bounds(), first.bounds);
        assert_eq!(count(&events, "create"), 2);
        assert_eq!(
            count(&events, "close"),
            if fault == Fault::Create { 1 } else { 2 }
        );
        assert_eq!(count(&events, "initialize"), 1);
        let layout = run(Config {
            monitors: vec![first, second],
            fault,
            faulty_monitor: Some(1),
            ..Config::default()
        })
        .0
        .unwrap()
        .unwrap();
        assert_eq!(layout.selected_bounds(), second.bounds);
        assert_eq!(layout.selection(), DisplaySelection::FirstFallback);
    }
}

#[test]
fn probes_are_serial_and_settings_and_apartment_are_reused_and_released_in_order() {
    let events = Events::default();
    let mut owner = Owner::new(RecordingCalls {
        config: Config {
            monitors: vec![
                monitor(1, 0, 0, 100, 100, true),
                monitor(2, 100, 0, 100, 100, false),
            ],
            ..Config::default()
        },
        events: events.clone(),
        close_calls: 0,
        restore_calls: 0,
        query_calls: 0,
        active_monitor: 0,
    })
    .unwrap();
    // An early scale query cannot overwrite the existing balanced MTA token.
    owner.text_scale().unwrap();
    assert_eq!(owner.monitors().unwrap().len(), 2);
    owner.finish().unwrap();
    owner.finish().unwrap();
    drop(owner);
    let names: Vec<_> = events.lock().iter().map(|(name, _)| *name).collect();
    assert_eq!(count(&names, "initialize"), 1);
    assert_eq!(count(&names, "activate"), 1);
    assert_eq!(count(&names, "text"), 3);
    assert_eq!(count(&names, "settings-drop"), 1);
    assert_eq!(count(&names, "uninitialize"), 1);
    let probes: Vec<_> = names
        .iter()
        .copied()
        .filter(|name| matches!(*name, "create" | "close"))
        .collect();
    assert_eq!(probes, ["create", "close", "create", "close"]);
    let retirement: Vec<_> = names
        .iter()
        .copied()
        .filter(|name| {
            matches!(
                *name,
                "settings-drop" | "targets-drop" | "manager-drop" | "uninitialize" | "restore"
            )
        })
        .collect();
    assert_eq!(
        retirement,
        [
            "settings-drop",
            "targets-drop",
            "manager-drop",
            "uninitialize",
            "restore"
        ]
    );
}

#[test]
fn ccd_size_races_are_bounded_and_failed_outputs_never_get_used() {
    let (result, events) = run(Config {
        size_steps: vec![(0, 0), (1, 1)],
        query_statuses: vec![122, 0],
        ..Config::default()
    });
    assert!(result.unwrap().is_some());
    assert_eq!(count(&events, "config-sizes"), 2);
    assert_eq!(count(&events, "config-query"), 2);
    assert_eq!(count(&events, "stable-id"), 1);
    for statuses in [vec![122, 122, 122, 0], vec![5, 0]] {
        let expected = statuses[0];
        let (result, events) = run(Config {
            query_statuses: statuses,
            ..Config::default()
        });
        assert_eq!(result, Err(DisplayContextError::Native { code: expected }));
        assert_eq!(
            count(&events, "config-query"),
            if expected == 122 { 3 } else { 1 }
        );
        assert_eq!(count(&events, "target-key"), 0);
        assert_eq!(count(&events, "initialize"), 0);
    }
    for sizes in [(SNAPSHOT_LIMIT as u32 + 1, 1), (1, u32::MAX)] {
        let (result, events) = run(Config {
            snapshot_sizes: Some(sizes),
            ..Config::default()
        });
        assert_eq!(result, Err(DisplayContextError::InvalidData));
        assert_eq!(count(&events, "config-query"), 0);
    }
    for returned_counts in [(2, 1), (1, 2)] {
        let (result, events) = run(Config {
            returned_counts: Some(returned_counts),
            ..Config::default()
        });
        assert_eq!(result, Err(DisplayContextError::InvalidData));
        assert_eq!(count(&events, "target-key"), 0);
    }
    let (result, events) = run(Config {
        returned_counts: Some((0, 0)),
        ..Config::default()
    });
    assert_eq!(result, Ok(None));
    assert_eq!(count(&events, "target-key"), 0);
}

#[test]
fn first_checked_probe_cleanup_failure_aborts_before_next_candidate() {
    let (result, events) = run(Config {
        monitors: vec![
            monitor(1, 0, 0, 100, 100, true),
            monitor(2, 100, 0, 100, 100, false),
        ],
        fault: Fault::CloseOnce,
        ..Config::default()
    });
    assert_eq!(result, Err(native_error()));
    assert_eq!(count(&events, "create"), 1);
    assert_eq!(count(&events, "close"), 2);
    assert_eq!(count(&events, "callback"), 1);
}

#[test]
fn accepted_host_drop_keeps_same_owner_resources_alive_until_completion() {
    let events = Events::default();
    let mut host = host(Config::default(), events.clone(), false);
    let factory = host.factory.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    host.factory = Arc::new(move || {
        entered_tx.send(thread::current().id()).unwrap();
        release_rx
            .lock()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        factory()
    });
    let (done_tx, done_rx) = mpsc::channel();
    let callback_events = events.clone();
    host.read(Box::new(move |result| {
        record(&callback_events, "callback");
        done_tx.send(result).unwrap();
    }))
    .unwrap();
    let owner = entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    drop(host);
    release_tx.send(()).unwrap();
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .is_some()
    );
    let events = events.lock();
    assert!(events.iter().all(|(_, thread)| *thread == owner));
    assert_eq!(events.last().unwrap().0, "callback");
    assert_eq!(events[events.len() - 2].0, "driver-drop");
}

#[test]
fn partial_manager_unwind_and_target_caps_retire_acquired_resources_only() {
    for fault in [Fault::ManagerPanic, Fault::TargetsPanic] {
        let (result, events) = run(Config {
            fault,
            ..Config::default()
        });
        assert_eq!(result, Err(DisplayContextError::Unavailable));
        assert_eq!(count(&events, "uninitialize"), 1);
        assert_eq!(
            count(&events, "manager-drop"),
            usize::from(fault == Fault::TargetsPanic)
        );
        assert_eq!(count(&events, "targets-drop"), 0);
        assert_eq!(&events[events.len() - 2..], &["driver-drop", "callback"]);
    }
    let (result, events) = run(Config {
        targets: Some(vec![target(key(1), "opaque"); SNAPSHOT_LIMIT + 1]),
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::InvalidData));
    assert_eq!(count(&events, "targets-drop"), 1);
    assert_eq!(count(&events, "manager-drop"), 1);
    assert_eq!(count(&events, "create"), 0);
}

#[test]
fn cleanup_errors_never_become_empty_and_first_checked_failure_is_retained() {
    let later = DisplayContextError::Native { code: 702 };
    let (result, events) = run(Config {
        fault: Fault::CloseOnce,
        restore_error: Some(later),
        ..Config::default()
    });
    assert_eq!(result, Err(native_error()));
    assert_eq!(count(&events, "close"), 2);
    assert_eq!(count(&events, "restore"), 2);
    let (result, events) = run(Config {
        targets: Some(vec![]),
        restore_error: Some(later),
        ..Config::default()
    });
    assert_eq!(result, Err(later));
    assert_eq!(count(&events, "create"), 0);
    assert_eq!(count(&events, "restore"), 2);
    assert_eq!(count(&events, "callback"), 1);
}
