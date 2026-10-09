// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Recorded payloads only: no hook, OS input, registration, or suppression.

use super::{BareWinReducer, KeyEvent};

const LEFT_WIN: u32 = 0x5b;
const RIGHT_WIN: u32 = 0x5c;
const LETTER_K: u32 = 0x4b;

fn hardware(key: u32, down: bool) -> KeyEvent {
    KeyEvent {
        key,
        down,
        injected: false,
        unknown: false,
    }
}

fn injected(key: u32, down: bool) -> KeyEvent {
    KeyEvent {
        injected: true,
        ..hardware(key, down)
    }
}

fn unknown(key: u32, down: bool) -> KeyEvent {
    KeyEvent {
        unknown: true,
        ..hardware(key, down)
    }
}

struct Recording {
    reducer: BareWinReducer,
    candidates: Vec<u64>,
}

impl Recording {
    fn enabled(generation: u64) -> Self {
        let mut reducer = BareWinReducer::new();
        reducer.reset(true, generation, [false; 256]);
        Self {
            reducer,
            candidates: Vec::new(),
        }
    }

    fn send(&mut self, events: &[KeyEvent]) {
        for &event in events {
            if let Some(generation) = self.reducer.key(event) {
                self.candidates.push(generation);
            }
        }
    }

    fn clean_press(&mut self, key: u32) {
        self.send(&[hardware(key, true), hardware(key, false)]);
    }
}

#[test]
fn new_reducer_has_no_enabled_authority() {
    let mut reducer = BareWinReducer::new();
    for key in [LEFT_WIN, RIGHT_WIN] {
        assert_eq!(reducer.key(hardware(key, true)), None);
        assert_eq!(reducer.key(hardware(key, false)), None);
    }
}

#[test]
fn either_literal_win_emits_once_only_on_its_clean_release() {
    for key in [LEFT_WIN, RIGHT_WIN] {
        let mut recording = Recording::enabled(17);
        recording.send(&[hardware(key, true)]);
        assert!(recording.candidates.is_empty());
        recording.send(&[hardware(key, false)]);
        assert_eq!(recording.candidates, [17]);
        recording.send(&[hardware(key, false), hardware(key, false)]);
        assert_eq!(recording.candidates, [17]);
        recording.clean_press(key);
        assert_eq!(recording.candidates, [17, 17]);
    }
}

#[test]
fn balanced_alternating_win_gestures_each_emit_one_candidate() {
    let mut recording = Recording::enabled(23);
    for key in [LEFT_WIN, RIGHT_WIN, LEFT_WIN, RIGHT_WIN] {
        recording.clean_press(key);
    }
    assert_eq!(recording.candidates, [23, 23, 23, 23]);
}

#[test]
fn repeated_win_down_taints_without_inflating_the_held_balance() {
    for key in [LEFT_WIN, RIGHT_WIN] {
        let mut recording = Recording::enabled(31);
        recording.send(&[
            hardware(key, true),
            hardware(key, true),
            hardware(key, true),
            hardware(key, false),
        ]);
        assert!(recording.candidates.is_empty());
        recording.clean_press(key);
        assert_eq!(recording.candidates, [31]);
    }
}

#[test]
fn simultaneous_wins_stay_tainted_in_either_release_order() {
    for first in [LEFT_WIN, RIGHT_WIN] {
        let second = if first == LEFT_WIN {
            RIGHT_WIN
        } else {
            LEFT_WIN
        };
        for releases in [[first, second], [second, first]] {
            let mut recording = Recording::enabled(37);
            recording.send(&[
                hardware(first, true),
                hardware(second, true),
                hardware(releases[0], false),
                hardware(releases[1], false),
            ]);
            assert!(recording.candidates.is_empty());
            recording.clean_press(second);
            assert_eq!(recording.candidates, [37]);
        }
    }
}

#[test]
fn every_other_vk_taints_a_win_chord_in_either_release_order() {
    // Includes all generic/sided modifiers, letters, function and non-modifier keys.
    for win in [LEFT_WIN, RIGHT_WIN] {
        for other in (1..=255).filter(|key| !matches!(*key, LEFT_WIN | RIGHT_WIN)) {
            for releases in [[win, other], [other, win]] {
                let mut recording = Recording::enabled(41);
                recording.send(&[
                    hardware(win, true),
                    hardware(other, true),
                    hardware(releases[0], false),
                    hardware(releases[1], false),
                ]);
                assert!(recording.candidates.is_empty(), "Win {win}, key {other}");
                recording.clean_press(win);
                assert_eq!(recording.candidates, [41]);
            }
        }
    }
}

#[test]
fn every_other_vk_preheld_before_win_blocks_the_whole_gesture() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        for other in (1..=255).filter(|key| !matches!(*key, LEFT_WIN | RIGHT_WIN)) {
            let mut recording = Recording::enabled(43);
            recording.send(&[
                hardware(other, true),
                hardware(win, true),
                hardware(other, false),
                hardware(win, false),
            ]);
            assert!(recording.candidates.is_empty(), "Win {win}, key {other}");
            recording.clean_press(win);
            assert_eq!(recording.candidates, [43]);
        }
    }
}

#[test]
fn unmatched_releases_cancel_a_pending_win_candidate() {
    for stray in [LEFT_WIN, RIGHT_WIN, LETTER_K, 0x10, 255] {
        let win = if stray == LEFT_WIN {
            RIGHT_WIN
        } else {
            LEFT_WIN
        };
        let mut recording = Recording::enabled(47);
        recording.send(&[
            hardware(win, true),
            hardware(stray, false),
            hardware(win, false),
        ]);
        assert!(recording.candidates.is_empty());
        recording.clean_press(win);
        assert_eq!(recording.candidates, [47]);
    }
}

#[test]
fn repeated_non_win_down_does_not_leave_a_phantom_held_key() {
    let mut recording = Recording::enabled(53);
    recording.send(&[
        hardware(LETTER_K, true),
        hardware(LETTER_K, true),
        hardware(LEFT_WIN, true),
        hardware(LEFT_WIN, false),
        hardware(LETTER_K, false),
    ]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(RIGHT_WIN);
    assert_eq!(recording.candidates, [53]);
}

#[test]
fn entirely_injected_win_gestures_never_emit_candidates() {
    // The owner folds both injected and lower-integrity hook flags into injected.
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut recording = Recording::enabled(59);
        recording.send(&[
            injected(win, true),
            injected(win, true),
            injected(win, false),
        ]);
        assert!(recording.candidates.is_empty());
        recording.clean_press(win);
        assert_eq!(recording.candidates, [59]);
    }
}

#[test]
fn injected_release_cannot_make_a_physical_repeat_look_like_a_fresh_press() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut recording = Recording::enabled(61);
        recording.send(&[
            hardware(win, true),
            injected(win, false),
            hardware(win, true),
            hardware(win, false),
        ]);
        assert!(recording.candidates.is_empty());
        recording.clean_press(win);
        assert_eq!(recording.candidates, [61]);
    }
}

#[test]
fn both_input_origins_must_drain_even_for_the_same_vk() {
    for releases in [
        [injected(LEFT_WIN, false), hardware(LEFT_WIN, false)],
        [hardware(LEFT_WIN, false), injected(LEFT_WIN, false)],
    ] {
        let mut recording = Recording::enabled(67);
        recording.send(&[injected(LEFT_WIN, true), hardware(LEFT_WIN, true)]);
        recording.send(&[releases[0]]);
        recording.clean_press(RIGHT_WIN);
        assert!(recording.candidates.is_empty());
        recording.send(&[releases[1]]);
        recording.clean_press(RIGHT_WIN);
        assert_eq!(recording.candidates, [67]);
    }
}

#[test]
fn injected_other_keys_taint_and_preheld_injected_keys_block_win() {
    for preheld in [false, true] {
        let mut recording = Recording::enabled(71);
        if preheld {
            recording.send(&[injected(LETTER_K, true), hardware(LEFT_WIN, true)]);
        } else {
            recording.send(&[hardware(LEFT_WIN, true), injected(LETTER_K, true)]);
        }
        recording.send(&[hardware(LEFT_WIN, false)]);
        recording.clean_press(RIGHT_WIN);
        assert!(recording.candidates.is_empty());
        recording.send(&[injected(LETTER_K, false)]);
        recording.clean_press(RIGHT_WIN);
        assert_eq!(recording.candidates, [71]);
    }
}

#[test]
fn even_an_unmatched_injected_other_key_release_taints_a_gesture() {
    let mut recording = Recording::enabled(73);
    recording.send(&[
        hardware(LEFT_WIN, true),
        injected(LETTER_K, false),
        hardware(LEFT_WIN, false),
    ]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [73]);
}

#[test]
fn unknown_valid_key_events_taint_but_preserve_key_balance() {
    for sequence in [
        [unknown(LEFT_WIN, true), hardware(LEFT_WIN, false)],
        [hardware(LEFT_WIN, true), unknown(LEFT_WIN, false)],
        [unknown(LEFT_WIN, true), unknown(LEFT_WIN, false)],
    ] {
        let mut recording = Recording::enabled(79);
        recording.send(&sequence);
        assert!(recording.candidates.is_empty());
        recording.clean_press(LEFT_WIN);
        assert_eq!(recording.candidates, [79]);
    }

    let mut recording = Recording::enabled(79);
    recording.send(&[
        hardware(LEFT_WIN, true),
        unknown(LETTER_K, true),
        hardware(LEFT_WIN, false),
    ]);
    recording.clean_press(RIGHT_WIN);
    assert!(recording.candidates.is_empty());
    recording.send(&[hardware(LETTER_K, false)]);
    recording.clean_press(RIGHT_WIN);
    assert_eq!(recording.candidates, [79]);
}

#[test]
fn unknown_unmatched_release_cancels_a_clean_win_candidate() {
    let mut recording = Recording::enabled(83);
    recording.send(&[
        hardware(LEFT_WIN, true),
        unknown(LETTER_K, false),
        hardware(LEFT_WIN, false),
    ]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [83]);
}

#[test]
fn untrackable_keys_fail_closed_until_an_authoritative_reset() {
    for key in [0, 256, u32::MAX] {
        for event in [
            hardware(key, true),
            injected(key, false),
            unknown(key, true),
        ] {
            let mut recording = Recording::enabled(89);
            recording.send(&[hardware(LEFT_WIN, true), event, hardware(LEFT_WIN, false)]);
            recording.clean_press(RIGHT_WIN);
            assert!(recording.candidates.is_empty());
            assert!(recording.reducer.rearm_required());
            recording.reducer.reset(true, 97, [false; 256]);
            assert!(!recording.reducer.rearm_required());
            recording.clean_press(RIGHT_WIN);
            assert_eq!(recording.candidates, [97]);
        }
    }
}

#[test]
fn reset_preheld_win_cannot_start_from_its_repeat_or_release() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut held = [false; 256];
        held[win as usize] = true;
        let mut recording = Recording::enabled(101);
        recording.reducer.reset(true, 103, held);
        recording.send(&[hardware(win, true), hardware(win, false)]);
        assert!(recording.candidates.is_empty());
        recording.clean_press(win);
        assert_eq!(recording.candidates, [103]);
    }
}

#[test]
fn reset_preheld_keys_all_must_drain_and_cannot_salvage_an_inflight_win() {
    let mut held = [false; 256];
    held[LETTER_K as usize] = true;
    held[0xa0] = true;
    let mut recording = Recording::enabled(107);
    recording.reducer.reset(true, 109, held);
    recording.send(&[
        hardware(LETTER_K, false),
        hardware(LEFT_WIN, true),
        hardware(0xa0, false),
        hardware(LEFT_WIN, false),
    ]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [109]);
}

#[test]
fn fully_held_snapshot_drains_without_balance_overflow_or_phantom_keys() {
    let mut held = [true; 256];
    held[0] = false;
    let mut recording = Recording::enabled(113);
    recording.reducer.reset(true, 127, held);
    for key in 1..=255 {
        recording.send(&[hardware(key, false)]);
    }
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [127]);
}

#[test]
fn reserved_zero_slot_in_snapshot_remains_fail_closed_until_reset() {
    let mut held = [false; 256];
    held[0] = true;
    let mut recording = Recording::enabled(131);
    recording.reducer.reset(true, 137, held);
    recording.clean_press(LEFT_WIN);
    assert!(recording.candidates.is_empty());
    recording.reducer.reset(true, 139, [false; 256]);
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [139]);
}

#[test]
fn pause_cancels_a_candidate_and_reenable_requires_a_fresh_gesture() {
    let mut recording = Recording::enabled(149);
    recording.send(&[hardware(LEFT_WIN, true)]);
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    recording.reducer.reset(false, 151, held);
    recording.send(&[hardware(LEFT_WIN, false)]);
    recording.clean_press(RIGHT_WIN);
    assert!(recording.candidates.is_empty());
    recording.reducer.reset(true, 157, [false; 256]);
    recording.clean_press(RIGHT_WIN);
    assert_eq!(recording.candidates, [157]);
}

#[test]
fn capture_pause_tracks_keys_and_does_not_replay_them_on_resume() {
    let mut recording = Recording::enabled(163);
    recording.reducer.reset(false, 167, [false; 256]);
    recording.send(&[hardware(LEFT_WIN, true), hardware(LETTER_K, true)]);
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    held[LETTER_K as usize] = true;
    recording.reducer.reset(true, 173, held);
    recording.send(&[hardware(LETTER_K, false), hardware(LEFT_WIN, false)]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [173]);
}

#[test]
fn enabled_reconfiguration_cancels_the_old_generation_mid_gesture() {
    let mut recording = Recording::enabled(179);
    recording.send(&[hardware(LEFT_WIN, true)]);
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    recording.reducer.reset(true, 181, held);
    recording.send(&[hardware(LEFT_WIN, false)]);
    assert!(recording.candidates.is_empty());
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [181]);
}

#[test]
fn reset_replaces_old_held_and_injected_state_instead_of_merging_it() {
    let mut recording = Recording::enabled(191);
    recording.send(&[hardware(LETTER_K, true), injected(RIGHT_WIN, true)]);
    recording.reducer.reset(true, 193, [false; 256]);
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [193]);
}

#[test]
fn candidate_generation_is_the_exact_owner_value_without_arithmetic() {
    for generation in [0, u64::MAX] {
        let mut recording = Recording::enabled(generation);
        recording.clean_press(LEFT_WIN);
        assert_eq!(recording.candidates, [generation]);
    }
}

#[test]
fn genuine_hardware_release_drains_unknown_origin_seed_without_triggering() {
    for key in [LEFT_WIN, RIGHT_WIN, LETTER_K] {
        let mut held = [false; 256];
        held[key as usize] = true;
        let mut recording = Recording::enabled(197);
        recording.reducer.reset(true, 199, held);
        assert!(!recording.reducer.rearm_required());
        recording.send(&[hardware(key, false)]);
        assert!(recording.candidates.is_empty());
        assert!(!recording.reducer.rearm_required());
        recording.clean_press(LEFT_WIN);
        assert_eq!(recording.candidates, [199]);
    }
}

#[test]
fn injected_release_of_unknown_origin_seed_requires_explicit_rearm() {
    // An SDK high bit could come from either genuine or preexisting injected input.
    for key in [LEFT_WIN, RIGHT_WIN, LETTER_K] {
        let mut held = [false; 256];
        held[key as usize] = true;
        let mut recording = Recording::enabled(211);
        recording.reducer.reset(true, 223, held);
        recording.send(&[injected(key, false)]);
        assert!(recording.reducer.rearm_required());
        recording.clean_press(LEFT_WIN);
        recording.send(&[hardware(key, false)]);
        recording.clean_press(RIGHT_WIN);
        assert!(recording.candidates.is_empty());
        assert!(recording.reducer.rearm_required());

        recording.reducer.reset(true, 227, [false; 256]);
        assert!(!recording.reducer.rearm_required());
        recording.send(&[hardware(LEFT_WIN, false)]);
        assert!(recording.candidates.is_empty());
        recording.send(&[hardware(LEFT_WIN, true)]);
        assert!(recording.candidates.is_empty());
        recording.send(&[hardware(LEFT_WIN, false)]);
        assert_eq!(recording.candidates, [227]);
    }
}

#[test]
fn ambiguous_real_hardware_seed_and_injected_release_never_admit_a_repeat() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut held = [false; 256];
        held[win as usize] = true;
        let mut recording = Recording::enabled(229);
        recording.reducer.reset(true, 233, held);
        recording.send(&[
            hardware(win, true),
            injected(win, false),
            hardware(win, true),
            hardware(win, false),
        ]);
        recording.clean_press(win);
        assert!(recording.candidates.is_empty());
        assert!(recording.reducer.rearm_required());
    }
}

#[test]
fn observed_injected_down_does_not_disambiguate_a_preheld_seed() {
    let mut held = [false; 256];
    held[LETTER_K as usize] = true;
    let mut recording = Recording::enabled(239);
    recording.reducer.reset(true, 241, held);
    recording.send(&[injected(LETTER_K, true), injected(LETTER_K, false)]);
    assert!(recording.reducer.rearm_required());
    recording.clean_press(LEFT_WIN);
    assert!(recording.candidates.is_empty());
}

#[test]
fn genuine_release_balances_seed_and_observed_hardware_down_together() {
    for win in [LEFT_WIN, RIGHT_WIN] {
        let mut held = [false; 256];
        held[win as usize] = true;
        let mut recording = Recording::enabled(251);
        recording.reducer.reset(true, 257, held);
        recording.send(&[hardware(win, true), hardware(win, false)]);
        assert!(recording.candidates.is_empty());
        assert!(!recording.reducer.rearm_required());
        recording.clean_press(win);
        assert_eq!(recording.candidates, [257]);
    }
}

#[test]
fn one_ambiguous_seed_release_keeps_rearm_latched_after_other_keys_drain() {
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    held[LETTER_K as usize] = true;
    let mut recording = Recording::enabled(263);
    recording.reducer.reset(true, 269, held);
    recording.send(&[
        hardware(LETTER_K, false),
        injected(LEFT_WIN, false),
        hardware(LEFT_WIN, false),
    ]);
    recording.clean_press(RIGHT_WIN);
    assert!(recording.candidates.is_empty());
    assert!(recording.reducer.rearm_required());
    recording.reducer.reset(true, 271, [false; 256]);
    recording.clean_press(RIGHT_WIN);
    assert_eq!(recording.candidates, [271]);
}

#[test]
fn paused_ambiguous_seed_release_is_reported_and_reset_clears_rearm() {
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    let mut recording = Recording::enabled(277);
    recording.reducer.reset(false, 281, held);
    recording.send(&[injected(LEFT_WIN, false)]);
    assert!(recording.reducer.rearm_required());
    recording.reducer.reset(false, 283, [false; 256]);
    assert!(!recording.reducer.rearm_required());
    recording.clean_press(LEFT_WIN);
    assert!(recording.candidates.is_empty());
    recording.reducer.reset(true, 293, [false; 256]);
    recording.clean_press(LEFT_WIN);
    assert_eq!(recording.candidates, [293]);
}

#[test]
fn unknown_flagged_seed_release_cannot_supply_hardware_balance_authority() {
    let mut held = [false; 256];
    held[LEFT_WIN as usize] = true;
    let mut recording = Recording::enabled(307);
    recording.reducer.reset(true, 311, held);
    recording.send(&[unknown(LEFT_WIN, false)]);
    assert!(recording.reducer.rearm_required());
    recording.clean_press(RIGHT_WIN);
    assert!(recording.candidates.is_empty());
}
