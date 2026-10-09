// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! SDK-layout recordings only: these tests allocate Rust memory, never WLAN/COM/Shell.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::mem::offset_of;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use tessera_system::network::{NetworkError, NetworkErrorKind, Observation, RadioState, WifiBss};
use windows::Win32::NetworkManagement::WiFi::*;
use windows::core::GUID;

use super::callback::Context;
use super::calls::{NativeCalls, Reply};
use super::owner::Owner;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Interfaces,
    Radio,
    Available,
    Bss,
    Current,
}

struct Step {
    stage: Stage,
    status: u32,
    pointer: usize,
    size: usize,
}

struct State {
    steps: VecDeque<Step>,
    allocations: HashMap<usize, Layout>,
    events: Vec<String>,
    freed: Vec<usize>,
    open: (u32, u32, usize),
    close: u32,
    register_status: u32,
    unregister_status: VecDeque<u32>,
    callback: WLAN_NOTIFICATION_CALLBACK,
    context: usize,
    settings_error: Option<NetworkError>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            steps: VecDeque::new(),
            allocations: HashMap::new(),
            events: Vec::new(),
            freed: Vec::new(),
            open: (0, 2, 41),
            close: 0,
            register_status: 0,
            unregister_status: VecDeque::new(),
            callback: None,
            context: 0,
            settings_error: None,
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        for (pointer, layout) in self.allocations.drain() {
            // SAFETY: test-only leftovers still have their original allocator layout.
            unsafe { dealloc(pointer as *mut u8, layout) };
        }
    }
}

#[derive(Clone, Default)]
struct Recording(Arc<Mutex<State>>);

impl Recording {
    fn stage(&self, stage: Stage, status: u32, size: usize, fill: impl FnOnce(*mut u8)) {
        let layout = Layout::from_size_align(size.max(1), 16).unwrap();
        // SAFETY: valid bounded test allocation; every pointer is tracked until free.
        let pointer = unsafe { alloc_zeroed(layout) };
        assert!(!pointer.is_null());
        fill(pointer);
        let mut state = self.0.lock();
        state.allocations.insert(pointer as usize, layout);
        state.steps.push_back(Step {
            stage,
            status,
            pointer: pointer as usize,
            size,
        });
    }

    fn list<T: Copy>(&self, stage: Stage, status: u32, offset: usize, entries: &[T]) {
        let size = offset + std::mem::size_of_val(entries);
        self.stage(stage, status, size, |pointer| {
            // SAFETY: header and entry storage have checked test allocation sizes.
            unsafe {
                if stage == Stage::Bss {
                    pointer.cast::<u32>().write(size as u32);
                    pointer.add(4).cast::<u32>().write(entries.len() as u32);
                } else {
                    pointer.cast::<u32>().write(entries.len() as u32);
                }
                for (index, entry) in entries.iter().enumerate() {
                    pointer
                        .add(offset + index * size_of::<T>())
                        .cast::<T>()
                        .write_unaligned(*entry);
                }
            }
        });
    }

    fn value<T: Copy>(&self, stage: Stage, status: u32, value: T) {
        self.stage(stage, status, size_of::<T>(), |pointer| {
            // SAFETY: appropriately sized initialized recording buffer.
            unsafe { pointer.cast::<T>().write_unaligned(value) };
        });
    }

    fn next(&self, stage: Stage, id: Option<&GUID>) -> Reply {
        let mut state = self.0.lock();
        let step = state.steps.pop_front().expect("unexpected native stage");
        assert_eq!(step.stage, stage);
        state
            .events
            .push(format!("{stage:?}:{}", id.map_or(0, GUID::to_u128)));
        Reply {
            status: step.status,
            data: step.pointer as *mut c_void,
            size: Some(step.size),
        }
    }

    fn interfaces(&self, entries: &[WLAN_INTERFACE_INFO]) {
        self.list(
            Stage::Interfaces,
            0,
            offset_of!(WLAN_INTERFACE_INFO_LIST, InterfaceInfo),
            entries,
        );
    }

    fn adapter(
        &self,
        statuses: [u32; 4],
        entries: &[WLAN_BSS_ENTRY],
        metadata: &[WLAN_AVAILABLE_NETWORK],
    ) {
        self.value(
            Stage::Radio,
            statuses[0],
            radio(dot11_radio_state_on, dot11_radio_state_on),
        );
        self.list(
            Stage::Available,
            statuses[1],
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            metadata,
        );
        if statuses[1] == 5 {
            return;
        }
        self.list(
            Stage::Bss,
            statuses[2],
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            entries,
        );
        if statuses[2] != 0 || entries.is_empty() {
            return;
        }
        let mut current = WLAN_CONNECTION_ATTRIBUTES {
            isState: wlan_interface_state_connected,
            ..Default::default()
        };
        current.wlanAssociationAttributes.dot11Ssid = entries[0].dot11Ssid;
        current.wlanAssociationAttributes.dot11Bssid = entries[0].dot11Bssid;
        current.wlanAssociationAttributes.dot11BssType = entries[0].dot11BssType;
        self.value(Stage::Current, statuses[3], current);
    }

    fn notify(&self, source: u32, code: u32) {
        let (callback, context) = {
            let state = self.0.lock();
            (state.callback, state.context)
        };
        if let Some(callback) = callback {
            let mut header = L2_NOTIFICATION_DATA {
                NotificationSource: WLAN_NOTIFICATION_SOURCES(source),
                NotificationCode: code,
                dwDataSize: u32::MAX,
                pData: usize::MAX as *mut c_void,
                ..Default::default()
            };
            // SAFETY: owner retains this test Context until recorded NONE barrier;
            // hostile pData is intentionally not dereferenced by the callback.
            unsafe { callback(&mut header, context as *mut c_void) };
        }
    }

    fn settled(&self) {
        let state = self.0.lock();
        assert!(state.steps.is_empty());
        assert!(state.allocations.is_empty());
        let mut freed = state.freed.clone();
        freed.sort_unstable();
        freed.dedup();
        assert_eq!(
            freed.len(),
            state.freed.len(),
            "allocation freed more than once"
        );
        assert_eq!(
            state
                .events
                .iter()
                .filter(|event| event.as_str() == "close:41")
                .count(),
            1
        );
    }
}

// SAFETY: the recording owns typed initialized allocations until free and models
// successful NONE as a callback barrier. All callbacks are synchronous test calls.
unsafe impl NativeCalls for Recording {
    fn open(&self, version: u32) -> (u32, u32, usize) {
        assert_eq!(version, 2);
        let mut state = self.0.lock();
        state.events.push(format!("open:{version}"));
        state.open
    }
    fn close(&self, handle: usize) -> u32 {
        let mut state = self.0.lock();
        state.events.push(format!("close:{handle}"));
        state.close
    }
    fn interfaces(&self, handle: usize) -> Reply {
        assert_eq!(handle, 41);
        self.next(Stage::Interfaces, None)
    }
    fn query(&self, handle: usize, id: &GUID, opcode: WLAN_INTF_OPCODE) -> Reply {
        assert_eq!(handle, 41);
        let stage = if opcode == wlan_intf_opcode_radio_state {
            Stage::Radio
        } else {
            assert_eq!(opcode, wlan_intf_opcode_current_connection);
            Stage::Current
        };
        self.next(stage, Some(id))
    }
    fn available(&self, handle: usize, id: &GUID, flags: u32) -> Reply {
        assert_eq!(handle, 41);
        assert_eq!(flags, 0);
        self.next(Stage::Available, Some(id))
    }
    fn bss(&self, handle: usize, id: &GUID) -> Reply {
        assert_eq!(handle, 41);
        self.next(Stage::Bss, Some(id))
    }
    unsafe fn free(&self, pointer: *mut c_void) {
        let mut state = self.0.lock();
        let layout = state
            .allocations
            .remove(&(pointer as usize))
            .expect("foreign or duplicate free");
        state.freed.push(pointer as usize);
        state.events.push("free".into());
        // SAFETY: exact test allocation ownership and original layout transferred.
        unsafe { dealloc(pointer.cast::<u8>(), layout) };
    }
    fn register(
        &self,
        handle: usize,
        source: u32,
        callback: WLAN_NOTIFICATION_CALLBACK,
        context: *const c_void,
    ) -> u32 {
        assert_eq!(handle, 41);
        if source == WLAN_NOTIFICATION_SOURCE_ACM.0 {
            assert!(callback.is_some());
            assert!(!context.is_null());
            let mut state = self.0.lock();
            state.events.push("ACM".into());
            state.callback = callback;
            state.context = context as usize;
            state.register_status
        } else {
            assert_eq!(source, WLAN_NOTIFICATION_SOURCE_NONE.0);
            assert!(callback.is_none());
            assert!(context.is_null());
            self.notify(
                WLAN_NOTIFICATION_SOURCE_ACM.0,
                wlan_notification_acm_scan_list_refresh.0 as u32,
            );
            let mut state = self.0.lock();
            state.events.push("NONE".into());
            let status = state.unregister_status.pop_front().unwrap_or(0);
            if status == 0 {
                state.callback = None;
                state.context = 0;
            }
            status
        }
    }
    fn open_settings(&self) -> Result<(), NetworkError> {
        let mut state = self.0.lock();
        state.events.push("settings".into());
        state.settings_error.clone().map_or(Ok(()), Err)
    }
    fn retirement_error(&self, error: &NetworkError) {
        self.0
            .lock()
            .events
            .push(format!("retirement:{:?}", error.kind));
    }
}

fn interface(id: u128) -> WLAN_INTERFACE_INFO {
    WLAN_INTERFACE_INFO {
        InterfaceGuid: GUID::from_u128(id),
        isState: wlan_interface_state_connected,
        ..Default::default()
    }
}

fn raw_ssid(bytes: &[u8]) -> DOT11_SSID {
    let mut raw = DOT11_SSID {
        uSSIDLength: bytes.len() as u32,
        ..Default::default()
    };
    raw.ucSSID[..bytes.len()].copy_from_slice(bytes);
    raw
}

fn bss(bytes: &[u8], bssid: u8, secured: bool) -> WLAN_BSS_ENTRY {
    WLAN_BSS_ENTRY {
        dot11Ssid: raw_ssid(bytes),
        dot11Bssid: [bssid; 6],
        dot11BssType: dot11_BSS_type_infrastructure,
        uLinkQuality: 85,
        lRssi: -53,
        ulChCenterFrequency: 5_180_000,
        usCapabilityInformation: if secured { 0x10 } else { 0 },
        ..Default::default()
    }
}

fn metadata(bytes: &[u8], known: bool, secured: bool) -> WLAN_AVAILABLE_NETWORK {
    WLAN_AVAILABLE_NETWORK {
        dot11Ssid: raw_ssid(bytes),
        dot11BssType: dot11_BSS_type_infrastructure,
        bSecurityEnabled: secured.into(),
        dwFlags: if known {
            WLAN_AVAILABLE_NETWORK_HAS_PROFILE
        } else {
            0
        },
        dot11DefaultAuthAlgorithm: if secured {
            DOT11_AUTH_ALGO_RSNA_PSK
        } else {
            DOT11_AUTH_ALGO_80211_OPEN
        },
        ..Default::default()
    }
}

fn radio(software: DOT11_RADIO_STATE, hardware: DOT11_RADIO_STATE) -> WLAN_RADIO_STATE {
    let mut raw = WLAN_RADIO_STATE {
        dwNumberOfPhys: 1,
        ..Default::default()
    };
    raw.PhyRadioState[0] = WLAN_PHY_RADIO_STATE {
        dwPhyIndex: 0,
        dot11SoftwareRadioState: software,
        dot11HardwareRadioState: hardware,
    };
    raw
}

fn rows(snapshot: &tessera_system::network::NetworkSnapshot, index: usize) -> &[WifiBss] {
    let Observation::Ready(rows) = &snapshot.interfaces[index].discovery else {
        panic!("expected rows")
    };
    rows
}

#[test]
fn native_network_owner_is_pure_and_settings_does_not_acquire_wlan() {
    let calls = Recording::default();
    let mut owner = Owner::new(calls.clone());
    assert!(calls.0.lock().events.is_empty());
    owner.open_settings().unwrap();
    calls.0.lock().settings_error = Some(NetworkError::new(
        NetworkErrorKind::AccessDenied,
        "dispatch denied",
    ));
    assert_eq!(
        owner.open_settings().unwrap_err().kind,
        NetworkErrorKind::AccessDenied
    );
    drop(owner);
    assert_eq!(calls.0.lock().events, ["settings", "settings"]);
}

#[test]
fn native_network_each_allocation_stage_frees_once_and_keeps_partial_observations() {
    for failed in [
        Stage::Interfaces,
        Stage::Radio,
        Stage::Available,
        Stage::Bss,
        Stage::Current,
    ] {
        let calls = Recording::default();
        let entry = bss(b"raw", 1, true);
        if failed == Stage::Interfaces {
            calls.list(
                Stage::Interfaces,
                1722,
                offset_of!(WLAN_INTERFACE_INFO_LIST, InterfaceInfo),
                &[interface(1)],
            );
        } else {
            calls.interfaces(&[interface(1)]);
            calls.adapter(
                [
                    if failed == Stage::Radio { 1722 } else { 0 },
                    if failed == Stage::Available { 1722 } else { 0 },
                    if failed == Stage::Bss { 1722 } else { 0 },
                    if failed == Stage::Current { 5 } else { 0 },
                ],
                &[entry],
                &[metadata(b"raw", true, true)],
            );
        }
        let mut owner = Owner::new(calls.clone());
        let result = owner.read();
        if failed == Stage::Interfaces {
            assert_eq!(
                result.unwrap_err().kind,
                NetworkErrorKind::ServiceUnavailable
            );
        } else {
            let snapshot = result.unwrap();
            if failed == Stage::Radio {
                assert!(matches!(
                    snapshot.interfaces[0].radio,
                    Observation::Unavailable(_)
                ));
            }
            if failed == Stage::Bss {
                assert!(matches!(
                    snapshot.interfaces[0].discovery,
                    Observation::Unavailable(_)
                ));
            } else {
                let row = &rows(&snapshot, 0)[0];
                if failed == Stage::Available {
                    assert_eq!((row.known, row.secured, &row.auth), (None, None, &None));
                }
                if failed == Stage::Current {
                    assert_eq!(row.connected, None);
                }
            }
        }
        drop(owner);
        calls.settled();
    }
}

#[test]
fn native_network_open_failures_close_anomalous_handles_exactly_once() {
    for open in [(5, 0, 0), (1722, 0, 41), (0, 1, 41), (0, 2, 0)] {
        let calls = Recording::default();
        calls.0.lock().open = open;
        let mut owner = Owner::new(calls.clone());
        assert!(owner.read().is_err());
        drop(owner);
        assert_eq!(
            calls
                .0
                .lock()
                .events
                .iter()
                .filter(|event| event.starts_with("close:"))
                .count(),
            usize::from(open.2 != 0)
        );
    }
}

#[test]
fn native_network_raw_identity_security_collision_and_exact_bssid_join() {
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    let entries = [
        bss(&[0xff, 0], 1, true),
        bss(&[0xff, 0], 2, true),
        bss(&[0xff, 0], 3, false),
        bss(&[0xfe, 0], 4, true),
        bss(b"", 5, true),
    ];
    let mut contradictory = metadata(&[0xff, 0], false, true);
    contradictory.dot11DefaultAuthAlgorithm = DOT11_AUTH_ALGO_WPA_PSK;
    let mut wrong_type = metadata(&[0xfe, 0], true, true);
    wrong_type.dot11BssType = dot11_BSS_type_independent;
    calls.adapter(
        [0; 4],
        &entries,
        &[
            metadata(&[0xff, 0], true, true),
            contradictory,
            metadata(&[0xff, 0], false, false),
            wrong_type,
            metadata(b"", true, true),
        ],
    );
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    let rows = rows(&snapshot, 0);
    assert_eq!(rows[0].ssid.as_bytes(), &[0xff, 0]);
    assert_eq!(
        (rows[0].connected, rows[1].connected),
        (Some(true), Some(false))
    );
    assert_eq!((rows[0].known, &rows[0].auth), (None, &None));
    assert_eq!((rows[2].known, rows[2].secured), (Some(false), Some(false)));
    assert_eq!((rows[3].known, &rows[3].auth), (None, &None));
    assert_eq!((rows[4].known, &rows[4].auth), (None, &None));
    assert_eq!(
        (
            rows[0].frequency_khz,
            rows[0].signal_percent,
            rows[0].rssi_dbm
        ),
        (5_180_000, 85, -53)
    );
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_privacy_denial_is_local_and_does_not_retry_sensitive_calls() {
    let calls = Recording::default();
    calls.interfaces(&[interface(2), interface(1)]);
    calls.adapter([0, 5, 0, 0], &[bss(b"private", 2, true)], &[]);
    calls.adapter(
        [0; 4],
        &[bss(b"other", 1, false)],
        &[metadata(b"other", false, false)],
    );
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    assert_eq!(snapshot.interfaces[0].id.as_bytes(), &1_u128.to_be_bytes());
    assert_eq!(rows(&snapshot, 0).len(), 1);
    assert!(
        matches!(&snapshot.interfaces[1].discovery, Observation::Unavailable(error) if error.kind == NetworkErrorKind::AccessDenied)
    );
    let state = calls.0.lock();
    assert!(
        !state
            .events
            .iter()
            .any(|event| event == "Bss:2" || event == "Current:2")
    );
    drop(state);
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_radio_combinations_and_failed_discovery_remain_distinct() {
    for (software, hardware, expected) in [
        (
            dot11_radio_state_on,
            dot11_radio_state_on,
            RadioState::Enabled,
        ),
        (
            dot11_radio_state_on,
            dot11_radio_state_off,
            RadioState::Disabled,
        ),
        (
            dot11_radio_state_off,
            dot11_radio_state_on,
            RadioState::Disabled,
        ),
        (
            dot11_radio_state_unknown,
            dot11_radio_state_on,
            RadioState::Unknown,
        ),
    ] {
        let calls = Recording::default();
        calls.interfaces(&[interface(1)]);
        calls.value(Stage::Radio, 0, radio(software, hardware));
        calls.list::<WLAN_AVAILABLE_NETWORK>(
            Stage::Available,
            1722,
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            &[],
        );
        calls.list::<WLAN_BSS_ENTRY>(
            Stage::Bss,
            1722,
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            &[],
        );
        let mut owner = Owner::new(calls.clone());
        let snapshot = owner.read().unwrap();
        assert_eq!(snapshot.interfaces[0].radio, Observation::Ready(expected));
        assert!(matches!(
            snapshot.interfaces[0].discovery,
            Observation::Unavailable(_)
        ));
        drop(owner);
        calls.settled();
    }
}

#[test]
fn native_network_empty_interfaces_and_empty_bss_never_read_entry_zero() {
    let calls = Recording::default();
    calls.interfaces(&[]);
    let mut owner = Owner::new(calls.clone());
    assert!(owner.read().unwrap().interfaces.is_empty());
    drop(owner);
    calls.settled();
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    calls.adapter([0; 4], &[], &[]);
    let mut owner = Owner::new(calls.clone());
    assert!(rows(&owner.read().unwrap(), 0).is_empty());
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_hostile_bss_size_count_ssid_and_signal_are_rejected() {
    for case in 0..5 {
        let calls = Recording::default();
        calls.interfaces(&[interface(1)]);
        calls.value(
            Stage::Radio,
            0,
            radio(dot11_radio_state_on, dot11_radio_state_on),
        );
        calls.list::<WLAN_AVAILABLE_NETWORK>(
            Stage::Available,
            0,
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            &[],
        );
        if case < 3 {
            calls.stage(Stage::Bss, 0, 8, |pointer| {
                // SAFETY: initialized test header; count/size are deliberately hostile.
                unsafe {
                    pointer.cast::<u32>().write(if case == 0 {
                        0
                    } else if case == 1 {
                        u32::MAX
                    } else {
                        8
                    });
                    pointer
                        .add(4)
                        .cast::<u32>()
                        .write(if case == 2 { u32::MAX } else { 1 });
                }
            });
        } else {
            let mut entry = bss(b"raw", 1, true);
            if case == 3 {
                entry.dot11Ssid.uSSIDLength = 33;
            } else {
                entry.uLinkQuality = 101;
            }
            calls.adapter_current_only(&entry);
        }
        let mut owner = Owner::new(calls.clone());
        let snapshot = owner.read().unwrap();
        assert!(
            matches!(&snapshot.interfaces[0].discovery, Observation::Unavailable(error) if error.kind == NetworkErrorKind::InvalidData)
        );
        drop(owner);
        calls.settled();
    }
}

impl Recording {
    fn adapter_current_only(&self, entry: &WLAN_BSS_ENTRY) {
        self.list(
            Stage::Bss,
            0,
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            &[*entry],
        );
        // Parsing does not report malformed BSS as disconnected even if a
        // connection query is independently unavailable.
        self.value(Stage::Current, 5, WLAN_CONNECTION_ATTRIBUTES::default());
    }
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn wake(count: Arc<AtomicUsize>, dropped: Arc<AtomicUsize>) -> Arc<dyn Fn() + Send + Sync> {
    let probe = DropProbe(dropped);
    Arc::new(move || {
        let _ = &probe;
        count.fetch_add(1, Ordering::SeqCst);
    })
}

#[test]
fn native_network_watch_header_filter_and_none_barrier_precede_context_drop() {
    let calls = Recording::default();
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut owner = Owner::new(calls.clone());
    owner
        .register(wake(count.clone(), dropped.clone()))
        .unwrap();
    calls.notify(WLAN_NOTIFICATION_SOURCE_MSM.0, 26);
    calls.notify(WLAN_NOTIFICATION_SOURCE_ACM.0, 0);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    calls.notify(WLAN_NOTIFICATION_SOURCE_ACM.0, 26);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    owner.unregister().unwrap();
    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "callback during NONE must see closed admission"
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    drop(owner);
    assert_eq!(calls.0.lock().events, ["open:2", "ACM", "NONE", "close:41"]);
}

#[test]
fn native_network_callback_panics_do_not_cross_the_abi() {
    let calls = Recording::default();
    let mut owner = Owner::new(calls.clone());
    owner
        .register(Arc::new(|| panic!("recorded wake panic")))
        .unwrap();
    calls.notify(WLAN_NOTIFICATION_SOURCE_ACM.0, 26);
    owner.unregister().unwrap();
    drop(owner);
}

#[test]
fn native_network_failed_unregister_keeps_context_until_later_barrier() {
    let calls = Recording::default();
    calls.0.lock().unregister_status = VecDeque::from([1722, 0]);
    let count = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut owner = Owner::new(calls.clone());
    owner
        .register(wake(count.clone(), dropped.clone()))
        .unwrap();
    assert_eq!(
        owner.unregister().unwrap_err().kind,
        NetworkErrorKind::ServiceUnavailable
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    calls.notify(WLAN_NOTIFICATION_SOURCE_ACM.0, 26);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        owner.register(Arc::new(|| {})).unwrap_err().kind,
        NetworkErrorKind::Busy
    );
    owner.unregister().unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    drop(owner);
}

#[test]
fn native_network_failed_registration_retires_context_and_does_not_poison_reads() {
    let calls = Recording::default();
    calls.0.lock().register_status = 5;
    let dropped = Arc::new(AtomicUsize::new(0));
    let mut owner = Owner::new(calls.clone());
    assert_eq!(
        owner
            .register(wake(Arc::new(AtomicUsize::new(0)), dropped.clone()))
            .unwrap_err()
            .kind,
        NetworkErrorKind::AccessDenied
    );
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
    calls.interfaces(&[]);
    assert!(owner.read().unwrap().interfaces.is_empty());
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_unproven_retirement_retains_reachable_context_even_after_close() {
    for close_status in [0, 1722] {
        let calls = Recording::default();
        {
            let mut state = calls.0.lock();
            state.unregister_status.push_back(1722);
            state.close = close_status;
        }
        let count = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut owner = Owner::new(calls.clone());
        owner
            .register(wake(count.clone(), dropped.clone()))
            .unwrap();
        let context = calls.0.lock().context;
        drop(owner);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        calls.notify(WLAN_NOTIFICATION_SOURCE_ACM.0, 26);
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let mut state = calls.0.lock();
        assert_eq!(
            state
                .events
                .iter()
                .filter(|event| event.as_str() == "close:41")
                .count(),
            1
        );
        assert!(
            state
                .events
                .iter()
                .any(|event| event.starts_with("retirement:"))
        );
        // Test-only external barrier: no native function was called, and the fake
        // callback table is now cleared. Reclaim the intentional leak to keep the
        // test runner clean; production never assumes this artificial guarantee.
        state.callback = None;
        state.context = 0;
        drop(state);
        // SAFETY: the synchronous recording table has no remaining callback, and
        // Owner explicitly forgot this one Box after an unproven barrier.
        unsafe { drop(Box::from_raw(context as *mut Context)) };
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn native_network_radio_zero_mixed_and_malformed_phys_are_not_disabled_guesses() {
    for case in 0..4 {
        let calls = Recording::default();
        calls.interfaces(&[interface(1)]);
        let mut raw = radio(dot11_radio_state_off, dot11_radio_state_on);
        let expected = match case {
            0 => {
                raw.dwNumberOfPhys = 0;
                Some(RadioState::Unknown)
            }
            1 => {
                raw.dwNumberOfPhys = 2;
                raw.PhyRadioState[1] =
                    radio(dot11_radio_state_on, dot11_radio_state_on).PhyRadioState[0];
                Some(RadioState::Enabled)
            }
            2 => {
                raw.dwNumberOfPhys = 2;
                raw.PhyRadioState[1] =
                    radio(dot11_radio_state_unknown, dot11_radio_state_on).PhyRadioState[0];
                Some(RadioState::Unknown)
            }
            _ => {
                raw.dwNumberOfPhys = 65;
                None
            }
        };
        calls.value(Stage::Radio, 0, raw);
        calls.list::<WLAN_AVAILABLE_NETWORK>(
            Stage::Available,
            0,
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            &[],
        );
        calls.list::<WLAN_BSS_ENTRY>(
            Stage::Bss,
            0,
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            &[],
        );
        let mut owner = Owner::new(calls.clone());
        let snapshot = owner.read().unwrap();
        if let Some(expected) = expected {
            assert_eq!(snapshot.interfaces[0].radio, Observation::Ready(expected));
        } else {
            assert!(
                matches!(&snapshot.interfaces[0].radio, Observation::Unavailable(error) if error.kind == NetworkErrorKind::InvalidData)
            );
        }
        drop(owner);
        calls.settled();
    }
}

#[test]
fn native_network_buffer_overflow_truncation_and_null_success_are_rejected() {
    use super::buffer::Buffer;
    let calls = Recording::default();
    calls.value(Stage::Radio, 0, 1_u32);
    let buffer = Buffer::acquire(&calls, calls.next(Stage::Radio, None), "recording").unwrap();
    assert!(buffer.span(usize::MAX, 2, 2).is_err());
    assert!(buffer.span(0, usize::MAX, 2).is_err());
    assert!(buffer.read::<WLAN_RADIO_STATE>(0).is_err());
    drop(buffer);
    assert!(calls.0.lock().allocations.is_empty());
    let reply = Reply {
        status: 0,
        data: std::ptr::null_mut(),
        size: Some(0),
    };
    assert!(Buffer::acquire(&calls, reply, "recording").is_err());
}

#[test]
fn native_network_raw_max_length_and_hidden_identity_do_not_use_display_join() {
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    let bytes = [0xff; 32];
    let entries = [bss(&bytes, 1, true), bss(b"", 2, false)];
    let mut metadata = metadata(&bytes, true, true);
    metadata.dot11DefaultAuthAlgorithm = DOT11_AUTH_ALGORITHM(-123);
    calls.adapter([0; 4], &entries, &[metadata]);
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    let rows = rows(&snapshot, 0);
    assert_eq!(rows[0].ssid.as_bytes(), &bytes);
    assert_eq!(rows[0].known, Some(true));
    assert_eq!(rows[0].auth, None);
    assert!(rows[1].ssid.as_bytes().is_empty());
    assert_eq!(rows[1].known, None);
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_malformed_available_metadata_preserves_bss_with_unknown_flags() {
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    let mut invalid = metadata(b"raw", true, true);
    invalid.dot11Ssid.uSSIDLength = 33;
    calls.adapter([0; 4], &[bss(b"raw", 1, true)], &[invalid]);
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    let row = &rows(&snapshot, 0)[0];
    assert_eq!((row.known, row.secured, &row.auth), (None, None, &None));
    assert_eq!(row.connected, Some(true));
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_bss_denial_skips_current_query_but_preserves_radio() {
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    calls.adapter(
        [0, 0, 5, 0],
        &[bss(b"raw", 1, true)],
        &[metadata(b"raw", true, true)],
    );
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    assert_eq!(
        snapshot.interfaces[0].radio,
        Observation::Ready(RadioState::Enabled)
    );
    assert!(
        matches!(&snapshot.interfaces[0].discovery, Observation::Unavailable(error) if error.kind == NetworkErrorKind::AccessDenied)
    );
    assert!(
        !calls
            .0
            .lock()
            .events
            .iter()
            .any(|event| event.starts_with("Current:"))
    );
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_successful_disconnected_observation_and_fhss_frequency_are_honest() {
    let calls = Recording::default();
    calls.interfaces(&[interface(1)]);
    calls.value(
        Stage::Radio,
        0,
        radio(dot11_radio_state_on, dot11_radio_state_on),
    );
    calls.list(
        Stage::Available,
        0,
        offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
        &[metadata(b"raw", true, true)],
    );
    let mut entry = bss(b"raw", 1, true);
    entry.dot11BssPhyType = dot11_phy_type_fhss;
    calls.list(
        Stage::Bss,
        0,
        offset_of!(WLAN_BSS_LIST, wlanBssEntries),
        &[entry],
    );
    calls.value(
        Stage::Current,
        0,
        WLAN_CONNECTION_ATTRIBUTES {
            isState: wlan_interface_state_disconnected,
            ..Default::default()
        },
    );
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    let row = &rows(&snapshot, 0)[0];
    assert_eq!(row.connected, Some(false));
    assert_eq!(row.frequency_khz, 0);
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_confirmed_enum_disconnected_skips_current_and_marks_cached_rows_false() {
    let calls = Recording::default();
    let disconnected = WLAN_INTERFACE_INFO {
        isState: wlan_interface_state_disconnected,
        ..interface(1)
    };
    calls.interfaces(&[disconnected]);
    calls.value(
        Stage::Radio,
        0,
        radio(dot11_radio_state_on, dot11_radio_state_on),
    );
    calls.list(
        Stage::Available,
        0,
        offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
        &[metadata(b"raw", true, true)],
    );
    calls.list(
        Stage::Bss,
        0,
        offset_of!(WLAN_BSS_LIST, wlanBssEntries),
        &[bss(b"raw", 1, true)],
    );
    let mut owner = Owner::new(calls.clone());
    let snapshot = owner.read().unwrap();
    assert_eq!(rows(&snapshot, 0)[0].connected, Some(false));
    assert!(
        !calls
            .0
            .lock()
            .events
            .iter()
            .any(|event| event.starts_with("Current:"))
    );
    drop(owner);
    calls.settled();
}

#[test]
fn native_network_unknown_or_transitional_enum_state_does_not_turn_query_error_into_false() {
    for state in [
        wlan_interface_state_not_ready,
        wlan_interface_state_associating,
    ] {
        let calls = Recording::default();
        calls.interfaces(&[WLAN_INTERFACE_INFO {
            isState: state,
            ..interface(1)
        }]);
        calls.adapter(
            [0, 0, 0, 5023],
            &[bss(b"raw", 1, true)],
            &[metadata(b"raw", true, true)],
        );
        let mut owner = Owner::new(calls.clone());
        let snapshot = owner.read().unwrap();
        assert_eq!(rows(&snapshot, 0)[0].connected, None);
        assert!(
            calls
                .0
                .lock()
                .events
                .iter()
                .any(|event| event.starts_with("Current:"))
        );
        drop(owner);
        calls.settled();
    }
}

#[test]
fn native_network_same_bssid_conflicting_raw_identity_or_bss_type_is_unknown() {
    for case in 0..3 {
        let calls = Recording::default();
        calls.interfaces(&[interface(1)]);
        calls.value(
            Stage::Radio,
            0,
            radio(dot11_radio_state_on, dot11_radio_state_on),
        );
        calls.list::<WLAN_AVAILABLE_NETWORK>(
            Stage::Available,
            0,
            offset_of!(WLAN_AVAILABLE_NETWORK_LIST, Network),
            &[],
        );
        let entry = if case == 0 {
            bss(b"", 1, true)
        } else {
            bss(&[0xff, 0], 1, true)
        };
        calls.list(
            Stage::Bss,
            0,
            offset_of!(WLAN_BSS_LIST, wlanBssEntries),
            &[entry],
        );
        let mut current = WLAN_CONNECTION_ATTRIBUTES {
            isState: wlan_interface_state_connected,
            ..Default::default()
        };
        current.wlanAssociationAttributes.dot11Bssid = entry.dot11Bssid;
        current.wlanAssociationAttributes.dot11Ssid = if case == 0 {
            raw_ssid(b"named hidden network")
        } else if case == 1 {
            raw_ssid(&[0xfe, 0])
        } else {
            entry.dot11Ssid
        };
        current.wlanAssociationAttributes.dot11BssType = if case == 2 {
            dot11_BSS_type_independent
        } else {
            entry.dot11BssType
        };
        calls.value(Stage::Current, 0, current);
        let mut owner = Owner::new(calls.clone());
        let snapshot = owner.read().unwrap();
        assert_eq!(rows(&snapshot, 0)[0].connected, None);
        drop(owner);
        calls.settled();
    }
}
