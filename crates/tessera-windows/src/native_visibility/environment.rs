// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use tessera_system::visibility::PointerEnvironment;

pub(super) const DEVICE_CAP: usize = 32;
pub(super) const TOUCH_BITS: u32 = 0x03;
pub(super) const INTEGRATED_PEN: i32 = 1;
pub(super) const EXTERNAL_PEN: i32 = 2;
pub(super) const TOUCH: i32 = 3;
pub(super) const TOUCH_PAD: i32 = 4;

/// Startup-only read seam. The list carries only device kinds, never native
/// handles, product strings, monitor identity or caller-controlled registrations.
pub(super) trait EnvironmentCalls {
    fn digitizer_bits(&self) -> u32;
    fn device_count(&self) -> Result<u32, u32>;
    fn device_types(&self, kinds: &mut [i32]) -> Result<u32, u32>;
}

/// A positive system-wide touch capability conservatively protects all bars;
/// it is not a claim about touch-primary mode or an individual physical monitor.
/// Zero metric values are ambiguous. Only a successful bounded count/list with
/// known non-touch kinds may establish false. Races and failures stay unknown.
pub(super) fn pointer_environment(calls: &impl EnvironmentCalls) -> PointerEnvironment {
    let touch_capable = sample(calls);
    PointerEnvironment { touch_capable }
}

fn sample(calls: &impl EnvironmentCalls) -> Option<bool> {
    if calls.digitizer_bits() & TOUCH_BITS != 0 {
        // NID_READY is intentionally not required: installed touch is enough
        // to inhibit hiding, including temporarily unready device/service state.
        return Some(true);
    }
    let expected = usize::try_from(calls.device_count().ok()?).ok()?;
    if expected > DEVICE_CAP {
        return None;
    }
    let mut kinds = [0; DEVICE_CAP];
    // Even a zero count gets a second successful count through the empty-list
    // adapter. A changed count never becomes a fabricated no-touch result.
    let actual = usize::try_from(calls.device_types(&mut kinds[..expected]).ok()?).ok()?;
    if actual != expected {
        return None;
    }
    let kinds = &kinds[..actual];
    if kinds.contains(&TOUCH) {
        return Some(true);
    }
    if kinds
        .iter()
        .all(|kind| matches!(*kind, INTEGRATED_PEN | EXTERNAL_PEN | TOUCH_PAD))
    {
        Some(false)
    } else {
        None
    }
}
