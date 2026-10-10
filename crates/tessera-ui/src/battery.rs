// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One battery cache/watch for passive toolbar lifetime and independent popup leases.

use crate::generated::{BatteryMenu, BatteryRow, PopoverMotion, TileBounds};
use crate::popup_placement::{self as placement, PopupRect};
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};
use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use tessera_system::battery::{
    BatteryError, BatteryEvent, BatteryFact, BatteryHost, BatterySettingsAccepted, BatterySnapshot,
    BatteryState, EnergySaverState, PowerSupplyState,
};

mod lifecycle;
mod mailbox;
mod presentation;
use mailbox::Mailbox;

impl TransientComponent for BatteryMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }
    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

/// Parent binds through Weak<Toolbar> and readonly source admission, never a
/// popup operation or a strong controller cycle. Unknown/error remains visible.
#[derive(Clone)]
pub(crate) struct BatteryProjection {
    pub visible: bool,
    pub text: SharedString,
    pub accessible_label: SharedString,
    pub percent: Option<u8>,
    pub activation_key: SharedString,
}
#[derive(Clone, Copy, Eq, PartialEq)]
struct SettingsToken {
    id: u64,
    session: u64,
}
#[derive(Clone, Copy, PartialEq)]
struct Frame {
    position: PhysicalPosition,
    size: PhysicalSize,
    scale: f32,
}
#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}
#[derive(Default)]
struct State {
    sequence: u64,
    session: u64,
    exhausted: bool,
    root_active: bool,
    acquiring: bool,
    provider: Option<Arc<dyn BatteryHost>>,
    unsupported: bool,
    factory_error: Option<BatteryError>,
    snapshot: Option<BatterySnapshot>,
    stale: bool,
    change: u64,
    read_requested: bool,
    read_flight: Option<(u64, u64)>,
    read_error: Option<BatteryError>,
    settings_flight: Option<SettingsToken>,
    settings_notice: String,
    watch_epoch: Option<u64>,
    subscribing: bool,
    watch_guard: Option<Box<dyn Send>>,
    watch_status: String,
    refresh_key: SharedString,
    settings_key: SharedString,
    input_frame: Option<Frame>,
}
impl State {
    fn next(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        match self.sequence.checked_add(1) {
            Some(next) => {
                self.sequence = next;
                Some(next)
            }
            None => {
                self.exhausted = true;
                None
            }
        }
    }
    fn retire_session(&mut self) {
        match self.session.checked_add(1) {
            Some(next) => self.session = next,
            None => self.exhausted = true,
        }
        self.refresh_key = SharedString::default();
        self.settings_key = SharedString::default();
        self.input_frame = None;
    }
}
struct FlagGuard<'a>(&'a Cell<bool>);
impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

type ToolbarProjection = Rc<dyn Fn(BatteryProjection)>;

pub(crate) struct BatteryController {
    surface: TransientWindow<BatteryMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    toolbar_projection: RefCell<Option<ToolbarProjection>>,
    root_admission: RefCell<Option<Rc<dyn Fn() -> bool>>>,
    projecting: Cell<bool>,
    presenting: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    focus_seen: Cell<bool>,
}
impl BatteryController {
    fn active(&self) -> bool {
        let root_active = self.state.borrow().root_active;
        root_active || self.is_open()
    }
    fn root_admitted(&self) -> bool {
        let current = self.root_admission.borrow().clone();
        current.is_some_and(|current| current())
    }
    fn frame(&self) -> Frame {
        let window = self.component().window();
        Frame {
            position: window.position(),
            size: window.size(),
            scale: window.scale_factor(),
        }
    }
    fn current(&self, session: u64) -> bool {
        let current = {
            let state = self.state.borrow();
            state.session == session && !state.exhausted
        };
        current && self.is_open()
    }
    fn admits(&self, key: &SharedString, settings: bool) -> bool {
        if key.is_empty() || !self.is_open() || self.projecting.get() || self.presenting.get() {
            return false;
        }
        let frame = self.frame();
        let state = self.state.borrow();
        !state.exhausted
            && state.input_frame == Some(frame)
            && *key
                == if settings {
                    state.settings_key.clone()
                } else {
                    state.refresh_key.clone()
                }
    }
    fn refresh(self: &Rc<Self>, key: SharedString) {
        if !self.root_admitted() {
            self.stop_root();
            return;
        }
        if !self.admits(&key, false) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.read_flight.is_some() {
                return;
            }
            state.factory_error = None;
            state.read_error = None;
            state.read_requested = true;
        }
        self.pump();
    }
    fn settings(self: &Rc<Self>, key: SharedString) {
        if !self.root_admitted() {
            self.stop_root();
            return;
        }
        if !self.admits(&key, true) {
            return;
        }
        let frame = self.frame();
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if state.settings_flight.is_some() {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(id) = state.next() else {
                return;
            };
            let token = SettingsToken {
                id,
                session: state.session,
            };
            state.settings_flight = Some(token);
            state.settings_notice.clear();
            (provider, token)
        };
        self.mailbox.lock().settings_expected = Some(token);
        self.project();
        // Generated setters and projection may retire the source. Fitting our
        // own busy feedback is deferred until after admission, not a new source.
        if !self.root_admitted() || !self.current(token.session) || self.frame() != frame {
            self.state.borrow_mut().settings_flight = None;
            self.mailbox.lock().settings_expected = None;
            self.project();
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        if let Err(error) = provider.open_settings(Box::new(move |result| {
            mailbox::settings_complete(&mailbox, &root, token, result);
        })) {
            mailbox::settings_complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
        let _ = self.refit();
    }
    fn pump(self: &Rc<Self>) {
        if !self.root_admitted() {
            self.stop_root();
            return;
        }
        if !self.active() || self.state.borrow().exhausted || self.state.borrow().unsupported {
            self.project();
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.acquiring {
                return;
            }
            let acquire =
                state.provider.is_none() && !state.unsupported && state.factory_error.is_none();
            state.acquiring = acquire;
            acquire
        };
        if acquire {
            let result = self.host.battery_host();
            {
                let mut state = self.state.borrow_mut();
                state.acquiring = false;
                match result {
                    Ok(Some(provider)) => state.provider = Some(provider),
                    Ok(None) => {
                        state.unsupported = true;
                        state.read_requested = false;
                    }
                    Err(BatteryError::Unsupported) => {
                        state.unsupported = true;
                        state.read_requested = false;
                    }
                    Err(error) => {
                        state.factory_error = Some(error);
                        state.read_requested = false;
                        state.exhausted |= error == BatteryError::Exhausted;
                    }
                }
            }
            if !self.root_admitted() {
                self.stop_root();
                return;
            }
            self.project_and_fit();
            if !self.active() || self.state.borrow().exhausted {
                return;
            }
        }
        self.subscribe();
        if !self.root_admitted() {
            self.stop_root();
            return;
        }
        if !self.active() || self.state.borrow().exhausted {
            return;
        }
        let request = {
            let mut state = self.state.borrow_mut();
            if state.unsupported || !state.read_requested || state.read_flight.is_some() {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(id) = state.next() else {
                return;
            };
            state.read_requested = false;
            state.read_flight = Some((id, state.change));
            state.read_error = None;
            (provider, id)
        };
        let (provider, id) = request;
        self.mailbox.lock().read_expected = Some(id);
        self.project_and_fit();
        if !self.root_admitted() || !self.active() || self.state.borrow().exhausted {
            self.state.borrow_mut().read_flight = None;
            self.mailbox.lock().read_expected = None;
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        if let Err(error) = provider.read(Box::new(move |result| {
            mailbox::read_complete(&mailbox, &root, id, result);
        })) {
            mailbox::read_complete(&self.mailbox, &self.surface.as_weak(), id, Err(error));
        }
    }
    fn subscribe(&self) {
        if !self.root_admitted() || !self.active() {
            return;
        }
        let request = {
            let mut state = self.state.borrow_mut();
            if state.watch_epoch.is_some() || state.subscribing {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(epoch) = state.next() else {
                return;
            };
            state.watch_epoch = Some(epoch);
            state.subscribing = true;
            state.watch_status = "Starting Windows power notifications…".into();
            (provider, epoch)
        };
        let (provider, epoch) = request;
        self.mailbox.lock().watch_expected = Some(epoch);
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        let result = provider.subscribe(Arc::new(move |event| {
            mailbox::event(&mailbox, &root, epoch, event);
        }));
        let active = self.root_admitted() && self.active();
        let mut retired = None;
        let mut close = false;
        {
            let mut state = self.state.borrow_mut();
            state.subscribing = false;
            if state.watch_epoch != Some(epoch) || !active {
                if let Ok(guard) = result {
                    retired = guard;
                }
            } else {
                match result {
                    Ok(Some(guard)) => state.watch_guard = Some(guard),
                    Ok(None) => {
                        close = true;
                        state.watch_status =
                            "Windows power notifications unsupported; use Refresh.".into();
                    }
                    Err(error) => {
                        close = true;
                        state.exhausted |= error == BatteryError::Exhausted;
                        state.watch_status = format!(
                            "Windows power notifications unavailable: {error}. Use Refresh."
                        );
                    }
                }
            }
        }
        if close {
            self.mailbox.lock().close_watch();
        }
        drop(retired);
        if !self.root_admitted() {
            self.stop_root();
        }
    }
    fn retire_watch_if_idle(&self) {
        if self.active() {
            return;
        }
        self.mailbox.lock().close_watch();
        let retired = {
            let mut state = self.state.borrow_mut();
            state.watch_epoch = None;
            state.read_requested = false;
            state.stale = state.snapshot.is_some();
            state.watch_status =
                "Windows power notifications paused while the toolbar and popup are inactive."
                    .into();
            state.watch_guard.take()
        };
        drop(retired);
    }
    fn drain(self: &Rc<Self>) {
        if self.projecting.get() || self.presenting.get() {
            return;
        }
        if !self.root_admitted() {
            self.stop_root();
        }
        let delivery = self.mailbox.lock().take();
        let open = self.is_open();
        {
            let mut state = self.state.borrow_mut();
            // Apply invalidation first: a read racing a native event stays
            // explicitly stale and earns one coalesced subsequent cache read.
            if delivery.changed {
                if let Some(change) = state.change.checked_add(1) {
                    state.change = change;
                } else {
                    state.exhausted = true;
                }
                state.stale = true;
                state.read_requested = true;
            }
            if let Some((id, result)) = delivery.read
                && let Some((expected, change)) = state.read_flight
                && id == expected
            {
                state.read_flight = None;
                match result {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.stale = change != state.change;
                    }
                    Err(error) => {
                        state.read_error = Some(error);
                        state.stale = true;
                        state.exhausted |= error == BatteryError::Exhausted;
                        if error == BatteryError::Unsupported {
                            state.unsupported = true;
                            state.read_requested = false;
                        }
                    }
                }
            }
            if let Some((token, result)) = delivery.settings
                && state.settings_flight == Some(token)
            {
                state.settings_flight = None;
                state.exhausted |= result == Err(BatteryError::Exhausted);
                if open && state.session == token.session {
                    state.settings_notice = match result {
                        Ok(_) => "Windows accepted the Settings launch; visibility is unconfirmed."
                            .into(),
                        Err(error) => format!("Settings launch failed: {error}"),
                    };
                }
            }
            if let Some(event) = delivery.watch {
                state.watch_status = match event {
                    BatteryEvent::WatchReady => "Windows power change notifications active.".into(),
                    BatteryEvent::WatchUnavailable(error) => {
                        state.stale = state.snapshot.is_some();
                        state.exhausted |= error == BatteryError::Exhausted;
                        format!("Windows power notifications unavailable: {error}. Use Refresh.")
                    }
                    BatteryEvent::Changed => unreachable!(),
                };
            }
        }
        self.project_and_fit();
        self.pump();
    }
}
impl Drop for BatteryController {
    fn drop(&mut self) {
        self.fit_timer.stop();
        self.focus_watch.stop();
        self.mailbox.lock().close_watch();
        // Completion closures retain their mailbox; never join native work.
        let guard = self.state.get_mut().watch_guard.take();
        drop(guard);
        self.surface.hide();
    }
}
