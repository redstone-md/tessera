// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the displays are called: the name each monitor gives itself in
//! its EDID, as the display settings of Windows show it, the name GDI
//! gives its output (`\\.\DISPLAY2`), and where it stands among the
//! others from left to right. This is the one place Tessera reads them;
//! the surfaces choose their displays by them.
//!
//! The friendly names come from `QueryDisplayConfig` and
//! `DisplayConfigGetDeviceInfo`, which ask the display driver and so are
//! read once at start and again when the displays change, not every
//! frame. Where the driver does not answer, the name GDI keeps for the
//! monitor is taken, which is often "Generic PnP Monitor".

#![allow(unsafe_code)]

use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR,
    MONITORINFOEXW,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
use windows::core::{BOOL, PCWSTR};

use super::ScreenRect;
use super::files::text_in;

/// How often the configuration is asked again when it changed between
/// asking for its size and reading it.
const ATTEMPTS: usize = 3;

/// What a display is called, and where it stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayName {
    /// The name GDI gives its output, as `\\.\DISPLAY2`.
    pub device: String,
    /// The name the monitor gives itself, as `DELL U2720Q`; the name of
    /// its driver where it gives none.
    pub friendly: String,
    /// Where it stands among the displays from left to right, the first
    /// being 1; above before below where two start at the same place.
    pub position: usize,
    pub primary: bool,
}

/// A monitor as GDI lists it.
struct Monitor {
    device: String,
    frame: ScreenRect,
    primary: bool,
}

/// The displays there are now, in the order Windows lists them, which is
/// the order of the display ids of GPUI.
pub fn display_names() -> Vec<DisplayName> {
    let monitors = monitors();
    let friendly = friendly_names();
    let positions = positions(
        &monitors
            .iter()
            .map(|monitor| monitor.frame)
            .collect::<Vec<_>>(),
    );
    monitors
        .into_iter()
        .zip(positions)
        .map(|(monitor, position)| {
            let named = friendly
                .iter()
                .find(|(device, _)| device.eq_ignore_ascii_case(&monitor.device))
                .map(|(_, name)| name.clone());
            let friendly = named
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| gdi_name(&monitor.device));
            DisplayName {
                device: monitor.device,
                friendly,
                position,
                primary: monitor.primary,
            }
        })
        .collect()
}

/// For each of `frames`, where it stands from left to right, from 1;
/// above before below where two start at the same place.
fn positions(frames: &[ScreenRect]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..frames.len()).collect();
    order.sort_by_key(|&index| (frames[index].left, frames[index].top));
    let mut positions = vec![0; frames.len()];
    for (rank, index) in order.into_iter().enumerate() {
        positions[index] = rank + 1;
    }
    positions
}

/// The monitors, in the order Windows lists them.
fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn add(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: `list` is the pointer `monitors` passed, to a list that
        // outlives the enumeration, and `info` says its own size, which
        // is that of the larger structure.
        unsafe {
            let list = &mut *(list.0 as *mut Vec<Monitor>);
            if GetMonitorInfoW(monitor, (&raw mut info).cast()).as_bool() {
                list.push(Monitor {
                    device: text_in(&info.szDevice),
                    frame: info.monitorInfo.rcMonitor.into(),
                    primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                });
            } else {
                // The list keeps the order of the ids of GPUI.
                list.push(Monitor {
                    device: String::new(),
                    frame: ScreenRect {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    },
                    primary: false,
                });
            }
        }
        true.into()
    }
    let mut list: Vec<Monitor> = Vec::new();
    // SAFETY: the enumeration is synchronous and `list` outlives it.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(add), LPARAM(&raw mut list as isize));
    }
    list
}

/// The name GDI keeps for the monitor on the output `device`, which is
/// that of its driver; the name of the output where there is none.
fn gdi_name(device: &str) -> String {
    let wide: Vec<u16> = device.encode_utf16().chain([0]).collect();
    let mut found = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    // SAFETY: `wide` is NUL-terminated and `found` says its own size; both
    // outlive the call.
    let read = unsafe { EnumDisplayDevicesW(PCWSTR(wide.as_ptr()), 0, &raw mut found, 0) };
    let name = read.as_bool().then(|| text_in(&found.DeviceString));
    name.filter(|name| !name.is_empty())
        .unwrap_or_else(|| device.trim_start_matches(r"\\.\").to_owned())
}

/// The name the monitor gives itself, by the name of its output, for each
/// active display path; empty where the driver does not say.
fn friendly_names() -> Vec<(String, String)> {
    let Some(paths) = active_paths() else {
        return Vec::new();
    };
    let mut names: Vec<(String, String)> = Vec::new();
    for path in paths {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>(),
                path.sourceInfo.adapterId,
                path.sourceInfo.id,
            ),
            ..Default::default()
        };
        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
            header: header(
                DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>(),
                path.targetInfo.adapterId,
                path.targetInfo.id,
            ),
            ..Default::default()
        };
        // SAFETY: each request starts with its header, which states the
        // size of the whole structure; both outlive the calls.
        let (source_read, target_read) = unsafe {
            (
                DisplayConfigGetDeviceInfo(&raw mut source.header),
                DisplayConfigGetDeviceInfo(&raw mut target.header),
            )
        };
        if source_read != ERROR_SUCCESS.0 as i32 {
            continue;
        }
        let device = text_in(&source.viewGdiDeviceName);
        let friendly = match target_read == ERROR_SUCCESS.0 as i32 {
            true => text_in(&target.monitorFriendlyDeviceName),
            false => String::new(),
        };
        // A display shown on two monitors is one output: the first name
        // stays.
        if !names.iter().any(|(known, _)| *known == device) {
            names.push((device, friendly));
        }
    }
    names
}

/// The header of a request of `DisplayConfigGetDeviceInfo`.
fn header(
    kind: windows::Win32::Devices::Display::DISPLAYCONFIG_DEVICE_INFO_TYPE,
    size: usize,
    adapter: windows::Win32::Foundation::LUID,
    id: u32,
) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER {
        r#type: kind,
        size: size as u32,
        adapterId: adapter,
        id,
    }
}

/// The display paths that show something now; `None` where Windows does
/// not say.
fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
    for _ in 0..ATTEMPTS {
        let (mut paths, mut modes) = (0_u32, 0_u32);
        // SAFETY: both counts outlive the call, which only writes them.
        let sized = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &raw mut paths, &raw mut modes)
        };
        if sized != ERROR_SUCCESS {
            return None;
        }
        let mut path_info = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_info = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        // SAFETY: the arrays have the lengths that are passed with them and
        // outlive the call; active paths are asked without a topology.
        let read = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &raw mut paths,
                path_info.as_mut_ptr(),
                &raw mut modes,
                mode_info.as_mut_ptr(),
                None,
            )
        };
        match read {
            ERROR_SUCCESS => {
                path_info.truncate(paths as usize);
                return Some(path_info);
            }
            // Displays came or went in between.
            ERROR_INSUFFICIENT_BUFFER => continue,
            _ => return None,
        }
    }
    None
}

/// The output of the display GPUI numbers `index`, as GDI names it, read
/// now; `None` for a number no display has.
pub fn display_device(index: usize) -> Option<String> {
    let device = monitors().into_iter().nth(index)?.device;
    (!device.is_empty()).then_some(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(left: i32, top: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right: left + 100,
            bottom: top + 100,
        }
    }

    #[test]
    fn displays_are_counted_from_left_to_right() {
        let frames = [frame(1920, 0), frame(0, 0), frame(-1280, 200)];
        assert_eq!(positions(&frames), [3, 2, 1]);
        // Above comes before below where two start at the same place.
        let stacked = [frame(0, 1080), frame(0, 0)];
        assert_eq!(positions(&stacked), [2, 1]);
        assert!(positions(&[]).is_empty());
    }
}
