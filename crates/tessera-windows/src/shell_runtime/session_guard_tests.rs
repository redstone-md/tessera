// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! In-memory TaskbarBackend drives the same engine as Windows. No test touches
//! Explorer, appbar registration, Winlogon or a real desktop.

use super::placement::{CapturedAppbarState, CapturedPlacement};
use super::session_guard::{SessionPresentation, TaskbarBackend, TaskbarHide, finish_session};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct Desktop {
    window: Option<isize>,
    captures: usize,
    original: Option<TaskbarHide>,
    hidden_flags: Vec<(isize, u32)>,
    restored: Vec<(isize, TaskbarHide)>,
    steps: Vec<&'static str>,
    fail_hide: bool,
    fail_restore: bool,
    incomplete: bool,
}

struct Fixture(Rc<RefCell<Desktop>>);
impl TaskbarBackend for Fixture {
    type Window = isize;
    type Error = &'static str;
    fn find(&mut self) -> Result<Option<isize>, Self::Error> {
        Ok(self.0.borrow().window)
    }
    fn capture(&mut self, _: isize) -> Result<Option<TaskbarHide>, Self::Error> {
        let mut state = self.0.borrow_mut();
        if state.incomplete {
            return Ok(None);
        }
        state.captures += 1;
        Ok(state.original)
    }
    fn hide(&mut self, window: isize, original: TaskbarHide) -> Result<(), Self::Error> {
        let mut state = self.0.borrow_mut();
        state
            .hidden_flags
            .push((window, original.appbar.hidden_flags()));
        state.steps.push("hide");
        if state.fail_hide {
            Err("hide readback failed")
        } else {
            Ok(())
        }
    }
    fn restore(&mut self, window: Option<isize>, original: TaskbarHide) -> Result<(), Self::Error> {
        let window = window.ok_or("taskbar disappeared during restore")?;
        let mut state = self.0.borrow_mut();
        state.restored.push((window, original));
        state.steps.push("restore");
        if state.fail_restore {
            Err("restore readback failed")
        } else {
            Ok(())
        }
    }
}

fn snapshot(visible: bool, flags: u32) -> TaskbarHide {
    TaskbarHide {
        placement: CapturedPlacement::new(1).unwrap(),
        appbar: CapturedAppbarState::new(flags).unwrap(),
        visible,
    }
}

fn fixture(
    window: Option<isize>,
    original: TaskbarHide,
) -> (SessionPresentation<Fixture>, Rc<RefCell<Desktop>>) {
    let state = Rc::new(RefCell::new(Desktop {
        window,
        original: Some(original),
        ..Default::default()
    }));
    (SessionPresentation::new(Fixture(Rc::clone(&state))), state)
}

#[test]
fn originally_hidden_normal_placement_restores_visibility_false_not_minimized_guess() {
    let original = snapshot(false, 2);
    assert_eq!(original.placement.show_command, 1);
    assert_eq!(original.restore_command(), 0);
    let (mut engine, state) = fixture(Some(10), original);
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    engine.restore().unwrap();
    assert_eq!(state.borrow().hidden_flags, [(10, 3)]);
    assert_eq!(state.borrow().restored, [(10, original)]);
}

#[test]
fn flags_two_and_three_roundtrip_and_hide_both_have_autohide() {
    for flags in [2, 3] {
        let original = snapshot(true, flags);
        let (mut engine, state) = fixture(Some(10), original);
        engine.prepare().unwrap();
        engine.hide_after_ready().unwrap();
        engine.restore().unwrap();
        assert_eq!(state.borrow().hidden_flags, [(10, 3)]);
        assert_eq!(state.borrow().restored[0].1.appbar.flags, flags);
    }
}

#[test]
fn missing_initial_taskbar_captures_later_original_once() {
    let original = snapshot(false, 2);
    let (mut engine, state) = fixture(None, original);
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    assert_eq!(state.borrow().captures, 0);
    state.borrow_mut().window = Some(20);
    engine.rehide(20).unwrap();
    state.borrow_mut().original = Some(snapshot(true, 3));
    engine.rehide(20).unwrap();
    engine.restore().unwrap();
    assert_eq!(state.borrow().captures, 1);
    assert_eq!(state.borrow().restored, [(20, original)]);
}

#[test]
fn reappearance_restores_fresh_handle_with_original_backup() {
    let original = snapshot(true, 2);
    let (mut engine, state) = fixture(Some(10), original);
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().window = Some(99);
    state.borrow_mut().original = Some(snapshot(false, 3));
    engine.rehide(99).unwrap();
    engine.restore().unwrap();
    assert_eq!(state.borrow().captures, 1);
    assert_eq!(state.borrow().hidden_flags, [(10, 3), (99, 3)]);
    assert_eq!(state.borrow().restored, [(99, original)]);
}

#[test]
fn replacement_after_watcher_stops_is_also_resolved_at_restore() {
    let original = snapshot(true, 3);
    let (mut engine, state) = fixture(Some(10), original);
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().window = Some(30);
    engine.restore().unwrap();
    assert_eq!(state.borrow().restored, [(30, original)]);
}

#[test]
fn failed_rehide_ends_session_immediately_reaps_before_restore() {
    let (mut engine, state) = fixture(Some(10), snapshot(true, 2));
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().fail_hide = true;
    let failed = engine.rehide(10);
    // This is exactly the native supervisor's failure branch, not deferred
    // draining at normal GUI exit or a heartbeat timeout.
    let result = finish_session(
        failed,
        || {
            state.borrow_mut().steps.push("stop-and-reap");
            Ok(())
        },
        || engine.restore(),
    );
    assert_eq!(result, Err("hide readback failed"));
    assert_eq!(
        state.borrow().steps,
        ["hide", "hide", "stop-and-reap", "restore"]
    );
}

#[test]
fn partial_startup_hide_failure_still_rolls_back_and_cleanup_errors_do_not_skip_restore() {
    let (mut engine, state) = fixture(Some(10), snapshot(true, 2));
    engine.prepare().unwrap();
    state.borrow_mut().fail_hide = true;
    let failed = engine.hide_after_ready();
    let result = finish_session(failed, || Err("stop failed"), || engine.restore());
    assert_eq!(result, Err("stop failed"));
    assert_eq!(state.borrow().restored.len(), 1);
}

#[test]
fn incomplete_create_candidate_is_not_a_failed_mutation_and_show_can_capture() {
    let (mut engine, state) = fixture(None, snapshot(true, 2));
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().window = Some(10);
    state.borrow_mut().incomplete = true;
    engine.rehide(10).unwrap();
    assert!(state.borrow().hidden_flags.is_empty());
    state.borrow_mut().incomplete = false;
    engine.rehide(10).unwrap();
    assert_eq!(state.borrow().captures, 1);
    assert_eq!(state.borrow().hidden_flags, [(10, 3)]);
}

#[test]
fn failed_restore_retains_backup_for_raii_retry_and_success_is_idempotent() {
    let (mut engine, state) = fixture(Some(10), snapshot(true, 2));
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().fail_restore = true;
    assert_eq!(engine.restore(), Err("restore readback failed"));
    state.borrow_mut().fail_restore = false;
    engine.restore().unwrap();
    engine.restore().unwrap();
    assert_eq!(state.borrow().restored.len(), 2);
    assert_eq!(state.borrow().captures, 1);
}

#[test]
fn prepare_and_unready_events_do_not_hide() {
    let (mut engine, state) = fixture(Some(10), snapshot(true, 2));
    engine.prepare().unwrap();
    engine.rehide(10).unwrap();
    engine.restore().unwrap();
    assert!(state.borrow().hidden_flags.is_empty());
    assert!(state.borrow().restored.is_empty());
}

#[test]
fn disappearance_after_mutation_is_not_claimed_as_verified_restore() {
    let (mut engine, state) = fixture(Some(10), snapshot(true, 2));
    engine.prepare().unwrap();
    engine.hide_after_ready().unwrap();
    state.borrow_mut().window = None;
    assert_eq!(engine.restore(), Err("taskbar disappeared during restore"));
    state.borrow_mut().window = Some(20);
    engine.restore().unwrap();
    assert_eq!(state.borrow().restored[0].0, 20);
}

#[cfg(not(windows))]
#[test]
fn off_windows_public_native_operations_preserve_typed_unsupported_results() {
    assert!(matches!(
        super::OwnedShellSurface::attach(0, super::ShellSurfaceKind::Toolbar),
        Err(super::ShellRuntimeError::UnsupportedPlatform)
    ));
    assert!(matches!(
        super::start_desktop_session(),
        Err(super::ShellRuntimeError::UnsupportedPlatform)
    ));
    assert!(matches!(
        super::run_desktop_session(std::path::Path::new("Tessera.exe")),
        Err(super::ShellRuntimeError::UnsupportedPlatform)
    ));
    assert!(matches!(
        super::desktop_identity(),
        Err(crate::apps::ApplicationError::UnsupportedPlatform)
    ));
    assert!(matches!(
        super::clock_text(),
        Err(crate::apps::ApplicationError::UnsupportedPlatform)
    ));
}
