// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How bright a display is. A monitor is asked over DDC/CI, the built-in
//! panel of a laptop through its driver.

use std::ffi::c_void;
use std::mem::size_of;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Devices::Display::{
    DISPLAY_BRIGHTNESS, DISPLAYPOLICY_AC, DISPLAYPOLICY_DC, DestroyPhysicalMonitors,
    GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR,
    IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS, IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS, PHYSICAL_MONITOR,
    SetMonitorBrightness,
};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, POINT};
use windows::Win32::Graphics::Gdi::{HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::core::w;

use super::DisplayKey;

/// The most the driver of a built-in panel counts to.
const PANEL_FULL: u8 = 100;

/// The brightness of a display, from 0 to 1.
pub struct ScreenBrightness;

impl ScreenBrightness {
    /// How bright `display` is, or the primary one without a display.
    /// `None` when nothing there says: many monitors have DDC/CI switched
    /// off in their own menu. Asking a monitor takes a part of a second.
    pub fn read(display: Option<DisplayKey>) -> Option<f32> {
        let monitors = Monitors::of(display);
        let asked = monitors.ranges().next().map(|(_, range)| range.share());
        asked.or_else(|| Panel::open().ok()?.read())
    }

    /// Makes `display` as bright as `share` says, or the primary one
    /// without a display.
    pub fn set(display: Option<DisplayKey>, share: f32) -> Result<()> {
        let monitors = Monitors::of(display);
        let mut set = false;
        for (monitor, range) in monitors.ranges() {
            // SAFETY: the handle is one of `monitors`, which is alive.
            set |= unsafe { SetMonitorBrightness(monitor, range.level(share)) } != 0;
        }
        if set {
            return Ok(());
        }
        Panel::open()
            .and_then(|panel| panel.set(share))
            .context("the display does not let its brightness be set")
    }
}

/// The least, the current and the most brightness a monitor counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    least: u32,
    now: u32,
    most: u32,
}

impl Range {
    /// Where in the range the monitor is, from 0 to 1.
    fn share(self) -> f32 {
        let span = self.most.saturating_sub(self.least).max(1);
        (self.now.saturating_sub(self.least) as f32 / span as f32).clamp(0., 1.)
    }

    /// What the monitor counts at `share` of the range.
    fn level(self, share: f32) -> u32 {
        let span = self.most.saturating_sub(self.least);
        self.least + (share.clamp(0., 1.) * span as f32).round() as u32
    }
}

/// The monitors behind a display, until dropped.
struct Monitors(Vec<PHYSICAL_MONITOR>);

impl Monitors {
    fn of(display: Option<DisplayKey>) -> Self {
        // SAFETY: the calls fill a count and a list that outlive them, and
        // fail for a display that is gone.
        unsafe {
            let display = match display {
                Some(display) => HMONITOR(display.0 as *mut c_void),
                None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
            };
            let mut count = 0;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(display, &raw mut count).is_err() {
                return Self(Vec::new());
            }
            let mut monitors = vec![PHYSICAL_MONITOR::default(); count as usize];
            match GetPhysicalMonitorsFromHMONITOR(display, &mut monitors) {
                Ok(()) => Self(monitors),
                Err(_) => Self(Vec::new()),
            }
        }
    }

    /// The monitors that say how bright they are, with what they say.
    fn ranges(&self) -> impl Iterator<Item = (HANDLE, Range)> {
        self.0.iter().filter_map(|monitor| {
            let handle = monitor.hPhysicalMonitor;
            let (mut least, mut now, mut most) = (0, 0, 0);
            // SAFETY: the handle is alive as long as `self`, and the three
            // numbers outlive the call that fills them.
            let said = unsafe {
                GetMonitorBrightness(handle, &raw mut least, &raw mut now, &raw mut most)
            };
            (said != 0 && most > least).then_some((handle, Range { least, now, most }))
        })
    }
}

impl Drop for Monitors {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            // SAFETY: the handles came from `GetPhysicalMonitorsFromHMONITOR`
            // and are given back once.
            let _ = unsafe { DestroyPhysicalMonitors(&self.0) };
        }
    }
}

/// The built-in panel, until dropped.
struct Panel(HANDLE);

impl Panel {
    fn open() -> Result<Self> {
        // SAFETY: the name is a constant, and the handle is closed when
        // the panel is dropped.
        let handle = unsafe {
            CreateFileW(
                w!(r"\\.\LCD"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            )
        };
        Ok(Self(handle.context("there is no built-in panel")?))
    }

    fn read(&self) -> Option<f32> {
        let mut levels = DISPLAY_BRIGHTNESS::default();
        let mut filled = 0;
        // SAFETY: `levels` and `filled` outlive the call, which is told the
        // size of what it may fill.
        unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS,
                None,
                0,
                Some((&raw mut levels).cast()),
                size_of::<DISPLAY_BRIGHTNESS>() as u32,
                Some(&raw mut filled),
                None,
            )
        }
        .ok()?;
        Some(panel_share(levels))
    }

    fn set(&self, share: f32) -> Result<()> {
        let level = (share.clamp(0., 1.) * f32::from(PANEL_FULL)).round() as u8;
        let levels = DISPLAY_BRIGHTNESS {
            ucDisplayPolicy: (DISPLAYPOLICY_AC | DISPLAYPOLICY_DC) as u8,
            ucACBrightness: level,
            ucDCBrightness: level,
        };
        let mut filled = 0;
        // SAFETY: `levels` and `filled` outlive the call, which is told the
        // size of what it reads.
        unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS,
                Some((&raw const levels).cast()),
                size_of::<DISPLAY_BRIGHTNESS>() as u32,
                None,
                0,
                Some(&raw mut filled),
                None,
            )
        }
        .context("the built-in panel refused the brightness")?;
        if filled > size_of::<DISPLAY_BRIGHTNESS>() as u32 {
            bail!("the built-in panel answered with more than it was asked");
        }
        Ok(())
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// How bright the panel is by `levels`: the level for the power it runs on.
fn panel_share(levels: DISPLAY_BRIGHTNESS) -> f32 {
    let level = if u32::from(levels.ucDisplayPolicy) == DISPLAYPOLICY_DC {
        levels.ucDCBrightness
    } else {
        levels.ucACBrightness
    };
    (f32::from(level) / f32::from(PANEL_FULL)).clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_is_as_bright_as_its_place_in_its_range() {
        let range = |least, now, most| Range { least, now, most };
        assert_eq!(range(0, 50, 100).share(), 0.5);
        assert_eq!(range(20, 20, 120).share(), 0.);
        assert_eq!(range(20, 120, 120).share(), 1.);
        assert_eq!(range(0, 300, 255).share(), 1.);
        assert_eq!(range(20, 0, 120).level(0.5), 70);
        assert_eq!(range(0, 0, 255).level(1.4), 255);
        assert_eq!(range(10, 0, 90).level(-1.), 10);
    }

    #[test]
    fn a_panel_is_as_bright_as_the_level_for_its_power() {
        let levels = |policy, ac, dc| DISPLAY_BRIGHTNESS {
            ucDisplayPolicy: policy,
            ucACBrightness: ac,
            ucDCBrightness: dc,
        };
        assert_eq!(panel_share(levels(1, 80, 40)), 0.8);
        assert_eq!(panel_share(levels(2, 80, 40)), 0.4);
        assert_eq!(panel_share(levels(3, 80, 40)), 0.8);
    }

    /// Asks the displays of this machine and sets each to what it said, so
    /// nothing changes.
    #[test]
    #[ignore = "talks to the monitors of the machine"]
    fn the_primary_display_takes_the_brightness_it_has() {
        let share = ScreenBrightness::read(None);
        println!("the primary display says {share:?}");
        if let Some(share) = share {
            ScreenBrightness::set(None, share).unwrap();
            assert_eq!(ScreenBrightness::read(None), Some(share));
        }
    }
}
