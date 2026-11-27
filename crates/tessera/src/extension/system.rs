// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The state of the machine and of the shell as extensions are told of
//! it, and whether it changed.
//!
//! Extensions run on threads of their own, which may wait for Windows:
//! the media, the meters and the network are read there when asked, and
//! a reading is kept for a moment so that several extensions share it.
//! What only the thread that draws knows, the workspaces and the colors of
//! the theme, that thread writes down here while an extension may ask
//! ([`publish`]). While a widget watches the media or the meters, a thread
//! of its own reads them every second and counts the changes, which the
//! widgets compare with what they saw ([`generation`]).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{App, Hsla, Rgba};

use super::access::WatchKind;
use crate::platform::{
    Battery, Media, MediaCommand, MemoryLoad, Network, NetworkTraffic, NowPlaying, ProcessorLoad,
    WindowEvents,
};
use crate::theme::Theme;
use crate::workspaces::Workspaces;

/// For how long a reading of the media is given again.
const MEDIA_KEPT: Duration = Duration::from_millis(500);

/// For how long a reading of the meters is given again.
const METERS_KEPT: Duration = Duration::from_secs(1);

/// How often the thread that watches reads what is watched.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the media player plays, as it was read at most half a second
/// ago.
pub fn media() -> Result<Option<NowPlaying>, String> {
    type Reading = Result<Option<NowPlaying>, String>;
    static READ: Mutex<Option<(Instant, Reading)>> = Mutex::new(None);
    let mut read = lock(&READ);
    if let Some((at, reading)) = read.as_ref()
        && at.elapsed() < MEDIA_KEPT
    {
        return reading.clone();
    }
    let reading = Media::now_playing(None);
    *read = Some((Instant::now(), reading.clone()));
    reading
}

/// What `media-control` asks of the player of [`media`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaDeed {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
}

/// Asks the player Windows considers current to do `deed`.
pub fn media_control(deed: MediaDeed) -> Result<(), String> {
    let command = match deed {
        MediaDeed::Play => MediaCommand::Play,
        MediaDeed::Pause => MediaCommand::Pause,
        MediaDeed::Toggle => MediaCommand::Toggle,
        MediaDeed::Next => MediaCommand::Next,
        MediaDeed::Previous => MediaCommand::Previous,
    };
    Media::send(None, command)?;
    // What plays now is to be read again.
    bump(WatchKind::Media);
    Ok(())
}

/// The meters of the machine, as one reading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meters {
    pub cpu: Option<f32>,
    pub memory: Option<MemoryLoad>,
    pub battery: Option<Battery>,
    /// Bytes per second received and sent.
    pub traffic: Option<(f64, f64)>,
}

/// What reads the meters; the processor and the network count since the
/// reading before.
#[derive(Default)]
struct MeterReader {
    processor: ProcessorLoad,
    network: NetworkTraffic,
    last: Option<(Instant, Meters)>,
}

static METERS: Mutex<Option<MeterReader>> = Mutex::new(None);

/// The meters as they were read at most a second ago.
pub fn meters() -> Meters {
    let mut reader = lock(&METERS);
    let reader = reader.get_or_insert_with(MeterReader::default);
    if let Some((at, meters)) = reader.last
        && at.elapsed() < METERS_KEPT
    {
        return meters;
    }
    let meters = Meters {
        cpu: reader.processor.read(),
        memory: MemoryLoad::read(),
        battery: Battery::read(),
        traffic: reader.network.read(),
    };
    reader.last = Some((Instant::now(), meters));
    meters
}

/// The connection Windows prefers, read again only after Windows said
/// that it changed.
pub fn network() -> Option<Network> {
    static READ: Mutex<Option<(u64, Option<Network>)>> = Mutex::new(None);
    Network::watch();
    let changes = Network::changes();
    let mut read = lock(&READ);
    if let Some((seen, network)) = read.as_ref()
        && *seen == changes
    {
        return network.clone();
    }
    let network = Network::current();
    *read = Some((changes, network.clone()));
    network
}

/// The workspaces of the display the pointer is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceState {
    pub count: u32,
    pub current: u32,
    pub names: Vec<String>,
    /// Whether they are Tessera's own rather than the desktops of Windows.
    pub tessera: bool,
}

/// What the thread that draws writes down for extensions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellState {
    pub workspaces: WorkspaceState,
    /// The workspace each window lives on, by the number of the window.
    pub homes: HashMap<isize, u32>,
    /// The colors of the theme as `#rrggbb`, in the order of the record
    /// `palette` of the interface.
    pub palette: [String; 7],
}

impl ShellState {
    /// What the shell shows now.
    pub fn of(cx: &App) -> Self {
        let desktops = Workspaces::now(None, cx);
        let residents = Workspaces::residents(None, cx);
        let mut homes = HashMap::new();
        for (place, windows) in residents.iter().flatten().enumerate() {
            for resident in windows {
                homes.insert(resident.id.raw(), place as u32);
            }
        }
        let names = (0..desktops.count)
            .map(|index| Workspaces::label(index, cx))
            .collect();
        let colors = cx.global::<Theme>().colors;
        Self {
            workspaces: WorkspaceState {
                count: desktops.count as u32,
                current: desktops.current as u32,
                names,
                tessera: residents.is_some(),
            },
            homes,
            palette: [
                colors.surface_container,
                colors.surface_container_highest,
                colors.on_surface,
                colors.on_surface_variant,
                colors.outline,
                colors.primary,
                colors.secondary_container,
            ]
            .map(hex),
        }
    }
}

/// `color` as `#rrggbb`.
pub fn hex(color: Hsla) -> String {
    let Rgba { r, g, b, .. } = color.to_rgb();
    let byte = |part: f32| (part.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02x}{:02x}{:02x}", byte(r), byte(g), byte(b))
}

static SHELL: Mutex<Option<ShellState>> = Mutex::new(None);

/// Writes down what the shell shows, and counts a change of the
/// workspaces or of the theme. It is for the thread that draws.
pub fn publish(state: ShellState) {
    let mut shell = lock(&SHELL);
    let before = shell.take().unwrap_or_default();
    if before.workspaces != state.workspaces || before.homes != state.homes {
        bump(WatchKind::Workspaces);
    }
    if before.palette != state.palette {
        bump(WatchKind::Theme);
    }
    *shell = Some(state);
}

/// What the shell showed when it was last written down; `None` before
/// that.
pub fn shell() -> Option<ShellState> {
    lock(&SHELL).clone()
}

/// The changes counted for each kind, but those of the windows and the
/// network, which Windows counts.
static CHANGES: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

fn place(kind: WatchKind) -> usize {
    WatchKind::ALL
        .iter()
        .position(|known| *known == kind)
        .unwrap_or_default()
}

/// Counts a change of `kind`.
fn bump(kind: WatchKind) {
    CHANGES[place(kind)].fetch_add(1, Ordering::Relaxed);
}

/// A number that changes whenever `kind` changed. That the windows
/// change is known only once [`start_watching`] asked Windows to tell.
pub fn generation(kind: WatchKind) -> u64 {
    match kind {
        WatchKind::Windows => WindowEvents::generation(),
        WatchKind::Network => {
            Network::watch();
            Network::changes()
        }
        _ => CHANGES[place(kind)].load(Ordering::Relaxed),
    }
}

/// Asks Windows to tell of what happens to windows, for widgets that
/// watch them. It is for the thread that draws, which handles messages.
pub fn start_watching(kinds: &[WatchKind]) {
    if kinds.contains(&WatchKind::Windows) && !WindowEvents::watch() {
        report!("Windows does not report window changes, so extensions learn of them late");
    }
}

/// How many widgets watch each kind the watching thread reads.
#[derive(Default)]
struct Watchers {
    media: usize,
    meters: usize,
    running: bool,
}

static WATCHERS: Mutex<Watchers> = Mutex::new(Watchers {
    media: 0,
    meters: 0,
    running: false,
});

/// Has the media and the meters read every second for as long as it is
/// kept, as far as `kinds` hold them.
pub struct Watch {
    media: bool,
    meters: bool,
}

impl Watch {
    pub fn new(kinds: &[WatchKind]) -> Self {
        let watch = Self {
            media: kinds.contains(&WatchKind::Media),
            meters: kinds.contains(&WatchKind::Meters),
        };
        let mut watchers = lock(&WATCHERS);
        watchers.media += usize::from(watch.media);
        watchers.meters += usize::from(watch.meters);
        let wanted = watchers.media + watchers.meters > 0;
        if wanted && !watchers.running {
            watchers.running = true;
            let started = thread::Builder::new()
                .name("extension watch".to_owned())
                .spawn(read_on);
            if let Err(error) = started {
                watchers.running = false;
                report!("extensions are not told when the media or the meters change: {error}");
            }
        }
        watch
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut watchers = lock(&WATCHERS);
        watchers.media -= usize::from(self.media);
        watchers.meters -= usize::from(self.meters);
    }
}

/// Reads what is watched every second, counting each change, until
/// nothing is watched.
fn read_on() {
    let mut playing = None;
    let mut measured = None;
    loop {
        let (media, meters) = {
            let mut watchers = lock(&WATCHERS);
            if watchers.media + watchers.meters == 0 {
                watchers.running = false;
                return;
            }
            (watchers.media > 0, watchers.meters > 0)
        };
        if media {
            // What plays, not how far: that changes every second.
            let now = self::media()
                .map(|now| now.map(|now| (now.app, now.title, now.artist, now.playing)));
            if playing.as_ref() != Some(&now) {
                playing = Some(now);
                bump(WatchKind::Media);
            }
        }
        if meters {
            let now = self::meters();
            if measured != Some(now) {
                measured = Some(now);
                bump(WatchKind::Meters);
            }
        }
        thread::sleep(WATCH_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_written_as_hex() {
        let color: Hsla = gpui::rgb(0xadd28e).into();
        assert_eq!(hex(color), "#add28e");
        assert_eq!(hex(gpui::black()), "#000000");
    }

    #[test]
    fn a_change_of_the_shell_is_counted_where_it_is() {
        let workspaces = generation(WatchKind::Workspaces);
        let theme = generation(WatchKind::Theme);
        let mut state = ShellState::default();
        publish(state.clone());
        state.workspaces.count = 3;
        publish(state.clone());
        assert!(generation(WatchKind::Workspaces) > workspaces);
        let seen = generation(WatchKind::Workspaces);
        publish(state.clone());
        assert_eq!(generation(WatchKind::Workspaces), seen);
        state.palette[0] = "#ffffff".to_owned();
        publish(state);
        assert!(generation(WatchKind::Theme) > theme);
        assert_eq!(shell().unwrap().palette[0], "#ffffff");
    }
}
