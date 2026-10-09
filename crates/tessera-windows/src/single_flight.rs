// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Private admission lifetime shared by display queries and Power requests.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Clone-shared admission only; no queue, lock held across work, or cancellation.
#[derive(Clone, Default)]
pub(crate) struct FlightGate {
    busy: Arc<AtomicBool>,
}

impl FlightGate {
    pub(crate) fn try_enter(&self) -> Option<Flight> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        Some(Flight {
            busy: Arc::clone(&self.busy),
        })
    }
}

/// Releasing this owner admits the next request, including callback reentry.
pub(crate) struct Flight {
    busy: Arc<AtomicBool>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_gates_share_one_flight_and_default_gates_are_independent() {
        let gate = FlightGate::default();
        let clone = gate.clone();
        let independent = FlightGate::default();
        let flight = gate.try_enter().unwrap();
        assert!(gate.try_enter().is_none());
        assert!(clone.try_enter().is_none());
        assert!(independent.try_enter().is_some());
        drop(flight);
        let next = clone.try_enter().unwrap();
        assert!(gate.try_enter().is_none());
        drop(next);
        assert!(gate.try_enter().is_some());
    }

    #[test]
    fn unwinding_releases_admission_without_poisoning() {
        let gate = FlightGate::default();
        let result = std::panic::catch_unwind(|| {
            let _flight = gate.try_enter().unwrap();
            panic!("recorded flight unwind");
        });
        assert!(result.is_err());
        assert!(gate.try_enter().is_some());
    }
}
