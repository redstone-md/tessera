// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use parking_lot::Mutex;
use std::sync::Arc;
use tessera_system::dock_utilities::DockUtilityErrorKind;

#[derive(Clone, Copy)]
enum Setup {
    Ready,
    ClassFailure,
    OwnerFailure,
    EmptyPanic,
}

struct RecordingCalls {
    events: Arc<Mutex<Vec<&'static str>>>,
    setup: Setup,
    initialize_status: i32,
    empty_status: i32,
    creator: std::thread::ThreadId,
    _not_send: Rc<()>,
}

impl RecordingCalls {
    fn record(&self, event: &'static str) {
        assert_eq!(std::thread::current().id(), self.creator);
        self.events.lock().push(event);
    }
}

impl Calls for RecordingCalls {
    type Class = u8;
    type Owner = u8;

    fn initialize(&self, model: i32) -> i32 {
        self.record("initialize");
        assert_eq!(model, 2);
        self.initialize_status
    }

    fn uninitialize(&self) {
        self.record("uninitialize");
    }

    fn register_class(&self) -> Result<Self::Class, DockUtilityError> {
        self.record("register");
        if matches!(self.setup, Setup::ClassFailure) {
            Err(native_error(0x8007_0005, "recorded owner class failure"))
        } else {
            Ok(1)
        }
    }

    fn unregister_class(&self, class: &Self::Class) {
        assert_eq!(*class, 1);
        self.record("unregister");
    }

    fn create_owner(
        &self,
        class: &Self::Class,
        style: u32,
    ) -> Result<Self::Owner, DockUtilityError> {
        self.record("create-owner");
        assert_eq!(*class, 1);
        assert_eq!(style, 0x8000_0000); // Normal top-level WS_POPUP only.
        if matches!(self.setup, Setup::OwnerFailure) {
            Err(native_error(0x8000_4005, "recorded owner failure"))
        } else {
            Ok(2)
        }
    }

    fn destroy_owner(&self, owner: &Self::Owner) {
        assert_eq!(*owner, 2);
        self.record("destroy-owner");
    }

    fn empty(&self, owner: &Self::Owner, root: *const u16, flags: u32) -> i32 {
        self.record("empty");
        assert_eq!(*owner, 2);
        assert!(root.is_null()); // Aggregate, never a drive supplied by a caller.
        assert_eq!(flags, 4); // Sound only, retain confirmation and progress.
        if matches!(self.setup, Setup::EmptyPanic) {
            panic!("recorded native driver panic");
        }
        self.empty_status
    }
}

impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.record("driver-drop");
    }
}

fn recording_driver(
    events: Arc<Mutex<Vec<&'static str>>>,
    setup: Setup,
    initialize_status: i32,
    empty_status: i32,
) -> RecycleBinMutationDriver<RecordingCalls> {
    RecycleBinMutationDriver {
        calls: RecordingCalls {
            events,
            setup,
            initialize_status,
            empty_status,
            creator: std::thread::current().id(),
            _not_send: Rc::new(()),
        },
        _thread_bound: PhantomData,
    }
}

#[test]
fn raw_hresult_zero_positive_and_negative_preserved_with_fixed_aggregate_call() {
    for status in [0, 1, i32::MAX, -1, 0x8007_0005_u32 as i32] {
        for initialize in [0, 1] {
            // S_OK and S_FALSE both require balancing.
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut driver = recording_driver(events.clone(), Setup::Ready, initialize, status);
            let outcome = driver.empty().unwrap();
            assert_eq!(outcome.native_hresult, status);
            assert_eq!(
                *events.lock(),
                [
                    "initialize",
                    "register",
                    "create-owner",
                    "empty",
                    "destroy-owner",
                    "unregister",
                    "uninitialize",
                ]
            );
            drop(driver);
            assert_eq!(events.lock().last(), Some(&"driver-drop"));
        }
    }
}

#[test]
fn changed_mode_and_failed_sta_never_register_owner_or_enter_empty() {
    for status in [0x8001_0106_u32 as i32, 0x8007_0005_u32 as i32] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut driver = recording_driver(events.clone(), Setup::Ready, status, 0);
        let error = driver.empty().unwrap_err();
        assert_eq!(
            error.kind,
            if status == 0x8007_0005_u32 as i32 {
                DockUtilityErrorKind::AccessDenied
            } else {
                DockUtilityErrorKind::Other
            }
        );
        assert_eq!(*events.lock(), ["initialize"]);
        drop(driver);
        assert_eq!(*events.lock(), ["initialize", "driver-drop"]);
    }
}

#[test]
fn class_and_owner_setup_failures_release_only_acquired_resources_without_empty() {
    for setup in [Setup::ClassFailure, Setup::OwnerFailure] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut driver = recording_driver(events.clone(), setup, 1, 0);
        assert!(driver.empty().is_err());
        let expected: &[&str] = if matches!(setup, Setup::ClassFailure) {
            &["initialize", "register", "uninitialize"]
        } else {
            &[
                "initialize",
                "register",
                "create-owner",
                "unregister",
                "uninitialize",
            ]
        };
        assert_eq!(&*events.lock(), expected);
        drop(driver);
        assert!(!events.lock().contains(&"empty"));
    }
}

#[test]
fn actual_request_worker_releases_owner_class_sta_and_non_send_driver_before_completion() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let caller = std::thread::current().id();
    let host = crate::recycle_bin_mutation::worker::host({
        let events = events.clone();
        move || {
            assert_ne!(std::thread::current().id(), caller);
            Ok(recording_driver(events.clone(), Setup::Ready, 0, 1))
        }
    });
    assert!(events.lock().is_empty()); // Native factory performs no initialization.
    let (sender, receiver) = std::sync::mpsc::channel();
    host.empty(Box::new({
        let events = events.clone();
        move |result| {
            events.lock().push("completion");
            sender.send(result).unwrap();
        }
    }))
    .unwrap();
    drop(host);
    assert_eq!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .native_hresult,
        1
    );
    assert_eq!(
        *events.lock(),
        [
            "initialize",
            "register",
            "create-owner",
            "empty",
            "destroy-owner",
            "unregister",
            "uninitialize",
            "driver-drop",
            "completion",
        ]
    );
}

#[test]
fn entered_driver_panic_unwinds_owner_class_sta_before_exact_one_error_completion() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let host = crate::recycle_bin_mutation::worker::host({
        let events = events.clone();
        move || Ok(recording_driver(events.clone(), Setup::EmptyPanic, 0, 0))
    });
    let (sender, receiver) = std::sync::mpsc::channel();
    host.empty(Box::new({
        let events = events.clone();
        move |result| {
            events.lock().push("completion");
            sender.send(result).unwrap();
        }
    }))
    .unwrap();
    let error = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, DockUtilityErrorKind::Other);
    assert_eq!(
        *events.lock(),
        [
            "initialize",
            "register",
            "create-owner",
            "empty",
            "destroy-owner",
            "unregister",
            "uninitialize",
            "driver-drop",
            "completion",
        ]
    );
    assert!(receiver.try_recv().is_err());
}

#[test]
fn owner_close_filter_masks_system_command_low_bits_and_defaults_other_messages() {
    assert!(suppress_owner_close(0x0010, 0));
    assert!(suppress_owner_close(0x0010, usize::MAX));
    for low_bits in 0..16 {
        assert!(suppress_owner_close(0x0112, 0xF060 | low_bits));
    }
    for (message, wparam) in [
        (0x0001, 0),
        (0x0081, 0),
        (0x0112, 0xF020),
        (0x0112, 0xF030),
        (0x0002, 0xF060),
    ] {
        assert!(!suppress_owner_close(message, wparam));
    }
}
