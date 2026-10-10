// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! SDK-origin device IDs are the only monitor authority; order is incidental.

use tessera_system::wallpaper::WallpaperError;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};
use windows::core::PCWSTR;

use super::source::NativeName;

const MAX_MONITORS: u32 = 32;
const MAX_DEVICE_PATH_UNITS: usize = 1_024;

pub(super) fn desktop() -> Result<IDesktopWallpaper, WallpaperError> {
    // SAFETY: only called from the native owner STA; no interface crosses threads.
    unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_INPROC_SERVER) }
        .map_err(|_| WallpaperError::Unavailable)
}

#[derive(PartialEq, Eq)]
pub(super) struct Topology {
    pub(super) monitors: Vec<Monitor>,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Monitor {
    id: NativeName,
    rect: [i32; 4],
}

impl Monitor {
    pub(super) fn id(&self) -> PCWSTR {
        self.id.as_pcwstr()
    }

    pub(super) fn caption(&self) -> Result<String, WallpaperError> {
        let device =
            String::from_utf16(self.id.units()).map_err(|_| WallpaperError::Unavailable)?;
        let device: String = device
            .chars()
            .filter(|character| !character.is_control())
            .take(192)
            .collect();
        if device.trim().is_empty() {
            return Err(WallpaperError::Unavailable);
        }
        let [left, top, right, bottom] = self.rect;
        let width = i64::from(right) - i64::from(left);
        let height = i64::from(bottom) - i64::from(top);
        // Bounded display metadata only. No invented ordinal, primary status
        // or friendly name substitutes for the actual SDK ID and rectangle.
        Ok(format!("{width}×{height} at ({left}, {top}) · {device}"))
    }
}

impl Topology {
    pub(super) fn capture(desktop: &IDesktopWallpaper) -> Result<Self, WallpaperError> {
        let count = unsafe { desktop.GetMonitorDevicePathCount() }
            .map_err(|_| WallpaperError::Unavailable)?;
        if !(1..=MAX_MONITORS).contains(&count) {
            return Err(WallpaperError::Unavailable);
        }
        let mut monitors = Vec::with_capacity(count as usize);
        for index in 0..count {
            // Enumeration indices come from this SDK count, never UI input.
            let id = NativeName::take(
                unsafe { desktop.GetMonitorDevicePathAt(index) }
                    .map_err(|_| WallpaperError::Unavailable)?,
                MAX_DEVICE_PATH_UNITS,
            )?;
            let rect = unsafe { desktop.GetMonitorRECT(id.as_pcwstr()) }
                .map_err(|_| WallpaperError::Unavailable)?;
            if rect.left >= rect.right || rect.top >= rect.bottom {
                return Err(WallpaperError::Unavailable);
            }
            monitors.push(Monitor {
                id,
                rect: [rect.left, rect.top, rect.right, rect.bottom],
            });
        }
        if unsafe { desktop.GetMonitorDevicePathCount() }
            .map_err(|_| WallpaperError::Unavailable)?
            != count
        {
            return Err(WallpaperError::Unavailable);
        }
        monitors.sort_unstable_by(|left, right| left.id.units().cmp(right.id.units()));
        if monitors.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err(WallpaperError::Unavailable);
        }
        Ok(Self { monitors })
    }

    pub(super) fn matches(&self, desktop: &IDesktopWallpaper) -> bool {
        Self::capture(desktop).is_ok_and(|fresh| fresh == *self)
    }
}
