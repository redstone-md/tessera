// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Snapshot presentation only; no RefCell or mailbox guard crosses a Slint setter.

use std::sync::Arc;

use tessera_system::wallpaper::WallpaperHost;

use super::{
    Command, Flight, PositionController, ResetFlag, Session, State, caption, position_index,
};

struct Projection {
    revision: u64,
    provider: Option<Arc<dyn WallpaperHost>>,
    busy: bool,
    command_available: bool,
    index: i32,
    observed: slint::SharedString,
    status: slint::SharedString,
}

impl Projection {
    fn capture(state: &State, session: Session) -> Self {
        let pending = state
            .flight
            .as_ref()
            .filter(|flight| flight.session == session);
        let (index, observed, command_available) = match pending.map(|flight| &flight.command) {
            Some(Command::Write { issued, index, .. }) => (
                *index,
                format!(
                    "Last native observation: {} (consumed for this request).",
                    caption(issued.position)
                ),
                true,
            ),
            _ => match (state.observation.as_ref(), state.desired) {
                (Some(observation), Some(desired)) => (
                    position_index(desired),
                    format!(
                        "Native observed position: {}.",
                        caption(observation.position)
                    ),
                    true,
                ),
                _ => (
                    -1,
                    "Native current position has not been read or is unknown.".into(),
                    false,
                ),
            },
        };
        let status = match state.flight.as_ref() {
            Some(flight) if flight.session != session => "A previous global position request is pending. Its result will not be shown in this view.".into(),
            Some(flight) => match &flight.command {
                Command::Read => "Reading the actual global Windows wallpaper position…".into(),
                Command::Write { desired, .. } => format!("Requesting {} globally on the system's monitors, including future monitors… Awaiting fresh native readback.", caption(*desired)),
            },
            None if state.provider.is_none() => "Native global Windows position is unavailable. No current position or default is assumed.".into(),
            None if state.notice.is_empty() => "Read current explicitly to obtain an actual native observation. Nothing is read or changed automatically.".into(),
            None => state.notice.clone(),
        };
        Self {
            revision: state.sequence,
            provider: state.provider.clone(),
            busy: state.flight.is_some(),
            command_available,
            index,
            observed: observed.into(),
            status: status.into(),
        }
    }
}

impl PositionController {
    pub(super) fn project(&self, session: Session, intent: Option<&Flight>) -> bool {
        if self.projecting.replace(true) {
            return false;
        }
        let _guard = ResetFlag(&self.projecting);
        let projection = Projection::capture(&self.state.borrow(), session);
        let Some(panel) = self.panel.upgrade() else {
            return false;
        };
        let current = || {
            if !self.current(session)
                || !self.provider_current(projection.provider.as_ref())
                || intent.is_some_and(|flight| {
                    !self.intent_current(session, &flight.command) || !self.flight_current(flight)
                })
            {
                return false;
            }
            self.state.borrow().sequence == projection.revision
        };
        macro_rules! publish {
            ($setter:ident, $value:expr) => {
                if !current() {
                    return false;
                }
                panel.$setter($value);
                if !current() {
                    return false;
                }
            };
        }
        publish!(set_wallpaper_position_read_enabled, false);
        publish!(set_wallpaper_position_apply_enabled, false);
        publish!(set_wallpaper_position_desired_enabled, false);
        // Retain readonly write command geometry until its final admission check.
        publish!(
            set_wallpaper_position_command_available,
            projection.command_available
        );
        publish!(set_wallpaper_position_desired_index, projection.index);
        publish!(set_wallpaper_position_observed, projection.observed);
        publish!(
            set_wallpaper_position_available,
            projection.provider.is_some()
        );
        publish!(set_wallpaper_position_busy, projection.busy);
        publish!(
            set_wallpaper_position_input_key,
            projection.revision.to_string().into()
        );
        publish!(set_wallpaper_position_status, projection.status);
        let enabled = projection.provider.is_some() && !projection.busy;
        publish!(
            set_wallpaper_position_desired_enabled,
            enabled && projection.command_available
        );
        publish!(
            set_wallpaper_position_apply_enabled,
            enabled && projection.command_available
        );
        publish!(set_wallpaper_position_read_enabled, enabled);
        true
    }

    pub(super) fn clear_projection(&self, revision: u64) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = ResetFlag(&self.projecting);
        let busy = self.state.borrow().flight.is_some();
        let Some(panel) = self.panel.upgrade() else {
            return;
        };
        macro_rules! clear {
            ($setter:ident, $value:expr) => {
                if !self.stopped(revision) {
                    return;
                }
                panel.$setter($value);
                if !self.stopped(revision) {
                    return;
                }
            };
        }
        clear!(set_wallpaper_position_read_enabled, false);
        clear!(set_wallpaper_position_apply_enabled, false);
        clear!(set_wallpaper_position_desired_enabled, false);
        clear!(set_wallpaper_position_command_available, false);
        clear!(set_wallpaper_position_desired_index, -1);
        clear!(
            set_wallpaper_position_observed,
            "Native current position is unknown; Read current again.".into()
        );
        clear!(
            set_wallpaper_position_input_key,
            slint::SharedString::default()
        );
        clear!(set_wallpaper_position_available, false);
        clear!(set_wallpaper_position_busy, busy);
        clear!(set_wallpaper_position_status, "The position observation is no longer current. Read current explicitly; no automatic retry.".into());
    }
}
