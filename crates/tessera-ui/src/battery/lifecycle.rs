// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

fn anchor(source: Frame, bounds: &TileBounds) -> Result<PhysicalPosition, String> {
    let scale = source.scale;
    if source.size.width == 0
        || source.size.height == 0
        || !scale.is_finite()
        || scale <= 0.0
        || !bounds.origin.x.is_finite()
        || !bounds.origin.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
        || bounds.origin.x < 0.0
        || bounds.origin.y < 0.0
        || bounds.origin.x + bounds.width > source.size.width as f32 / scale
        || bounds.origin.y + bounds.height > source.size.height as f32 / scale
    {
        return Err("Battery needs valid visible native source bounds.".into());
    }
    let bottom = i32::try_from(i64::from(source.position.y) + i64::from(source.size.height))
        .map_err(|_| "Battery source geometry is outside native coordinates.")?;
    placement::physical_anchor(
        PhysicalPosition::new(source.position.x, bottom),
        scale,
        (bounds.origin.x + bounds.width / 2.0, -10.0),
    )
    .map_err(str::to_owned)
}
fn source_frame(source: &slint::Window) -> Frame {
    Frame {
        position: source.position(),
        size: source.size(),
        scale: source.scale_factor(),
    }
}
impl BatteryController {
    pub(crate) fn new(host: Arc<dyn DesktopHost>) -> Result<Rc<Self>, slint::PlatformError> {
        let controller = Rc::new(Self {
            surface: TransientWindow::new(host.clone(), BatteryMenu::new()?, SurfaceKind::Popup),
            host,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            toolbar_projection: RefCell::default(),
            projecting: Cell::new(false),
            root_admission: RefCell::default(),
            presenting: Cell::new(false),
            placement: Cell::new(None),
            rect: RefCell::default(),
            fit_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            focus_seen: Cell::new(false),
        });
        let weak = Rc::downgrade(&controller);
        controller.surface.on_battery_event_ready(move || {
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
    pub(crate) fn component(&self) -> &BatteryMenu {
        &self.surface
    }
    pub(crate) fn is_open(&self) -> bool {
        self.surface.is_visible() && self.component().window().is_visible()
    }
    pub(crate) fn bind_toolbar(self: &Rc<Self>, project: impl Fn(BatteryProjection) + 'static) {
        let retired = self.toolbar_projection.replace(Some(Rc::new(project)));
        drop(retired);
        self.project();
    }
    /// Effects require a current readonly Root/bar admission predicate. The
    /// parent supplies weak captures only; absence fails closed.
    pub(crate) fn set_root_admission(self: &Rc<Self>, current: impl Fn() -> bool + 'static) {
        let retired = self.root_admission.replace(Some(Rc::new(current)));
        drop(retired);
    }
    /// Called only after readonly root/bar source admission. No popup-operation
    /// token participates in this passive lease; menu switches leave it alive.
    pub(crate) fn start_root(self: &Rc<Self>) {
        if !self.root_admitted() {
            self.stop_root();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.root_active || state.exhausted {
                return;
            }
            state.root_active = true;
            state.read_requested = true;
        }
        self.pump();
    }
    /// Root loss retires popup input and pending demand, never an accepted flight.
    pub(crate) fn stop_root(&self) {
        let root_active = self.state.borrow().root_active;
        if !root_active
            && !self.is_open()
            && !self.presenting.get()
            && self.placement.get().is_none()
        {
            self.retire_watch_if_idle();
            return;
        }
        self.state.borrow_mut().root_active = false;
        self.hide();
    }
    pub(crate) fn show(
        self: &Rc<Self>,
        source: &slint::Window,
        bounds: TileBounds,
        context: DockContext,
    ) -> Result<(), String> {
        if self.presenting.get() || self.projecting.get() {
            return Err("Battery presentation is already in progress.".into());
        }
        self.hide();
        if !self.root_admitted()
            || !source.is_visible()
            || context.fullscreen_active()
            || self.state.borrow().exhausted
        {
            return Err("Battery needs an admitted visible source.".into());
        }
        let source_geometry = source_frame(source);
        let anchor = anchor(source_geometry, &bounds)?;
        self.placement.set(Some(Placement {
            anchor,
            context,
            scale: source_geometry.scale,
        }));
        let session = {
            let mut state = self.state.borrow_mut();
            state.settings_notice.clear();
            state.read_requested |= state.snapshot.is_none() || state.stale;
            state.session
        };
        self.project();
        let presentation = {
            self.presenting.set(true);
            let _guard = FlagGuard(&self.presenting);
            self.preferred_rect().and_then(|rect| {
                self.surface
                    .present(rect.position, rect.size)
                    .map(|shown| (rect, shown))
            })
        };
        match presentation {
            Ok((rect, true))
                if self.root_admitted()
                    && self.current(session)
                    && source.is_visible()
                    && source_frame(source) == source_geometry =>
            {
                *self.rect.borrow_mut() = Some(rect)
            }
            Ok(_) => {
                self.hide();
                return Ok(());
            }
            Err(error) => {
                self.hide();
                return Err(error);
            }
        }
        self.project();
        self.surface.invoke_focus_content();
        if !self.current(session) {
            return Ok(());
        }
        let focus = self.surface.request_focus();
        if !self.current(session) {
            return Ok(());
        }
        if !self.root_admitted() || !source.is_visible() || source_frame(source) != source_geometry
        {
            self.hide();
            return Ok(());
        }
        self.watch_focus();
        self.drain();
        focus.map_err(|_| "Battery opened, but keyboard focus was not granted.".into())
    }
    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.focus_watch.stop();
        self.focus_seen.set(false);
        {
            let mut state = self.state.borrow_mut();
            state.retire_session();
            if !state.root_active {
                state.read_requested = false;
            }
        }
        // Retire held native input synchronously, before lease/host reentry.
        self.component().invoke_cancel_input();
        self.component().set_refresh_key(SharedString::default());
        self.component().set_settings_key(SharedString::default());
        self.placement.set(None);
        self.rect.replace(None);
        self.surface.hide();
        self.retire_watch_if_idle();
        self.project();
    }
    fn retire_input(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.refresh_key = SharedString::default();
            state.settings_key = SharedString::default();
            state.input_frame = None;
        }
        self.component().invoke_cancel_input();
        self.component().set_refresh_key(SharedString::default());
        self.component().set_settings_key(SharedString::default());
    }
    pub(crate) fn disable_motion(&self) {
        self.surface.disable_motion();
    }
    pub(crate) fn apply_theme(&self, theme: PresentationTheme) {
        self.retire_input();
        self.component().apply_presentation_theme(theme);
        self.project();
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
        let position = self
            .placement
            .get()
            .ok_or("Battery placement is unavailable.")?;
        placement::place_centered(
            position.context,
            position.anchor,
            (
                self.component().get_popup_content_width(),
                self.component().get_popup_content_height(),
            ),
            position.scale,
        )
    }
    pub(crate) fn refit(&self) -> Result<(), String> {
        if !self.is_open() {
            return Ok(());
        }
        match self.preferred_rect() {
            Ok(rect) => {
                let changed = self.rect.borrow().as_ref() != Some(&rect);
                if changed {
                    self.retire_input();
                    if self.surface.reposition(rect.position, rect.size) {
                        self.rect.replace(Some(rect));
                        self.project();
                    }
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
        // Native focus-only lifetime watch, never a power observation poll.
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
