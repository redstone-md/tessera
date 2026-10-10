// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Identity admission and display-only projection for the audio owner's inventory.
//! Scheduling, watch, completion and popup lifetimes remain in QuickSettings.

use super::failure;
use crate::generated::{AudioDeviceView, AudioSessionView, TileBounds};
use crate::sanitize::bounded_text;
use slint::Model;
use std::collections::{HashSet, VecDeque};
use tessera_system::audio::{
    AudioDefaultRoles, AudioDevice, AudioDeviceCommand, AudioDeviceKey, AudioDevicesResult,
    AudioDevicesSnapshot, AudioError, AudioRole, AudioSession, AudioSessionKey,
};

/// A focused native Slider supplies the exact leading key/unchanged-thumb target.
/// Geometry is used only for capture, never as endpoint or session authority.
pub(super) struct AudioFocusFrame {
    key: String,
    #[cfg(any(windows, test))]
    bounds: TileBounds,
}

/// Physical hold admission comes from the popup's existing native filter. AX
/// value actions have no physical hold and therefore remain discrete captures.
/// Cancellation keeps a tombstone through the genuine buffered native release.
struct DeviceInputFrame {
    generation: u64,
    presentation: std::rc::Rc<()>,
    handles: Vec<String>,
}

#[derive(Default)]
pub(super) struct PhysicalAudioInput {
    pointer: bool,
    keys: HashSet<&'static str>,
    captured: Option<String>,
    frame: Option<std::rc::Rc<DeviceInputFrame>>,
    #[cfg(any(windows, test))]
    pointer_release_pending: bool,
    #[cfg(any(windows, test))]
    touch_release_pending: bool,
    #[cfg(any(windows, test))]
    touches: HashSet<(slint::winit_030::winit::event::DeviceId, u64)>,
}

impl PhysicalAudioInput {
    fn active(&self) -> bool {
        let active = self.pointer || !self.keys.is_empty();
        #[cfg(any(windows, test))]
        {
            active || !self.touches.is_empty() || self.touch_release_pending
        }
        #[cfg(not(any(windows, test)))]
        {
            active
        }
    }
}

#[derive(Default)]
pub(super) struct DevicesState {
    pub supported: bool,
    pub snapshot: Option<AudioDevicesSnapshot>,
    pub selected: Option<AudioDeviceKey>,
    pub read_requested: bool,
    pub stale: bool,
    pub readback_epoch: i32,
    pub pending: VecDeque<AudioDeviceCommand>,
    pub status: String,
    device_model: std::rc::Rc<slint::VecModel<AudioDeviceView>>,
    session_model: std::rc::Rc<slint::VecModel<AudioSessionView>>,
}

#[derive(Default)]
pub(super) struct DevicesProjection {
    pub devices: Vec<AudioDeviceView>,
    pub sessions: Vec<AudioSessionView>,
    pub selected: AudioDeviceView,
    pub status: String,
    pub mixer_status: String,
    pub enabled: bool,
    pub loading: bool,
}

impl DevicesState {
    pub fn retire(&mut self) {
        self.snapshot = None;
        self.selected = None;
        self.pending.clear();
        self.read_requested = false;
        self.stale = false;
        self.status.clear();
    }

    pub fn device(&self, generation: u64, handle: &str) -> Option<&AudioDevice> {
        self.all_devices()
            .find(|device| device_handle(generation, &device.key) == handle)
    }

    pub fn session(&self, generation: u64, handle: &str) -> Option<&AudioSession> {
        let selected = self.selected_device()?;
        selected
            .sessions
            .as_ref()
            .ok()?
            .iter()
            .find(|session| session_handle(generation, &session.key) == handle)
    }

    pub fn selected_device(&self) -> Option<&AudioDevice> {
        self.all_devices()
            .find(|device| Some(&device.key) == self.selected.as_ref())
    }

    fn all_devices(&self) -> impl Iterator<Item = &AudioDevice> {
        self.snapshot.iter().flat_map(|snapshot| {
            [&snapshot.output.devices, &snapshot.input.devices]
                .into_iter()
                .filter_map(|devices| devices.as_ref().ok())
                .flat_map(|devices| devices.iter())
        })
    }

    pub fn accept(&mut self, result: AudioDevicesResult, command: Option<&AudioDeviceCommand>) {
        self.readback_epoch = self.readback_epoch.wrapping_add(1);
        let had_selection = self.selected.is_some();
        self.snapshot = Some(result.snapshot);
        self.stale = false;
        if self.selected_device().is_none() {
            let selected = if had_selection {
                None
            } else {
                self.all_devices().next().map(|device| device.key.clone())
            };
            self.selected = selected;
        }
        let snapshot = self.snapshot.as_ref().expect("accepted device snapshot");
        let keys: Vec<_> = [&snapshot.output.devices, &snapshot.input.devices]
            .into_iter()
            .filter_map(|devices| devices.as_ref().ok())
            .flat_map(|devices| devices.iter())
            .map(|device| device.key.clone())
            .collect();
        self.pending
            .retain(|command| keys.contains(command.endpoint()));
        self.status = if !result.roles.is_empty() {
            result
                .roles
                .iter()
                .map(|role| {
                    let name = match role.role {
                        AudioRole::Console => "Console",
                        AudioRole::Multimedia => "Multimedia",
                        AudioRole::Communications => "Communications",
                    };
                    match &role.result {
                        Ok(()) => format!("{name}: confirmed"),
                        Err(error) => failure(name, error),
                    }
                })
                .collect::<Vec<_>>()
                .join("; ")
        } else if matches!(command, Some(AudioDeviceCommand::OpenDeviceSettings { .. })) {
            "Sound Settings launch requested; Windows controls opening and focus.".into()
        } else if had_selection && self.selected.is_none() {
            "Selected device disconnected; choose a current device.".into()
        } else if command.is_some() {
            String::new()
        } else {
            // A watch-triggered read must not erase a partial/default-role result.
            self.status.clone()
        };
    }

    pub fn failed(&mut self, error: &AudioError) {
        self.pending.clear();
        self.stale = true;
        self.status = failure("Devices or mixer unavailable; refresh", error);
    }

    pub fn queue(&mut self, command: AudioDeviceCommand) -> bool {
        // One pending value per exact native target and kind; all remaining
        // commands are bounded rather than an unbounded drag/action backlog.
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|existing| same_target(existing, &command))
        {
            *existing = command;
        } else if self.pending.len() < 32 {
            self.pending.push_back(command);
        } else {
            self.status = "Audio command queue is full; wait for readback.".into();
            return false;
        }
        true
    }

    pub fn next_command(&mut self) -> Option<AudioDeviceCommand> {
        if self.stale {
            return None;
        }
        while let Some(command) = self.pending.pop_front() {
            let Some(device) = self
                .all_devices()
                .find(|device| &device.key == command.endpoint())
            else {
                continue;
            };
            let needed = match &command {
                AudioDeviceCommand::SetVolume { volume, .. } => device
                    .level
                    .as_ref()
                    .is_ok_and(|level| &level.volume != volume),
                AudioDeviceCommand::SetMuted { muted, .. } => device
                    .level
                    .as_ref()
                    .is_ok_and(|level| &level.muted != muted),
                AudioDeviceCommand::SetSessionVolume { session, volume } => device
                    .sessions
                    .as_ref()
                    .ok()
                    .and_then(|sessions| sessions.iter().find(|current| &current.key == session))
                    .is_some_and(|current| {
                        current
                            .level
                            .as_ref()
                            .is_ok_and(|level| &level.volume != volume)
                    }),
                AudioDeviceCommand::SetSessionMuted { session, muted } => device
                    .sessions
                    .as_ref()
                    .ok()
                    .and_then(|sessions| sessions.iter().find(|current| &current.key == session))
                    .is_some_and(|current| {
                        current
                            .level
                            .as_ref()
                            .is_ok_and(|level| &level.muted != muted)
                    }),
                _ => true,
            };
            if needed {
                return Some(command);
            }
        }
        None
    }

    pub fn projection(&self, generation: u64, busy: bool) -> DevicesProjection {
        let mut projection = DevicesProjection {
            enabled: self.snapshot.is_some() && !self.stale,
            loading: self.snapshot.is_none() && self.read_requested,
            status: self.status.clone(),
            ..DevicesProjection::default()
        };
        if busy {
            projection.status = if projection.status.is_empty() {
                "Applying audio change; waiting for native readback…".into()
            } else {
                format!(
                    "{} Applying audio change; waiting for native readback…",
                    projection.status
                )
            };
        }
        let Some(snapshot) = &self.snapshot else {
            return projection;
        };
        for (flow, input) in [(&snapshot.output, false), (&snapshot.input, true)] {
            let devices = match &flow.devices {
                Ok(devices) => devices,
                Err(error) => {
                    append(
                        &mut projection.status,
                        failure(
                            if input {
                                "Input inventory"
                            } else {
                                "Output inventory"
                            },
                            error,
                        ),
                    );
                    continue;
                }
            };
            for device in devices {
                let selected = Some(&device.key) == self.selected.as_ref();
                let level = device.level.as_ref().ok();
                let mut view = AudioDeviceView {
                    key: device_handle(generation, &device.key).into(),
                    name: bounded_text(&device.name, 160).into(),
                    roles: role_labels(&flow.roles, &device.key.id).into(),
                    input,
                    selected,
                    ready: level.is_some() && !self.stale,
                    muted: level.is_some_and(|level| level.muted),
                    percent: level.map_or(0.0, |level| level.volume.scalar() * 100.0),
                    status: device
                        .level
                        .as_ref()
                        .err()
                        .map_or_else(String::new, |error| {
                            failure("Master volume unavailable", error)
                        })
                        .into(),
                    default_enabled: snapshot.default_role_control.is_ok() && !busy && !self.stale,
                    communications_enabled: snapshot.default_role_control.is_ok()
                        && !busy
                        && !self.stale,
                    settings_enabled: snapshot.device_settings.is_ok() && !busy && !self.stale,
                };
                if selected {
                    let mut notices = view.status.to_string();
                    if let Err(error) = &device.watch {
                        append(
                            &mut notices,
                            failure("Device live updates unavailable; use Refresh", error),
                        );
                    }
                    if let Err(error) = &snapshot.default_role_control {
                        append(&mut notices, failure("Default role controls", error));
                    }
                    if let Err(error) = &snapshot.device_settings {
                        append(&mut notices, failure("Sound Settings", error));
                    }
                    view.status = bounded_text(&notices, 480).into();
                    projection.selected = view.clone();
                    projection.sessions = match &device.sessions {
                        Ok(sessions) => {
                            if sessions.is_empty() && !input {
                                projection.mixer_status = "No current output sessions.".into();
                            }
                            sessions
                                .iter()
                                .map(|session| {
                                    let level = session.level.as_ref().ok();
                                    AudioSessionView {
                                        key: session_handle(generation, &session.key).into(),
                                        name: bounded_text(&session.name, 160).into(),
                                        ready: level.is_some() && !self.stale,
                                        muted: level.is_some_and(|level| level.muted),
                                        percent: level
                                            .map_or(0.0, |level| level.volume.scalar() * 100.0),
                                        status: session
                                            .level
                                            .as_ref()
                                            .err()
                                            .map_or_else(
                                                || {
                                                    if session.active {
                                                        "Playing".into()
                                                    } else {
                                                        "Inactive".into()
                                                    }
                                                },
                                                |error| {
                                                    failure("Session volume unavailable", error)
                                                },
                                            )
                                            .into(),
                                    }
                                })
                                .collect()
                        }
                        Err(error) => {
                            if !input {
                                projection.mixer_status = failure("Output mixer", error);
                            }
                            Vec::new()
                        }
                    };
                }
                projection.devices.push(view);
            }
        }
        if projection.devices.is_empty() && projection.status.is_empty() {
            projection.status = "No active audio devices.".into();
        }
        projection.status = bounded_text(&projection.status, 640);
        projection
    }
}

fn append(target: &mut String, notice: String) {
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(&notice);
}

// Length-prefixed UTF-8 covers every identity field, even when providers scope
// incarnation counters per endpoint/session. Handles are compared to regenerated
// keys from the retained snapshot; they are never parsed into effect authority.
fn device_handle(generation: u64, key: &AudioDeviceKey) -> String {
    let id = key.id.as_str();
    format!("d:{generation}:{}:{id}:{}", id.len(), key.incarnation)
}

fn session_handle(generation: u64, key: &AudioSessionKey) -> String {
    let id = key.endpoint.id.as_str();
    let instance = &key.instance_id;
    format!(
        "s:{generation}:{}:{id}:{}:{}:{instance}:{}",
        id.len(),
        key.endpoint.incarnation,
        instance.len(),
        key.incarnation
    )
}

fn role_labels(roles: &AudioDefaultRoles, id: &tessera_system::audio::EndpointId) -> String {
    [
        ("Console", &roles.console),
        ("Multimedia", &roles.multimedia),
        ("Communications", &roles.communications),
    ]
    .into_iter()
    .filter_map(|(name, value)| match value {
        Ok(Some(default)) if default == id => Some(name.to_string()),
        Err(_) => Some(format!("{name}: unknown")),
        _ => None,
    })
    .collect::<Vec<_>>()
    .join(" · ")
}

fn same_target(left: &AudioDeviceCommand, right: &AudioDeviceCommand) -> bool {
    match (left, right) {
        (
            AudioDeviceCommand::SetVolume { endpoint: a, .. },
            AudioDeviceCommand::SetVolume { endpoint: b, .. },
        )
        | (
            AudioDeviceCommand::SetMuted { endpoint: a, .. },
            AudioDeviceCommand::SetMuted { endpoint: b, .. },
        )
        | (
            AudioDeviceCommand::OpenDeviceSettings { endpoint: a },
            AudioDeviceCommand::OpenDeviceSettings { endpoint: b },
        ) => a == b,
        (
            AudioDeviceCommand::SetSessionVolume { session: a, .. },
            AudioDeviceCommand::SetSessionVolume { session: b, .. },
        )
        | (
            AudioDeviceCommand::SetSessionMuted { session: a, .. },
            AudioDeviceCommand::SetSessionMuted { session: b, .. },
        ) => a == b,
        (
            AudioDeviceCommand::SetDefault {
                endpoint: a,
                target: x,
            },
            AudioDeviceCommand::SetDefault {
                endpoint: b,
                target: y,
            },
        ) => a == b && x == y,
        _ => false,
    }
}

impl super::QuickSettingsController {
    pub(super) fn install_devices_callbacks(self: &std::rc::Rc<Self>) {
        let weak = std::rc::Rc::downgrade(self);
        self.surface
            .on_device_focus_frame(move |key, focused, bounds| {
                if let Some(controller) = weak.upgrade() {
                    controller.device_focus_frame(key.as_str(), focused, bounds);
                }
            });
        let weak = std::rc::Rc::downgrade(self);
        self.surface
            .on_device_value_accessible_requested(move |key, value| {
                weak.upgrade().is_some_and(|controller| {
                    let session = {
                        let state = controller.state.borrow();
                        state
                            .devices
                            .session(state.generation, key.as_str())
                            .is_some()
                    };
                    controller.device_volume_input(key.as_str(), value, session, false)
                })
            });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_device_selected(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.select_device(key.as_str());
            }
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_device_volume_requested(move |key, value| {
            weak.upgrade()
                .is_some_and(|controller| controller.device_volume(key.as_str(), value, false))
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_device_volume_released(move |key, value| {
            weak.upgrade()
                .is_some_and(|controller| controller.device_volume(key.as_str(), value, false))
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_session_volume_requested(move |key, value| {
            weak.upgrade()
                .is_some_and(|controller| controller.device_volume(key.as_str(), value, true))
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_session_volume_released(move |key, value| {
            weak.upgrade()
                .is_some_and(|controller| controller.device_volume(key.as_str(), value, true))
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_device_mute_requested(move |key, value| {
            if let Some(controller) = weak.upgrade() {
                controller.device_mute(key.as_str(), value, false);
            }
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_session_mute_requested(move |key, value| {
            if let Some(controller) = weak.upgrade() {
                controller.device_mute(key.as_str(), value, true);
            }
        });
        let weak = std::rc::Rc::downgrade(self);
        self.surface
            .on_device_default_requested(move |key, communications| {
                if let Some(controller) = weak.upgrade() {
                    controller.device_default(key.as_str(), communications);
                }
            });
        let weak = std::rc::Rc::downgrade(self);
        self.surface.on_device_settings_requested(move |key| {
            if let Some(controller) = weak.upgrade() {
                controller.device_settings(key.as_str());
            }
        });
    }

    fn device_focus_frame(&self, handle: &str, focused: bool, bounds: TileBounds) {
        if !focused {
            let mut current = self.audio_focus.borrow_mut();
            if current.as_ref().is_some_and(|frame| frame.key == handle) {
                *current = None;
            }
            return;
        }
        if !self.source_input_ready() {
            return;
        }
        let valid = {
            let state = self.state.borrow();
            !state.devices.stale
                && (state.devices.selected_device().is_some_and(|device| {
                    device.level.is_ok() && device_handle(state.generation, &device.key) == handle
                }) || state
                    .devices
                    .session(state.generation, handle)
                    .is_some_and(|session| session.level.is_ok()))
        };
        if !valid
            || !bounds.origin.x.is_finite()
            || !bounds.origin.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return;
        }
        *self.audio_focus.borrow_mut() = Some(AudioFocusFrame {
            key: handle.to_owned(),
            #[cfg(any(windows, test))]
            bounds,
        });
        let mut input = self.device_input.borrow_mut();
        if input.active()
            && input.captured.is_none()
            && input
                .frame
                .as_ref()
                .is_some_and(|frame| frame.handles.iter().any(|key| key == handle))
        {
            input.captured = Some(handle.to_owned());
        }
    }

    #[cfg(any(windows, test))]
    fn focused_device_key(
        &self,
        frame: &DeviceInputFrame,
        position: Option<slint::LogicalPosition>,
    ) -> Option<String> {
        let focused = self.audio_focus.borrow();
        let focused = focused.as_ref()?;
        if !frame.handles.contains(&focused.key) {
            return None;
        }
        if let Some(position) = position {
            let bounds = &focused.bounds;
            if !position.x.is_finite()
                || !position.y.is_finite()
                || position.x < bounds.origin.x
                || position.y < bounds.origin.y
                || position.x >= bounds.origin.x + bounds.width
                || position.y >= bounds.origin.y + bounds.height
            {
                return None;
            }
        }
        Some(focused.key.clone())
    }

    fn select_device(&self, handle: &str) {
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.devices.stale {
                return;
            }
            let Some(key) = state
                .devices
                .device(state.generation, handle)
                .map(|device| device.key.clone())
            else {
                return;
            };
            if state.devices.selected.as_ref() != Some(&key) {
                // Cancel only unsubmitted device/session intentions. An already
                // accepted flight remains owned until its terminal completion.
                state.devices.pending.clear();
            }
            state.devices.selected = Some(key);
        }
        // Do not release a held native gesture on selection: its original
        // handle must stay suppressed until the actual native release.
        self.project_and_fit();
    }

    fn device_volume(self: &std::rc::Rc<Self>, handle: &str, percent: f32, session: bool) -> bool {
        self.device_volume_input(handle, percent, session, true)
    }

    fn device_volume_input(
        self: &std::rc::Rc<Self>,
        handle: &str,
        percent: f32,
        session: bool,
        physical: bool,
    ) -> bool {
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return false;
        }
        let Ok(volume) = super::Volume::from_scalar(percent.round() / 100.0) else {
            return false;
        };
        let command = {
            let state = self.state.borrow();
            if state.devices.stale {
                return false;
            }
            if session {
                let Some(current) = state.devices.session(state.generation, handle) else {
                    return false;
                };
                if current.level.is_err() {
                    return false;
                }
                AudioDeviceCommand::SetSessionVolume {
                    session: current.key.clone(),
                    volume,
                }
            } else {
                let Some(current) = state.devices.device(state.generation, handle) else {
                    return false;
                };
                if state.devices.selected.as_ref() != Some(&current.key) || current.level.is_err() {
                    return false;
                }
                AudioDeviceCommand::SetVolume {
                    endpoint: current.key.clone(),
                    volume,
                }
            }
        };
        let admitted = if physical {
            self.admit_device_value(handle)
        } else {
            self.source_input_ready()
        };
        if !admitted {
            return false;
        }
        self.submit_device(command)
    }

    fn device_mute(self: &std::rc::Rc<Self>, handle: &str, muted: bool, session: bool) {
        let command = {
            let state = self.state.borrow();
            if state.devices.stale {
                return;
            }
            if session {
                let Some(current) = state.devices.session(state.generation, handle) else {
                    return;
                };
                if current.level.is_err() {
                    return;
                }
                AudioDeviceCommand::SetSessionMuted {
                    session: current.key.clone(),
                    muted,
                }
            } else {
                let Some(current) = state.devices.device(state.generation, handle) else {
                    return;
                };
                if state.devices.selected.as_ref() != Some(&current.key) || current.level.is_err() {
                    return;
                }
                AudioDeviceCommand::SetMuted {
                    endpoint: current.key.clone(),
                    muted,
                }
            }
        };
        self.submit_device(command);
    }

    fn device_default(self: &std::rc::Rc<Self>, handle: &str, communications: bool) {
        let key = {
            let state = self.state.borrow();
            if state.devices.stale
                || state
                    .devices
                    .snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.default_role_control.is_err())
            {
                return;
            }
            let Some(current) = state.devices.device(state.generation, handle) else {
                return;
            };
            if state.devices.selected.as_ref() != Some(&current.key) {
                return;
            }
            current.key.clone()
        };
        self.submit_device(AudioDeviceCommand::SetDefault {
            endpoint: key,
            target: if communications {
                super::AudioDefaultTarget::Communications
            } else {
                super::AudioDefaultTarget::MultimediaAndConsole
            },
        });
    }

    fn device_settings(self: &std::rc::Rc<Self>, handle: &str) {
        let key = {
            let state = self.state.borrow();
            if state.devices.stale
                || state
                    .devices
                    .snapshot
                    .as_ref()
                    .is_none_or(|snapshot| snapshot.device_settings.is_err())
            {
                return;
            }
            let Some(current) = state.devices.device(state.generation, handle) else {
                return;
            };
            if state.devices.selected.as_ref() != Some(&current.key) {
                return;
            }
            current.key.clone()
        };
        self.submit_device(AudioDeviceCommand::OpenDeviceSettings { endpoint: key });
    }

    fn submit_device(self: &std::rc::Rc<Self>, command: AudioDeviceCommand) -> bool {
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
            return false;
        }
        let admitted = {
            let mut state = self.state.borrow_mut();
            if state.devices.stale || state.devices.selected.as_ref() != Some(command.endpoint()) {
                return false;
            }
            let valid = match &command {
                AudioDeviceCommand::SetSessionVolume { session, .. }
                | AudioDeviceCommand::SetSessionMuted { session, .. } => state
                    .devices
                    .session(state.generation, &session_handle(state.generation, session))
                    .is_some_and(|current| &current.key == session),
                _ => state
                    .devices
                    .all_devices()
                    .any(|current| &current.key == command.endpoint()),
            };
            if !valid {
                return false;
            }
            state.devices.queue(command)
        };
        self.project_and_fit();
        if self.source_is_current(&epoch) {
            self.pump();
        }
        admitted && self.presentation_is_current(&epoch)
    }

    fn admit_device_value(&self, handle: &str) -> bool {
        if !self.source_input_ready() {
            return false;
        }
        let mut input = self.device_input.borrow_mut();
        if input.active() {
            let Some(frame) = &input.frame else {
                return false;
            };
            if !self.presentation_is_current(&frame.presentation)
                || self.state.borrow().generation != frame.generation
                || !frame.handles.iter().any(|allowed| allowed == handle)
            {
                return false;
            }
            // Survives conditional widget replacement, inventory reorder and
            // selection changes until every physical hold actually releases.
            input
                .captured
                .get_or_insert_with(|| handle.to_owned())
                .as_str()
                == handle
        } else {
            // AX is a discrete action against its currently presented handle.
            true
        }
    }

    #[cfg(any(windows, test))]
    fn capture_device_frame(&self) -> Option<std::rc::Rc<DeviceInputFrame>> {
        if !self.state.borrow().devices.supported {
            return None;
        }
        let epoch = self.presentation_epoch.borrow().clone();
        if !self.source_is_current(&epoch) {
            return None;
        }
        let state = self.state.borrow();
        if state.devices.stale {
            return None;
        }
        let device = state.devices.selected_device()?;
        let mut handles = Vec::new();
        if device.level.is_ok() {
            handles.push(device_handle(state.generation, &device.key));
        }
        if let Ok(sessions) = &device.sessions {
            handles.extend(
                sessions
                    .iter()
                    .filter(|session| session.level.is_ok())
                    .map(|session| session_handle(state.generation, &session.key)),
            );
        }
        Some(std::rc::Rc::new(DeviceInputFrame {
            generation: state.generation,
            presentation: epoch,
            handles,
        }))
    }

    #[cfg(any(windows, test))]
    fn update_device_input(&self, change: impl FnOnce(&mut PhysicalAudioInput)) {
        let (was_active, active) = {
            let mut input = self.device_input.borrow_mut();
            let was_active = input.active();
            change(&mut input);
            if !input.active() {
                input.captured = None;
                input.frame = None;
            }
            (was_active, input.active())
        };
        self.surface.set_device_physical_input(active);
        if was_active && !active {
            self.surface
                .set_device_input_epoch(self.surface.get_device_input_epoch().wrapping_add(1));
        }
    }

    #[cfg(any(windows, test))]
    pub(super) fn device_pointer(&self, position: Option<slint::LogicalPosition>, pressed: bool) {
        let frame = pressed.then(|| self.capture_device_frame()).flatten();
        let captured = position.and_then(|position| {
            frame
                .as_ref()
                .and_then(|frame| self.focused_device_key(frame, Some(position)))
        });
        self.update_device_input(|input| {
            if pressed {
                if !input.active()
                    || (input.pointer_release_pending
                        && input.keys.is_empty()
                        && input.touches.is_empty()
                        && !input.touch_release_pending)
                {
                    input.frame = frame;
                    input.captured = captured;
                }
                input.pointer = true;
                input.pointer_release_pending = false;
            } else {
                // Keep exact authority through buffered moves and Slider release.
                input.pointer = true;
                input.pointer_release_pending = true;
            }
        });
    }

    #[cfg(any(windows, test))]
    pub(super) fn device_key(&self, key: &'static str, pressed: bool, repeat: bool) {
        let frame = (pressed && !repeat)
            .then(|| self.capture_device_frame())
            .flatten();
        let captured = frame
            .as_ref()
            .and_then(|frame| self.focused_device_key(frame, None));
        let frame = captured.as_ref().and(frame);
        self.update_device_input(|input| {
            if pressed {
                if !input.active() {
                    // A repeat without an admitted leading press is a tombstone.
                    input.frame = frame;
                    input.captured = captured;
                }
                input.keys.insert(key);
            } else {
                input.keys.remove(key);
            }
        });
    }

    #[cfg(any(windows, test))]
    pub(super) fn device_touch(
        &self,
        device: slint::winit_030::winit::event::DeviceId,
        id: u64,
        phase: slint::winit_030::winit::event::TouchPhase,
        position: Option<slint::LogicalPosition>,
    ) -> bool {
        use slint::winit_030::winit::event::TouchPhase;
        let frame = (phase == TouchPhase::Started)
            .then(|| self.capture_device_frame())
            .flatten();
        let captured = position.and_then(|position| {
            frame
                .as_ref()
                .and_then(|frame| self.focused_device_key(frame, Some(position)))
        });
        self.update_device_input(|input| match phase {
            TouchPhase::Started => {
                if !input.active() {
                    input.frame = frame;
                    input.captured = captured;
                }
                input.touch_release_pending = false;
                input.touches.insert((device, id));
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                input.touches.remove(&(device, id));
                input.touch_release_pending = input.touches.is_empty();
            }
            TouchPhase::Moved => {
                if !input.touches.contains(&(device, id)) {
                    input.frame = None;
                    input.touches.insert((device, id));
                }
            }
        });
        self.device_input.borrow().touch_release_pending
    }

    #[cfg(any(windows, test))]
    pub(super) fn finish_device_releases(&self) {
        self.update_device_input(|input| {
            if input.pointer_release_pending {
                input.pointer = false;
                input.pointer_release_pending = false;
            }
            input.touch_release_pending = false;
        });
    }

    pub(super) fn cancel_device_input(&self) {
        self.audio_focus.borrow_mut().take();
        let active = {
            let mut input = self.device_input.borrow_mut();
            input.frame = None;
            input.captured = None;
            input.active()
        };
        self.surface.set_device_physical_input(active);
        self.surface
            .set_device_input_epoch(self.surface.get_device_input_epoch().wrapping_add(1));
    }

    pub(super) fn project_devices(&self) {
        let (supported, projection, device_model, session_model, readback_epoch) = {
            let state = self.state.borrow();
            let busy = state.flight.as_ref().is_some_and(|flight| {
                flight.token.generation == state.generation
                    && matches!(flight.request, super::Request::DeviceCommand(_))
            });
            let reading = state.flight.as_ref().is_some_and(|flight| {
                flight.token.generation == state.generation
                    && matches!(flight.request, super::Request::ReadDevices)
            });
            let mut projection = state.devices.projection(state.generation, busy);
            projection.loading |= reading && state.devices.snapshot.is_none();
            (
                state.devices.supported,
                projection,
                state.devices.device_model.clone(),
                state.devices.session_model.clone(),
                state.devices.readback_epoch,
            )
        };
        self.surface.set_devices_supported(supported);
        self.surface.set_devices_enabled(projection.enabled);
        self.surface.set_devices_loading(projection.loading);
        self.surface.set_devices_status(projection.status.into());
        self.surface
            .set_mixer_status(projection.mixer_status.into());
        update_model(&device_model, projection.devices);
        update_model(&session_model, projection.sessions);
        self.surface.set_devices(device_model.into());
        self.surface.set_audio_sessions(session_model.into());
        self.surface.set_selected_device(projection.selected);
        self.surface.set_device_readback_epoch(readback_epoch);
    }
}

fn update_model<T: Clone + PartialEq + 'static>(model: &slint::VecModel<T>, rows: Vec<T>) {
    // Updating existing rows retains the genuine Slider's native input capture
    // and its first-handle latch instead of recreating the whole repeater.
    for (index, row) in rows.iter().enumerate() {
        if index < model.row_count() {
            if model.row_data(index).as_ref() != Some(row) {
                model.set_row_data(index, row.clone());
            }
        } else {
            model.push(row.clone());
        }
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
}
