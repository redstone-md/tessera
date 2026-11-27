// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Whether a display shows high dynamic range, which Windows calls
//! advanced color, and switching that.

use std::ffi::c_void;
use std::io;
use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_SUCCESS, LUID, POINT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MONITORINFOEXW, MonitorFromPoint,
};

use super::DisplayKey;

/// The display can show advanced color.
const SUPPORTED: u32 = 1;
/// It shows advanced color now.
const ENABLED: u32 = 1 << 1;
/// Something keeps it from showing advanced color.
const FORCE_DISABLED: u32 = 1 << 3;

/// The high dynamic range of one display that is capable of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hdr {
    adapter: LUID,
    target: u32,
    /// Whether the display shows it now.
    pub on: bool,
}

impl Hdr {
    /// That of `display`, or of the primary one without a display; `None`
    /// for a display that has none or does not say.
    pub fn of(display: Option<DisplayKey>) -> Option<Self> {
        let device = device_of(display)?;
        let path = paths()
            .into_iter()
            .find(|path| source_of(path).is_some_and(|name| name == device))?;
        let (adapter, target) = (path.targetInfo.adapterId, path.targetInfo.id);
        let state = state_of(adapter, target)?;
        let capable = state & SUPPORTED != 0 && state & FORCE_DISABLED == 0;
        capable.then_some(Self {
            adapter,
            target,
            on: state & ENABLED != 0,
        })
    }

    /// Has the display show high dynamic range, or not.
    pub fn set(&self, on: bool) -> io::Result<()> {
        let mut packet = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
                size: size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32,
                adapterId: self.adapter,
                id: self.target,
            },
            ..Default::default()
        };
        packet.Anonymous.value = u32::from(on);
        // SAFETY: the packet says its own size and outlives the call.
        match unsafe { DisplayConfigSetDeviceInfo(&raw const packet.header) } {
            0 => Ok(()),
            error => Err(io::Error::from_raw_os_error(error)),
        }
    }
}

/// The name Windows gives the device of `display`, as UTF-16 up to its
/// NUL.
fn device_of(display: Option<DisplayKey>) -> Option<Vec<u16>> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: the structure says its own size and outlives the call, which
    // fails for a display that is gone.
    let read = unsafe {
        let display = match display {
            Some(display) => HMONITOR(display.0 as *mut c_void),
            None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
        };
        GetMonitorInfoW(display, (&raw mut info).cast())
    };
    read.as_bool().then(|| text_of(&info.szDevice))
}

/// The units of `name` before its NUL.
fn text_of(name: &[u16]) -> Vec<u16> {
    let end = name.iter().position(|&unit| unit == 0);
    name[..end.unwrap_or(name.len())].to_vec()
}

/// The connections between the adapters and the displays in use.
pub(super) fn paths() -> Vec<DISPLAYCONFIG_PATH_INFO> {
    let (mut path_count, mut mode_count) = (0, 0);
    // SAFETY: the counts outlive the calls, and the lists are as long as
    // the counts say.
    unsafe {
        let sized = GetDisplayConfigBufferSizes(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut path_count,
            &raw mut mode_count,
        );
        if sized != ERROR_SUCCESS {
            return Vec::new();
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![Default::default(); mode_count as usize];
        let read = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &raw mut path_count,
            paths.as_mut_ptr(),
            &raw mut mode_count,
            modes.as_mut_ptr(),
            None,
        );
        if read != ERROR_SUCCESS {
            return Vec::new();
        }
        paths.truncate(path_count as usize);
        paths
    }
}

/// The name of the device `path` starts at.
fn source_of(path: &DISPLAYCONFIG_PATH_INFO) -> Option<Vec<u16>> {
    let mut packet = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: the packet says its own size and outlives the call.
    let read = unsafe { DisplayConfigGetDeviceInfo(&raw mut packet.header) };
    (read == 0).then(|| text_of(&packet.viewGdiDeviceName))
}

/// What Windows says about the advanced color of a display, as bits.
fn state_of(adapter: LUID, target: u32) -> Option<u32> {
    let mut packet = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size: size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
            adapterId: adapter,
            id: target,
        },
        ..Default::default()
    };
    // SAFETY: the packet says its own size and outlives the call, after
    // which the bits are read as the plain number they are.
    unsafe {
        let read = DisplayConfigGetDeviceInfo(&raw mut packet.header);
        (read == 0).then_some(packet.Anonymous.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_name_ends_at_its_nul() {
        assert_eq!(text_of(&[65, 66, 0, 67]), [65, 66]);
        assert_eq!(text_of(&[65, 66]), [65, 66]);
        assert!(text_of(&[0]).is_empty());
    }

    /// Reads the displays of the machine it runs on.
    #[test]
    #[ignore = "prints what the displays of this machine say"]
    fn what_the_primary_display_says() {
        println!("paths: {}", paths().len());
        println!("primary: {:?}", Hdr::of(None).map(|hdr| hdr.on));
    }
}
