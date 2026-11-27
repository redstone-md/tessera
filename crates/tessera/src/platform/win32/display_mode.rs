// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How the displays of the computer show the desktop, as Win+P chooses it
//! in Windows: on the screen of the computer only, the same on all of
//! them, extended over all of them, or on the second screen only.
//!
//! The flyout of Win+P is part of Explorer; this asks and sets the same
//! thing with `QueryDisplayConfig` and `SetDisplayConfig`. Setting it
//! changes the modes of the displays, which takes a moment and may wait
//! for the driver, so both are to be called away from the thread that
//! draws.

use anyhow::{Result, bail};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_TOPOLOGY_CLONE,
    DISPLAYCONFIG_TOPOLOGY_EXTEND, DISPLAYCONFIG_TOPOLOGY_EXTERNAL, DISPLAYCONFIG_TOPOLOGY_ID,
    DISPLAYCONFIG_TOPOLOGY_INTERNAL, GetDisplayConfigBufferSizes, QDC_DATABASE_CURRENT,
    QueryDisplayConfig, SDC_APPLY, SDC_TOPOLOGY_CLONE, SDC_TOPOLOGY_EXTEND, SDC_TOPOLOGY_EXTERNAL,
    SDC_TOPOLOGY_INTERNAL, SDC_VALIDATE, SET_DISPLAY_CONFIG_FLAGS, SetDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};

/// How often the configuration is asked again when it changed between
/// asking for its size and reading it.
const ATTEMPTS: usize = 3;

/// A way of showing the desktop on the displays of the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisplayMode {
    /// Only the screen of the computer (the internal one of a laptop, or
    /// the first one) shows anything.
    PcScreenOnly,
    /// Every display shows the same.
    Duplicate,
    /// The desktop goes on from one display to the next.
    Extend,
    /// Only the other screen shows anything.
    SecondScreenOnly,
}

impl DisplayMode {
    /// The modes, in the order Windows offers them.
    pub const ALL: [Self; 4] = [
        Self::PcScreenOnly,
        Self::Duplicate,
        Self::Extend,
        Self::SecondScreenOnly,
    ];

    fn flag(self) -> SET_DISPLAY_CONFIG_FLAGS {
        match self {
            Self::PcScreenOnly => SDC_TOPOLOGY_INTERNAL,
            Self::Duplicate => SDC_TOPOLOGY_CLONE,
            Self::Extend => SDC_TOPOLOGY_EXTEND,
            Self::SecondScreenOnly => SDC_TOPOLOGY_EXTERNAL,
        }
    }

    fn of_topology(topology: DISPLAYCONFIG_TOPOLOGY_ID) -> Option<Self> {
        Some(match topology {
            DISPLAYCONFIG_TOPOLOGY_INTERNAL => Self::PcScreenOnly,
            DISPLAYCONFIG_TOPOLOGY_CLONE => Self::Duplicate,
            DISPLAYCONFIG_TOPOLOGY_EXTEND => Self::Extend,
            DISPLAYCONFIG_TOPOLOGY_EXTERNAL => Self::SecondScreenOnly,
            _ => return None,
        })
    }

    /// The mode Windows keeps for the displays connected now; `None`
    /// where it does not say, as for an arrangement made by hand that is
    /// none of the four.
    pub fn current() -> Option<Self> {
        for _ in 0..ATTEMPTS {
            let (mut paths, mut modes) = (0_u32, 0_u32);
            // SAFETY: both counts outlive the call, which only writes them.
            let sized = unsafe {
                GetDisplayConfigBufferSizes(QDC_DATABASE_CURRENT, &raw mut paths, &raw mut modes)
            };
            if sized != ERROR_SUCCESS {
                return None;
            }
            let mut path_info = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
            let mut mode_info = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
            let mut topology = DISPLAYCONFIG_TOPOLOGY_ID::default();
            // SAFETY: the arrays have the lengths that are passed with
            // them, and they and the topology outlive the call.
            let read = unsafe {
                QueryDisplayConfig(
                    QDC_DATABASE_CURRENT,
                    &raw mut paths,
                    path_info.as_mut_ptr(),
                    &raw mut modes,
                    mode_info.as_mut_ptr(),
                    Some(&raw mut topology),
                )
            };
            match read {
                ERROR_SUCCESS => return Self::of_topology(topology),
                // Displays came or went in between.
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return None,
            }
        }
        None
    }

    /// Whether Windows can show the desktop this way with the displays
    /// connected now: the modes for two displays need two.
    pub fn possible(self) -> bool {
        // SAFETY: without paths and modes the call only checks whether
        // the database has a configuration for the topology.
        unsafe {
            SetDisplayConfig(None, None, SDC_VALIDATE | self.flag()) == ERROR_SUCCESS.0 as i32
        }
    }

    /// Shows the desktop this way, with the arrangement Windows keeps for
    /// it, as Win+P does.
    pub fn apply(self) -> Result<()> {
        // SAFETY: without paths and modes the call takes the
        // configuration of the database for the topology.
        let status = unsafe { SetDisplayConfig(None, None, SDC_APPLY | self.flag()) };
        if status != ERROR_SUCCESS.0 as i32 {
            let error =
                windows::core::Error::from(windows::core::HRESULT::from_win32(status as u32));
            bail!("the displays were not switched to {self:?}: {error}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_is_a_topology_of_windows() {
        for mode in DisplayMode::ALL {
            let topology = DISPLAYCONFIG_TOPOLOGY_ID(mode.flag().0 as i32);
            assert_eq!(DisplayMode::of_topology(topology), Some(mode));
        }
        assert_eq!(
            DisplayMode::of_topology(DISPLAYCONFIG_TOPOLOGY_ID(0x10)),
            None
        );
    }
}
