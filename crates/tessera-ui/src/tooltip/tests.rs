// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::{Dock, Palette, PopoverMotion, SeelenPalette};
use crate::{PanelPreferences, PanelSnapshot, SystemAction};
use parking_lot::Mutex;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Host {
    events: Arc<Mutex<Vec<&'static str>>>,
    deny_attach: AtomicBool,
    close_during_attach: AtomicBool,
    allow_motion: AtomicBool,
}

struct Lease(Arc<Mutex<Vec<&'static str>>>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.lock().push("detach");
    }
}

impl DesktopHost for Host {
    fn observe(&self) -> Result<PanelSnapshot, String> {
        panic!("Hover must not observe the desktop")
    }
    fn activate(&self, _: &str) -> Result<(), String> {
        panic!("Hover must not activate windows")
    }
    fn launch(&self, _: &str) -> Result<(), String> {
        panic!("Hover must not launch applications")
    }
    fn system_action(&self, _: SystemAction) -> Result<(), String> {
        panic!("Hover must not dispatch system actions")
    }
    fn save_preferences(&self, _: &PanelPreferences) -> Result<(), String> {
        panic!("Hover must not save preferences")
    }
    fn request_ui_focus(&self, _: &slint::Window) -> Result<(), String> {
        panic!("Tooltip must never request focus")
    }
    fn ui_animations_enabled(&self) -> bool {
        self.allow_motion.load(Ordering::Relaxed)
    }
    fn configure_surface(
        &self,
        kind: SurfaceKind,
        window: &slint::Window,
    ) -> Result<Option<Box<dyn std::any::Any>>, String> {
        assert_eq!(kind, SurfaceKind::Tooltip);
        assert!(window.is_visible());
        if self.deny_attach.load(Ordering::Relaxed) {
            return Err("Native attachment denied".into());
        }
        self.events.lock().push("attach");
        if self.close_during_attach.load(Ordering::Relaxed) {
            window.dispatch_event(slint::platform::WindowEvent::CloseRequested);
            assert!(
                window.is_visible(),
                "in-flight native attachment keeps its HWND alive until its local lease is released"
            );
        }
        Ok(Some(Box::new(Lease(Arc::clone(&self.events)))))
    }
}

fn advance(milliseconds: u64) {
    i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(milliseconds));
    slint::platform::update_timers_and_animations();
}

fn bounds() -> TileBounds {
    TileBounds {
        origin: slint::LogicalPosition::new(16.0, 16.0),
        width: 40.0,
        height: 40.0,
    }
}

#[test]
fn delay_uses_current_owner_theme_and_scale_then_leave_releases_without_focus() {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host::default());
    let owner = Dock::new().unwrap();
    owner.show().unwrap();
    let tooltip = TooltipController::new(host.clone(), Rc::new(|error| panic!("{error}"))).unwrap();
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "Editor — full label",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(99);
    assert!(!tooltip.surface.is_visible());
    assert!(host.events.lock().is_empty());
    host.allow_motion.store(true, Ordering::Relaxed);
    owner
        .global::<Palette>()
        .set_color_scheme(slint::language::ColorScheme::Light);
    owner
        .global::<SeelenPalette>()
        .set_color_scheme(slint::language::ColorScheme::Dark);
    owner
        .window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: 1.5 });
    advance(1);
    assert!(tooltip.surface.is_visible());
    let expected = placement::place(
        context,
        placement::tile_rect(owner.window().position(), 1.5, &bounds()).unwrap(),
        (
            tooltip.surface.get_tooltip_width(),
            tooltip.surface.get_tooltip_height(),
        ),
        1.5,
        Side::Top,
    )
    .unwrap();
    assert_eq!(tooltip.window().size(), expected.size);
    assert_eq!(tooltip.surface.get_content(), "Editor — full label");
    assert_eq!(
        tooltip.surface.global::<Palette>().get_color_scheme(),
        slint::language::ColorScheme::Light
    );
    assert_eq!(
        tooltip.surface.global::<SeelenPalette>().get_color_scheme(),
        slint::language::ColorScheme::Dark
    );
    let motion = tooltip.surface.global::<PopoverMotion>();
    assert!(motion.get_presented());
    assert!(
        motion.get_enabled(),
        "permission is read at presentation, not scheduling"
    );
    tooltip.dismiss(true);
    advance(99);
    assert!(tooltip.surface.is_visible());
    advance(1);
    assert!(!tooltip.surface.is_visible());
    assert!(!motion.get_presented());
    assert!(
        !motion.get_enabled(),
        "dismissal cancels optional motion immediately"
    );
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
}

#[test]
fn pending_hover_cancels_and_native_failure_recovers_without_retaining_controller() {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host::default());
    let owner = Dock::new().unwrap();
    owner.show().unwrap();
    let errors = Rc::new(RefCell::new(Vec::new()));
    let messages = Rc::clone(&errors);
    let tooltip = TooltipController::new(
        host.clone(),
        Rc::new(move |error| messages.borrow_mut().push(error)),
    )
    .unwrap();
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "old",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(50);
    let newer = TileBounds {
        origin: slint::LogicalPosition::new(64.0, 16.0),
        ..bounds()
    };
    tooltip
        .schedule(&owner, SurfaceKind::Dock, "new", newer, context, Side::Top)
        .unwrap();
    tooltip.dismiss_for(SurfaceKind::Dock, bounds().origin, true);
    advance(50);
    assert!(!tooltip.surface.is_visible());
    advance(50);
    assert_eq!(tooltip.surface.get_content(), "new");
    assert!(
        tooltip.surface.is_visible(),
        "a stale leave must not cancel the newer hover"
    );
    tooltip.dismiss(false);
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "hidden",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    owner.hide().unwrap();
    advance(100);
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
    owner.show().unwrap();
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "cancelled",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    tooltip.dismiss(false);
    advance(100);
    let fullscreen = DockContext::new(0, 0, 1920, 1080, true).unwrap();
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "fullscreen",
            bounds(),
            fullscreen,
            Side::Top,
        )
        .unwrap();
    advance(100);
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
    host.deny_attach.store(true, Ordering::Relaxed);
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "denied",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(100);
    assert!(!tooltip.surface.is_visible());
    assert_eq!(
        errors.borrow().as_slice(),
        &["Tooltip: Native attachment denied"]
    );
    host.deny_attach.store(false, Ordering::Relaxed);
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "recovered",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(100);
    assert_eq!(tooltip.surface.get_content(), "recovered");
    assert!(tooltip.surface.is_visible());
    let cache = Rc::new(RefCell::new(Some(Rc::clone(&tooltip))));
    let scope =
        crate::transient_window::TransientScope::new(Rc::clone(&cache), TooltipController::hide);
    drop(scope);
    assert!(cache.borrow().is_none());
    assert!(!tooltip.surface.is_visible());
    assert_eq!(
        &*host.events.lock(),
        &["attach", "detach", "attach", "detach"]
    );
    let weak = Rc::downgrade(&tooltip);
    drop(tooltip);
    assert!(weak.upgrade().is_none());
}

#[test]
fn synchronous_close_during_attachment_cannot_resurrect_a_tooltip_or_keep_its_lease() {
    i_slint_backend_testing::init_no_event_loop();
    let host = Arc::new(Host {
        close_during_attach: AtomicBool::new(true),
        ..Host::default()
    });
    let owner = Dock::new().unwrap();
    owner.show().unwrap();
    let tooltip = TooltipController::new(host.clone(), Rc::new(|error| panic!("{error}"))).unwrap();
    let context = DockContext::new(0, 0, 1920, 1080, false).unwrap();
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "cancelled",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(100);
    assert!(
        !tooltip.surface.is_visible(),
        "a reentrant close must win: actual_window_visible={}, native_events={:?}",
        tooltip.window().is_visible(),
        *host.events.lock(),
    );
    assert!(!tooltip.window().is_visible());
    assert_eq!(&*host.events.lock(), &["attach", "detach"]);
    host.close_during_attach.store(false, Ordering::Relaxed);
    tooltip
        .schedule(
            &owner,
            SurfaceKind::Dock,
            "retry",
            bounds(),
            context,
            Side::Top,
        )
        .unwrap();
    advance(100);
    assert!(tooltip.surface.is_visible());
    tooltip.hide();
    assert_eq!(
        &*host.events.lock(),
        &["attach", "detach", "attach", "detach"]
    );
}
