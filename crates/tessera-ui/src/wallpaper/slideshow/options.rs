// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Local closed proposals for a single global options write, never source replacement.

use super::*;
use native_collection::{Interval, Options, OptionsReadback};
use native_slideshow::{OptionsDisposition, OptionsOutcome};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::wallpaper) struct Proposal {
    pub(in crate::wallpaper) index: i32,
    pub(in crate::wallpaper) shuffle: bool,
}

impl Proposal {
    pub(super) fn from_readback(options: OptionsReadback) -> Self {
        Self {
            index: Interval::ALL
                .iter()
                .position(|interval| interval.milliseconds() == options.interval_ms)
                .map_or(-1, |index| index as i32),
            shuffle: options.shuffle,
        }
    }

    pub(in crate::wallpaper) fn options(self) -> Option<Options> {
        let interval = *Interval::ALL.get(usize::try_from(self.index).ok()?)?;
        Some(Options {
            interval,
            shuffle: self.shuffle,
        })
    }

    fn displayed(panel: &Panel) -> Self {
        Self {
            index: panel.get_wallpaper_slideshow_interval_index(),
            shuffle: panel.get_wallpaper_slideshow_shuffle(),
        }
    }
}

pub(in crate::wallpaper) struct Projection {
    pub(in crate::wallpaper) available: bool,
    pub(in crate::wallpaper) editable: bool,
    pub(in crate::wallpaper) apply_enabled: bool,
    pub(in crate::wallpaper) index: i32,
    pub(in crate::wallpaper) shuffle: bool,
}

impl Projection {
    pub(super) fn capture(state: &State) -> Self {
        let current = state.slideshow.as_ref();
        let proposal = current.and_then(|current| current.proposal);
        let editable = state.provider.is_some()
            && state.flight.is_none()
            && current.is_some_and(|current| {
                current.observation.target.is_some()
                    && active(&current.observation)
                    && proposal.is_some()
            });
        Self {
            available: proposal.is_some(),
            editable,
            apply_enabled: editable
                && proposal.is_some_and(|proposal| proposal.options().is_some()),
            index: proposal.map_or(-1, |proposal| proposal.index),
            shuffle: proposal.is_some_and(|proposal| proposal.shuffle),
        }
    }
}

impl WallpaperController {
    pub(in crate::wallpaper) fn slideshow_options_current(&self, panel: &Panel) -> bool {
        if self.acquiring.replace(true) {
            return false;
        }
        let _guard = ResetFlag(&self.acquiring);
        let (snapshot, revision, session, provider, ticket) = {
            let state = self.state.borrow();
            let Some(current) = state.slideshow.as_ref() else {
                return false;
            };
            let Some(session) = state.session else {
                return false;
            };
            let Some(provider) = state.provider.clone() else {
                return false;
            };
            (
                current.clone(),
                state.sequence,
                session,
                provider,
                state.flight.as_ref().map(|flight| flight.ticket),
            )
        };
        let displayed = Proposal::displayed(panel);
        if snapshot.proposal != Some(displayed)
            || displayed.options().is_none()
            || !active(&snapshot.observation)
        {
            return false;
        }
        // All foreign getters run without state borrows; re-check the live input
        // after source/focus/provider admission, then compare a pure state snapshot.
        if !self.current(session)
            || !Self::focused(panel)
            || !self.provider_current(Some(&provider))
            || !panel.get_wallpaper_slideshow_options_input_active()
            || !panel.get_wallpaper_slideshow_options_control_visible()
            || Proposal::displayed(panel) != displayed
        {
            return false;
        }
        let state = self.state.borrow();
        if state.sequence != revision
            || state.session != Some(session)
            || state
                .provider
                .as_ref()
                .is_none_or(|current| !Arc::ptr_eq(current, &provider))
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
                && flight.operation == Operation::SetSlideshowOptions
                && flight.slideshow_options == Some(displayed)
                && snapshot.observation.target.is_none()
                && flight.slideshow.as_ref().is_some_and(|issued| {
                    issued.target.as_ref().is_some_and(TargetWeak::is_alive)
                });
        }
        snapshot.observation.target.is_some()
    }

    pub(in crate::wallpaper) fn slideshow_options_changed(
        self: &Rc<Self>,
        changed_index: Option<i32>,
    ) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let (retired, session) = {
            self.acquiring.set(true);
            let _guard = ResetFlag(&self.acquiring);
            let (snapshot, revision, session, provider) = {
                let state = self.state.borrow();
                if state.flight.is_some() {
                    return;
                }
                let Some(current) = state.slideshow.as_ref() else {
                    return;
                };
                if current.observation.target.is_none() || !active(&current.observation) {
                    return;
                }
                let Some(session) = state.session else { return };
                let Some(provider) = state.provider.clone() else {
                    return;
                };
                (current.clone(), state.sequence, session, provider)
            };
            let Some(original) = snapshot.proposal else {
                return;
            };
            let Some((panel, _)) = self.source() else {
                return;
            };
            let displayed = Proposal::displayed(&panel);
            let valid = match changed_index {
                Some(index) => {
                    index == displayed.index
                        && displayed.options().is_some()
                        && displayed.shuffle == original.shuffle
                }
                None => {
                    displayed.index == original.index
                        && (displayed.index == -1 || displayed.options().is_some())
                }
            };
            if !self.current(session)
                || !Self::focused(&panel)
                || !self.provider_current(Some(&provider))
                || !options_edit_current(&panel, changed_index)
                || Proposal::displayed(&panel) != displayed
            {
                return;
            }
            let mut state = self.state.borrow_mut();
            if state.sequence != revision
                || state.session != Some(session)
                || state.flight.is_some()
                || state
                    .provider
                    .as_ref()
                    .is_none_or(|current| !Arc::ptr_eq(current, &provider))
                || state.slideshow.as_ref().is_none_or(|current| {
                    current.index != snapshot.index
                        || current.observation != snapshot.observation
                        || current.proposal != snapshot.proposal
                })
            {
                return;
            }
            let Some(current) = state.slideshow.as_mut() else {
                return;
            };
            let retired = if valid {
                current.proposal = Some(displayed);
                None
            } else {
                current.proposal = None;
                current.observation.target.take()
            };
            state.slideshow_notice = if valid {
                "Current-policy options were edited locally only. Actual raw SDK milliseconds/flags remain the separate observation above. Apply changes global options without replacing or restarting the source.".into()
            } else {
                "Invalid current-policy options proposal; control authority was retired. No SDK work was requested. Read current again explicitly.".into()
            };
            let _ = state.next();
            (retired, session)
        };
        drop(retired);
        // Draft edits rotate the shared key, retiring held Apply/advance gestures.
        if !self.project(session, None) {
            self.stop_root();
        }
    }
}

fn options_edit_current(panel: &Panel, changed_index: Option<i32>) -> bool {
    if changed_index.is_some() {
        panel.get_wallpaper_slideshow_interval_input_active()
            && panel.get_wallpaper_slideshow_interval_control_visible()
    } else {
        panel.get_wallpaper_slideshow_shuffle_input_active()
            && panel.get_wallpaper_slideshow_shuffle_control_visible()
    }
}

pub(super) fn outcome_projection(
    issued: Option<&Issued>,
    outcome: OptionsOutcome,
) -> (Option<Current>, String) {
    let receipt = match outcome.disposition {
        OptionsDisposition::AlreadyCurrent => {
            "Revalidated native options were already current; no setter was called."
        }
        OptionsDisposition::Accepted => "Global SetSlideshowOptions accepted.",
        OptionsDisposition::Rejected => "Global SetSlideshowOptions rejected.",
    };
    let invalid = || {
        format!(
            "{receipt} Fresh native readback was invalid; current facts/control authority were cleared. Read current again explicitly. Independent readback is not persistence, rendered-pixel or sole-request causality proof. No retry or rollback was requested."
        )
    };
    let Some(issued) = issued.filter(|issued| issued.target.is_some()) else {
        return (None, invalid());
    };
    let current = match outcome.observation {
        Some(observation) => match Current::new(observation, Some(issued)) {
            Some(current) => Some(current),
            None => return (None, invalid()),
        },
        None => None,
    };
    let facts = if current.is_some() {
        "Fresh actual policy facts were read; monitor choice resets and proposals reflect only exact supported native intervals."
    } else {
        "Fresh policy facts are unavailable; authority is cleared. Read current again explicitly."
    };
    (
        current,
        format!(
            "{receipt} {facts} This request does not replace or restart the slideshow source. Independent readback is not persistence, rendered-pixel or sole-request causality proof; concurrent writers and transitions can intervene. No retry or rollback was performed."
        ),
    )
}
