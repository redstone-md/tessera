// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread::{ThreadId, current};
use std::time::Duration;

use parking_lot::Mutex;
use tessera_system::dock_utilities::DockUtilityErrorKind;
use tessera_system::recycle_bin::{
    RecycleBinHost, RecycleBinWatchCallback, RecycleBinWatchCompletion, RecycleBinWatchGuard,
};

use super::*;
use crate::recycle_bin::worker;

#[derive(Clone, Copy)]
struct Plan {
    initialization: i32,
    signed: SignedInfo,
    query_error: Option<u32>,
    activation_error: Option<u32>,
    open_error: Option<u32>,
    panic_query: bool,
    panic_target: bool,
    panic_open: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            initialization: 0,
            signed: SignedInfo {
                item_count: 1,
                size_in_bytes: 0,
            },
            query_error: None,
            activation_error: None,
            open_error: None,
            panic_query: false,
            panic_target: false,
            panic_open: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    Initialize(i32),
    Query,
    Activate(u128, u32),
    Target(i32, u16),
    Open,
    ClearTarget,
    ReleaseShell,
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
    fn assert_worker(&self) {
        let log = self.0.lock();
        let owner = log.first().unwrap().0;
        assert_ne!(owner, current().id());
        assert!(log.iter().all(|(thread, _)| *thread == owner));
    }
}

struct RecordingCalls {
    log: Recording,
    plan: Plan,
    _thread_bound: Rc<()>,
}
impl Drop for RecordingCalls {
    fn drop(&mut self) {
        self.log.push(Event::DriverDrop);
    }
}
struct RecordingShell {
    log: Recording,
    _thread_bound: Rc<()>,
}
impl Drop for RecordingShell {
    fn drop(&mut self) {
        self.log.push(Event::ReleaseShell);
    }
}
struct RecordingTarget {
    log: Recording,
    value: i32,
    vartype: u16,
    _thread_bound: Rc<()>,
}
impl Drop for RecordingTarget {
    fn drop(&mut self) {
        self.log.push(Event::ClearTarget);
    }
}

impl Calls for RecordingCalls {
    type Shell = RecordingShell;
    type Target = RecordingTarget;

    fn initialize(&self, model: i32) -> i32 {
        self.log.push(Event::Initialize(model));
        self.plan.initialization
    }
    fn query(&self) -> Result<SignedInfo, u32> {
        self.log.push(Event::Query);
        assert!(!self.plan.panic_query, "recording query panic");
        self.plan.query_error.map_or(Ok(self.plan.signed), Err)
    }
    fn create_shell(&self, class: u128, context: u32) -> Result<Self::Shell, u32> {
        self.log.push(Event::Activate(class, context));
        if let Some(code) = self.plan.activation_error {
            return Err(code);
        }
        Ok(RecordingShell {
            log: self.log.clone(),
            _thread_bound: Rc::new(()),
        })
    }
    fn make_target(&self, value: i32) -> Self::Target {
        self.log.push(Event::Target(value, 3));
        assert!(!self.plan.panic_target, "recording target panic");
        RecordingTarget {
            log: self.log.clone(),
            value,
            vartype: 3,
            _thread_bound: Rc::new(()),
        }
    }
    fn open(&self, _: &Self::Shell, target: &Self::Target) -> Result<(), u32> {
        assert_eq!((target.value, target.vartype), (10, 3));
        self.log.push(Event::Open);
        assert!(!self.plan.panic_open, "recording open panic");
        self.plan.open_error.map_or(Ok(()), Err)
    }
    fn uninitialize(&self) {
        self.log.push(Event::Uninitialize);
    }
}

fn unsupported_watch(
    _: RecycleBinWatchCallback,
    _: RecycleBinWatchCompletion,
) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError> {
    Err(native_error(0x8000_4001, "Recording watch"))
}

fn host(log: &Recording, plans: Vec<Plan>) -> Arc<dyn RecycleBinHost> {
    let log = log.clone();
    let mut plans = plans.into_iter();
    worker::start(
        move || {
            Ok(RecycleBinDriver {
                calls: RecordingCalls {
                    log: log.clone(),
                    plan: plans.next().expect("unexpected retry"),
                    _thread_bound: Rc::new(()),
                },
            })
        },
        unsupported_watch,
    )
    .unwrap()
}

fn read(
    host: &Arc<dyn RecycleBinHost>,
    log: &Recording,
) -> Result<RecycleBinInfo, DockUtilityError> {
    let log = log.clone();
    let (done, result) = channel();
    host.read(Box::new(move |completion| {
        log.push(Event::Completion);
        done.send(completion).unwrap();
    }))
    .unwrap();
    let value = result.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        result.recv_timeout(Duration::from_secs(5)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    value
}

fn open(host: &Arc<dyn RecycleBinHost>, log: &Recording) -> Result<(), DockUtilityError> {
    let log = log.clone();
    let (done, result) = channel();
    host.open(Box::new(move |completion| {
        log.push(Event::Completion);
        done.send(completion).unwrap();
    }))
    .unwrap();
    let value = result.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        result.recv_timeout(Duration::from_secs(5)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
    value
}

#[test]
fn query_validates_signed_fields_independently_without_shell_activation() {
    for (count, bytes, expected) in [
        (0, 0, Some((0, 0))),
        (1, 0, Some((1, 0))),
        (i64::MAX, i64::MAX, Some((i64::MAX as u64, i64::MAX as u64))),
        (-1, 0, None),
        (0, -1, None),
        (-1, -1, None),
    ] {
        let log = Recording::default();
        let host = host(
            &log,
            vec![Plan {
                signed: SignedInfo {
                    item_count: count,
                    size_in_bytes: bytes,
                },
                ..Default::default()
            }],
        );
        match expected {
            Some((count, bytes)) => assert_eq!(
                read(&host, &log).unwrap(),
                RecycleBinInfo {
                    item_count: count,
                    size_in_bytes: bytes
                }
            ),
            None => assert_eq!(
                read(&host, &log).unwrap_err().kind,
                DockUtilityErrorKind::Other
            ),
        }
        assert_eq!(
            log.events(),
            [
                Event::Initialize(2),
                Event::Query,
                Event::Uninitialize,
                Event::DriverDrop,
                Event::Completion
            ]
        );
        log.assert_worker();
    }
}

#[test]
fn failed_query_never_uses_provider_output_and_unwind_releases_sta() {
    for plan in [
        Plan {
            query_error: Some(0x8007_0005),
            signed: SignedInfo {
                item_count: -1,
                size_in_bytes: -1,
            },
            ..Default::default()
        },
        Plan {
            panic_query: true,
            ..Default::default()
        },
    ] {
        let log = Recording::default();
        let expected = if plan.panic_query {
            DockUtilityErrorKind::Other
        } else {
            DockUtilityErrorKind::AccessDenied
        };
        let host = host(&log, vec![plan]);
        assert_eq!(read(&host, &log).unwrap_err().kind, expected);
        assert_eq!(
            log.events(),
            [
                Event::Initialize(2),
                Event::Query,
                Event::Uninitialize,
                Event::DriverDrop,
                Event::Completion
            ]
        );
    }
}

#[test]
fn base_shell_fixed_i32_target_reverse_drops_and_sta_balance_for_s_ok_s_false() {
    let log = Recording::default();
    let host = host(
        &log,
        vec![
            Plan::default(),
            Plan {
                initialization: 1,
                ..Default::default()
            },
        ],
    );
    open(&host, &log).unwrap();
    // The first completion is before any second request; idle owns no apartment.
    let one = vec![
        Event::Initialize(2),
        Event::Activate(SHELL_CLSID, 0x8001),
        Event::Target(10, 3),
        Event::Open,
        Event::ClearTarget,
        Event::ReleaseShell,
        Event::Uninitialize,
        Event::DriverDrop,
        Event::Completion,
    ];
    assert_eq!(log.events(), one);
    open(&host, &log).unwrap();
    assert_eq!(
        log.events(),
        one.iter().chain(one.iter()).copied().collect::<Vec<_>>()
    );
    log.assert_worker();
}

#[test]
fn changed_mode_never_activates_queries_or_uninitializes() {
    for reading in [true, false] {
        let log = Recording::default();
        let host = host(
            &log,
            vec![Plan {
                initialization: 0x8001_0106_u32 as i32,
                ..Default::default()
            }],
        );
        let kind = if reading {
            read(&host, &log).unwrap_err().kind
        } else {
            open(&host, &log).unwrap_err().kind
        };
        assert_eq!(kind, DockUtilityErrorKind::Other);
        assert_eq!(
            log.events(),
            [Event::Initialize(2), Event::DriverDrop, Event::Completion]
        );
    }
}

#[test]
fn activation_failure_does_not_construct_target_or_open() {
    let log = Recording::default();
    let host = host(
        &log,
        vec![Plan {
            activation_error: Some(0x8004_0154),
            ..Default::default()
        }],
    );
    assert_eq!(
        open(&host, &log).unwrap_err().kind,
        DockUtilityErrorKind::Unsupported
    );
    assert_eq!(
        log.events(),
        [
            Event::Initialize(2),
            Event::Activate(SHELL_CLSID, SHELL_CONTEXT),
            Event::Uninitialize,
            Event::DriverDrop,
            Event::Completion
        ]
    );
}

#[test]
fn open_error_and_panic_have_one_dispatch_no_fallback_and_reverse_release() {
    for plan in [
        Plan {
            open_error: Some(0x8007_0005),
            ..Default::default()
        },
        Plan {
            panic_open: true,
            ..Default::default()
        },
    ] {
        let log = Recording::default();
        let host = host(&log, vec![plan]);
        assert_eq!(
            open(&host, &log).unwrap_err().kind,
            if plan.panic_open {
                DockUtilityErrorKind::Other
            } else {
                DockUtilityErrorKind::AccessDenied
            }
        );
        assert_eq!(
            log.events(),
            [
                Event::Initialize(2),
                Event::Activate(SHELL_CLSID, SHELL_CONTEXT),
                Event::Target(10, 3),
                Event::Open,
                Event::ClearTarget,
                Event::ReleaseShell,
                Event::Uninitialize,
                Event::DriverDrop,
                Event::Completion
            ]
        );
    }
}

#[test]
fn target_construction_panic_releases_shell_before_apartment() {
    let log = Recording::default();
    let host = host(
        &log,
        vec![Plan {
            panic_target: true,
            ..Default::default()
        }],
    );
    assert_eq!(
        open(&host, &log).unwrap_err().kind,
        DockUtilityErrorKind::Other
    );
    assert_eq!(
        log.events(),
        [
            Event::Initialize(2),
            Event::Activate(SHELL_CLSID, SHELL_CONTEXT),
            Event::Target(10, 3),
            Event::ReleaseShell,
            Event::Uninitialize,
            Event::DriverDrop,
            Event::Completion
        ]
    );
}

#[cfg(windows)]
#[test]
fn sdk_query_uses_architecture_layout_initialized_header_and_null_root() {
    use windows::Win32::UI::Shell::SHQUERYRBINFO;
    let expected_size = if cfg!(target_arch = "x86") { 20 } else { 24 };
    assert_eq!(std::mem::size_of::<SHQUERYRBINFO>(), expected_size);
    let info = sdk::query_aggregate(|root, info| {
        assert!(root.is_null());
        let (size, count, bytes) = (info.cbSize, info.i64NumItems, info.i64Size);
        assert_eq!((size, count, bytes), (expected_size as u32, 0, 0));
        info.i64NumItems = i64::MAX;
        info.i64Size = 0;
        Ok(())
    })
    .unwrap();
    assert_eq!((info.item_count, info.size_in_bytes), (i64::MAX, 0));
    assert!(
        sdk::query_aggregate(|_, info| {
            info.i64NumItems = -1;
            info.i64Size = -1;
            Err(0x8007_0005)
        })
        .is_err()
    );
}

#[cfg(windows)]
#[test]
fn sdk_binding_pins_base_iid_fixed_activation_and_inline_vt_i4() {
    use windows::Win32::System::Com::{
        CLSCTX_DISABLE_AAA, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::{VARIANT, VT_I4};
    use windows::Win32::UI::Shell::{IShellDispatch, Shell, ssfBITBUCKET};
    use windows::core::{GUID, Interface};
    assert_eq!(Shell, GUID::from_u128(SHELL_CLSID));
    assert_eq!(IShellDispatch::IID, GUID::from_u128(SHELL_INTERFACE_IID));
    assert_eq!((CLSCTX_INPROC_SERVER | CLSCTX_DISABLE_AAA).0, SHELL_CONTEXT);
    assert_eq!(COINIT_APARTMENTTHREADED.0, STA_MODEL);
    assert_eq!(ssfBITBUCKET.0, RECYCLE_TARGET);
    // Pure layout fixture: integer owns no resources. Suppress VariantClear so
    // this test makes no native calls; production keeps normal owned Drop.
    let target = std::mem::ManuallyDrop::new(VARIANT::from(ssfBITBUCKET.0));
    assert_eq!(target.vt(), VT_I4);
    // SAFETY: constructed from i32, so only the active VT_I4 scalar is inspected.
    let (value, reserved) = unsafe {
        (
            target.Anonymous.Anonymous.Anonymous.lVal,
            [
                target.Anonymous.Anonymous.wReserved1,
                target.Anonymous.Anonymous.wReserved2,
                target.Anonymous.Anonymous.wReserved3,
            ],
        )
    };
    assert_eq!(value, 10);
    assert_eq!(reserved, [0; 3]);
}
