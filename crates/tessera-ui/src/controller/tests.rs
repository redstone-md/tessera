// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

#[cfg(debug_assertions)]
use i_slint_backend_testing::ElementHandle;

use super::*;
use crate::{PanelWindow, Theme};

type Observation = dyn Fn() -> Result<PanelSnapshot, String> + Send + Sync;

struct FixtureHost {
    source: Box<Observation>,
    observe_calls: AtomicUsize,
    activations: Mutex<Vec<String>>,
    activation_result: Mutex<Result<(), String>>,
    saves: Mutex<Vec<PanelPreferences>>,
    save_result: Mutex<Result<(), String>>,
}

impl FixtureHost {
    fn new(
        source: impl Fn() -> Result<PanelSnapshot, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            source: Box::new(source),
            observe_calls: AtomicUsize::new(0),
            activations: Mutex::new(Vec::new()),
            activation_result: Mutex::new(Ok(())),
            saves: Mutex::new(Vec::new()),
            save_result: Mutex::new(Ok(())),
        })
    }

    fn returning(snapshot: PanelSnapshot) -> Arc<Self> {
        Self::new(move || Ok(snapshot.clone()))
    }
}

impl DesktopHost for FixtureHost {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        (self.source)()
    }

    fn activate(&self, key: &str) -> Result<(), String> {
        self.activations.lock().push(key.to_owned());
        self.activation_result.lock().clone()
    }

    fn save_preferences(&self, preferences: &PanelPreferences) -> Result<(), String> {
        self.saves.lock().push(*preferences);
        self.save_result.lock().clone()
    }
}

fn snapshot() -> PanelSnapshot {
    PanelSnapshot::new(
        2,
        vec![
            PanelWindow::new("editor-key".into(), "Rust Editor\nwindow".into(), false),
            PanelWindow::new("browser-key".into(), "Web Browser".into(), true),
        ],
        1,
    )
}

// Slint allows one threaded backend per process. All worker scenarios share
// this event loop; joins below are test-only and no production UI waits on them.
#[test]
fn workers_are_single_flight_recover_from_errors_and_do_not_retain_closed_windows() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = PanelController::new(&panel, host.clone());
    controller.refresh().unwrap().join().unwrap();
    assert!(panel.get_refreshing());
    panel.invoke_refresh_requested();
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 1);

    let failed_panel = Panel::new().unwrap();
    let attempts = AtomicUsize::new(0);
    let retry_host = FixtureHost::new(move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("native query\nfailed\u{202e}".into())
        } else {
            Ok(PanelSnapshot::new(1, Vec::new(), 0))
        }
    });
    let retry = PanelController::new(&failed_panel, retry_host.clone());
    apply_result(&retry, &failed_panel, Ok(snapshot()));
    retry.refresh().unwrap().join().unwrap();

    let panicked_panel = Panel::new().unwrap();
    let panicked = PanelController::new(
        &panicked_panel,
        FixtureHost::new(|| panic!("private fixture")),
    );
    panicked.refresh().unwrap().join().unwrap();

    let closing_panel = Panel::new().unwrap();
    let closed = closing_panel.as_weak();
    let (started, entered) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let closing = PanelController::new(
        &closing_panel,
        FixtureHost::new(move || {
            started.send(()).unwrap();
            gate.lock().recv_timeout(Duration::from_secs(2)).unwrap();
            Ok(PanelSnapshot::new(1, Vec::new(), 0))
        }),
    );
    let worker = closing.refresh().unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(closing_panel);
    assert!(closed.upgrade().is_none());
    release.send(()).unwrap();
    worker.join().unwrap();

    let failed = failed_panel.as_weak();
    slint::invoke_from_event_loop(move || {
        let panel = failed.upgrade().unwrap();
        assert!(panel.get_stale());
        assert!(!panel.get_refreshing());
        let error = panel.get_status();
        assert!(error.contains("retained data is stale"));
        panel.invoke_activate_requested("editor-key".into());
        assert!(retry_host.activations.lock().is_empty());
        panel.set_search("EDITOR".into());
        panel.invoke_filter_requested();
        assert_eq!(panel.get_status(), error);
        assert_eq!(panel.get_rows().row_count(), 1);
        // Immediate fixture retry queues delivery before the quit callback.
        retry.refresh().unwrap().join().unwrap();
        slint::invoke_from_event_loop(|| slint::quit_event_loop().unwrap()).unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();

    assert!(!panel.get_refreshing());
    assert_eq!(
        panel.get_rows().row_data(0).unwrap().caption,
        "Rust Editor window"
    );
    #[cfg(debug_assertions)]
    assert_eq!(
        ElementHandle::find_by_accessible_label(&panel, "Refresh")
            .next()
            .unwrap()
            .accessible_enabled(),
        Some(true)
    );
    assert!(!failed_panel.get_stale());
    assert_eq!(failed_panel.get_rows().row_count(), 0);
    assert!(failed_panel.get_status().contains("No visible windows"));
    assert!(!panicked_panel.get_refreshing());
    assert!(
        panicked_panel
            .get_status()
            .contains("Observation failed unexpectedly")
    );
    assert!(!panicked_panel.get_status().contains("private fixture"));
    assert!(closed.upgrade().is_none());
}

#[test]
fn search_callbacks_preserve_keys_and_activation_guards_and_errors_are_enforced() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let controller = PanelController::new(&panel, host.clone());
    apply_result(&controller, &panel, Ok(snapshot()));
    panel.set_search("BROWSER".into());
    panel.invoke_filter_requested();
    assert_eq!(panel.get_rows().row_count(), 1);
    assert_eq!(panel.get_rows().row_data(0).unwrap().key, "browser-key");
    panel.invoke_activate_requested("browser-key".into());
    assert_eq!(host.activations.lock().as_slice(), ["browser-key"]);
    panel.invoke_activate_requested("editor-key".into());
    panel.set_refreshing(true);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_refreshing(false);
    panel.set_stale(true);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_stale(false);
    panel.set_has_snapshot(false);
    panel.invoke_activate_requested("browser-key".into());
    panel.set_has_snapshot(true);
    assert_eq!(host.activations.lock().len(), 1);
    *host.activation_result.lock() = Err("denied\u{202e}".into());
    panel.invoke_activate_requested("browser-key".into());
    assert!(panel.get_status().contains("Activation failed"));
    assert!(!panel.get_status().contains('\u{202e}'));
    panel.set_search("".into());
    panel.invoke_filter_requested();
    assert_eq!(panel.get_rows().row_count(), 2);
    assert_eq!(host.observe_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn preferences_save_only_on_explicit_callback_and_errors_are_visible() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let host = FixtureHost::returning(snapshot());
    let _controller = PanelController::new(&panel, host.clone());
    panel.set_theme_index(2);
    panel.set_compact(true);
    assert!(host.saves.lock().is_empty());
    panel.invoke_save_preferences_requested();
    assert_eq!(
        host.saves.lock().as_slice(),
        [PanelPreferences::new(Theme::Dark, true)]
    );
    assert_eq!(panel.get_status(), "Preferences saved");
    *host.save_result.lock() = Err("disk full\u{200f}".into());
    panel.invoke_save_preferences_requested();
    assert!(panel.get_status().contains("Could not save preferences"));
    assert!(!panel.get_status().contains('\u{200f}'));
}

#[test]
fn initial_failure_is_not_stale_data_and_visible_rows_are_bounded() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    let controller = PanelController::new(&panel, FixtureHost::returning(snapshot()));
    apply_result(&controller, &panel, Err("desktop unavailable".into()));
    assert!(!panel.get_has_snapshot());
    assert!(!panel.get_status().contains("retained data"));
    let rows = (0..crate::projection::MAX_ROWS + 12)
        .map(|index| PanelWindow::new(index.to_string(), String::new(), false))
        .collect();
    apply_result(&controller, &panel, Ok(PanelSnapshot::new(1, rows, 3)));
    assert_eq!(panel.get_rows().row_count(), crate::projection::MAX_ROWS);
    assert_eq!(
        panel.get_rows().row_data(0).unwrap().caption,
        "(untitled window)"
    );
    assert!(panel.get_status().contains("128 of 140"));
}
