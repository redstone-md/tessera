// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Actual policy facts and exact native members, orthogonal to file-selection drafts.

use super::*;
use native_collection::NativeStep;
use native_slideshow::{AdvanceOutcome, Direction, Observation, TargetWeak};
use slint::Model;

mod options;
pub(super) use options::Proposal as OptionsProposal;

#[derive(Clone)]
pub(super) struct Current {
    pub(super) observation: Observation,
    pub(super) index: i32,
    pub(super) proposal: Option<OptionsProposal>,
}

impl Current {
    fn new(mut observation: Observation, issued: Option<&Issued>) -> Option<Self> {
        if observation.monitors.len() > 32
            || observation
                .monitors
                .iter()
                .enumerate()
                .any(|(index, monitor)| {
                    observation.monitors[..index]
                        .iter()
                        .any(|previous| previous.target == monitor.target)
                        || issued.is_some_and(|issued| issued.members.contains(&monitor.target))
                })
            || observation.target.as_ref().is_some_and(|target| {
                !active(&observation)
                    || observation.monitors.is_empty()
                    || observation.options.is_none()
                    || observation
                        .native_entry_count
                        .is_none_or(|count| !(1..=32).contains(&count))
                    || issued
                        .and_then(|issued| issued.target.as_ref())
                        .is_some_and(|old| old.matches(target))
            })
        {
            return None;
        }
        for monitor in &mut observation.monitors {
            if monitor.caption.chars().take(257).count() > 256 {
                return None;
            }
            monitor.caption = selection::bounded_caption(&monitor.caption, 256)?;
        }
        Some(Self {
            proposal: observation.options.map(OptionsProposal::from_readback),
            observation,
            index: -1,
        })
    }

    pub(super) fn member(&self, index: i32) -> Option<WallpaperMonitorTarget> {
        self.observation
            .monitors
            .get(usize::try_from(index).ok()?)
            .map(|monitor| monitor.target.clone())
    }

    pub(super) fn issued(&self, direction: Option<Direction>) -> Issued {
        Issued {
            target: self
                .observation
                .target
                .as_ref()
                .map(native_slideshow::Target::downgrade),
            members: self
                .observation
                .monitors
                .iter()
                .map(|monitor| monitor.target.clone())
                .collect(),
            monitor: self.member(self.index),
            index: self.index,
            direction,
        }
    }
}

#[derive(Clone)]
pub(super) struct Issued {
    pub(super) target: Option<TargetWeak>,
    members: Vec<WallpaperMonitorTarget>,
    pub(super) monitor: Option<WallpaperMonitorTarget>,
    pub(super) index: i32,
    pub(super) direction: Option<Direction>,
}

fn active(observation: &Observation) -> bool {
    observation.state.enabled
        && observation.state.slideshow
        && !observation.state.disabled_by_remote_session
}

pub(super) struct Projection {
    pub(super) captions: Vec<slint::SharedString>,
    pub(super) index: i32,
    pub(super) selector_available: bool,
    pub(super) advance_enabled: bool,
    pub(super) facts: slint::SharedString,
    pub(super) status: slint::SharedString,
    pub(super) options: options::Projection,
}

impl Projection {
    pub(super) fn capture(state: &State, session: Session) -> Self {
        let current = state.slideshow.as_ref();
        let facts = current.map(|current| {
            let observation = &current.observation;
            let options = observation.options.map(|options| format!(
                "Native interval: {} ms; shuffle: {}.", options.interval_ms, options.shuffle
            )).unwrap_or_else(|| "Native interval/shuffle unavailable.".into());
            let count = observation.native_entry_count.map(|count| format!(
                "Native array members: {count}. One folder member is not an image count; folder contents are not enumerated."
            )).unwrap_or_else(|| "Native array member count unavailable.".into());
            format!("SDK enabled: {}; slideshow configured: {}; disabled by remote session: {}. {options} {count}",
                observation.state.enabled, observation.state.slideshow, observation.state.disabled_by_remote_session)
        }).unwrap_or_else(|| "Native current slideshow policy has not been read.".into());
        let status = match state.flight.as_ref() {
            Some(flight) if flight.session != session => "A previous native request is pending; its old facts will not be restored here.".into(),
            Some(flight) if flight.operation == Operation::ReadSlideshow => "Reading actual native policy, options and captured displays…".into(),
            Some(flight) if matches!(flight.operation, Operation::AdvanceSlideshow(_)) => "Requesting one exact captured SDK monitor; awaiting setter receipt and independent fresh native readback…".into(),
            Some(flight) if flight.operation == Operation::SetSlideshowOptions => "Requesting global current-policy options only; the slideshow source is not replaced or restarted. Awaiting independent native receipt/readback…".into(),
            _ if !state.slideshow_notice.is_empty() => state.slideshow_notice.clone(),
            _ if current.is_some_and(|current| current.observation.target.is_some()) => "Choose a genuine captured display before Previous or Next. SDK flags do not prove rendered advancement.".into(),
            _ if current.is_some() => "Actual native facts are read-only: no safe active policy authority was issued. Read current again explicitly when ready.".into(),
            _ => "Read current explicitly; no current policy or monitor is assumed.".into(),
        };
        Self {
            captions: current
                .map(|current| {
                    current
                        .observation
                        .monitors
                        .iter()
                        .map(|monitor| monitor.caption.as_str().into())
                        .collect()
                })
                .unwrap_or_default(),
            index: current.map_or(-1, |current| current.index),
            selector_available: current
                .is_some_and(|current| !current.observation.monitors.is_empty()),
            advance_enabled: state.provider.is_some()
                && state.flight.is_none()
                && current.is_some_and(|current| {
                    current.observation.target.is_some()
                        && active(&current.observation)
                        && current.member(current.index).is_some()
                }),
            facts: facts.into(),
            status: status.into(),
            options: options::Projection::capture(state),
        }
    }
}

impl WallpaperController {
    fn slideshow_model_current(current: &Current, panel: &Panel) -> bool {
        let model = panel.get_wallpaper_slideshow_monitors();
        model.row_count() == current.observation.monitors.len()
            && current
                .observation
                .monitors
                .iter()
                .enumerate()
                .all(|(index, monitor)| {
                    model
                        .row_data(index)
                        .is_some_and(|caption| caption.as_str() == monitor.caption.as_str())
                })
    }

    pub(super) fn slideshow_choice_current(&self, panel: &Panel, direction: Direction) -> bool {
        // Model getters are foreign callbacks too. Block recursive admission,
        // but keep Root/source checks live throughout this read-only validation.
        if self.acquiring.replace(true) {
            return false;
        }
        let _guard = ResetFlag(&self.acquiring);
        let (snapshot, revision, session, ticket) = {
            let state = self.state.borrow();
            let Some(current) = state.slideshow.as_ref() else {
                return false;
            };
            let Some(session) = state.session else {
                return false;
            };
            (
                current.clone(),
                state.sequence,
                session,
                state.flight.as_ref().map(|flight| flight.ticket),
            )
        };
        let index = panel.get_wallpaper_slideshow_monitor_index();
        if index != snapshot.index
            || !active(&snapshot.observation)
            || !Self::slideshow_model_current(&snapshot, panel)
        {
            return false;
        }
        let Some(member) = snapshot.member(index) else {
            return false;
        };
        if !self.current(session) || !Self::focused(panel) {
            return false;
        }
        let input = match direction {
            Direction::Backward => {
                panel.get_wallpaper_slideshow_previous_input_active()
                    && panel.get_wallpaper_slideshow_previous_control_visible()
            }
            Direction::Forward => {
                panel.get_wallpaper_slideshow_next_input_active()
                    && panel.get_wallpaper_slideshow_next_control_visible()
            }
        };
        if !input || panel.get_wallpaper_slideshow_monitor_index() != index {
            return false;
        }
        // No Panel/model getter runs under this final pure state borrow.
        let state = self.state.borrow();
        if state.sequence != revision
            || state.session != Some(session)
            || state.flight.as_ref().map(|flight| flight.ticket) != ticket
            || state.slideshow.as_ref().is_none_or(|current| {
                current.index != snapshot.index
                    || current.observation != snapshot.observation
                    || current.proposal != snapshot.proposal
            })
        {
            return false;
        }
        if let Some(flight) = state.flight.as_ref() {
            return flight.session == session
                && flight.operation == Operation::AdvanceSlideshow(direction)
                && snapshot.observation.target.is_none()
                && flight.slideshow.as_ref().is_some_and(|issued| {
                    issued.index == index
                        && issued.monitor.as_ref() == Some(&member)
                        && issued.direction == Some(direction)
                        && issued.target.as_ref().is_some_and(TargetWeak::is_alive)
                });
        }
        snapshot.observation.target.is_some()
    }

    pub(super) fn slideshow_monitor_changed(self: &Rc<Self>, index: i32) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let retired = {
            self.acquiring.set(true);
            let _guard = ResetFlag(&self.acquiring);
            let (snapshot, revision, session) = {
                let state = self.state.borrow();
                if state.flight.is_some() {
                    return;
                }
                let Some(current) = state.slideshow.as_ref() else {
                    return;
                };
                let Some(session) = state.session else { return };
                (current.clone(), state.sequence, session)
            };
            let Some((panel, _)) = self.source() else {
                return;
            };
            let displayed_index = panel.get_wallpaper_slideshow_monitor_index();
            let valid = displayed_index == index
                && snapshot.member(index).is_some()
                && Self::slideshow_model_current(&snapshot, &panel);
            // Foreign model callbacks may retire the Root or replace data.
            // Re-read native focus, live source and the callback latch first.
            if !self.current(session)
                || !Self::focused(&panel)
                || !panel.get_wallpaper_slideshow_monitor_input_active()
                || panel.get_wallpaper_slideshow_monitor_index() != displayed_index
            {
                return;
            }
            let mut state = self.state.borrow_mut();
            if state.sequence != revision
                || state.session != Some(session)
                || state.flight.is_some()
                || state.slideshow.as_ref().is_none_or(|current| {
                    current.index != snapshot.index || current.observation != snapshot.observation
                })
            {
                return;
            }
            let Some(current) = state.slideshow.as_mut() else {
                return;
            };
            let retired = if valid {
                current.index = index;
                None
            } else {
                current.index = -1;
                current.observation.target.take()
            };
            state.slideshow_notice.clear();
            let _ = state.next();
            (retired, session)
        };
        let (retired, session) = retired;
        drop(retired);
        // Rotate the shared activation key so held gestures cannot change scope.
        if !self.project(session, None) {
            self.stop_root();
        }
    }

    pub(super) fn receive_slideshow(self: &Rc<Self>, flight: Flight, reply: Reply) {
        let (current, notice) = match (flight.operation, reply) {
            (Operation::ReadSlideshow, Reply::SlideshowRead(Ok(observation))) => {
                match Current::new(observation, flight.slideshow.as_ref()) {
                    Some(current) => (Some(current), String::new()),
                    None => (None, invalid_notice().into()),
                }
            }
            (Operation::AdvanceSlideshow(_), Reply::SlideshowAdvanced(Ok(outcome))) => {
                outcome_projection(flight.slideshow.as_ref(), outcome)
            }
            (Operation::SetSlideshowOptions, Reply::SlideshowOptionsSet(Ok(outcome))) => {
                options::outcome_projection(flight.slideshow.as_ref(), outcome)
            }
            (Operation::ReadSlideshow, Reply::SlideshowRead(Err(error)))
            | (Operation::AdvanceSlideshow(_), Reply::SlideshowAdvanced(Err(error)))
            | (Operation::SetSlideshowOptions, Reply::SlideshowOptionsSet(Err(error))) => {
                let notice = match error {
                    WallpaperError::Busy => {
                        "The shared native wallpaper provider is busy. No retry is queued; Read current again explicitly."
                    }
                    WallpaperError::InvalidTarget => {
                        "The exact current policy/display observation is no longer valid. No setter was submitted; Read current again."
                    }
                    WallpaperError::Unavailable => {
                        "Native slideshow facts are unavailable. Effects may already have occurred; no rollback or retry was performed. Read current again explicitly."
                    }
                };
                (None, notice.into())
            }
            _ => (None, invalid_notice().into()),
        };
        if !self.current(flight.session) {
            drop(current);
            self.refresh_root();
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            state.slideshow_notice = notice;
            std::mem::replace(&mut state.slideshow, current)
        };
        drop(retired);
        if !self.project(flight.session, None) {
            self.stop_root();
        }
    }
}

fn outcome_projection(
    issued: Option<&Issued>,
    outcome: AdvanceOutcome,
) -> (Option<Current>, String) {
    let AdvanceOutcome {
        receipt,
        changed,
        observation,
        selected_monitor,
    } = outcome;
    let setter = match receipt {
        NativeStep::Accepted => "AdvanceSlideshow accepted",
        NativeStep::Rejected => "AdvanceSlideshow rejected",
        NativeStep::NotSubmitted => return (None, invalid_notice().into()),
    };
    let Some(issued) = issued else {
        return (None, invalid_notice().into());
    };
    if issued.monitor.is_none() || issued.direction.is_none() || issued.target.is_none() {
        return (None, invalid_notice().into());
    }
    let current = match observation {
        Some(observation) => {
            let Some(mut current) = Current::new(observation, Some(issued)) else {
                return (None, format!("{setter}. {}", invalid_notice()));
            };
            if let Some(selected) = selected_monitor {
                if current.observation.target.is_none() {
                    return (None, format!("{setter}. {}", invalid_notice()));
                }
                let Some(index) = current
                    .observation
                    .monitors
                    .iter()
                    .position(|monitor| monitor.target == selected)
                else {
                    return (None, format!("{setter}. {}", invalid_notice()));
                };
                current.index = index as i32;
            }
            Some(current)
        }
        None if selected_monitor.is_none() => None,
        None => return (None, format!("{setter}. {}", invalid_notice())),
    };
    let change = match changed {
        Some(true) => "Independent native wallpaper file identity/metadata readback changed.",
        Some(false) => {
            "Independent native wallpaper file identity/metadata readback was unchanged."
        }
        None => "Native wallpaper file change is unknown.",
    };
    let facts = if current.is_some() {
        "Fresh policy facts were read."
    } else {
        "Fresh policy facts are unavailable; Read current again."
    };
    (
        current,
        format!(
            "{setter}. {change} {facts} This does not verify rendered pixels or attribute a change solely to this request: automatic transitions, panorama and concurrent writers can affect readback. No rollback or retry was performed."
        ),
    )
}

fn invalid_notice() -> &'static str {
    "Invalid native slideshow response; current facts and control authority were cleared. Effects are unknown; Read current again. No retry or rollback was requested."
}
