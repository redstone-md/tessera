// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Portable records and validation used by the native presentation adapters.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapturedPlacement {
    pub(crate) show_command: u32,
}

impl CapturedPlacement {
    pub(crate) const SW_HIDE: u32 = 0;
    pub(crate) const SW_SHOWNORMAL: u32 = 1;

    /// GetWindowPlacement documents normal, minimized and maximized show states.
    /// None of these tells us IsWindowVisible: that is captured separately.
    pub(crate) fn new(show_command: u32) -> Option<Self> {
        matches!(show_command, Self::SW_SHOWNORMAL | 2 | 3).then_some(Self { show_command })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CapturedAppbarState {
    pub(crate) flags: u32,
}

impl CapturedAppbarState {
    pub(crate) const ABS_AUTOHIDE: u32 = 1;

    pub(crate) fn new(flags: u32) -> Option<Self> {
        (flags <= 3).then_some(Self { flags })
    }

    /// Auto-hide releases Explorer's normal work-area reservation. Keep every
    /// other captured flag, then restore the full original word at session end.
    pub(crate) fn hidden_flags(self) -> u32 {
        self.flags | Self::ABS_AUTOHIDE
    }
}

/// Slint retains Windows HWNDs while hidden. A detached appbar must therefore
/// keep its own bar's nonactivation role until HWND destruction, even though
/// reservations and every other captured style are released.
pub(crate) fn detached_surface_styles(original: u32, kind: super::ShellSurfaceKind) -> u32 {
    // Documented Win32 extended style; portable fixtures do not call Win32.
    const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
    match kind {
        super::ShellSurfaceKind::Dock
        | super::ShellSurfaceKind::Toolbar
        | super::ShellSurfaceKind::Tooltip => original | WS_EX_NOACTIVATE,
        super::ShellSurfaceKind::Launcher | super::ShellSurfaceKind::Popup => original,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TaskbarFacts {
    pub class_matches: bool,
    pub title_empty: bool,
    pub root_unowned: bool,
    pub primary_monitor: bool,
    pub width: i64,
    pub height: i64,
    pub process_id: u32,
    pub explorer_image: bool,
    pub process_session: u32,
    pub own_session: u32,
}

impl TaskbarFacts {
    pub(crate) fn eligible(self) -> bool {
        self.class_matches
            && self.title_empty
            && self.root_unowned
            && self.primary_monitor
            && self.width > 0
            && self.height > 0
            && self.process_id != 0
            && self.explorer_image
            && self.process_session == self.own_session
    }
}

/// Exact sibling routing, shared by native command construction and pure tests.
pub(crate) fn session_command(gui: &std::path::Path) -> Option<(std::path::PathBuf, &'static str)> {
    let name = gui.file_name()?.to_str()?;
    if !gui.is_absolute() || !["Tessera.exe", "tessera-desktop.exe"].contains(&name) {
        return None;
    }
    Some((gui.parent()?.join("tessera-shell.exe"), "--desktop-session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_bars_keep_nonactivation_and_all_other_original_styles() {
        use super::super::ShellSurfaceKind;
        const NOACTIVATE: u32 = 0x0800_0000;
        for original in [0, 0x80, 0x8, 0x40000, NOACTIVATE] {
            for kind in [
                ShellSurfaceKind::Dock,
                ShellSurfaceKind::Toolbar,
                ShellSurfaceKind::Tooltip,
            ] {
                let detached = detached_surface_styles(original, kind);
                assert_ne!(detached & NOACTIVATE, 0);
                assert_eq!(detached & !NOACTIVATE, original & !NOACTIVATE);
            }
            assert_eq!(
                detached_surface_styles(original, ShellSurfaceKind::Launcher),
                original
            );
            assert_eq!(
                detached_surface_styles(original, ShellSurfaceKind::Popup),
                original
            );
        }
    }

    #[test]
    fn hide_enables_autohide_without_losing_original_flags() {
        for flags in [0, 1, 2, 3] {
            let original = CapturedAppbarState::new(flags).unwrap();
            assert_eq!(original.hidden_flags(), flags | 1);
            assert_eq!(original.flags, flags);
        }
        assert!(CapturedAppbarState::new(4).is_none());
    }

    #[test]
    fn normal_full_width_taskbar_is_not_an_application_window_filter() {
        // Visibility and tool-window style deliberately are not eligibility fields.
        let valid = TaskbarFacts {
            class_matches: true,
            title_empty: true,
            root_unowned: true,
            primary_monitor: true,
            width: 1920,
            height: 48,
            process_id: 42,
            explorer_image: true,
            process_session: 2,
            own_session: 2,
        };
        assert!(valid.eligible());
        assert!(
            !TaskbarFacts {
                process_id: 0,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                explorer_image: false,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                process_session: 3,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                primary_monitor: false,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                class_matches: false,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                title_empty: false,
                ..valid
            }
            .eligible()
        );
        assert!(
            !TaskbarFacts {
                root_unowned: false,
                ..valid
            }
            .eligible()
        );
        assert!(!TaskbarFacts { width: 0, ..valid }.eligible());
        assert!(!TaskbarFacts { height: 0, ..valid }.eligible());
    }

    #[test]
    fn command_targets_supervisor_not_gui_and_keeps_argument_separate() {
        let folder = std::env::temp_dir().join("Tessera fixture folder");
        for name in ["Tessera.exe", "tessera-desktop.exe"] {
            let (exe, flag) = session_command(&folder.join(name)).unwrap();
            assert_eq!(exe, folder.join("tessera-shell.exe"));
            assert_eq!(flag, "--desktop-session");
        }
        assert!(session_command(&folder.join("other.exe")).is_none());
        assert!(session_command(std::path::Path::new("Tessera.exe")).is_none());
    }
}
