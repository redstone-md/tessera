// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit, native-confirmed aggregate mutation, separate from observation.

use crate::dock_utilities::DockUtilityError;

/// Records the exact native return, not consent, cancellation, deletion or count.
/// Even a negative HRESULT is an outcome when the native operation returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecycleBinEmptyOutcome {
    pub native_hresult: i32,
}

pub type RecycleBinEmptyCompletion =
    Box<dyn FnOnce(Result<RecycleBinEmptyOutcome, DockUtilityError>) + Send + 'static>;

/// Prompt admission of one explicit intent, with no queued mutation or retry.
///
/// Accepted `Ok` transfers exactly one completion, possibly inline/reentrant;
/// immediate `Err` transfers none. Setup/driver failures use `Err`, while native
/// HRESULTs remain unmodified outcomes. Reconcile with a fresh aggregate read
/// after every terminal return: this interface never proves the bin is empty.
/// Native confirmation/cleanup may stall indefinitely; dropping a host retires
/// no accepted operation and must not join the native owner thread.
pub trait RecycleBinMutationHost: Send + Sync + 'static {
    fn empty(&self, completion: RecycleBinEmptyCompletion) -> Result<(), DockUtilityError>;
}
