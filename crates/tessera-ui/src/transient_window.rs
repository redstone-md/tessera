// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shared lifetime boundary for separately rendered native transient surfaces.

use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::sync::Arc;

use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};

use crate::{DesktopHost, SurfaceKind};

pub(crate) struct TransientWindow<C: ComponentHandle> {
    component: C,
    host: Arc<dyn DesktopHost>,
    kind: SurfaceKind,
    lease: RefCell<Option<Box<dyn std::any::Any>>>,
    visible: Cell<bool>,
}

impl<C: ComponentHandle> TransientWindow<C> {
    pub(crate) fn new(host: Arc<dyn DesktopHost>, component: C, kind: SurfaceKind) -> Self {
        Self {
            component,
            host,
            kind,
            lease: RefCell::default(),
            visible: Cell::new(false),
        }
    }

    /// Placement is already validated by the caller. Replacing a presentation
    /// releases its old native role before moving or showing the window again.
    pub(crate) fn present(
        &self,
        position: PhysicalPosition,
        size: PhysicalSize,
    ) -> Result<(), String> {
        self.hide();
        self.component.window().set_position(position);
        self.component.window().set_size(size);
        self.component.show().map_err(|error| error.to_string())?;
        match self
            .host
            .configure_surface(self.kind, self.component.window())
        {
            Ok(lease) => *self.lease.borrow_mut() = lease,
            Err(error) => {
                let _ = self.component.hide();
                return Err(error);
            }
        }
        self.visible.set(true);
        Ok(())
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visible.get()
    }

    /// Request only for an explicitly opened interactive popup. Passive
    /// transient surfaces never get a foreground request through this boundary.
    pub(crate) fn request_focus(&self) -> Result<(), String> {
        if !self.visible.get() || self.kind != SurfaceKind::Popup {
            return Err("Only a visible interactive popup can request focus.".into());
        }
        self.host.request_ui_focus(self.component.window())
    }

    pub(crate) fn hide(&self) {
        self.visible.set(false);
        let lease = self.lease.borrow_mut().take();
        drop(lease);
        let _ = self.component.hide();
    }
}

// Generated component properties/callbacks remain available, while native
// presentation and lease teardown are centralized above.
impl<C: ComponentHandle> Deref for TransientWindow<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl<C: ComponentHandle> Drop for TransientWindow<C> {
    fn drop(&mut self) {
        self.hide();
    }
}
