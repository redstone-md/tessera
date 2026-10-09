// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Narrow private child spawn/join, NOT another worker, queue or service.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread::{self, JoinHandle};

use super::{Outcome, PowerError};

pub(super) type Job = Box<dyn FnOnce() -> Outcome + Send + 'static>;
pub(super) type Spawn = fn(Job) -> std::io::Result<JoinHandle<Outcome>>;
type PanicPayload = Box<dyn Any + Send + 'static>;

pub(super) fn spawn(job: Job) -> std::io::Result<JoinHandle<Outcome>> {
    thread::Builder::new()
        .name("tessera-power-operation".into())
        .spawn(job)
}

/// Successful join (including Err(child panic)) proves actual retirement. A
/// panic OF join is distinct: Rust/Windows may have failed its native wait.
enum JoinEvidence<T> {
    Retired(thread::Result<T>),
    Unproven(PanicPayload),
}

fn join_evidence<T>(outcome: thread::Result<thread::Result<T>>) -> JoinEvidence<T> {
    match outcome {
        Ok(result) => JoinEvidence::Retired(result),
        Err(payload) => JoinEvidence::Unproven(payload),
    }
}

fn discard_panic(payload: PanicPayload) {
    // A child panic payload can have custom Drop. Dispose on the NEUTRAL outer
    // only, containing even a panic from that Drop without exposing its text.
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
        std::mem::forget(payload);
    }
}

fn quarantine() -> ! {
    // No retirement proof means no normal return into worker.catch_unwind:
    // its flight and completion remain owned by this parked BACKGROUND worker.
    // Spurious unparks never release it. No process kill or forced thread exit.
    loop {
        thread::park();
    }
}

pub(super) fn run(job: Job, spawn_child: Spawn) -> Outcome {
    let child = match catch_unwind(AssertUnwindSafe(|| spawn_child(job))) {
        Ok(Ok(child)) => child,
        Ok(Err(_)) => return Err(PowerError::Unavailable),
        Err(payload) => {
            // Private spawn failure must neither start nor retain the job.
            discard_panic(payload);
            return Err(PowerError::Unavailable);
        }
    };
    match join_evidence(catch_unwind(AssertUnwindSafe(|| child.join()))) {
        JoinEvidence::Retired(Ok(result)) => result,
        JoinEvidence::Retired(Err(payload)) => {
            discard_panic(payload);
            Err(PowerError::Unavailable)
        }
        JoinEvidence::Unproven(payload) => {
            // Do not run an arbitrary panic-payload Drop that could unwind back
            // to the worker's normal gate-release/callback path without proof.
            std::mem::forget(payload);
            quarantine()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_evidence_quarantines_only_unproven_wait_not_retired_child_panic() {
        let success: thread::Result<thread::Result<Outcome>> =
            Ok(Ok(Ok(super::super::PowerRequestAccepted)));
        assert!(matches!(
            join_evidence(success),
            JoinEvidence::Retired(Ok(Ok(_)))
        ));
        let child_panic: thread::Result<thread::Result<Outcome>> = Ok(Err(Box::new("child")));
        assert!(matches!(
            join_evidence(child_panic),
            JoinEvidence::Retired(Err(_))
        ));
        let wait_panic: thread::Result<thread::Result<Outcome>> = Err(Box::new("wait"));
        assert!(matches!(
            join_evidence(wait_panic),
            JoinEvidence::Unproven(_)
        ));
        // Deliberately pure: never start an immortal quarantine thread to test it.
    }
}
