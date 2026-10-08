// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread::{ThreadId, current};
use std::time::Duration;

use parking_lot::Mutex;
use tessera_system::dock_utilities::{DockUtilitiesHost, DockUtilityErrorKind};

use super::*;
use crate::dock_utilities::worker;

#[derive(Clone, Copy, Default)]
struct Plan {
    initialization: i32,
    activation_error: Option<u32>,
    toggle_error: Option<u32>,
    panic_activation: bool,
    panic_toggle: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Initialize(i32),
    Activate(u128, u32),
    Toggle,
    Release,
    Uninitialize,
    DriverDrop,
    Completion,
}

#[derive(Clone, Default)]
struct Recording(Arc<Mutex<Vec<(ThreadId, Event)>>>);

impl Recording {
    fn push(&self, event: Event) {
        self.0.lock().push((current().id(), event));
    }

    fn events(&self) -> Vec<Event> {
        self.0.lock().iter().map(|(_, event)| *event).collect()
    }

    fn assert_worker_confined(&self) {
        let log = self.0.lock();
        let worker = log.first().unwrap().0;
        assert_ne!(worker, current().id());
        assert!(log.iter().all(|(thread, _)| *thread == worker));
    }
}

struct RecordingCalls {
    recording: Recording,
    plan: Plan,
    _thread_bound: Rc<()>,
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.recording.push(Event::DriverDrop);
    }
}

struct RecordingShell {
    recording: Recording,
    _thread_bound: Rc<()>,
}

impl Drop for RecordingShell {
    fn drop(&mut self) {
        self.recording.push(Event::Release);
    }
}

impl Calls for RecordingCalls {
    type Shell = RecordingShell;

    fn initialize(&self, model: i32) -> i32 {
        self.recording.push(Event::Initialize(model));
        self.plan.initialization
    }

    fn create_shell(&self, class: u128, context: u32) -> Result<Self::Shell, u32> {
        self.recording.push(Event::Activate(class, context));
        assert!(!self.plan.panic_activation, "recorded activation panic");
        if let Some(code) = self.plan.activation_error {
            return Err(code);
        }
        Ok(RecordingShell {
            recording: self.recording.clone(),
            _thread_bound: Rc::new(()),
        })
    }

    fn toggle_desktop(&self, _: &Self::Shell) -> Result<(), u32> {
        self.recording.push(Event::Toggle);
        assert!(!self.plan.panic_toggle, "recorded toggle panic");
        self.plan.toggle_error.map_or(Ok(()), Err)
    }

    fn uninitialize(&self) {
        self.recording.push(Event::Uninitialize);
    }
}

fn host(recording: &Recording, plans: Vec<Plan>) -> Arc<dyn DockUtilitiesHost> {
    let recording = recording.clone();
    let mut plans = plans.into_iter();
    worker::start(move || {
        Ok(DockUtilitiesDriver {
            calls: RecordingCalls {
                recording: recording.clone(),
                plan: plans.next().expect("unexpected automatic retry"),
                _thread_bound: Rc::new(()),
            },
        })
    })
    .unwrap()
}

fn request(
    host: &Arc<dyn DockUtilitiesHost>,
    recording: &Recording,
) -> Result<(), DockUtilityError> {
    let recording = recording.clone();
    let (done, result) = channel();
    host.toggle_desktop(Box::new(move |completion| {
        recording.push(Event::Completion);
        done.send(completion).unwrap();
    }))
    .unwrap();
    let completion = result
        .recv_timeout(Duration::from_secs(5))
        .expect("recording action stalled");
    assert!(matches!(
        result.recv_timeout(Duration::from_secs(5)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    completion
}

fn success_events() -> Vec<Event> {
    vec![
        Event::Initialize(2),
        Event::Activate(SHELL_CLSID, 0x8001),
        Event::Toggle,
        Event::Release,
        Event::Uninitialize,
        Event::DriverDrop,
        Event::Completion,
    ]
}

#[test]
fn fixed_shell_activation_and_strict_sta_constants() {
    assert_eq!(SHELL_CLSID, 0x13709620_c279_11ce_a49e_444553540000);
    assert_eq!(SHELL_INTERFACE_IID, 0x286e6f1b_7113_4355_9562_96b7e9d64c54);
    assert_eq!(SHELL_CONTEXT, 0x8001);
    assert_eq!(STA_MODEL, 2);
    #[cfg(windows)]
    {
        use windows::Win32::System::Com::{
            CLSCTX_DISABLE_AAA, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{IShellDispatch6, Shell};
        use windows::core::Interface;
        assert_eq!(SHELL_CLSID, Shell.to_u128());
        assert_eq!(SHELL_INTERFACE_IID, IShellDispatch6::IID.to_u128());
        assert_eq!(SHELL_CONTEXT, (CLSCTX_INPROC_SERVER | CLSCTX_DISABLE_AAA).0);
        assert_eq!(STA_MODEL, COINIT_APARTMENTTHREADED.0);
    }
}

#[test]
fn native_construction_is_lazy_and_two_requests_have_two_complete_lifetimes() {
    let recording = Recording::default();
    let host = host(&recording, vec![Plan::default(), Plan::default()]);
    assert!(recording.events().is_empty());
    request(&host, &recording).unwrap();
    // Completion itself proves release/uninitialize happened before the idle
    // worker waits; there are no later destruction events to wait for.
    assert_eq!(recording.events(), success_events());
    request(&host, &recording).unwrap();
    assert_eq!(recording.events(), success_events().repeat(2));
    recording.assert_worker_confined();
}

#[test]
fn s_false_initialization_is_balanced_after_shell_release() {
    let recording = Recording::default();
    let host = host(
        &recording,
        vec![Plan {
            initialization: 1,
            ..Plan::default()
        }],
    );
    request(&host, &recording).unwrap();
    assert_eq!(recording.events(), success_events());
    recording.assert_worker_confined();
}

#[test]
fn failed_strict_sta_does_not_uninitialize_or_toggle_and_later_intent_retries() {
    let recording = Recording::default();
    let host = host(
        &recording,
        vec![
            Plan {
                initialization: 0x8001_0106u32 as i32,
                ..Plan::default()
            },
            Plan::default(),
        ],
    );
    let error = request(&host, &recording).unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Other);
    assert_eq!(error.message, "Dock utility COM initialization: 0x80010106");
    let mut expected = vec![Event::Initialize(2), Event::DriverDrop, Event::Completion];
    assert_eq!(recording.events(), expected);
    request(&host, &recording).unwrap();
    expected.extend(success_events());
    assert_eq!(recording.events(), expected);
    recording.assert_worker_confined();
}

#[test]
fn interface_failure_releases_apartment_without_toggling() {
    let recording = Recording::default();
    let host = host(
        &recording,
        vec![Plan {
            initialization: 1,
            activation_error: Some(0x8000_4002),
            ..Plan::default()
        }],
    );
    let error = request(&host, &recording).unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Unsupported);
    assert_eq!(error.message, "Dock utility Shell activation: 0x80004002");
    assert_eq!(
        recording.events(),
        vec![
            Event::Initialize(2),
            Event::Activate(SHELL_CLSID, 0x8001),
            Event::Uninitialize,
            Event::DriverDrop,
            Event::Completion,
        ]
    );
    recording.assert_worker_confined();
}

#[test]
fn accepted_toggle_error_completes_once_and_never_double_toggles() {
    let recording = Recording::default();
    let host = host(
        &recording,
        vec![Plan {
            initialization: 1,
            toggle_error: Some(0x8001_0108),
            ..Plan::default()
        }],
    );
    let error = request(&host, &recording).unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Unavailable);
    assert_eq!(error.message, "Dock utility ToggleDesktop: 0x80010108");
    assert_eq!(recording.events(), success_events());
    recording.assert_worker_confined();
}

#[test]
fn activation_and_toggle_panics_unwind_request_resources_before_completion() {
    for activation in [true, false] {
        let recording = Recording::default();
        let host = host(
            &recording,
            vec![Plan {
                initialization: 1,
                panic_activation: activation,
                panic_toggle: !activation,
                ..Plan::default()
            }],
        );
        let error = request(&host, &recording).unwrap_err();
        assert_eq!(error.kind, DockUtilityErrorKind::Other);
        assert_eq!(error.message, "Dock utility worker: 0x80004005");
        let mut expected = success_events();
        if activation {
            expected.retain(|event| !matches!(event, Event::Toggle | Event::Release));
        }
        assert_eq!(recording.events(), expected);
        recording.assert_worker_confined();
    }
}
