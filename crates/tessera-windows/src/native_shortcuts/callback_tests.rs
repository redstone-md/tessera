// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! In-memory callback reducer fixtures only: no hook is installed or invoked,
//! and no SDK function (including state/cursor reads) is called.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};

fn context(generation: u64) -> HookContext {
    let gate = Arc::new(Gate {
        generation: AtomicU64::new(generation),
        listening: AtomicBool::new(true),
        closed: AtomicBool::new(false),
        retirement: AtomicU64::new(0),
        events: AtomicUsize::new(0),
    });
    let mut reducer = BareWinReducer::new();
    reducer.reset(true, generation, [false; 256]);
    HookContext {
        gate,
        reducer,
        enabled: true,
        faulted: false,
        ring: [None; RING_CAPACITY],
        head: 0,
        length: 0,
    }
}

fn key(context: &mut HookContext, key: u32, down: bool, extra_flags: u32) -> bool {
    context.key(
        KBDLLHOOKSTRUCT {
            vkCode: key,
            flags: extra_flags | if down { 0 } else { LLKHF_UP },
            ..KBDLLHOOKSTRUCT::default()
        },
        if down { WM_KEYDOWN } else { WM_KEYUP },
    )
}

#[test]
fn only_clean_release_reserves_one_bounded_relay_slot() {
    let mut context = context(7);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(key(&mut context, 0x5b, false, 0));
    assert_eq!(context.gate.events.load(Ordering::Acquire), 1);
    assert_eq!(
        context.pop(),
        Some(RawTrigger {
            generation: 7,
            action: ShortcutAction::ToggleLauncher,
            reserved: true
        })
    );
    assert!(!key(&mut context, 0x5b, false, 0));
}

#[test]
fn full_relay_never_suppresses_and_next_fresh_gesture_can_recover() {
    let mut context = context(9);
    context.gate.events.store(RING_CAPACITY, Ordering::Release);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    assert_eq!(context.length, 0);
    context.gate.events.store(0, Ordering::Release);
    assert!(!key(&mut context, 0x5c, true, 0));
    assert!(key(&mut context, 0x5c, false, 0));
    context.clear();
    assert_eq!(context.gate.events.load(Ordering::Acquire), 0);
}

#[test]
fn reconfigure_or_listener_retirement_between_down_up_is_passthrough() {
    for retire_listener in [false, true] {
        let mut context = context(12);
        assert!(!key(&mut context, 0x5b, true, 0));
        if retire_listener {
            context.gate.listening.store(false, Ordering::Release);
        } else {
            context.gate.generation.store(13, Ordering::Release);
        }
        assert!(!key(&mut context, 0x5b, false, 0));
        assert_eq!(context.gate.events.load(Ordering::Acquire), 0);
        assert_eq!(context.length, 0);
    }
}

#[test]
fn injected_lower_integrity_unknown_flags_alt_and_flag_mismatch_never_suppress() {
    for flags in [LLKHF_INJECTED, LLKHF_LOWER_IL_INJECTED, LLKHF_ALTDOWN, 0x40] {
        let mut context = context(1);
        assert!(!key(&mut context, 0x5b, true, flags));
        assert!(!key(&mut context, 0x5b, false, flags));
        assert_eq!(context.gate.events.load(Ordering::Acquire), 0);
    }
    let mut context = context(1);
    assert!(!context.key(
        KBDLLHOOKSTRUCT {
            vkCode: 0x5b,
            flags: LLKHF_UP,
            ..Default::default()
        },
        WM_KEYDOWN
    ));
    assert!(!key(&mut context, 0x5b, false, 0));
}

#[test]
fn cleanup_cancels_queued_reservations_before_native_retirement() {
    let mut context = context(4);
    for _ in 0..RING_CAPACITY {
        assert!(!key(&mut context, 0x5b, true, 0));
        assert!(key(&mut context, 0x5b, false, 0));
    }
    assert_eq!(context.length, RING_CAPACITY);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    context.clear();
    assert!(!context.enabled);
    assert_eq!(context.length, 0);
    assert_eq!(context.gate.events.load(Ordering::Acquire), 0);
}

#[test]
fn owner_snapshot_has_only_balanced_keyboard_identities() {
    for key in [0, 1, 2, 4, 5, 6, 0x10, 0x11, 0x12, 256] {
        assert!(!snapshot_key(key));
    }
    // Cancel is a keyboard key, not a mouse button despite its adjacent VK.
    for key in [3, 0x41, 0x5b, 0x5c, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5] {
        assert!(snapshot_key(key));
    }
}

#[test]
fn preheld_sided_modifiers_balance_generic_payloads_before_next_clean_win() {
    for (side, generic, scan, flags) in [
        (0xa0, 0x10, 0x2a, 0),
        (0xa1, 0x10, 0x36, 0),
        (0xa2, 0x11, 0x1d, 0),
        (0xa3, 0x11, 0x1d, LLKHF_EXTENDED),
        (0xa4, 0x12, 0x38, 0),
        (0xa5, 0x12, 0x38, LLKHF_EXTENDED),
    ] {
        let mut context = context(8);
        let mut held = [false; 256];
        held[side] = true;
        context.reducer.reset(true, 8, held);
        assert!(!key(&mut context, 0x5b, true, 0));
        assert!(!key(&mut context, 0x5b, false, 0));
        assert!(!context.key(
            KBDLLHOOKSTRUCT {
                vkCode: generic,
                scanCode: scan,
                flags: flags | LLKHF_UP,
                ..Default::default()
            },
            WM_KEYUP
        ));
        assert!(!key(&mut context, 0x5b, true, 0));
        assert!(key(&mut context, 0x5b, false, 0));
        context.clear();
    }
}

#[test]
fn unrecognized_generic_shift_side_requires_authoritative_rearm() {
    let mut context = context(3);
    assert!(!key(&mut context, 0x10, true, 0));
    assert!(!key(&mut context, 0x10, false, 0));
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    context.reducer.reset(true, 4, [false; 256]);
    context.gate.generation.store(4, Ordering::Release);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(key(&mut context, 0x5b, false, 0));
    context.clear();
}

#[test]
fn unknown_seed_injected_release_requests_owner_rearm_without_fake_hardware_balance() {
    let mut context = context(5);
    let mut held = [false; 256];
    held[0x41] = true;
    context.reset_snapshot(5, held, true);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    assert!(!key(&mut context, 0x41, false, LLKHF_INJECTED));
    assert_eq!(context.next_event(), Ok(Some(BackendEvent::RearmRequired)));
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    assert_eq!(context.gate.events.load(Ordering::Acquire), 0);

    // Owner retirement and an explicit new, neutral configure are distinct.
    context.clear();
    assert_eq!(context.next_event(), Ok(None));
    context.gate.generation.store(6, Ordering::Release);
    context.reset_snapshot(6, [false; 256], true);
    assert!(!key(&mut context, 0x5b, false, 0));
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(key(&mut context, 0x5b, false, 0));
    assert_eq!(
        context.next_event(),
        Ok(Some(BackendEvent::Triggered(RawTrigger {
            generation: 6,
            action: ShortcutAction::ToggleLauncher,
            reserved: true,
        })))
    );
}

#[test]
fn observed_physical_hold_cannot_be_released_by_injected_keyup() {
    let mut context = context(4);
    assert!(!key(&mut context, 0x41, true, 0));
    assert!(!key(&mut context, 0x41, false, LLKHF_INJECTED));
    assert_eq!(context.next_event(), Ok(None));
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(!key(&mut context, 0x5b, false, 0));
    assert!(!key(&mut context, 0x41, false, 0));
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(key(&mut context, 0x5b, false, 0));
    context.clear();
}

#[test]
fn gesture_must_start_after_subscriber_admission_not_just_finish_after_it() {
    let mut context = context(3);
    context.gate.listening.store(false, Ordering::Release);
    assert!(!key(&mut context, 0x5b, true, 0));
    context.gate.listening.store(true, Ordering::Release);
    assert!(!key(&mut context, 0x5b, false, 0));
    assert_eq!(context.gate.events.load(Ordering::Acquire), 0);
    assert!(!key(&mut context, 0x5b, true, 0));
    assert!(key(&mut context, 0x5b, false, 0));
    context.clear();
}
