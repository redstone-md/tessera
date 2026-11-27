// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Window rules: each `[[rule]]` of the configuration names windows by the
//! file name of their program, the packaged application they belong to,
//! their title or their window class, and
//! says what is done to them when they are first seen: whether they are
//! tiled, the workspace and the display they go to, where they are placed
//! and how large, whether they are maximized, kept above the others or
//! drawn see-through, and whether Windows draws their thin border. A rule
//! with `apply = "always"` does it again whenever the window changes, as
//! far as it differs.
//!
//! A name is matched as a pattern in which `*` stands for any text and
//! `?` for any one character, whatever the case of the letters; one that
//! begins with `re:` is a regular expression instead. Every rule that
//! names a window applies, in the order of the file, so that a later one
//! has the last word.
//!
//! What the rules say of tiling, of the dock and of the window switcher
//! is asked for with [`Rules::of`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use toml_edit::{Array, Table, value};

use crate::config::edit::ConfigFile;
use crate::platform::{AppWindow, AppWindowId, Display, ScreenRect, WindowEvents};
use crate::snap::{Zone, carried};
use crate::tiling::Tiler;
use crate::workspaces::Workspaces;

/// How often it is checked whether something happened to the windows.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// For how long after a window is first seen it is placed again where
/// its rules put it, if it moved: many programs put their window where
/// they had it last right after showing it.
const SETTLING: Duration = Duration::from_secs(1);

/// How far a window may be from where its rules put it and still be
/// there, in physical pixels.
const STILL_THERE: i32 = 4;

/// The least and most opacity a rule may ask for.
const OPACITIES: (f32, f32) = (0.1, 1.);

/// A name of windows as a rule gives it: a pattern with `*` and `?`, or a
/// regular expression after `re:`.
#[derive(Clone, Debug)]
pub struct Pattern {
    text: String,
    matcher: Matcher,
}

#[derive(Clone, Debug)]
enum Matcher {
    /// The pattern in lowercase.
    Glob(String),
    Regex(Regex),
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

impl Pattern {
    /// The pattern `text` is, or why it is none.
    pub fn parse(text: &str) -> Result<Self, String> {
        let matcher = match text.strip_prefix("re:") {
            Some(expression) => {
                let built = RegexBuilder::new(expression).case_insensitive(true).build();
                Matcher::Regex(
                    built.map_err(|error| format!("`{text}` is no regular expression: {error}"))?,
                )
            }
            None => Matcher::Glob(text.to_lowercase()),
        };
        Ok(Self {
            text: text.to_owned(),
            matcher,
        })
    }

    /// Whether `name` is one the pattern names.
    pub fn matches(&self, name: &str) -> bool {
        match &self.matcher {
            Matcher::Glob(pattern) => glob(pattern, &name.to_lowercase()),
            Matcher::Regex(expression) => expression.is_match(name),
        }
    }
}

/// Whether all of `text` is what `pattern` describes, `*` standing for any
/// text and `?` for any one character.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) =
        (pattern.chars().collect(), text.chars().collect());
    let (mut at, mut from) = (0, 0);
    // Where the last star was, and where in the text it began to stand.
    let mut star: Option<(usize, usize)> = None;
    while from < text.len() {
        match pattern.get(at) {
            Some('*') => {
                star = Some((at, from));
                at += 1;
            }
            Some(&one) if one == '?' || one == text[from] => {
                at += 1;
                from += 1;
            }
            _ => match star {
                // The star stands for one character more.
                Some((place, began)) => {
                    at = place + 1;
                    from = began + 1;
                    star = Some((place, began + 1));
                }
                None => return false,
            },
        }
    }
    pattern[at.min(pattern.len())..]
        .iter()
        .all(|&rest| rest == '*')
}

/// When a rule does what it says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Apply {
    /// When the window is first seen.
    #[default]
    Initially,
    /// Whenever something happens to the window, as far as it differs.
    Always,
}

/// Where a rule puts a window: in the middle of the work area, or with its
/// top left corner at shares of it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(try_from = "PositionText")]
pub enum Position {
    Center,
    At(f32, f32),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PositionText {
    Name(String),
    At([f32; 2]),
}

impl TryFrom<PositionText> for Position {
    type Error = String;

    fn try_from(text: PositionText) -> Result<Self, String> {
        let share = |value: f32| value.is_finite() && (0. ..=1.).contains(&value);
        match text {
            PositionText::Name(name) if name == "center" => Ok(Self::Center),
            PositionText::At([x, y]) if share(x) && share(y) => Ok(Self::At(x, y)),
            _ => Err(
                "`position` is `center` or `[x, y]` in shares of the work area, from 0 to 1"
                    .to_owned(),
            ),
        }
    }
}

/// A `[[rule]]`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleConfig {
    /// The file name of the program of the window, as `notepad.exe`. For
    /// an application of the Universal Windows Platform it is that of the
    /// frame host, ApplicationFrameHost.exe, the same for all of them.
    exe: Option<Pattern>,
    /// The identifier of the packaged (Store) application the window
    /// belongs to, its AppUserModelID, as
    /// `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`.
    app: Option<Pattern>,
    /// What the title bar of the window says.
    title: Option<Pattern>,
    /// The window class the program made the window of.
    class: Option<Pattern>,
    /// Whether the window is left out of the tiles.
    float: Option<bool>,
    /// Whether the window is tiled whatever `[tiling]` says of it, or not.
    tile: Option<bool>,
    /// The workspace of Tessera's own the window goes to, from 1.
    workspace: Option<usize>,
    /// The display the window goes to, counted from 1 from the left.
    display: Option<usize>,
    position: Option<Position>,
    /// `[width, height]` in shares of the work area.
    size: Option<[f32; 2]>,
    maximize: Option<bool>,
    /// Whether the window is kept above the others.
    above: Option<bool>,
    /// How opaque the window is drawn, from 0.1 to 1.
    opacity: Option<f32>,
    /// Whether Windows draws the thin border of the window.
    border: Option<bool>,
    /// Whether the dock leaves the window out.
    skip_dock: Option<bool>,
    /// Whether the window switcher leaves the window out.
    skip_switcher: Option<bool>,
    #[serde(default)]
    apply: Apply,
}

/// What is known of a window to match it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// The file name of its program.
    pub exe: Option<String>,
    /// The packaged application it belongs to.
    pub app: Option<String>,
    pub title: String,
    pub class: String,
}

impl Named {
    fn of(window: &AppWindow) -> Self {
        let exe = window.executable.as_deref().and_then(Path::file_name);
        Self {
            exe: exe.map(|name| name.to_string_lossy().into_owned()),
            app: window.app_id.clone(),
            title: window.id.title(),
            class: window.id.class(),
        }
    }
}

impl RuleConfig {
    /// Why the rule cannot be used, if it cannot.
    fn fault(&self) -> Option<&'static str> {
        let share = |value: f32| value.is_finite() && value > 0. && value <= 1.;
        let names = [&self.exe, &self.app, &self.title, &self.class];
        if names.iter().all(|name| name.is_none()) {
            return Some("it names no windows: it needs `exe`, `app`, `title` or `class`");
        }
        if self
            .opacity
            .is_some_and(|opacity| !(OPACITIES.0..=OPACITIES.1).contains(&opacity))
        {
            return Some("its `opacity` is not from 0.1 to 1");
        }
        if self
            .size
            .is_some_and(|[width, height]| !share(width) || !share(height))
        {
            return Some("its `size` is not `[width, height]` in shares of the work area");
        }
        if self.workspace == Some(0) || self.display == Some(0) {
            return Some("its `workspace` and `display` are counted from 1");
        }
        None
    }

    /// Whether the rule names the window `named`.
    fn matches(&self, named: &Named) -> bool {
        let fits = |pattern: &Option<Pattern>, name: Option<&str>| match pattern {
            None => true,
            Some(pattern) => name.is_some_and(|name| pattern.matches(name)),
        };
        fits(&self.exe, named.exe.as_deref())
            && fits(&self.app, named.app.as_deref())
            && fits(&self.title, Some(&named.title))
            && fits(&self.class, Some(&named.class))
    }
}

/// What the rules that name a window say of it, a later rule having the
/// last word.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Effective {
    /// Whether the window is tiled: `Some(false)` for one that floats.
    pub tile: Option<bool>,
    pub workspace: Option<usize>,
    pub display: Option<usize>,
    pub position: Option<Position>,
    pub size: Option<[f32; 2]>,
    pub maximize: Option<bool>,
    pub above: Option<bool>,
    pub opacity: Option<f32>,
    pub border: Option<bool>,
    pub skip_dock: Option<bool>,
    pub skip_switcher: Option<bool>,
}

impl Effective {
    /// What `rules` say of the window `named`.
    fn of<'a>(rules: impl IntoIterator<Item = &'a RuleConfig>, named: &Named) -> Self {
        let mut effective = Self::default();
        for rule in rules.into_iter().filter(|rule| rule.matches(named)) {
            take(&mut effective.tile, rule.float.map(|float| !float));
            take(&mut effective.tile, rule.tile);
            take(&mut effective.workspace, rule.workspace);
            take(&mut effective.display, rule.display);
            take(&mut effective.position, rule.position);
            take(&mut effective.size, rule.size);
            take(&mut effective.maximize, rule.maximize);
            take(&mut effective.above, rule.above);
            take(&mut effective.opacity, rule.opacity);
            take(&mut effective.border, rule.border);
            take(&mut effective.skip_dock, rule.skip_dock);
            take(&mut effective.skip_switcher, rule.skip_switcher);
        }
        effective
    }

    /// What this says that `before` did not: each of its parts that
    /// differs from that of `before`, and nothing else.
    fn newly_said(&self, before: &Self) -> Self {
        fn new<T: Copy + PartialEq>(now: Option<T>, before: Option<T>) -> Option<T> {
            now.filter(|_| now != before)
        }
        Self {
            tile: new(self.tile, before.tile),
            workspace: new(self.workspace, before.workspace),
            display: new(self.display, before.display),
            position: new(self.position, before.position),
            size: new(self.size, before.size),
            maximize: new(self.maximize, before.maximize),
            above: new(self.above, before.above),
            opacity: new(self.opacity, before.opacity),
            border: new(self.border, before.border),
            skip_dock: new(self.skip_dock, before.skip_dock),
            skip_switcher: new(self.skip_switcher, before.skip_switcher),
        }
    }

    /// Whether the window is to be placed at all.
    fn places(&self) -> bool {
        self.display.is_some() || self.position.is_some() || self.size.is_some()
    }
}

/// Has `into` be `from` where there is something in `from`.
fn take<T>(into: &mut Option<T>, from: Option<T>) {
    if from.is_some() {
        *into = from;
    }
}

/// Where a window of `frame` goes in `work` when it is put at `position`
/// and made `size` large, either of which may be missing: it keeps what
/// is not given, and fits in `work`. `None` when neither is given.
pub fn ruled_frame(
    frame: ScreenRect,
    work: ScreenRect,
    position: Option<Position>,
    size: Option<[f32; 2]>,
) -> Option<ScreenRect> {
    if position.is_none() && size.is_none() {
        return None;
    }
    let (room_width, room_height) = (work.right - work.left, work.bottom - work.top);
    let (width, height) = match size {
        Some([width, height]) => (
            (room_width as f32 * width).round() as i32,
            (room_height as f32 * height).round() as i32,
        ),
        None => (frame.right - frame.left, frame.bottom - frame.top),
    };
    let (width, height) = (
        width.clamp(1, room_width.max(1)),
        height.clamp(1, room_height.max(1)),
    );
    let (left, top) = match position {
        Some(Position::Center) => (
            work.left + (room_width - width) / 2,
            work.top + (room_height - height) / 2,
        ),
        Some(Position::At(x, y)) => (
            work.left + (room_width as f32 * x).round() as i32,
            work.top + (room_height as f32 * y).round() as i32,
        ),
        None => (frame.left, frame.top),
    };
    let left = left.clamp(work.left, work.right - width);
    let top = top.clamp(work.top, work.bottom - height);
    Some(ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    })
}

/// A window the rules have seen.
struct Seen {
    named: Named,
    effective: Effective,
    /// What the rules that apply always say of it.
    always: Effective,
    first: Instant,
    /// Where the rules put it, while it may still move by itself.
    placed: Option<ScreenRect>,
}

/// Keeps the rules and applies them.
#[derive(Default)]
pub struct Rules {
    rules: Vec<RuleConfig>,
    seen: HashMap<AppWindowId, Seen>,
    /// Counts the tasks that were started to watch the windows; only the
    /// latest goes on.
    task: u64,
}

impl Global for Rules {}

impl Rules {
    /// Applies `rules` from now on to the windows that appear.
    pub fn start(rules: &[RuleConfig], cx: &mut App) {
        cx.set_global(Self::default());
        Self::configure(rules, cx);
    }

    /// Takes over the rules of a configuration that was read again, if they
    /// changed. The windows that are open already, when Tessera starts or
    /// the rules change, count as seen: only the rules that apply always
    /// change them.
    pub fn configure(rules: &[RuleConfig], cx: &mut App) {
        let usable: Vec<RuleConfig> = { rules.iter() }
            .filter(|rule| match rule.fault() {
                Some(fault) => {
                    report!("a `[[rule]]` is left out: {fault}");
                    false
                }
                None => true,
            })
            .cloned()
            .collect();
        let this = cx.global_mut::<Self>();
        if this.rules == usable && this.task > 0 {
            return;
        }
        this.rules = usable;
        this.task += 1;
        let task = this.task;
        // The windows there are now count as seen with the new rules.
        let windows = AppWindow::enumerate();
        let rules = this.rules.clone();
        for window in &windows {
            let named = Named::of(window);
            let seen = this.seen.entry(window.id).or_insert_with(|| Seen {
                named: named.clone(),
                effective: Effective::default(),
                always: Effective::default(),
                first: Instant::now(),
                placed: None,
            });
            seen.effective = Effective::of(&rules, &named);
            seen.always = Effective::of(always(&rules), &named);
            seen.named = named;
        }
        Self::hand_over_tiling(cx);
        if rules.is_empty() {
            return;
        }
        if !WindowEvents::watch() {
            report!("Windows does not report window changes, so rules apply late");
        }
        cx.spawn(async move |cx: &mut AsyncApp| {
            let mut seen = None;
            loop {
                let going = cx.update(|cx| {
                    if cx.global::<Self>().task != task {
                        return false;
                    }
                    let now = WindowEvents::generation();
                    let settling =
                        { cx.global::<Self>().seen.values() }.any(|seen| seen.placed.is_some());
                    if seen != Some(now) || settling {
                        seen = Some(now);
                        Self::look(cx);
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

    /// What the rules say of `window`.
    #[allow(
        dead_code,
        reason = "for the dock and the window switcher, which leave windows out"
    )]
    pub fn of(window: &AppWindow, cx: &App) -> Effective {
        let Some(this) = cx.try_global::<Self>() else {
            return Effective::default();
        };
        match this.seen.get(&window.id) {
            Some(seen) => seen.effective,
            None => Effective::of(&this.rules, &Named::of(window)),
        }
    }

    /// Takes in the windows there are, and applies the rules to the new
    /// ones and those that apply always to the others.
    fn look(cx: &mut App) {
        let windows = AppWindow::enumerate();
        let this = cx.global_mut::<Self>();
        this.seen.retain(|id, _| id.exists());
        let now = Instant::now();
        let mut new = Vec::new();
        let mut again = Vec::new();
        let rules = std::mem::take(&mut this.rules);
        // A window is put back only while it settles, also one that is no
        // longer listed, as one hidden on another workspace is not.
        for seen in this.seen.values_mut() {
            if now.duration_since(seen.first) >= SETTLING {
                seen.placed = None;
            }
        }
        let mut late = Vec::new();
        for window in &windows {
            match this.seen.get_mut(&window.id) {
                None => {
                    let named = Named::of(window);
                    let effective = Effective::of(&rules, &named);
                    let always = Effective::of(always(&rules), &named);
                    this.seen.insert(
                        window.id,
                        Seen {
                            named,
                            effective,
                            always,
                            first: now,
                            placed: None,
                        },
                    );
                    new.push(window.clone());
                }
                Some(seen) => {
                    // A title changes, and with it what the rules say. Many
                    // programs give their window its title a moment after
                    // showing it: while it settles, what a rule now says
                    // of it that none said before is done as at first sight.
                    let title = window.id.title();
                    if title != seen.named.title {
                        seen.named.title = title;
                        let before = seen.effective;
                        seen.effective = Effective::of(&rules, &seen.named);
                        seen.always = Effective::of(always(&rules), &seen.named);
                        let newly = seen.effective.newly_said(&before);
                        let settling = now.duration_since(seen.first) < SETTLING;
                        if settling && newly != Effective::default() {
                            late.push((window.clone(), newly));
                        }
                    }
                    if seen.always != Effective::default() || seen.placed.is_some() {
                        again.push(window.clone());
                    }
                }
            }
        }
        this.rules = rules;
        Self::hand_over_tiling(cx);
        for window in new {
            let effective = cx
                .global::<Self>()
                .seen
                .get(&window.id)
                .map(|seen| seen.effective);
            if let Some(effective) = effective {
                Self::first_sight(&window, &effective, cx);
            }
        }
        for (window, newly) in late {
            Self::first_sight(&window, &newly, cx);
        }
        for window in again {
            Self::once_more(&window, cx);
        }
    }

    /// Tells the tiling which windows the rules tile and which they leave
    /// alone, and which have no border, so that it does not give them one.
    fn hand_over_tiling(cx: &mut App) {
        let seen = &cx.global::<Self>().seen;
        let ruled: HashMap<AppWindowId, bool> = { seen.iter() }
            .filter_map(|(&id, seen)| Some((id, seen.effective.tile?)))
            .collect();
        let borderless: HashSet<AppWindowId> = { seen.iter() }
            .filter(|(_, seen)| seen.effective.border == Some(false))
            .map(|(&id, _)| id)
            .collect();
        Tiler::rule(ruled, borderless, cx);
    }

    /// Does what the rules `effective` say to a window seen for the first
    /// time.
    fn first_sight(window: &AppWindow, effective: &Effective, cx: &mut App) {
        let id = window.id;
        if let Some(workspace) = effective.workspace {
            let own = Workspaces::residents(None, cx).map_or(0, |rows| rows.len());
            if (1..=own).contains(&workspace) {
                Workspaces::move_window(id, workspace - 1, cx);
            }
        }
        let placed = Self::place(window, effective, cx);
        if let Some(seen) = cx.global_mut::<Self>().seen.get_mut(&id)
            && placed.is_some()
        {
            seen.placed = placed;
        }
        Self::dress(id, effective, true);
    }

    /// Does again what the rules that apply always say, as far as it
    /// differs, and puts a window that moved right after it was placed
    /// back where its rules put it.
    fn once_more(window: &AppWindow, cx: &mut App) {
        let Some(seen) = cx.global::<Self>().seen.get(&window.id) else {
            return;
        };
        let (always, placed) = (seen.always, seen.placed);
        let id = window.id;
        if let (Some(placed), Some(frame)) = (placed, id.frame()) {
            let moved = (frame.left - placed.left).abs() > STILL_THERE
                || (frame.top - placed.top).abs() > STILL_THERE;
            if moved && !id.is_maximized() {
                id.place(placed);
            }
        }
        if always.places() || always.maximize.is_some() {
            let there = id.frame();
            let wanted = Self::wanted_frame(window, &always);
            let off = there.zip(wanted).is_some_and(|(there, wanted)| {
                (there.left - wanted.left).abs() > STILL_THERE
                    || (there.top - wanted.top).abs() > STILL_THERE
                    || (there.right - wanted.right).abs() > STILL_THERE
                    || (there.bottom - wanted.bottom).abs() > STILL_THERE
            });
            let maximized = always
                .maximize
                .is_some_and(|maximize| maximize != id.is_maximized());
            if (off && !id.is_maximized()) || maximized {
                Self::place(window, &always, cx);
            }
        }
        Self::dress(id, &always, false);
    }

    /// Where the rules `effective` put `window`, unless the tiling has it.
    fn wanted_frame(window: &AppWindow, effective: &Effective) -> Option<ScreenRect> {
        let frame = window.id.frame()?;
        let displays = Display::all();
        let from = Display::of_window(window.id)?;
        let to = effective
            .display
            .and_then(|number| displays.get(number - 1))
            .copied()
            .unwrap_or(from);
        let carried = match to.key == from.key {
            true => frame,
            false => carried(frame, from.work, to.work),
        };
        ruled_frame(carried, to.work, effective.position, effective.size).or(Some(carried))
    }

    /// Puts `window` where `effective` says and maximizes it if it says
    /// so, unless the tiling has it. Where it was put is returned.
    fn place(window: &AppWindow, effective: &Effective, cx: &App) -> Option<ScreenRect> {
        let id = window.id;
        if Tiler::tiles(window, cx) {
            return None;
        }
        let frame = effective
            .places()
            .then(|| Self::wanted_frame(window, effective))
            .flatten();
        match (effective.maximize, frame) {
            (Some(true), frame) => {
                match frame.or_else(|| id.restored_frame()) {
                    Some(frame) => id.place_restored(frame, true),
                    None => id.maximize(),
                }
                None
            }
            (Some(false), Some(frame)) if id.is_maximized() => {
                id.place_restored(frame, false);
                Some(frame)
            }
            (Some(false), None) if id.is_maximized() => {
                id.restore();
                None
            }
            (_, Some(frame)) if !id.is_maximized() => {
                id.place(frame);
                Some(frame)
            }
            _ => None,
        }
    }

    /// Keeps the window `id` above the others, draws it see-through or
    /// without its border as `effective` says, as far as that differs
    /// from how it is. The border is taken away only when the window is
    /// first seen; the tiling leaves it away from then on.
    fn dress(id: AppWindowId, effective: &Effective, first: bool) {
        if let Some(above) = effective.above
            && above != id.is_topmost()
        {
            id.set_topmost(above);
        }
        if let Some(opacity) = effective.opacity
            && (id.opacity() - opacity).abs() > 0.01
        {
            id.set_opacity(opacity);
        }
        if first && effective.border == Some(false) {
            id.hide_border();
        }
    }
}

/// Those of `rules` that apply always.
fn always(rules: &[RuleConfig]) -> impl Iterator<Item = &RuleConfig> {
    rules.iter().filter(|rule| rule.apply == Apply::Always)
}

/// Writes a `[[rule]]` for the window `id` into the configuration: one that
/// names its program and its window class, or the packaged application it
/// belongs to, and puts it where it is now, as
/// large as it is, above the others and as see-through as it is now. Where
/// the file is is returned.
pub fn write_rule_for(id: AppWindowId) -> Result<PathBuf, String> {
    let window = { AppWindow::enumerate().into_iter() }
        .find(|window| window.id == id)
        .ok_or("the window is gone")?;
    let named = Named::of(&window);
    let display = Display::of_window(id).ok_or("the window is on no display")?;
    let frame = id.frame().ok_or("the window is gone")?;
    let mut rule = Table::new();
    for (key, name) in naming(&named) {
        rule.insert(key, value(name));
    }
    if !id.is_maximized() {
        let [x, y, width, height] = Zone::of(frame, display.work).written();
        let pair = |first: f64, second: f64| {
            let mut pair = Array::new();
            pair.push(first.clamp(0., 1.));
            pair.push(second.clamp(0.001, 1.));
            value(pair)
        };
        rule.insert("position", pair(x, y));
        rule.insert("size", pair(width, height));
    } else {
        rule.insert("maximize", value(true));
    }
    if id.is_topmost() {
        rule.insert("above", value(true));
    }
    let opacity = id.opacity();
    if opacity < 0.99 {
        rule.insert("opacity", value((f64::from(opacity) * 100.).round() / 100.));
    }
    let note = format!(
        "Written from the window menu for \u{201c}{}\u{201d}.\nChange or remove what it does; see the [[rule]] section of CONFIGURATION.md.",
        named.title.replace('\n', " ")
    );
    let mut file = ConfigFile::open()?;
    if !file.add_rule(rule, &note) {
        return Err("the configuration has a `rule` that is no list of tables".to_owned());
    }
    file.save()?;
    ConfigFile::location()
}

/// The program of the windows of the applications of the Universal
/// Windows Platform, whatever application they show.
const FRAME_HOST: &str = "ApplicationFrameHost.exe";

/// How a rule written for the window `named` names it: by the packaged
/// application it belongs to, for which the program and the window class
/// are those of the frame host or say nothing more; else by its program
/// and its window class. A window of the frame host whose application is
/// not known, or of a program that cannot be read, is named by its title,
/// word for word.
fn naming(named: &Named) -> Vec<(&'static str, String)> {
    if let Some(app) = &named.app {
        return vec![("app", app.clone())];
    }
    let exe = { named.exe.as_ref() }.filter(|exe| !exe.eq_ignore_ascii_case(FRAME_HOST));
    let mut naming = match exe {
        Some(exe) => vec![("exe", exe.clone())],
        None => vec![("title", format!("re:^{}$", regex::escape(&named.title)))],
    };
    if !named.class.is_empty() {
        naming.push(("class", named.class.clone()));
    }
    naming
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn a_pattern_stands_for_names() {
        assert!(glob("*", ""));
        assert!(glob("note*.exe", "notepad.exe"));
        assert!(glob("*pad.exe", "notepad.exe"));
        assert!(glob("n?tepad.exe", "notepad.exe"));
        assert!(glob("*e*e*", "notepad.exe"));
        assert!(!glob("note*.exe", "notepad.com"));
        assert!(!glob("n?tepad", "notepad.exe"));
        assert!(!glob("", "a"));
        let pattern = Pattern::parse("*Picture-in-Picture*").unwrap();
        assert!(pattern.matches("Firefox — picture-in-picture"));
        assert!(!pattern.matches("Firefox"));
        let expression = Pattern::parse("re:^(code|cursor)\\.exe$").unwrap();
        assert!(expression.matches("Code.exe"));
        assert!(!expression.matches("vscode.exe"));
        assert!(Pattern::parse("re:(").is_err());
    }

    #[test]
    fn every_rule_that_names_a_window_applies_and_the_last_one_wins() {
        let rules: Vec<RuleConfig> = toml::from_str::<HashMap<String, Vec<RuleConfig>>>(
            r#"
            [[rule]]
            exe = "*.exe"
            above = true
            opacity = 0.9

            [[rule]]
            app = "Microsoft.WindowsCalculator_*"
            title = "Calculator"
            float = true
            opacity = 0.8
            position = "center"

            [[rule]]
            class = "Chrome_*"
            tile = true
            apply = "always"
            "#,
        )
        .unwrap()
        .remove("rule")
        .unwrap();
        let calculator = Named {
            exe: Some("ApplicationFrameHost.exe".to_owned()),
            app: Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned()),
            title: "Calculator".to_owned(),
            class: "ApplicationFrameWindow".to_owned(),
        };
        let effective = Effective::of(&rules, &calculator);
        assert_eq!(effective.tile, Some(false));
        assert_eq!(effective.above, Some(true));
        assert_eq!(effective.opacity, Some(0.8));
        assert_eq!(effective.position, Some(Position::Center));
        let browser = Named {
            exe: Some("chrome.exe".to_owned()),
            app: None,
            title: "New Tab".to_owned(),
            class: "Chrome_WidgetWin_1".to_owned(),
        };
        assert_eq!(Effective::of(&rules, &browser).tile, Some(true));
        assert_eq!(Effective::of(always(&rules), &browser).tile, Some(true));
        assert_eq!(
            Effective::of(always(&rules), &calculator),
            Effective::default()
        );
        // A window whose program cannot be read has no file name to match.
        let unknown = Named {
            exe: None,
            ..calculator
        };
        assert_eq!(Effective::of(&rules[..1], &unknown), Effective::default());
    }

    #[test]
    fn a_written_rule_names_a_packaged_window_by_its_application() {
        let calculator = Named {
            exe: Some("ApplicationFrameHost.exe".to_owned()),
            app: Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned()),
            title: "Calculator".to_owned(),
            class: "ApplicationFrameWindow".to_owned(),
        };
        let app = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned();
        assert_eq!(naming(&calculator), vec![("app", app)]);
        let unknown = Named {
            app: None,
            title: "Settings (1)".to_owned(),
            ..calculator
        };
        let naming = naming(&unknown);
        assert_eq!(naming[0], ("title", "re:^Settings \\(1\\)$".to_owned()));
        assert_eq!(naming[1], ("class", "ApplicationFrameWindow".to_owned()));
        let pattern = Pattern::parse(&naming[0].1).unwrap();
        assert!(pattern.matches("Settings (1)"));
        assert!(!pattern.matches("Settings (1) - Edge"));
    }

    #[test]
    fn a_rule_is_checked() {
        let rule = |text: &str| toml::from_str::<RuleConfig>(text);
        assert!(
            rule("title = \"x\"\nopacity = 0.05")
                .unwrap()
                .fault()
                .is_some()
        );
        assert!(rule("opacity = 0.5").unwrap().fault().is_some());
        assert!(
            rule("exe = \"a.exe\"\nworkspace = 0")
                .unwrap()
                .fault()
                .is_some()
        );
        assert!(
            rule("exe = \"a.exe\"\nsize = [1.5, 0.5]")
                .unwrap()
                .fault()
                .is_some()
        );
        assert!(
            rule("exe = \"a.exe\"\nposition = [0.1, 0.2]\nsize = [0.5, 0.5]")
                .unwrap()
                .fault()
                .is_none()
        );
        assert!(rule("exe = \"a.exe\"\nposition = \"left\"").is_err());
        assert!(rule("exe = \"a.exe\"\nfloats = true").is_err());
        assert!(rule("exe = \"re:(\"").is_err());
    }

    #[test]
    fn a_rule_places_a_window_within_the_work_area() {
        let work = rect(0, 0, 1920, 1040);
        let frame = rect(100, 100, 900, 700);
        assert_eq!(ruled_frame(frame, work, None, None), None);
        let centered = ruled_frame(frame, work, Some(Position::Center), None);
        assert_eq!(centered, Some(rect(560, 220, 1360, 820)));
        let sized = ruled_frame(frame, work, Some(Position::At(0.5, 0.)), Some([0.5, 1.]));
        assert_eq!(sized, Some(rect(960, 0, 1920, 1040)));
        // A window put too far right is kept on the display.
        let kept = ruled_frame(frame, work, Some(Position::At(0.9, 0.9)), None);
        assert_eq!(kept, Some(rect(1120, 440, 1920, 1040)));
        let resized = ruled_frame(frame, work, None, Some([0.25, 0.5]));
        assert_eq!(resized, Some(rect(100, 100, 580, 620)));
    }
}
