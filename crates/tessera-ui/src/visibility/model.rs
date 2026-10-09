// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use std::time::Duration;
use tessera_system::visibility::{BarFacts, PointerEnvironment, VisibilityDecision};

pub(super) fn protected_facts(mut facts: BarFacts, environment: PointerEnvironment) -> BarFacts {
    facts.touch_primary = match (facts.touch_primary, environment.touch_capable) {
        // Any known touch capability is a conservative inhibit, not a claim of
        // per-monitor physical-primary identity. Unknown cannot become false.
        (Some(true), _) | (_, Some(true)) => Some(true),
        (known, _) if known.is_some() => known,
        (None, Some(false)) => Some(false),
        _ => None,
    };
    facts
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DeadlineToken {
    pub revision: u64,
    pub sequence: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Deadline {
    pub token: DeadlineToken,
    pub target: bool,
    pub at: Duration,
    delay: Duration,
}

/// Autohide state and outer fullscreen suppression are independent. A workspace
/// freeze therefore cannot accidentally turn fullscreen suppression into a new
/// permanent autohide result.
pub(super) struct BarState {
    autohide_visible: bool,
    suppressed: bool,
    sequence: u64,
    pub pending: Option<Deadline>,
}

impl Default for BarState {
    fn default() -> Self {
        Self {
            autohide_visible: true,
            suppressed: false,
            sequence: 0,
            pending: None,
        }
    }
}

impl BarState {
    pub fn visible(&self) -> bool {
        self.autohide_visible && !self.suppressed
    }

    pub fn invalidate(&mut self) {
        self.pending = None;
    }

    pub fn apply(&mut self, decision: VisibilityDecision, revision: u64, now: Duration) {
        self.suppressed = decision == VisibilityDecision::HideNow;
        match decision {
            VisibilityDecision::HideNow | VisibilityDecision::Keep => self.invalidate(),
            VisibilityDecision::ShowNow => {
                self.invalidate();
                self.autohide_visible = true;
            }
            VisibilityDecision::ShowAfter(delay) => self.schedule(true, delay, revision, now),
            VisibilityDecision::HideAfter(delay) => self.schedule(false, delay, revision, now),
        }
    }

    fn schedule(&mut self, target: bool, delay: Duration, revision: u64, now: Duration) {
        if self.autohide_visible == target {
            self.invalidate();
            return;
        }
        if self.pending.is_some_and(|pending| {
            pending.target == target && pending.token.revision == revision && pending.delay == delay
        }) {
            // New mouse coordinates for the same target must not postpone it.
            return;
        }
        let Some(sequence) = self.sequence.checked_add(1) else {
            self.invalidate();
            self.autohide_visible = true;
            return;
        };
        let Some(at) = now.checked_add(delay) else {
            self.invalidate();
            self.autohide_visible = true;
            return;
        };
        self.sequence = sequence;
        self.pending = Some(Deadline {
            token: DeadlineToken { revision, sequence },
            target,
            at,
            delay,
        });
    }

    pub fn expire(&mut self, token: DeadlineToken, revision: u64, now: Duration) -> bool {
        let Some(pending) = self.pending else {
            return false;
        };
        if token != pending.token || token.revision != revision || now < pending.at {
            return false;
        }
        self.pending = None;
        self.autohide_visible = pending.target;
        true
    }
}
