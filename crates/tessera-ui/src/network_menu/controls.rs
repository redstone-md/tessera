// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::NetworkControlRow;

impl NetworkController {
    fn control_frame_valid(&self) -> bool {
        let Some(placement) = self.placement.get() else {
            return false;
        };
        let Some((position, size)) = self
            .rect
            .borrow()
            .as_ref()
            .map(|frame| (frame.position, frame.size))
        else {
            return false;
        };
        self.component().window().scale_factor() == placement.scale
            && self.component().window().position() == position
            && self.component().window().size() == size
    }

    pub(super) fn control_frame_changed(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.row_keys.clear();
            state.radio_keys.clear();
            state.command_key = SharedString::default();
        }
        self.component().set_command_key(SharedString::default());
        self.component().set_password(SharedString::default());
        self.project_controls();
    }

    pub(super) fn select_control(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() || !self.control_frame_valid() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if key.is_empty() || state.loading() || state.command_flight.is_some() {
                return;
            }
            let Some((_, target)) = state.row_keys.iter().find(|(issued, _)| *issued == key) else {
                return;
            };
            let Some(control) = state
                .controls
                .iter()
                .find(|control| control.target == *target)
                .cloned()
            else {
                return;
            };
            if !control.connected && control.connect == NetworkConnectCapability::Unavailable {
                return;
            }
            state.selected = Some(control);
            state.control_notice.clear();
        }
        // External replacement clears the pinned native TextInput's undo/offset
        // state; a retired credential slot also retires composition.
        self.component().set_credentials_active(false);
        self.component().set_password(SharedString::default());
        self.project_and_fit();
    }

    pub(super) fn submit_control(self: &Rc<Self>, key: SharedString, password: SharedString) {
        if !self.accepts_input() || !self.control_frame_valid() {
            return;
        }
        let (provider, token, command) = {
            let mut state = self.state.borrow_mut();
            if key.is_empty()
                || key != state.command_key
                || state.command_flight.is_some()
                || state.loading()
            {
                return;
            }
            let Some(selected) = state.selected.clone() else {
                return;
            };
            if !state.controls.iter().any(|control| control == &selected) {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let token = state.token();
            let command = if selected.connected {
                NetworkCommand::Disconnect {
                    target: selected.target,
                }
            } else {
                NetworkCommand::Connect {
                    target: selected.target,
                    password: (selected.connect == NetworkConnectCapability::Wpa2Personal)
                        .then(|| NetworkPassword::new(password.as_str())),
                }
            };
            state.command_flight = Some(token);
            state.command_radio = false;
            state.command_key = SharedString::default();
            state.row_keys.clear();
            state.radio_keys.clear();
            state.selected = None;
            state.control_notice = "Request pending; native acceptance and connection completion are not yet confirmed.".into();
            (provider, token, command)
        };
        self.dispatch_control(provider, token, command);
    }

    pub(super) fn submit_radio(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() || !self.control_frame_valid() {
            return;
        }
        let (provider, token, command) = {
            let mut state = self.state.borrow_mut();
            if key.is_empty()
                || state.loading()
                || state.command_flight.is_some()
                || state.settings_flight.is_some()
            {
                return;
            }
            let Some((_, target, enabled)) = state
                .radio_keys
                .iter()
                .find(|(issued, _, _)| *issued == key)
                .cloned()
            else {
                return;
            };
            if !state
                .radios
                .iter()
                .any(|radio| radio.target == Some(target))
            {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let token = state.token();
            state.command_flight = Some(token);
            state.command_radio = true;
            state.selected = None;
            state.row_keys.clear();
            state.radio_keys.clear();
            state.command_key = SharedString::default();
            state.control_notice =
                "Software-radio request pending; native initiation is not yet confirmed.".into();
            (
                provider,
                token,
                NetworkCommand::SetRadio { target, enabled },
            )
        };
        self.dispatch_control(provider, token, command);
    }

    fn dispatch_control(
        self: &Rc<Self>,
        provider: Arc<dyn NetworkHost>,
        token: Token,
        command: NetworkCommand,
    ) {
        self.mailbox.lock().expected_command = Some(token);
        // Clearing unsubmitted UI credentials must not cancel an accepted flight.
        self.component().set_command_key(SharedString::default());
        self.component().set_credentials_active(false);
        self.component().set_password(SharedString::default());
        let frame_valid = self.control_frame_valid();
        let model_valid = {
            let state = self.state.borrow();
            state.command_flight == Some(token)
                && state
                    .provider
                    .as_ref()
                    .is_some_and(|current| Arc::ptr_eq(current, &provider))
                && match &command {
                    NetworkCommand::SetRadio { target, .. } => state
                        .radios
                        .iter()
                        .any(|radio| radio.target == Some(*target)),
                    NetworkCommand::Connect { target, .. }
                    | NetworkCommand::Disconnect { target } => state
                        .controls
                        .iter()
                        .any(|control| control.target == *target),
                }
        };
        if !self.current(token.session) || !frame_valid || !model_valid {
            if self.state.borrow().command_flight == Some(token) {
                self.state.borrow_mut().command_flight = None;
            }
            if self.mailbox.lock().expected_command == Some(token) {
                self.mailbox.lock().expected_command = None;
            }
            return;
        }
        let mailbox = self.mailbox.clone();
        let root = self.surface.as_weak();
        let accepted_mailbox = self.mailbox.clone();
        let accepted_root = self.surface.as_weak();
        if let Err(error) = provider.command_with_acceptance(
            command,
            Box::new(move || mailbox::command_accepted(&accepted_mailbox, &accepted_root, token)),
            Box::new(move |result| mailbox::command_complete(&mailbox, &root, token, result)),
        ) {
            mailbox::command_complete(&self.mailbox, &self.surface.as_weak(), token, Err(error));
        }
        self.project_and_fit();
    }

    pub(super) fn project_controls(&self) {
        let open = self.is_open();
        let (session, rows, selection, command_key, credentials, supported) = {
            let mut state = self.state.borrow_mut();
            let input = open
                && !self.presenting.get()
                && !state.loading()
                && state.command_flight.is_none()
                && state.settings_flight.is_none();
            let input = input
                && state.session != u64::MAX
                && state.sequence != u64::MAX
                && state.key_sequence != u64::MAX;
            state.row_keys.clear();
            let controls = state.controls.clone();
            let mut rows = Vec::with_capacity(controls.len());
            for control in controls {
                let usable =
                    control.connected || control.connect != NetworkConnectCapability::Unavailable;
                let key = if input && usable {
                    state.key()
                } else {
                    SharedString::default()
                };
                if !key.is_empty() {
                    state.row_keys.push((key.clone(), control.target));
                }
                let method = if control.connected {
                    "Connected · Disconnect"
                } else {
                    match control.connect {
                        NetworkConnectCapability::SavedProfile => {
                            "Connect using Windows saved profile"
                        }
                        NetworkConnectCapability::Open => "Connect to open network · no password",
                        NetworkConnectCapability::Wpa2Personal => {
                            "Connect · WPA2-Personal password · not saved"
                        }
                        NetworkConnectCapability::Unavailable => {
                            "Connection security or policy unavailable / unsupported"
                        }
                    }
                };
                rows.push(NetworkControlRow {
                    label: bounded_text(&control.ssid.display_name(), 96).into(),
                    details: format!(
                        "{} · AP {} · {method}",
                        bounded_text(&control.interface_name, 80),
                        presentation::bssid(&control.bssid)
                    )
                    .into(),
                    key,
                });
            }
            let selection = state.selected.as_ref().map(|control| {
                (
                    format!(
                        "{} · {} · AP {}",
                        bounded_text(&control.ssid.display_name(), 96),
                        bounded_text(&control.interface_name, 80),
                        presentation::bssid(&control.bssid)
                    ),
                    control.connected,
                    !control.connected && control.connect == NetworkConnectCapability::Wpa2Personal,
                )
            });
            state.command_key = if input && selection.is_some() {
                state.key()
            } else {
                SharedString::default()
            };
            let credentials = selection.as_ref().is_some_and(|(_, _, password)| *password) && input;
            (
                state.session,
                rows,
                selection,
                state.command_key.clone(),
                credentials,
                state.controls_supported,
            )
        };
        let root = self.component();
        root.set_command_key(SharedString::default());
        root.set_connection_controls_supported(supported);
        root.set_control_rows(ModelRc::new(slint::VecModel::from(rows)));
        root.set_selected_network(
            selection
                .as_ref()
                .map(|(label, _, _)| SharedString::from(label.as_str()))
                .unwrap_or_default(),
        );
        root.set_command_label(
            if selection
                .as_ref()
                .is_some_and(|(_, connected, _)| *connected)
            {
                "Disconnect"
            } else {
                "Connect"
            }
            .into(),
        );
        if !credentials {
            root.set_credentials_active(false);
            root.set_password(SharedString::default());
        }
        if self.current(session) {
            root.set_credentials_active(credentials);
            root.set_command_key(command_key);
        }
        self.project_radios();
    }

    fn project_radios(&self) {
        let open = self.is_open();
        let frame_valid = self.control_frame_valid();
        let (session, supported, rows) = {
            let mut state = self.state.borrow_mut();
            state.radio_keys.clear();
            let input = open
                && frame_valid
                && !self.presenting.get()
                && !state.loading()
                && state.command_flight.is_none()
                && state.settings_flight.is_none()
                && state.session != u64::MAX
                && state.sequence != u64::MAX
                && state.key_sequence != u64::MAX;
            let radios = state.radios.clone();
            let mut rows = Vec::with_capacity(radios.len() * 2);
            for radio in radios {
                let details = radio_details(&radio);
                for enabled in [true, false] {
                    let key = if input && radio.target.is_some() {
                        state.key()
                    } else {
                        SharedString::default()
                    };
                    if let Some(target) = radio.target
                        && !key.is_empty()
                    {
                        state.radio_keys.push((key.clone(), target, enabled));
                    }
                    rows.push(NetworkControlRow {
                        label: format!(
                            "{} · Wi-Fi software {}",
                            bounded_text(&radio.interface_name, 80),
                            if enabled { "On" } else { "Off" }
                        )
                        .into(),
                        details: details.clone().into(),
                        key,
                    });
                }
            }
            (state.session, state.radios_supported, rows)
        };
        if self.current(session) {
            self.component().set_radio_controls_supported(supported);
            self.component()
                .set_radio_rows(ModelRc::new(slint::VecModel::from(rows)));
        }
    }
}

fn radio_details(radio: &NetworkRadioControl) -> String {
    let counts = |select: fn(&tessera_system::network::NetworkRadioPhy) -> RadioState| {
        let on = radio
            .phys
            .iter()
            .filter(|phy| select(phy) == RadioState::Enabled)
            .count();
        let off = radio
            .phys
            .iter()
            .filter(|phy| select(phy) == RadioState::Disabled)
            .count();
        let unknown = radio.phys.len() - on - off;
        format!("On {on} / Off {off} / ? {unknown}")
    };
    format!(
        "Software: {}\nHardware (read-only): {}\nEffective: {}",
        counts(|phy| phy.software),
        counts(|phy| phy.hardware),
        counts(|phy| phy.effective)
    )
}

pub(super) fn radio_result_notice(result: &NetworkRadioResult) -> String {
    let desired = if result.requested {
        RadioState::Enabled
    } else {
        RadioState::Disabled
    };
    let accepted = result
        .phys
        .iter()
        .filter(|phy| phy.initiation == NetworkRadioInitiation::Accepted)
        .count();
    let already = result
        .phys
        .iter()
        .filter(|phy| phy.initiation == NetworkRadioInitiation::AlreadyObserved)
        .count();
    let failed = result
        .phys
        .iter()
        .filter(|phy| matches!(&phy.initiation, NetworkRadioInitiation::Unavailable(_)))
        .count();
    let confirmed = result
        .phys
        .iter()
        .filter(|phy| matches!(&phy.readback, Observation::Ready(phy) if phy.software == desired))
        .count();
    let unconfirmed = result.phys.len() - confirmed;
    let hardware_off = result.phys.iter().filter(|phy| {
        matches!(&phy.readback, Observation::Ready(phy) if phy.hardware == RadioState::Disabled)
    }).count();
    let effective_on = result.phys.iter().filter(|phy| {
        matches!(&phy.readback, Observation::Ready(phy) if phy.effective == RadioState::Enabled)
    }).count();
    let unavailable = result
        .phys
        .iter()
        .filter(|phy| matches!(&phy.readback, Observation::Unavailable(_)))
        .count();
    let final_notice = result.readback_error.as_ref().map(|error| {
        format!(" Final full-interface readback unavailable. {} Earlier per-PHY observations may precede the last write.", failure(error.kind))
    }).unwrap_or_default();
    format!(
        "Software {} · {accepted} native writes accepted, {already} already observed, {failed} denied/stale; requested software observed on {confirmed}/{} PHYs, {unconfirmed} unconfirmed ({unavailable} readbacks unavailable). Hardware switch Off on {hardware_off}; effective On observed on {effective_on}.{final_notice} No retry, connection request or profile save was issued.",
        if result.requested { "On" } else { "Off" },
        result.phys.len()
    )
}

pub(super) fn apply_radio_result(state: &mut State, result: &NetworkRadioResult) {
    if result.readback_error.is_some() {
        return;
    }
    let phys = result
        .phys
        .iter()
        .map(|phy| match &phy.readback {
            Observation::Ready(phy) => Some(*phy),
            Observation::Unavailable(_) => None,
        })
        .collect::<Option<Vec<_>>>();
    let Some(phys) = phys.filter(|phys| !phys.is_empty()) else {
        return;
    };
    if let Some(radio) = state
        .radios
        .iter_mut()
        .find(|radio| radio.interface == result.interface && radio.target == Some(result.target))
        && radio.phys.len() == phys.len()
        && radio
            .phys
            .iter()
            .zip(&phys)
            .all(|(before, after)| before.id == after.id)
    {
        radio.phys.clone_from(&phys);
    }
    if let Some(interface) = state.snapshot.as_mut().and_then(|snapshot| {
        snapshot
            .interfaces
            .iter_mut()
            .find(|interface| interface.id == result.interface)
    }) {
        let effective = if phys.iter().any(|phy| phy.effective == RadioState::Enabled) {
            RadioState::Enabled
        } else if phys.iter().all(|phy| phy.effective == RadioState::Disabled) {
            RadioState::Disabled
        } else {
            RadioState::Unknown
        };
        interface.radio = Observation::Ready(effective);
    }
}
