// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! User-opened WLAN cache popup with optional scoped connection controls. No scan timer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use slint::{ComponentHandle, ModelRc, PhysicalPosition, PhysicalSize, SharedString};
use tessera_system::network::{
    NetworkCommand, NetworkCommandOutcome, NetworkConnectCapability, NetworkControl,
    NetworkControlInventory, NetworkControlView, NetworkError, NetworkErrorKind, NetworkEvent,
    NetworkHost, NetworkPassword, NetworkRadioControl, NetworkRadioInitiation,
    NetworkRadioInventory, NetworkRadioResult, NetworkSnapshot, NetworkTarget, Observation,
    RadioState,
};

use crate::generated::{NetworkMenu, PopoverMotion, TileBounds};
use crate::popup_placement::{self as placement, PopupRect};
use crate::sanitize::bounded_text;
use crate::theme::{PresentationTheme, ThemedComponent};
use crate::transient_window::{TransientComponent, TransientWindow};
use crate::{DesktopHost, DockContext, SurfaceKind};

mod controls;
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
    controls: Vec<NetworkControl>,
    controls_supported: bool,
    radios: Vec<NetworkRadioControl>,
    radios_supported: bool,
    radio_keys: Vec<(SharedString, NetworkTarget, bool)>,
    radio_notice: String,
    command_radio: bool,
    selected: Option<NetworkControl>,
    row_keys: Vec<(SharedString, NetworkTarget)>,
    command_key: SharedString,
    command_flight: Option<Token>,
    control_notice: String,
    inventory_notice: String,
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
        let Some(sequence) = self
            .key_sequence
            .checked_add(1)
            .filter(|value| *value != u64::MAX)
        else {
            self.key_sequence = u64::MAX;
            return SharedString::default();
        };
        self.key_sequence = sequence;
        format!("network-{:x}-{:x}", self.session, self.key_sequence).into()
    }
    fn token(&mut self) -> Token {
        self.sequence = self.sequence.saturating_add(1);
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
        session != u64::MAX && self.is_open() && self.state.borrow().session == session
    }
    fn accepts_input(&self) -> bool {
        let open = self.is_open();
        let state = self.state.borrow();
        open && !self.projecting.get()
            && !self.presenting.get()
            && state.session != u64::MAX
            && state.sequence != u64::MAX
            && state.key_sequence != u64::MAX
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
            state.inventory_notice.clear();
            if state.command_flight.is_none() {
                state.control_notice.clear();
            }
            state.controls.clear();
            state.selected = None;
            state.row_keys.clear();
            state.radios.clear();
            state.radio_keys.clear();
            state.command_key = SharedString::default();
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
        if self.state.borrow().sequence == u64::MAX {
            {
                let mut state = self.state.borrow_mut();
                state.read_requested = false;
                state.notice = "Network request counter exhausted; restart required.".into();
            }
            self.project_and_fit();
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
        if let Err(error) = provider.read_control_view(Box::new(move |result| {
            mailbox::control_view_complete(&mailbox, &root, token, result)
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
                    state.controls.clear();
                    state.controls_supported = false;
                    state.selected = None;
                    state.row_keys.clear();
                    state.command_key = SharedString::default();
                    state.radios.clear();
                    state.radio_keys.clear();
                    state.radios_supported = false;
                    state.radio_notice.clear();
                    if let Some((inventory_token, inventory)) = delivery.radios
                        && inventory_token == token
                    {
                        state.radios_supported = inventory.is_some();
                        match inventory {
                            Some(Observation::Ready(radios)) => {
                                state.radios = radios.interfaces;
                                state.radio_notice = radios
                                    .unavailable
                                    .first()
                                    .map(|error| {
                                        format!(
                                            "Some radio facts/controls are unavailable. {}",
                                            failure(error.kind)
                                        )
                                    })
                                    .unwrap_or_default();
                            }
                            Some(Observation::Unavailable(error)) => {
                                state.radio_notice = format!(
                                    "Radio facts/controls unavailable. {}",
                                    failure(error.kind)
                                );
                            }
                            None => {}
                        }
                    }
                    if let Some((inventory_token, inventory)) = delivery.controls
                        && inventory_token == token
                    {
                        state.controls_supported = inventory.is_some();
                        match inventory {
                            Some(Observation::Ready(controls)) => {
                                state.controls = controls.networks;
                                state.inventory_notice = controls
                                    .unavailable
                                    .first()
                                    .map(|error| {
                                        format!(
                                            "Some connection targets are unavailable. {}",
                                            failure(error.kind)
                                        )
                                    })
                                    .unwrap_or_default();
                            }
                            Some(Observation::Unavailable(error)) => {
                                state.inventory_notice = format!(
                                    "Connection controls unavailable. {}",
                                    failure(error.kind)
                                )
                            }
                            None => state.inventory_notice.clear(),
                        }
                    }
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
            if let Some(token) = delivery.accepted
                && state.command_flight == Some(token)
                && visible
                && state.session == token.session
            {
                state.control_notice = if state.command_radio {
                    "Windows accepted at least one software-radio write; each PHY still requires actual readback."
                } else {
                    "Windows accepted the connection request; connecting/disconnecting is pending actual native readback."
                }.into();
            }
            if let Some((token, result)) = delivery.command
                && state.command_flight == Some(token)
            {
                state.command_flight = None;
                if visible && state.session == token.session {
                    state.control_notice = match result {
                        Ok(NetworkCommandOutcome::Connected) => "Windows readback confirms connection to the selected access point. Internet access is not confirmed.".into(),
                        Ok(NetworkCommandOutcome::Disconnected) => "Windows readback confirms that the selected adapter is disconnected.".into(),
                        Ok(NetworkCommandOutcome::RadioObserved(result)) => {
                            controls::apply_radio_result(&mut state, &result);
                            controls::radio_result_notice(&result)
                        },
                        Ok(NetworkCommandOutcome::Failed { reason }) => {
                            state.automatic_blocked = true;
                            format!("Windows could not complete the Wi-Fi connection (WLAN reason {reason}). No retry was issued.")
                        }
                        Ok(NetworkCommandOutcome::AcceptedUnconfirmed) => "Windows accepted the request, but connection completion is unconfirmed. No retry was issued. Refresh for current facts.".into(),
                        Err(error) => {
                            state.automatic_blocked = true;
                            format!("Wi-Fi request did not complete. {}", failure(error.kind))
                        },
                    };
                    state.selected = None;
                    if state.command_radio {
                        state.controls.clear();
                        state.row_keys.clear();
                        state.radio_keys.clear();
                        for radio in &mut state.radios {
                            radio.target = None;
                        }
                    }
                    state.read_requested = !state.automatic_blocked;
                }
            }
            if visible && delivery.watch_session == Some(state.session) {
                match delivery.watch {
                    Some(NetworkEvent::WatchReady) => {
                        state.watch_status =
                            "Windows cache change notifications active; no active scanning.".into()
                    }
                    Some(NetworkEvent::WatchUnavailable(error)) => {
                        state.controls.clear();
                        state.selected = None;
                        state.row_keys.clear();
                        state.radios.clear();
                        state.radio_keys.clear();
                        state.command_key = SharedString::default();
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
                if delivery.changed {
                    state.controls.clear();
                    state.selected = None;
                    state.row_keys.clear();
                    state.radios.clear();
                    state.radio_keys.clear();
                    state.command_key = SharedString::default();
                    if !state.automatic_blocked {
                        state.read_requested = true;
                    }
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
        let open = self.is_open();
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
            let input = open && !self.presenting.get();
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
            let notice = [
                &state.notice,
                &state.settings_notice,
                &state.control_notice,
                &state.inventory_notice,
                &state.radio_notice,
            ]
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
        self.project_controls();
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
