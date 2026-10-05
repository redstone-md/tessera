// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

#[cfg(debug_assertions)]
use i_slint_backend_testing::ElementHandle;
use parking_lot::Mutex;
use slint::Model;

use super::*;

// Slint permits one threaded testing backend per process. All asynchronous
// cases share this fixture and one event-loop run; there is no sleep polling.
#[test]
fn observation_workers_deliver_single_flight_results_and_ignore_closed_windows() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let panel = Panel::new().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let controller = PanelController::new(&panel, move || {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(PanelSnapshot::new(2, vec!["Editor\nwindow".into()], 1))
    });
    controller.refresh().unwrap().join().unwrap();
    // Source returned, but UI delivery is queued: Refresh must remain blocked.
    assert!(panel.get_refreshing());
    panel.invoke_refresh_requested();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let failed_panel = Panel::new().unwrap();
    apply_result(
        &failed_panel,
        Ok(PanelSnapshot::new(2, vec!["Editor".into()], 0)),
    );
    let before = failed_panel.get_summary().to_string();
    let attempts = AtomicUsize::new(0);
    let retry_controller = PanelController::new(&failed_panel, move || {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err("native query\nfailed\u{202e}".into())
        } else {
            Ok(PanelSnapshot::new(1, Vec::new(), 0))
        }
    });
    retry_controller.refresh().unwrap().join().unwrap();

    let panicked_panel = Panel::new().unwrap();
    let panicked_controller =
        PanelController::new(&panicked_panel, || panic!("private fixture panic payload"));
    panicked_controller.refresh().unwrap().join().unwrap();

    let closing_panel = Panel::new().unwrap();
    let closed = closing_panel.as_weak();
    let (started, entered) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let closing_controller = PanelController::new(&closing_panel, move || {
        started.send(()).unwrap();
        gate.lock().recv_timeout(Duration::from_secs(2)).unwrap();
        Ok(PanelSnapshot::new(1, Vec::new(), 0))
    });
    let worker = closing_controller.refresh().unwrap();
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(closing_panel);
    assert!(closed.upgrade().is_none());
    release.send(()).unwrap();
    worker.join().unwrap();

    let failed = failed_panel.as_weak();
    slint::invoke_from_event_loop(move || {
        let failed = failed.upgrade().unwrap();
        assert_eq!(failed.get_rows().row_data(0).unwrap(), "Editor");
        assert_eq!(failed.get_summary().as_str(), before);
        assert!(failed.get_status().contains("retained data is stale"));
        assert!(!failed.get_status().contains('\u{202e}'));
        assert!(!failed.get_refreshing());
        // Test-only join of an immediate fixture source ensures retry delivery
        // is queued before quit, without timing assumptions or polling.
        retry_controller.refresh().unwrap().join().unwrap();
        slint::invoke_from_event_loop(|| slint::quit_event_loop().unwrap()).unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();

    assert!(!panel.get_refreshing());
    assert_eq!(panel.get_rows().row_data(0).unwrap(), "Editor window");
    assert_eq!(
        panel.get_summary(),
        "Monitors: 2 | Windows: 1 | Warnings: 1"
    );
    #[cfg(debug_assertions)]
    {
        let refresh = ElementHandle::find_by_accessible_label(&panel, "Refresh")
            .next()
            .expect("standard button exposes its accessible action");
        assert_eq!(refresh.accessible_enabled(), Some(true));
    }
    assert!(failed_panel.get_has_snapshot());
    assert_eq!(failed_panel.get_rows().row_count(), 0);
    assert_eq!(failed_panel.get_status(), "No visible windows observed");
    assert!(!failed_panel.get_refreshing());
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
fn initial_failure_does_not_claim_stale_data_and_rows_are_bounded() {
    i_slint_backend_testing::init_no_event_loop();
    let panel = Panel::new().unwrap();
    apply_result(&panel, Err("desktop unavailable".into()));
    assert!(!panel.get_has_snapshot());
    assert!(!panel.get_status().contains("stale"));
    assert_eq!(panel.get_rows().row_count(), 0);

    apply_result(
        &panel,
        Ok(PanelSnapshot::new(1, vec!["".into(); MAX_ROWS + 12], 3)),
    );
    assert_eq!(panel.get_rows().row_count(), MAX_ROWS);
    assert_eq!(panel.get_rows().row_data(0).unwrap(), "(untitled window)");
    assert!(panel.get_status().contains("128 of 140"));
}
