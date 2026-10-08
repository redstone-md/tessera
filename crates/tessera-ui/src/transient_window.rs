// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared lifetime boundary for separately rendered native transient surfaces.

use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};

use crate::generated::{ContextMenuSurface, PopoverMotion, TooltipSurface};
use crate::{DesktopHost, SurfaceKind};

#[cfg(test)]
mod tests;

pub(crate) trait TransientComponent: ComponentHandle {
    fn motion(&self) -> PopoverMotion<'_>;
    fn set_presentation_opacity(&self, opacity: f32);

    fn reset_presentation(&self) {
        self.update_presentation(false, false, || true);
    }

    // Standalone generated previews have no lifetime owner. Retain their
    // helpers; native TransientWindow operations use the guarded path below.
    #[allow(dead_code)]
    fn reveal(&self, motion_enabled: bool) {
        self.update_presentation(motion_enabled, true, || true);
    }

    // Recheck ownership between generated property effects: any callback may
    // cancel this presentation or install a newer one on the same component.
    fn update_presentation(
        &self,
        motion_enabled: bool,
        presented: bool,
        is_current: impl Fn() -> bool,
    ) -> bool {
        if !is_current() {
            return false;
        }
        let motion = self.motion();
        if !is_current() {
            return false;
        }
        motion.set_enabled(motion_enabled);
        if !is_current() {
            return false;
        }
        motion.set_presented(presented);
        if !is_current() {
            return false;
        }
        self.set_presentation_opacity(if presented { 1.0 } else { 0.0 });
        is_current()
    }

    #[allow(dead_code)]
    fn disable_motion(&self) {
        let motion = self.motion();
        motion.set_enabled(false);
        self.set_presentation_opacity(if motion.get_presented() { 1.0 } else { 0.0 });
    }
}

impl TransientComponent for TooltipSurface {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

impl TransientComponent for ContextMenuSurface {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visibility {
    Hidden,
    Retiring,
    Presenting,
    Cancelled,
    Visible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Lifecycle {
    operation: u64,
    visibility: Visibility,
}

struct Presentation<'a, C: TransientComponent> {
    window: &'a TransientWindow<C>,
    operation: u64,
}

impl<C: TransientComponent> Drop for Presentation<'_, C> {
    fn drop(&mut self) {
        let mut state = self.window.lifecycle.get();
        if state.operation == self.operation
            && matches!(
                state.visibility,
                Visibility::Retiring | Visibility::Presenting | Visibility::Cancelled
            )
        {
            // A cancelled local attachment has already dropped before this
            // guard runs. Only now may a new presentation start or HWND hide.
            state.visibility = Visibility::Hidden;
            self.window.lifecycle.set(state);
            self.window.retire(state, true);
        }
    }
}

pub(crate) struct TransientWindow<C: TransientComponent> {
    component: C,
    host: Arc<dyn DesktopHost>,
    kind: SurfaceKind,
    lease: RefCell<Option<Box<dyn std::any::Any>>>,
    lifecycle: Cell<Lifecycle>,
}

impl<C: TransientComponent> TransientWindow<C> {
    pub(crate) fn new(host: Arc<dyn DesktopHost>, component: C, kind: SurfaceKind) -> Self {
        component.reset_presentation();
        Self {
            component,
            host,
            kind,
            lease: RefCell::default(),
            lifecycle: Cell::new(Lifecycle {
                operation: 0,
                visibility: Visibility::Hidden,
            }),
        }
    }

    /// Placement is validated by the caller. A synchronous cancellation
    /// returns false; its in-flight native lease drops before the HWND hides.
    pub(crate) fn present(
        &self,
        position: PhysicalPosition,
        size: PhysicalSize,
    ) -> Result<bool, String> {
        let mut state = self.lifecycle.get();
        if matches!(
            state.visibility,
            Visibility::Presenting | Visibility::Cancelled
        ) {
            return Err("Native transient presentation is already in progress.".into());
        }
        state.operation = state
            .operation
            .checked_add(1)
            .ok_or_else(|| "Native transient presentation identity is exhausted.".to_string())?;
        // Retiring permits a lease-Drop replacement, but reserves the outer
        // operation first so its continuation cannot overwrite that replacement.
        state.visibility = Visibility::Retiring;
        self.lifecycle.set(state);
        let _presentation = Presentation {
            window: self,
            operation: state.operation,
        };
        self.retire(state, true);
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        state.visibility = Visibility::Presenting;
        self.lifecycle.set(state);
        let motion_enabled = self.host.ui_animations_enabled();
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        self.component.window().set_position(position);
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        self.component.window().set_size(size);
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        self.component.show().map_err(|error| error.to_string())?;
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        let attachment = self
            .host
            .configure_surface(self.kind, self.component.window());
        if self.lifecycle.get() != state {
            // close/hide callbacks may run inside show/attachment. Never
            // publish their late lease or resurrect cancelled visibility.
            drop(attachment);
            return Ok(false);
        }
        // Even an unexpected previous lease must release outside the slot's
        // RefCell borrow; its destructor is allowed to call back into us.
        let previous = self.lease.replace(attachment?);
        drop(previous);
        if self.lifecycle.get() != state {
            return Ok(false);
        }
        state.visibility = Visibility::Visible;
        self.lifecycle.set(state);
        Ok(self
            .component
            .update_presentation(motion_enabled, true, || self.lifecycle.get() == state))
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.lifecycle.get().visibility == Visibility::Visible
    }
    pub(crate) fn disable_motion(&self) {
        let state = self.lifecycle.get();
        let motion = self.component.motion();
        if self.lifecycle.get() != state {
            return;
        }
        motion.set_enabled(false);
        if self.lifecycle.get() != state {
            return;
        }
        let opacity = if motion.get_presented() { 1.0 } else { 0.0 };
        if self.lifecycle.get() != state {
            return;
        }
        self.component.set_presentation_opacity(opacity);
    }

    /// Refit an already attached transient without replaying show/focus/motion.
    /// Unlike a bar's AppBar lease, this lease configures a static native role.
    /// The caller validates placement; closed/in-flight windows stay closed.
    pub(crate) fn reposition(&self, position: PhysicalPosition, size: PhysicalSize) -> bool {
        let state = self.lifecycle.get();
        if state.visibility != Visibility::Visible {
            return false;
        }
        self.component.window().set_position(position);
        if self.lifecycle.get() != state {
            return false;
        }
        self.component.window().set_size(size);
        self.lifecycle.get() == state
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
        let mut state = self.lifecycle.get();
        let in_flight = matches!(
            state.visibility,
            Visibility::Presenting | Visibility::Cancelled
        );
        state.visibility = if in_flight {
            Visibility::Cancelled
        } else {
            Visibility::Hidden
        };
        self.lifecycle.set(state);
        self.retire(state, !in_flight);
    }

    fn retire(&self, state: Lifecycle, hide_window: bool) {
        if self.lifecycle.get() != state {
            return;
        }
        let lease = self.lease.borrow_mut().take();
        drop(lease);
        let still_current = self
            .component
            .update_presentation(false, false, || self.lifecycle.get() == state);
        // During attachment, its local lease has not reached our slot yet.
        // The presentation guard hides only after that lease is discarded.
        // A lease-Drop replacement instead owns the current window and motion.
        if still_current && hide_window {
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
