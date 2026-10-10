// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

fn describe<T>(fact: BatteryFact<T>, value: impl FnOnce(T) -> String) -> String {
    match fact {
        BatteryFact::Known(known) => value(known),
        BatteryFact::Unknown => "Unknown (Windows did not report a usable value)".into(),
        BatteryFact::Unavailable(error) => format!("Unavailable: {error}"),
    }
}
fn battery_state(value: BatteryState) -> &'static str {
    match value {
        BatteryState::NotPresent => "No system battery",
        BatteryState::Discharging => "Discharging",
        BatteryState::Idle => "Present, not charging or discharging",
        BatteryState::Charging => "Charging",
    }
}
fn percent(snapshot: BatterySnapshot) -> Option<u8> {
    match snapshot.percent {
        BatteryFact::Known(value) if value <= 100 => Some(value),
        _ => None,
    }
}
fn row(label: &str, value: String) -> BatteryRow {
    BatteryRow {
        label: label.into(),
        value: value.into(),
    }
}
fn rows(snapshot: BatterySnapshot) -> Vec<BatteryRow> {
    let absent = snapshot.battery == BatteryFact::Known(BatteryState::NotPresent);
    vec![
        row(
            "Battery",
            describe(snapshot.battery, |state| battery_state(state).into()),
        ),
        row(
            "Power supply",
            describe(snapshot.supply, |state| match state {
                PowerSupplyState::NotPresent => "No power supply connected".into(),
                PowerSupplyState::Inadequate => "Connected, inadequate power".into(),
                PowerSupplyState::Adequate => "Connected, adequate power".into(),
            }),
        ),
        row(
            "Remaining charge",
            if absent {
                "Not applicable: no system battery".into()
            } else {
                describe(snapshot.percent, |value| {
                    if value <= 100 {
                        format!("{value}%")
                    } else {
                        BatteryError::InvalidData.to_string()
                    }
                })
            },
        ),
        row(
            "Remaining runtime",
            if absent {
                "Not applicable: no system battery".into()
            } else {
                describe(snapshot.remaining_seconds, |seconds| {
                    if seconds == u32::MAX {
                        "Unknown (Windows did not report an estimate)".into()
                    } else {
                        format!("Windows estimate: {seconds} seconds")
                    }
                })
            },
        ),
        row(
            "Energy saver (read-only)",
            describe(snapshot.energy_saver, |state| match state {
                EnergySaverState::Disabled => "Disabled by Windows".into(),
                EnergySaverState::Off => "Off".into(),
                EnergySaverState::On => "On".into(),
            }),
        ),
    ]
}
fn partial(snapshot: BatterySnapshot) -> bool {
    !matches!(snapshot.battery, BatteryFact::Known(_))
        || !matches!(snapshot.supply, BatteryFact::Known(_))
        || percent(snapshot).is_none()
        || !matches!(snapshot.remaining_seconds, BatteryFact::Known(value) if value != u32::MAX)
        || !matches!(snapshot.energy_saver, BatteryFact::Known(_))
}
fn feedback(state: &State) -> String {
    let mut messages = Vec::new();
    if state.exhausted {
        messages.push(BatteryError::Exhausted.to_string());
    }
    if state.unsupported {
        messages.push("Battery capability is not available on this host.".into());
    }
    if state.acquiring {
        messages.push("Loading battery capability…".into());
    }
    if state.read_flight.is_some() {
        messages.push(
            if state.snapshot.is_some() {
                "Refreshing Windows battery cache…"
            } else {
                "Reading Windows battery cache…"
            }
            .into(),
        );
    }
    if let Some(error) = state.factory_error.or(state.read_error) {
        messages.push(error.to_string());
    }
    if state.stale && state.snapshot.is_some() {
        messages.push("Cached facts may be stale; they are not current-state confirmation.".into());
    }
    if let Some(snapshot) = state.snapshot {
        if partial(snapshot) {
            messages.push("Some Windows facts are unknown or unavailable.".into());
        }
    } else if !state.unsupported && state.read_flight.is_none() && !state.acquiring {
        messages.push("No battery observations available yet.".into());
    }
    if state.settings_flight.is_some() {
        messages.push("Requesting Windows Settings launch…".into());
    }
    if !state.settings_notice.is_empty() {
        messages.push(state.settings_notice.clone());
    }
    messages.join("\n")
}
struct Projection {
    sequence: u64,
    session: u64,
    root_active: bool,
    rows: Vec<BatteryRow>,
    feedback: String,
    watch: String,
    refresh_key: SharedString,
    settings_key: SharedString,
    toolbar: BatteryProjection,
}
impl BatteryController {
    pub(super) fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let _guard = FlagGuard(&self.projecting);
        let open = self.is_open();
        let frame = if open { Some(self.frame()) } else { None };
        let plan = {
            let mut state = self.state.borrow_mut();
            let id = state.next();
            let enabled = open && !state.exhausted && !state.unsupported;
            state.refresh_key = if enabled && state.read_flight.is_none() && !state.acquiring {
                id.map_or_else(SharedString::default, |id| {
                    format!("battery-refresh:{}:{id}", state.session).into()
                })
            } else {
                SharedString::default()
            };
            state.settings_key =
                if enabled && state.provider.is_some() && state.settings_flight.is_none() {
                    id.map_or_else(SharedString::default, |id| {
                        format!("battery-settings:{}:{id}", state.session).into()
                    })
                } else {
                    SharedString::default()
                };
            state.input_frame = frame;
            let absent = state.snapshot.is_some_and(|snapshot| {
                snapshot.battery == BatteryFact::Known(BatteryState::NotPresent)
            });
            let known_percent = if absent {
                None
            } else {
                state.snapshot.and_then(percent)
            };
            let mut label = match state.snapshot {
                Some(snapshot) => format!(
                    "Battery: {}",
                    describe(snapshot.battery, |value| battery_state(value).into())
                ),
                None => "Battery status unknown".into(),
            };
            if let Some(value) = known_percent {
                label.push_str(&format!("; {value}% remaining"));
            }
            if state.stale {
                label.push_str("; cached facts may be stale");
            }
            if state.read_flight.is_some() || state.acquiring {
                label.push_str("; loading");
            }
            if let Some(error) = state.factory_error.or(state.read_error) {
                label.push_str(&format!("; {error}"));
            }
            if state.exhausted {
                label.push_str("; battery identifiers exhausted; restart required");
            }
            let visible = state.root_active
                && !state.unsupported
                && (!absent
                    || state.stale
                    || state.read_error.is_some()
                    || state.factory_error.is_some());
            Projection {
                sequence: state.sequence,
                session: state.session,
                root_active: state.root_active,
                rows: state.snapshot.map_or_else(Vec::new, rows),
                feedback: feedback(&state),
                watch: state.watch_status.clone(),
                refresh_key: state.refresh_key.clone(),
                settings_key: state.settings_key.clone(),
                toolbar: BatteryProjection {
                    visible,
                    text: known_percent
                        .map_or_else(|| "Battery".into(), |value| format!("{value}%").into()),
                    accessible_label: label.into(),
                    percent: known_percent,
                    activation_key: if visible && !state.exhausted {
                        id.map_or_else(SharedString::default, |id| {
                            format!("battery-toolbar:{id}").into()
                        })
                    } else {
                        SharedString::default()
                    },
                },
            }
        };
        let current = || {
            let state = self.state.borrow();
            state.sequence == plan.sequence
                && state.session == plan.session
                && state.root_active == plan.root_active
        };
        self.component().invoke_cancel_input();
        if !current() {
            return;
        }
        self.component()
            .set_rows(ModelRc::new(slint::VecModel::from(plan.rows)));
        if !current() {
            return;
        }
        self.component().set_feedback(plan.feedback.into());
        if !current() {
            return;
        }
        self.component().set_watch_status(plan.watch.into());
        if !current() {
            return;
        }
        self.component().set_refresh_key(plan.refresh_key);
        if !current() {
            return;
        }
        self.component().set_settings_key(plan.settings_key);
        if !current() {
            return;
        }
        let projection = self.toolbar_projection.borrow().clone();
        if let Some(projection) = projection {
            projection(plan.toolbar);
        }
    }
    pub(super) fn project_and_fit(&self) {
        self.project();
        if self.is_open() {
            let _ = self.refit();
        }
    }
}
