// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Portable geometry/contract tests: negative origins, 96/144/192 DPI,
//! mismatched toolbar dimensions, non-vacuous no-toolbar result, and
//! disappearance handling. Windows fixtures live in the integration tests.

use super::*;
use tessera_core::Rect;

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
    Rect::new(x, y, w, h).unwrap()
}

#[test]
fn toolbar_matches_contract_across_dpi_and_negative_origins() {
    for (x, y, width, dpi) in [
        (-1920, -1080, 1920, 96),
        (0, 0, 3840, 96),
        (-2560, 0, 2560, 144),
        (7680, -4320, 1920, 192),
    ] {
        let monitor = rect(x, y, width, 2160);
        let expected = expected_toolbar_rect(monitor, dpi).unwrap();
        assert_eq!(expected.x(), monitor.x(), "left edge");
        assert_eq!(expected.y(), monitor.y(), "top edge is monitor bounds top");
        assert_eq!(expected.width(), monitor.width(), "full monitor width");
        assert_eq!(
            expected.height(),
            (f64::from(tessera_core::TOOLBAR_HEIGHT_LOGICAL) * f64::from(dpi) / 96.0).round()
                as u32,
            "round(shared logical toolbar height * dpi/96)"
        );
        assert_eq!(check_toolbar(expected, monitor, dpi), Ok(None));
    }
}

#[test]
fn expected_toolbar_heights_round_the_shared_allocation() {
    let monitor = rect(0, 0, 1000, 1000);
    assert_eq!(expected_toolbar_rect(monitor, 96).unwrap().height(), 40);
    assert_eq!(expected_toolbar_rect(monitor, 99).unwrap().height(), 41);
    assert_eq!(expected_toolbar_rect(monitor, 120).unwrap().height(), 50);
    assert_eq!(expected_toolbar_rect(monitor, 144).unwrap().height(), 60);
    assert_eq!(expected_toolbar_rect(monitor, 192).unwrap().height(), 80);
    // Zero DPI is invalid input: an error, never a fabricated fallback.
    assert!(expected_toolbar_rect(monitor, 0).is_err());
}

#[test]
fn mismatched_toolbar_reports_exact_deltas() {
    let monitor = rect(-1920, 0, 1920, 1040);
    // Synthetic case: actual sits 32px lower and is 4px too short.
    let actual = rect(-1920, 32, 1920, 36);
    let mismatch = check_toolbar(actual, monitor, 96)
        .unwrap()
        .expect("must detect mismatch");
    assert_eq!(
        mismatch.divergences(),
        vec![("top".to_owned(), 32), ("height".to_owned(), -4),]
    );

    // Same monitor bounds, wrong width: work-area-width instead of full.
    let narrow = rect(-1920, 0, 1888, 40);
    let mismatch = check_toolbar(narrow, monitor, 96)
        .unwrap()
        .expect("must detect width mismatch");
    assert_eq!(mismatch.divergences(), vec![("width".to_owned(), -32)]);

    // 144 DPI bar built at 96 height.
    let too_short = rect(-1920, 0, 1920, 40);
    let mismatch = check_toolbar(too_short, monitor, 144)
        .unwrap()
        .expect("must detect height mismatch");
    assert_eq!(mismatch.divergences(), vec![("height".to_owned(), -20)]);
}

#[test]
fn no_toolbar_is_reported_not_vacuously_successful() {
    let scan_without_toolbar = SurfaceScan {
        monitors: vec![(1, rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1000))],
        rows: Vec::new(),
        warnings: Vec::new(),
    };
    let error = report(&scan_without_toolbar).expect_err("no toolbar must fail");
    assert!(matches!(error, CheckError::NoToolbar));

    // A stale toolbar row (diagnostics unavailable) is also red, never green.
    let stale = SurfaceScan {
        monitors: vec![(1, rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1000))],
        rows: vec![SurfaceRow {
            kind: "toolbar",
            pid: 7,
            window_id: 0xABC,
            monitor_id: Some(1),
            bounds: rect(0, 0, 1920, 32),
            title: "Tessera toolbar",
            class_name: "Static".into(),
            dpi: None,
            style: None,
            ex_style: None,
            show_command: None,
            unavailable: Some("window vanished or pid changed".into()),
        }],
        warnings: Vec::new(),
    };
    let error = report(&stale).expect_err("stale diagnostics must fail");
    assert!(matches!(
        &error,
        CheckError::Incomplete {
            window_id: 0xABC,
            ..
        }
    ));
}

/// Shared verification path reused from the module (no local duplicate).
#[allow(clippy::needless_pass_by_value)]
fn report(scan: &SurfaceScan) -> Result<(), CheckError> {
    let toolbar = verify_toolbar(scan)?;
    let dpi = toolbar
        .dpi
        .expect("verify_toolbar requires complete diagnostics");
    let &(_, bounds, _) = scan
        .monitors
        .iter()
        .find(|(id, _, _)| Some(*id) == toolbar.monitor_id)
        .expect("verify_toolbar requires the matched monitor");
    let verdict = check_toolbar(toolbar.bounds, bounds, dpi)
        .expect("verify_toolbar rejects invalid DPI before this point");
    match verdict {
        None => Ok(()),
        Some(mismatch) => Err(CheckError::GeometryMismatch(Box::new(mismatch))),
    }
}

#[test]
fn passing_toolbar_requires_real_observed_row() {
    let scan = SurfaceScan {
        monitors: vec![(1, rect(0, 0, 1920, 1040), rect(0, 0, 1920, 1000))],
        rows: vec![SurfaceRow {
            kind: "toolbar",
            pid: 7,
            window_id: 0xABC,
            monitor_id: Some(1),
            bounds: rect(0, 0, 1920, 40),
            title: "Tessera toolbar",
            class_name: "Static".into(),
            dpi: Some(96),
            style: Some(0x5600_0000),
            ex_style: Some(0),
            show_command: Some(1),
            unavailable: None,
        }],
        warnings: Vec::new(),
    };
    assert!(report(&scan).is_ok());

    // A row missing ANY diagnostic field must not pass either.
    for (field, reason) in [
        ("dpi", "DPI unavailable"),
        ("style", "style unavailable"),
        ("ex_style", "ex-style unavailable"),
        ("show_command", "placement unavailable"),
    ] {
        let mut partial = scan.clone();
        match field {
            "dpi" => partial.rows[0].dpi = None,
            "style" => partial.rows[0].style = None,
            "ex_style" => partial.rows[0].ex_style = None,
            "show_command" => partial.rows[0].show_command = None,
            _ => unreachable!(),
        }
        partial.rows[0].unavailable = Some(reason.into());
        assert!(
            report(&partial).is_err(),
            "incomplete diagnostics must not pass: missing {field}"
        );
    }
}

#[test]
fn surface_titles_come_from_current_slint_sources() {
    let toolbar_ui = include_str!("../../../tessera-ui/ui/toolbar.slint");
    let dock_ui = include_str!("../../../tessera-ui/ui/dock.slint");
    let launcher_ui = include_str!("../../../tessera-ui/ui/launcher.slint");
    for (kind, title) in SURFACE_TITLES {
        let source = match kind {
            "toolbar" => toolbar_ui,
            "dock" => dock_ui,
            "launcher" => launcher_ui,
            _ => unreachable!(),
        };
        assert!(
            source.contains(&format!("title: \"{title}\"")),
            "{kind} title `{title}` must exist in its current .slint source"
        );
    }
}
