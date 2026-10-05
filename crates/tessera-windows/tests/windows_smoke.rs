#![cfg(windows)]
#![allow(unsafe_code)]

use std::collections::HashSet;

use windows_sys::Win32::UI::HiDpi::{AreDpiAwarenessContextsEqual, GetThreadDpiAwarenessContext};

#[test]
fn observation_restores_dpi_and_returns_consistent_records() {
    // SAFETY: queries return opaque context tokens for this test thread.
    let before = unsafe { GetThreadDpiAwarenessContext() };
    let result = tessera_windows::observe();
    // SAFETY: context tokens are compared by user32, never dereferenced here.
    let restored = unsafe { AreDpiAwarenessContextsEqual(before, GetThreadDpiAwarenessContext()) };
    assert_ne!(
        restored, 0,
        "DPI context must be restored on success or error"
    );
    let snapshot = result.expect("Windows observation must succeed (an empty desktop is valid)");

    let monitor_ids: HashSet<_> = snapshot
        .monitors()
        .iter()
        .map(|monitor| monitor.id())
        .collect();
    assert_eq!(monitor_ids.len(), snapshot.monitors().len());
    let window_ids: HashSet<_> = snapshot
        .windows()
        .iter()
        .map(|window| window.id())
        .collect();
    assert_eq!(window_ids.len(), snapshot.windows().len());
    for window in snapshot.windows() {
        if let Some(monitor) = window.monitor_id() {
            assert!(monitor_ids.contains(&monitor));
        }
        if window.minimized() {
            assert!(!window.covers_monitor());
        }
    }
}
