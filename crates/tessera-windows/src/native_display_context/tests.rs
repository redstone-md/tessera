// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::owner::{Calls, Owner};
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
}
#[derive(Clone)]
struct Config {
    monitors: Vec<Monitor>,
    dpi: u32,
    text: f64,
    fault: Fault,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            monitors: vec![monitor(1, -1200, -100, 1200, 900, true)],
            dpi: 132,
            text: 1.125,
            fault: Fault::None,
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
}
impl Calls for RecordingCalls {
    type Dpi = ();
    type Window = usize;
    type Apartment = ();
    type Settings = ();

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
    fn create_window(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<usize, DisplayContextError> {
        record(&self.events, "create");
        assert!(x >= selected.bounds.x() && x < selected.bounds.right());
        assert!(y >= selected.bounds.y() && y < selected.bounds.bottom());
        if self.config.fault == Fault::Create {
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
            self.config.fault,
            Fault::Association | Fault::Awareness
        ))
    }
    fn dpi(&mut self, _: &usize) -> Result<u32, DisplayContextError> {
        record(&self.events, "dpi");
        if self.config.fault == Fault::Dpi {
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
        if self.config.fault == Fault::Activate {
            return Err(native_error());
        }
        Ok(())
    }
    fn text_scale(&mut self, _: &()) -> Result<f64, DisplayContextError> {
        record(&self.events, "text");
        if self.config.fault == Fault::TextPanic {
            panic!("recorded settings query panic");
        }
        if self.config.fault == Fault::Text {
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
    assert!(matches!(
        geometry(&[other_primary, primary]),
        Err(DisplayContextError::InvalidData)
    ));
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
fn invalid_topology_never_allocates_a_probe() {
    let (result, events) = run(Config {
        monitors: vec![
            monitor(1, 0, 0, 100, 100, true),
            monitor(2, 100, 0, 100, 100, true),
        ],
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::InvalidData));
    assert_eq!(
        events,
        [
            "factory",
            "enter",
            "collect",
            "restore",
            "dpi-discard",
            "driver-drop",
            "callback"
        ]
    );
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
            "create",
            "association-awareness",
            "dpi",
            "initialize",
            "activate",
            "text",
            "settings-drop",
            "close",
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
            Fault::Association | Fault::Awareness => DisplayContextError::InvalidData,
            Fault::Panic | Fault::TextPanic => DisplayContextError::Unavailable,
            _ => native_error(),
        };
        assert_eq!(result, Err(expected));
        assert_eq!(&events[events.len() - 2..], &["driver-drop", "callback"]);
        assert_eq!(events.contains(&"restore"), fault != Fault::Enter);
        assert_eq!(events.contains(&"dpi-discard"), fault != Fault::Enter);
        assert_eq!(
            events.contains(&"uninitialize"),
            matches!(
                fault,
                Fault::Activate
                    | Fault::Text
                    | Fault::TextPanic
                    | Fault::Close
                    | Fault::CloseOnce
                    | Fault::Restore
                    | Fault::RestoreOnce
            )
        );
        if matches!(
            fault,
            Fault::Association
                | Fault::Awareness
                | Fault::Dpi
                | Fault::Initialize
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
        assert_eq!(result, Err(DisplayContextError::InvalidData));
    }
    let (result, events) = run(Config {
        dpi: 0,
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::InvalidData));
    assert!(!events.contains(&"initialize"));
    let (result, _) = run(Config {
        dpi: 1,
        text: f64::from_bits(1),
        ..Config::default()
    });
    assert_eq!(result, Err(DisplayContextError::InvalidData));
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
            "settings-drop",
            "close",
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
            "create",
            "association-awareness",
            "dpi",
            "initialize",
            "activate",
            "text",
            "close",
            "settings-drop",
            "uninitialize",
            "restore",
            "dpi-discard",
            "driver-drop",
            "callback"
        ]
    );
}
