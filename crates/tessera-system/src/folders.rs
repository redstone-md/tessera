// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Known-folder availability and explicit opening, without portable paths.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

/// A current-user known-folder category, not a filesystem address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FolderId {
    Recent,
    Desktop,
    Downloads,
    Documents,
    Music,
    Pictures,
    Videos,
}

impl FolderId {
    /// Source presentation order; every snapshot contains exactly these rows.
    pub const ALL: [Self; 7] = [
        Self::Recent,
        Self::Desktop,
        Self::Downloads,
        Self::Documents,
        Self::Music,
        Self::Pictures,
        Self::Videos,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Recent => 0,
            Self::Desktop => 1,
            Self::Downloads => 2,
            Self::Documents => 3,
            Self::Music => 4,
            Self::Pictures => 5,
            Self::Videos => 6,
        }
    }
}

/// Provider-issued, process-local expectation; never persist or display it.
///
/// Providers validate their own private payload, provider identity and category
/// before dispatch. This projection is not a transferable global capability.
#[derive(Clone)]
pub struct FolderTarget(Arc<dyn Any + Send + Sync>);

impl FolderTarget {
    pub fn new<T: Any + Send + Sync>(target: T) -> Self {
        Self(Arc::new(target))
    }

    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl fmt::Debug for FolderTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FolderTarget(<opaque>)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderErrorKind {
    Unsupported,
    AccessDenied,
    Unavailable,
    TargetChanged,
    Busy,
    Stopped,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderError {
    pub kind: FolderErrorKind,
    pub message: String,
}

impl FolderError {
    pub fn new(kind: FolderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for FolderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FolderError {}

#[derive(Clone, Debug)]
pub enum FolderAvailability {
    Ready(FolderTarget),
    Unavailable(FolderError),
}

/// Closed seven-row projection: no arbitrary rows or duplicate categories.
#[derive(Clone, Debug)]
pub struct FolderSnapshot {
    rows: [FolderAvailability; 7],
}

impl FolderSnapshot {
    pub fn new(rows: [FolderAvailability; 7]) -> Self {
        Self { rows }
    }

    pub fn get(&self, folder: FolderId) -> &FolderAvailability {
        &self.rows[folder.index()]
    }
}

pub type FolderReadCompletion =
    Box<dyn FnOnce(Result<FolderSnapshot, FolderError>) + Send + 'static>;
pub type FolderOpenCompletion = Box<dyn FnOnce(Result<(), FolderError>) + Send + 'static>;

/// Prompt, nonblocking request acceptance for explicit folder actions.
///
/// `Ok` accepts exactly one completion; immediate `Err` guarantees none.
/// Completions may run inline or on a worker. UI callers post and generation-
/// gate them. Opening revalidates the provider-issued expectation against a
/// fresh current-user resolution before shell dispatch. Success acknowledges
/// dispatch acceptance, not that an Explorer window became visible.
pub trait FolderHost: Send + Sync + 'static {
    fn read(&self, completion: FolderReadCompletion) -> Result<(), FolderError>;
    fn open(
        &self,
        folder: FolderId,
        expected: FolderTarget,
        completion: FolderOpenCompletion,
    ) -> Result<(), FolderError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_keeps_exact_source_order() {
        let snapshot = FolderSnapshot::new(std::array::from_fn(|index| {
            FolderAvailability::Ready(FolderTarget::new(index))
        }));
        for (index, folder) in FolderId::ALL.into_iter().enumerate() {
            let FolderAvailability::Ready(target) = snapshot.get(folder) else {
                panic!("ordered row must be ready");
            };
            assert_eq!(target.get::<usize>(), Some(&index));
        }
    }

    #[test]
    fn target_clone_preserves_private_type_without_debug_disclosure() {
        struct PrivateIdentity(&'static str);
        let target = FolderTarget::new(PrivateIdentity("private native address"));
        let cloned = target.clone();
        assert_eq!(
            cloned.get::<PrivateIdentity>().unwrap().0,
            "private native address"
        );
        assert!(cloned.get::<String>().is_none());
        assert_eq!(format!("{target:?}"), "FolderTarget(<opaque>)");
    }
}
