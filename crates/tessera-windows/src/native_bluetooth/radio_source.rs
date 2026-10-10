// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One source watch on the radio owner's apartment; events only revoke authority.
use super::winrt::native_error;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tessera_system::bluetooth::{BluetoothError, BluetoothErrorKind};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationUpdate, DeviceWatcher, DeviceWatcherStatus,
};
use windows::Foundation::TypedEventHandler;
use windows::core::{HSTRING, IInspectable};

pub(super) fn invalidate(revision: &AtomicU64) {
    // MAX is a permanent tombstone, never a wrapped/reusable source revision.
    let _ = revision.fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
        Some(value.saturating_add(1))
    });
}

#[derive(Default)]
struct Enumeration {
    ready: bool,
    failed: bool,
}

pub(super) struct SourceRevision {
    revision: AtomicU64,
    live: AtomicBool,
    enumeration: Mutex<Enumeration>,
    wake: Condvar,
}

impl SourceRevision {
    fn new() -> Self {
        Self {
            revision: AtomicU64::new(0),
            live: AtomicBool::new(true),
            enumeration: Mutex::default(),
            wake: Condvar::new(),
        }
    }

    fn changed(&self) {
        invalidate(&self.revision);
    }

    fn failed(&self) {
        self.live.store(false, Ordering::Release);
        self.changed();
        self.enumeration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed = true;
        self.wake.notify_all();
    }

    pub(super) fn current(&self, captured: u64) -> bool {
        captured != u64::MAX
            && self.live.load(Ordering::Acquire)
            && self.revision.load(Ordering::Acquire) == captured
    }

    fn snapshot(&self) -> Result<u64, BluetoothError> {
        let revision = self.revision.load(Ordering::Acquire);
        if self.current(revision) {
            Ok(revision)
        } else {
            Err(unavailable())
        }
    }
}

pub(super) struct SourceWatcher {
    watcher: DeviceWatcher,
    pub(super) source: Arc<SourceRevision>,
    added: Option<i64>,
    updated: Option<i64>,
    removed: Option<i64>,
    enumerated: Option<i64>,
    stopped: Option<i64>,
    started: bool,
}

impl SourceWatcher {
    pub(super) fn new(selector: &HSTRING) -> Result<Self, BluetoothError> {
        let watcher = DeviceInformation::CreateWatcherAqsFilter(selector).map_err(|error| {
            native_error(error, "Bluetooth radio source watch could not be created")
        })?;
        let mut owned = Self {
            watcher,
            source: Arc::new(SourceRevision::new()),
            added: None,
            updated: None,
            removed: None,
            enumerated: None,
            stopped: None,
            started: false,
        };
        let source = owned.source.clone();
        owned.added = Some(
            owned
                .watcher
                .Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
                    move |_, _| {
                        source.changed();
                        Ok(())
                    },
                ))
                .map_err(|error| {
                    native_error(error, "Bluetooth radio arrival subscription failed")
                })?,
        );
        let source = owned.source.clone();
        owned.updated = Some(
            owned
                .watcher
                .Updated(
                    &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
                        move |_, _| {
                            source.changed();
                            Ok(())
                        },
                    ),
                )
                .map_err(|error| {
                    native_error(error, "Bluetooth radio update subscription failed")
                })?,
        );
        let source = owned.source.clone();
        owned.removed = Some(
            owned
                .watcher
                .Removed(
                    &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
                        move |_, _| {
                            source.changed();
                            Ok(())
                        },
                    ),
                )
                .map_err(|error| {
                    native_error(error, "Bluetooth radio removal subscription failed")
                })?,
        );
        let source = owned.source.clone();
        owned.enumerated = Some(
            owned
                .watcher
                .EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(
                    move |_, _| {
                        source
                            .enumeration
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .ready = true;
                        source.wake.notify_all();
                        Ok(())
                    },
                ))
                .map_err(|error| {
                    native_error(
                        error,
                        "Bluetooth radio source readiness subscription failed",
                    )
                })?,
        );
        let source = owned.source.clone();
        owned.stopped = Some(
            owned
                .watcher
                .Stopped(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(
                    move |_, _| {
                        source.failed();
                        Ok(())
                    },
                ))
                .map_err(|error| {
                    native_error(
                        error,
                        "Bluetooth radio source retirement subscription failed",
                    )
                })?,
        );
        owned.started = true;
        owned
            .watcher
            .Start()
            .map_err(|error| native_error(error, "Bluetooth radio source watch could not start"))?;
        // Initial Added events are not post-capture incarnations. Await the
        // documented enumeration barrier on this worker before capturing source.
        let enumeration = owned
            .source
            .enumeration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let enumeration = owned
            .source
            .wake
            .wait_while(enumeration, |state| !state.ready && !state.failed)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if enumeration.failed {
            return Err(unavailable());
        }
        drop(enumeration);
        owned.capture()?;
        Ok(owned)
    }

    pub(super) fn capture(&self) -> Result<u64, BluetoothError> {
        let revision = self.source.snapshot()?;
        self.validate(revision)?;
        Ok(revision)
    }

    pub(super) fn validate(&self, captured: u64) -> Result<(), BluetoothError> {
        let status = self.watcher.Status().map_err(|error| {
            self.source.failed();
            native_error(error, "Bluetooth radio source watch status is unavailable")
        })?;
        if status != DeviceWatcherStatus::EnumerationCompleted {
            self.source.failed();
            return Err(unavailable());
        }
        if self.source.current(captured) {
            Ok(())
        } else {
            Err(unavailable())
        }
    }
}

impl Drop for SourceWatcher {
    fn drop(&mut self) {
        self.source.failed();
        if self.started {
            let _ = self.watcher.Stop();
        }
        // Stop is asynchronous. Unsubscribe instead of waiting/joining; any
        // already-running callback owns only an inert, permanently retired stamp.
        if let Some(token) = self.added.take() {
            let _ = self.watcher.RemoveAdded(token);
        }
        if let Some(token) = self.updated.take() {
            let _ = self.watcher.RemoveUpdated(token);
        }
        if let Some(token) = self.removed.take() {
            let _ = self.watcher.RemoveRemoved(token);
        }
        if let Some(token) = self.enumerated.take() {
            let _ = self.watcher.RemoveEnumerationCompleted(token);
        }
        if let Some(token) = self.stopped.take() {
            let _ = self.watcher.RemoveStopped(token);
        }
    }
}

fn unavailable() -> BluetoothError {
    BluetoothError::new(
        BluetoothErrorKind::Unavailable,
        "Bluetooth radio source watch changed, failed, retired, or exhausted. Refresh before another request.",
    )
}
