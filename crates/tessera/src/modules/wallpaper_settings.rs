// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The settings of the wallpaper. Widget `wallpaper-settings`, in the
//! popup of the same name in the middle of the display, which the action
//! `wallpaper-settings` opens: where the wallpaper comes from, the
//! collections (a folder is added as one, they are moved and taken away,
//! and one is chosen for all displays or for single ones), how long a
//! picture stays and whether they are shuffled, how a picture is fitted
//! and where it lies, its adjustments, and how videos play.
//!
//! What is chosen is written to `[wallpaper]` of the configuration file,
//! the rest of which stays as it is, and the shell takes it over at once;
//! a slider writes once it has stood still for a moment. Reading and
//! writing the file is done off the thread of the interface.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, Div, FontWeight, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement,
    Styled, WeakEntity, Window, div, px, rgb,
};
use toml_edit::{InlineTable, Value};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::display_frames;
use crate::popup::Popups;
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Slider, icon, list_title, one_line, state_layer, switch};
use crate::wallpaper::{
    self, Blend, Edit, FASTEST, Fit, Flip, PAUSE_COVERAGE, Place, Position, SHORTEST_INTERVAL,
    SLOWEST, Shown, Source, parse_rgba,
};

/// The name of the popup, of its widget and of the action that opens it.
const NAME: &str = "wallpaper-settings";

/// How large the popup is, in logical pixels.
const WIDTH: f32 = 480.;
const HEIGHT: f32 = 680.;

/// How long a slider stands still before what it set is written.
const WRITE_AFTER: Duration = Duration::from_millis(400);

/// How long the popup takes to go before the dialog of Windows that
/// chooses a folder is shown, which would otherwise belong to the popup
/// and close with it.
const BROWSE_AFTER: Duration = Duration::from_millis(350);

/// The longest a picture stays that a slider sets: a day.
const LONGEST_INTERVAL: f32 = 86_400.;

/// The strongest blur a slider sets, in pixels.
const MOST_BLUR: f32 = 48.;

/// How much of a color is laid over the wallpaper when it is chosen
/// first, of 255.
const STRENGTH: u8 = 96;

/// The colors offered to lay over the wallpaper, as red, green and blue.
const SWATCHES: [[u8; 3]; 6] = [
    [0x00, 0x00, 0x00],
    [0xff, 0xff, 0xff],
    [0x10, 0x20, 0x50],
    [0x70, 0x42, 0x14],
    [0x4a, 0x1a, 0x6b],
    [0x0b, 0x5d, 0x5e],
];

/// The side of a small round button, and the height of a chip.
const BUTTON: f32 = 28.;
const CHIP: f32 = 30.;

/// The width of the label before a slider.
const LABEL_WIDTH: f32 = 136.;

const ICON_UP: &str = "\u{e70e}";
const ICON_DOWN: &str = "\u{e70d}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_ADD: &str = "\u{e710}";
const ICON_FOLDER: &str = "\u{e8b7}";
const ICON_NONE: &str = "\u{e711}";

pub struct WallpaperSettingsModule;

impl Module for WallpaperSettingsModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(WallpaperSettings::new).into())?;
        widgets.register_popup(PopupConfig {
            name: NAME.to_owned(),
            place: PopupPlace::Center,
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: NAME.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register(NAME, Action::new(|_, cx| Shell::toggle_popup(NAME, cx)))
    }
}

/// A value a slider sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Knob {
    Interval,
    Blur,
    Brightness,
    Contrast,
    Saturation,
    Strength,
    Speed,
    Volume,
    Coverage,
}

impl Knob {
    /// In the order of [`TRACKS`].
    const ALL: [Self; 9] = [
        Self::Interval,
        Self::Blur,
        Self::Brightness,
        Self::Contrast,
        Self::Saturation,
        Self::Strength,
        Self::Speed,
        Self::Volume,
        Self::Coverage,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|knob| *knob == self).unwrap_or(0)
    }

    fn id(self) -> &'static str {
        match self {
            Self::Interval => "wallpaper-interval",
            Self::Blur => "wallpaper-blur",
            Self::Brightness => "wallpaper-brightness",
            Self::Contrast => "wallpaper-contrast",
            Self::Saturation => "wallpaper-saturation",
            Self::Strength => "wallpaper-strength",
            Self::Speed => "wallpaper-speed",
            Self::Volume => "wallpaper-volume",
            Self::Coverage => "wallpaper-coverage",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Interval => "Change every",
            Self::Blur => "Blur",
            Self::Brightness => "Brightness",
            Self::Contrast => "Contrast",
            Self::Saturation => "Saturation",
            Self::Strength => "Strength",
            Self::Speed => "Speed",
            Self::Volume => "Volume",
            Self::Coverage => "Pause when covered",
        }
    }

    /// The key of `[wallpaper]` it writes; the strength is written in
    /// `overlay`.
    fn key(self) -> &'static str {
        match self {
            Self::Interval => "interval",
            Self::Blur => "blur",
            Self::Brightness => "brightness",
            Self::Contrast => "contrast",
            Self::Saturation => "saturation",
            Self::Strength => "overlay",
            Self::Speed => "speed",
            Self::Volume => "volume",
            Self::Coverage => "pause_coverage",
        }
    }

    /// Where the handle stands for `value`, from 0 to 1.
    fn share_of(self, value: f32) -> f32 {
        let share = match self {
            Self::Interval => {
                let least = SHORTEST_INTERVAL as f32;
                (value.max(least) / least).ln() / (LONGEST_INTERVAL / least).ln()
            }
            Self::Blur => value / MOST_BLUR,
            Self::Brightness | Self::Contrast | Self::Saturation => value / 2.,
            Self::Strength => value / 255.,
            Self::Speed => (value.max(SLOWEST) / SLOWEST).ln() / (FASTEST / SLOWEST).ln(),
            Self::Volume | Self::Coverage => value,
        };
        if share.is_nan() {
            0.
        } else {
            share.clamp(0., 1.)
        }
    }

    /// The value for a handle at `share`, rounded to what is worth
    /// telling apart.
    fn value_at(self, share: f32) -> f32 {
        let share = share.clamp(0., 1.);
        let round = |value: f32, step: f32| (value / step).round() * step;
        match self {
            Self::Interval => {
                let least = SHORTEST_INTERVAL as f32;
                let seconds = least * (LONGEST_INTERVAL / least).powf(share);
                // Whole minutes past a minute, whole hours past an hour.
                let step = if seconds >= 3600. {
                    3600.
                } else if seconds >= 60. {
                    60.
                } else {
                    1.
                };
                round(seconds, step)
            }
            Self::Blur => round(share * MOST_BLUR, 1.),
            Self::Brightness | Self::Contrast | Self::Saturation => round(share * 2., 0.05),
            Self::Strength => round(share * 255., 1.),
            Self::Speed => round(SLOWEST * (FASTEST / SLOWEST).powf(share), 0.05),
            Self::Volume | Self::Coverage => round(share, 0.01),
        }
    }

    /// `value` as the settings show it.
    fn text(self, value: f32) -> String {
        let percent = |share: f32| format!("{}%", (share * 100.).round());
        match self {
            Self::Interval => duration_text(value.round() as u64),
            Self::Blur => format!("{} px", value.round()),
            Self::Brightness | Self::Contrast | Self::Saturation | Self::Volume => percent(value),
            Self::Coverage => percent(value),
            Self::Strength => percent(value / 255.),
            Self::Speed => format!("{:.2}×", value),
        }
    }

    /// What the configuration has for it.
    fn value_in(self, shown: &Shown) -> f32 {
        match self {
            Self::Interval => shown.interval as f32,
            Self::Blur => shown.blur,
            Self::Brightness => shown.brightness,
            Self::Contrast => shown.contrast,
            Self::Saturation => shown.saturation,
            Self::Strength => {
                overlay_of(shown).map_or(f32::from(STRENGTH), |([.., alpha], _)| f32::from(alpha))
            }
            Self::Speed => shown.speed,
            Self::Volume => shown.volume,
            Self::Coverage => shown.pause_coverage,
        }
    }

    /// What is written for `value`: nothing where it is what is taken
    /// without the key.
    fn written(self, value: f32) -> Option<Value> {
        let unset = match self {
            Self::Interval => None,
            Self::Blur => Some(0.),
            Self::Brightness | Self::Contrast | Self::Saturation | Self::Speed | Self::Volume => {
                Some(1.)
            }
            Self::Coverage => Some(PAUSE_COVERAGE),
            Self::Strength => None,
        };
        if unset.is_some_and(|unset| (unset - value).abs() < 1e-3) {
            return None;
        }
        Some(match self {
            Self::Interval | Self::Blur => Value::from(value.round() as i64),
            _ => Value::from((f64::from(value) * 100.).round() / 100.),
        })
    }
}

/// The change of `key`, `interval` or `shuffle`: in the collection `own`
/// where it has the key itself, else in `[wallpaper]`.
fn turns_edit(own: Option<&String>, key: &'static str, value: Option<Value>) -> Edit {
    match own {
        Some(name) => Edit::Collection(name.clone(), key, value),
        None => Edit::Key(key, value),
    }
}

/// `seconds` as hours, minutes and seconds.
fn duration_text(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The color laid over the wallpaper and how, where one is.
fn overlay_of(shown: &Shown) -> Option<([u8; 4], Blend)> {
    let overlay = shown.overlay.as_ref()?;
    Some((parse_rgba(&overlay.color).ok()?, overlay.blend))
}

/// An overlay of `color` laid on as `blend`, as the file writes it.
fn overlay_value([red, green, blue, alpha]: [u8; 4], blend: Blend) -> Value {
    let mut table = InlineTable::new();
    let color = format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}");
    table.insert("color", color.into());
    if blend != Blend::Normal {
        table.insert("blend", blend.name().into());
    }
    Value::InlineTable(table)
}

/// A name for a collection of the folder `folder` that none of `taken`
/// has.
fn collection_name(folder: &std::path::Path, taken: &[String]) -> String {
    let name = { folder.file_name() }
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| folder.display().to_string());
    let free = |candidate: &String| !taken.contains(candidate);
    std::iter::once(name.clone())
        .chain((2..).map(|number| format!("{name} {number}")))
        .find(free)
        .unwrap_or(name)
}

/// Where the handle of each slider is found in the view, and what is told
/// of what it sets: one pair for each of [`Knob::ALL`].
type Track = (
    fn(&mut WallpaperSettings) -> &mut Slider,
    fn(&mut WallpaperSettings, f32, &mut Context<WallpaperSettings>),
);

fn slider_of<const KNOB: usize>(view: &mut WallpaperSettings) -> &mut Slider {
    &mut view.sliders[KNOB]
}

fn slid<const KNOB: usize>(
    view: &mut WallpaperSettings,
    share: f32,
    cx: &mut Context<WallpaperSettings>,
) {
    view.slide(Knob::ALL[KNOB], share, cx);
}

const TRACKS: [Track; 9] = [
    (slider_of::<0>, slid::<0>),
    (slider_of::<1>, slid::<1>),
    (slider_of::<2>, slid::<2>),
    (slider_of::<3>, slid::<3>),
    (slider_of::<4>, slid::<4>),
    (slider_of::<5>, slid::<5>),
    (slider_of::<6>, slid::<6>),
    (slider_of::<7>, slid::<7>),
    (slider_of::<8>, slid::<8>),
];

/// Something to click.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    Source(Source),
    /// The collection at a place for all displays; `None` for the
    /// pictures of `[wallpaper]` itself.
    Collection(Option<usize>),
    Up(usize),
    Down(usize),
    Remove(usize),
    /// Adds a folder as a collection.
    Add,
    Shuffle,
    Fit(Fit),
    Place(Place),
    Flip(Flip),
    /// A color of [`SWATCHES`] laid over, or none.
    Swatch(Option<usize>),
    Blend(Blend),
    Sound,
    /// The collection the display with a number shows; `None` for that
    /// of all displays.
    Display(usize, Option<usize>),
}

/// How a control looks.
enum Face {
    /// A round button with an icon.
    Glyph(&'static str),
    /// A word that is chosen or not.
    Chip(SharedString, bool),
    /// A color that is chosen or not; `None` for no color.
    Swatch(Option<[u8; 3]>, bool),
    /// A place of the grid of positions, chosen or not.
    Cell(bool),
}

/// The settings of the wallpaper.
struct WallpaperSettings {
    /// What the file says, once it was read.
    shown: Option<Shown>,
    failure: Option<SharedString>,
    sliders: Vec<Slider>,
    /// What each slider shows, in its own unit.
    values: [f32; 9],
    /// What is still to be written, and a count of changes that has the
    /// one before last not write.
    pending: Vec<Edit>,
    changes: usize,
    /// How many displays there are.
    displays: usize,
    interactions: HashMap<Control, Interaction>,
    scroll: ScrollHandle,
}

impl WallpaperSettings {
    fn new(cx: &mut Context<Self>) -> Self {
        let theme = *cx.global::<Theme>();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async { (wallpaper::read_settings(), display_frames().len()) })
                .await;
            this.update(cx, |this, cx| {
                let (config, displays) = read;
                this.displays = displays;
                match config {
                    Ok(config) => this.take(config.shown(), cx),
                    Err(error) => this.failure = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            shown: None,
            failure: None,
            sliders: Knob::ALL.iter().map(|_| Slider::new(&theme, 0.)).collect(),
            values: [0.; 9],
            pending: Vec::new(),
            changes: 0,
            displays: 0,
            interactions: HashMap::new(),
            scroll: ScrollHandle::new(),
        }
    }

    /// Shows what the file says, the sliders included.
    fn take(&mut self, shown: Shown, cx: &mut Context<Self>) {
        let theme = *cx.global::<Theme>();
        for (at, knob) in Knob::ALL.into_iter().enumerate() {
            let value = knob.value_in(&shown);
            self.values[at] = value;
            self.sliders[at] = Slider::new(&theme, knob.share_of(value));
        }
        self.shown = Some(shown);
    }

    /// A slider was moved to `share`.
    fn slide(&mut self, knob: Knob, share: f32, cx: &mut Context<Self>) {
        let value = knob.value_at(share);
        if self.values[knob.index()] == value {
            return;
        }
        self.values[knob.index()] = value;
        let edit = match knob {
            Knob::Strength => {
                let (color, blend) = self.overlay();
                let [red, green, blue, _] = color;
                let color = [red, green, blue, value as u8];
                Edit::Key("overlay", Some(overlay_value(color, blend)))
            }
            Knob::Interval => {
                let own = self
                    .shown
                    .as_ref()
                    .and_then(|shown| shown.interval_in.as_ref());
                turns_edit(own, knob.key(), knob.written(value))
            }
            _ => Edit::Key(knob.key(), knob.written(value)),
        };
        self.queue(vec![edit], false, cx);
        cx.notify();
    }

    /// The color laid over the wallpaper and how: the one chosen, or the
    /// first of the swatches at the strength of the slider.
    fn overlay(&self) -> ([u8; 4], Blend) {
        let chosen = self.shown.as_ref().and_then(overlay_of);
        chosen.unwrap_or_else(|| {
            let [red, green, blue] = SWATCHES[0];
            let strength = self.values[Knob::Strength.index()] as u8;
            ([red, green, blue, strength], Blend::Normal)
        })
    }

    /// Keeps `edits` to be written: at once, or once nothing else was
    /// asked for a moment. A later change of a key replaces the one before.
    fn queue(&mut self, edits: Vec<Edit>, now: bool, cx: &mut Context<Self>) {
        for edit in edits {
            self.pending.retain(|pending| !pending.same_key(&edit));
            self.pending.push(edit);
        }
        self.changes += 1;
        let change = self.changes;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if !now {
                cx.background_executor().timer(WRITE_AFTER).await;
            }
            let edits = this.update(cx, |this, _| {
                (this.changes == change).then(|| std::mem::take(&mut this.pending))
            });
            let Ok(Some(edits)) = edits else {
                return;
            };
            if edits.is_empty() {
                return;
            }
            let written = cx
                .background_executor()
                .spawn(async move { wallpaper::write_settings(edits) })
                .await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(config) => {
                        this.failure = None;
                        // The sliders keep what they show: one may be moved
                        // still.
                        this.shown = Some(config.shown());
                        cx.defer(Shell::config_changed);
                    }
                    Err(error) => {
                        report!("{error}");
                        this.failure = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What clicking `control` changes in the file.
    fn edits(&self, control: &Control) -> Vec<Edit> {
        let Some(shown) = &self.shown else {
            return Vec::new();
        };
        let named = |index: usize| shown.collections.get(index).map(|(name, _)| name.clone());
        match control {
            Control::Source(source) => vec![Edit::Key("source", Some(source.name().into()))],
            Control::Collection(None) => vec![Edit::Key("collection", None)],
            Control::Collection(Some(index)) => match named(*index) {
                Some(name) => vec![
                    Edit::Key("collection", Some(name.into())),
                    Edit::Key("source", Some(Source::Files.name().into())),
                ],
                None => Vec::new(),
            },
            // By name: the list shown is not yet that of the file while a
            // change before is being written.
            Control::Up(index) => named(*index)
                .map(|name| Edit::MoveCollection(name, false))
                .into_iter()
                .collect(),
            Control::Down(index) => named(*index)
                .map(|name| Edit::MoveCollection(name, true))
                .into_iter()
                .collect(),
            Control::Remove(index) => named(*index)
                .map(Edit::RemoveCollection)
                .into_iter()
                .collect(),
            Control::Add => Vec::new(),
            Control::Shuffle => vec![turns_edit(
                shown.shuffle_in.as_ref(),
                "shuffle",
                Some((!shown.shuffle).into()),
            )],
            Control::Fit(fit) => vec![Edit::Key("fit", Some(fit.name().into()))],
            Control::Place(place) => {
                let value = (*place != Place::Center).then(|| place.name().into());
                vec![Edit::Key("position", value)]
            }
            Control::Flip(flip) => {
                let value = (*flip != Flip::None).then(|| flip.name().into());
                vec![Edit::Key("flip", value)]
            }
            Control::Swatch(None) => vec![Edit::Key("overlay", None)],
            Control::Swatch(Some(index)) => {
                let (color, blend) = self.overlay();
                let [red, green, blue] = SWATCHES[*index];
                let color = [red, green, blue, color[3].max(1)];
                vec![Edit::Key("overlay", Some(overlay_value(color, blend)))]
            }
            Control::Blend(blend) => {
                let (color, _) = self.overlay();
                vec![Edit::Key("overlay", Some(overlay_value(color, *blend)))]
            }
            Control::Sound => vec![Edit::Key("sound", Some((!shown.sound).into()))],
            Control::Display(number, None) => vec![Edit::Display(*number, "collection", None)],
            Control::Display(number, Some(index)) => match named(*index) {
                Some(name) => vec![Edit::Display(*number, "collection", Some(name.into()))],
                None => Vec::new(),
            },
        }
    }

    fn act(&mut self, control: &Control, cx: &mut Context<Self>) {
        if *control == Control::Add {
            self.add_folder(cx);
            return;
        }
        let edits = self.edits(control);
        if !edits.is_empty() {
            self.queue(edits, true, cx);
        }
    }

    /// Closes the popup, has Windows choose folders, adds each as a
    /// collection and opens the popup again.
    fn add_folder(&mut self, cx: &mut Context<Self>) {
        let taken: Vec<String> = { self.shown.iter() }
            .flat_map(|shown| shown.collections.iter().map(|(name, _)| name.clone()))
            .collect();
        let prompt = Language::of(cx).say("Add");
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(BROWSE_AFTER).await;
            let options = PathPromptOptions {
                files: false,
                directories: true,
                multiple: true,
                prompt: Some(SharedString::new_static(prompt)),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let folders: Vec<PathBuf> = match chosen.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Err(error)) => {
                    report!("no folder can be chosen: {error:#}");
                    Vec::new()
                }
                _ => Vec::new(),
            };
            let mut names = taken;
            let mut edits = Vec::new();
            for folder in folders {
                let name = collection_name(&folder, &names);
                names.push(name.clone());
                edits.push(Edit::AddCollection(name, vec![folder]));
            }
            if !edits.is_empty() {
                let written = cx
                    .background_executor()
                    .spawn(async move { wallpaper::write_settings(edits) })
                    .await;
                if let Err(error) = written {
                    report!("{error}");
                }
            }
            cx.update(|cx| {
                Shell::config_changed(cx);
                Shell::toggle_popup(NAME, cx);
            })
            .ok();
        })
        .detach();
    }

    /// The control `control`, which looks as `face` says.
    fn control(
        &mut self,
        control: Control,
        face: Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let state = self
            .interactions
            .entry(control.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let base = div()
            .id(SharedString::from(format!("wallpaper-{control:?}")))
            .flex_none()
            .flex()
            .items_center()
            .justify_center();
        let element = match face {
            Face::Glyph(glyph) => base
                .size(px(BUTTON))
                .rounded(theme.shape.full)
                .bg(state_layer(&theme, colors.surface_container, state))
                .text_color(colors.on_surface_variant)
                .child(icon(&theme, glyph).text_size(theme.typography.label_size)),
            Face::Chip(text, chosen) => {
                let (ground, color) = if chosen {
                    (colors.secondary_container, colors.primary)
                } else {
                    (colors.surface_container_highest, colors.on_surface)
                };
                base.h(px(CHIP))
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .rounded(theme.shape.full)
                    .bg(state_layer(&theme, ground, state))
                    .text_color(color)
                    .child(text)
            }
            Face::Swatch(color, chosen) => {
                let ring = if chosen {
                    colors.primary
                } else {
                    colors.outline
                };
                let ground = match color {
                    Some([red, green, blue]) => {
                        rgb(u32::from_be_bytes([0, red, green, blue])).into()
                    }
                    None => colors.surface_container_highest,
                };
                let swatch = base
                    .size(px(BUTTON))
                    .rounded(theme.shape.full)
                    .border_2()
                    .border_color(ring)
                    .bg(state_layer(&theme, ground, state));
                match color {
                    Some(_) => swatch,
                    None => swatch
                        .text_color(colors.on_surface_variant)
                        .child(icon(&theme, ICON_NONE).text_size(theme.typography.label_size)),
                }
            }
            Face::Cell(chosen) => {
                let ground = if chosen {
                    colors.primary
                } else {
                    colors.surface_container_highest
                };
                base.size(px(BUTTON * 0.8))
                    .rounded(theme.shape.small)
                    .bg(state_layer(&theme, ground, state))
            }
        };
        let action = control.clone();
        let element = element.on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
        Interaction::track(element, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
    }

    /// Chips in a row that wraps.
    fn chips(
        &mut self,
        choices: Vec<(Control, &'static str, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let chips: Vec<Stateful<Div>> = { choices.into_iter() }
            .map(|(control, text, chosen)| {
                let face = Face::Chip(SharedString::new_static(language.say(text)), chosen);
                self.control(control, face, window, cx)
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .gap(theme.spacing.tiny)
            .children(chips)
    }

    /// A slider with its label and the value it shows.
    fn knob(
        &mut self,
        knob: Knob,
        dimmed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let at = knob.index();
        let value = self.values[at];
        let (of, set) = TRACKS[at];
        self.sliders[at].dim(dimmed);
        let track = self.sliders[at].track(knob.id(), knob.share_of(value), window, cx, of, set);
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_none()
                    .w(px(LABEL_WIDTH))
                    .child(one_line(language.say(knob.label()))),
            )
            .child(track)
            .child(
                div()
                    .flex_none()
                    .w(theme.slider.value_width + px(16.))
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .child(knob.text(value)),
            )
    }

    /// A label before something that is not a slider.
    fn labelled(&self, label: &'static str, item: impl IntoElement, cx: &Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_none()
                    .w(px(LABEL_WIDTH))
                    .child(one_line(Language::of(cx).say(label))),
            )
            .child(div().flex_1().min_w_0().child(item))
    }

    fn sources(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let choices = { Source::ALL.into_iter() }
            .map(|source| {
                (
                    Control::Source(source),
                    source_label(source),
                    shown.source == source,
                )
            })
            .collect();
        self.chips(choices, window, cx)
    }

    fn collections(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let count = shown.collections.len();
        let mut rows = Vec::with_capacity(count + 2);
        let own = shown.collection.is_none();
        let face = Face::Chip(SharedString::new_static(language.say("Own pictures")), own);
        rows.push(
            div()
                .flex()
                .child(self.control(Control::Collection(None), face, window, cx))
                .into_any_element(),
        );
        for (index, (name, first)) in shown.collections.iter().enumerate() {
            let chosen = shown.collection.as_ref() == Some(name);
            let face = Face::Chip(name.clone().into(), chosen);
            let pick = self.control(Control::Collection(Some(index)), face, window, cx);
            let note = first
                .as_ref()
                .map(|first| first.display().to_string())
                .unwrap_or_default();
            let mut buttons = Vec::new();
            if index > 0 {
                buttons.push(self.control(Control::Up(index), Face::Glyph(ICON_UP), window, cx));
            }
            if index + 1 < count {
                let down = Face::Glyph(ICON_DOWN);
                buttons.push(self.control(Control::Down(index), down, window, cx));
            }
            let remove = Face::Glyph(ICON_REMOVE);
            buttons.push(self.control(Control::Remove(index), remove, window, cx));
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(pick)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(note)),
                    )
                    .children(buttons)
                    .into_any_element(),
            );
        }
        let add = self.control(Control::Add, Face::Glyph(ICON_ADD), window, cx);
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .child(add)
                .child(icon(&theme, ICON_FOLDER).text_color(theme.colors.on_surface_variant))
                .child(one_line(language.say("Add a folder as a collection")))
                .into_any_element(),
        );
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .children(rows)
    }

    fn positions(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let chosen = match shown.position {
            Some(Position::Named(place)) => Some(place),
            None => Some(Place::Center),
            // A place given in percent is none of the grid.
            Some(Position::At(_)) => None,
        };
        let cells: Vec<Stateful<Div>> = { Place::GRID.into_iter() }
            .map(|place| {
                let face = Face::Cell(chosen == Some(place));
                self.control(Control::Place(place), face, window, cx)
            })
            .collect();
        let mut rows = Vec::new();
        let mut cells = cells.into_iter();
        for _ in 0..3 {
            rows.push(
                div()
                    .flex()
                    .gap(theme.spacing.tiny)
                    .children(cells.by_ref().take(3)),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .children(rows)
    }

    fn overlays(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let current = overlay_of(shown);
        let none = self.control(
            Control::Swatch(None),
            Face::Swatch(None, current.is_none()),
            window,
            cx,
        );
        let mut swatches = vec![none];
        for (index, color) in SWATCHES.into_iter().enumerate() {
            let chosen =
                current.is_some_and(|([red, green, blue, _], _)| [red, green, blue] == color);
            let face = Face::Swatch(Some(color), chosen);
            swatches.push(self.control(Control::Swatch(Some(index)), face, window, cx));
        }
        let blend = current.map_or(Blend::Normal, |(_, blend)| blend);
        let blends = { Blend::ALL.into_iter() }
            .map(|kind| (Control::Blend(kind), blend_label(kind), kind == blend))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.spacing.tiny)
                    .children(swatches),
            )
            .child(self.knob(Knob::Strength, current.is_none(), window, cx))
            .child(self.chips(blends, window, cx))
    }

    fn sound(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(Control::Sound)
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let travel = if shown.sound { 1. } else { 0. };
        let toggle = switch(&theme, "wallpaper-sound", travel, state)
            .on_click(cx.listener(|this, _, _, cx| this.act(&Control::Sound, cx)));
        let toggle = Interaction::track(toggle, cx, |this: &mut Self| {
            this.interactions.get_mut(&Control::Sound)
        });
        self.labelled("Sound", div().flex().child(toggle), cx)
    }

    fn shuffle(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(Control::Shuffle)
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let travel = if shown.shuffle { 1. } else { 0. };
        let toggle = switch(&theme, "wallpaper-shuffle", travel, state)
            .on_click(cx.listener(|this, _, _, cx| this.act(&Control::Shuffle, cx)));
        let toggle = Interaction::track(toggle, cx, |this: &mut Self| {
            this.interactions.get_mut(&Control::Shuffle)
        });
        self.labelled("Shuffle", div().flex().child(toggle), cx)
    }

    fn displays(&mut self, shown: &Shown, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut rows = Vec::new();
        for number in 1..=self.displays {
            let chosen = { shown.displays.iter() }
                .find(|(of, _)| *of == number)
                .and_then(|(_, collection)| collection.as_ref());
            let all = Face::Chip(
                SharedString::new_static(language.say("As on all displays")),
                chosen.is_none(),
            );
            let mut chips = vec![self.control(Control::Display(number, None), all, window, cx)];
            for (index, (name, _)) in shown.collections.iter().enumerate() {
                let face = Face::Chip(name.clone().into(), chosen == Some(name));
                chips.push(self.control(Control::Display(number, Some(index)), face, window, cx));
            }
            let title = fill(language.say("Display {}"), &[&number.to_string()]);
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.tiny)
                    .child(one_line(title))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(theme.spacing.tiny)
                            .children(chips),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .children(rows)
    }
}

/// The label of a source.
fn source_label(source: Source) -> &'static str {
    match source {
        Source::Files => "Pictures and videos",
        Source::Windows => "Wallpaper of Windows",
        Source::Daily => "Picture of the day",
        Source::Color => "Color",
    }
}

fn fit_label(fit: Fit) -> &'static str {
    match fit {
        Fit::Cover => "Cover",
        Fit::Contain => "Contain",
        Fit::Stretch => "Stretch",
        Fit::Center => "Center",
        Fit::Tile => "Tile",
        Fit::Span => "Span",
    }
}

fn flip_label(flip: Flip) -> &'static str {
    match flip {
        Flip::None => "None",
        Flip::Horizontal => "Horizontal",
        Flip::Vertical => "Vertical",
        Flip::Both => "Both",
    }
}

fn blend_label(blend: Blend) -> &'static str {
    match blend {
        Blend::Normal => "Normal",
        Blend::Multiply => "Multiply",
        Blend::Screen => "Screen",
        Blend::Overlay => "Overlay",
        Blend::Darken => "Darken",
        Blend::Lighten => "Lighten",
        Blend::SoftLight => "Soft light",
    }
}

impl Render for WallpaperSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let title = div()
            .flex_none()
            .px(theme.spacing.small)
            .pb(theme.spacing.small)
            .font_weight(FontWeight::MEDIUM)
            .child(one_line(language.say("Wallpaper settings")));
        let failure = self.failure.clone().map(|failure| {
            div()
                .flex_none()
                .px(theme.spacing.small)
                .text_color(theme.colors.on_surface_variant)
                .child(failure)
        });
        let mut body = div()
            .id("wallpaper-settings-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .px(theme.spacing.small);
        if let Some(shown) = self.shown.clone() {
            let fits = { Fit::ALL.into_iter() }
                .map(|fit| (Control::Fit(fit), fit_label(fit), shown.fit == fit))
                .collect();
            let flips = { Flip::ALL.into_iter() }
                .map(|flip| (Control::Flip(flip), flip_label(flip), shown.flip == flip))
                .collect();
            let positioned = shown.fit.takes_position();
            let sources = self.sources(&shown, window, cx);
            let collections = self.collections(&shown, window, cx);
            let interval = self.knob(Knob::Interval, false, window, cx);
            let shuffle = self.shuffle(&shown, window, cx);
            let fits = self.chips(fits, window, cx);
            let positions = self.positions(&shown, window, cx);
            let adjustments: Vec<Div> = [Knob::Blur, Knob::Brightness, Knob::Contrast]
                .into_iter()
                .chain([Knob::Saturation])
                .map(|knob| self.knob(knob, false, window, cx))
                .collect();
            let flips = self.chips(flips, window, cx);
            let overlays = self.overlays(&shown, window, cx);
            let speed = self.knob(Knob::Speed, false, window, cx);
            let sound = self.sound(&shown, window, cx);
            let volume = self.knob(Knob::Volume, !shown.sound, window, cx);
            let coverage = self.knob(Knob::Coverage, false, window, cx);
            let displays = self.displays(&shown, window, cx);
            let note = |text: &'static str| {
                div()
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.on_surface_variant)
                    .child(language.say(text))
            };
            body = body
                .child(list_title(&theme, language.say("Source")))
                .child(sources)
                .child(list_title(&theme, language.say("Collections")))
                .child(collections)
                .child(interval)
                .child(shuffle)
                .child(list_title(&theme, language.say("Fit")))
                .child(fits)
                .child(self.labelled(
                    "Position",
                    positions.opacity(if positioned { 1. } else { 0.5 }),
                    cx,
                ))
                .child(list_title(&theme, language.say("Adjustments")))
                .children(adjustments)
                .child(self.labelled("Flip", flips, cx))
                .child(list_title(&theme, language.say("Overlay")))
                .child(overlays)
                .child(list_title(&theme, language.say("Videos")))
                .child(speed)
                .child(sound)
                .child(volume)
                .child(coverage)
                .child(note(
                    "Videos are mirrored, placed and overlaid; blur, brightness, contrast and saturation are not applied to them.",
                ))
                .child(list_title(&theme, language.say("Displays")))
                .child(displays)
                .child(div().h(theme.spacing.small));
        }
        div()
            .id(NAME)
            .size_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(title)
            .children(failure)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_opens_the_settings() {
        let mut actions = Actions::default();
        WallpaperSettingsModule.actions(&mut actions).unwrap();
        assert_eq!(actions.check(NAME, None), Ok(()));
        assert!(!actions.takes_reserved_keys(NAME));
    }

    #[test]
    fn a_slider_comes_back_to_the_value_it_was_given() {
        for knob in Knob::ALL {
            for value in [0., 0.5, 1., 1.5, 2.] {
                let share = knob.share_of(value);
                assert!((0. ..=1.).contains(&share), "{knob:?} {value} {share}");
            }
        }
        let interval = Knob::Interval;
        assert_eq!(interval.value_at(0.), SHORTEST_INTERVAL as f32);
        assert_eq!(interval.value_at(1.), LONGEST_INTERVAL);
        let ten_minutes = interval.share_of(600.);
        assert_eq!(interval.value_at(ten_minutes), 600.);
        assert_eq!(Knob::Speed.value_at(Knob::Speed.share_of(1.)), 1.);
        assert_eq!(Knob::Speed.value_at(0.), SLOWEST);
        assert_eq!(Knob::Speed.value_at(1.), FASTEST);
        assert_eq!(Knob::Blur.value_at(0.5), 24.);
        assert_eq!(Knob::Brightness.value_at(Knob::Brightness.share_of(1.)), 1.);
    }

    #[test]
    fn a_value_that_is_taken_without_its_key_is_not_written() {
        assert!(Knob::Blur.written(0.).is_none());
        assert!(Knob::Brightness.written(1.).is_none());
        assert!(Knob::Coverage.written(PAUSE_COVERAGE).is_none());
        assert_eq!(Knob::Blur.written(12.).unwrap().as_integer(), Some(12));
        assert_eq!(Knob::Contrast.written(1.25).unwrap().as_float(), Some(1.25));
        assert_eq!(
            Knob::Interval.written(600.).unwrap().as_integer(),
            Some(600)
        );
    }

    #[test]
    fn values_are_shown_in_their_units() {
        assert_eq!(Knob::Interval.text(5.), "0:05");
        assert_eq!(Knob::Interval.text(600.), "10:00");
        assert_eq!(Knob::Interval.text(5400.), "1:30:00");
        assert_eq!(Knob::Blur.text(12.), "12 px");
        assert_eq!(Knob::Brightness.text(1.25), "125%");
        assert_eq!(Knob::Strength.text(255.), "100%");
        assert_eq!(Knob::Speed.text(1.5), "1.50×");
    }

    #[test]
    fn an_overlay_is_written_as_a_color_with_its_alpha() {
        let value = overlay_value([0x10, 0x20, 0x30, 0x80], Blend::Multiply);
        let table = value.as_inline_table().unwrap();
        assert_eq!(table.get("color").unwrap().as_str(), Some("#10203080"));
        assert_eq!(table.get("blend").unwrap().as_str(), Some("multiply"));
        let normal = overlay_value([0, 0, 0, 255], Blend::Normal);
        assert!(normal.as_inline_table().unwrap().get("blend").is_none());
    }

    #[test]
    fn a_new_collection_gets_a_name_no_other_has() {
        let folder = std::path::Path::new(r"C:\Pictures\Sea");
        assert_eq!(collection_name(folder, &[]), "Sea");
        let taken = ["Sea".to_owned(), "Sea 2".to_owned()];
        assert_eq!(collection_name(folder, &taken), "Sea 3");
    }
}
