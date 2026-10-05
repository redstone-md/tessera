// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Encapsulated snapshot and record types. Field storage is private; all
//! access goes through getters. Portable (no OS types in the public model).

use crate::error::ObservationWarning;
use tessera_core::{Rect, WindowId};

/// Opaque identifier for one monitor within a snapshot. Value-semantic copy
/// handle; the raw `HMONITOR` bits are meaningful only for that snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonitorId(u64);

impl MonitorId {
    #[cfg(windows)]
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn value(self) -> u64 {
        self.0
    }
}

/// One physical monitor.
#[derive(Debug, Clone)]
pub struct ObservedMonitor {
    id: MonitorId,
    device_name: String,
    bounds: Rect,
    work_area: Rect,
    primary: bool,
}

impl ObservedMonitor {
    #[cfg(windows)]
    pub(crate) fn new(
        id: MonitorId,
        device_name: String,
        bounds: Rect,
        work_area: Rect,
        primary: bool,
    ) -> Self {
        Self {
            id,
            device_name,
            bounds,
            work_area,
            primary,
        }
    }

    pub fn id(&self) -> MonitorId {
        self.id
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn work_area(&self) -> Rect {
        self.work_area
    }

    pub fn primary(&self) -> bool {
        self.primary
    }
}

/// One observed top-level desktop window.
#[derive(Debug, Clone)]
pub struct ObservedWindow {
    pub(crate) id: WindowId,
    pub(crate) process_id: u32,
    pub(crate) title: String,
    pub(crate) class_name: String,
    pub(crate) bounds: Rect,
    pub(crate) monitor_id: Option<MonitorId>,
    pub(crate) minimized: bool,
    pub(crate) maximized: bool,
    pub(crate) cloaked: Option<bool>,
    pub(crate) tool_window: bool,
    pub(crate) owned: bool,
    pub(crate) covers_monitor: bool,
}

impl ObservedWindow {
    pub fn id(&self) -> WindowId {
        self.id
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn class_name(&self) -> &str {
        &self.class_name
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    pub fn monitor_id(&self) -> Option<MonitorId> {
        self.monitor_id
    }

    pub fn minimized(&self) -> bool {
        self.minimized
    }

    pub fn maximized(&self) -> bool {
        self.maximized
    }

    /// `Some(true/false)` when DWM cloaking was readable, `None` on failure.
    pub fn cloaked(&self) -> Option<bool> {
        self.cloaked
    }

    pub fn tool_window(&self) -> bool {
        self.tool_window
    }

    pub fn owned(&self) -> bool {
        self.owned
    }

    /// Conservative geometric hint only; not fullscreen truth. `false` when
    /// the window is minimized.
    pub fn covers_monitor(&self) -> bool {
        self.covers_monitor
    }
}

/// One read-only observation pass. Objects may change during collection.
#[derive(Debug, Clone)]
pub struct DesktopSnapshot {
    monitors: Vec<ObservedMonitor>,
    windows: Vec<ObservedWindow>,
    warnings: Vec<ObservationWarning>,
}

impl DesktopSnapshot {
    #[cfg(windows)]
    pub(crate) fn new(
        monitors: Vec<ObservedMonitor>,
        windows: Vec<ObservedWindow>,
        warnings: Vec<ObservationWarning>,
    ) -> Self {
        Self {
            monitors,
            windows,
            warnings,
        }
    }

    pub fn monitors(&self) -> &[ObservedMonitor] {
        &self.monitors
    }

    pub fn windows(&self) -> &[ObservedWindow] {
        &self.windows
    }

    pub fn warnings(&self) -> &[ObservationWarning] {
        &self.warnings
    }
}
