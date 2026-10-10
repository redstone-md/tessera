// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared host-facing state for the active surface (panel window or dock
//! strip).
//!
//! One [`SurfaceCore`] owns the retained observation, the retained catalog,
//! the complete applied preferences, and the coalescing notification bus.
//! The surface controller registers its routes on the core at construction.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use parking_lot::Mutex;
use slint::invoke_from_event_loop;

use crate::{
    DesktopHost, DockContext, MAX_PINS, PanelApplication, PanelPreferences, PanelSnapshot, sanitize,
};

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
    /// Complete last successfully saved record (or seed). Immediate collection
    /// saves clone it, so they cannot accidentally persist appearance previews.
    applied_preferences: Mutex<PanelPreferences>,
    dirty: Mutex<bool>,
    /// UI-thread routes installed by the surface controller.
    routes: Mutex<Option<Routes>>,
    /// A Send-safe signal containing only the weak component handle.
    apply_route: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Single-flight completion waiting for its UI-thread component callback.
    pending_observation: Mutex<Option<Result<PanelSnapshot, String>>>,
    /// Last host-reported identity text; refreshed with each observation.
    identity: Mutex<crate::ShellIdentity>,
}

/// Surface-owned UI-thread hooks: whether a worker is running right now, and
/// how to start one. `is_refreshing` reads the surface's own busy property.
#[derive(Clone)]
pub(crate) struct Routes {
    pub(crate) is_refreshing: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) start_refresh: Arc<dyn Fn() + Send + Sync>,
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
            applied_preferences: Mutex::new(preferences.clone()),
            dirty: Mutex::new(false),
            routes: Mutex::new(None),
            apply_route: Mutex::new(None),
            pending_observation: Mutex::new(None),
            identity: Mutex::new(crate::ShellIdentity::default()),
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

    /// Installs the surface's UI-thread routes before its first refresh.
    /// Notifications received earlier remain dirty for one follow-up refresh.
    pub(crate) fn install_routes(&self, routes: Routes) {
        *self.routes.lock() = Some(routes);
    }

    /// Installs the UI-thread observation-completion route. Must be called
    /// on the UI thread before any worker can finish.
    pub(crate) fn install_apply_route(&self, route: Arc<dyn Fn() + Send + Sync>) {
        *self.apply_route.lock() = Some(route);
    }

    /// Store one single-flight result, then signal its UI-thread callback.
    /// No controller, component state or native lease enters the worker closure.
    pub(crate) fn apply_observation(self: &Arc<Self>, result: Result<PanelSnapshot, String>) {
        *self.pending_observation.lock() = Some(result);
        if let Some(route) = self.apply_route.lock().clone() {
            let dispatched = invoke_from_event_loop({
                let route = Arc::clone(&route);
                move || route()
            });
            if dispatched.is_err() {
                // A weak component refuses upgrades from a different thread.
                // Direct delivery is useful only for same-thread startup errors.
                route();
            }
        }
    }

    pub(crate) fn take_observation(&self) -> Option<Result<PanelSnapshot, String>> {
        self.pending_observation.lock().take()
    }

    /// Builds the watcher callback. Out-of-context notifications are
    /// marshalled onto the UI thread and coalesced there; the closure only
    /// captures `self`, so it lives as long as the watcher guard.
    pub(crate) fn notification_callback(self: &Arc<Self>) -> Arc<dyn Fn() + Send + Sync> {
        let core = Arc::clone(self);
        Arc::new(move || core.clone().notify_from_watcher())
    }

    /// Pre-route notifications mark dirty at arrival, before queued delivery.
    /// Once routes exist, marshal to the UI thread; a missing dispatcher falls
    /// back to the same coalescing path without moving any component handles.
    fn notify_from_watcher(self: Arc<Self>) {
        if self.routes.lock().is_none() {
            *self.dirty.lock() = true;
            return;
        }
        let dispatched = invoke_from_event_loop({
            let core = Arc::clone(&self);
            move || core.dispatch_notification()
        });
        if dispatched.is_err() {
            self.dispatch_notification();
        }
    }

    fn dispatch_notification(&self) {
        let routes = self.routes.lock().clone();
        if let Some(routes) = routes {
            self.notify_on_ui_thread(&routes);
        } else {
            *self.dirty.lock() = true;
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

    pub(crate) fn dock_context(&self) -> Option<DockContext> {
        self.snapshot
            .lock()
            .as_ref()
            .and_then(PanelSnapshot::dock_context)
    }

    pub(crate) fn store_snapshot(&self, snapshot: PanelSnapshot) {
        if let Some(applications) = snapshot.applications() {
            *self.catalog.lock() = applications.to_vec();
        }
        *self.snapshot.lock() = Some(snapshot);
    }

    pub(crate) fn pins(&self) -> Vec<String> {
        self.applied_preferences.lock().pinned_apps().to_vec()
    }

    pub(crate) fn launcher_favorites(&self) -> Vec<String> {
        self.applied_preferences
            .lock()
            .launcher_favorites()
            .to_vec()
    }

    pub(crate) fn applied_preferences(&self) -> PanelPreferences {
        self.applied_preferences.lock().clone()
    }

    pub(crate) fn catalog(&self) -> Vec<PanelApplication> {
        self.catalog.lock().clone()
    }

    pub(crate) fn applied_dock_edge(&self) -> crate::DockEdge {
        self.applied_preferences.lock().dock_edge()
    }

    /// Commits the whole record only after the caller's host save succeeds.
    pub(crate) fn record_applied(&self, preferences: &PanelPreferences) {
        *self.applied_preferences.lock() = preferences.clone();
    }

    pub(crate) fn pin_capacity_full(&self, pins: &[String]) -> bool {
        pins.len() >= MAX_PINS
    }

    /// Refreshes identity from the host (best-effort: an error keeps the
    /// previous text; only bounded genuine values are stored).
    pub(crate) fn refresh_identity(&self) {
        let identity = self
            .host
            .shell_identity()
            .unwrap_or_else(|_| self.identity.lock().clone());
        *self.identity.lock() = crate::ShellIdentity {
            user_name: sanitize::bounded_text(&identity.user_name, 64),
            clock: sanitize::bounded_text(&identity.clock, 64),
            language: sanitize::bounded_text(&identity.language, 32),
            focused_window_key: identity.focused_window_key,
        };
    }

    pub(crate) fn identity(&self) -> crate::ShellIdentity {
        self.identity.lock().clone()
    }

    /// Explicit clock-only refresh (timer path, at most once per minute);
    /// never queries windows, foreground, or any other identity input.
    pub(crate) fn refresh_clock(&self) -> Option<String> {
        self.host.clock_text().ok().map(|clock| {
            let clock = sanitize::bounded_text(&clock, 64);
            self.identity.lock().clock = clock.clone();
            clock
        })
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
            .with_source_seed(crate::SourceSeed::from_rgb(0x123456).unwrap())
            .with_dock(crate::DockEdge::Left, vec!["known-key".into()])
            .with_launcher_favorites(vec!["favorite-only".into(), "known-key".into()])
            .unwrap()
    }

    #[test]
    fn pins_and_edge_seed_from_preferences_and_record_on_save() {
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        assert!(error.is_none());
        assert_eq!(core.pins(), vec!["known-key".to_string()]);
        assert_eq!(core.applied_preferences(), preferences());
        assert_eq!(core.launcher_favorites(), ["favorite-only", "known-key"]);
        assert_eq!(core.applied_preferences().theme(), Theme::Dark);
        assert_eq!(core.applied_dock_edge(), crate::DockEdge::Left);

        // A successful explicit save replaces the complete record.
        core.record_applied(
            &PanelPreferences::new(Theme::Light, false)
                .with_dock(crate::DockEdge::Right, Vec::new())
                .with_launcher_favorites(vec!["new-favorite".into()])
                .unwrap(),
        );
        assert_eq!(core.applied_preferences().theme(), Theme::Light);
        assert!(core.pins().is_empty());
        assert_eq!(core.launcher_favorites(), ["new-favorite"]);
        assert_eq!(
            core.applied_preferences(),
            PanelPreferences::new(Theme::Light, false)
                .with_dock(crate::DockEdge::Right, Vec::new())
                .with_launcher_favorites(vec!["new-favorite".into()])
                .unwrap()
        );
    }

    #[test]
    fn immediate_collection_save_clones_full_record_without_persisting_preview() {
        // A live appearance preview does not change the complete applied seed.
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        let persisted = core
            .applied_preferences()
            .with_dock(core.applied_dock_edge(), vec!["new-pin".into()]);
        assert_eq!(persisted.theme(), Theme::Dark);
        assert_eq!(persisted.source_seed().rgb(), 0x123456);
        assert!(persisted.compact());
        assert_eq!(persisted.dock_edge(), crate::DockEdge::Left);
        assert_eq!(
            persisted.launcher_favorites(),
            preferences().launcher_favorites()
        );
        assert_eq!(core.applied_preferences(), preferences());
    }

    #[test]
    fn catalog_retains_full_inventory_until_explicit_empty_snapshot() {
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        let catalog: Vec<_> = (0..80)
            .map(|index| {
                PanelApplication::new(format!("k{index}"), format!("App {index}"), None).unwrap()
            })
            .collect();
        core.store_snapshot(
            PanelSnapshot::new(1, Vec::new(), 0).with_applications(catalog.clone()),
        );
        assert_eq!(core.catalog(), catalog);
        core.store_snapshot(PanelSnapshot::new(1, Vec::new(), 0));
        assert_eq!(core.catalog(), catalog);
        core.store_snapshot(PanelSnapshot::new(1, Vec::new(), 0).with_applications(Vec::new()));
        assert!(core.catalog().is_empty());
        assert_eq!(
            core.launcher_favorites(),
            preferences().launcher_favorites()
        );
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
    fn notifications_before_routes_survive_as_one_follow_up() {
        let mut error = None;
        let (core, _guard) = SurfaceCore::new(CoalesceHost::counting(), &preferences(), &mut error);
        // Subscription can deliver a burst before UI construction completes.
        for _ in 0..3 {
            core.clone().notify_from_watcher();
        }
        let refreshes = Arc::new(AtomicUsize::new(0));
        let requested = Arc::clone(&refreshes);
        core.install_routes(Routes {
            is_refreshing: Arc::new(|| true),
            start_refresh: Arc::new(move || {
                requested.fetch_add(1, Ordering::SeqCst);
            }),
        });
        assert_eq!(refreshes.load(Ordering::SeqCst), 0);
        assert!(core.take_dirty());
        assert!(!core.take_dirty());
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
