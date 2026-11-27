// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What `[tiling]` in the configuration says, and what it comes to for
//! each workspace of each display.

use std::path::Path;

use gpui::Hsla;
use serde::Deserialize;

use super::displays::{DisplayName, DisplaySelector};
use super::layouts::LayoutChoice;
use crate::platform::ScreenRect;

/// The distance between tiles when the configuration sets none, and the
/// widest it may set, in pixels of the screen; the same holds for the
/// room around the tiles and at the sides of a display.
const DEFAULT_GAP: f32 = 8.;
const WIDEST_GAP: f32 = 200.;
const WIDEST_INSET: f32 = 1000.;

/// How much of its room a tile gains or gives with each step of `width`
/// and `height` when the configuration says nothing, and the least and
/// most it may say.
const DEFAULT_STEP: f32 = 0.05;
const STEPS: (f32, f32) = (0.01, 0.5);

/// The border Tessera draws when the configuration gives no width, in
/// pixels of the screen, and the widest it may be.
const DEFAULT_BORDER: f32 = 3.;
const WIDEST_BORDER: f32 = 50.;

/// How high the bar of tabs over a stack is when the configuration says
/// nothing, and the range it may say, in pixels of the screen.
const DEFAULT_BAR: f32 = 32.;
const BARS: (f32, f32) = (16., 96.);

/// The share of the space a floated window takes along a side the
/// configuration gives no share for.
const DEFAULT_FLOAT: f32 = 0.6;

/// The category that stands for every category in `auto_stack`.
const EVERY_CATEGORY: &str = "*";

/// What the configuration says about tiling.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TilingConfig {
    /// Whether windows are tiled from the start.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub layout: LayoutChoice,
    /// The distance between tiles, in pixels of the screen.
    gap: Option<f32>,
    /// The room around the tiles; the gap when none is given.
    padding: Option<Sides>,
    /// The room left free at each side of the space the bars leave.
    margin: Option<Sides>,
    /// The programs whose windows are tiled, by the file name of the
    /// executable; every program when empty.
    #[serde(default)]
    only: Vec<String>,
    /// The programs whose windows are left alone.
    #[serde(default)]
    except: Vec<String>,
    /// Pieces of titles: a window whose title has one of them in it is
    /// left alone, whatever the case of the letters.
    #[serde(default)]
    except_titles: Vec<String>,
    /// Pieces of window class names: a window whose class has one of them
    /// in it is left alone, whatever the case of the letters.
    #[serde(default)]
    except_classes: Vec<String>,
    /// How the tiled window in front is told apart.
    #[serde(default)]
    border: Border,
    /// The size a window taken out of the tiles is given.
    float: Option<FloatSize>,
    /// What dropping a tile on another does.
    #[serde(default)]
    drag: DragMode,
    /// How much of its room a tile gains or gives with each step.
    resize_step: Option<f32>,
    /// When a tile shows a bar of tabs over its windows.
    #[serde(default)]
    stack_bar: StackBarShown,
    /// How high that bar is, in pixels of the screen.
    stack_bar_height: Option<f32>,
    /// Which new windows join the stack of a tiled window of the same
    /// category of the window rules, on the same workspace of the same
    /// display.
    #[serde(default)]
    auto_stack: AutoStack,
    /// What is other on some displays.
    #[serde(default)]
    display: Vec<DisplayTiling>,
    /// What is other on some workspaces.
    #[serde(default)]
    workspace: Vec<WorkspaceTiling>,
}

/// The same room at every side, or a room for each.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Sides {
    All(f32),
    Each(EachSide),
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EachSide {
    #[serde(default)]
    top: f32,
    #[serde(default)]
    right: f32,
    #[serde(default)]
    bottom: f32,
    #[serde(default)]
    left: f32,
}

/// The room at each side of something, in pixels of the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inset {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Inset {
    fn all(room: i32) -> Self {
        Self {
            top: room,
            right: room,
            bottom: room,
            left: room,
        }
    }

    /// `rect` without the room at its sides; at least a pixel is left.
    pub fn inside(self, rect: ScreenRect) -> ScreenRect {
        let left = rect.left + self.left;
        let top = rect.top + self.top;
        ScreenRect {
            left,
            top,
            right: (rect.right - self.right).max(left + 1),
            bottom: (rect.bottom - self.bottom).max(top + 1),
        }
    }
}

impl Sides {
    /// The room, or `None` where a side is not usable, which is reported
    /// as `what` when `loud`.
    fn inset(self, what: &str, loud: bool) -> Option<Inset> {
        let usable = |room: f32| room.is_finite() && (0. ..=WIDEST_INSET).contains(&room);
        let (top, right, bottom, left) = match self {
            Self::All(room) => (room, room, room, room),
            Self::Each(each) => (each.top, each.right, each.bottom, each.left),
        };
        if [top, right, bottom, left].into_iter().all(usable) {
            let px = |room: f32| room.round() as i32;
            return Some(Inset {
                top: px(top),
                right: px(right),
                bottom: px(bottom),
                left: px(left),
            });
        }
        if loud {
            report!(
                "`{what}` is not a number from 0 to {WIDEST_INSET} at each side, so it is left out"
            );
        }
        None
    }
}

/// How the tiled window in front is told apart: `border = true` has
/// Windows color its thin border, a table has Tessera draw one.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Border {
    Windows(bool),
    Drawn(DrawnBorder),
}

impl Default for Border {
    fn default() -> Self {
        Self::Windows(false)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawnBorder {
    enabled: Option<bool>,
    width: Option<f32>,
    /// How far out of the window the border begins.
    offset: Option<f32>,
    /// `#rrggbb` or `#rrggbbaa`; the primary color of the theme without.
    color: Option<String>,
}

/// The border Tessera draws, as it is to be drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawnLook {
    pub width: i32,
    pub offset: i32,
    /// As 0xAARRGGBB; `None` for the primary color of the theme.
    pub color: Option<u32>,
}

/// How the window in front is told apart, as it is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marking {
    None,
    /// Windows colors its thin border.
    Windows,
    Drawn(DrawnLook),
}

/// The size of a window taken out of the tiles, in shares of the space
/// the bars leave.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FloatSize {
    #[serde(default = "float_share")]
    width: f32,
    #[serde(default = "float_share")]
    height: f32,
}

fn float_share() -> f32 {
    DEFAULT_FLOAT
}

/// When a tile shows a bar of tabs over its windows, as Seelen UI's
/// `WmStackBarVisibility` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StackBarShown {
    /// Over every tile, also one of a single window.
    Always,
    /// Over a stack of two windows or more.
    #[default]
    AsNeeded,
    Never,
}

/// The bars of tabs over the stacks, as they are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bars {
    pub shown: StackBarShown,
    /// In pixels of the screen.
    pub height: i32,
}

impl Bars {
    /// How much of the top of a tile of `count` windows the bar takes.
    pub fn over(self, count: usize) -> i32 {
        let shown = match self.shown {
            StackBarShown::Always => count >= 1,
            StackBarShown::AsNeeded => count >= 2,
            StackBarShown::Never => false,
        };
        if shown { self.height } else { 0 }
    }
}

/// Which new windows join a stack by their category: `true` for every
/// category, or the categories by name; `"*"` among them is every one.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum AutoStack {
    All(bool),
    Categories(Vec<String>),
}

impl Default for AutoStack {
    fn default() -> Self {
        Self::All(false)
    }
}

impl AutoStack {
    /// Whether a new window of `category` joins a window of that category.
    pub fn takes(&self, category: &str) -> bool {
        match self {
            Self::All(all) => *all,
            Self::Categories(names) => names
                .iter()
                .any(|name| name == EVERY_CATEGORY || name.eq_ignore_ascii_case(category)),
        }
    }

    /// Whether any window joins a stack by its category.
    pub fn any(&self) -> bool {
        match self {
            Self::All(all) => *all,
            Self::Categories(names) => !names.is_empty(),
        }
    }
}

/// What dropping a tile on another does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DragMode {
    /// The two windows trade tiles.
    #[default]
    Swap,
    /// The dragged window goes to the place of the other, which moves on
    /// with those after it.
    Sort,
}

/// What is other on the displays an entry names.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayTiling {
    #[serde(rename = "match")]
    selector: DisplaySelector,
    layout: Option<LayoutChoice>,
    gap: Option<f32>,
    padding: Option<Sides>,
    margin: Option<Sides>,
}

/// What is other on a workspace, on every display or on those `display`
/// names.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTiling {
    /// The workspace, counted from 1.
    number: usize,
    display: Option<DisplaySelector>,
    layout: Option<LayoutChoice>,
    gap: Option<f32>,
    padding: Option<Sides>,
    margin: Option<Sides>,
}

/// A list of entries that say what is other somewhere:
/// `[[tiling.display]]` or `[[tiling.workspace]]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OverrideKind {
    Display,
    Workspace,
}

impl OverrideKind {
    pub const ALL: [Self; 2] = [Self::Display, Self::Workspace];

    /// The key of the list in `[tiling]`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Display => "display",
            Self::Workspace => "workspace",
        }
    }

    /// The key of an entry that says where it holds: `match` for a
    /// display, `number` for a workspace.
    pub fn selector(self) -> &'static str {
        match self {
            Self::Display => "match",
            Self::Workspace => "number",
        }
    }

    /// An entry as one is added: for the first display, or the first
    /// workspace, saying nothing other yet.
    pub fn new_entry(self) -> toml::Table {
        let mut entry = toml::Table::new();
        let selector = match self {
            Self::Display => toml::Value::String("1".to_owned()),
            Self::Workspace => toml::Value::Integer(1),
        };
        entry.insert(self.selector().to_owned(), selector);
        entry
    }

    /// Whether `entry` is an entry of this list the tiling can use, or
    /// why it is not.
    pub fn check(self, entry: &toml::Table) -> Result<(), String> {
        let list = toml::Value::Array(vec![toml::Value::Table(entry.clone())]);
        let mut tiling = toml::Table::new();
        tiling.insert(self.key().to_owned(), list);
        let read: Result<TilingConfig, _> = toml::Value::Table(tiling).try_into();
        read.map(drop).map_err(|error| error.message().to_owned())
    }
}

/// What an entry of `[[tiling.display]]` or `[[tiling.workspace]]` says.
struct Part<'a> {
    what: &'static str,
    layout: Option<&'a LayoutChoice>,
    gap: Option<f32>,
    padding: Option<Sides>,
    margin: Option<Sides>,
}

impl DisplayTiling {
    fn part(&self) -> Part<'_> {
        Part {
            what: "tiling.display",
            layout: self.layout.as_ref(),
            gap: self.gap,
            padding: self.padding,
            margin: self.margin,
        }
    }
}

impl WorkspaceTiling {
    fn part(&self) -> Part<'_> {
        Part {
            what: "tiling.workspace",
            layout: self.layout.as_ref(),
            gap: self.gap,
            padding: self.padding,
            margin: self.margin,
        }
    }
}

/// What the configuration comes to for one workspace of one display.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub layout: LayoutChoice,
    pub gap: i32,
    pub padding: Inset,
    pub margin: Inset,
}

impl Place {
    /// Where the tiles go in `work`, the space the bars leave.
    pub fn area(&self, work: ScreenRect) -> ScreenRect {
        self.padding.inside(self.margin.inside(work))
    }
}

/// The parts of the configuration that apply everywhere, checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knobs {
    pub marking: Marking,
    pub float: Option<(f32, f32)>,
    pub drag: DragMode,
    pub step: f32,
    pub bars: Bars,
}

/// A gap, or `None` where it is not usable, which is reported as `what`
/// when `loud`.
fn gap(gap: f32, what: &str, loud: bool) -> Option<i32> {
    if gap.is_finite() && (0. ..=WIDEST_GAP).contains(&gap) {
        return Some(gap.round() as i32);
    }
    if loud {
        report!("`{what}` is not a number from 0 to {WIDEST_GAP}, so it is left out");
    }
    None
}

/// The color `text` writes as `#rrggbb` or `#rrggbbaa`, as 0xAARRGGBB.
pub fn color(text: &str) -> Option<u32> {
    let digits = text.strip_prefix('#')?;
    let value = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(0xff00_0000 | value),
        8 => Some(value.rotate_right(8)),
        _ => None,
    }
}

/// `color` as 0xAARRGGBB.
pub fn argb(color: Hsla) -> u32 {
    let color = color.to_rgb();
    let part = |part: f32| (part.clamp(0., 1.) * 255.).round() as u32;
    part(color.a) << 24 | part(color.r) << 16 | part(color.g) << 8 | part(color.b)
}

impl TilingConfig {
    /// Whether the windows of the program at `executable` are tiled. A
    /// program that Windows does not let be read is one whose windows it
    /// does not let be moved either.
    pub fn covers(&self, executable: Option<&Path>) -> bool {
        super::listed(&self.only, &self.except, executable)
    }

    /// Which new windows join a stack by their category.
    pub fn auto_stack(&self) -> &AutoStack {
        &self.auto_stack
    }

    /// The pieces of titles and of class names that leave a window alone.
    pub fn exceptions(&self) -> (&[String], &[String]) {
        (&self.except_titles, &self.except_classes)
    }

    /// Whether the configuration says something other about layouts than
    /// `other`, so that the layouts chosen while Tessera ran give way.
    pub fn layouts_differ(&self, other: &Self) -> bool {
        let layouts = |config: &Self| {
            let displays: Vec<_> = config
                .display
                .iter()
                .map(|entry| entry.layout.clone())
                .collect();
            let workspaces: Vec<_> = { config.workspace.iter() }
                .map(|entry| entry.layout.clone())
                .collect();
            (config.layout.clone(), displays, workspaces)
        };
        layouts(self) != layouts(other)
    }

    /// The checked parts that apply everywhere; what is not usable is
    /// reported and left as built in.
    pub fn knobs(&self) -> Knobs {
        let marking = match &self.border {
            Border::Windows(false) => Marking::None,
            Border::Windows(true) => Marking::Windows,
            Border::Drawn(drawn) if drawn.enabled == Some(false) => Marking::None,
            Border::Drawn(drawn) => {
                let length = |value: Option<f32>, built_in: f32, what: &str| {
                    let value = value.unwrap_or(built_in);
                    match value.is_finite() && (0. ..=WIDEST_BORDER).contains(&value) {
                        true => value.round() as i32,
                        false => {
                            report!(
                                "`tiling.border.{what}` is not a number from 0 to {WIDEST_BORDER}, so it is {built_in}"
                            );
                            built_in as i32
                        }
                    }
                };
                let color = drawn.color.as_deref().and_then(|text| {
                    let read = color(text);
                    if read.is_none() {
                        report!("`tiling.border.color` is not written as #rrggbb or #rrggbbaa, so it is the primary color of the theme");
                    }
                    read
                });
                Marking::Drawn(DrawnLook {
                    width: length(drawn.width, DEFAULT_BORDER, "width").max(1),
                    offset: length(drawn.offset, 0., "offset"),
                    color,
                })
            }
        };
        let float = self.float.and_then(|size| {
            let usable = |share: f32| share.is_finite() && (0.1..=1.).contains(&share);
            if usable(size.width) && usable(size.height) {
                return Some((size.width, size.height));
            }
            report!("`tiling.float` gives shares of the space from 0.1 to 1, so floated windows go back where they were");
            None
        });
        let step = self.resize_step.unwrap_or(DEFAULT_STEP);
        let (least, most) = STEPS;
        let step = match step.is_finite() && (least..=most).contains(&step) {
            true => step,
            false => {
                report!(
                    "`tiling.resize_step` is not a number from {least} to {most}, so it is {DEFAULT_STEP}"
                );
                DEFAULT_STEP
            }
        };
        let height = self.stack_bar_height.unwrap_or(DEFAULT_BAR);
        let (low, high) = BARS;
        let height = match height.is_finite() && (low..=high).contains(&height) {
            true => height.round() as i32,
            false => {
                report!(
                    "`tiling.stack_bar_height` is not a number from {low} to {high}, so it is {DEFAULT_BAR}"
                );
                DEFAULT_BAR as i32
            }
        };
        Knobs {
            marking,
            float,
            drag: self.drag,
            step,
            bars: Bars {
                shown: self.stack_bar,
                height,
            },
        }
    }

    /// What the configuration says of the workspace at `workspace`,
    /// counted from 0, of `display`: what `[tiling]` says, then each entry
    /// of `[[tiling.display]]` that names the display, then each of
    /// `[[tiling.workspace]]` that names the workspace, a later one
    /// having the last word. Values that are not usable are left out;
    /// [`TilingConfig::check`] reports them.
    pub fn place(&self, display: &DisplayName, workspace: usize) -> Place {
        let displays = { self.display.iter() }
            .filter(|entry| entry.selector.matches(display))
            .map(DisplayTiling::part);
        let named = |entry: &&WorkspaceTiling| {
            entry.number == workspace + 1
                && entry
                    .display
                    .as_ref()
                    .is_none_or(|selector| selector.matches(display))
        };
        let workspaces = self
            .workspace
            .iter()
            .filter(named)
            .map(WorkspaceTiling::part);
        self.resolve(displays.chain(workspaces), false)
    }

    /// Reports every value that is not usable, of `[tiling]` and of each
    /// of its entries.
    pub fn check(&self) {
        let displays = self.display.iter().map(DisplayTiling::part);
        let workspaces = self.workspace.iter().map(WorkspaceTiling::part);
        self.resolve(displays.chain(workspaces), true);
    }

    /// What `[tiling]` says with `entries` over it, in order; what is not
    /// usable is reported when `loud`.
    fn resolve<'a>(&'a self, entries: impl Iterator<Item = Part<'a>>, loud: bool) -> Place {
        let inset =
            |sides: Option<Sides>, what: &str| sides.and_then(|sides| sides.inset(what, loud));
        let mut gap_set = self.gap.and_then(|value| gap(value, "tiling.gap", loud));
        let mut padding = inset(self.padding, "tiling.padding");
        let mut margin = inset(self.margin, "tiling.margin");
        let mut layout = self.layout.clone();
        for entry in entries {
            let what = entry.what;
            if let Some(chosen) = entry.layout {
                layout = chosen.clone();
            }
            let given = entry
                .gap
                .and_then(|value| gap(value, &format!("{what}.gap"), loud));
            gap_set = given.or(gap_set);
            padding = inset(entry.padding, &format!("{what}.padding")).or(padding);
            margin = inset(entry.margin, &format!("{what}.margin")).or(margin);
        }
        let gap = gap_set.unwrap_or(DEFAULT_GAP as i32);
        Place {
            layout,
            gap,
            padding: padding.unwrap_or(Inset::all(gap)),
            margin: margin.unwrap_or_default(),
        }
    }

    /// The layouts the configuration names, to check that each exists.
    pub fn layouts(&self) -> Vec<&LayoutChoice> {
        let displays = self
            .display
            .iter()
            .filter_map(|entry| entry.layout.as_ref());
        let workspaces = self
            .workspace
            .iter()
            .filter_map(|entry| entry.layout.as_ref());
        std::iter::once(&self.layout)
            .chain(displays)
            .chain(workspaces)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiling::layouts::TileLayout;

    fn config(text: &str) -> TilingConfig {
        toml::from_str(text).unwrap()
    }

    fn display(position: usize) -> DisplayName {
        DisplayName {
            position,
            ..DisplayName::default()
        }
    }

    #[test]
    fn the_configuration_chooses_the_gap_and_the_room_around() {
        let all = config("");
        assert!(!all.enabled);
        assert_eq!(all.layout, LayoutChoice::Builtin(TileLayout::Master));
        let place = all.place(&display(1), 0);
        assert_eq!(
            (place.gap, place.padding, place.margin),
            (8, Inset::all(8), Inset::default())
        );

        let some = config("gap = 0\npadding = { top = 4, left = 10 }\nmargin = 20");
        let place = some.place(&display(1), 0);
        assert_eq!(place.gap, 0);
        let padding = Inset {
            top: 4,
            right: 0,
            bottom: 0,
            left: 10,
        };
        assert_eq!((place.padding, place.margin), (padding, Inset::all(20)));
        let work = ScreenRect {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 600,
        };
        let area = place.area(work);
        assert_eq!(
            (area.left, area.top, area.right, area.bottom),
            (30, 24, 980, 580)
        );

        assert_eq!(config("gap = -3").place(&display(1), 0).gap, 8);
        assert_eq!(
            config("layout = \"grid\"").layout,
            LayoutChoice::Builtin(TileLayout::Grid)
        );
        assert!(toml::from_str::<TilingConfig>("gaps = 1").is_err());
        assert!(toml::from_str::<TilingConfig>("padding = { up = 1 }").is_err());
        assert!(toml::from_str::<TilingConfig>("drag = \"throw\"").is_err());
    }

    #[test]
    fn displays_and_workspaces_have_the_last_word() {
        let text = r#"
            layout = "columns"
            gap = 4
            [[display]]
            match = "2"
            layout = "rows"
            gap = 12
            [[workspace]]
            number = 3
            layout = "monocle"
            [[workspace]]
            number = 2
            display = "1"
            layout = "tall"
        "#;
        let config = config(text);
        let layout = |position, workspace| config.place(&display(position), workspace).layout;
        assert_eq!(layout(1, 0), LayoutChoice::Builtin(TileLayout::Columns));
        assert_eq!(layout(2, 0), LayoutChoice::Builtin(TileLayout::Rows));
        assert_eq!(config.place(&display(2), 0).gap, 12);
        assert_eq!(config.place(&display(1), 2).gap, 4);
        assert_eq!(layout(2, 2), LayoutChoice::Builtin(TileLayout::Monocle));
        assert_eq!(layout(1, 1), LayoutChoice::File("tall".to_owned()));
        assert_eq!(layout(2, 1), LayoutChoice::Builtin(TileLayout::Rows));
        assert_eq!(config.layouts().len(), 4);
        assert!(toml::from_str::<TilingConfig>("[[display]]\nmatch = \"0\"").is_err());
    }

    #[test]
    fn the_window_in_front_is_marked_as_the_border_says() {
        assert_eq!(config("").knobs().marking, Marking::None);
        assert_eq!(config("border = true").knobs().marking, Marking::Windows);
        let drawn = config("border = { width = 4, offset = 2, color = \"#ff000080\" }");
        assert_eq!(
            drawn.knobs().marking,
            Marking::Drawn(DrawnLook {
                width: 4,
                offset: 2,
                color: Some(0x80ff_0000),
            })
        );
        let plain = config("border = { }").knobs().marking;
        assert_eq!(
            plain,
            Marking::Drawn(DrawnLook {
                width: 3,
                offset: 0,
                color: None,
            })
        );
        assert_eq!(
            config("border = { enabled = false }").knobs().marking,
            Marking::None
        );
        assert!(toml::from_str::<TilingConfig>("border = { thickness = 2 }").is_err());
        assert_eq!(color("#102030"), Some(0xff10_2030));
        assert_eq!(color("102030"), None);
        assert_eq!(color("#1020"), None);
    }

    #[test]
    fn floating_size_drag_and_step_are_checked() {
        let knobs =
            config("float = { width = 0.5, height = 0.6 }\ndrag = \"sort\"\nresize_step = 0.1")
                .knobs();
        assert_eq!(knobs.float, Some((0.5, 0.6)));
        assert_eq!(knobs.drag, DragMode::Sort);
        assert_eq!(knobs.step, 0.1);
        let unusable = config("float = { width = 2, height = 0.5 }\nresize_step = 3").knobs();
        assert_eq!((unusable.float, unusable.step), (None, DEFAULT_STEP));
        assert_eq!(config("").knobs().drag, DragMode::Swap);
        // A share that is not given is the one built in.
        let wide = config("float = { width = 0.9 }").knobs();
        assert_eq!(wide.float, Some((0.9, DEFAULT_FLOAT)));
    }

    #[test]
    fn entries_of_displays_and_workspaces_are_checked() {
        for kind in OverrideKind::ALL {
            assert_eq!(kind.check(&kind.new_entry()), Ok(()));
        }
        let entry = |text: &str| toml::from_str::<toml::Table>(text).unwrap();
        let display = OverrideKind::Display;
        assert!(
            display
                .check(&entry("match = \"2\"\nlayout = \"rows\"\ngap = 4"))
                .is_ok()
        );
        assert!(display.check(&entry("match = \"0\"")).is_err());
        assert!(display.check(&entry("number = 1")).is_err());
        let workspace = OverrideKind::Workspace;
        assert!(
            workspace
                .check(&entry("number = 3\npadding = { top = 2 }"))
                .is_ok()
        );
        assert!(workspace.check(&entry("number = 3\nwidth = 2")).is_err());
        assert!(workspace.check(&entry("layout = \"rows\"")).is_err());
    }

    #[test]
    fn the_stack_bar_and_automatic_stacks_are_read() {
        let built_in = config("").knobs().bars;
        assert_eq!(
            built_in,
            Bars {
                shown: StackBarShown::AsNeeded,
                height: 32,
            }
        );
        assert_eq!((built_in.over(1), built_in.over(2)), (0, 32));
        let always = config("stack_bar = \"always\"\nstack_bar_height = 24")
            .knobs()
            .bars;
        assert_eq!((always.over(1), always.height), (24, 24));
        let never = config("stack_bar = \"never\"").knobs().bars;
        assert_eq!(never.over(5), 0);
        assert_eq!(config("stack_bar_height = 2").knobs().bars.height, 32);
        assert!(toml::from_str::<TilingConfig>("stack_bar = \"sometimes\"").is_err());

        assert!(!config("").auto_stack().any());
        let all = config("auto_stack = true");
        assert!(all.auto_stack().any() && all.auto_stack().takes("browser"));
        let some = config("auto_stack = [\"Browser\", \"terminal\"]");
        assert!(some.auto_stack().takes("browser"));
        assert!(!some.auto_stack().takes("editor"));
        assert!(config("auto_stack = [\"*\"]").auto_stack().takes("editor"));
        assert!(!config("auto_stack = []").auto_stack().any());
        assert!(toml::from_str::<TilingConfig>("auto_stack = 3").is_err());
    }

    #[test]
    fn a_change_of_layouts_is_told_apart_from_other_changes() {
        let base = config("layout = \"rows\"");
        assert!(!base.layouts_differ(&config("layout = \"rows\"\ngap = 3")));
        assert!(base.layouts_differ(&config("layout = \"grid\"")));
        let entry = config("layout = \"rows\"\n[[workspace]]\nnumber = 1\nlayout = \"bsp\"");
        assert!(base.layouts_differ(&entry));
    }
}
