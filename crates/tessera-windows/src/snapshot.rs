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
    #[cfg(any(windows, test))]
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
    visibility_windows: Option<Vec<ObservedWindow>>,
    foreground_window_id: Option<WindowId>,
    foreground_interactable: Option<bool>,
    application_identities: std::collections::HashMap<WindowId, String>,
}

impl DesktopSnapshot {
    #[cfg(any(windows, test))]
    pub(crate) fn new(
        monitors: Vec<ObservedMonitor>,
        windows: Vec<ObservedWindow>,
        warnings: Vec<ObservationWarning>,
    ) -> Self {
        Self {
            monitors,
            windows,
            warnings,
            visibility_windows: None,
            foreground_window_id: None,
            foreground_interactable: None,
            application_identities: Default::default(),
        }
    }

    #[cfg(windows)]
    pub(crate) fn with_application_identities(
        mut self,
        identities: std::collections::HashMap<WindowId, String>,
    ) -> Self {
        self.application_identities = identities;
        self
    }

    /// Proven application identity for grouping only, never effect authority.
    /// AUMIDs use `aumid:`, executable identities use `exe:`; absence is honest.
    pub fn window_application_identity(&self, id: WindowId) -> Option<&str> {
        self.application_identities.get(&id).map(String::as_str)
    }

    /// Derives uncapped visibility facts from the same observation pass.
    /// Authorization deliberately reuses activation admission, not a looser
    /// shell-specific filter. Unknown reads never become an empty clear desktop.
    #[cfg(any(windows, test))]
    pub(crate) fn with_visibility_facts(mut self, foreground: Option<WindowId>) -> Self {
        self.foreground_window_id = foreground;
        if !self.warnings.is_empty() {
            return self;
        }
        let eligible: Vec<_> = self
            .windows
            .iter()
            .filter(|window| crate::ActivationTarget::from_window(window).is_some())
            .collect();
        if eligible.iter().any(|window| {
            window.cloaked().is_none() || (!window.minimized() && window.monitor_id().is_none())
        }) {
            return self;
        }
        self.foreground_interactable =
            foreground.map(|id| eligible.iter().any(|window| window.id() == id));
        self.visibility_windows = Some(
            eligible
                .into_iter()
                .filter(|window| !window.minimized())
                .cloned()
                .collect(),
        );
        self
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

    /// Complete, uncapped eligible nonminimized windows in physical pixels.
    /// `None` means a window read or monitor assignment was incomplete.
    pub fn visibility_windows(&self) -> Option<&[ObservedWindow]> {
        self.visibility_windows.as_deref()
    }

    /// Foreground identity sampled immediately before this pass's enumeration.
    pub fn foreground_window_id(&self) -> Option<WindowId> {
        self.foreground_window_id
    }

    /// Whether that foreground belongs to the complete admitted window set.
    /// A null foreground or incomplete pass remains unknown, not false.
    pub fn foreground_interactable(&self) -> Option<bool> {
        self.foreground_interactable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: u64) -> ObservedWindow {
        ObservedWindow {
            id: WindowId::new(id),
            process_id: std::process::id().wrapping_add(1),
            title: format!("Application {id}"),
            class_name: "Application".into(),
            bounds: Rect::new(-1920, -100, 100, 100).unwrap(),
            monitor_id: Some(MonitorId::new(1)),
            minimized: false,
            maximized: false,
            cloaked: Some(false),
            tool_window: false,
            owned: false,
            covers_monitor: false,
        }
    }

    fn observe(windows: Vec<ObservedWindow>, foreground: Option<WindowId>) -> DesktopSnapshot {
        DesktopSnapshot::new(Vec::new(), windows, Vec::new()).with_visibility_facts(foreground)
    }

    #[test]
    fn visibility_facts_keep_all_windows_before_any_ui_cap() {
        let mut windows: Vec<_> = (1..=200).map(window).collect();
        windows[199].bounds = Rect::new(-2, -100, 100, 100).unwrap();
        windows[199].monitor_id = Some(MonitorId::new(2));
        let snapshot = observe(windows, Some(WindowId::new(1)));
        let facts = snapshot.visibility_windows().unwrap();
        assert_eq!(facts.len(), 200);
        assert_eq!(facts[199].bounds().x(), -2);
        assert_eq!(facts[199].monitor_id(), Some(MonitorId::new(2)));
        assert_eq!(facts[0].bounds().x(), -1920);
        assert_eq!(snapshot.foreground_interactable(), Some(true));
        assert_eq!(snapshot.foreground_window_id(), Some(WindowId::new(1)));
    }

    #[test]
    fn minimized_and_unadmitted_windows_preserve_records_but_not_overlap_facts() {
        let mut minimized = window(1);
        minimized.minimized = true;
        minimized.monitor_id = None;
        let mut tool = window(2);
        tool.tool_window = true;
        let mut desktop = window(3);
        desktop.class_name = "WorkerW".into();
        let snapshot = observe(
            vec![minimized, tool, desktop, window(4)],
            Some(WindowId::new(3)),
        );
        assert_eq!(snapshot.windows().len(), 4);
        let facts = snapshot.visibility_windows().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].id(), WindowId::new(4));
        assert_eq!(snapshot.foreground_interactable(), Some(false));
    }

    #[test]
    fn incomplete_cloak_or_monitor_read_stays_unknown() {
        for missing_cloak in [false, true] {
            let mut unknown = window(1);
            if missing_cloak {
                unknown.cloaked = None;
            } else {
                unknown.monitor_id = None;
            }
            let snapshot = observe(vec![unknown], Some(WindowId::new(1)));
            assert!(snapshot.visibility_windows().is_none());
            assert_eq!(snapshot.foreground_interactable(), None);
            assert_eq!(snapshot.windows().len(), 1);
        }
    }

    #[test]
    fn null_foreground_is_unknown_not_noninteractable() {
        let snapshot = observe(vec![window(1)], None);
        assert!(snapshot.visibility_windows().is_some());
        assert_eq!(snapshot.foreground_interactable(), None);
    }

    #[cfg(windows)]
    #[test]
    fn failed_or_invalid_rectangle_never_becomes_empty_complete_facts() {
        for operation in ["GetWindowRect", "ValidateWindowRect"] {
            let warning = ObservationWarning::new(WindowId::new(2), operation, 13);
            let snapshot = DesktopSnapshot::new(Vec::new(), vec![window(1)], vec![warning])
                .with_visibility_facts(Some(WindowId::new(1)));
            assert!(snapshot.visibility_windows().is_none());
            assert_eq!(snapshot.foreground_interactable(), None);
            assert_eq!(snapshot.warnings().len(), 1);
        }
    }
}
