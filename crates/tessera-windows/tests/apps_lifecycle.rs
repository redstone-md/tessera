// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.
//! Native public API lifecycle; private identity/pixel tests live with their implementation.
#![cfg(windows)]
use std::sync::Arc;
use tessera_windows::{catalog, foreground_window_id, watch_desktop};

#[test]
fn catalog_is_bounded_sorted_and_self_consistent() {
    let apps = catalog().expect("AppsFolder enumeration");
    assert!(apps.len() <= 1024);
    let ids: std::collections::HashSet<_> = apps.iter().map(|app| app.id()).collect();
    assert_eq!(ids.len(), apps.len());
    assert!(
        apps.windows(2)
            .all(|pair| pair[0].name().to_uppercase() <= pair[1].name().to_uppercase())
    );
    for app in &apps {
        assert!(!app.id().is_empty() && !app.id().contains('\0'));
        assert!(app.id().encode_utf16().count() <= 1024);
    }
}

#[test]
fn foreground_query_does_not_need_focus_changes() {
    if let Some(id) = foreground_window_id() {
        assert_ne!(id.value(), 0);
    }
}

#[test]
fn watcher_starts_stops_and_restarts_cleanly() {
    let watcher = watch_desktop(Arc::new(|| {})).expect("watcher starts");
    watcher.stop();
    let watcher = watch_desktop(Arc::new(|| {})).expect("watcher restarts");
    drop(watcher);
}
