// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Night light, two ways.
//!
//! [`WindowsNightLight`] is the one of Windows. Windows keeps its state in
//! the CloudStore of the account, as records whose layout it does not
//! document; what is done here follows what others found out about them.
//! A record is `43 42 01 00 0A 02 01 00 2A 06`, the time it was written as
//! a varint of seconds since 1970, `2A 2B 0E`, the size of what follows as
//! a varint, and that, which starts with `43 42 01 00` again. In the state,
//! that part has the field `10 00` right after its start while night light
//! is on and lacks it while it is off. In the settings, the warmth is the
//! field `CF 28` with the color temperature in kelvins as a zigzag varint.
//! A record that is not laid out so is not changed. Windows applies what
//! is written when it notices the change, which its settings and
//! Explorer's quick settings make it do; whether it notices without
//! Explorer is not known.
//!
//! [`GammaNight`] is Tessera's own: it multiplies the gamma ramp of every
//! display with the color of a light of the temperature asked for, which
//! works without Explorer. Windows refuses ramps far from the straight line
//! unless the value `GdiIcmGammaRange` = 256 (a DWORD) is set under
//! `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ICM`; a refused ramp
//! is reported and the warmest one accepted stays. Windows also puts the
//! ramps back when a display changes its mode, so the night light is
//! asked again from time to time.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Graphics::Gdi::{
    CreateDCW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICEW, DeleteDC, EnumDisplayDevicesW,
    HDC,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, RRF_RT_REG_BINARY, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};
use windows::core::{PCWSTR, w};

/// The coldest and the warmest light night light gives, in kelvins:
/// daylight, and what the settings of Windows go down to.
pub const COLDEST: u32 = 6500;
pub const WARMEST: u32 = 1200;

const STATE_KEY: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.bluelightreduction.bluelightreductionstate\\windows.data.bluelightreduction.bluelightreductionstate"
);
const SETTINGS_KEY: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.bluelightreduction.settings\\windows.data.bluelightreduction.settings"
);
const DATA: PCWSTR = w!("Data");

/// The largest record that is read.
const MOST_BYTES: usize = 4096;

/// The record of the CloudStore, as described at the top.
mod record {
    use super::*;

    /// How a record starts, up to the time it was written.
    const PREFIX: [u8; 10] = [0x43, 0x42, 0x01, 0x00, 0x0A, 0x02, 0x01, 0x00, 0x2A, 0x06];
    /// What stands between the time and the size of the content.
    const CONTENT_TAG: [u8; 3] = [0x2A, 0x2B, 0x0E];
    /// How the content starts.
    const HEADER: [u8; 4] = [0x43, 0x42, 0x01, 0x00];
    /// The field that is there while night light is on.
    const ON_FIELD: [u8; 2] = [0x10, 0x00];
    /// The field of the color temperature in the settings.
    const TEMPERATURE_TAG: [u8; 2] = [0xCF, 0x28];

    /// Where the parts of a record are.
    struct Parts {
        stamp: Range<usize>,
        size: Range<usize>,
        content: Range<usize>,
    }

    /// The number written as a varint at `at`, and how many bytes it
    /// takes.
    pub(super) fn varint(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
        let mut value = 0u64;
        for (index, byte) in bytes.get(at..)?.iter().enumerate().take(10) {
            value |= u64::from(byte & 0x7F) << (7 * index);
            if byte & 0x80 == 0 {
                return Some((value, index + 1));
            }
        }
        None
    }

    /// `value` written as a varint.
    pub(super) fn encode(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let low = (value & 0x7F) as u8;
            value >>= 7;
            if value == 0 {
                bytes.push(low);
                return bytes;
            }
            bytes.push(low | 0x80);
        }
    }

    fn parts(record: &[u8]) -> Option<Parts> {
        if !record.starts_with(&PREFIX) {
            return None;
        }
        let (_, stamp_length) = varint(record, PREFIX.len())?;
        let stamp = PREFIX.len()..PREFIX.len() + stamp_length;
        if record.get(stamp.end..stamp.end + CONTENT_TAG.len())? != CONTENT_TAG {
            return None;
        }
        let size_at = stamp.end + CONTENT_TAG.len();
        let (size, size_length) = varint(record, size_at)?;
        let start = size_at + size_length;
        let content = start..start.checked_add(usize::try_from(size).ok()?)?;
        if content.end > record.len() || !record[content.clone()].starts_with(&HEADER) {
            return None;
        }
        Some(Parts {
            stamp,
            size: size_at..start,
            content,
        })
    }

    /// Writes the time `now` (seconds since 1970) into `record` in place
    /// of the time it holds, or one second later than that where `now` is
    /// not later; `None` where the time would not take as many bytes.
    fn stamped(mut record: Vec<u8>, now: u64) -> Option<Vec<u8>> {
        let stamp = parts(&record)?.stamp;
        let (old, length) = varint(&record, stamp.start)?;
        let later = old.checked_add(1)?;
        let bytes = [now.max(later), later]
            .into_iter()
            .map(encode)
            .find(|bytes| bytes.len() == length)?;
        record.splice(stamp, bytes);
        Some(record)
    }

    /// Whether the state `record` says night light is on; `None` for a
    /// record that is not laid out as expected.
    pub fn is_on(record: &[u8]) -> Option<bool> {
        let parts = parts(record)?;
        let content = &record[parts.content];
        Some(content[HEADER.len()..].starts_with(&ON_FIELD))
    }

    /// The state `record` with night light `on`, written at `now`.
    pub fn switched(record: &[u8], on: bool, now: u64) -> Option<Vec<u8>> {
        let parts = parts(record)?;
        let was_on = is_on(record)?;
        let mut changed = record.to_vec();
        let field = parts.content.start + HEADER.len();
        let size = parts.content.len();
        // From the end to the start, so that what is changed later is
        // still where it was found.
        let size = match (was_on, on) {
            (false, true) => {
                changed.splice(field..field, ON_FIELD);
                size + ON_FIELD.len()
            }
            (true, false) => {
                changed.drain(field..field + ON_FIELD.len());
                size - ON_FIELD.len()
            }
            _ => size,
        };
        changed.splice(parts.size, encode(size as u64));
        let changed = stamped(changed, now)?;
        (is_on(&changed)? == on).then_some(changed)
    }

    /// Where the color temperature is in the settings `record`, and what
    /// it is.
    fn temperature_at(record: &[u8]) -> Option<(Range<usize>, u32)> {
        let parts = parts(record)?;
        let content = parts.content.clone();
        (content.start..content.end.saturating_sub(TEMPERATURE_TAG.len())).find_map(|at| {
            if !record[at..content.end].starts_with(&TEMPERATURE_TAG) {
                return None;
            }
            let value_at = at + TEMPERATURE_TAG.len();
            let (value, length) = varint(&record[..content.end], value_at)?;
            // A zigzag varint: a number that is not negative is twice
            // itself.
            let kelvins = u32::try_from(value / 2).ok()?;
            let plausible = value.is_multiple_of(2) && (WARMEST..=COLDEST).contains(&kelvins);
            plausible.then_some((value_at..value_at + length, kelvins))
        })
    }

    /// The color temperature the settings `record` give, in kelvins.
    pub fn temperature(record: &[u8]) -> Option<u32> {
        temperature_at(record).map(|(_, kelvins)| kelvins)
    }

    /// The settings `record` with the color temperature `kelvins`, written
    /// at `now`; `None` where the record holds none to replace.
    pub fn with_temperature(record: &[u8], kelvins: u32, now: u64) -> Option<Vec<u8>> {
        let (at, _) = temperature_at(record)?;
        let kelvins = kelvins.clamp(WARMEST, COLDEST);
        let bytes = encode(u64::from(kelvins) * 2);
        if bytes.len() != at.len() {
            return None;
        }
        let mut changed = record.to_vec();
        changed.splice(at, bytes);
        stamped(changed, now)
    }
}

/// The seconds since 1970.
fn now() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |since| since.as_secs())
}

/// The value `Data` of the CloudStore key `key`; `None` where there is
/// none, as before night light was ever opened in the settings.
fn read_record(key: PCWSTR) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; MOST_BYTES];
    let mut size = bytes.len() as u32;
    // SAFETY: the out-parameters outlive the call, and `size` is the size
    // of the buffer.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            DATA,
            RRF_RT_REG_BINARY,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    bytes.truncate(size as usize);
    Some(bytes)
}

fn write_record(key: PCWSTR, record: &[u8]) -> Result<(), String> {
    // SAFETY: the bytes outlive the call and are as many as is said.
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key,
            DATA,
            REG_BINARY.0,
            Some(record.as_ptr().cast()),
            record.len() as u32,
        )
    };
    match status.is_ok() {
        true => Ok(()),
        false => Err(format!(
            "the night light of Windows was not written: {}",
            std::io::Error::from_raw_os_error(status.0 as i32)
        )),
    }
}

/// What the night light of Windows is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowsNight {
    pub on: bool,
    /// The color temperature in kelvins, where the settings hold one.
    pub kelvins: Option<u32>,
}

/// The night light of Windows, through the CloudStore of the account.
pub struct WindowsNightLight;

impl WindowsNightLight {
    /// What it is; where Windows keeps no state Tessera can read, why, as
    /// a sentence.
    pub fn read() -> Result<WindowsNight, &'static str> {
        let state = read_record(STATE_KEY)
            .ok_or("Windows keeps no state of night light yet; open it once in the settings")?;
        let on = record::is_on(&state)
            .ok_or("Windows keeps the state of night light in a way Tessera does not know")?;
        let kelvins = read_record(SETTINGS_KEY).and_then(|settings| record::temperature(&settings));
        Ok(WindowsNight { on, kelvins })
    }

    /// Switches it on or off, and checks that the state reads as written.
    pub fn switch(on: bool) -> Result<(), String> {
        let state = read_record(STATE_KEY).ok_or("Windows keeps no state of night light yet")?;
        let switched = record::switched(&state, on, now())
            .ok_or("the state of night light that Windows keeps is laid out in an unknown way")?;
        write_record(STATE_KEY, &switched)?;
        let written = read_record(STATE_KEY).and_then(|state| record::is_on(&state));
        match written == Some(on) {
            true => Ok(()),
            false => Err("the night light of Windows did not keep what was written".to_owned()),
        }
    }

    /// Sets its color temperature, in kelvins.
    pub fn set_kelvins(kelvins: u32) -> Result<(), String> {
        let settings =
            read_record(SETTINGS_KEY).ok_or("Windows keeps no settings of night light yet")?;
        let changed = record::with_temperature(&settings, kelvins, now()).ok_or(
            "the settings of night light hold no strength yet; set it once in the settings of Windows",
        )?;
        write_record(SETTINGS_KEY, &changed)
    }
}

/// The share of red, green and blue in the light of a black body at
/// `kelvins`, against that of daylight at [`COLDEST`]: the approximation
/// of Tanner Helland, which is close enough for a screen.
pub fn tint(kelvins: u32) -> [f32; 3] {
    fn rgb(kelvins: f32) -> [f32; 3] {
        let t = kelvins / 100.;
        let red = match t <= 66. {
            true => 255.,
            false => 329.698_73 * (t - 60.).powf(-0.133_204_76),
        };
        let green = match t <= 66. {
            true => 99.470_8 * t.ln() - 161.119_57,
            false => 288.122_17 * (t - 60.).powf(-0.075_514_85),
        };
        let blue = match t {
            t if t >= 66. => 255.,
            t if t <= 19. => 0.,
            t => 138.517_73 * (t - 10.).ln() - 305.044_8,
        };
        [red, green, blue].map(|channel| channel.clamp(0., 255.))
    }
    let kelvins = kelvins.clamp(WARMEST, COLDEST) as f32;
    let (warm, day) = (rgb(kelvins), rgb(COLDEST as f32));
    [0, 1, 2].map(|channel| (warm[channel] / day[channel]).clamp(0., 1.))
}

/// How many entries a gamma ramp has for each of red, green and blue.
const RAMP: usize = 256;

/// A gamma ramp: the red, then the green, then the blue entries.
type Ramp = [u16; RAMP * 3];

/// The ramp that changes nothing.
fn straight() -> Ramp {
    let mut ramp = [0; RAMP * 3];
    for (index, entry) in ramp.iter_mut().enumerate() {
        *entry = ((index % RAMP) * 257) as u16;
    }
    ramp
}

/// `original` with each of its channels multiplied by its share of
/// `tint`.
fn tinted(original: &Ramp, tint: [f32; 3]) -> Ramp {
    let mut ramp = *original;
    for (index, entry) in ramp.iter_mut().enumerate() {
        let share = tint[index / RAMP];
        *entry = (f32::from(*entry) * share).round() as u16;
    }
    ramp
}

/// Whether `ramp` looks tinted already rather than calibrated: its blue
/// ends well below its red, as one that Tessera left behind when it was
/// stopped without putting the original back.
fn looks_tinted(ramp: &Ramp) -> bool {
    let red = f32::from(ramp[RAMP - 1]);
    let blue = f32::from(ramp[RAMP * 3 - 1]);
    red == 0. || blue < red * 0.9
}

/// Whether `tint` changes nothing.
fn is_daylight(tint: [f32; 3]) -> bool {
    tint.iter().all(|share| (share - 1.).abs() < 0.001)
}

/// The steps of the fade from one tint to another, and the time between
/// two of them.
const FADE_STEPS: u32 = 20;
const FADE_STEP: Duration = Duration::from_millis(40);

/// What Tessera did to the ramps of the displays.
struct Gamma {
    /// The ramps the displays had before Tessera changed them, by the name
    /// of the device.
    originals: HashMap<Vec<u16>, Ramp>,
    /// The tint the displays were last given.
    shown: [f32; 3],
    /// The displays that refused a ramp, reported once each.
    refused: HashSet<Vec<u16>>,
    /// Whether the originals were put back for good, as Tessera quits.
    closed: bool,
}

static GAMMA: LazyLock<Mutex<Gamma>> = LazyLock::new(|| {
    Mutex::new(Gamma {
        originals: HashMap::new(),
        shown: [1.; 3],
        refused: HashSet::new(),
        closed: false,
    })
});

/// Tessera's own night light, which tints the gamma ramps of the displays.
/// The ramps are changed on a thread of its own, in a short fade.
pub struct GammaNight;

impl GammaNight {
    /// Tints the displays as a light of `kelvins` would, or gives them
    /// back their own colors with `None`. Asking again for what is shown
    /// gives it again to displays that lost it.
    pub fn show(kelvins: Option<u32>) {
        static ORDERS: OnceLock<Mutex<Sender<Option<u32>>>> = OnceLock::new();
        let orders = ORDERS.get_or_init(|| {
            let (sender, receiver) = mpsc::channel();
            let started = thread::Builder::new()
                .name("tessera-night-light".to_owned())
                .spawn(move || fade(&receiver));
            if let Err(error) = started {
                report!("night light cannot tint the displays: {error}");
            }
            Mutex::new(sender)
        });
        let sent = orders.lock().map(|orders| orders.send(kelvins));
        if !matches!(sent, Ok(Ok(()))) {
            report!("night light cannot tint the displays: its thread is gone");
        }
    }

    /// Gives every display that Tessera tinted its own ramp back, at once
    /// and for good: for when Tessera quits.
    pub fn restore() {
        let Ok(mut gamma) = GAMMA.lock() else {
            return;
        };
        gamma.closed = true;
        for (device, original) in &gamma.originals {
            if let Some(dc) = DisplayDc::open(device) {
                dc.set(original);
            }
        }
    }
}

/// What the thread of [`GammaNight`] does: fades to each tint asked for,
/// taking the latest when several were asked meanwhile.
fn fade(orders: &Receiver<Option<u32>>) {
    let Ok(mut wanted) = orders.recv() else {
        return;
    };
    'orders: loop {
        while let Ok(later) = orders.try_recv() {
            wanted = later;
        }
        let goal = wanted.map_or([1.; 3], tint);
        let from = GAMMA.lock().map_or([1.; 3], |gamma| gamma.shown);
        let steps = if from == goal { 1 } else { FADE_STEPS };
        for step in 1..=steps {
            if let Ok(later) = orders.try_recv() {
                wanted = later;
                continue 'orders;
            }
            let share = step as f32 / steps as f32;
            let tint =
                [0, 1, 2].map(|channel| from[channel] + (goal[channel] - from[channel]) * share);
            show_tint(tint);
            if step < steps {
                thread::sleep(FADE_STEP);
            }
        }
        match orders.recv() {
            Ok(next) => wanted = next,
            Err(_) => return,
        }
    }
}

/// Gives every display `tint`.
fn show_tint(tint: [f32; 3]) {
    let Ok(mut gamma) = GAMMA.lock() else {
        return;
    };
    if gamma.closed {
        return;
    }
    let daylight = is_daylight(tint);
    for device in displays() {
        let Some(dc) = DisplayDc::open(&device) else {
            continue;
        };
        let original = match gamma.originals.get(&device) {
            Some(original) => *original,
            None => {
                let read = dc.get().filter(|ramp| !looks_tinted(ramp));
                let original = read.unwrap_or_else(straight);
                gamma.originals.insert(device.clone(), original);
                original
            }
        };
        let ramp = match daylight {
            true => original,
            false => tinted(&original, tint),
        };
        if !dc.set(&ramp) && gamma.refused.insert(device.clone()) {
            report!(
                "Windows refused the colors of night light on {}; a warmer light needs the DWORD \
                 GdiIcmGammaRange = 256 under HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\ICM",
                String::from_utf16_lossy(&device)
            );
        }
    }
    gamma.shown = tint;
}

/// The device names of the displays that are part of the desktop, each
/// ending in its NUL.
fn displays() -> Vec<Vec<u16>> {
    let mut found = Vec::new();
    for index in 0.. {
        let mut device = DISPLAY_DEVICEW {
            cb: size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: the structure says its own size and outlives the call.
        let listed = unsafe { EnumDisplayDevicesW(None, index, &raw mut device, 0) };
        if !listed.as_bool() {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP
            == DISPLAY_DEVICE_ATTACHED_TO_DESKTOP
        {
            let end = device.DeviceName.iter().position(|&unit| unit == 0);
            let end = end.unwrap_or(device.DeviceName.len() - 1);
            found.push(device.DeviceName[..=end].to_vec());
        }
    }
    found
}

/// A device context of one display, deleted when dropped.
struct DisplayDc(HDC);

impl DisplayDc {
    /// That of the display `device`, a name ending in its NUL.
    fn open(device: &[u16]) -> Option<Self> {
        if device.last() != Some(&0) {
            return None;
        }
        // SAFETY: the name ends in its NUL and outlives the call.
        let dc = unsafe { CreateDCW(w!("DISPLAY"), PCWSTR(device.as_ptr()), None, None) };
        (!dc.is_invalid()).then_some(Self(dc))
    }

    fn get(&self) -> Option<Ramp> {
        let mut ramp = [0u16; RAMP * 3];
        // SAFETY: the ramp is as large as the function fills and outlives
        // the call.
        let read = unsafe { GetDeviceGammaRamp(self.0, ramp.as_mut_ptr().cast()) };
        read.as_bool().then_some(ramp)
    }

    fn set(&self, ramp: &Ramp) -> bool {
        // SAFETY: the ramp is as large as the function reads and outlives
        // the call.
        unsafe { SetDeviceGammaRamp(self.0, ramp.as_ptr().cast()) }.as_bool()
    }
}

impl Drop for DisplayDc {
    fn drop(&mut self) {
        // SAFETY: the context was created by this value and is deleted once.
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::record::*;
    use super::*;

    /// A state record of Windows 10 with night light off, as it was found
    /// in the registry, written on 2021-05-23.
    const OFF: [u8; 41] = [
        0x43, 0x42, 0x01, 0x00, 0x0A, 0x02, 0x01, 0x00, 0x2A, 0x06, 0xB7, 0xE2, 0xA9, 0x85, 0x06,
        0x2A, 0x2B, 0x0E, 0x13, 0x43, 0x42, 0x01, 0x00, 0xD0, 0x0A, 0x02, 0xC6, 0x14, 0xB5, 0x9B,
        0xFA, 0xC8, 0xDE, 0x9C, 0xB4, 0xEC, 0x01, 0x00, 0x00, 0x00, 0x00,
    ];

    /// When [`OFF`] was written.
    const WRITTEN: u64 = 1_621_782_839;

    /// A record that holds `content`, written at `stamp`.
    fn record_of(stamp: u64, content: &[u8]) -> Vec<u8> {
        let mut record = vec![0x43, 0x42, 0x01, 0x00, 0x0A, 0x02, 0x01, 0x00, 0x2A, 0x06];
        record.extend(encode(stamp));
        record.extend([0x2A, 0x2B, 0x0E]);
        record.extend(encode(content.len() as u64));
        record.extend(content);
        record.extend([0, 0, 0]);
        record
    }

    #[test]
    fn a_varint_is_read_as_it_is_written() {
        for value in [0, 1, 127, 128, 300, 16_383, 16_384, WRITTEN, u64::MAX] {
            let bytes = encode(value);
            assert_eq!(varint(&bytes, 0), Some((value, bytes.len())));
        }
        assert_eq!(varint(&OFF, 10), Some((WRITTEN, 5)));
        assert_eq!(varint(&[0x80, 0x80], 0), None);
    }

    #[test]
    fn the_state_of_windows_says_whether_night_light_is_on() {
        assert_eq!(is_on(&OFF), Some(false));
        let on = switched(&OFF, true, WRITTEN + 60).unwrap();
        assert_eq!(is_on(&on), Some(true));
        assert_eq!(on.len(), OFF.len() + 2);
        // The size of the content grew by the field, which follows its
        // start.
        assert_eq!(on[18], 0x15);
        assert_eq!(&on[19..25], &[0x43, 0x42, 0x01, 0x00, 0x10, 0x00]);
        assert_eq!(varint(&on, 10), Some((WRITTEN + 60, 5)));
        // Switched off again, it is what it was but for the time.
        let off = switched(&on, false, WRITTEN + 120).unwrap();
        assert_eq!(&off[..10], &OFF[..10]);
        assert_eq!(&off[15..], &OFF[15..]);
        assert_eq!(is_on(&off), Some(false));
    }

    #[test]
    fn a_record_is_written_later_than_it_was_even_with_a_clock_behind() {
        let on = switched(&OFF, true, 5).unwrap();
        assert_eq!(varint(&on, 10), Some((WRITTEN + 1, 5)));
        // Switching to what it is changes only the time.
        let same = switched(&OFF, false, WRITTEN + 9).unwrap();
        assert_eq!(&same[15..], &OFF[15..]);
    }

    #[test]
    fn a_record_laid_out_otherwise_is_left_alone() {
        assert_eq!(is_on(&[]), None);
        assert_eq!(is_on(&OFF[..20]), None);
        let mut strange = OFF;
        strange[19] = 0x44;
        assert_eq!(is_on(&strange), None);
        assert_eq!(switched(&strange, true, WRITTEN), None);
    }

    #[test]
    fn the_settings_of_windows_give_the_warmth_of_the_light() {
        // A temperature of 3998 K among the other fields of the settings.
        let content = [
            0x43, 0x42, 0x01, 0x00, 0x02, 0x01, 0xCA, 0x14, 0x0E, 0x15, 0x00, 0xCF, 0x28, 0xBC,
            0x3E, 0xCA, 0x32, 0x0E, 0x10, 0x2E, 0x36, 0x00, 0x00,
        ];
        let settings = record_of(WRITTEN, &content);
        assert_eq!(temperature(&settings), Some(3998));
        let warmer = with_temperature(&settings, 2700, WRITTEN + 1).unwrap();
        assert_eq!(temperature(&warmer), Some(2700));
        assert_eq!(warmer.len(), settings.len());
        // Asked for more than there is, it gives the warmest.
        let warmest = with_temperature(&settings, 100, WRITTEN + 2).unwrap();
        assert_eq!(temperature(&warmest), Some(WARMEST));
        // Settings without the field have nothing to give or to change.
        let bare = record_of(WRITTEN, &content[..11]);
        assert_eq!(temperature(&bare), None);
        assert_eq!(with_temperature(&bare, 2700, WRITTEN), None);
    }

    #[test]
    fn daylight_tints_nothing_and_a_warm_light_takes_blue_first() {
        assert!(is_daylight(tint(COLDEST)));
        assert!(is_daylight(tint(9000)));
        let [red, green, blue] = tint(3400);
        assert_eq!(red, 1.);
        assert!(blue < green && green < 1.);
        let [_, _, warmest] = tint(WARMEST);
        assert!(warmest < blue);
    }

    #[test]
    fn a_ramp_is_tinted_channel_by_channel() {
        let ramp = tinted(&straight(), [1., 0.5, 0.]);
        assert_eq!(ramp[RAMP - 1], 65_535);
        assert_eq!(ramp[RAMP * 2 - 1], 32_768);
        assert_eq!(ramp[RAMP * 3 - 1], 0);
        assert_eq!(ramp[RAMP + 2], 257);
        assert!(looks_tinted(&ramp));
        assert!(!looks_tinted(&straight()));
    }

    /// Reads night light of the machine it runs on; changes nothing.
    #[test]
    #[ignore = "asks the night light of this machine"]
    fn the_machine_says_what_its_night_light_is() {
        println!("{:?}", WindowsNightLight::read());
        println!("{} displays", displays().len());
    }
}
