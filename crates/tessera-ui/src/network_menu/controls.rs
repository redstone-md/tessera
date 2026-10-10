// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::NetworkControlRow;

impl NetworkController {
    fn control_frame_valid(&self) -> bool {
        let Some(placement) = self.placement.get() else {
            return false;
        };
        let frame = self.rect.borrow();
        let Some(frame) = frame.as_ref() else {
            return false;
        };
        self.component().window().scale_factor() == placement.scale
            && self.component().window().position() == frame.position
            && self.component().window().size() == frame.size
    }

    pub(super) fn control_frame_changed(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.row_keys.clear();
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
            state.command_key = SharedString::default();
            state.row_keys.clear();
            state.selected = None;
            state.control_notice = "Request pending; native acceptance and connection completion are not yet confirmed.".into();
            (provider, token, command)
        };
        self.mailbox.lock().expected_command = Some(token);
        // Clearing unsubmitted UI credentials must not cancel an accepted flight.
        self.component().set_command_key(SharedString::default());
        self.component().set_credentials_active(false);
        self.component().set_password(SharedString::default());
        if !self.current(token.session) || !self.control_frame_valid() {
            self.state.borrow_mut().command_flight = None;
            self.mailbox.lock().expected_command = None;
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
        let (session, rows, selection, command_key, credentials, supported) = {
            let mut state = self.state.borrow_mut();
            let input = self.is_open()
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
    }
}
