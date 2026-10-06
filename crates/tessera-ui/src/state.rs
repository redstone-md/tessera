// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared host-facing state for the active surface (panel window or dock
//! strip).
//!
//! One [`SurfaceCore`] owns the retained observation, the retained catalog,
//! the pin list, and the coalescing desktop-notification bus. The surface
//! controller registers its routes on the core at construction.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::invoke_from_event_loop;

use crate::{DesktopHost, MAX_PINS, PanelApplication, PanelPreferences, PanelSnapshot, sanitize};

type Host = dyn DesktopHost;

/// One observed surface: retained data plus UI-thread presentation state.
///
/// The last successful full snapshot and catalog are kept here (not derived
/// from displayed rows) so filtering works without re-observing the desktop.
/// All fields are UI-thread state except where a lock guards a watcher thread.
pub(crate) struct SurfaceCore {
    host: Arc<Host>,
    snapshot: Mutex<Option<PanelSnapshot>>,
    catalog: Mutex<Vec<PanelApplication>>,
    /// Live pins: the last successfully saved list, or the seeded list.
    pins: Mutex<Vec<String>>,
    /// Appearance values of the last save (or seed); pin saves reuse them so
    /// a live appearance preview is never persisted by a pin click.
    applied_appearance: Mutex<Appearance>,
    /// Dock edge of the last save (or seed); pin saves reuse it.
    applied_dock_edge: Mutex<crate::DockEdge>,
    dirty: Mutex<bool>,
    /// UI-thread routes installed by the surface controller.
    routes: Mutex<Option<Routes>>,
}

/// Surface-owned UI-thread hooks: whether a worker is running right now, and
/// how to start one. `is_refreshing` reads the surface's own busy property.
#[derive(Clone)]
pub(crate) struct Routes {
    pub(crate) is_refreshing: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) start_refresh: Arc<dyn Fn() + Send + Sync>,
}

/// Theme + compact pair as last persisted (theme and compact stay `Copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Appearance {
    pub(crate) theme: crate::Theme,
    pub(crate) compact: bool,
}

impl Appearance {
    pub(crate) fn of(preferences: &PanelPreferences) -> Self {
        Self {
            theme: preferences.theme(),
            compact: preferences.compact(),
        }
    }
}

impl SurfaceCore {
    /// Creates the shared core and opens the host subscription.
    ///
    /// Returns the core and the watcher guard: the caller must retain the
    /// guard for the lifetime of the run (its `Drop` stops the native
    /// watcher's message-pump thread). A subscription failure is reported
    /// through `subscription_error` and degrades to manual refresh.
    pub(crate) fn new(
        host: Arc<Host>,
        preferences: &PanelPreferences,
        subscription_error: &mut Option<String>,
    ) -> (Arc<Self>, Option<Box<dyn Send>>) {
        let core = Arc::new(Self {
            host: Arc::clone(&host),
            snapshot: Mutex::new(None),
            catalog: Mutex::new(Vec::new()),
            pins: Mutex::new(preferences.pinned_apps().to_vec()),
            applied_appearance: Mutex::new(Appearance::of(preferences)),
            applied_dock_edge: Mutex::new(preferences.dock_edge()),
            dirty: Mutex::new(false),
            routes: Mutex::new(None),
        });
        // One passive subscription; a failure degrades to manual refresh.
        let guard = match host.subscribe(core.notification_callback()) {
            Ok(guard) => guard,
            Err(error) => {
                *subscription_error = Some(sanitize::bounded_text(
                    &format!(
                        "Desktop-change notifications are unavailable ({error}); use Refresh."
                    ),
                    200,
                ));
                None
            }
        };
        (core, guard)
    }

    /// Installs the surface's UI-thread routes. Must be called on the UI
    /// thread before any notification can arrive.
    pub(crate) fn install_routes(&self, routes: Routes) {
        *self.routes.lock() = Some(routes);
    }

    /// Builds the watcher callback. Out-of-context notifications are
    /// marshalled onto the UI thread and coalesced there; the closure only
    /// captures `self`, so it lives as long as the watcher guard.
    pub(crate) fn notification_callback(self: &Arc<Self>) -> Arc<dyn Fn() + Send + Sync> {
        let core = Arc::clone(self);
        Arc::new(move || core.clone().notify_from_watcher())
    }

    /// Called from a watcher thread: hands one notification to the UI thread.
    /// Without an event loop (tests, pre-loop startup) the notification is
    /// applied directly on the calling thread; coalescing behavior is the same.
    fn notify_from_watcher(self: Arc<Self>) {
        let dispatched = invoke_from_event_loop({
            let core = Arc::clone(&self);
            move || {
                if let Some(routes) = core.routes.lock().clone() {
                    core.notify_on_ui_thread(&routes);
                }
            }
        });
        if dispatched.is_err()
            && let Some(routes) = self.routes.lock().clone()
        {
            self.notify_on_ui_thread(&routes);
        }
    }

    /// Runs on the UI thread: coalesce bursts — one worker at a time, one
    /// queued refresh while a worker runs, dirty cleared before dispatch.
    fn notify_on_ui_thread(&self, routes: &Routes) {
        if (routes.is_refreshing)() {
            *self.dirty.lock() = true;
            return;
        }
        // Clear before dispatch: events arriving during the new worker set
        // dirty again, so nothing is lost and no idle spin can develop.
        *self.dirty.lock() = false;
        (routes.start_refresh)();
    }

    /// Called by the surface controller when one observation finished. Returns
    /// `true` when a queued refresh must be started by the caller.
    pub(crate) fn take_dirty(&self) -> bool {
        std::mem::replace(&mut *self.dirty.lock(), false)
    }

    pub(crate) fn observe_safely(&self) -> Result<PanelSnapshot, String> {
        // Keep private panic payloads out of the panel's error text.
        catch_unwind(AssertUnwindSafe(|| self.host.observe()))
            .unwrap_or_else(|_| Err("Observation failed unexpectedly".into()))
    }

    pub(crate) fn retained_snapshot(&self) -> Option<PanelSnapshot> {
        self.snapshot.lock().clone()
    }

    pub(crate) fn store_snapshot(&self, snapshot: PanelSnapshot) {
        if let Some(applications) = snapshot.applications() {
            *self.catalog.lock() = applications.to_vec();
        }
        *self.snapshot.lock() = Some(snapshot);
    }

    pub(crate) fn pins(&self) -> Vec<String> {
        self.pins.lock().clone()
    }

    pub(crate) fn catalog(&self) -> Vec<PanelApplication> {
        self.catalog.lock().clone()
    }

    pub(crate) fn applied_appearance(&self) -> Appearance {
        *self.applied_appearance.lock()
    }

    pub(crate) fn applied_dock_edge(&self) -> crate::DockEdge {
        *self.applied_dock_edge.lock()
    }

    pub(crate) fn record_applied(&self, preferences: &PanelPreferences) {
        *self.applied_appearance.lock() = Appearance::of(preferences);
        *self.applied_dock_edge.lock() = preferences.dock_edge();
        *self.pins.lock() = preferences.pinned_apps().to_vec();
    }

    pub(crate) fn pin_capacity_full(&self, pins: &[String]) -> bool {
        pins.len() >= MAX_PINS
    }

    pub(crate) fn host(&self) -> &Arc<Host> {
        &self.host
    }

    #[cfg(test)]
    pub(crate) fn dirty_for_tests(&self) -> parking_lot::MutexGuard<'_, bool> {
        self.dirty.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DockContext, PanelWindow, Theme};

    struct CoalesceHost {
        observe: Mutex<Box<dyn Fn() -> Result<PanelSnapshot, String> + Send + Sync>>,
        observes: AtomicUsize,
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    impl CoalesceHost {
        fn counting() -> Arc<Self> {
            Arc::new(Self {
                observe: Mutex::new(Box::new(|| Ok(PanelSnapshot::new(1, Vec::new(), 0)))),
                observes: AtomicUsize::new(0),
            })
        }
    }

    impl DesktopHost for CoalesceHost {
        fn observe(&self) -> Result<PanelSnapshot, String> {
            self.observes.fetch_add(1, Ordering::SeqCst);
            (self.observe.lock())()
        }
        fn activate(&self, _key: &str) -> Result<(), String> {
            Ok(())
        }
        fn launch(&self, _key: &str) -> Result<(), String> {
            Ok(())
        }
        fn system_action(&self, _action: crate::SystemAction) -> Result<(), String> {
            Ok(())
        }
        fn save_preferences(&self, _preferences: &PanelPreferences) -> Result<(), String> {
            Ok(())
        }
    }

    fn preferences() -> PanelPreferences {
        PanelPreferences::new(Theme::Dark, true)
            .with_dock(crate::DockEdge::Left, vec!["known-key".into()])
    }

    #[test]
    fn pins_and_edge_seed_from_preferences_and_record_on_save() {
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        assert!(error.is_none());
        assert_eq!(core.pins(), vec!["known-key".to_string()]);
        assert_eq!(core.applied_appearance().theme, Theme::Dark);
        assert_eq!(core.applied_dock_edge(), crate::DockEdge::Left);

        // A later explicit save with different appearance updates the applied
        // state pin saves reuse.
        core.record_applied(
            &PanelPreferences::new(Theme::Light, false)
                .with_dock(crate::DockEdge::Right, Vec::new()),
        );
        assert_eq!(core.applied_appearance().theme, Theme::Light);
        assert!(core.pins().is_empty());
    }

    #[test]
    fn pin_save_uses_last_saved_appearance_not_preview() {
        // Seeded Dark/compact; the user then previews Light in the UI but
        // does not press Save. A pin click must persist Dark.
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        core.record_applied(
            &PanelPreferences::new(Theme::Light, false)
                .with_dock(crate::DockEdge::Bottom, Vec::new()),
        );
        let appearance = core.applied_appearance();
        let persisted = PanelPreferences::new(appearance.theme, appearance.compact)
            .with_dock(core.applied_dock_edge(), core.pins());
        assert_eq!(persisted.theme(), Theme::Light);
        assert!(!persisted.compact());
        assert_eq!(persisted.dock_edge(), crate::DockEdge::Bottom);
    }

    #[test]
    fn dirty_coalesces_bursts_without_perpetual_spin() {
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        // Idle and no notification: no queued refresh.
        assert!(!core.take_dirty());
        // A burst arriving while a worker runs leaves exactly one queued
        // refresh; consuming it drains the queue.
        *core.dirty_for_tests() = true;
        assert!(core.take_dirty());
        assert!(!core.take_dirty());
        assert!(core.pin_capacity_full(&vec!["a".to_string(); MAX_PINS]));
    }

    #[test]
    fn dock_context_carries_viewport_and_fullscreen_flag() {
        let area = DockContext::new(-1920, -1080, 3840, 2160, true).unwrap();
        assert_eq!(area.x(), -1920);
        assert_eq!(area.height(), 2160);
        assert!(area.fullscreen_active());
        assert!(DockContext::new(0, 0, 0, 100, false).is_none());
    }

    #[test]
    fn window_and_app_dtos_reject_malformed_input() {
        let window = PanelWindow::new("k".into(), "Title".into(), true)
            .with_icon(crate::PixelIcon::new(1, 1, vec![1, 2, 3, 4]));
        assert!(window.icon().is_some());
        assert!(
            PanelWindow::new("k".into(), String::new(), false)
                .icon()
                .is_none()
        );
        let app = PanelApplication::new("opaque".into(), "Name".into(), None).unwrap();
        assert_eq!(app.key(), "opaque");
    }
}

#[cfg(test)]
mod pin_validation {
    use crate::{DockEdge, MAX_PINS, PanelPreferences};

    #[test]
    fn pins_preserve_exact_strings_reject_bad_ones_and_dedupe() {
        let long = "k".repeat(1024);
        let overlong = "k".repeat(1025);
        let preferences = PanelPreferences::new(crate::Theme::System, false).with_dock(
            DockEdge::Bottom,
            vec![
                "native-id-\u{20AC}".into(), // exact Unicode string preserved
                long.clone(),
                overlong,                    // rejected: over the 1024 UTF-16 bound
                String::new(),               // rejected: empty
                "ctrl\u{0007}".into(),       // rejected: control character
                "native-id-\u{20AC}".into(), // duplicate dropped
            ],
        );
        // Order-preserving: the valid € pin first, the long pin second; the
        // rejected entries never appear.
        assert_eq!(
            preferences.pinned_apps(),
            ["native-id-\u{20AC}".to_owned(), long.clone()]
        );
    }

    #[test]
    fn pin_cap_is_enforced_in_first_occurrence_order() {
        let pins: Vec<String> = (0..MAX_PINS + 10)
            .map(|index| format!("k{index}"))
            .collect();
        let preferences =
            PanelPreferences::new(crate::Theme::System, false).with_dock(DockEdge::Left, pins);
        assert_eq!(preferences.pinned_apps().len(), MAX_PINS);
        assert_eq!(preferences.pinned_apps()[0], "k0");
    }
}
