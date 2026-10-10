// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

// Native toolbar geometry owns Y; the actual launch tile owns X.
fn toolbar_anchor(
    origin: PhysicalPosition,
    size: PhysicalSize,
    scale: f32,
    bounds: &TileBounds,
) -> Result<PhysicalPosition, String> {
    if size.width == 0
        || size.height == 0
        || !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err("Network needs positive native geometry and valid input bounds.".into());
    }
    let bottom = i32::try_from(i64::from(origin.y) + i64::from(size.height))
        .map_err(|_| "Network source bottom is outside native coordinates.")?;
    placement::physical_anchor(
        PhysicalPosition::new(origin.x, bottom),
        scale,
        (bounds.origin.x + bounds.width / 2.0, -10.0),
    )
    .map_err(str::to_owned)
}

impl NetworkController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), NetworkMenu::new()?, SurfaceKind::Popup),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            projecting: Cell::new(false),
            presenting: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            focus_seen: Cell::new(false),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_network_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.drain();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_refresh_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.refresh(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_settings_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.settings(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_control_selected(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.select_control(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller
            .surface
            .on_command_requested(move |key, password| {
                if let Some(controller) = weak.upgrade() {
                    controller.submit_control(key, password);
                }
            });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_radio_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.submit_radio(key);
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_hide_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_preferred_size_changed(move || {
            if let Some(controller) = weak.upgrade() {
                controller.schedule_fit();
            }
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.window().on_close_requested(move || {
            if let Some(controller) = weak.upgrade() {
                controller.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
        Ok(controller)
    }

    pub(crate) fn component(&self) -> &NetworkMenu {
        &self.surface
    }
    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible() && self.component().window().is_visible()
    }

    pub(crate) fn show(
        self: &Rc<Self>,
        source: &slint::Window,
        bounds: TileBounds,
        context: DockContext,
    ) -> Result<(), String> {
        if self.presenting.get() {
            return Err("Network presentation is already in progress.".into());
        }
        self.hide();
        if !source.is_visible() || context.fullscreen_active() {
            return Err("Network needs a visible source and valid input bounds.".into());
        }
        let scale = source.scale_factor();
        let anchor = toolbar_anchor(source.position(), source.size(), scale, &bounds)?;
        self.placement.set(Some(Placement {
            anchor,
            context,
            scale,
        }));
        let session = {
            let mut state = self.state.borrow_mut();
            state.read_requested = true;
            state.session
        };
        self.project();
        let presentation = {
            self.presenting.set(true);
            let _guard = Guard(&self.presenting);
            self.preferred_rect().and_then(|rect| {
                self.surface
                    .present(rect.position, rect.size)
                    .map(|shown| (rect, shown))
            })
        };
        match presentation {
            Ok((rect, true)) if self.current(session) => *self.rect.borrow_mut() = Some(rect),
            Ok(_) => {
                self.hide();
                return Ok(());
            }
            Err(error) => {
                self.hide();
                return Err(error);
            }
        }
        self.surface.invoke_focus_content();
        if !self.current(session) {
            return Ok(());
        }
        let focus = self.surface.request_focus();
        if !self.current(session) {
            return Ok(());
        }
        self.watch_focus();
        self.drain();
        focus.map_err(|error| {
            bounded_text(
                &format!("Network opened, but keyboard focus was not granted: {error}"),
                240,
            )
        })
    }

    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.focus_watch.stop();
        self.focus_seen.set(false);
        let (watch, session) = {
            let mut state = self.state.borrow_mut();
            state.session = state.session.saturating_add(1);
            state.read_requested = false;
            state.refresh_key = SharedString::default();
            state.settings_key = SharedString::default();
            state.snapshot = None;
            state.controls.clear();
            state.controls_supported = false;
            state.selected = None;
            state.row_keys.clear();
            state.radios.clear();
            state.radio_keys.clear();
            state.radios_supported = false;
            state.radio_notice.clear();
            state.command_key = SharedString::default();
            state.control_notice.clear();
            state.inventory_notice.clear();
            state.notice.clear();
            state.settings_notice.clear();
            state.watch_status.clear();
            state.automatic_blocked = false;
            state.watch_started = false;
            // Accepted reads/actions retain their original tokens across reopen.
            (state.watch_guard.take(), state.session)
        };
        self.mailbox.lock().close_watch();
        // Guard cleanup can call back; admission/session are already retired.
        drop(watch);
        if self.state.borrow().session != session {
            return;
        }
        self.placement.set(None);
        self.rect.borrow_mut().take();
        self.surface.set_refresh_key(SharedString::default());
        self.surface.set_settings_key(SharedString::default());
        self.surface.set_connected(ModelRc::default());
        self.surface.set_saved(ModelRc::default());
        self.surface.set_available(ModelRc::default());
        self.surface.set_hidden(ModelRc::default());
        self.surface.set_control_rows(ModelRc::default());
        self.surface.set_connection_controls_supported(false);
        self.surface.set_radio_controls_supported(false);
        self.surface.set_radio_rows(ModelRc::default());
        self.surface.set_command_key(SharedString::default());
        self.surface.set_selected_network(SharedString::default());
        self.surface.set_credentials_active(false);
        self.surface.set_password(SharedString::default());
        self.surface.set_notice(SharedString::default());
        self.surface.set_watch_status(SharedString::default());
        self.surface.set_radio_text(SharedString::default());
        self.surface.set_summary(SharedString::default());
        self.surface.hide();
    }

    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }
    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.component().apply_presentation_theme(theme);
        let _ = self.refit();
    }
    pub(crate) fn close_if_geometry_changed(&self, context: DockContext, scale: f32) {
        if let Some(previous) = self.placement.get() {
            let old = previous.context;
            if context.fullscreen_active()
                || previous.scale != scale
                || (old.x(), old.y(), old.width(), old.height())
                    != (context.x(), context.y(), context.width(), context.height())
            {
                self.hide();
            }
        }
    }

    fn preferred_rect(&self) -> Result<PopupRect, String> {
        let placement = self
            .placement
            .get()
            .ok_or("Network placement is unavailable.")?;
        placement::place_centered(
            placement.context,
            placement.anchor,
            (
                self.component().get_popup_content_width(),
                self.component().get_popup_content_height(),
            ),
            placement.scale,
        )
    }
    pub(crate) fn refit(&self) -> Result<(), String> {
        if !self.is_open() {
            return Ok(());
        }
        match self.preferred_rect() {
            Ok(rect) => {
                let changed = self.rect.borrow().as_ref() != Some(&rect);
                if changed && self.surface.reposition(rect.position, rect.size) {
                    *self.rect.borrow_mut() = Some(rect);
                    self.control_frame_changed();
                }
                Ok(())
            }
            Err(error) => {
                self.hide();
                Err(error)
            }
        }
    }
    fn schedule_fit(self: &Rc<Self>) {
        if !self.is_open() || self.placement.get().is_none() || self.fit_timer.running() {
            return;
        }
        let weak = Rc::downgrade(self);
        let session = self.state.borrow().session;
        self.fit_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade()
                    && controller.current(session)
                {
                    let _ = controller.refit();
                }
            });
    }
    fn watch_focus(self: &Rc<Self>) {
        let focused = self.is_focused();
        self.focus_seen.set(focused == Some(true));
        if focused.is_none() || !self.is_open() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.focus_watch.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if let Some(controller) = weak.upgrade() {
                    if !controller.is_open() {
                        controller.focus_watch.stop();
                        return;
                    }
                    match controller.is_focused() {
                        Some(true) => controller.focus_seen.set(true),
                        Some(false) if controller.focus_seen.get() => controller.hide(),
                        _ => {}
                    }
                }
            },
        );
    }
    #[cfg(any(windows, test))]
    fn is_focused(&self) -> Option<bool> {
        use slint::winit_030::WinitWindowAccessor;
        self.surface
            .window()
            .with_winit_window(|window| window.has_focus())
    }
    #[cfg(not(any(windows, test)))]
    fn is_focused(&self) -> Option<bool> {
        None
    }
}
