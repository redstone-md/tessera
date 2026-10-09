// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Desktop-spanning Power presentation with six directly dispatched native intents.
//! Domain flights survive hidden presentation retirement; no command is replayed.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, PhysicalPosition, PhysicalSize};
use tessera_system::display_context::{DisplayContextError, DisplayContextHost, DisplayLayout};
use tessera_system::power::{
    PowerAction, PowerError, PowerHost, PowerRequestAccepted, PowerUpdatePolicy,
};
use tessera_system::power_updates::{PowerUpdateHint, PowerUpdatesError, PowerUpdatesHost};

use crate::generated::{
    FocusTokens, Palette, PopoverMotion, PowerMenuAction, PowerMenuSurface, SeelenPalette,
};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, SurfaceKind, Theme};

mod mailbox;
mod presentation;
mod updates;
use mailbox::{Mailbox, complete_lock, complete_read, complete_updates};

#[cfg(test)]
pub(crate) mod source_measure_tests;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod tests;

impl TransientComponent for PowerMenuSurface {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }

    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Token {
    generation: u64,
    sequence: u64,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Reserved,
    Submitted,
}

#[derive(Clone, Copy)]
struct Flight {
    token: Token,
    phase: Phase,
}

#[derive(Default)]
struct State {
    generation: u64,
    sequence: u64,
    closed: bool,
    epoch: u64,
    desired: bool,
    opening: bool,
    authorized: Option<u64>,
    theme: Theme,
    layout: Option<DisplayLayout>,
    display: Option<Arc<dyn DisplayContextHost>>,
    power: Option<Arc<dyn PowerHost>>,
    updates: Option<Arc<dyn PowerUpdatesHost>>,
    updates_dirty: bool,
    updates_read: Option<Flight>,
    updates_hint: Option<PowerUpdateHint>,
    updates_status: String,
    install_updates: bool,
    initialized: bool,
    initial_updates_generation: Option<u64>,
    user_name: String,
    hidden_deadline: Option<presentation::HiddenDeadline>,
    read_dirty: bool,
    read: Option<Flight>,
    lock: Option<Flight>,
}

impl State {
    fn advance(&mut self) -> Result<u64, String> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or("Power menu presentation identity is exhausted.")?;
        Ok(self.generation)
    }

    fn token(&mut self) -> Result<Token, String> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Power menu request identity is exhausted.")?;
        Ok(Token {
            generation: self.generation,
            sequence: self.sequence,
            epoch: self.epoch,
        })
    }

    fn current(&self, generation: u64) -> bool {
        !self.closed && self.desired && self.generation == generation
    }
}

type ReadResult = Result<Option<DisplayLayout>, DisplayContextError>;
type LockResult = Result<PowerRequestAccepted, PowerError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    position: PhysicalPosition,
    size: PhysicalSize,
}

impl Frame {
    fn from_layout(layout: DisplayLayout) -> Result<Self, String> {
        let desktop = layout.desktop_bounds();
        let width = desktop.width();
        let height = desktop.height();
        // Product resource limits, not a universal GPU capability guarantee.
        if width > 16_384 || height > 16_384 || u64::from(width) * u64::from(height) > 67_108_864 {
            return Err("The desktop span exceeds the Power menu frame budget.".into());
        }
        Ok(Self {
            position: PhysicalPosition::new(desktop.x(), desktop.y()),
            size: PhysicalSize::new(width, height),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LogicalFit {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    metric: f32,
}

impl LogicalFit {
    fn new(layout: DisplayLayout, native_scale: f32) -> Result<Self, String> {
        let scale = f64::from(native_scale);
        if !scale.is_finite() || scale <= 0.0 {
            return Err("The Power surface has no valid native scale.".into());
        }
        let desktop = layout.desktop_bounds();
        let selected = layout.selected_bounds();
        let metric = layout.presentation_scale() / scale;
        let values = [
            (i64::from(selected.x()) - i64::from(desktop.x())) as f64 / scale,
            (i64::from(selected.y()) - i64::from(desktop.y())) as f64 / scale,
            f64::from(selected.width()) / scale,
            f64::from(selected.height()) / scale,
            metric,
        ];
        // Validate all downstream lengths, not merely the ratio cast. No 96DPI
        // or Launcher scale fallback is allowed when native scale is invalid.
        if values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0 || *value > f64::from(f32::MAX))
            || values[2] <= 0.0
            || values[3] <= 0.0
            || metric <= 0.0
            || !(metric * 512.0).is_finite()
            || metric * 512.0 > f64::from(f32::MAX)
            || f64::from(desktop.width()) / scale > f64::from(f32::MAX)
            || f64::from(desktop.height()) / scale > f64::from(f32::MAX)
        {
            return Err("Power menu display metrics cannot be represented safely.".into());
        }
        let [x, y, width, height, metric] = values.map(|value| value as f32);
        if width <= 0.0 || height <= 0.0 || metric <= 0.0 {
            return Err("Power menu display metrics are too small to represent.".into());
        }
        Ok(Self {
            x,
            y,
            width,
            height,
            metric,
        })
    }
}

struct Processing<'a>(&'a Cell<bool>);
impl Drop for Processing<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

struct NativeEffect<'a> {
    active: &'a Cell<bool>,
    previous: bool,
}

impl<'a> NativeEffect<'a> {
    fn enter(active: &'a Cell<bool>) -> Self {
        Self {
            active,
            previous: active.replace(true),
        }
    }
}

impl Drop for NativeEffect<'_> {
    fn drop(&mut self) {
        self.active.set(self.previous);
    }
}

pub(crate) struct PowerMenuController {
    surface: RefCell<Option<Rc<TransientWindow<PowerMenuSurface>>>>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    weak: Weak<Self>,
    on_opened: Rc<dyn Fn()>,
    on_result: Rc<dyn Fn(Result<(), String>)>,
    frame: Cell<Option<Frame>>,
    fit_timer: slint::Timer,
    work_timer: slint::Timer,
    focus_watch: slint::Timer,
    retirement_timer: slint::Timer,
    focus_seen: Cell<bool>,
    processing: Cell<bool>,
    native_effect: Cell<bool>,
    motion_suppressed: Cell<bool>,
}

impl PowerMenuController {
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        on_opened: Rc<dyn Fn()>,
        on_result: Rc<dyn Fn(Result<(), String>)>,
    ) -> Result<Rc<Self>, String> {
        let component =
            PowerMenuSurface::new().map_err(|_| "Could not create the Power menu surface.")?;
        let surface = Rc::new(TransientWindow::new(
            host.clone(),
            component,
            SurfaceKind::Popup,
        ));
        let controller = Rc::new_cyclic(|weak| Self {
            surface: RefCell::new(Some(surface.clone())),
            host,
            state: RefCell::new(State {
                epoch: 1,
                install_updates: true,
                ..State::default()
            }),
            mailbox: Arc::new(Mutex::default()),
            weak: weak.clone(),
            on_opened,
            on_result,
            frame: Cell::new(None),
            fit_timer: slint::Timer::default(),
            work_timer: slint::Timer::default(),
            focus_watch: slint::Timer::default(),
            retirement_timer: slint::Timer::default(),
            focus_seen: Cell::new(false),
            processing: Cell::new(false),
            native_effect: Cell::new(false),
            motion_suppressed: Cell::new(false),
        });
        controller.attach_callbacks(&surface, 1);
        controller.mailbox.lock().install(1, surface.as_weak())?;
        controller.project(0);
        Ok(controller)
    }

    #[cfg(test)]
    pub(crate) fn component(&self) -> PowerMenuSurface {
        self.component_if_present()
            .expect("current Power presentation")
    }

    #[cfg(test)]
    pub(crate) fn component_if_present(&self) -> Option<PowerMenuSurface> {
        self.surface_snapshot()
            .map(|(_, surface)| surface.clone_strong())
    }

    pub(crate) fn is_visible(&self) -> bool {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return false;
        };
        self.epoch_is(epoch)
            && surface.is_visible()
            && surface.window().is_visible()
            && self.epoch_is(epoch)
    }

    /// Cache-only seam for the root's presentation-independent display relay.
    /// Never discovers a provider, reads native state or creates a surface.
    pub(crate) fn display_provider(&self) -> Option<Arc<dyn DisplayContextHost>> {
        let state = self.state.borrow();
        if state.closed {
            None
        } else {
            state.display.clone()
        }
    }

    /// True means a fresh context request was scheduled, not that a popup is
    /// already visible. The root validates genuine Launcher trigger authority.
    pub(crate) fn show(&self, theme: Theme) -> Result<bool, String> {
        // Cancel before constructors, provider factories or generated effects.
        self.retirement_timer.stop();
        let generation = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return Err("The Power menu is closed.".into());
            }
            let generation = match state.advance() {
                Ok(generation) => generation,
                Err(error) => {
                    drop(state);
                    self.close();
                    return Err(error);
                }
            };
            state.theme = theme;
            state.desired = true;
            state.opening = true;
            state.authorized = None;
            state.layout = None;
            state.hidden_deadline = None;
            state.updates_dirty = true;
            if !state.initialized {
                state.initial_updates_generation = None;
            }
            state.read_dirty = true;
            generation
        };
        match self.ensure_surface(generation) {
            Ok(true) => {}
            Ok(false) => return Ok(false),
            Err(error) => {
                if self.current(generation) {
                    self.hide();
                }
                return Err(error);
            }
        }
        self.process_events();
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return Ok(false);
        };
        if !self.current(generation) || !self.epoch_is(epoch) {
            return Ok(false);
        }
        self.motion_suppressed.set(false);
        // Explicit replacement intent cancels an older native operation at its
        // own lifetime seam, so its position/show/configure continuation cannot
        // mutate the replacement. Settled visible refits stay attached.
        if self.native_effect.get() {
            surface.hide();
            if !self.current(generation) {
                return Ok(false);
            }
        }
        self.apply_theme(generation);
        if !self.current(generation) {
            return Ok(false);
        }
        self.project(generation);
        if !self.current(generation) {
            return Ok(false);
        }
        self.pump_read();
        if self.current(generation) {
            self.pump_updates();
        }
        Ok(self.current(generation))
    }

    pub(crate) fn hide(&self) {
        self.fit_timer.stop();
        self.work_timer.stop();
        self.focus_watch.stop();
        self.retirement_timer.stop();
        let presentation = self.surface_snapshot();
        self.focus_seen.set(false);
        let generation = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return;
            }
            let Ok(generation) = state.advance() else {
                drop(state);
                self.close();
                return;
            };
            state.desired = false;
            state.opening = false;
            state.authorized = None;
            state.layout = None;
            state.read_dirty = false;
            state.updates_dirty = false;
            state.hidden_deadline =
                presentation
                    .as_ref()
                    .map(|(epoch, _)| presentation::HiddenDeadline {
                        epoch: *epoch,
                        generation,
                    });
            generation
        };
        self.frame.set(None);
        self.project(generation);
        // Property and lease destructors may have installed a replacement.
        // Never let the older hide tear that replacement down.
        if let Some((epoch, surface)) = presentation
            && self.retired(generation)
            && self.epoch_is(epoch)
        {
            surface.hide();
        }
        self.arm_completed_hide();
    }

    /// Called by the root's TransientScope before its HWND teardown. Submitted
    /// work is not cancelled; late completion simply loses visual authority.
    pub(crate) fn close(&self) {
        self.fit_timer.stop();
        self.work_timer.stop();
        self.focus_watch.stop();
        self.retirement_timer.stop();
        let (display, power, updates, surface) = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return;
            }
            // Revoke every callback/presentation authority before native/UI or
            // provider Drop can reenter. Accepted native work is not cancelled.
            state.closed = true;
            state.generation = state.generation.checked_add(1).unwrap_or(state.generation);
            state.epoch = state.epoch.checked_add(1).unwrap_or(state.epoch);
            state.desired = false;
            state.opening = false;
            state.authorized = None;
            state.layout = None;
            state.read_dirty = false;
            state.updates_dirty = false;
            state.hidden_deadline = None;
            if state
                .read
                .is_some_and(|flight| flight.phase == Phase::Reserved)
            {
                state.read = None;
            }
            if state
                .lock
                .is_some_and(|flight| flight.phase == Phase::Reserved)
            {
                state.lock = None;
            }
            if state
                .updates_read
                .is_some_and(|flight| flight.phase == Phase::Reserved)
            {
                state.updates_read = None;
            }
            (
                state.display.take(),
                state.power.take(),
                state.updates.take(),
                self.surface.borrow_mut().take(),
            )
        };
        self.mailbox.lock().close();
        self.frame.set(None);
        self.focus_seen.set(false);
        if let Some(surface) = surface {
            surface.hide();
            drop(surface);
        }
        drop((display, power, updates));
    }

    pub(crate) fn set_theme(&self, theme: Theme) {
        let generation = {
            let mut state = self.state.borrow_mut();
            if state.closed {
                return;
            }
            state.theme = theme;
            state.generation
        };
        self.apply_theme(generation);
        if self.current(generation) && self.is_visible() {
            self.refit_or_report(generation);
        }
    }

    pub(crate) fn disable_motion(&self) {
        self.motion_suppressed.set(true);
        if let Some((epoch, surface)) = self.surface_snapshot()
            && self.epoch_is(epoch)
        {
            surface.disable_motion();
        }
    }

    pub(crate) fn update_motion(&self) {
        let generation = self.state.borrow().generation;
        if self.surface_snapshot().is_none() {
            return;
        }
        let enabled = self.host.ui_animations_enabled();
        if self.generation_is(generation) && !enabled {
            self.disable_motion();
        }
        // Enabling permission does not replay an existing presentation.
    }

    /// One dirty bit coalesces topology refreshes behind an accepted read.
    /// This path refits an attached popup without show/focus/motion replay.
    pub(crate) fn refresh_display(&self) {
        {
            let mut state = self.state.borrow_mut();
            if state.closed || !state.desired {
                return;
            }
            state.read_dirty = true;
        }
        self.pump_read();
    }

    pub(crate) fn process_events(&self) {
        if self.processing.replace(true) {
            return;
        }
        let _processing = Processing(&self.processing);
        let (read, lock, updates) = {
            let mut mailbox = self.mailbox.lock();
            mailbox.wake_queued = false;
            (
                mailbox.read.take(),
                mailbox.lock.take(),
                mailbox.updates.take(),
            )
        };
        if let Some((token, result)) = lock {
            let live = {
                let mut state = self.state.borrow_mut();
                if state.lock.is_some_and(|flight| flight.token == token) {
                    state.lock = None;
                    !state.closed
                } else {
                    false
                }
            };
            if live {
                let generation = self.state.borrow().generation;
                self.project(generation);
                if !self.state.borrow().closed {
                    (self.on_result)(
                        result
                            .map(|_| ())
                            .map_err(|error| format!("Power request failed: {error}")),
                    );
                }
            }
        }
        if let Some((token, result)) = updates {
            self.observe_updates(token, result);
        }
        if let Some((token, result)) = read {
            let current = {
                let mut state = self.state.borrow_mut();
                if state.read.is_some_and(|flight| flight.token == token) {
                    state.read = None;
                    state.current(token.generation) && state.epoch == token.epoch
                } else {
                    false
                }
            };
            if current {
                match result {
                    Ok(Some(layout)) => self.present_layout(token.generation, layout),
                    Ok(None) => self.fail_scope(
                        token.generation,
                        "No display is available for the Power menu.".into(),
                    ),
                    Err(error) => self.fail_scope(
                        token.generation,
                        format!("Could not read Power menu displays: {error}"),
                    ),
                }
            }
        }
        self.pump_read();
        self.pump_updates();
        // Only finite terminal draining while a current presentation exists.
        // Presentationless terminals/root receipts may wait for explicit reopen.
        let pending = self.mailbox.lock().pending();
        if pending {
            self.schedule_work();
        }
    }

    fn generation_is(&self, generation: u64) -> bool {
        let state = self.state.borrow();
        !state.closed && state.generation == generation
    }

    fn current(&self, generation: u64) -> bool {
        self.state.borrow().current(generation)
    }

    fn retired(&self, generation: u64) -> bool {
        let state = self.state.borrow();
        !state.closed && state.generation == generation && !state.desired
    }

    fn project(&self, generation: u64) {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        let (busy, enabled, pending, choice, user_name, status) = {
            let state = self.state.borrow();
            if state.closed || state.generation != generation || state.epoch != epoch {
                return;
            }
            (
                state.lock.is_some(),
                state.current(generation)
                    && state.authorized == Some(generation)
                    && state.lock.is_none(),
                state.updates_hint == Some(PowerUpdateHint::Pending),
                state.install_updates,
                state.user_name.clone(),
                state.updates_status.clone(),
            )
        };
        macro_rules! apply {
            ($effect:expr) => {
                if !self.scope_is(generation, epoch) {
                    return;
                }
                $effect;
                if !self.scope_is(generation, epoch) {
                    return;
                }
            };
        }
        apply!(surface.set_lock_busy(busy));
        apply!(surface.set_action_enabled(enabled));
        apply!(surface.set_updates_known_pending(pending));
        apply!(surface.set_install_updates(choice));
        apply!(surface.set_user_name(user_name.into()));
        apply!(surface.set_updates_status(status.into()));
    }

    fn apply_theme(&self, generation: u64) {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        let theme = self.state.borrow().theme;
        let scheme = match theme {
            Theme::System => slint::language::ColorScheme::Unknown,
            Theme::Light => slint::language::ColorScheme::Light,
            Theme::Dark => slint::language::ColorScheme::Dark,
        };
        if !self.scope_is(generation, epoch) || self.state.borrow().theme != theme {
            return;
        }
        surface.global::<Palette>().set_color_scheme(scheme);
        if !self.scope_is(generation, epoch) || self.state.borrow().theme != theme {
            return;
        }
        surface.global::<SeelenPalette>().set_color_scheme(scheme);
    }

    fn pump_read(&self) {
        let reservation = {
            let mut state = self.state.borrow_mut();
            if state.closed
                || !state.desired
                || !state.read_dirty
                || state.read.is_some()
                || self.surface.borrow().is_none()
            {
                return;
            }
            let token = match state.token() {
                Ok(token) => token,
                Err(error) => {
                    drop(state);
                    self.report(Err(error));
                    self.close();
                    return;
                }
            };
            state.read_dirty = false;
            state.read = Some(Flight {
                token,
                phase: Phase::Reserved,
            });
            (token, state.display.clone())
        };
        let (token, cached) = reservation;
        self.mailbox.lock().read.expected = Some(token);
        let provider = match cached {
            Some(provider) => Some(provider),
            None => match self.host.display_context_host() {
                Ok(Some(provider)) => {
                    {
                        let mut state = self.state.borrow_mut();
                        if !state.closed && state.display.is_none() {
                            state.display = Some(provider.clone());
                        }
                    }
                    Some(provider)
                }
                Ok(None) => {
                    self.finish_read(token, Err(DisplayContextError::Unsupported));
                    None
                }
                Err(error) => {
                    self.finish_read(token, Err(error));
                    None
                }
            },
        };
        let Some(provider) = provider else {
            return;
        };
        if !self.read_current(token) {
            self.cancel_read(token);
            self.schedule_work();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if let Some(flight) = state.read.as_mut().filter(|flight| flight.token == token) {
                flight.phase = Phase::Submitted;
            } else {
                return;
            }
        }
        let mailbox = self.mailbox.clone();
        let result = provider.read(Box::new(move |result| {
            complete_read(&mailbox, token, result)
        }));
        if let Err(error) = result {
            self.finish_read(token, Err(error));
        }
    }

    fn read_current(&self, token: Token) -> bool {
        let state = self.state.borrow();
        state.current(token.generation)
            && state.epoch == token.epoch
            && state.read.is_some_and(|flight| flight.token == token)
    }

    fn finish_read(&self, token: Token, result: ReadResult) {
        complete_read(&self.mailbox, token, result);
    }

    fn clear_read_slot(&self, token: Token) {
        self.mailbox.lock().read.clear(token);
    }

    fn clear_lock_slot(&self, token: Token) {
        self.mailbox.lock().lock.clear(token);
    }

    fn cancel_read(&self, token: Token) {
        {
            let mut state = self.state.borrow_mut();
            if state
                .read
                .is_some_and(|flight| flight.token == token && flight.phase == Phase::Reserved)
            {
                state.read = None;
            }
        }
        self.clear_read_slot(token);
    }

    fn activate(&self, action: PowerMenuAction) {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        if !self.is_visible() {
            return;
        }
        // Snapshot the closed typed intent at genuine input reservation, before
        // any generated getter, hide, lease Drop or provider callback can reenter.
        let (token, native_action) = {
            let mut state = self.state.borrow_mut();
            let generation = state.generation;
            if !state.current(generation)
                || state.epoch != epoch
                || state.authorized != Some(generation)
                || state.lock.is_some()
            {
                return;
            }
            let updates =
                if state.updates_hint == Some(PowerUpdateHint::Pending) && state.install_updates {
                    PowerUpdatePolicy::RequestInstallation
                } else {
                    PowerUpdatePolicy::OmitExplicitInstallation
                };
            let native_action = match action {
                PowerMenuAction::LockSession => PowerAction::LockSession,
                PowerMenuAction::LogOut => PowerAction::LogOut,
                PowerMenuAction::PowerOff => PowerAction::PowerOff { updates },
                PowerMenuAction::Reboot => PowerAction::Reboot { updates },
                PowerMenuAction::Suspend => PowerAction::Suspend,
                PowerMenuAction::Hibernate => PowerAction::Hibernate,
            };
            let token = match state.token() {
                Ok(token) => token,
                Err(error) => {
                    drop(state);
                    self.report(Err(error));
                    self.close();
                    return;
                }
            };
            state.lock = Some(Flight {
                token,
                phase: Phase::Reserved,
            });
            (token, native_action)
        };
        let enabled = surface.get_action_enabled();
        let busy = surface.get_lock_busy();
        if !self.scope_is(token.generation, epoch) || !enabled || busy || !self.is_visible() {
            // These flags were not projected busy yet. Do not turn a genuinely
            // disabled widget back on merely because injected input was rejected.
            {
                let mut state = self.state.borrow_mut();
                if state
                    .lock
                    .is_some_and(|flight| flight.token == token && flight.phase == Phase::Reserved)
                {
                    state.lock = None;
                }
            }
            self.clear_lock_slot(token);
            return;
        }
        let Some(retirement) = token.generation.checked_add(1) else {
            self.cancel_lock(token);
            self.close();
            return;
        };
        // Exact own hide and epoch, never broad SurfaceNone, authorize entry.
        self.hide();
        if !self.lock_retired(token, retirement) {
            self.cancel_lock(token);
            return;
        }
        let cached = self.state.borrow().power.clone();
        let provider = match cached {
            Some(provider) => Ok(Some(provider)),
            None => self.host.power_host(),
        };
        let provider = match provider {
            Ok(Some(provider)) => {
                {
                    let mut state = self.state.borrow_mut();
                    if !state.closed && state.power.is_none() {
                        state.power = Some(provider.clone());
                    }
                }
                provider
            }
            result => {
                let error = match result {
                    Ok(None) => PowerError::Unsupported,
                    Err(error) => error,
                    Ok(Some(_)) => unreachable!(),
                };
                let current = self.lock_retired(token, retirement);
                self.cancel_lock(token);
                if current && self.retired(retirement) {
                    self.report(Err(format!("Power request failed: {error}")));
                }
                return;
            }
        };
        if !self.lock_retired(token, retirement) {
            self.cancel_lock(token);
            return;
        }
        self.mailbox.lock().lock.expected = Some(token);
        {
            let mut state = self.state.borrow_mut();
            let Some(flight) = state.lock.as_mut().filter(|flight| flight.token == token) else {
                return;
            };
            flight.phase = Phase::Submitted;
        }
        let mailbox = self.mailbox.clone();
        let result = provider.perform(
            native_action,
            Box::new(move |result| {
                complete_lock(&mailbox, token, result);
            }),
        );
        // Accepted work outlives every presentation. No replay, queue or join.
        if let Err(error) = result {
            complete_lock(&self.mailbox, token, Err(error));
        }
    }

    fn lock_retired(&self, token: Token, retirement: u64) -> bool {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return false;
        };
        let reserved = {
            let state = self.state.borrow();
            !state.closed
                && state.generation == retirement
                && state.epoch == token.epoch
                && epoch == token.epoch
                && !state.desired
                && state
                    .lock
                    .is_some_and(|flight| flight.token == token && flight.phase == Phase::Reserved)
        };
        reserved
            && !surface.is_visible()
            && !surface.window().is_visible()
            && self.scope_is(retirement, epoch)
    }

    fn cancel_lock(&self, token: Token) {
        let generation = {
            let mut state = self.state.borrow_mut();
            if state
                .lock
                .is_some_and(|flight| flight.token == token && flight.phase == Phase::Reserved)
            {
                state.lock = None;
            }
            state.generation
        };
        self.clear_lock_slot(token);
        self.project(generation);
    }

    fn apply_fit(
        &self,
        generation: u64,
        layout: DisplayLayout,
        fit: LogicalFit,
        native_scale: f32,
    ) -> bool {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return false;
        };
        let current = || {
            self.current(generation)
                && self.epoch_is(epoch)
                && self.state.borrow().layout == Some(layout)
                && surface.window().scale_factor() == native_scale
        };
        macro_rules! apply {
            ($effect:expr) => {
                if !current() {
                    return false;
                }
                $effect;
                if !current() {
                    return false;
                }
            };
        }
        apply!(surface.set_metric_scale(fit.metric));
        apply!(
            surface
                .global::<FocusTokens>()
                .set_outline_width(2.0 * fit.metric)
        );
        apply!(
            surface
                .global::<FocusTokens>()
                .set_outline_offset(2.0 * fit.metric)
        );
        apply!(surface.set_selected_x(fit.x));
        apply!(surface.set_selected_y(fit.y));
        apply!(surface.set_selected_width(fit.width));
        apply!(surface.set_selected_height(fit.height));
        true
    }

    fn fit_current_scale(&self, generation: u64, layout: DisplayLayout) -> Result<bool, String> {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return Ok(false);
        };
        // A reentrant SDK scale change invalidates the entire old conversion,
        // even if visibility did not change. One bounded retry settles it.
        for _ in 0..2 {
            if !self.current(generation)
                || !self.epoch_is(epoch)
                || self.state.borrow().layout != Some(layout)
            {
                return Ok(false);
            }
            let native_scale = surface.window().scale_factor();
            let fit = LogicalFit::new(layout, native_scale)?;
            if self.apply_fit(generation, layout, fit, native_scale) {
                return Ok(true);
            }
        }
        if !self.current(generation)
            || !self.epoch_is(epoch)
            || self.state.borrow().layout != Some(layout)
        {
            return Ok(false);
        }
        Err("The Power surface scale did not settle during fitting.".into())
    }

    fn present_layout(&self, generation: u64, layout: DisplayLayout) {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        let frame = match Frame::from_layout(layout) {
            Ok(frame) => frame,
            Err(error) => {
                self.fail_scope(generation, error);
                return;
            }
        };
        if !self.current(generation) || !self.epoch_is(epoch) {
            return;
        }
        let (opening, metadata_ready) = {
            let mut state = self.state.borrow_mut();
            state.layout = Some(layout);
            state.authorized = None;
            let ready = state.initialized || state.initial_updates_generation == Some(generation);
            // Source initialization ends at the metadata barrier, not native
            // window attachment success. Later trigger refresh is non-awaited.
            if ready {
                state.initialized = true;
            }
            (state.opening, ready)
        };
        if !metadata_ready {
            return;
        }
        if !opening && !self.is_visible() {
            self.fail_scope(
                generation,
                "The Power menu lost its visible native surface.".into(),
            );
            return;
        }
        self.project(generation);
        if !self.current(generation) || !self.epoch_is(epoch) {
            return;
        }
        match self.fit_current_scale(generation, layout) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.fail_scope(generation, error);
                return;
            }
        }
        let newly_presented = !self.is_visible();
        let reposition = self.needs_reposition(frame);
        if !self.current(generation) || !self.epoch_is(epoch) {
            return;
        }
        let native_effect = NativeEffect::enter(&self.native_effect);
        let presented = if newly_presented {
            surface.present(frame.position, frame.size)
        } else if reposition {
            Ok(surface.reposition(frame.position, frame.size))
        } else {
            Ok(true)
        };
        drop(native_effect);
        self.arm_completed_hide();
        // A root motion-off signal may arrive after the native presenter's
        // initial permission read. Never restore that stale permission.
        if self.current(generation) && self.epoch_is(epoch) && self.motion_suppressed.get() {
            surface.disable_motion();
        }
        match presented {
            Ok(true) if self.current(generation) && self.epoch_is(epoch) && self.is_visible() => {
                self.frame.set(Some(frame))
            }
            Ok(_) => {
                if self.current(generation) {
                    self.fail_scope(generation, "The Power menu did not become visible.".into());
                }
                return;
            }
            Err(_) => {
                self.fail_scope(
                    generation,
                    "Could not present the native Power menu surface.".into(),
                );
                return;
            }
        }
        // Native creation/configuration may change the current root scale.
        // Refit only: never replay show, foreground or presentation motion.
        match self.refit(generation) {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                self.fail_scope(generation, error);
                return;
            }
        }
        if !self.current(generation) || !self.epoch_is(epoch) || !self.is_visible() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.authorized = Some(generation);
            state.opening = false;
        }
        self.project(generation);
        if !self.current(generation) || !self.epoch_is(epoch) || !self.is_visible() {
            return;
        }
        if newly_presented {
            (self.on_opened)();
            if !self.current(generation) || !self.epoch_is(epoch) || !self.is_visible() {
                return;
            }
            surface.invoke_focus_content();
            if !self.current(generation) || !self.epoch_is(epoch) || !self.is_visible() {
                return;
            }
            let focus = surface.request_focus();
            if !self.current(generation) || !self.epoch_is(epoch) || !self.is_visible() {
                return;
            }
            if focus.is_err() {
                self.report(Err(
                    "Power menu opened, but keyboard focus was not granted.".into(),
                ));
            }
        }
        if self.current(generation) && self.epoch_is(epoch) && self.is_visible() {
            self.watch_focus(generation);
        }
    }

    fn needs_reposition(&self, frame: Frame) -> bool {
        let Some((_, surface)) = self.surface_snapshot() else {
            return false;
        };
        self.frame.get() != Some(frame)
            || surface.window().size() != frame.size
            || surface.window().position() != frame.position
    }

    fn refit(&self, generation: u64) -> Result<bool, String> {
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return Ok(false);
        };
        if !self.current(generation) || !self.is_visible() {
            return Ok(false);
        }
        let Some(layout) = self.state.borrow().layout else {
            return Ok(false);
        };
        let frame = Frame::from_layout(layout)?;
        if !self.fit_current_scale(generation, layout)? {
            return Ok(false);
        }
        // DPI/backend resize can drift physical geometry while the requested
        // layout stays equal. Compare SDK truth, not only our last request.
        let reposition = self.needs_reposition(frame);
        if !self.current(generation) || !self.epoch_is(epoch) {
            return Ok(false);
        }
        if reposition {
            let native_effect = NativeEffect::enter(&self.native_effect);
            let repositioned = surface.reposition(frame.position, frame.size);
            drop(native_effect);
            self.arm_completed_hide();
            if !repositioned || !self.current(generation) || !self.epoch_is(epoch) {
                return Ok(false);
            }
            self.frame.set(Some(frame));
            if !self.fit_current_scale(generation, layout)? {
                return Ok(false);
            }
        }
        Ok(self.current(generation) && self.epoch_is(epoch) && self.is_visible())
    }

    fn refit_or_report(&self, generation: u64) {
        if let Err(error) = self.refit(generation) {
            self.fail_scope(generation, error);
        }
    }

    fn fail_scope(&self, generation: u64, error: String) {
        if !self.current(generation) {
            return;
        }
        self.hide();
        if self.retired(generation.checked_add(1).unwrap_or(generation)) {
            self.report(Err(error));
        }
    }

    fn report(&self, result: Result<(), String>) {
        if !self.state.borrow().closed {
            (self.on_result)(result);
        }
    }

    fn schedule_fit(&self) {
        if !self.is_visible() || self.fit_timer.running() {
            return;
        }
        let generation = self.state.borrow().generation;
        let Some((epoch, _)) = self.surface_snapshot() else {
            return;
        };
        let weak = self.weak.clone();
        self.fit_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak
                    .upgrade()
                    .filter(|controller| controller.scope_is(generation, epoch))
                {
                    controller.refit_or_report(generation);
                }
            });
    }

    fn schedule_work(&self) {
        if self.surface_snapshot().is_none() || self.work_timer.running() {
            return;
        }
        let weak = self.weak.clone();
        self.work_timer
            .start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.process_events();
                }
            });
    }

    fn watch_focus(&self, generation: u64) {
        let Some((epoch, _)) = self.surface_snapshot() else {
            return;
        };
        if !self.current(generation) || !self.is_visible() {
            return;
        }
        let focus = self.is_focused();
        self.focus_seen
            .set(self.focus_seen.get() || focus == Some(true));
        if focus.is_none() {
            return;
        }
        let weak = self.weak.clone();
        self.focus_watch.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(100),
            move || {
                if let Some(controller) = weak.upgrade() {
                    if !controller.current(generation)
                        || !controller.epoch_is(epoch)
                        || !controller.is_visible()
                    {
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
        let (epoch, surface) = self.surface_snapshot()?;
        let focus = surface
            .window()
            .with_winit_window(|window| window.has_focus());
        if self.epoch_is(epoch) { focus } else { None }
    }

    #[cfg(not(any(windows, test)))]
    fn is_focused(&self) -> Option<bool> {
        None
    }
}

impl Drop for PowerMenuController {
    fn drop(&mut self) {
        self.close();
    }
}
