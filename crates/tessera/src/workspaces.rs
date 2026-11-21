// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Workspaces: either the virtual desktops of Windows, or workspaces of
//! Tessera's own, which hide the windows of every workspace but the one
//! shown.
//!
//! A window belongs to the workspace it was first seen on. Windows that
//! are hidden are written down before they are hidden, so that a Tessera
//! that was terminated shows them again when it is next started; one that
//! quits shows them itself.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;

use crate::config::config_dir;
use crate::platform::{
    AppWindow, AppWindowId, Hotkey, Hotkeys, Modifiers, VirtualDesktops, WindowEvents,
};
use crate::tiling::listed;

/// How often it is checked whether something happened to the windows.
const CHECK_INTERVAL: Duration = Duration::from_millis(100);

/// How often the windows are looked at even though nothing was heard of
/// them.
const LOOK_INTERVAL: Duration = Duration::from_secs(1);

/// How long a window may take to hide. One that is still, or again, seen
/// on another workspace after that showed itself, and stays.
const HIDING_TIME: Duration = Duration::from_millis(1500);

/// The fewest and the most workspaces of its own Tessera keeps.
const FEWEST: usize = 2;
const MOST: usize = 9;

/// The file of the configuration directory that lists the hidden windows.
const RECORD: &str = "hidden-windows";

/// What is added to the place of a workspace for the number of the key
/// combination that sends a window there.
const SENDING: u32 = 16;

/// What the configuration says about workspaces.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacesConfig {
    /// How many workspaces of its own Tessera keeps. Without it, the
    /// workspaces are the virtual desktops of Windows.
    own: Option<usize>,
    /// The programs whose windows belong to a workspace, by the file name
    /// of the executable; every program when empty.
    #[serde(default)]
    only: Vec<String>,
    /// The programs whose windows are seen on every workspace.
    #[serde(default)]
    except: Vec<String>,
    /// The keys held with a digit to go to the workspace of that number,
    /// and with Shift as well to send the window in front there.
    keys: Option<String>,
}

impl WorkspacesConfig {
    /// How many workspaces of its own Tessera keeps, if it keeps any. A
    /// number that is not usable is reported and counts as none.
    fn count(&self) -> Option<usize> {
        let count = self.own?;
        if (FEWEST..=MOST).contains(&count) {
            return Some(count);
        }
        report!(
            "`workspaces.own` is not a number from {FEWEST} to {MOST}, so the workspaces are the virtual desktops of Windows"
        );
        None
    }

    /// The key combinations for `count` workspaces: one for each to go
    /// there and, unless Shift is among the keys held already, one to send
    /// a window there.
    fn hotkeys(&self, count: usize) -> Vec<Hotkey> {
        let Some(keys) = self.keys.as_deref().filter(|_| count > 0) else {
            return Vec::new();
        };
        let Some(modifiers) = Modifiers::parse(keys) else {
            report!(
                "`workspaces.keys` names keys among alt, ctrl, shift and win joined by +, not `{keys}`"
            );
            return Vec::new();
        };
        let sending = Modifiers {
            shift: true,
            ..modifiers
        };
        (0..count.min(MOST) as u32)
            .flat_map(|place| {
                let key = b'1' + place as u8;
                let going = Hotkey {
                    id: place,
                    modifiers,
                    key,
                };
                let sent = Hotkey {
                    id: SENDING + place,
                    modifiers: sending,
                    key,
                };
                [Some(going), (!modifiers.shift).then_some(sent)]
            })
            .flatten()
            .collect()
    }
}

/// Which window belongs to which workspace, and which are hidden.
#[derive(Default)]
struct Homes {
    /// How many workspaces there are; none when they are not kept here.
    count: usize,
    current: usize,
    /// The workspace of every window that was seen.
    home: HashMap<AppWindowId, usize>,
    /// The windows that were asked to hide, and when.
    hidden: HashMap<AppWindowId, Instant>,
    /// The window that was last in front, workspace by workspace.
    fronts: Vec<Option<AppWindowId>>,
}

/// What is to be done to the windows after a change.
#[derive(Debug, Default, PartialEq, Eq)]
struct Change {
    hide: Vec<AppWindowId>,
    show: Vec<AppWindowId>,
    /// The window to bring to the front.
    front: Option<AppWindowId>,
}

impl Homes {
    fn new(count: usize) -> Self {
        Self {
            count,
            fronts: vec![None; count],
            ..Self::default()
        }
    }

    /// Takes in the windows that are seen right now and the one in front.
    /// Whether the hidden windows changed is returned.
    fn see(&mut self, seen: &[AppWindowId], front: Option<AppWindowId>, now: Instant) -> bool {
        let mut changed = false;
        for &id in seen {
            let home = *self.home.entry(id).or_insert(self.current);
            if home == self.current {
                continue;
            }
            let hiding = self
                .hidden
                .get(&id)
                .is_some_and(|&since| now.duration_since(since) < HIDING_TIME);
            if !hiding {
                // The window showed itself, or was never hidden.
                self.home.insert(id, self.current);
                changed |= self.hidden.remove(&id).is_some();
            }
        }
        if let Some(front) = front
            && self.home.get(&front) == Some(&self.current)
        {
            self.fronts[self.current] = Some(front);
        }
        changed
    }

    /// Forgets the windows of which `exists` says that they are gone.
    /// Whether the hidden windows changed is returned.
    fn forget(&mut self, exists: impl Fn(AppWindowId) -> bool) -> bool {
        let hidden = self.hidden.len();
        self.home.retain(|&id, _| exists(id));
        self.hidden.retain(|&id, _| exists(id));
        for front in &mut self.fronts {
            *front = front.filter(|&id| exists(id));
        }
        hidden != self.hidden.len()
    }

    /// Goes to the workspace at `index`.
    fn go(&mut self, index: usize, now: Instant) -> Change {
        if index >= self.count || index == self.current {
            return Change::default();
        }
        let mut change = Change::default();
        for (&id, &home) in &self.home {
            if home == self.current && !self.hidden.contains_key(&id) {
                change.hide.push(id);
            } else if home == index {
                change.show.push(id);
            }
        }
        for &id in &change.hide {
            self.hidden.insert(id, now);
        }
        for id in &change.show {
            self.hidden.remove(id);
        }
        self.current = index;
        change.front = self.fronts[index];
        change
    }

    /// Sends the window that was last in front on the workspace shown to
    /// the workspace at `index`.
    fn send(&mut self, index: usize, now: Instant) -> Change {
        if index >= self.count || index == self.current {
            return Change::default();
        }
        let Some(id) = self.fronts[self.current].take() else {
            return Change::default();
        };
        self.home.insert(id, index);
        self.hidden.insert(id, now);
        self.fronts[index] = Some(id);
        Change {
            hide: vec![id],
            ..Change::default()
        }
    }
}

/// Keeps the workspaces.
pub struct Workspaces {
    config: WorkspacesConfig,
    homes: Homes,
    /// Counts the tasks that were started to watch the windows; only the
    /// latest goes on.
    task: u64,
}

impl Global for Workspaces {}

impl Workspaces {
    /// Shows the windows a Tessera before this one left hidden, and keeps
    /// workspaces as `config` says.
    pub fn start(config: &WorkspacesConfig, cx: &mut App) {
        Self::recover();
        cx.set_global(Self {
            config: WorkspacesConfig::default(),
            homes: Homes::default(),
            task: 0,
        });
        cx.on_app_quit(|cx| {
            cx.global_mut::<Self>().show_all();
            async {}
        })
        .detach();
        let presses = Hotkeys::presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(pressed) = presses.recv().await {
                let answered = cx.update(|cx| match pressed.checked_sub(SENDING) {
                    Some(place) => Self::send_front(place as usize, cx),
                    None => Self::show(pressed as usize, cx),
                });
                if answered.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self::configure(config, cx);
    }

    /// Takes over a configuration that was read again, if it says
    /// something else about workspaces than the one before. Every window
    /// is shown and starts on the first workspace again.
    pub fn configure(config: &WorkspacesConfig, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.config == *config {
            return;
        }
        this.show_all();
        this.config = config.clone();
        this.homes = Homes::new(config.count().unwrap_or(0));
        this.task += 1;
        let task = this.task;
        match Hotkeys::set(config.hotkeys(this.homes.count)) {
            Ok(refused) if refused.is_empty() => {}
            Ok(refused) => {
                let digits: Vec<String> = refused
                    .iter()
                    .map(|id| match id.checked_sub(SENDING) {
                        Some(place) => format!("Shift+{}", place + 1),
                        None => (id + 1).to_string(),
                    })
                    .collect();
                report!(
                    "Windows did not give the workspace keys with {}: another program holds them",
                    digits.join(", ")
                );
            }
            Err(error) => report!("the workspaces have no keys: {error:#}"),
        }
        if this.homes.count == 0 {
            return;
        }
        if !WindowEvents::watch() {
            report!("Windows does not report window changes, so workspaces follow them late");
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = None;
            let mut looked = Instant::now();
            loop {
                let going = cx.update(|cx| {
                    let this = cx.global_mut::<Self>();
                    if this.task != task {
                        return false;
                    }
                    let now = WindowEvents::generation();
                    if seen != Some(now) || looked.elapsed() >= LOOK_INTERVAL {
                        (seen, looked) = (Some(now), Instant::now());
                        this.look();
                    }
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(CHECK_INTERVAL).await;
            }
        })
        .detach();
    }

    /// How many workspaces there are and which one is shown: Tessera's
    /// own, or else the virtual desktops of Windows.
    pub fn now(cx: &App) -> VirtualDesktops {
        let homes = &cx.global::<Self>().homes;
        if homes.count == 0 {
            return VirtualDesktops::now();
        }
        VirtualDesktops {
            count: homes.count,
            current: homes.current,
        }
    }

    /// Goes to the workspace at `index`.
    pub fn show(index: usize, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.homes.count == 0 {
            if !VirtualDesktops::now().show(index) {
                report!("Windows did not take the keys that go to another desktop");
            }
            return;
        }
        this.look();
        let change = this.homes.go(index, Instant::now());
        this.apply(change);
    }

    /// Sends the window that was last in front to the workspace at
    /// `index`. The virtual desktops of Windows offer no way to do that.
    pub fn send_front(index: usize, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if this.homes.count == 0 {
            return;
        }
        this.look();
        let change = this.homes.send(index, Instant::now());
        this.apply(change);
    }

    /// Takes in what windows there are.
    fn look(&mut self) {
        let seen: Vec<AppWindowId> = AppWindow::enumerate()
            .into_iter()
            .filter(|window| {
                listed(
                    &self.config.only,
                    &self.config.except,
                    window.executable.as_deref(),
                )
            })
            .map(|window| window.id)
            .collect();
        let mut changed = self.homes.forget(AppWindowId::exists);
        changed |= self
            .homes
            .see(&seen, AppWindowId::foreground(), Instant::now());
        if changed {
            self.write_record();
        }
    }

    /// Hides and shows windows as `change` says. What is hidden is
    /// written down first.
    fn apply(&mut self, change: Change) {
        self.write_record();
        for id in change.hide {
            id.hide();
        }
        for id in change.show {
            id.unhide();
        }
        if let Some(front) = change.front {
            front.activate();
        }
    }

    /// Shows every window that is hidden.
    fn show_all(&mut self) {
        for (id, _) in self.homes.hidden.drain() {
            id.unhide();
        }
        self.write_record();
    }

    fn record() -> Option<PathBuf> {
        Some(config_dir()?.join(RECORD))
    }

    /// Writes down which windows are hidden: the window and its process,
    /// line by line. No file is kept while none is.
    fn write_record(&self) {
        let Some(path) = Self::record() else {
            return;
        };
        if self.homes.hidden.is_empty() {
            let _ = fs::remove_file(&path);
            return;
        }
        let lines: String = self
            .homes
            .hidden
            .keys()
            .map(|id| format!("{} {}\n", id.raw(), id.process()))
            .collect();
        if let Err(error) = fs::write(&path, lines) {
            report!(
                "{} was not written, so windows hidden now stay hidden if Tessera is terminated: {error}",
                path.display()
            );
        }
    }

    /// Shows the windows that are written down as hidden, if they are
    /// still the same windows and still hidden.
    fn recover() {
        let Some(path) = Self::record() else {
            return;
        };
        let Ok(lines) = fs::read_to_string(&path) else {
            return;
        };
        for (raw, process) in lines.lines().filter_map(parse) {
            let id = AppWindowId::from_raw(raw);
            if id.exists() && id.process() == process && !id.is_shown() {
                id.unhide();
            }
        }
        let _ = fs::remove_file(&path);
    }
}

/// The window and the process a line of the record names.
fn parse(line: &str) -> Option<(isize, u32)> {
    let (window, process) = line.split_once(' ')?;
    Some((window.parse().ok()?, process.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: isize) -> AppWindowId {
        AppWindowId::from_raw(raw)
    }

    fn sorted(mut ids: Vec<AppWindowId>) -> Vec<isize> {
        ids.sort_by_key(|id| id.raw());
        ids.into_iter().map(AppWindowId::raw).collect()
    }

    #[test]
    fn keys_go_to_each_workspace_and_send_there_with_shift() {
        let config = |keys: &str| WorkspacesConfig {
            keys: Some(keys.to_owned()),
            ..WorkspacesConfig::default()
        };
        let keys = config("alt").hotkeys(2);
        let seen: Vec<_> = keys
            .iter()
            .map(|key| (key.id, key.key, key.modifiers.alt, key.modifiers.shift))
            .collect();
        assert_eq!(
            seen,
            [
                (0, b'1', true, false),
                (SENDING, b'1', true, true),
                (1, b'2', true, false),
                (SENDING + 1, b'2', true, true),
            ]
        );
        assert_eq!(config("alt+shift").hotkeys(3).len(), 3);
        assert!(config("alt").hotkeys(0).is_empty());
        assert!(config("alt+2").hotkeys(3).is_empty());
        assert!(WorkspacesConfig::default().hotkeys(3).is_empty());
    }

    #[test]
    fn going_to_a_workspace_hides_the_windows_of_the_one_left() {
        let start = Instant::now();
        let mut homes = Homes::new(3);
        homes.see(&[id(1), id(2)], Some(id(2)), start);
        let change = homes.go(1, start);
        assert_eq!(sorted(change.hide), [1, 2]);
        assert!(change.show.is_empty());
        assert_eq!(change.front, None);

        homes.see(&[id(3)], Some(id(3)), start);
        let back = homes.go(0, start);
        assert_eq!(sorted(back.hide), [3]);
        assert_eq!(sorted(back.show), [1, 2]);
        assert_eq!(back.front, Some(id(2)));
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [3]);
    }

    #[test]
    fn a_window_still_hiding_keeps_its_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], None, start);
        homes.go(1, start);
        assert!(!homes.see(&[id(1)], None, start + HIDING_TIME / 2));
        assert_eq!(homes.home[&id(1)], 0);
    }

    #[test]
    fn a_window_that_shows_itself_joins_the_workspace_shown() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], None, start);
        homes.go(1, start);
        assert!(homes.see(&[id(1)], None, start + HIDING_TIME));
        assert_eq!(homes.home[&id(1)], 1);
        assert!(homes.hidden.is_empty());
    }

    #[test]
    fn the_window_in_front_is_sent_to_another_workspace() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        let change = homes.send(1, start);
        assert_eq!(sorted(change.hide), [1]);
        assert_eq!(homes.send(1, start), Change::default());
        let there = homes.go(1, start);
        assert_eq!(sorted(there.hide), [2]);
        assert_eq!(sorted(there.show), [1]);
        assert_eq!(there.front, Some(id(1)));
    }

    #[test]
    fn windows_that_are_gone_are_forgotten() {
        let start = Instant::now();
        let mut homes = Homes::new(2);
        homes.see(&[id(1), id(2)], Some(id(1)), start);
        homes.go(1, start);
        assert!(homes.forget(|window| window == id(2)));
        assert_eq!(sorted(homes.hidden.keys().copied().collect()), [2]);
        assert_eq!(homes.fronts[0], None);
    }

    #[test]
    fn workspaces_out_of_reach_change_nothing() {
        let mut homes = Homes::new(2);
        homes.see(&[id(1)], Some(id(1)), Instant::now());
        assert_eq!(homes.go(2, Instant::now()), Change::default());
        assert_eq!(homes.go(0, Instant::now()), Change::default());
        assert_eq!(homes.send(0, Instant::now()), Change::default());
    }

    #[test]
    fn the_count_of_workspaces_is_checked() {
        let config = |text: &str| toml::from_str::<WorkspacesConfig>(text).unwrap();
        assert_eq!(config("").count(), None);
        assert_eq!(config("own = 4").count(), Some(4));
        assert_eq!(config("own = 1").count(), None);
        assert_eq!(config("own = 10").count(), None);
    }

    #[test]
    fn a_line_of_the_record_names_a_window_and_its_process() {
        assert_eq!(parse("1234 56"), Some((1234, 56)));
        assert_eq!(parse("1234"), None);
        assert_eq!(parse("window 56"), None);
    }
}
