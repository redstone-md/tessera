// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One passive tooltip per presentation controller; no inline toolkit popup.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::ComponentHandle;

use crate::generated::{PopoverTokens, TileBounds, TooltipSurface};
use crate::theme::ThemedComponent;
use crate::transient_window::TransientWindow;
use crate::{DesktopHost, DockContext, SurfaceKind, sanitize};

mod placement;
pub(crate) use placement::Side;
#[cfg(test)]
mod tests;

pub(crate) struct TooltipController {
    surface: TransientWindow<TooltipSurface>,
    delay: slint::Timer,
    source: Cell<Option<(SurfaceKind, slint::LogicalPosition)>>,
    on_error: Rc<dyn Fn(String)>,
}

impl TooltipController {
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        on_error: Rc<dyn Fn(String)>,
    ) -> Result<Rc<Self>, slint::PlatformError> {
        let tooltip = Rc::new(Self {
            surface: TransientWindow::new(host, TooltipSurface::new()?, SurfaceKind::Tooltip),
            delay: slint::Timer::default(),
            on_error,
            source: Cell::new(None),
        });
        let weak = Rc::downgrade(&tooltip);
        tooltip.window().on_close_requested(move || {
            if let Some(tooltip) = weak.upgrade() {
                tooltip.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(tooltip)
    }

    pub(crate) fn schedule<C: ThemedComponent + 'static>(
        self: &Rc<Self>,
        owner: &C,
        source: SurfaceKind,
        content: &str,
        bounds: TileBounds,
        context: DockContext,
        side: Side,
    ) -> Result<(), String> {
        self.hide();
        let content = sanitize::bounded_text(content, 512);
        if content.is_empty() || context.fullscreen_active() || !owner.window().is_visible() {
            return Ok(());
        }
        placement::tile_rect(
            owner.window().position(),
            owner.window().scale_factor(),
            &bounds,
        )?;
        self.source.set(Some((source, bounds.origin)));
        let owner = owner.as_weak();
        let weak = Rc::downgrade(self);
        self.delay.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(100),
            move || {
                let Some(tooltip) = weak.upgrade() else {
                    return;
                };
                let Some(owner) = owner.upgrade() else { return };
                if !owner.window().is_visible() {
                    return;
                }
                if let Err(error) = tooltip.present_for(&owner, &content, &bounds, context, side) {
                    (tooltip.on_error)(format!("Tooltip: {error}"));
                }
            },
        );
        Ok(())
    }

    fn present_for<C: ThemedComponent>(
        &self,
        owner: &C,
        content: &str,
        bounds: &TileBounds,
        context: DockContext,
        side: Side,
    ) -> Result<(), String> {
        self.surface
            .apply_presentation_theme(owner.presentation_theme());
        let owner = owner.window();
        let scale = owner.scale_factor();
        let tile = placement::tile_rect(owner.position(), scale, bounds)?;
        self.surface.set_content(content.into());
        let margin = self.surface.global::<PopoverTokens>().get_shadow_margin();
        self.surface.set_max_content_width(
            (context.width() as f32 / scale - 2.0 * margin).clamp(16.0, 500.0),
        );
        let rect = placement::place(
            context,
            tile,
            (
                self.surface.get_tooltip_width(),
                self.surface.get_tooltip_height(),
            ),
            scale,
            side,
        )?;
        self.surface.present(rect.position, rect.size).map(|_| ())
    }

    pub(crate) fn dismiss_for(
        self: &Rc<Self>,
        source: SurfaceKind,
        origin: slint::LogicalPosition,
        delayed: bool,
    ) {
        // A late leave from the previous tile must not cancel a newer hover,
        // including one on the other native owner window.
        if !delayed || self.source.get() == Some((source, origin)) {
            self.dismiss(delayed);
        }
    }

    /// Mouse leave uses the reference 100ms delay; click, disable, refresh,
    /// geometry changes and teardown cancel pending work immediately.
    pub(crate) fn dismiss(self: &Rc<Self>, delayed: bool) {
        self.delay.stop();
        if !delayed || !self.surface.is_visible() {
            self.hide();
            return;
        }
        let weak = Rc::downgrade(self);
        self.delay.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(100),
            move || {
                if let Some(tooltip) = weak.upgrade() {
                    tooltip.hide();
                }
            },
        );
    }

    pub(crate) fn hide(&self) {
        self.delay.stop();
        self.source.set(None);
        self.surface.hide();
    }
    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }

    fn window(&self) -> &slint::Window {
        self.surface.window()
    }
}

impl Drop for TooltipController {
    fn drop(&mut self) {
        self.hide();
    }
}
