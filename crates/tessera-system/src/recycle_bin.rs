// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Fixed Recycle Bin information, opening and invalidation, without deletion authority.

use std::sync::Arc;

use crate::dock_utilities::{DockUtilityCompletion, DockUtilityError, DockUtilityErrorKind};

/// An aggregate snapshot, not an inventory or authorization to delete its items.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecycleBinInfo {
    pub item_count: u64,
    pub size_in_bytes: u64,
}

impl RecycleBinInfo {
    pub fn is_empty(&self) -> bool {
        self.item_count == 0
    }
}

pub type RecycleBinReadCompletion =
    Box<dyn FnOnce(Result<RecycleBinInfo, DockUtilityError>) + Send + 'static>;
pub type RecycleBinCompletion = DockUtilityCompletion;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecycleBinWatchEvent {
    Invalidated,
    Unavailable(DockUtilityErrorKind),
}

pub type RecycleBinWatchCallback = Arc<dyn Fn(RecycleBinWatchEvent) + Send + Sync + 'static>;
pub type RecycleBinWatchCompletion = Box<dyn FnOnce(Result<(), DockUtilityError>) + Send + 'static>;

/// Dropping retires future callback admission and requests asynchronous cleanup.
/// It never joins the owner thread; an already admitted callback may finish.
pub trait RecycleBinWatchGuard: Send + 'static {}

/// Prompt acceptance of operations on one native-owned, fixed namespace.
///
/// Read/open `Ok` transfers exactly one completion; immediate `Err` transfers
/// none. Completion can be inline/reentrant. Open success acknowledges dispatch,
/// not a visible Explorer window or focus. There is deliberately no Empty method.
///
/// Watch `Ok` accepts startup, not installed readiness, and transfers exactly one
/// readiness completion. An early guard drop completes readiness with Stopped.
/// Events start only after a successful ready callback returns; notifications
/// during startup coalesce into one dirty hint. Startup failure uses ready Err;
/// later failure uses Unavailable. Read/open remain independent of watch success.
/// Native calls can stall: no execution timeout, cancellation or retry is promised.
pub trait RecycleBinHost: Send + Sync + 'static {
    fn read(&self, completion: RecycleBinReadCompletion) -> Result<(), DockUtilityError>;
    fn open(&self, completion: RecycleBinCompletion) -> Result<(), DockUtilityError>;
    fn watch(
        &self,
        events: RecycleBinWatchCallback,
        ready: RecycleBinWatchCompletion,
    ) -> Result<Box<dyn RecycleBinWatchGuard>, DockUtilityError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullness_depends_on_count_not_size() {
        let nonempty = RecycleBinInfo {
            item_count: 1,
            size_in_bytes: 0,
        };
        assert!(!nonempty.is_empty());
        assert_eq!(nonempty.clone(), nonempty);
        assert!(
            RecycleBinInfo {
                item_count: 0,
                size_in_bytes: u64::MAX,
            }
            .is_empty()
        );
    }
}
