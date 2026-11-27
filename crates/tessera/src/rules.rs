// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Window rules, as KWin has them: each `[[rule]]` of the configuration
//! names windows by the file name or the path of their program, the
//! packaged application they belong to, their title or their window class
//! (see [`matching`] for how), and says what is done to them when they are
//! first seen: whether they are tiled, the workspace and the display they
//! go to, where they are placed and how large, whether they are maximized
//! or minimized, made to fill their display, kept above or below the
//! others, drawn see-through (also only while they are not in front),
//! without their thin border or their title bar, and how small and large
//! they may be. A rule with `apply = "always"` does it again whenever the
//! window changes, as far as it differs; one with `apply = "remember"`
//! opens the next window it names where the last one was.
//!
//! Every rule that names a window applies, in the order of the file, so
//! that a later one has the last word. The sets of rules that come with
//! Tessera and that `rule_templates` switches on come before the rules of
//! the user.
//!
//! What the rules say of tiling, of the dock and of the window switcher
//! is asked for with [`Rules::of`].

pub mod matching;
mod memory;
mod templates;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use gpui::{App, AsyncApp, Global};
use serde::Deserialize;
use toml_edit::{Array, Table, value};

use crate::config::edit::ConfigFile;
use crate::platform::{AppWindow, AppWindowId, Display, ScreenRect, WindowEvents};
use crate::snap::{Zone, carried};
use crate::tiling::Tiler;
use crate::workspaces::Workspaces;

use self::matching::{Condition, MatchMode, Named, Naming};
use self::memory::{Kept, Memory};

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

/// The least and most a side of `min_size` and `max_size` may be, in
/// logical pixels.
const SIDES: (u32, u32) = (1, 20_000);

/// When a rule does what it says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Apply {
    /// When the window is first seen.
    #[default]
    Initially,
    /// Whenever something happens to the window, as far as it differs.
    Always,
    /// When the window is first seen, and the next window the rule names
    /// opens where the last one was.
    Remember,
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

/// What a `[[rule]]` says is done to the windows it names.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Says {
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
    minimized: Option<bool>,
    /// Whether the window covers all of its display, without title bar
    /// or frame.
    fullscreen: Option<bool>,
    /// Whether the window is kept above the others.
    above: Option<bool>,
    /// Whether the window is kept below the others.
    below: Option<bool>,
    /// Whether the window is shown on every workspace.
    sticky: Option<bool>,
    /// How opaque the window is drawn, from 0.1 to 1.
    opacity: Option<f32>,
    /// How opaque it is drawn while another window is in front.
    inactive_opacity: Option<f32>,
    /// Whether Windows draws the thin border of the window.
    border: Option<bool>,
    /// Whether the title bar of the window is taken away.
    no_titlebar: Option<bool>,
    /// The least and the most `[width, height]`, in logical pixels.
    min_size: Option<[u32; 2]>,
    max_size: Option<[u32; 2]>,
    /// A word for the kind of the window, as `browser` or `game`.
    category: Option<String>,
    /// Whether the dock leaves the window out.
    skip_dock: Option<bool>,
    /// Whether the window switcher leaves the window out.
    skip_switcher: Option<bool>,
    #[serde(default)]
    apply: Apply,
}

/// A `[[rule]]`: which windows it names, and what it says of them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleConfig {
    condition: Condition,
    says: Says,
    /// `category`, kept once for all windows.
    category: Option<&'static str>,
    /// What the frames of its windows are remembered by, for a rule that
    /// remembers them.
    remembered_as: Option<&'static str>,
}

impl<'de> Deserialize<'de> for RuleConfig {
    /// A rule is one table: the keys of [`Naming`] name the windows, the
    /// others say what is done to them, and each part refuses keys it
    /// does not know.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut says = toml::Table::deserialize(deserializer)?;
        let mut naming = toml::Table::new();
        for key in Naming::KEYS {
            if let Some(found) = says.remove(key) {
                naming.insert(key.to_owned(), found);
            }
        }
        let naming: Naming = toml::Value::Table(naming)
            .try_into()
            .map_err(D::Error::custom)?;
        let condition = naming
            .condition(MatchMode::Glob)
            .map_err(D::Error::custom)?;
        let says: Says = toml::Value::Table(says)
            .try_into()
            .map_err(D::Error::custom)?;
        let category = says.category.as_deref().map(interned);
        let remembered_as =
            (says.apply == Apply::Remember).then(|| interned(&condition.to_string()));
        Ok(Self {
            condition,
            says,
            category,
            remembered_as,
        })
    }
}

/// `text`, kept for as long as Tessera runs, once for each text: the
/// categories and the names of rules, which are few.
fn interned(text: &str) -> &'static str {
    static KEPT: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashSet::new);
    if let Some(&known) = kept.get(text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    kept.insert(leaked);
    leaked
}

impl RuleConfig {
    /// Why the rule cannot be used, if it cannot.
    fn fault(&self) -> Option<&'static str> {
        let says = &self.says;
        let share = |value: f32| value.is_finite() && value > 0. && value <= 1.;
        let opaque = |opacity: Option<f32>| {
            opacity.is_some_and(|opacity| !(OPACITIES.0..=OPACITIES.1).contains(&opacity))
        };
        let side = |size: Option<[u32; 2]>| {
            size.is_some_and(|sides| sides.iter().any(|side| !(SIDES.0..=SIDES.1).contains(side)))
        };
        if !self.condition.names_something() {
            return Some("it names no windows: it needs `exe`, `path`, `app`, `title` or `class`");
        }
        if opaque(says.opacity) || opaque(says.inactive_opacity) {
            return Some("its `opacity` or `inactive_opacity` is not from 0.1 to 1");
        }
        if says
            .size
            .is_some_and(|[width, height]| !share(width) || !share(height))
        {
            return Some("its `size` is not `[width, height]` in shares of the work area");
        }
        if says.workspace == Some(0) || says.display == Some(0) {
            return Some("its `workspace` and `display` are counted from 1");
        }
        if side(says.min_size) || side(says.max_size) {
            return Some("its `min_size` or `max_size` is not `[width, height]` from 1 to 20000");
        }
        if let (Some(least), Some(most)) = (says.min_size, says.max_size)
            && (least[0] > most[0] || least[1] > most[1])
        {
            return Some("its `min_size` is larger than its `max_size`");
        }
        if says.above == Some(true) && says.below == Some(true) {
            return Some("it keeps the window both `above` and `below`");
        }
        None
    }

    /// Whether the rule names the window `named`.
    fn matches(&self, named: &Named) -> bool {
        self.condition.holds(named)
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
    pub minimized: Option<bool>,
    pub fullscreen: Option<bool>,
    pub above: Option<bool>,
    pub below: Option<bool>,
    pub sticky: Option<bool>,
    pub opacity: Option<f32>,
    pub inactive_opacity: Option<f32>,
    pub border: Option<bool>,
    pub no_titlebar: Option<bool>,
    pub min_size: Option<[u32; 2]>,
    pub max_size: Option<[u32; 2]>,
    /// The kind of the window, as `browser`, for what groups windows.
    pub category: Option<&'static str>,
    pub skip_dock: Option<bool>,
    pub skip_switcher: Option<bool>,
    /// What the frame of the window is remembered by, where a rule that
    /// remembers names it.
    pub remembered_as: Option<&'static str>,
}

impl Effective {
    /// What `rules` say of the window `named`.
    fn of<'a>(rules: impl IntoIterator<Item = &'a RuleConfig>, named: &Named) -> Self {
        let mut effective = Self::default();
        for rule in rules.into_iter().filter(|rule| rule.matches(named)) {
            let says = &rule.says;
            take(&mut effective.tile, says.float.map(|float| !float));
            take(&mut effective.tile, says.tile);
            take(&mut effective.workspace, says.workspace);
            take(&mut effective.display, says.display);
            take(&mut effective.position, says.position);
            take(&mut effective.size, says.size);
            take(&mut effective.maximize, says.maximize);
            take(&mut effective.minimized, says.minimized);
            take(&mut effective.fullscreen, says.fullscreen);
            take(&mut effective.above, says.above);
            take(&mut effective.below, says.below);
            take(&mut effective.sticky, says.sticky);
            take(&mut effective.opacity, says.opacity);
            take(&mut effective.inactive_opacity, says.inactive_opacity);
            take(&mut effective.border, says.border);
            take(&mut effective.no_titlebar, says.no_titlebar);
            take(&mut effective.min_size, says.min_size);
            take(&mut effective.max_size, says.max_size);
            take(&mut effective.category, rule.category);
            take(&mut effective.skip_dock, says.skip_dock);
            take(&mut effective.skip_switcher, says.skip_switcher);
            take(&mut effective.remembered_as, rule.remembered_as);
            // A window kept above by a later rule is no longer kept
            // below by an earlier one, and the other way round.
            if says.above == Some(true) {
                effective.below = None;
            }
            if says.below == Some(true) {
                effective.above = None;
            }
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
            minimized: new(self.minimized, before.minimized),
            fullscreen: new(self.fullscreen, before.fullscreen),
            above: new(self.above, before.above),
            below: new(self.below, before.below),
            sticky: new(self.sticky, before.sticky),
            opacity: new(self.opacity, before.opacity),
            inactive_opacity: new(self.inactive_opacity, before.inactive_opacity),
            border: new(self.border, before.border),
            no_titlebar: new(self.no_titlebar, before.no_titlebar),
            min_size: new(self.min_size, before.min_size),
            max_size: new(self.max_size, before.max_size),
            category: new(self.category, before.category),
            skip_dock: new(self.skip_dock, before.skip_dock),
            skip_switcher: new(self.skip_switcher, before.skip_switcher),
            remembered_as: new(self.remembered_as, before.remembered_as),
        }
    }

    /// Whether the window is to be placed at all.
    fn places(&self) -> bool {
        self.display.is_some() || self.position.is_some() || self.size.is_some()
    }

    /// How opaque the window is to be drawn, in front or not; `None` where
    /// the rules say nothing of it.
    fn opacity_for(&self, in_front: bool) -> Option<f32> {
        match (in_front, self.inactive_opacity) {
            (false, Some(inactive)) => Some(inactive),
            (true, Some(_)) => Some(self.opacity.unwrap_or(1.)),
            (_, None) => self.opacity,
        }
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

/// `frame` made at least `least` and at most `most` large, in logical
/// pixels at `scale` physical ones each, keeping its top left corner;
/// `None` where it is so already.
pub fn bounded(
    frame: ScreenRect,
    least: Option<[u32; 2]>,
    most: Option<[u32; 2]>,
    scale: f32,
) -> Option<ScreenRect> {
    let physical = |logical: u32| (logical as f32 * scale).round() as i32;
    let fit = |extent: i32, axis: usize| {
        let extent = match most {
            Some(most) => extent.min(physical(most[axis])),
            None => extent,
        };
        match least {
            Some(least) => extent.max(physical(least[axis])),
            None => extent,
        }
    };
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    let (fitted_width, fitted_height) = (fit(width, 0), fit(height, 1));
    ((fitted_width, fitted_height) != (width, height)).then_some(ScreenRect {
        right: frame.left + fitted_width,
        bottom: frame.top + fitted_height,
        ..frame
    })
}

/// Whether `a` and `b` are the same within [`STILL_THERE`] on every side.
fn near(a: ScreenRect, b: ScreenRect) -> bool {
    (a.left - b.left).abs() <= STILL_THERE
        && (a.top - b.top).abs() <= STILL_THERE
        && (a.right - b.right).abs() <= STILL_THERE
        && (a.bottom - b.bottom).abs() <= STILL_THERE
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

/// The sets of rules of `rule_templates`, by their names.
#[derive(Default)]
struct RuleTemplates(Vec<String>);

impl Global for RuleTemplates {}

/// Keeps the rules and applies them.
#[derive(Default)]
pub struct Rules {
    rules: Vec<RuleConfig>,
    seen: HashMap<AppWindowId, Seen>,
    /// Counts the tasks that were started to watch the windows; only the
    /// latest goes on.
    task: u64,
    /// Whether it was said that `sticky` does nothing yet.
    told_sticky: bool,
}

impl Global for Rules {}

impl Rules {
    /// Has the sets of rules `names` come before the rules of the user
    /// from the next [`Rules::start`] or [`Rules::configure`] on.
    pub fn use_templates(names: &[String], cx: &mut App) {
        cx.set_global(RuleTemplates(names.to_vec()));
    }

    /// Applies `rules` from now on to the windows that appear.
    pub fn start(rules: &[RuleConfig], cx: &mut App) {
        cx.set_global(Self::default());
        Memory::read(cx);
        crate::window_actions::give_back_on_quit(cx);
        Self::configure(rules, cx);
    }

    /// Takes over the rules of a configuration that was read again, if they
    /// changed. The windows that are open already, when Tessera starts or
    /// the rules change, count as seen: only the rules that apply always
    /// and the sizes of `min_size` and `max_size` change them, and what a
    /// rule that is gone or changed took from them is given back.
    pub fn configure(rules: &[RuleConfig], cx: &mut App) {
        let templates = cx.default_global::<RuleTemplates>().0.clone();
        let all = templates::rules_of_all(&templates);
        let usable: Vec<RuleConfig> = { all.iter().chain(rules) }
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
        let mut changed = Vec::new();
        for window in &windows {
            let named = Named::of(window);
            let seen = this.seen.entry(window.id).or_insert_with(|| Seen {
                named: named.clone(),
                effective: Effective::default(),
                always: Effective::default(),
                first: Instant::now(),
                placed: None,
            });
            let before = seen.effective;
            seen.effective = Effective::of(&rules, &named);
            seen.always = Effective::of(always(&rules), &named);
            seen.named = named;
            changed.push((window.id, before, seen.effective));
        }
        Self::hand_over_tiling(cx);
        for (id, before, now) in changed {
            Self::undress(id, &before, &now);
            Self::bound(id, &now, None, cx);
        }
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

    /// Takes in that the user ended moving or resizing the window `id`:
    /// the sizes of `min_size` and `max_size` are kept, and a window whose
    /// frame a rule remembers is remembered where it is now.
    pub fn move_ended(id: AppWindowId, cx: &mut App) {
        let Some(seen) = cx.try_global::<Self>().and_then(|this| this.seen.get(&id)) else {
            return;
        };
        let effective = seen.effective;
        Self::bound(id, &effective, None, cx);
        Self::remember(id, &effective, cx);
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
                    let follows = seen.effective.inactive_opacity.is_some()
                        || seen.effective.remembered_as.is_some();
                    if seen.always != Effective::default() || seen.placed.is_some() || follows {
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
            .filter_map(|(&id, seen)| {
                // A window that fills its display is no tile.
                let tile = match seen.effective.fullscreen {
                    Some(true) => Some(false),
                    _ => seen.effective.tile,
                };
                Some((id, tile?))
            })
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
        if effective.sticky == Some(true) {
            Self::stick(id, cx);
        }
        let (placed, maximizing) = match Self::recall(window, effective, cx) {
            Some(placed) => (placed, placed.is_none()),
            None => (
                Self::place(window, effective, cx),
                effective.maximize == Some(true),
            ),
        };
        if let Some(seen) = cx.global_mut::<Self>().seen.get_mut(&id)
            && placed.is_some()
        {
            seen.placed = placed;
        }
        Self::dress(id, effective, true);
        if effective.fullscreen != Some(true) && !maximizing {
            Self::bound(id, effective, placed, cx);
        }
        if effective.minimized == Some(true) {
            id.minimize();
        }
    }

    /// Does again what the rules that apply always say, as far as it
    /// differs, puts a window that moved right after it was placed back
    /// where its rules put it, draws a window as opaque as it is to be in
    /// front or not, and remembers where a window is whose frame a rule
    /// remembers.
    fn once_more(window: &AppWindow, cx: &mut App) {
        let Some(seen) = cx.global::<Self>().seen.get(&window.id) else {
            return;
        };
        let (effective, always, placed) = (seen.effective, seen.always, seen.placed);
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
            let off = there
                .zip(wanted)
                .is_some_and(|(there, wanted)| !near(there, wanted));
            let maximized = always
                .maximize
                .is_some_and(|maximize| maximize != id.is_maximized());
            if (off && !id.is_maximized()) || maximized {
                Self::place(window, &always, cx);
            }
        }
        if always.minimized == Some(true) && !id.is_minimized() {
            id.minimize();
        }
        Self::dress(id, &always, false);
        if effective.inactive_opacity.is_some() {
            Self::shade(id, &effective);
        }
        if placed.is_none() {
            Self::remember(id, &effective, cx);
        }
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
        if Tiler::tiles(window, cx) || effective.fullscreen == Some(true) {
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

    /// Puts `window` where the last window its remembering rule named
    /// was, if that is still on a display and the tiling does not have it.
    /// Whether it was put there is returned, with where unless it is being
    /// maximized.
    fn recall(window: &AppWindow, effective: &Effective, cx: &App) -> Option<Option<ScreenRect>> {
        let rule = effective.remembered_as?;
        let kept = Memory::get(rule, cx)?;
        if Tiler::tiles(window, cx) || effective.fullscreen == Some(true) {
            return None;
        }
        let frame = kept.rect();
        let shown = { Display::all().into_iter() }.any(|display| {
            frame.left < display.work.right
                && frame.right > display.work.left
                && frame.top < display.work.bottom
                && frame.bottom > display.work.top
        });
        if !shown {
            return None;
        }
        window.id.place_restored(frame, kept.maximized);
        Some((!kept.maximized).then_some(frame))
    }

    /// Remembers where the window `id` is, if a rule remembers its frame
    /// and it is neither minimized nor tiled.
    fn remember(id: AppWindowId, effective: &Effective, cx: &mut App) {
        let Some(rule) = effective.remembered_as else {
            return;
        };
        if id.is_minimized() || Tiler::manages(id, cx) {
            return;
        }
        let maximized = id.is_maximized();
        let frame = match maximized {
            true => id.restored_frame(),
            false => id.frame(),
        };
        if let Some(frame) = frame {
            Memory::keep(rule, Kept::new(frame, maximized), cx);
        }
    }

    /// Keeps the window `id` within `min_size` and `max_size` of
    /// `effective`, unless it is maximized, minimized or tiled: at `sent`,
    /// where it was just sent, or else where it is.
    fn bound(id: AppWindowId, effective: &Effective, sent: Option<ScreenRect>, cx: &App) {
        if effective.min_size.is_none() && effective.max_size.is_none() {
            return;
        }
        if id.is_maximized() || id.is_minimized() || Tiler::manages(id, cx) {
            return;
        }
        let (Some(frame), Some(display)) = (sent.or_else(|| id.frame()), Display::of_window(id))
        else {
            return;
        };
        let fitted = bounded(frame, effective.min_size, effective.max_size, display.scale);
        if let Some(fitted) = fitted {
            id.place(fitted);
        }
    }

    /// Shows the window `id` on every workspace. Tessera's own workspaces
    /// cannot do that yet, which is said once.
    fn stick(_id: AppWindowId, cx: &mut App) {
        let this = cx.global_mut::<Self>();
        if !this.told_sticky {
            this.told_sticky = true;
            report!(
                "`sticky` of a [[rule]] does nothing yet: Tessera's workspaces do not show a window on all of them"
            );
        }
    }

    /// Keeps the window `id` above or below the others, has it fill its
    /// display, draws it see-through or without its border or title bar as
    /// `effective` says, as far as that differs from how it is. The border
    /// is taken away only when the window is first seen; the tiling leaves
    /// it away from then on.
    fn dress(id: AppWindowId, effective: &Effective, first: bool) {
        if let Some(above) = effective.above
            && above != id.is_topmost()
        {
            id.set_topmost(above);
        }
        if let Some(below) = effective.below
            && below != id.is_kept_below()
        {
            id.keep_below(below);
        }
        if effective.inactive_opacity.is_none()
            && let Some(opacity) = effective.opacity
            && (id.opacity() - opacity).abs() > 0.01
        {
            id.set_opacity(opacity);
        }
        if first && effective.inactive_opacity.is_some() {
            Self::shade(id, effective);
        }
        if first && effective.border == Some(false) {
            id.hide_border();
        }
        if let Some(untitled) = effective.no_titlebar
            && untitled != id.title_bar_taken()
        {
            id.show_title_bar(!untitled);
        }
        if let Some(full) = effective.fullscreen
            && full != id.fills_display()
        {
            let display = Display::of_window(id).map(|display| display.frame);
            match full {
                true if display.is_some() => id.fill_display(display),
                true => {}
                false => id.fill_display(None),
            }
        }
    }

    /// Draws the window `id` as opaque as `effective` has it while it is
    /// in front or while it is not, as it is now.
    fn shade(id: AppWindowId, effective: &Effective) {
        let in_front = AppWindowId::foreground() == Some(id);
        if let Some(opacity) = effective.opacity_for(in_front)
            && (id.opacity() - opacity).abs() > 0.01
        {
            id.set_opacity(opacity);
        }
    }

    /// Gives the window `id` back what a rule took from it that the rules
    /// `now` no longer take: its title bar, its frame, its place in the
    /// order of windows and its opacity.
    fn undress(id: AppWindowId, before: &Effective, now: &Effective) {
        if before.no_titlebar == Some(true) && now.no_titlebar != Some(true) && id.title_bar_taken()
        {
            id.show_title_bar(true);
        }
        if before.fullscreen == Some(true) && now.fullscreen != Some(true) && id.fills_display() {
            id.fill_display(None);
        }
        if before.below == Some(true) && now.below != Some(true) && id.is_kept_below() {
            id.keep_below(false);
        }
        if before.inactive_opacity.is_some() && now.inactive_opacity.is_none() {
            let opacity = now.opacity.unwrap_or(1.);
            if (id.opacity() - opacity).abs() > 0.01 {
                id.set_opacity(opacity);
            }
        }
    }
}

/// Those of `rules` that apply always.
fn always(rules: &[RuleConfig]) -> impl Iterator<Item = &RuleConfig> {
    rules.iter().filter(|rule| rule.says.apply == Apply::Always)
}

/// Writes a `[[rule]]` for the window `id` into the configuration: one that
/// names its program and its window class, or the packaged application it
/// belongs to, word for word, and puts it where it is now, as large as it
/// is, above or below the others, without its title bar, filling its
/// display and as see-through as it is now. Where the file is is
/// returned.
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
    rule.insert("match", value("equals"));
    if id.fills_display() {
        rule.insert("fullscreen", value(true));
    } else if !id.is_maximized() {
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
    if id.is_kept_below() {
        rule.insert("below", value(true));
    }
    if id.title_bar_taken() {
        rule.insert("no_titlebar", value(true));
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

/// How a rule written for the window `named` names it, each name word for
/// word: by the packaged application it belongs to, for which the
/// program and the window class are those of the frame host or say
/// nothing more; else by its program and its window class. A window of
/// the frame host whose application is not known, or of a program that
/// cannot be read, is named by its title.
fn naming(named: &Named) -> Vec<(&'static str, String)> {
    if let Some(app) = &named.app {
        return vec![("app", app.clone())];
    }
    let exe = { named.exe.as_ref() }.filter(|exe| !exe.eq_ignore_ascii_case(FRAME_HOST));
    let mut naming = match exe {
        Some(exe) => vec![("exe", exe.clone())],
        None => vec![("title", named.title.clone())],
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

    fn rules(text: &str) -> Vec<RuleConfig> {
        toml::from_str::<HashMap<String, Vec<RuleConfig>>>(text)
            .unwrap()
            .remove("rule")
            .unwrap()
    }

    fn named(exe: &str, app: Option<&str>, title: &str, class: &str) -> Named {
        Named {
            exe: Some(exe.to_owned()),
            path: None,
            app: app.map(str::to_owned),
            title: title.to_owned(),
            class: class.to_owned(),
        }
    }

    #[test]
    fn every_rule_that_names_a_window_applies_and_the_last_one_wins() {
        let rules = rules(
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
        );
        let calculator = named(
            "ApplicationFrameHost.exe",
            Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            "Calculator",
            "ApplicationFrameWindow",
        );
        let effective = Effective::of(&rules, &calculator);
        assert_eq!(effective.tile, Some(false));
        assert_eq!(effective.above, Some(true));
        assert_eq!(effective.opacity, Some(0.8));
        assert_eq!(effective.position, Some(Position::Center));
        let browser = named("chrome.exe", None, "New Tab", "Chrome_WidgetWin_1");
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
    fn the_new_properties_are_taken_and_the_last_rule_wins() {
        let rules = rules(
            r#"
            [[rule]]
            exe = "game.exe"
            fullscreen = true
            below = true
            minimized = true
            no_titlebar = true
            inactive_opacity = 0.6
            min_size = [400, 300]
            max_size = [1600, 1200]
            category = "game"
            sticky = true
            apply = "remember"

            [[rule]]
            exe = "game.exe"
            title = "Launcher"
            above = true
            category = "game-launcher"
            "#,
        );
        let game = Effective::of(&rules, &named("game.exe", None, "Game", "Game"));
        assert_eq!(game.fullscreen, Some(true));
        assert_eq!(game.below, Some(true));
        assert_eq!(game.minimized, Some(true));
        assert_eq!(game.no_titlebar, Some(true));
        assert_eq!(game.inactive_opacity, Some(0.6));
        assert_eq!(game.min_size, Some([400, 300]));
        assert_eq!(game.max_size, Some([1600, 1200]));
        assert_eq!(game.category, Some("game"));
        assert_eq!(game.sticky, Some(true));
        assert_eq!(game.remembered_as, Some("exe glob \"game.exe\""));
        let launcher = Effective::of(&rules, &named("game.exe", None, "Launcher", "Game"));
        assert_eq!(launcher.category, Some("game-launcher"));
        // Kept above by the later rule, it is no longer kept below.
        assert_eq!(launcher.above, Some(true));
        assert_eq!(launcher.below, None);
        // A rule that remembers counts as one that applies initially.
        assert_eq!(
            Effective::of(always(&rules), &named("game.exe", None, "Game", "Game")),
            Effective::default()
        );
    }

    #[test]
    fn a_window_in_front_is_drawn_as_opaque_as_it_is_to_be_then() {
        let both = Effective {
            opacity: Some(0.9),
            inactive_opacity: Some(0.5),
            ..Effective::default()
        };
        assert_eq!(both.opacity_for(true), Some(0.9));
        assert_eq!(both.opacity_for(false), Some(0.5));
        let inactive = Effective {
            inactive_opacity: Some(0.5),
            ..Effective::default()
        };
        assert_eq!(inactive.opacity_for(true), Some(1.));
        let plain = Effective {
            opacity: Some(0.8),
            ..Effective::default()
        };
        assert_eq!(plain.opacity_for(false), Some(0.8));
        assert_eq!(Effective::default().opacity_for(false), None);
    }

    #[test]
    fn a_window_is_kept_within_its_sizes() {
        let frame = rect(100, 100, 300, 200);
        assert_eq!(bounded(frame, None, None, 1.), None);
        assert_eq!(
            bounded(frame, Some([400, 50]), None, 1.),
            Some(rect(100, 100, 500, 200))
        );
        assert_eq!(
            bounded(frame, None, Some([100, 100]), 1.5),
            Some(rect(100, 100, 250, 200))
        );
        assert_eq!(bounded(frame, Some([100, 50]), Some([300, 300]), 1.), None);
    }

    #[test]
    fn a_written_rule_names_a_window_word_for_word() {
        let calculator = named(
            "ApplicationFrameHost.exe",
            Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            "Calculator",
            "ApplicationFrameWindow",
        );
        let app = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".to_owned();
        assert_eq!(naming(&calculator), vec![("app", app)]);
        let unknown = Named {
            app: None,
            title: "Settings (1)".to_owned(),
            ..calculator
        };
        let naming = naming(&unknown);
        assert_eq!(naming[0], ("title", "Settings (1)".to_owned()));
        assert_eq!(naming[1], ("class", "ApplicationFrameWindow".to_owned()));
        // As written with `match = "equals"`, it names that window alone.
        let written = format!(
            "[[rule]]\ntitle = {:?}\nclass = {:?}\nmatch = \"equals\"\nfloat = true",
            naming[0].1, naming[1].1
        );
        let rule = &rules(&written)[0];
        let window = |title: &str| {
            named(
                "ApplicationFrameHost.exe",
                None,
                title,
                "ApplicationFrameWindow",
            )
        };
        assert!(rule.matches(&window("Settings (1)")));
        assert!(!rule.matches(&window("Settings (1) - Edge")));
    }

    #[test]
    fn a_rule_is_checked() {
        let rule = |text: &str| toml::from_str::<RuleConfig>(text);
        let fault = |text: &str| rule(text).unwrap().fault();
        assert!(fault("title = \"x\"\nopacity = 0.05").is_some());
        assert!(fault("title = \"x\"\ninactive_opacity = 1.5").is_some());
        assert!(fault("opacity = 0.5").is_some());
        assert!(fault("not = {}\nopacity = 0.5").is_some());
        assert!(fault("exe = \"a.exe\"\nworkspace = 0").is_some());
        assert!(fault("exe = \"a.exe\"\nsize = [1.5, 0.5]").is_some());
        assert!(fault("exe = \"a.exe\"\nmin_size = [0, 10]").is_some());
        assert!(fault("exe = \"a.exe\"\nmin_size = [500, 10]\nmax_size = [400, 400]").is_some());
        assert!(fault("exe = \"a.exe\"\nabove = true\nbelow = true").is_some());
        assert!(fault("exe = \"a.exe\"\nposition = [0.1, 0.2]\nsize = [0.5, 0.5]").is_none());
        assert!(fault("path = 'C:\\Games\\*'\nmatch = \"glob\"\napply = \"remember\"").is_none());
        assert!(fault("any = [{ exe = \"a.exe\" }, { exe = \"b.exe\" }]\nfloat = true").is_none());
        assert!(rule("exe = \"a.exe\"\nposition = \"left\"").is_err());
        assert!(rule("exe = \"a.exe\"\nfloats = true").is_err());
        assert!(rule("exe = \"a.exe\"\nnot = { float = true }").is_err());
        assert!(rule("exe = \"re:(\"").is_err());
        assert!(rule("exe = \"(\"\nmatch = \"regex\"").is_err());
        assert!(rule("exe = \"a.exe\"\napply = \"sometimes\"").is_err());
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
