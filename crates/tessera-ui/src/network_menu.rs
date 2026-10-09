// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User-opened, read-only WLAN cache popup. No startup discovery or scan timer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString};
use tessera_system::network::{
    NetworkError, NetworkErrorKind, NetworkEvent, NetworkHost, NetworkSnapshot,
};

use crate::generated::{NetworkMenu, PopoverMotion, TileBounds};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

mod lifecycle;
mod mailbox;
mod presentation;
#[cfg(test)]
mod tests;

use mailbox::Mailbox;
use presentation::failure;

impl TransientComponent for NetworkMenu {
    fn motion(&self) -> PopoverMotion<'_> {
        self.global::<PopoverMotion>()
    }
    fn set_presentation_opacity(&self, opacity: f32) {
        self.invoke_set_presentation_opacity(opacity);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Token {
    session: u64,
    sequence: u64,
}

#[derive(Default)]
struct State {
    session: u64,
    sequence: u64,
    key_sequence: u64,
    provider: Option<Arc<dyn NetworkHost>>,
    acquiring: bool,
    read_requested: bool,
    flight: Option<Token>,
    settings_flight: Option<Token>,
    snapshot: Option<NetworkSnapshot>,
    refresh_key: SharedString,
    settings_key: SharedString,
    notice: String,
    settings_notice: String,
    watch_status: String,
    watch_started: bool,
    subscribing: bool,
    watch_guard: Option<Box<dyn Send>>,
    automatic_blocked: bool,
}

impl State {
    fn loading(&self) -> bool {
        self.acquiring || self.subscribing || self.read_requested || self.flight.is_some()
    }
    fn key(&mut self) -> SharedString {
        self.key_sequence = self.key_sequence.wrapping_add(1);
        format!("network-{:x}-{:x}", self.session, self.key_sequence).into()
    }
    fn token(&mut self) -> Token {
        self.sequence = self.sequence.wrapping_add(1);
        Token {
            session: self.session,
            sequence: self.sequence,
        }
    }
}

#[derive(Clone, Copy)]
struct Placement {
    anchor: PhysicalPosition,
    context: DockContext,
    scale: f32,
}
struct Guard<'a>(&'a Cell<bool>);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct NetworkController {
    surface: TransientWindow<NetworkMenu>,
    host: Arc<dyn DesktopHost>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    projecting: Cell<bool>,
    presenting: Cell<bool>,
    placement: Cell<Option<Placement>>,
    rect: RefCell<Option<PopupRect>>,
    fit_timer: slint::Timer,
    focus_watch: slint::Timer,
    focus_seen: Cell<bool>,
}

impl NetworkController {
    fn current(&self, session: u64) -> bool {
        self.is_open() && self.state.borrow().session == session
    }
    fn accepts_input(&self) -> bool {
        self.is_open() && !self.projecting.get() && !self.presenting.get()
    }

    fn refresh(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if key.is_empty() || key != state.refresh_key || state.loading() {
                return;
            }
            // Only an explicit user retry lifts the privacy/failure barrier.
            state.automatic_blocked = false;
            state.notice.clear();
            state.settings_notice.clear();
            state.read_requested = true;
        }
        self.pump();
    }

    fn settings(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() {
            return;
        }
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if key.is_empty() || key != state.settings_key || state.settings_flight.is_some() {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let token = state.token();
            state.settings_flight = Some(token);
            state.settings_notice.clear();
            (provider, token)
        };
        self.mailbox.lock().expected_settings = Some(token);
        self.project_and_fit();
        if !self.current(token.session) {
            self.state.borrow_mut().settings_flight = None;
            self.mailbox.lock().expected_settings = None;
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        if let Err(error) = provider.open_settings(Box::new(move |result| {
            mailbox::settings_complete(&mailbox, &root, token, result)
        })) {
            mailbox::settings_complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
    }

    // Provider factories and effects are prompt acceptance boundaries. Never
    // hold UI-state/native-lease borrows while invoking an external host.
    fn pump(self: &Rc<Self>) {
        if !self.is_open() {
            return;
        }
        let acquire = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some()
                || state.acquiring
                || state.subscribing
                || !state.read_requested
            {
                return;
            }
            if state.provider.is_none() {
                state.acquiring = true;
                Some(state.session)
            } else {
                None
            }
        };
        if let Some(session) = acquire {
            let result = self.host.network_host();
            {
                let mut state = self.state.borrow_mut();
                state.acquiring = false;
                match result {
                    Ok(Some(provider)) => state.provider = Some(provider),
                    result if state.session == session => {
                        state.read_requested = false;
                        state.automatic_blocked = true;
                        state.notice = match result {
                            Err(error) => failure(error.kind),
                            Ok(None) => failure(NetworkErrorKind::Unsupported),
                            Ok(Some(_)) => unreachable!(),
                        }
                        .into();
                    }
                    _ => {}
                }
            }
            self.project_and_fit();
            if !self.current(session) {
                self.pump();
                return;
            }
        }
        let session = self.state.borrow().session;
        self.subscribe_watch();
        if !self.current(session) {
            self.pump();
            return;
        }
        let (provider, token) = {
            let mut state = self.state.borrow_mut();
            if !self.is_open() || state.flight.is_some() || !state.read_requested {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            state.read_requested = false;
            let token = state.token();
            state.flight = Some(token);
            (provider, token)
        };
        self.mailbox.lock().expected_read = Some(token);
        self.project_and_fit();
        if !self.current(token.session) {
            self.state.borrow_mut().flight = None;
            self.mailbox.lock().expected_read = None;
            self.pump();
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        if let Err(error) = provider.read(Box::new(move |result| {
            mailbox::read_complete(&mailbox, &root, token, result)
        })) {
            mailbox::read_complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
    }

    fn subscribe_watch(&self) {
        let (provider, session) = {
            let mut state = self.state.borrow_mut();
            if state.watch_started || state.subscribing || !self.is_open() {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            state.watch_started = true;
            state.subscribing = true;
            state.watch_status = "Starting Windows cache change notifications…".into();
            (provider, state.session)
        };
        self.mailbox.lock().watch_session = Some(session);
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        let result = provider.subscribe(Arc::new(move |event| {
            mailbox::event(&mailbox, &root, session, event)
        }));
        let mut retired = None;
        let mut close_admission = false;
        {
            let mut state = self.state.borrow_mut();
            state.subscribing = false;
            if state.session != session || !self.is_open() {
                if let Ok(guard) = result {
                    retired = guard;
                }
            } else {
                match result {
                    Ok(Some(guard)) => state.watch_guard = Some(guard),
                    Ok(None) => {
                        close_admission = true;
                        state.watch_status =
                            "Change notifications unsupported; use Refresh to re-read the cache."
                                .into();
                    }
                    Err(error) => {
                        close_admission = true;
                        state.watch_status = format!(
                            "Change notifications unavailable. {} Use Refresh.",
                            failure(error.kind)
                        );
                    }
                }
            }
        }
        if close_admission {
            let mut mailbox = self.mailbox.lock();
            if mailbox.watch_session == Some(session) {
                mailbox.close_watch();
            }
        }
        drop(retired);
    }

    fn drain(self: &Rc<Self>) {
        if self.projecting.get() || self.presenting.get() {
            return;
        }
        let delivery = self.mailbox.lock().take();
        let visible = self.is_open();
        {
            let mut state = self.state.borrow_mut();
            if let Some((token, result)) = delivery.read
                && state.flight == Some(token)
            {
                state.flight = None;
                if visible && state.session == token.session {
                    match result {
                        Ok(snapshot) => {
                            state.automatic_blocked =
                                presentation::automatic_retry_blocked(&snapshot);
                            state.snapshot = Some(snapshot);
                            state.notice.clear();
                        }
                        Err(error) => {
                            state.notice = format!(
                                "{}{}",
                                failure(error.kind),
                                if state.snapshot.is_some() {
                                    " The previous cache snapshot remains displayed below."
                                } else {
                                    ""
                                }
                            );
                            state.automatic_blocked = true;
                        }
                    }
                    // A queued dirty hint cannot retry a denied/failed read.
                    if state.automatic_blocked {
                        state.read_requested = false;
                    }
                }
            }
            if let Some((token, result)) = delivery.settings
                && state.settings_flight == Some(token)
            {
                state.settings_flight = None;
                if visible && state.session == token.session {
                    state.settings_notice = match result {
                        Ok(()) => "Windows accepted the Network Settings request; this does not confirm that Settings is visible.".into(),
                        Err(error) => format!("Network Settings could not be opened. {}", failure(error.kind)),
                    };
                }
            }
            if visible && delivery.watch_session == Some(state.session) {
                match delivery.watch {
                    Some(NetworkEvent::WatchReady) => {
                        state.watch_status =
                            "Windows cache change notifications active; no active scanning.".into()
                    }
                    Some(NetworkEvent::WatchUnavailable(error)) => {
                        state.watch_status = format!(
                            "Change notifications unavailable. {} Use Refresh.",
                            failure(error.kind)
                        );
                        if error.kind == NetworkErrorKind::AccessDenied {
                            state.automatic_blocked = true;
                            state.read_requested = false;
                        }
                    }
                    _ => {}
                }
                if delivery.changed && !state.automatic_blocked {
                    state.read_requested = true;
                }
            }
        }
        if visible {
            self.project_and_fit();
            self.pump();
        }
    }

    fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = Guard(&self.projecting);
        let (
            session,
            projection,
            loading,
            settings_loading,
            notice,
            watch_status,
            refresh_key,
            settings_key,
        ) = {
            let mut state = self.state.borrow_mut();
            let projection = state
                .snapshot
                .as_ref()
                .map(presentation::project)
                .unwrap_or_default();
            let loading = state.loading();
            let settings_loading = state.settings_flight.is_some();
            let input = self.is_open() && !self.presenting.get();
            state.refresh_key = if input && !loading {
                state.key()
            } else {
                SharedString::default()
            };
            state.settings_key = if input && !settings_loading && state.provider.is_some() {
                state.key()
            } else {
                SharedString::default()
            };
            let notice = [&state.notice, &state.settings_notice]
                .into_iter()
                .filter(|notice| !notice.is_empty())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            (
                state.session,
                projection,
                loading,
                settings_loading,
                notice,
                state.watch_status.clone(),
                state.refresh_key.clone(),
                state.settings_key.clone(),
            )
        };
        let root = self.component();
        root.set_refresh_key(SharedString::default());
        root.set_settings_key(SharedString::default());
        root.set_loading(loading);
        root.set_settings_loading(settings_loading);
        root.set_notice(notice.into());
        root.set_watch_status(watch_status.into());
        root.set_radio_text(projection.radio.into());
        root.set_summary(projection.summary.into());
        root.set_connected(ModelRc::new(slint::VecModel::from(projection.connected)));
        root.set_saved(ModelRc::new(slint::VecModel::from(projection.saved)));
        root.set_available(ModelRc::new(slint::VecModel::from(projection.available)));
        root.set_hidden(ModelRc::new(slint::VecModel::from(projection.hidden)));
        if self.current(session) {
            root.set_refresh_key(refresh_key);
            root.set_settings_key(settings_key);
        }
    }

    fn project_and_fit(&self) {
        self.project();
        let _ = self.refit();
    }
}

impl Drop for NetworkController {
    fn drop(&mut self) {
        self.hide();
    }
}
