// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared lifetime boundary for separately rendered native transient surfaces.

use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, Global, PhysicalPosition, PhysicalSize};

use crate::generated::PopoverMotion;
use crate::{DesktopHost, SurfaceKind};

pub(crate) trait TransientComponent: ComponentHandle {
    fn reset_presentation(&self);
    fn reveal(&self, motion_enabled: bool);
}

impl<C: ComponentHandle> TransientComponent for C
where
    for<'a> PopoverMotion<'a>: Global<'a, C>,
{
    fn reset_presentation(&self) {
        let motion = self.global::<PopoverMotion>();
        motion.set_enabled(false);
        motion.set_presented(false);
    }

    fn reveal(&self, motion_enabled: bool) {
        let motion = self.global::<PopoverMotion>();
        motion.set_enabled(motion_enabled);
        motion.set_presented(true);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Presenting,
    Cancelled,
    Visible,
}

struct Presentation<'a, C: TransientComponent>(&'a TransientWindow<C>);

impl<C: TransientComponent> Drop for Presentation<'_, C> {
    fn drop(&mut self) {
        if self.0.visibility.get() != Visibility::Visible {
            self.0.visibility.set(Visibility::Hidden);
            self.0.component.reset_presentation();
            let _ = self.0.component.hide();
        }
    }
}

pub(crate) struct TransientWindow<C: TransientComponent> {
    component: C,
    host: Arc<dyn DesktopHost>,
    kind: SurfaceKind,
    lease: RefCell<Option<Box<dyn std::any::Any>>>,
    visibility: Cell<Visibility>,
}

impl<C: TransientComponent> TransientWindow<C> {
    pub(crate) fn new(host: Arc<dyn DesktopHost>, component: C, kind: SurfaceKind) -> Self {
        component.reset_presentation();
        Self {
            component,
            host,
            kind,
            lease: RefCell::default(),
            visibility: Cell::new(Visibility::Hidden),
        }
    }

    /// Placement is validated by the caller. A synchronous cancellation
    /// returns false; its in-flight native lease drops before the HWND hides.
    pub(crate) fn present(
        &self,
        position: PhysicalPosition,
        size: PhysicalSize,
    ) -> Result<bool, String> {
        if matches!(
            self.visibility.get(),
            Visibility::Presenting | Visibility::Cancelled
        ) {
            return Err("Native transient presentation is already in progress.".into());
        }
        self.hide();
        let motion_enabled = self.host.ui_animations_enabled();
        self.visibility.set(Visibility::Presenting);
        let _presentation = Presentation(self);
        self.component.window().set_position(position);
        self.component.window().set_size(size);
        self.component.show().map_err(|error| error.to_string())?;
        if self.visibility.get() != Visibility::Presenting {
            return Ok(false);
        }
        let attachment = self
            .host
            .configure_surface(self.kind, self.component.window());
        if self.visibility.get() != Visibility::Presenting {
            // close/hide callbacks may run inside show/attachment. Never
            // publish their late lease or resurrect cancelled visibility.
            drop(attachment);
            return Ok(false);
        }
        *self.lease.borrow_mut() = attachment?;
        self.visibility.set(Visibility::Visible);
        self.component.reveal(motion_enabled);
        Ok(true)
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visibility.get() == Visibility::Visible
    }

    /// Request only for an explicitly opened interactive popup. Passive
    /// transient surfaces never get a foreground request through this boundary.
    pub(crate) fn request_focus(&self) -> Result<(), String> {
        if !self.is_visible() || self.kind != SurfaceKind::Popup {
            return Err("Only a visible interactive popup can request focus.".into());
        }
        self.host.request_ui_focus(self.component.window())
    }

    pub(crate) fn hide(&self) {
        let in_flight = matches!(
            self.visibility.get(),
            Visibility::Presenting | Visibility::Cancelled
        );
        self.visibility.set(if in_flight {
            Visibility::Cancelled
        } else {
            Visibility::Hidden
        });
        let lease = self.lease.borrow_mut().take();
        drop(lease);
        self.component.reset_presentation();
        // During attachment, its local lease has not reached our slot yet.
        // The presentation guard hides only after that lease is discarded.
        if !in_flight {
            let _ = self.component.hide();
        }
    }
}

// Generated component properties/callbacks remain available, while native
// presentation and lease teardown are centralized above.
impl<C: TransientComponent> Deref for TransientWindow<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl<C: TransientComponent> Drop for TransientWindow<C> {
    fn drop(&mut self) {
        self.hide();
    }
}

pub(crate) type TransientCache<T> = Rc<RefCell<Option<Rc<T>>>>;

/// Callbacks may retain their shared cache after the event loop returns.
/// Clear and explicitly dismiss its controller before any main HWND teardown.
pub(crate) struct TransientScope<T> {
    cache: TransientCache<T>,
    dismiss: fn(&T),
}

impl<T> TransientScope<T> {
    pub(crate) fn new(cache: TransientCache<T>, dismiss: fn(&T)) -> Self {
        Self { cache, dismiss }
    }
}

impl<T> Drop for TransientScope<T> {
    fn drop(&mut self) {
        let controller = self.cache.borrow_mut().take();
        if let Some(controller) = controller {
            (self.dismiss)(&controller);
        }
    }
}
