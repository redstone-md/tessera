// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Portable tests for the window-action pipeline, driven through the same
//! dispatch seam the native adapter uses, via an in-memory recording fixture.

use super::dispatch::{Dispatcher, run};
#[cfg(not(windows))]
use super::window_action;
use super::{WindowAction, WindowActionError};
use crate::activation::{ActivationError, ActivationTarget};
use tessera_core::WindowId;

const TARGET_PID: u32 = 4321;
const HWND_VALUE: u64 = 0x0000_0001_0000_1234;

fn target() -> ActivationTarget {
    ActivationTarget::new(WindowId::new(HWND_VALUE), TARGET_PID)
}

#[derive(Default)]
struct Fixture {
    foreground: Option<WindowId>,
    minimized: bool,
    minimize_count: usize,
    activate_count: usize,
    close_count: usize,
    /// When set, every effect is denied with this last-error code.
    deny_effects: Option<u32>,
    /// When set, live-state queries report this identity mismatch.
    break_live: bool,
}

impl Fixture {
    /// Applies live validation the way the native adapters do: every effect
    /// boundary first checks the live identity.
    fn validated(&self) -> Result<(), WindowActionError> {
        if self.break_live {
            return Err(WindowActionError::Unavailable);
        }
        Ok(())
    }
}

impl Dispatcher for Fixture {
    fn foreground(&self) -> Option<WindowId> {
        self.foreground
    }

    fn is_minimized(&self, _target: &ActivationTarget) -> Result<bool, WindowActionError> {
        if self.break_live {
            return Err(WindowActionError::Unavailable);
        }
        Ok(self.minimized)
    }

    fn activate(&mut self, _target: ActivationTarget) -> Result<(), WindowActionError> {
        self.validated()?;
        if let Some(code) = self.deny_effects {
            return Err(WindowActionError::NativeRequestDenied { code });
        }
        self.activate_count += 1;
        Ok(())
    }

    fn minimize(&mut self, _target: &ActivationTarget) -> Result<(), WindowActionError> {
        self.validated()?;
        if let Some(code) = self.deny_effects {
            return Err(WindowActionError::NativeRequestDenied { code });
        }
        self.minimize_count += 1;
        Ok(())
    }

    fn close(&mut self, _target: &ActivationTarget) -> Result<(), WindowActionError> {
        self.validated()?;
        if let Some(code) = self.deny_effects {
            return Err(WindowActionError::NativeRequestDenied { code });
        }
        self.close_count += 1;
        Ok(())
    }
}

fn apply(fixture: &mut Fixture, action: WindowAction) -> Result<(), WindowActionError> {
    run(fixture, target(), action)
}

#[test]
fn every_action_validates_live_state_before_any_effect() {
    // Foreground matches so the toggle reaches the live-state query; a
    // mismatch would route through activate, which validates itself.
    let mut fixture = Fixture {
        foreground: Some(WindowId::new(HWND_VALUE)),
        break_live: true,
        ..Fixture::default()
    };
    assert!(
        matches!(
            apply(&mut fixture, WindowAction::ActivateOrMinimize),
            Err(WindowActionError::Unavailable)
        ),
        "toggle must fail live validation"
    );
    for action in [
        WindowAction::Activate,
        WindowAction::Minimize,
        WindowAction::Close,
    ] {
        assert!(
            matches!(
                apply(&mut fixture, action),
                Err(WindowActionError::Unavailable)
            ),
            "{action:?} must fail at the effect boundary"
        );
    }
    assert_eq!(
        (
            fixture.minimize_count,
            fixture.activate_count,
            fixture.close_count
        ),
        (0, 0, 0)
    );
}

#[test]
fn foreground_restored_window_toggles_to_minimize() {
    let mut fixture = Fixture {
        foreground: Some(WindowId::new(HWND_VALUE)),
        minimized: false,
        ..Fixture::default()
    };
    apply(&mut fixture, WindowAction::ActivateOrMinimize).expect("minimize");
    assert_eq!(fixture.minimize_count, 1);
    assert_eq!(fixture.activate_count, 0);
}

#[test]
fn context_menu_activation_never_minimizes_a_focused_window() {
    let mut fixture = Fixture {
        foreground: Some(WindowId::new(HWND_VALUE)),
        ..Fixture::default()
    };
    apply(&mut fixture, WindowAction::Activate).expect("activate");
    assert_eq!(fixture.activate_count, 1);
    assert_eq!(fixture.minimize_count, 0);
}

#[test]
fn unfocused_toggle_activates() {
    let mut fixture = Fixture {
        foreground: Some(WindowId::new(999)),
        minimized: false,
        ..Fixture::default()
    };
    apply(&mut fixture, WindowAction::ActivateOrMinimize).expect("activate");
    assert_eq!(fixture.activate_count, 1);
    assert_eq!(fixture.minimize_count, 0);
}

#[test]
fn minimized_toggle_activates_even_if_reported_foreground() {
    let mut fixture = Fixture {
        foreground: Some(WindowId::new(HWND_VALUE)),
        minimized: true,
        ..Fixture::default()
    };
    apply(&mut fixture, WindowAction::ActivateOrMinimize).expect("activate");
    assert_eq!(fixture.activate_count, 1);
    assert_eq!(fixture.minimize_count, 0);
}

#[test]
fn minimize_and_close_are_recorded_as_requests() {
    let mut fixture = Fixture::default();
    apply(&mut fixture, WindowAction::Minimize).expect("minimize");
    apply(&mut fixture, WindowAction::Close).expect("close");
    assert_eq!(fixture.minimize_count, 1);
    assert_eq!(fixture.close_count, 1);
    assert_eq!(fixture.activate_count, 0);
}

#[test]
fn denied_native_operation_returns_error_not_success() {
    let mut fixture = Fixture {
        deny_effects: Some(5),
        ..Fixture::default()
    };
    assert!(matches!(
        apply(&mut fixture, WindowAction::Close),
        Err(WindowActionError::NativeRequestDenied { code: 5 })
    ));
    assert!(matches!(
        apply(&mut fixture, WindowAction::Minimize),
        Err(WindowActionError::NativeRequestDenied { code: 5 })
    ));
    assert_eq!((fixture.minimize_count, fixture.close_count), (0, 0));
}

#[test]
fn foreground_denied_activation_preserves_native_error_code() {
    let mut fixture = Fixture {
        deny_effects: Some(0),
        ..Fixture::default()
    };
    let error = WindowActionError::from(ActivationError::ForegroundDenied { code: 0 });
    assert!(matches!(
        error,
        WindowActionError::NativeRequestDenied { code: 0 }
    ));
    assert!(matches!(
        apply(&mut fixture, WindowAction::ActivateOrMinimize),
        Err(WindowActionError::NativeRequestDenied { code: 0 })
    ));
    assert_eq!(fixture.activate_count, 0);
}

#[cfg(not(windows))]
#[test]
fn off_windows_api_reports_unsupported_not_noop() {
    assert!(matches!(
        window_action(target(), WindowAction::Close),
        Err(WindowActionError::UnsupportedPlatform)
    ));
}
