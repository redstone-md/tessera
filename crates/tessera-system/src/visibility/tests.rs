// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
    Rect::new(x, y, width, height).unwrap()
}

#[test]
fn visibility_overlap_is_any_eligible_window_not_just_foreground() {
    let monitor = rect(-1920, -200, 1920, 1080);
    let hitbox = rect(-1500, 800, 1000, 80);
    let windows = [
        VisibilityWindow {
            bounds: rect(-1800, 0, 200, 200), // eligible foreground elsewhere
            belongs_to_monitor: true,
            minimized: false,
        },
        VisibilityWindow {
            bounds: rect(-1400, 790, 600, 90), // background intersects Dock
            belongs_to_monitor: true,
            minimized: false,
        },
    ];
    let mut facts = VisibilityFacts {
        monitor,
        hitbox,
        foreground_interactable: true,
        windows: &windows,
    };
    assert!(any_window_overlaps(&facts));
    facts.foreground_interactable = false; // desktop/owned launcher clears the gate
    assert!(!any_window_overlaps(&facts));
}

#[test]
fn visibility_overlap_excludes_minimized_foreign_monitor_and_touching_edges() {
    let monitor = rect(0, 0, 100, 100);
    let hitbox = rect(20, 80, 60, 20);
    for candidate in [
        VisibilityWindow {
            bounds: hitbox,
            belongs_to_monitor: false,
            minimized: false,
        },
        VisibilityWindow {
            bounds: hitbox,
            belongs_to_monitor: true,
            minimized: true,
        },
        VisibilityWindow {
            bounds: rect(20, 40, 60, 40),
            belongs_to_monitor: true,
            minimized: false,
        },
        VisibilityWindow {
            bounds: rect(80, 80, 20, 20),
            belongs_to_monitor: true,
            minimized: false,
        },
    ] {
        assert!(!any_window_overlaps(&VisibilityFacts {
            monitor,
            hitbox,
            foreground_interactable: true,
            windows: &[candidate],
        }));
    }
    assert!(strictly_intersects(hitbox, rect(79, 79, 1, 2)));
}

#[test]
fn visibility_overlap_uses_last_uncapped_candidate_beyond_display_row_limits() {
    let monitor = rect(-1920, -200, 1920, 1080);
    let hitbox = rect(-1500, 800, 1000, 80);
    let mut windows = vec![
        VisibilityWindow {
            bounds: rect(-1900, 0, 10, 10),
            belongs_to_monitor: true,
            minimized: false,
        };
        1024
    ];
    assert!(!any_window_overlaps(&VisibilityFacts {
        monitor,
        hitbox,
        foreground_interactable: true,
        windows: &windows,
    }));
    windows.push(VisibilityWindow {
        bounds: rect(-1400, 790, 600, 90),
        belongs_to_monitor: true,
        minimized: false,
    });
    assert!(any_window_overlaps(&VisibilityFacts {
        monitor,
        hitbox,
        foreground_interactable: true,
        windows: &windows,
    }));
}

#[test]
fn visibility_edges_have_source_tolerance_full_span_and_corner_precedence() {
    let monitor = rect(-300, -100, 200, 150);
    for delta in -3_i32..=3 {
        let expected = delta.abs() <= 2;
        assert_eq!(
            edge_at(
                monitor,
                PhysicalPoint {
                    x: -200,
                    y: -100 + delta
                }
            ) == Some(Edge::Top),
            expected
        );
        assert_eq!(
            edge_at(
                monitor,
                PhysicalPoint {
                    x: -200,
                    y: 49 + delta
                }
            ) == Some(Edge::Bottom),
            expected
        );
        assert_eq!(
            edge_at(
                monitor,
                PhysicalPoint {
                    x: -300 + delta,
                    y: 0
                }
            ) == Some(Edge::Left),
            expected
        );
        assert_eq!(
            edge_at(
                monitor,
                PhysicalPoint {
                    x: -101 + delta,
                    y: 0
                }
            ) == Some(Edge::Right),
            expected
        );
    }
    assert_eq!(
        edge_at(monitor, PhysicalPoint { x: -300, y: -100 }),
        Some(Edge::Top)
    );
    assert_eq!(
        edge_at(monitor, PhysicalPoint { x: -101, y: 49 }),
        Some(Edge::Bottom)
    );
    assert_eq!(
        edge_at(monitor, PhysicalPoint { x: -301, y: -100 }),
        Some(Edge::Left)
    );
    assert_eq!(
        edge_at(monitor, PhysicalPoint { x: -100, y: -100 }),
        Some(Edge::Right)
    );
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -200, y: -103 }), None);
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -200, y: 52 }), None);
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -303, y: 0 }), None);
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -98, y: 0 }), None);
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -50, y: -100 }), None);
    assert_eq!(edge_at(monitor, PhysicalPoint { x: -300, y: 80 }), None);
}

#[test]
fn visibility_edge_arithmetic_is_total_at_checked_i32_extremes_and_seams() {
    let minimum = rect(i32::MIN, i32::MIN, 4, 4);
    assert_eq!(
        edge_at(
            minimum,
            PhysicalPoint {
                x: i32::MIN,
                y: i32::MIN
            }
        ),
        Some(Edge::Top)
    );
    let maximum = rect(i32::MAX - 4, i32::MAX - 4, 4, 4);
    assert_eq!(
        edge_at(
            maximum,
            PhysicalPoint {
                x: i32::MAX - 1,
                y: i32::MAX - 1
            }
        ),
        Some(Edge::Bottom)
    );
    assert_eq!(
        edge_at(
            maximum,
            PhysicalPoint {
                x: i32::MIN,
                y: i32::MIN
            }
        ),
        None
    );
    let left = rect(-200, 0, 200, 100);
    let right = rect(0, 0, 200, 100);
    let point = PhysicalPoint { x: 0, y: 30 };
    assert_eq!(edge_at(left, point), Some(Edge::Right));
    assert_eq!(edge_at(right, point), Some(Edge::Left));
    assert!(Rect::new(i32::MAX, 0, 1, 1).is_err());
    assert!(Rect::new(0, 0, 0, 1).is_err());
}

fn inputs(mode: AutoHideMode, overlap: bool, at_edge: bool) -> VisibilityInputs {
    VisibilityInputs {
        config: BarConfig {
            mode,
            ..BarConfig::default()
        },
        facts: BarFacts {
            overlap: Some(overlap),
            own_focus: Some(false),
            touch_primary: Some(false),
            ..BarFacts::default()
        },
        pointer_ready: true,
        selected_edge: Some(at_edge),
    }
}

#[test]
fn visibility_modes_and_source_default_delay_table() {
    for overlap in [false, true] {
        for at_edge in [false, true] {
            assert_eq!(
                decide_visibility(inputs(AutoHideMode::Never, overlap, at_edge)),
                VisibilityDecision::ShowNow
            );
            assert_eq!(
                decide_visibility(inputs(AutoHideMode::Always, overlap, at_edge)),
                if at_edge {
                    VisibilityDecision::ShowAfter(Duration::from_millis(100))
                } else {
                    VisibilityDecision::HideAfter(Duration::from_millis(800))
                }
            );
            assert_eq!(
                decide_visibility(inputs(AutoHideMode::OnOverlap, overlap, at_edge)),
                if at_edge || !overlap {
                    VisibilityDecision::ShowAfter(DEFAULT_SHOW_DELAY)
                } else {
                    VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY)
                }
            );
        }
    }
}

#[test]
fn visibility_overrides_freeze_and_outer_fullscreen_are_distinct() {
    let base = inputs(AutoHideMode::OnOverlap, true, false);
    let mut changed = base;
    changed.facts.own_focus = Some(true);
    assert_eq!(
        decide_visibility(changed),
        VisibilityDecision::ShowAfter(DEFAULT_SHOW_DELAY)
    );
    changed = base;
    changed.facts.touch_primary = Some(true);
    assert_eq!(decide_visibility(changed), VisibilityDecision::ShowNow);
    changed = base;
    changed.facts.dragging = true;
    assert_eq!(decide_visibility(changed), VisibilityDecision::ShowNow);
    changed = base;
    changed.facts.attention_reveal = true;
    assert_eq!(decide_visibility(changed), VisibilityDecision::ShowNow);
    changed = base;
    changed.facts.workspace_switching = true;
    assert_eq!(decide_visibility(changed), VisibilityDecision::Keep);
    changed.facts.confirmed_fullscreen = true;
    changed.facts.dragging = true;
    changed.facts.attention_reveal = true;
    changed.facts.touch_primary = Some(true);
    changed.selected_edge = Some(true);
    changed.config.mode = AutoHideMode::Never;
    assert_eq!(decide_visibility(changed), VisibilityDecision::HideNow);
}

#[test]
fn visibility_unknown_required_facts_fail_visible_except_confirmed_fullscreen() {
    let base = inputs(AutoHideMode::OnOverlap, true, false);
    for unknown in [
        VisibilityInputs {
            pointer_ready: false,
            ..base
        },
        VisibilityInputs {
            selected_edge: None,
            ..base
        },
        VisibilityInputs {
            facts: BarFacts {
                own_focus: None,
                ..base.facts
            },
            ..base
        },
        VisibilityInputs {
            facts: BarFacts {
                touch_primary: None,
                ..base.facts
            },
            ..base
        },
        VisibilityInputs {
            facts: BarFacts {
                overlap: None,
                ..base.facts
            },
            ..base
        },
    ] {
        assert_eq!(decide_visibility(unknown), VisibilityDecision::ShowNow);
        assert_eq!(
            decide_visibility(VisibilityInputs {
                facts: BarFacts {
                    confirmed_fullscreen: true,
                    ..unknown.facts
                },
                ..unknown
            }),
            VisibilityDecision::HideNow
        );
    }
    let always = VisibilityInputs {
        config: BarConfig {
            mode: AutoHideMode::Always,
            ..base.config
        },
        facts: BarFacts {
            overlap: None,
            ..base.facts
        },
        ..base
    };
    assert_eq!(
        decide_visibility(always),
        VisibilityDecision::HideAfter(DEFAULT_HIDE_DELAY)
    );
}

#[test]
fn visibility_pointer_error_text_is_fixed_and_retains_native_code() {
    assert_eq!(
        PointerWatchError::Busy.to_string(),
        "A passive pointer watch is already active"
    );
    assert_eq!(
        PointerWatchError::Native {
            operation: PointerNativeOperation::ReadCursor,
            code: u32::MAX,
        }
        .to_string(),
        "Passive pointer operation ReadCursor failed (native code 0xffffffff)"
    );
    fn portable<T: Send + Sync + Copy + Eq + 'static>() {}
    portable::<PhysicalPoint>();
    portable::<PointerEnvironment>();
    portable::<PointerWatchEvent>();
    portable::<PointerWatchError>();
}
