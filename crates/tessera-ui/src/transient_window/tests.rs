// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Real generated surfaces and the SDK testing backend exercise the production
// lifetime seam; these hooks record callbacks, never Windows/native effects.
type UiHook = Box<dyn FnOnce()>;
type ConfigureHook = Box<dyn FnOnce(&slint::Window)>;
thread_local! {
    static ANIMATION_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
    static CONFIGURE_HOOK: RefCell<Option<ConfigureHook>> = const { RefCell::new(None) };
    static LEASE_DROP_HOOK: RefCell<Option<UiHook>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Host {
    animations: AtomicBool,
    animation_queries: AtomicUsize,
    attachments: AtomicUsize,
    drops: Arc<Mutex<Vec<usize>>>,
}

struct Lease {
    id: usize,
    drops: Arc<Mutex<Vec<usize>>>,
    _ui_thread: Rc<()>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.drops.lock().push(self.id);
        let hook = LEASE_DROP_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Transient presentation must not observe desktop state")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Unexpected application activation")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("Unexpected application launch")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("Unexpected system action")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Unexpected preference save")
    }
    fn ui_animations_enabled(&self) -> bool {
        self.animation_queries.fetch_add(1, Ordering::Relaxed);
        let hook = ANIMATION_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
        self.animations.load(Ordering::Relaxed)
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert!(matches!(kind, SurfaceKind::Popup | SurfaceKind::Tooltip));
        assert!(window.is_visible());
        let id = self.attachments.fetch_add(1, Ordering::Relaxed) + 1;
        let hook = CONFIGURE_HOOK.with(|hook| hook.borrow_mut().take());
        if let Some(hook) = hook {
            hook(window);
        }
        Ok(Some(Box::new(Lease {
            id,
            drops: Arc::clone(&self.drops),
            _ui_thread: Rc::new(()),
        })))
    }
}

fn menu() -> (Arc<Host>, Rc<TransientWindow<ContextMenuSurface>>) {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host::default());
    let surface = Rc::new(TransientWindow::new(
        host.clone(),
        ContextMenuSurface::new_with_metrics().unwrap(),
        SurfaceKind::Popup,
    ));
    (host, surface)
}

fn tooltip() -> (Arc<Host>, Rc<TransientWindow<TooltipSurface>>) {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host::default());
    let surface = Rc::new(TransientWindow::new(
        host.clone(),
        TooltipSurface::new().unwrap(),
        SurfaceKind::Tooltip,
    ));
    surface.set_content("Native transient".into());
    (host, surface)
}

fn present<C: TransientComponent>(surface: &TransientWindow<C>) -> Result<bool, String> {
    surface.present(PhysicalPosition::new(40, 80), PhysicalSize::new(240, 160))
}

fn assert_presented<C: TransientComponent>(
    surface: &TransientWindow<C>,
    motion_enabled: bool,
    lease_id: usize,
) {
    assert!(surface.is_visible());
    assert!(
        surface.component.window().is_visible(),
        "SDK window must remain visible"
    );
    let motion = surface.component.motion();
    assert!(motion.get_presented());
    assert_eq!(motion.get_enabled(), motion_enabled);
    let lease = surface.lease.borrow();
    assert_eq!(
        lease.as_ref().unwrap().downcast_ref::<Lease>().unwrap().id,
        lease_id
    );
}

#[cfg(debug_assertions)]
fn assert_menu_opacity(surface: &TransientWindow<ContextMenuSurface>, expected: f32) {
    let mut bodies = i_slint_backend_testing::ElementHandle::find_by_element_id(
        &surface.component,
        "ContextMenuSurface::body",
    );
    let body = bodies.next().unwrap();
    assert!(bodies.next().is_none());
    assert_eq!(body.computed_opacity(), expected);
}

#[test]
fn lease_drop_reopen_during_hide_preserves_sdk_visibility_motion_and_new_lease() {
    let (host, surface) = menu();
    assert!(present(&surface).unwrap());
    let weak = Rc::downgrade(&surface);
    let replacement_host = Arc::clone(&host);
    LEASE_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            assert!(
                surface.component.window().is_visible(),
                "lease releases before hide"
            );
            replacement_host.animations.store(true, Ordering::Relaxed);
            surface.set_selected_index(3);
            assert!(present(&surface).unwrap());
            i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(150));
            assert_presented(&surface, true, 2);
            #[cfg(debug_assertions)]
            assert_menu_opacity(&surface, 1.0);
        }));
    });

    surface.hide();
    assert_presented(&surface, true, 2);
    assert_eq!(surface.get_selected_index(), 3);
    #[cfg(debug_assertions)]
    assert_menu_opacity(&surface, 1.0);
    assert_eq!(*host.drops.lock(), [1]);
    assert_eq!(host.attachments.load(Ordering::Relaxed), 2);
    surface.hide();
    assert!(!surface.component.window().is_visible());
    assert_eq!(*host.drops.lock(), [1, 2]);
}

#[test]
fn lease_drop_replacement_during_present_retirement_wins_without_duplicate_attachment() {
    let (host, surface) = menu();
    assert!(present(&surface).unwrap());
    let weak = Rc::downgrade(&surface);
    LEASE_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            surface.set_selected_index(2);
            assert!(
                surface
                    .present(PhysicalPosition::new(300, 400), PhysicalSize::new(320, 180))
                    .unwrap()
            );
        }));
    });

    assert!(
        !present(&surface).unwrap(),
        "retired outer presentation must lose"
    );
    assert_presented(&surface, false, 2);
    assert_eq!(surface.get_selected_index(), 2);
    assert_eq!(
        surface.component.window().size(),
        PhysicalSize::new(320, 180)
    );
    assert_eq!(host.attachments.load(Ordering::Relaxed), 2);
    assert_eq!(host.animation_queries.load(Ordering::Relaxed), 2);
    assert_eq!(*host.drops.lock(), [1]);
    surface.hide();
    assert_eq!(*host.drops.lock(), [1, 2]);
}

#[test]
fn lease_drop_hide_during_retirement_cancels_outer_present_before_metadata() {
    let (host, surface) = menu();
    assert!(present(&surface).unwrap());
    let weak = Rc::downgrade(&surface);
    LEASE_DROP_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            surface.hide();
        }));
    });
    assert!(!present(&surface).unwrap());
    assert!(!surface.is_visible());
    assert!(!surface.component.window().is_visible());
    assert!(surface.lease.borrow().is_none());
    assert_eq!(host.attachments.load(Ordering::Relaxed), 1);
    assert_eq!(host.animation_queries.load(Ordering::Relaxed), 1);
    assert_eq!(*host.drops.lock(), [1]);
}

#[test]
fn configure_cancellation_releases_late_lease_before_sdk_hide_without_resurrection() {
    let (host, surface) = tooltip();
    let weak = Rc::downgrade(&surface);
    let late_drop_observed = Rc::new(Cell::new(false));
    let observed = Rc::clone(&late_drop_observed);
    CONFIGURE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |window| {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            surface.hide();
            surface.hide();
            assert!(!surface.is_visible());
            assert!(
                window.is_visible(),
                "cancel waits for the local late attachment"
            );
            assert!(
                present(&surface)
                    .unwrap_err()
                    .contains("already in progress")
            );
            let weak = Rc::downgrade(&surface);
            LEASE_DROP_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    let surface = weak.upgrade().unwrap();
                    assert!(surface.component.window().is_visible());
                    assert!(surface.lease.try_borrow_mut().is_ok());
                    assert!(
                        present(&surface)
                            .unwrap_err()
                            .contains("already in progress")
                    );
                    surface.hide();
                    observed.set(true);
                }));
            });
        }));
    });

    assert!(!present(&surface).unwrap());
    assert!(late_drop_observed.get());
    assert!(!surface.is_visible());
    assert!(!surface.component.window().is_visible());
    assert!(!surface.component.motion().get_presented());
    assert!(surface.lease.borrow().is_none());
    assert_eq!(*host.drops.lock(), [1]);
    assert!(
        present(&surface).unwrap(),
        "cleanup must unlock a later genuine reopen"
    );
    assert_presented(&surface, false, 2);
    surface.hide();
    assert_eq!(*host.drops.lock(), [1, 2]);
}

#[test]
fn animation_metadata_cancellation_cannot_show_or_attach() {
    let (host, surface) = tooltip();
    let weak = Rc::downgrade(&surface);
    ANIMATION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            assert!(
                present(&surface)
                    .unwrap_err()
                    .contains("already in progress")
            );
            surface.hide();
            assert!(
                present(&surface)
                    .unwrap_err()
                    .contains("already in progress")
            );
        }));
    });
    assert!(!present(&surface).unwrap());
    assert!(!surface.is_visible());
    assert!(!surface.component.window().is_visible());
    assert!(surface.lease.borrow().is_none());
    assert_eq!(host.attachments.load(Ordering::Relaxed), 0);
    assert_eq!(host.animation_queries.load(Ordering::Relaxed), 1);
    assert!(host.drops.lock().is_empty());
}

#[test]
fn generated_metric_callback_during_preparation_rejects_reentrant_presentation() {
    let (host, surface) = menu();
    let weak = Rc::downgrade(&surface);
    let called = Rc::new(Cell::new(false));
    let recorded = Rc::clone(&called);
    surface.on_metric_labels(move |_, _| {
        if !recorded.replace(true) {
            let surface = weak.upgrade().unwrap();
            assert!(surface.lease.try_borrow_mut().is_ok());
            assert!(
                present(&surface)
                    .unwrap_err()
                    .contains("already in progress")
            );
            let was_shown = surface.component.window().is_visible();
            surface.hide();
            assert_eq!(surface.component.window().is_visible(), was_shown);
        }
        "Native metric".into()
    });
    // The testing backend does not eagerly evaluate layout during show. Force
    // the real generated binding while metadata preparation is still guarded.
    let preparing = Rc::downgrade(&surface);
    ANIMATION_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let surface = preparing.upgrade().unwrap();
            surface.set_kind(crate::generated::DockMenuKind::Window);
            assert!(surface.get_menu_width() > 0.0);
        }));
    });
    assert!(!present(&surface).unwrap());
    assert!(
        called.get(),
        "genuine generated layout must cross the callback seam"
    );
    assert!(!surface.is_visible());
    assert!(!surface.component.window().is_visible());
    assert!(surface.lease.borrow().is_none());
    assert_eq!(host.attachments.load(Ordering::Relaxed), 0);
    assert!(host.drops.lock().is_empty());
}

#[test]
fn checked_operation_exhaustion_preserves_visible_lease_and_still_allows_hide() {
    let (host, surface) = menu();
    assert!(present(&surface).unwrap());
    surface.lifecycle.set(Lifecycle {
        operation: u64::MAX - 1,
        visibility: Visibility::Visible,
    });
    assert!(present(&surface).unwrap());
    assert_eq!(surface.lifecycle.get().operation, u64::MAX);
    let state = surface.lifecycle.get();
    assert!(
        present(&surface)
            .unwrap_err()
            .contains("identity is exhausted")
    );
    assert_eq!(surface.lifecycle.get(), state);
    assert_presented(&surface, false, 2);
    assert_eq!(host.attachments.load(Ordering::Relaxed), 2);
    assert_eq!(host.animation_queries.load(Ordering::Relaxed), 2);
    assert_eq!(*host.drops.lock(), [1]);
    surface.hide();
    assert!(!surface.component.window().is_visible());
    assert!(surface.lease.borrow().is_none());
    assert!(
        present(&surface)
            .unwrap_err()
            .contains("identity is exhausted")
    );
    assert_eq!(surface.lifecycle.get().operation, u64::MAX);
    assert_eq!(*host.drops.lock(), [1, 2]);
}
