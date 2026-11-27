// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The configuration of the displays, as the display settings of Windows
//! and the display page of KDE Plasma set it: which displays are on, where
//! each stands on the desktop, which one is the main one (the one at the
//! origin), and the resolution, refresh rate, orientation and scale of
//! each.
//!
//! The arrangement is read and set with `QueryDisplayConfig` and
//! `SetDisplayConfig`, each configuration checked with `SDC_VALIDATE`
//! before it is applied. A configuration applied is not saved for the
//! next start until it is kept, so that one the display cannot show is
//! gone after a restart. The modes a display offers come from
//! `EnumDisplaySettingsExW`. The scale goes through
//! `DisplayConfigGetDeviceInfo` and `DisplayConfigSetDeviceInfo` with the
//! two packet types the Windows settings use, which Microsoft does not
//! document; they are checked at each reading, and a display whose scale
//! cannot be read this way has none offered.
//!
//! Every call here may wait for the driver, which takes up to seconds
//! while the displays change, so all of it is to be called away from the
//! thread that draws.

use std::mem::size_of;

use anyhow::{Result, bail};
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_DEVICE_INFO_TYPE, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_RATIONAL, DISPLAYCONFIG_ROTATION,
    DISPLAYCONFIG_ROTATION_IDENTITY, DISPLAYCONFIG_ROTATION_ROTATE90,
    DISPLAYCONFIG_ROTATION_ROTATE180, DISPLAYCONFIG_ROTATION_ROTATE270,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ALL_PATHS, QDC_ONLY_ACTIVE_PATHS, QUERY_DISPLAY_CONFIG_FLAGS,
    QueryDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_SAVE_TO_DATABASE,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SDC_VALIDATE, SET_DISPLAY_CONFIG_FLAGS, SetDisplayConfig,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID};
use windows::Win32::Graphics::Gdi::{
    DEVMODE_DISPLAY_ORIENTATION, DEVMODEW, DISPLAYCONFIG_PATH_ACTIVE,
    DISPLAYCONFIG_PATH_MODE_IDX_INVALID, DMDO_90, DMDO_270, ENUM_DISPLAY_SETTINGS_FLAGS,
    ENUM_DISPLAY_SETTINGS_MODE, EnumDisplaySettingsExW,
};
use windows::core::{HRESULT, HSTRING};

use super::display_names;

/// How often the configuration is asked again when it changed between
/// asking for its size and reading it.
const ATTEMPTS: usize = 3;

/// The packet types that read and set the scale of a source, which the
/// Windows settings use and Microsoft does not document.
const GET_SCALE: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(-3);
const SET_SCALE: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(-4);

/// The scales Windows offers, in percent, in its order; a scale packet
/// counts in steps of this list from the one Windows recommends.
const SCALES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// The refresh rates that are a whole number of hertz on paper and a
/// thousandth less on the wire, as 59.94 for 60: Windows lists them as one
/// hertz less.
const BROADCAST_RATES: [u32; 9] = [24, 30, 48, 60, 72, 90, 120, 144, 240];

/// A display, by its graphics adapter and its number there; it stays the
/// same while the display is connected, whether it is on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OutputId {
    adapter: (u32, i32),
    target: u32,
}

/// The picture source a display shows: the part of the desktop it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SourceId {
    adapter: (u32, i32),
    id: u32,
}

impl SourceId {
    fn luid(self) -> LUID {
        LUID {
            LowPart: self.adapter.0,
            HighPart: self.adapter.1,
        }
    }
}

fn adapter_of(luid: LUID) -> (u32, i32) {
    (luid.LowPart, luid.HighPart)
}

/// How a display is turned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Rotation {
    #[default]
    Landscape,
    /// Turned a quarter to the right.
    Portrait,
    LandscapeFlipped,
    PortraitFlipped,
}

impl Rotation {
    pub const ALL: [Self; 4] = [
        Self::Landscape,
        Self::Portrait,
        Self::LandscapeFlipped,
        Self::PortraitFlipped,
    ];

    fn of(rotation: DISPLAYCONFIG_ROTATION) -> Self {
        match rotation {
            DISPLAYCONFIG_ROTATION_ROTATE90 => Self::Portrait,
            DISPLAYCONFIG_ROTATION_ROTATE180 => Self::LandscapeFlipped,
            DISPLAYCONFIG_ROTATION_ROTATE270 => Self::PortraitFlipped,
            _ => Self::Landscape,
        }
    }

    fn raw(self) -> DISPLAYCONFIG_ROTATION {
        match self {
            Self::Landscape => DISPLAYCONFIG_ROTATION_IDENTITY,
            Self::Portrait => DISPLAYCONFIG_ROTATION_ROTATE90,
            Self::LandscapeFlipped => DISPLAYCONFIG_ROTATION_ROTATE180,
            Self::PortraitFlipped => DISPLAYCONFIG_ROTATION_ROTATE270,
        }
    }

    /// Whether the desktop on the display is higher than wide where the
    /// mode is wider than high.
    pub fn is_upright(self) -> bool {
        matches!(self, Self::Portrait | Self::PortraitFlipped)
    }

    /// The size of the desktop on a display in `mode` turned this way.
    pub fn turn(self, (width, height): (u32, u32)) -> (u32, u32) {
        match self.is_upright() {
            true => (height, width),
            false => (width, height),
        }
    }
}

/// A mode a display offers, as the mode is, whichever way the display is
/// turned.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VideoMode {
    pub width: u32,
    pub height: u32,
    /// In whole hertz, as Windows lists them: 59 for 59.94.
    pub hertz: u32,
}

/// The scale of a display, in percent of 96 dots per inch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scale {
    pub current: u32,
    pub recommended: u32,
    /// The scales the display may be set to, smallest first.
    pub choices: Vec<u32>,
}

/// A display and how it shows the desktop.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayOutput {
    pub id: OutputId,
    /// The number of its desktop in Windows, as `\\.\DISPLAY2` gives 2;
    /// `None` for a display that is off.
    pub number: Option<u32>,
    /// The name of its desktop for GDI, as `\\.\DISPLAY2`; empty while it
    /// is off.
    pub device: String,
    /// The name the monitor gives itself, as `DELL U2720Q`, from the one
    /// list of display names; empty while it is off.
    pub friendly: String,
    pub active: bool,
    /// Where its top left corner stands on the desktop, in pixels.
    pub position: (i32, i32),
    /// The mode it shows, whichever way it is turned.
    pub mode: VideoMode,
    pub rotation: Rotation,
    /// The modes it offers; none while it is off.
    pub modes: Vec<VideoMode>,
    /// `None` where the scale cannot be read on this Windows.
    pub scale: Option<Scale>,
    /// The display it shows the same as, where it duplicates one.
    pub mirror_of: Option<OutputId>,
    source: Option<SourceId>,
}

impl DisplayOutput {
    /// The size of its part of the desktop, in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.rotation.turn((self.mode.width, self.mode.height))
    }

    /// A display that is off, for tests of what is made of displays.
    #[cfg(test)]
    pub fn test_output() -> Self {
        Self {
            id: OutputId {
                adapter: (0, 0),
                target: 0,
            },
            number: None,
            device: String::new(),
            friendly: String::new(),
            active: false,
            position: (0, 0),
            mode: VideoMode {
                width: 0,
                height: 0,
                hertz: 0,
            },
            rotation: Rotation::Landscape,
            modes: Vec::new(),
            scale: None,
            mirror_of: None,
            source: None,
        }
    }

    /// Whether it is the main display: the one whose own desktop stands
    /// at the origin. A display only switched on in a draft has no desktop
    /// yet, and one that shows the same as another has none of its own.
    pub fn is_primary(&self) -> bool {
        self.active
            && self.source.is_some()
            && self.mirror_of.is_none()
            && self.mode.width > 0
            && self.position == (0, 0)
    }
}

/// The configuration of the displays as it was read.
#[derive(Clone)]
pub struct DisplaySetup {
    pub outputs: Vec<DisplayOutput>,
    /// The active paths and their modes, as read, to put back.
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
    /// Every path between a source and a display that is connected, to
    /// switch a display on with.
    all_paths: Vec<DISPLAYCONFIG_PATH_INFO>,
}

impl std::fmt::Debug for DisplaySetup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        { formatter.debug_struct("DisplaySetup") }
            .field("outputs", &self.outputs)
            .field("paths", &self.paths.len())
            .finish_non_exhaustive()
    }
}

impl DisplaySetup {
    /// The displays connected now and how they show the desktop.
    pub fn read() -> Result<Self> {
        let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
        let (all_paths, _) = query(QDC_ALL_PATHS)?;
        let mut outputs: Vec<DisplayOutput> = Vec::new();
        let names = display_names();
        for path in &paths {
            let source = SourceId {
                adapter: adapter_of(path.sourceInfo.adapterId),
                id: path.sourceInfo.id,
            };
            let id = OutputId {
                adapter: adapter_of(path.targetInfo.adapterId),
                target: path.targetInfo.id,
            };
            // SAFETY: without QDC_VIRTUAL_MODE_AWARE the union holds the
            // index of the mode.
            let index = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let Some(mode) = modes
                .get(index)
                .filter(|mode| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            else {
                continue;
            };
            // SAFETY: the mode is a source mode, as its type says.
            let source_mode = unsafe { mode.Anonymous.sourceMode };
            let rotation = Rotation::of(path.targetInfo.rotation);
            let (width, height) = rotation.turn((source_mode.width, source_mode.height));
            let mirror_of = { outputs.iter() }
                .find(|output| output.source == Some(source))
                .map(|output| output.id);
            let device = source_name(source).unwrap_or_default();
            let friendly = { names.iter() }
                .find(|name| name.device.eq_ignore_ascii_case(&device))
                .map(|name| name.friendly.clone())
                .unwrap_or_default();
            outputs.push(DisplayOutput {
                friendly,
                id,
                number: display_number(&device),
                modes: video_modes(&device),
                active: true,
                position: (source_mode.position.x, source_mode.position.y),
                mode: VideoMode {
                    width,
                    height,
                    hertz: whole_hertz(path.targetInfo.refreshRate),
                },
                rotation,
                scale: scale_of_source(source),
                mirror_of,
                source: Some(source),
                device,
            });
        }
        for path in &all_paths {
            let id = OutputId {
                adapter: adapter_of(path.targetInfo.adapterId),
                target: path.targetInfo.id,
            };
            let connected = path.targetInfo.targetAvailable.as_bool();
            if connected && !outputs.iter().any(|output| output.id == id) {
                outputs.push(DisplayOutput {
                    id,
                    number: None,
                    device: String::new(),
                    friendly: String::new(),
                    active: false,
                    position: (0, 0),
                    mode: VideoMode {
                        width: 0,
                        height: 0,
                        hertz: 0,
                    },
                    rotation: Rotation::Landscape,
                    modes: Vec::new(),
                    scale: None,
                    mirror_of: None,
                    source: None,
                });
            }
        }
        if outputs.is_empty() {
            bail!("Windows tells of no display");
        }
        Ok(Self {
            outputs,
            paths,
            modes,
            all_paths,
        })
    }

    /// Shows the desktop on the displays as `wanted` says: those of this
    /// setup, changed. The configuration is checked before it is applied;
    /// one Windows refuses changes nothing and is the error. It is not
    /// saved for the next start: [`DisplaySetup::save`] does that once it
    /// is kept, so that a restart while it is tried brings the old one
    /// back. The scale of
    /// each display is set after; the scales Windows did not take are
    /// what the answer lists, the rest having been applied.
    pub fn apply(&self, wanted: &[DisplayOutput]) -> Result<Vec<String>> {
        let arranged = wanted.iter().any(|output| {
            let before = self.outputs.iter().find(|known| known.id == output.id);
            before.is_none_or(|before| !before.same_arrangement(output))
        });
        if arranged {
            let (paths, modes) = self.plan(wanted)?;
            let supplied = SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
            set(&paths, &modes, SDC_VALIDATE | supplied)
                .map_err(|error| error.context("Windows refuses this arrangement"))?;
            set(&paths, &modes, SDC_APPLY | supplied)?;
        }
        let mut problems = Vec::new();
        for output in wanted {
            let before = self.outputs.iter().find(|known| known.id == output.id);
            let (Some(source), Some(scale), Some(before)) = (output.source, &output.scale, before)
            else {
                continue;
            };
            if before.scale.as_ref().map(|scale| scale.current) != Some(scale.current) {
                problems.extend(
                    set_scale(source, scale)
                        .err()
                        .map(|error| format!("{error:#}")),
                );
            }
        }
        Ok(problems)
    }

    /// Saves the configuration the displays show now for the next start,
    /// as Windows does with one that is kept.
    pub fn save() -> Result<()> {
        let (paths, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
        let flags =
            SDC_APPLY | SDC_SAVE_TO_DATABASE | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        set(&paths, &modes, flags)
    }

    /// Puts the configuration back as it was read, scales and all, and
    /// saves it for the next start.
    pub fn restore(&self) -> Result<()> {
        let flags =
            SDC_APPLY | SDC_SAVE_TO_DATABASE | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES;
        set(&self.paths, &self.modes, flags)?;
        // Every scale is set back, even after one Windows refused.
        let mut refused = None;
        for output in &self.outputs {
            if let (Some(source), Some(scale)) = (output.source, &output.scale)
                && let Err(error) = set_scale(source, scale)
            {
                refused.get_or_insert(error);
            }
        }
        refused.map_or(Ok(()), Err)
    }

    /// The paths and modes that show `wanted`.
    fn plan(
        &self,
        wanted: &[DisplayOutput],
    ) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)> {
        let mut paths = Vec::new();
        let mut modes = Vec::new();
        // The new index of each source mode, by its source, for displays
        // that show the same source.
        let mut placed: Vec<(SourceId, u32)> = Vec::new();
        for output in wanted.iter().filter(|output| output.active) {
            let before = self.outputs.iter().find(|known| known.id == output.id);
            let path = self.paths.iter().find(|path| {
                adapter_of(path.targetInfo.adapterId) == output.id.adapter
                    && path.targetInfo.id == output.id.target
            });
            let (Some(before), Some(&path)) = (before.filter(|before| before.active), path) else {
                paths.push(self.path_to_switch_on(output.id, &paths)?);
                continue;
            };
            let mut path = path;
            path.targetInfo.rotation = output.rotation.raw();
            let Some(source) = before.source else {
                bail!("a display that is on has no source");
            };
            let index = match placed.iter().find(|(known, _)| *known == source) {
                Some(&(_, index)) => index,
                None => {
                    // SAFETY: without QDC_VIRTUAL_MODE_AWARE the union
                    // holds the index of the mode.
                    let old = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
                    let mut mode = self.modes[old];
                    let (width, height) = output.size();
                    // SAFETY: the mode is the source mode of the path.
                    let mut source_mode = unsafe { mode.Anonymous.sourceMode };
                    source_mode.width = width;
                    source_mode.height = height;
                    source_mode.position.x = output.position.0;
                    source_mode.position.y = output.position.1;
                    mode.Anonymous.sourceMode = source_mode;
                    modes.push(mode);
                    let index = (modes.len() - 1) as u32;
                    placed.push((source, index));
                    index
                }
            };
            path.sourceInfo.Anonymous.modeInfoIdx = index;
            // A new mode or rate has Windows choose the signal to match.
            let signal_kept = before.mode == output.mode && before.rotation == output.rotation;
            // SAFETY: as for the source.
            let target = unsafe { path.targetInfo.Anonymous.modeInfoIdx };
            match self.modes.get(target as usize).filter(|_| signal_kept) {
                Some(&mode) => {
                    modes.push(mode);
                    path.targetInfo.Anonymous.modeInfoIdx = (modes.len() - 1) as u32;
                }
                None => {
                    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
                    if before.mode.hertz != output.mode.hertz {
                        path.targetInfo.refreshRate = rational(output.mode.hertz);
                    }
                }
            }
            paths.push(path);
        }
        if paths.is_empty() {
            bail!("at least one display has to stay on");
        }
        Ok((paths, modes))
    }

    /// A path that switches the display `id` on, with a source no path in
    /// `taken` uses; Windows chooses its mode and place.
    fn path_to_switch_on(
        &self,
        id: OutputId,
        taken: &[DISPLAYCONFIG_PATH_INFO],
    ) -> Result<DISPLAYCONFIG_PATH_INFO> {
        let used = |path: &DISPLAYCONFIG_PATH_INFO| {
            taken.iter().chain(&self.paths).any(|other| {
                other.sourceInfo.id == path.sourceInfo.id
                    && other.sourceInfo.adapterId == path.sourceInfo.adapterId
            })
        };
        let Some(&found) = self.all_paths.iter().find(|path| {
            adapter_of(path.targetInfo.adapterId) == id.adapter
                && path.targetInfo.id == id.target
                && path.targetInfo.targetAvailable.as_bool()
                && !used(path)
        }) else {
            bail!("the graphics adapter has no free output for another display");
        };
        let mut path = found;
        path.flags |= DISPLAYCONFIG_PATH_ACTIVE;
        path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        Ok(path)
    }
}

impl DisplayOutput {
    /// Whether `other` stands where this does and shows what it shows.
    fn same_arrangement(&self, other: &Self) -> bool {
        self.active == other.active
            && self.position == other.position
            && self.mode == other.mode
            && self.rotation == other.rotation
    }
}

/// The paths and modes Windows has for `flags`.
fn query(
    flags: QUERY_DISPLAY_CONFIG_FLAGS,
) -> Result<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)> {
    for _ in 0..ATTEMPTS {
        let (mut path_count, mut mode_count) = (0_u32, 0_u32);
        // SAFETY: both counts outlive the call, which only writes them.
        let sized =
            unsafe { GetDisplayConfigBufferSizes(flags, &raw mut path_count, &raw mut mode_count) };
        if sized != ERROR_SUCCESS {
            bail!("Windows does not tell how the displays are set: {sized:?}");
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: the arrays have the lengths passed with them and outlive
        // the call.
        let read = unsafe {
            QueryDisplayConfig(
                flags,
                &raw mut path_count,
                paths.as_mut_ptr(),
                &raw mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        match read {
            ERROR_SUCCESS => {
                paths.truncate(path_count as usize);
                modes.truncate(mode_count as usize);
                return Ok((paths, modes));
            }
            // Displays came or went in between.
            ERROR_INSUFFICIENT_BUFFER => continue,
            error => bail!("Windows does not tell how the displays are set: {error:?}"),
        }
    }
    bail!("the displays kept changing while they were read")
}

/// Hands `paths` and `modes` to Windows with `flags`.
fn set(
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
    flags: SET_DISPLAY_CONFIG_FLAGS,
) -> Result<()> {
    // SAFETY: the arrays outlive the call, which only reads them.
    let status = unsafe { SetDisplayConfig(Some(paths), Some(modes), flags) };
    if status != ERROR_SUCCESS.0 as i32 {
        let error = windows::core::Error::from(HRESULT::from_win32(status as u32));
        bail!("the displays were not set: {}", error.message());
    }
    Ok(())
}

/// The name GDI gives the desktop of `source`, as `\\.\DISPLAY1`.
fn source_name(source: SourceId) -> Option<String> {
    let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: source.luid(),
            id: source.id,
        },
        ..Default::default()
    };
    // SAFETY: the packet is of the size its header says and outlives the
    // call that fills it.
    let read = unsafe { DisplayConfigGetDeviceInfo(&raw mut name.header) };
    if read != ERROR_SUCCESS.0 as i32 {
        return None;
    }
    let length = { name.viewGdiDeviceName.iter() }
        .position(|&unit| unit == 0)
        .unwrap_or(name.viewGdiDeviceName.len());
    Some(String::from_utf16_lossy(&name.viewGdiDeviceName[..length]))
}

/// The number at the end of a name such as `\\.\DISPLAY12`.
fn display_number(device: &str) -> Option<u32> {
    let digits = device.trim_end_matches(|letter: char| !letter.is_ascii_digit());
    let start = digits
        .trim_end_matches(|letter: char| letter.is_ascii_digit())
        .len();
    digits[start..].parse().ok()
}

/// The modes the desktop `device` offers, each once, largest and quickest
/// first, as the modes are whichever way the display is turned.
fn video_modes(device: &str) -> Vec<VideoMode> {
    if device.is_empty() {
        return Vec::new();
    }
    let name = HSTRING::from(device);
    let mut modes = Vec::new();
    for index in 0.. {
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: the name and the structure, sized as it says, outlive the
        // call that fills it.
        let listed = unsafe {
            EnumDisplaySettingsExW(
                &name,
                ENUM_DISPLAY_SETTINGS_MODE(index),
                &raw mut mode,
                ENUM_DISPLAY_SETTINGS_FLAGS(0),
            )
        };
        if !listed.as_bool() {
            break;
        }
        // SAFETY: for a display the union holds its position and
        // orientation; a mode listed without them has zeros there, which
        // is the orientation not turned.
        let orientation = unsafe { mode.Anonymous1.Anonymous2.dmDisplayOrientation };
        let listed = VideoMode {
            width: mode.dmPelsWidth,
            height: mode.dmPelsHeight,
            hertz: mode.dmDisplayFrequency,
        };
        modes.push((mode.dmBitsPerPel, unturned(orientation, listed)));
    }
    distinct_modes(modes)
}

/// The mode `listed` is: Windows lists the modes of a display in the
/// orientation it is turned to, so those of one turned upright higher
/// than wide, where [`DisplayOutput::mode`] keeps them as the panel has
/// them.
fn unturned(orientation: DEVMODE_DISPLAY_ORIENTATION, listed: VideoMode) -> VideoMode {
    match orientation {
        DMDO_90 | DMDO_270 => VideoMode {
            width: listed.height,
            height: listed.width,
            ..listed
        },
        _ => listed,
    }
}

/// Each mode once, of the most colors any mode has, largest and quickest
/// first; `listed` gives each with its bits per pixel.
fn distinct_modes(listed: Vec<(u32, VideoMode)>) -> Vec<VideoMode> {
    let deepest = listed.iter().map(|(bits, _)| *bits).max().unwrap_or(0);
    let mut modes: Vec<VideoMode> = { listed.into_iter() }
        .filter(|(bits, mode)| *bits == deepest && mode.width > 0 && mode.hertz > 1)
        .map(|(_, mode)| mode)
        .collect();
    modes.sort_unstable_by(|one, other| {
        let area = |mode: &VideoMode| u64::from(mode.width) * u64::from(mode.height);
        (area(other), other.width, other.hertz).cmp(&(area(one), one.width, one.hertz))
    });
    modes.dedup();
    modes
}

/// A refresh rate in whole hertz, as Windows lists them: 59.94 is 59.
fn whole_hertz(rate: DISPLAYCONFIG_RATIONAL) -> u32 {
    match rate.Denominator {
        0 => 0,
        denominator => rate.Numerator / denominator,
    }
}

/// The exact refresh rate Windows lists as `hertz`: a rate one below one
/// of television is that rate on the wire, 60000/1001 for 59.
fn rational(hertz: u32) -> DISPLAYCONFIG_RATIONAL {
    let broadcast = BROADCAST_RATES.contains(&(hertz + 1)) && !BROADCAST_RATES.contains(&hertz);
    match broadcast {
        true => DISPLAYCONFIG_RATIONAL {
            Numerator: (hertz + 1) * 1000,
            Denominator: 1001,
        },
        false => DISPLAYCONFIG_RATIONAL {
            Numerator: hertz,
            Denominator: 1,
        },
    }
}

/// How a refresh rate listed as `hertz` is said: with its fraction where
/// it is one of television.
pub fn hertz_text(hertz: u32) -> String {
    let rate = rational(hertz);
    match rate.Denominator {
        1 => format!("{hertz} Hz"),
        denominator => format!(
            "{:.2} Hz",
            f64::from(rate.Numerator) / f64::from(denominator)
        ),
    }
}

/// The scale packet: what a source may be set to, in steps from the scale
/// Windows recommends.
#[repr(C)]
#[derive(Clone, Copy)]
struct ScaleGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    least: i32,
    current: i32,
    most: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ScaleSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    steps: i32,
}

/// The scale given by steps from the recommended one, if the steps lie
/// within the scales Windows has: anything else says this Windows does
/// not read scales this way.
fn scale_of(least: i32, current: i32, most: i32) -> Option<Scale> {
    let recommended = usize::try_from(least.checked_neg()?).ok()?;
    let at = |steps: i32| {
        SCALES
            .get(usize::try_from(recommended as i32 + steps).ok()?)
            .copied()
    };
    if least > 0 || current < least || current > most {
        return None;
    }
    let choices = (least..=most).map(at).collect::<Option<Vec<u32>>>()?;
    Some(Scale {
        current: at(current)?,
        recommended: *SCALES.get(recommended)?,
        choices,
    })
}

/// The scale of `source`; `None` where it cannot be read.
fn scale_of_source(source: SourceId) -> Option<Scale> {
    let mut packet = ScaleGet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: GET_SCALE,
            size: size_of::<ScaleGet>() as u32,
            adapterId: source.luid(),
            id: source.id,
        },
        least: 0,
        current: 0,
        most: 0,
    };
    // SAFETY: the packet starts with the header, is of the size it says
    // and outlives the call; a Windows that does not know the type
    // refuses it without writing.
    let read = unsafe { DisplayConfigGetDeviceInfo(&raw mut packet.header) };
    if read != ERROR_SUCCESS.0 as i32 {
        return None;
    }
    scale_of(packet.least, packet.current, packet.most)
}

/// Sets `source` to the current scale of `scale`, and checks that it took.
fn set_scale(source: SourceId, scale: &Scale) -> Result<()> {
    let (Some(wanted), Some(recommended)) = (
        SCALES.iter().position(|&known| known == scale.current),
        SCALES.iter().position(|&known| known == scale.recommended),
    ) else {
        bail!("Windows has no scale of {}%", scale.current);
    };
    let packet = ScaleSet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: SET_SCALE,
            size: size_of::<ScaleSet>() as u32,
            adapterId: source.luid(),
            id: source.id,
        },
        steps: wanted as i32 - recommended as i32,
    };
    // SAFETY: the packet starts with the header, is of the size it says
    // and outlives the call, which only reads it.
    let status = unsafe { DisplayConfigSetDeviceInfo(&raw const packet.header) };
    let taken = scale_of_source(source).map(|now| now.current);
    if status != ERROR_SUCCESS.0 as i32 || taken != Some(scale.current) {
        bail!("Windows did not take the scale of {}%", scale.current);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scale_is_counted_from_the_one_windows_recommends() {
        let scale = scale_of(-2, 1, 4).unwrap();
        assert_eq!(scale.recommended, 150);
        assert_eq!(scale.current, 175);
        assert_eq!(scale.choices, vec![100, 125, 150, 175, 200, 225, 250]);
        assert_eq!(scale_of(0, 0, 0).unwrap().choices, vec![100]);
    }

    #[test]
    fn a_scale_packet_out_of_range_is_not_trusted() {
        assert_eq!(scale_of(1, 1, 2), None);
        assert_eq!(scale_of(-2, 3, 2), None);
        assert_eq!(scale_of(-2, 0, 20), None);
        assert_eq!(scale_of(-20, 0, 0), None);
    }

    #[test]
    fn rates_of_television_are_set_with_their_fraction() {
        let rate = rational(59);
        assert_eq!((rate.Numerator, rate.Denominator), (60000, 1001));
        let rate = rational(60);
        assert_eq!((rate.Numerator, rate.Denominator), (60, 1));
        let rate = rational(143);
        assert_eq!((rate.Numerator, rate.Denominator), (144000, 1001));
        assert_eq!(whole_hertz(rational(59)), 59);
        assert_eq!(whole_hertz(rational(165)), 165);
        assert_eq!(hertz_text(59), "59.94 Hz");
        assert_eq!(hertz_text(75), "75 Hz");
    }

    #[test]
    fn the_number_of_a_display_is_read_from_its_name() {
        assert_eq!(display_number("\\\\.\\DISPLAY1"), Some(1));
        assert_eq!(display_number("\\\\.\\DISPLAY12"), Some(12));
        assert_eq!(display_number(""), None);
    }

    #[test]
    fn modes_are_listed_once_largest_first() {
        let mode = |width, height, hertz| VideoMode {
            width,
            height,
            hertz,
        };
        let listed = vec![
            (32, mode(1920, 1080, 60)),
            (16, mode(2560, 1440, 60)),
            (32, mode(1920, 1080, 60)),
            (32, mode(1920, 1080, 144)),
            (32, mode(1280, 720, 60)),
            (32, mode(1920, 1200, 60)),
        ];
        assert_eq!(
            distinct_modes(listed),
            vec![
                mode(1920, 1200, 60),
                mode(1920, 1080, 144),
                mode(1920, 1080, 60),
                mode(1280, 720, 60),
            ]
        );
    }

    #[test]
    fn the_modes_of_a_display_turned_upright_are_kept_unturned() {
        let mode = |width, height| VideoMode {
            width,
            height,
            hertz: 60,
        };
        assert_eq!(unturned(DMDO_90, mode(1080, 1920)), mode(1920, 1080));
        assert_eq!(unturned(DMDO_270, mode(1080, 1920)), mode(1920, 1080));
        assert_eq!(
            unturned(DEVMODE_DISPLAY_ORIENTATION(0), mode(1920, 1080)),
            mode(1920, 1080)
        );
        assert_eq!(
            unturned(DEVMODE_DISPLAY_ORIENTATION(2), mode(1920, 1080)),
            mode(1920, 1080)
        );
    }

    #[test]
    fn only_a_desktop_of_its_own_at_the_origin_is_the_main_display() {
        let mut main = DisplayOutput::test_output();
        main.active = true;
        main.source = Some(SourceId {
            adapter: (0, 0),
            id: 0,
        });
        main.mode = VideoMode {
            width: 1920,
            height: 1080,
            hertz: 60,
        };
        assert!(main.is_primary());
        // Switched on in a draft, with no desktop yet.
        let mut switched_on = DisplayOutput::test_output();
        switched_on.active = true;
        assert!(!switched_on.is_primary());
        let mut mirror = main.clone();
        mirror.mirror_of = Some(main.id);
        assert!(!mirror.is_primary());
        main.position = (1920, 0);
        assert!(!main.is_primary());
    }

    #[test]
    fn a_turned_display_is_higher_than_wide() {
        assert_eq!(Rotation::Portrait.turn((1920, 1080)), (1080, 1920));
        assert_eq!(Rotation::LandscapeFlipped.turn((1920, 1080)), (1920, 1080));
        for rotation in Rotation::ALL {
            assert_eq!(Rotation::of(rotation.raw()), rotation);
        }
    }
}
