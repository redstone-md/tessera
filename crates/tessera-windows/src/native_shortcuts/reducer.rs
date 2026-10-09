// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure hook-payload reducer; a candidate is not delivery or release suppression.

const LEFT_WIN: u32 = 0x5b;
const RIGHT_WIN: u32 = 0x5c;

#[derive(Clone, Copy, Debug)]
pub(super) struct KeyEvent {
    pub(super) key: u32,
    pub(super) down: bool,
    pub(super) injected: bool,
    pub(super) unknown: bool,
}

pub(super) struct BareWinReducer {
    enabled: bool,
    generation: u64,
    preheld: [bool; 256],
    physical_held: [bool; 256],
    injected_held: [bool; 256],
    held_count: usize,
    candidate: Option<u32>,
    desynchronized: bool,
    rearm_required: bool,
}

impl BareWinReducer {
    pub(super) fn new() -> Self {
        Self {
            enabled: false,
            generation: 0,
            preheld: [false; 256],
            physical_held: [false; 256],
            injected_held: [false; 256],
            held_count: 0,
            candidate: None,
            desynchronized: false,
            rearm_required: false,
        }
    }

    /// Cancels any gesture. SDK held-key bits have unknown input origin, so the
    /// seed is separate from observed hardware/injected transitions after reset.
    pub(super) fn reset(&mut self, enabled: bool, generation: u64, held: [bool; 256]) {
        self.enabled = enabled;
        self.generation = generation;
        self.preheld = held;
        self.physical_held = [false; 256];
        self.injected_held = [false; 256];
        self.held_count = held.iter().filter(|&&down| down).count();
        self.candidate = None;
        self.desynchronized = held[0];
        self.rearm_required = false;
    }

    /// A seed release with untrusted origin requires an explicit owner reset
    /// using a fresh SDK snapshot; later key events cannot silently rearm it.
    pub(super) fn rearm_required(&self) -> bool {
        self.rearm_required
    }

    /// Constant-time, fixed-storage transitions. Only a balanced, untainted
    /// hardware Win down/up beginning with no held keys returns a generation.
    pub(super) fn key(&mut self, event: KeyEvent) -> Option<u64> {
        if !(1..=255).contains(&event.key) {
            // An untrackable key cannot be balanced safely by guessing. A fresh
            // authoritative reset is required before admitting another gesture.
            self.candidate = None;
            self.desynchronized = true;
            self.rearm_required = true;
            return None;
        }

        let held_before = self.held_count;
        let index = event.key as usize;
        if !event.down && self.preheld[index] {
            self.candidate = None;
            if event.injected || event.unknown {
                // The seed may represent a physical key. Never let an injected
                // or unknown release clear it or any observed hardware hold.
                self.desynchronized = true;
                self.rearm_required = true;
            } else {
                self.preheld[index] = false;
                self.held_count -= 1;
            }
        }
        let slot = if event.injected {
            &mut self.injected_held[index]
        } else {
            &mut self.physical_held[index]
        };
        let balanced = *slot != event.down;
        if balanced {
            *slot = event.down;
            if event.down {
                self.held_count += 1;
            } else {
                self.held_count -= 1;
            }
        }

        // Both observed origins and unknown-origin seeds prevent fresh starts.
        // Injected releases only balance observed injected keys, never hardware.
        if !self.enabled || self.desynchronized || event.injected || event.unknown || !balanced {
            self.candidate = None;
            return None;
        }

        if event.down {
            self.candidate = if held_before == 0 && matches!(event.key, LEFT_WIN | RIGHT_WIN) {
                Some(event.key)
            } else {
                None
            };
            None
        } else {
            let candidate = self.candidate.take();
            if candidate == Some(event.key) && self.held_count == 0 {
                Some(self.generation)
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "reducer_tests.rs"]
mod tests;
