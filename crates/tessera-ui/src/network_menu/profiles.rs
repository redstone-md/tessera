// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;
use crate::generated::NetworkControlRow;

pub(super) struct Confirmation {
    profile: NetworkProfileControl,
    session: u64,
    model: Token,
    position: PhysicalPosition,
    size: PhysicalSize,
    scale: f32,
}

impl NetworkController {
    fn profile_source_valid(&self) -> bool {
        if !self.is_open() || !self.control_frame_valid() {
            return false;
        }
        let session = self.state.borrow().session;
        let mailbox = self.mailbox.lock();
        // Posted native/model invalidation already retires confirmation, even
        // before the coalesced event-loop delivery is projected.
        mailbox.watch_session == Some(session)
            && !mailbox.changed
            && mailbox.read.is_none()
            && !matches!(mailbox.watch, Some(NetworkEvent::WatchUnavailable(_)))
    }

    pub(super) fn select_profile(self: &Rc<Self>, key: SharedString) {
        if !self.accepts_input() || !self.profile_source_valid() {
            return;
        }
        let position = self.component().window().position();
        let size = self.component().window().size();
        let scale = self.component().window().scale_factor();
        {
            let mut state = self.state.borrow_mut();
            if key.is_empty()
                || state.loading()
                || state.command_flight.is_some()
                || state.settings_flight.is_some()
            {
                return;
            }
            let Some((_, target)) = state.profile_keys.iter().find(|(issued, _)| *issued == key)
            else {
                return;
            };
            let Some(profile) = state
                .profiles
                .iter()
                .find(|profile| {
                    profile.target == Some(*target) && profile.forget == Observation::Ready(())
                })
                .cloned()
            else {
                return;
            };
            let Some(model) = state.profile_model else {
                return;
            };
            state.profile_confirmation = Some(Confirmation {
                profile,
                session: state.session,
                model,
                position,
                size,
                scale,
            });
            state.profile_confirm_key = SharedString::default();
            state.selected = None;
            state.command_key = SharedString::default();
            state.control_notice.clear();
        }
        self.component().set_credentials_active(false);
        self.component().set_password(SharedString::default());
        self.project_and_fit();
    }

    pub(super) fn confirm_profile(self: &Rc<Self>, key: SharedString, confirm: bool) {
        if !self.accepts_input() || !self.profile_source_valid() {
            return;
        }
        let position = self.component().window().position();
        let size = self.component().window().size();
        let scale = self.component().window().scale_factor();
        let dispatch = {
            let mut state = self.state.borrow_mut();
            if key.is_empty()
                || key != state.profile_confirm_key
                || state.loading()
                || state.command_flight.is_some()
                || state.settings_flight.is_some()
            {
                return;
            }
            let Some(confirmation) = state.profile_confirmation.as_ref() else {
                return;
            };
            if confirmation.session != state.session
                || Some(confirmation.model) != state.profile_model
                || confirmation.position != position
                || confirmation.size != size
                || confirmation.scale != scale
                || !state
                    .profiles
                    .iter()
                    .any(|profile| profile == &confirmation.profile)
                || !confirm
            {
                state.retire_profile_confirmation();
                None
            } else {
                let Some(target) = confirmation.profile.target else {
                    return;
                };
                let Some(provider) = state.provider.clone() else {
                    return;
                };
                let token = state.token();
                state.command_flight = Some(token);
                state.command_profile = true;
                state.command_radio = false;
                state.retire_profile_confirmation();
                state.selected = None;
                state.command_key = SharedString::default();
                state.row_keys.clear();
                state.radio_keys.clear();
                state.control_notice =
                    "Forget requested; native initiation and removal are not yet confirmed.".into();
                Some((provider, token, NetworkCommand::ForgetProfile { target }))
            }
        };
        self.component()
            .set_profile_confirm_key(SharedString::default());
        if let Some((provider, token, command)) = dispatch {
            self.dispatch_control(provider, token, command);
        } else {
            self.project_and_fit();
        }
    }

    pub(super) fn project_profiles(&self) {
        let valid = self.profile_source_valid();
        let position = self.component().window().position();
        let size = self.component().window().size();
        let scale = self.component().window().scale_factor();
        let (session, supported, rows, confirmation, key) = {
            let mut state = self.state.borrow_mut();
            let input = valid
                && !self.presenting.get()
                && !state.loading()
                && state.command_flight.is_none()
                && state.settings_flight.is_none()
                && state.session != u64::MAX
                && state.sequence != u64::MAX
                && state.key_sequence != u64::MAX;
            let confirmation_valid =
                state
                    .profile_confirmation
                    .as_ref()
                    .is_some_and(|confirmation| {
                        input
                            && confirmation.session == state.session
                            && Some(confirmation.model) == state.profile_model
                            && confirmation.position == position
                            && confirmation.size == size
                            && confirmation.scale == scale
                            && state
                                .profiles
                                .iter()
                                .any(|profile| profile == &confirmation.profile)
                    });
            if !confirmation_valid {
                state.retire_profile_confirmation();
            }
            state.profile_keys.clear();
            let mut rows = Vec::new();
            for profile in state.profiles.clone() {
                let key = if input
                    && profile.target.is_some()
                    && profile.forget == Observation::Ready(())
                {
                    state.key()
                } else {
                    SharedString::default()
                };
                if let Some(target) = profile.target
                    && !key.is_empty()
                {
                    state.profile_keys.push((key.clone(), target));
                }
                let scope = match profile.scope {
                    NetworkProfileScope::AllUsers => "All users",
                    NetworkProfileScope::CurrentUser => "Current user",
                    NetworkProfileScope::GroupPolicy => "Group policy · read-only",
                    NetworkProfileScope::Unsupported => "Unsupported policy · read-only",
                };
                let capability = match &profile.forget {
                    Observation::Ready(()) => "Forget saved credentials/autoconnect".to_owned(),
                    Observation::Unavailable(error) => {
                        format!("Read-only · {}", failure(error.kind))
                    }
                };
                rows.push(NetworkControlRow {
                    label: bounded_text(&profile.name, 96).into(),
                    details: format!(
                        "{} · {scope} · {capability}",
                        bounded_text(&profile.interface_name, 80)
                    )
                    .into(),
                    key,
                });
            }
            let confirmation = state
                .profile_confirmation
                .as_ref()
                .map(|confirmation| {
                    format!(
                        "Forget {} · {}?",
                        bounded_text(&confirmation.profile.name, 96),
                        bounded_text(&confirmation.profile.interface_name, 80)
                    )
                })
                .unwrap_or_default();
            state.profile_confirm_key = if input && !confirmation.is_empty() {
                state.key()
            } else {
                SharedString::default()
            };
            (
                state.session,
                state.profiles_supported,
                rows,
                confirmation,
                state.profile_confirm_key.clone(),
            )
        };
        let root = self.component();
        root.set_profile_confirm_key(SharedString::default());
        root.set_profile_controls_supported(supported);
        root.set_profile_rows(ModelRc::new(slint::VecModel::from(rows)));
        root.set_profile_confirmation(confirmation.into());
        if self.current(session) {
            root.set_profile_confirm_key(key);
        }
    }
}

pub(super) fn result_notice(result: &NetworkProfileResult) -> String {
    let initiation = match &result.initiation {
        NetworkProfileInitiation::NotSubmitted(error) => format!(
            "Forget was not submitted to Windows. {}",
            failure(error.kind)
        ),
        NetworkProfileInitiation::Accepted => {
            "Windows accepted the exact saved-profile deletion.".into()
        }
        NetworkProfileInitiation::NativeError { code, error } => format!(
            "Windows rejected saved-profile deletion (code {code}). {}",
            failure(error.kind)
        ),
    };
    let readback = match &result.readback {
        Observation::Ready(NetworkProfilePresence::Absent) => {
            "Actual exact-adapter readback confirms the saved profile is absent."
        }
        Observation::Ready(NetworkProfilePresence::Present) => {
            "Actual readback finds a saved profile under that exact native name; removal is not confirmed."
        }
        Observation::Ready(NetworkProfilePresence::Replaced) => {
            "Actual readback finds a changed profile under that native name; it was not retried."
        }
        Observation::Unavailable(_) => {
            "Exact-source readback is unavailable; profile removal is unknown."
        }
    };
    let error = match &result.readback {
        Observation::Unavailable(error) => format!(" {}", failure(error.kind)),
        _ => String::new(),
    };
    let descriptor = result
        .descriptor_error
        .as_ref()
        .map(|error| {
            format!(
                " Current profile descriptor verification is unavailable. {}",
                failure(error.kind)
            )
        })
        .unwrap_or_default();
    format!("{initiation} {readback}{error}{descriptor} No retry or rollback was issued.")
}

pub(super) fn apply_result(state: &mut State, result: &NetworkProfileResult) {
    if result.readback == Observation::Ready(NetworkProfilePresence::Absent) {
        state
            .profiles
            .retain(|profile| profile.target != Some(result.target));
    }
    state.profile_model = None;
    state.retire_profile_confirmation();
    for profile in &mut state.profiles {
        profile.target = None;
        profile.forget = Observation::Unavailable(NetworkError::new(
            NetworkErrorKind::DeviceChanged,
            "Refresh saved-profile facts before another Forget.",
        ));
    }
    // No automatic read/retry after denied initiation or inconclusive readback.
    state.automatic_blocked |= result.initiation != NetworkProfileInitiation::Accepted
        || result.readback != Observation::Ready(NetworkProfilePresence::Absent);
    state.controls.clear();
    state.row_keys.clear();
    state.radio_keys.clear();
    for radio in &mut state.radios {
        radio.target = None;
    }
}
