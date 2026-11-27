// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::bail;
use gpui::{Global, Hsla, Pixels, px, rgb};
use serde::Deserialize;

use crate::config::{Config, read_user_file, theme_file};
use crate::motion::SpringSpec;
use crate::options::{Kind, OptionDecl, OptionSet, UNSET, text_of};
use crate::platform::{accent_color, light_mode};
use crate::wallpaper::wallpaper_color;

mod contrast;
pub use contrast::Ground;
mod tonal;

/// The theme name that stands for colors derived from the accent color of
/// Windows instead of for a file.
pub const ACCENT_THEME: &str = "accent";

/// What a theme file writes as its seed to follow the accent color.
const ACCENT_SEED: &str = "accent";

/// The theme name that stands for colors derived from the wallpaper, and
/// what a theme file writes as its seed for that.
pub const WALLPAPER_THEME: &str = "wallpaper";
const WALLPAPER_SEED: &str = "wallpaper";

/// The name the settings of the built-in theme are kept under in
/// `[theme_options]`, where no theme file is named.
pub const BUILTIN_THEME: &str = "builtin";

/// What the built-in theme and the themes of the colors of Windows and of
/// the wallpaper declare can be set. The latter two leave out the accent
/// color, which Windows gives them.
const BUILTIN_SETTINGS: &str = r##"
[[setting]]
key = "accent"
kind = "color"
label = "Accent color"
description = "Every color is derived from it."
optional = true
unset_label = "Built-in"
affects = ["seed"]

[[setting]]
key = "scheme"
kind = "choice"
label = "Scheme"
affects = ["scheme"]
choices = [
    { value = "unset", label = "Default" },
    { value = "dark", label = "Dark" },
    { value = "light", label = "Light" },
    { value = "system", label = "As Windows" },
]

[[setting]]
key = "corners"
kind = "slider"
label = "Corner roundness"
description = "Multiplies the radii of the corners; fully round ones stay round."
min = 0
max = 2
step = 0.1
default = 1.0
unit = "×"
scale = ["shape.small", "shape.medium", "shape.large"]

[[setting]]
key = "density"
kind = "choice"
label = "Density"
choices = [
    { value = "unset", label = "Default" },
    { value = "compact", label = "Compact", overlay = { spacing = { tiny = 3, small = 6, medium = 12 } } },
    { value = "comfortable", label = "Comfortable", overlay = { spacing = { tiny = 5, small = 10, medium = 20 } } },
]

[[setting]]
key = "font"
kind = "font"
label = "Font"
optional = true
unset_label = "Default"
affects = ["typography.family"]

[[setting]]
key = "bar_height"
kind = "number"
label = "Height of the bar fields"
min = 24
max = 64
step = 2
unit = "px"
optional = true
unset_label = "Default"
affects = ["bar.pill_height"]

[[setting]]
key = "dock_size"
kind = "slider"
label = "Dock size"
description = "Multiplies the height of the dock, its buttons and their icons."
min = 0.6
max = 1.6
step = 0.05
default = 1.0
unit = "×"
scale = ["dock.container_height", "dock.item_size", "dock.icon_size"]
"##;

/// The longest length a theme file may set, in pixels. The built-in radius
/// of fully rounded corners is just below it.
const LENGTH_LIMIT: f32 = 10_000.;

/// What a theme file may set the stiffness of a spring to. Below, nothing
/// seems to move; above, the steps a spring is advanced in are too coarse.
const STIFFNESS_RANGE: (f32, f32) = (20., 5_000.);

/// What a theme file may set the damping ratio of a spring to: from
/// swinging for seconds to creeping.
const DAMPING_RANGE: (f32, f32) = (0.2, 2.);

/// The seed of the built-in colors, for the schemes derived from it.
const BUILT_IN_SEED: [u8; 3] = [0xad, 0xd2, 0x8e];

/// Whether surfaces are dark or light.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Scheme {
    #[default]
    Dark,
    Light,
    /// As the Windows settings say for the taskbar.
    System,
}

/// What of the Windows settings the colors were derived from, so that
/// they can be derived again when it changes. `None` for what they do not
/// depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Followed {
    accent: Option<[u8; 3]>,
    light: Option<bool>,
    /// The color of the wallpaper the colors were derived from, where
    /// they follow it; `Some(None)` while the wallpaper has none.
    wallpaper: Option<Option<[u8; 3]>>,
}

/// Material 3 color roles used by shell surfaces.
#[derive(Clone, Copy, Debug)]
pub struct ColorRoles {
    pub surface_container: Hsla,
    pub surface_container_highest: Hsla,
    pub on_surface: Hsla,
    pub on_surface_variant: Hsla,
    pub outline: Hsla,
    pub primary: Hsla,
    pub secondary_container: Hsla,
}

/// Corner radii.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub small: Pixels,
    pub medium: Pixels,
    pub large: Pixels,
    pub full: Pixels,
}

/// Distances between and inside components.
#[derive(Clone, Copy, Debug)]
pub struct Spacing {
    pub tiny: Pixels,
    pub small: Pixels,
    pub medium: Pixels,
}

/// Text styles.
#[derive(Clone, Copy, Debug)]
pub struct Typography {
    pub family: &'static str,
    pub label_size: Pixels,
    /// The font whose glyphs are the shell's icons.
    pub icon_family: &'static str,
    pub icon_size: Pixels,
}

/// How state changes move. The split into spatial and effects springs, and
/// the bounce of the spatial ones, follow the Material 3 Expressive motion
/// scheme; the numbers are Tessera's own.
#[derive(Clone, Copy, Debug)]
pub struct Motion {
    /// For position and size.
    pub spatial: SpringSpec,
    /// For small, quick position and size changes such as press feedback.
    pub spatial_fast: SpringSpec,
    /// For color and opacity, which must not overshoot.
    pub effects: SpringSpec,
}

/// Bar measurements.
#[derive(Clone, Copy, Debug)]
pub struct BarMetrics {
    pub pill_height: Pixels,
    /// Whether the widgets of a bar stand on it without a field of their
    /// own behind them.
    pub flat: bool,
}

/// Dock measurements.
#[derive(Clone, Copy, Debug)]
pub struct DockMetrics {
    pub container_height: Pixels,
    pub item_size: Pixels,
    pub icon_size: Pixels,
    pub indicator_height: Pixels,
    pub indicator_active_width: Pixels,
}

/// Slider measurements.
#[derive(Clone, Copy, Debug)]
pub struct SliderMetrics {
    pub track_height: Pixels,
    pub handle_width: Pixels,
    /// Also the height of the whole control.
    pub handle_height: Pixels,
    /// Space between the handle and the track on each side.
    pub handle_gap: Pixels,
    /// Corner radius of the track ends that face the handle.
    pub inner_radius: Pixels,
    /// Room for the value shown next to the track.
    pub value_width: Pixels,
}

/// Measurements of the wavy line that shows a level.
#[derive(Clone, Copy, Debug)]
pub struct MeterMetrics {
    pub width: Pixels,
    /// The thickness of the line.
    pub stroke: Pixels,
    /// How far the wave swings to each side of the line.
    pub amplitude: Pixels,
    /// The length of one wave.
    pub wavelength: Pixels,
    /// Space between the wave and the rest of the line.
    pub gap: Pixels,
}

/// Calendar measurements.
#[derive(Clone, Copy, Debug)]
pub struct CalendarMetrics {
    /// Width and height of one day.
    pub cell_size: Pixels,
}

/// Sizes of the application launcher.
#[derive(Clone, Copy, Debug)]
pub struct LauncherMetrics {
    pub field_height: Pixels,
    pub row_height: Pixels,
    pub icon_size: Pixels,
}

/// The appearance shared by every shell surface.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub followed: Followed,
    /// Whether what is read is kept apart from what it lies on.
    pub legible: bool,
    pub colors: ColorRoles,
    pub shape: Shape,
    pub spacing: Spacing,
    pub typography: Typography,
    pub motion: Motion,
    pub bar: BarMetrics,
    pub dock: DockMetrics,
    pub calendar: CalendarMetrics,
    pub slider: SliderMetrics,
    pub meter: MeterMetrics,
    pub launcher: LauncherMetrics,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            followed: Followed::default(),
            legible: false,
            colors: ColorRoles {
                surface_container: rgb(0x1e211a).into(),
                surface_container_highest: rgb(0x33362e).into(),
                on_surface: rgb(0xe2e3d9).into(),
                on_surface_variant: rgb(0xc4c8ba).into(),
                outline: rgb(0x8e9286).into(),
                primary: rgb(0xadd28e).into(),
                secondary_container: rgb(0x404a36).into(),
            },
            shape: Shape {
                small: px(8.),
                medium: px(16.),
                large: px(28.),
                full: px(9999.),
            },
            spacing: Spacing {
                tiny: px(4.),
                small: px(8.),
                medium: px(16.),
            },
            typography: Typography {
                family: "Segoe UI Variable Display",
                label_size: px(14.),
                icon_family: "Segoe Fluent Icons",
                icon_size: px(16.),
            },
            motion: Motion {
                spatial: SpringSpec {
                    stiffness: 380.,
                    damping_ratio: 0.7,
                },
                spatial_fast: SpringSpec {
                    stiffness: 800.,
                    damping_ratio: 0.6,
                },
                effects: SpringSpec {
                    stiffness: 1600.,
                    damping_ratio: 1.,
                },
            },
            bar: BarMetrics {
                pill_height: px(36.),
                flat: false,
            },
            dock: DockMetrics {
                container_height: px(64.),
                item_size: px(48.),
                icon_size: px(32.),
                indicator_height: px(4.),
                indicator_active_width: px(16.),
            },
            calendar: CalendarMetrics { cell_size: px(36.) },
            slider: SliderMetrics {
                track_height: px(16.),
                handle_width: px(4.),
                handle_height: px(44.),
                handle_gap: px(6.),
                inner_radius: px(2.),
                value_width: px(44.),
            },
            meter: MeterMetrics {
                width: px(44.),
                stroke: px(3.),
                amplitude: px(2.5),
                wavelength: px(14.),
                gap: px(4.),
            },
            launcher: LauncherMetrics {
                field_height: px(44.),
                row_height: px(44.),
                icon_size: px(28.),
            },
        }
    }
}

impl Global for Theme {}

impl Theme {
    /// `fill` as the field behind a widget of a bar has it: itself, or
    /// nothing where the bars are flat.
    pub fn on_bar(&self, fill: Hsla) -> Hsla {
        if self.bar.flat {
            // The content color at no strength, so that what is mixed
            // into it under the pointer tints and does not darken.
            self.colors.on_surface.opacity(0.)
        } else {
            fill
        }
    }

    /// Whether `text` is a theme file whose settings can be used. What of
    /// it is out of range is not looked at: that is reported when the theme
    /// is applied.
    pub fn check(text: &str) -> anyhow::Result<()> {
        let file = toml::from_str::<ThemeFile>(text)?;
        if let Some(problem) = file.declared().1.into_iter().next() {
            bail!("{problem}");
        }
        Ok(())
    }

    /// The theme `config` asks for: the built-in theme, the theme it names
    /// over it, and each theme stacked over that in turn, each with the
    /// values its settings are given. With `contrast`, colors that are
    /// read are then moved away from what they lie on until they have the
    /// contrast WCAG 2.1 asks for. A theme that cannot be used is
    /// reported and left out.
    pub fn from_config(config: &Config) -> Self {
        let base = config.theme.as_deref().unwrap_or(BUILTIN_THEME);
        let layers = config.theme_layers.iter().map(String::as_str);
        let mut merge = Merge::default();
        for name in std::iter::once(base).chain(layers) {
            if let Some(file) = source(name) {
                merge.layer(name, &file, config.theme_options.get(name));
            }
        }
        merge.finish(config.contrast)
    }

    /// Whether the widgets of a bar lie on the desktop and are to be read
    /// there: the bar then looks at what is under it.
    pub fn grounds_bars(&self) -> bool {
        self.bar.flat && self.legible
    }

    /// Whether the text of a widget without a field is read on `ground`
    /// between the pixels `from` and `to`.
    pub fn is_read_on(&self, ground: &Ground, from: f32, to: f32) -> bool {
        let contents = [self.colors.on_surface, self.colors.on_surface_variant];
        ground.carries(&contents, from, to, contrast::TEXT)
    }

    /// Derives the colors of the theme `name` from `seed`, a color or
    /// [`ACCENT_SEED`], as `scheme` says. What Windows does not tell or the
    /// file gets wrong is reported and replaced by what is built in. A dark
    /// theme without a seed keeps the built-in colors.
    fn derive(&mut self, seed: Option<&str>, scheme: Scheme, name: &str) {
        let light = match scheme {
            Scheme::Dark => false,
            Scheme::Light => true,
            Scheme::System => {
                let light = light_mode();
                if light.is_none() {
                    report!("Windows does not tell whether it is light or dark");
                }
                // Remembered even if untold, so that it is noticed once
                // Windows tells.
                self.followed.light = Some(light.unwrap_or(false));
                light.unwrap_or(false)
            }
        };
        let seed = match seed {
            None => None,
            Some(ACCENT_SEED) => {
                let accent = accent_color();
                if accent.is_none() {
                    report!("Windows does not tell its accent color");
                }
                self.followed.accent = Some(accent.unwrap_or(BUILT_IN_SEED));
                accent
            }
            Some(WALLPAPER_SEED) => {
                // Until a picture is read, the accent color stands in, and
                // is followed as well while it does.
                let color = wallpaper_color();
                self.followed.wallpaper = Some(color);
                color.or_else(|| {
                    let accent = accent_color();
                    self.followed.accent = Some(accent.unwrap_or(BUILT_IN_SEED));
                    accent
                })
            }
            Some(seed) => parse_rgb(seed)
                .inspect_err(|error| report!("theme `{name}`: {error:#}"))
                .ok(),
        };
        if seed.is_some() || light {
            self.colors = tonal::roles(seed.unwrap_or(BUILT_IN_SEED), light);
        }
    }

    /// Whether the colors were derived from settings of Windows that have
    /// changed since.
    pub fn is_stale(&self) -> bool {
        let Followed {
            accent,
            light,
            wallpaper,
        } = self.followed;
        accent.is_some_and(|accent| accent_color().unwrap_or(BUILT_IN_SEED) != accent)
            || light.is_some_and(|light| light_mode().unwrap_or(false) != light)
            || wallpaper.is_some_and(|color| wallpaper_color() != color)
    }

    /// Whether the colors are derived from the wallpaper, which then
    /// reads the wallpaper of Windows for them where it draws nothing.
    pub fn follows_wallpaper(&self) -> bool {
        self.followed.wallpaper.is_some()
    }
}

/// The settings the theme `name` declares, and what is wrong with those
/// that cannot be used; `None` where there is no such theme. The built-in
/// theme is [`BUILTIN_THEME`]. It reads a file, so it is not to be called
/// on the thread that draws.
pub fn declared_settings(name: &str) -> Option<(OptionSet, Vec<String>)> {
    source(name).map(|file| file.declared())
}

/// What the theme `name` is made of: the built-in theme and the themes of
/// the colors of Windows and of the wallpaper are made here, any other is
/// read from its file. One that cannot be read is reported and `None`.
fn source(name: &str) -> Option<ThemeFile> {
    match name {
        BUILTIN_THEME => return Some(ThemeFile::builtin(None)),
        ACCENT_THEME => return Some(ThemeFile::builtin(Some(ACCENT_SEED))),
        WALLPAPER_THEME => return Some(ThemeFile::builtin(Some(WALLPAPER_SEED))),
        _ => {}
    }
    if name.contains(['/', '\\', ':']) || name.starts_with('.') {
        report!("`{name}` is not a theme name, it is left out");
        return None;
    }
    let file = read_user_file::<ThemeFile>(&theme_file(name));
    if file.is_none() {
        report!("theme `{name}` is not loaded, it is left out");
    }
    file
}

/// What a group of tokens is called in the settings: what a setting of a
/// theme that changes the token at `path` is said to affect.
pub fn token_group(path: &str) -> &'static str {
    match path.split('.').next().unwrap_or_default() {
        "seed" | "scheme" | "colors" => "Colors",
        "shape" => "Corners",
        "spacing" => "Spacing",
        "typography" => "Font",
        "motion" => "Motion",
        "bar" => "Bar",
        "dock" => "Dock",
        "calendar" => "Calendar",
        "slider" => "Sliders",
        "meter" => "Meters",
        "launcher" => "Launcher",
        _ => "Other",
    }
}

/// The groups of tokens `setting` changes, each once, in the order they
/// first appear.
pub fn affected_groups(setting: &OptionDecl) -> Vec<&'static str> {
    let overlays = { setting.choices.iter() }.filter_map(|choice| choice.overlay.as_ref());
    let overlaid = overlays.flat_map(|overlay| overlay.keys().map(String::as_str));
    let paths = { setting.affects.iter().chain(&setting.scale) }
        .map(String::as_str)
        .chain(overlaid);
    let mut groups = Vec::new();
    for group in paths.map(token_group) {
        if !groups.contains(&group) {
            groups.push(group);
        }
    }
    groups
}

/// `value` put at the token `path`, such as `colors.primary`, as a theme
/// file would hold it.
fn overlay_at(path: &str, value: toml::Value) -> toml::Table {
    let mut parts: Vec<&str> = path.split('.').collect();
    let last = parts.pop().unwrap_or_default();
    let mut table = toml::Table::new();
    table.insert(last.to_owned(), value);
    for part in parts.into_iter().rev() {
        let mut outer = toml::Table::new();
        outer.insert(part.to_owned(), toml::Value::Table(table));
        table = outer;
    }
    table
}

/// `value` as a token holds it: a color of a theme has no transparency,
/// so what a color setting says of it is dropped.
fn token_value(setting: &OptionDecl, value: &toml::Value) -> toml::Value {
    match (setting.kind, value.as_str()) {
        (Kind::Color, Some(color)) if color.len() == 9 => color[..7].into(),
        _ => value.clone(),
    }
}

/// The themes stacked so far: the theme they make, and the seed and the
/// scheme its colors were last derived from, which a theme over them that
/// sets only one of the two derives the colors again with.
#[derive(Default)]
struct Merge {
    theme: Theme,
    seed: Option<String>,
    scheme: Option<Scheme>,
}

impl Merge {
    /// Applies the theme `name` of `file` over what is stacked so far: its
    /// seed and scheme, its tokens, and then its settings with `values`.
    fn layer(&mut self, name: &str, file: &ThemeFile, values: Option<&toml::Table>) {
        self.tokens(name, file);
        let (declared, problems) = file.declared();
        for problem in problems {
            report!("theme `{name}`: {problem}");
        }
        let empty = toml::Table::new();
        let owner = format!("theme `{name}`");
        let values = declared.resolve(values.unwrap_or(&empty), &owner);
        for setting in &declared.decls {
            if !OptionSet::visible(setting, &values) {
                continue;
            }
            if let Some(value) = values.get(&setting.key)
                && let Err(error) = self.set(name, setting, value)
            {
                report!("theme `{name}`: the setting `{}` {error}", setting.key);
            }
        }
    }

    /// Applies what `file` sets of the colors and the measures. A file that
    /// sets a seed or a scheme derives every color again from the seed and
    /// the scheme that are then the last ones set.
    fn tokens(&mut self, name: &str, file: &ThemeFile) {
        if file.seed.is_some() || file.scheme.is_some() {
            if file.seed.is_some() {
                self.seed.clone_from(&file.seed);
            }
            if file.scheme.is_some() {
                self.scheme = file.scheme;
            }
            self.theme.followed = Followed::default();
            if self.seed.is_none() {
                // Derived without a seed, a dark scheme keeps the built-in
                // colors, which light ones derived below would replace.
                self.theme.colors = Theme::default().colors;
            }
            let scheme = self.scheme.unwrap_or_default();
            self.theme.derive(self.seed.as_deref(), scheme, name);
        }
        if let Err(error) = file.colors.apply_to(&mut self.theme.colors) {
            report!("theme `{name}` is only partly applied: {error:#}");
        }
        for problem in file.apply_measures_to(&mut self.theme) {
            report!("theme `{name}`: {problem}, which is left as it was");
        }
    }

    /// Gives the setting `setting` of the theme `name` the value `value`:
    /// the tokens of its choice, the tokens it scales multiplied by it, or
    /// the tokens it affects set to it.
    fn set(&mut self, name: &str, setting: &OptionDecl, value: &toml::Value) -> Result<(), String> {
        if setting.kind == Kind::Choice {
            let chosen = setting
                .chosen(Some(value))
                .map(|index| &setting.choices[index]);
            if let Some(overlay) = chosen.and_then(|choice| choice.overlay.as_ref()) {
                let file = ThemeFile::from_table(overlay.clone())?;
                self.tokens(name, &file);
                return Ok(());
            }
            if value.as_str() == Some(UNSET) {
                return Ok(());
            }
        }
        if !setting.scale.is_empty() {
            let factor = OptionDecl::number(value).ok_or("scales by no number")?;
            for path in &setting.scale {
                self.theme.scale(path, factor as f32)?;
            }
            return Ok(());
        }
        for path in &setting.affects {
            let file = ThemeFile::from_table(overlay_at(path, token_value(setting, value)))?;
            self.tokens(name, &file);
        }
        Ok(())
    }

    /// The theme the stack makes, its colors made readable once, at the
    /// end, where `legible` asks for it.
    fn finish(self, legible: bool) -> Theme {
        let mut theme = self.theme;
        if legible {
            theme.colors.make_legible();
        }
        theme.legible = legible;
        theme
    }
}

impl Theme {
    /// The length at `path`, such as `shape.small`, that a setting may
    /// multiply. Fully rounded corners are no such length: they stay round.
    fn length_mut(&mut self, path: &str) -> Option<&mut Pixels> {
        Some(match path {
            "shape.small" => &mut self.shape.small,
            "shape.medium" => &mut self.shape.medium,
            "shape.large" => &mut self.shape.large,
            "spacing.tiny" => &mut self.spacing.tiny,
            "spacing.small" => &mut self.spacing.small,
            "spacing.medium" => &mut self.spacing.medium,
            "typography.label_size" => &mut self.typography.label_size,
            "typography.icon_size" => &mut self.typography.icon_size,
            "bar.pill_height" => &mut self.bar.pill_height,
            "dock.container_height" => &mut self.dock.container_height,
            "dock.item_size" => &mut self.dock.item_size,
            "dock.icon_size" => &mut self.dock.icon_size,
            "dock.indicator_height" => &mut self.dock.indicator_height,
            "dock.indicator_active_width" => &mut self.dock.indicator_active_width,
            "calendar.cell_size" => &mut self.calendar.cell_size,
            "slider.track_height" => &mut self.slider.track_height,
            "slider.handle_width" => &mut self.slider.handle_width,
            "slider.handle_height" => &mut self.slider.handle_height,
            "slider.handle_gap" => &mut self.slider.handle_gap,
            "slider.inner_radius" => &mut self.slider.inner_radius,
            "slider.value_width" => &mut self.slider.value_width,
            "meter.width" => &mut self.meter.width,
            "meter.stroke" => &mut self.meter.stroke,
            "meter.amplitude" => &mut self.meter.amplitude,
            "meter.wavelength" => &mut self.meter.wavelength,
            "meter.gap" => &mut self.meter.gap,
            "launcher.field_height" => &mut self.launcher.field_height,
            "launcher.row_height" => &mut self.launcher.row_height,
            "launcher.icon_size" => &mut self.launcher.icon_size,
            _ => return None,
        })
    }

    /// Multiplies the length at `path` by `factor`, within what a theme
    /// may set.
    fn scale(&mut self, path: &str, factor: f32) -> Result<(), String> {
        let length = self
            .length_mut(path)
            .ok_or_else(|| format!("scales `{path}`, which is no length that can be scaled"))?;
        let scaled = (f32::from(*length) * factor).clamp(0., LENGTH_LIMIT);
        *length = px(scaled);
        Ok(())
    }
}

/// A user theme: values to replace in the built-in theme.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    /// A color, `#rrggbb`, to derive every role from, or `accent` for the
    /// accent color of Windows. The roles of `colors` are applied over it.
    seed: Option<String>,
    /// Whether the derived colors are `dark`, `light` or, with `system`,
    /// what the Windows settings say for the taskbar. Dark where neither
    /// this nor a theme below says.
    scheme: Option<Scheme>,
    #[serde(default)]
    colors: ColorOverrides,
    #[serde(default)]
    shape: ShapeOverrides,
    #[serde(default)]
    spacing: SpacingOverrides,
    #[serde(default)]
    typography: TypographyOverrides,
    #[serde(default)]
    motion: MotionOverrides,
    #[serde(default)]
    bar: BarOverrides,
    #[serde(default)]
    dock: DockOverrides,
    #[serde(default)]
    calendar: CalendarOverrides,
    #[serde(default)]
    slider: SliderOverrides,
    #[serde(default)]
    meter: MeterOverrides,
    #[serde(default)]
    launcher: LauncherOverrides,
    /// What can be set of the theme in the settings, and which tokens it
    /// changes.
    #[serde(default, rename = "setting")]
    settings: Vec<OptionDecl>,
}

impl ThemeFile {
    /// What Tessera makes the built-in theme of, or, with `seed`, a theme
    /// that derives its colors from it as Windows says: the settings it
    /// declares, without the accent color where Windows gives the seed.
    fn builtin(seed: Option<&str>) -> Self {
        let file: Self = toml::from_str(BUILTIN_SETTINGS).expect("the built-in settings are valid");
        let mut settings = file.settings;
        if seed.is_some() {
            settings.retain(|setting| !setting.affects.iter().any(|path| path == "seed"));
        }
        Self {
            seed: seed.map(str::to_owned),
            scheme: seed.map(|_| Scheme::System),
            settings,
            ..Self::default()
        }
    }

    /// The tokens a table of a setting holds, written as in a theme file.
    fn from_table(table: toml::Table) -> Result<Self, String> {
        let file: Self = toml::Value::Table(table)
            .try_into()
            .map_err(|error| format!("names a token that is not one: {error}"))?;
        if !file.settings.is_empty() {
            return Err("declares settings inside a setting".to_owned());
        }
        Ok(file)
    }

    /// The settings that can be used, and what is wrong with the others,
    /// each as a sentence: a declaration that is not usable, or one whose
    /// tokens are not tokens of a theme.
    fn declared(&self) -> (OptionSet, Vec<String>) {
        let (declared, mut problems) = OptionSet::checked(self.settings.clone());
        let mut kept = OptionSet::default();
        for setting in declared.decls {
            match Self::check_tokens(&setting) {
                Ok(()) => kept.decls.push(setting),
                Err(error) => problems.push(format!("the setting `{}` {error}", setting.key)),
            }
        }
        (kept, problems)
    }

    /// Whether what `setting` changes are tokens of a theme that a value
    /// of its kind can be given.
    fn check_tokens(setting: &OptionDecl) -> Result<(), String> {
        if matches!(setting.kind, Kind::List | Kind::Pairs | Kind::Path) {
            return Err("is of a kind no token of a theme has".to_owned());
        }
        let sample: toml::Value = match setting.kind {
            Kind::Color => "#000000".into(),
            Kind::Toggle => true.into(),
            Kind::Number | Kind::Slider => 1.0.into(),
            Kind::Choice => { setting.choices.iter() }
                .find(|choice| !choice.is_unset())
                .map_or_else(|| "".into(), |choice| choice.value.clone()),
            _ => "Segoe UI".into(),
        };
        for path in &setting.affects {
            Self::from_table(overlay_at(path, sample.clone()))
                .map_err(|error| format!("affects `{path}`, which {error}"))?;
        }
        let mut probe = Theme::default();
        for path in &setting.scale {
            probe.scale(path, 1.)?;
        }
        for choice in &setting.choices {
            if let Some(overlay) = &choice.overlay {
                Self::from_table(overlay.clone()).map_err(|error| {
                    let value = text_of(&choice.value);
                    format!("has a choice `{value}` that {error}")
                })?;
            }
        }
        let changes = !setting.affects.is_empty() || !setting.scale.is_empty();
        let overlays = setting
            .choices
            .iter()
            .any(|choice| choice.overlay.is_some());
        if !changes && !overlays {
            return Err("changes no token".to_owned());
        }
        Ok(())
    }

    /// Replaces the shapes, sizes, font and motion that are set and
    /// usable. Returns what is not usable, each as the start of a
    /// sentence.
    fn apply_measures_to(&self, theme: &mut Theme) -> Vec<String> {
        let mut problems = Vec::new();
        self.shape
            .apply_to(&mut theme.shape, "shape", &mut problems);
        self.spacing
            .apply_to(&mut theme.spacing, "spacing", &mut problems);
        self.typography
            .apply_to(&mut theme.typography, &mut problems);
        self.motion.apply_to(&mut theme.motion, &mut problems);
        self.bar.apply_to(&mut theme.bar, &mut problems);
        self.dock.apply_to(&mut theme.dock, "dock", &mut problems);
        self.calendar
            .apply_to(&mut theme.calendar, "calendar", &mut problems);
        self.slider
            .apply_to(&mut theme.slider, "slider", &mut problems);
        self.meter
            .apply_to(&mut theme.meter, "meter", &mut problems);
        self.launcher
            .apply_to(&mut theme.launcher, "launcher", &mut problems);
        problems
    }
}

/// `value` if it lies between `least` and `most`; otherwise `None` and a
/// note in `problems` that names it `name`.
fn within(
    value: f32,
    (least, most): (f32, f32),
    name: &str,
    problems: &mut Vec<String>,
) -> Option<f32> {
    let usable = value.is_finite() && (least..=most).contains(&value);
    if !usable {
        problems.push(format!("`{name}` is not a number from {least} to {most}"));
    }
    usable.then_some(value)
}

/// Declares the lengths, in pixels, that a theme file may replace in one
/// section of the theme: the fields of `$target` that are named.
macro_rules! length_overrides {
    ($(#[$meta:meta])* $name:ident for $target:ty { $($field:ident),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Default, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name {
            $($field: Option<f32>,)+
        }

        impl $name {
            /// Replaces the lengths that are set and usable; the others
            /// are noted in `problems` by `section` and name.
            fn apply_to(&self, target: &mut $target, section: &str, problems: &mut Vec<String>) {
                $(
                    let name = format!("{section}.{}", stringify!($field));
                    let length = self.$field;
                    let length =
                        length.and_then(|length| within(length, (0., LENGTH_LIMIT), &name, problems));
                    if let Some(length) = length {
                        target.$field = px(length);
                    }
                )+
            }
        }
    };
}

length_overrides!(
    /// Replacement corner radii.
    ShapeOverrides for Shape { small, medium, large, full }
);
length_overrides!(
    /// Replacement distances.
    SpacingOverrides for Spacing { tiny, small, medium }
);
/// Replacement bar measurements.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BarOverrides {
    pill_height: Option<f32>,
    flat: Option<bool>,
}

impl BarOverrides {
    fn apply_to(&self, bar: &mut BarMetrics, problems: &mut Vec<String>) {
        let height = self.pill_height;
        let height = height
            .and_then(|height| within(height, (0., LENGTH_LIMIT), "bar.pill_height", problems));
        if let Some(height) = height {
            bar.pill_height = px(height);
        }
        if let Some(flat) = self.flat {
            bar.flat = flat;
        }
    }
}
length_overrides!(
    /// Replacement dock measurements.
    DockOverrides for DockMetrics {
        container_height,
        item_size,
        icon_size,
        indicator_height,
        indicator_active_width,
    }
);
length_overrides!(
    /// Replacement calendar measurements.
    CalendarOverrides for CalendarMetrics { cell_size }
);
length_overrides!(
    /// Replacement slider measurements.
    SliderOverrides for SliderMetrics {
        track_height,
        handle_width,
        handle_height,
        handle_gap,
        inner_radius,
        value_width,
    }
);
length_overrides!(
    /// Replacement measurements of the wavy line.
    MeterOverrides for MeterMetrics { width, stroke, amplitude, wavelength, gap }
);
length_overrides!(
    /// Replacement launcher sizes.
    LauncherOverrides for LauncherMetrics { field_height, row_height, icon_size }
);

/// A replacement font and text sizes. The font of the icons is not
/// replaceable: the shell names its icons by their place in that font.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypographyOverrides {
    /// The name of an installed font.
    family: Option<String>,
    label_size: Option<f32>,
    icon_size: Option<f32>,
}

impl TypographyOverrides {
    fn apply_to(&self, typography: &mut Typography, problems: &mut Vec<String>) {
        match self.family.as_deref().map(str::trim) {
            Some("") => problems.push("`typography.family` names no font".to_owned()),
            Some(family) => typography.family = kept(family),
            None => {}
        }
        let sizes = [
            (self.label_size, &mut typography.label_size, "label_size"),
            (self.icon_size, &mut typography.icon_size, "icon_size"),
        ];
        for (size, target, name) in sizes {
            let name = format!("typography.{name}");
            let size = size.and_then(|size| within(size, (1., LENGTH_LIMIT), &name, problems));
            if let Some(size) = size {
                *target = px(size);
            }
        }
    }
}

/// `name` for as long as the program runs, which is how long a theme may
/// be in use. Each name is kept once, however often themes are read.
fn kept(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    match names.get(name) {
        Some(kept) => kept,
        None => {
            let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
            names.insert(kept);
            kept
        }
    }
}

/// Replacement springs.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MotionOverrides {
    #[serde(default)]
    spatial: SpringOverrides,
    #[serde(default)]
    spatial_fast: SpringOverrides,
    #[serde(default)]
    effects: SpringOverrides,
}

impl MotionOverrides {
    fn apply_to(&self, motion: &mut Motion, problems: &mut Vec<String>) {
        let springs = [
            (&self.spatial, &mut motion.spatial, "spatial"),
            (&self.spatial_fast, &mut motion.spatial_fast, "spatial_fast"),
            (&self.effects, &mut motion.effects, "effects"),
        ];
        for (replacement, spring, name) in springs {
            replacement.apply_to(spring, name, problems);
        }
    }
}

/// What to replace of one spring.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpringOverrides {
    stiffness: Option<f32>,
    damping_ratio: Option<f32>,
}

impl SpringOverrides {
    fn apply_to(&self, spring: &mut SpringSpec, name: &str, problems: &mut Vec<String>) {
        let parts = [
            (
                self.stiffness,
                &mut spring.stiffness,
                "stiffness",
                STIFFNESS_RANGE,
            ),
            (
                self.damping_ratio,
                &mut spring.damping_ratio,
                "damping_ratio",
                DAMPING_RANGE,
            ),
        ];
        for (value, target, part, range) in parts {
            let name = format!("motion.{name}.{part}");
            if let Some(value) = value.and_then(|value| within(value, range, &name, problems)) {
                *target = value;
            }
        }
    }
}

/// Replacement colors as `#rrggbb`, by Material 3 role name.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColorOverrides {
    surface_container: Option<String>,
    surface_container_highest: Option<String>,
    on_surface: Option<String>,
    on_surface_variant: Option<String>,
    outline: Option<String>,
    primary: Option<String>,
    secondary_container: Option<String>,
}

impl ColorOverrides {
    /// Replaces the roles that are set. Stops at the first invalid color;
    /// roles before it stay replaced.
    fn apply_to(&self, colors: &mut ColorRoles) -> anyhow::Result<()> {
        let roles = [
            (&self.surface_container, &mut colors.surface_container),
            (
                &self.surface_container_highest,
                &mut colors.surface_container_highest,
            ),
            (&self.on_surface, &mut colors.on_surface),
            (&self.on_surface_variant, &mut colors.on_surface_variant),
            (&self.outline, &mut colors.outline),
            (&self.primary, &mut colors.primary),
            (&self.secondary_container, &mut colors.secondary_container),
        ];
        for (replacement, role) in roles {
            if let Some(text) = replacement {
                *role = parse_color(text)?;
            }
        }
        Ok(())
    }
}

fn parse_color(text: &str) -> anyhow::Result<Hsla> {
    let [red, green, blue] = parse_rgb(text)?.map(u32::from);
    Ok(rgb(red << 16 | green << 8 | blue).into())
}

/// `#rrggbb` as `[red, green, blue]`.
pub(crate) fn parse_rgb(text: &str) -> anyhow::Result<[u8; 3]> {
    let digits = text.strip_prefix('#').unwrap_or_default();
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("`{text}` is not a color of the form #rrggbb");
    }
    let [_, red, green, blue] = u32::from_str_radix(digits, 16)?.to_be_bytes();
    Ok([red, green, blue])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_parse_as_six_hex_digits() {
        assert_eq!(parse_color("#add28e").unwrap(), rgb(0xadd28e).into());
        for invalid in ["add28e", "#add", "#add28eff", "#+dd28e", ""] {
            assert!(parse_color(invalid).is_err(), "{invalid:?} was accepted");
        }
    }

    #[test]
    fn a_theme_file_may_name_a_seed() {
        let file: ThemeFile = toml::from_str("seed = \"#0078d4\"\n").unwrap();
        assert_eq!(
            parse_rgb(file.seed.as_deref().unwrap()).unwrap(),
            [0x00, 0x78, 0xd4]
        );
    }

    #[test]
    fn a_theme_file_may_name_a_scheme() {
        let file: ThemeFile = toml::from_str("scheme = \"light\"\n").unwrap();
        assert_eq!(file.scheme, Some(Scheme::Light));
        let mut theme = Theme::default();
        let dark = theme.colors;
        theme.derive(None, Scheme::Dark, "test");
        assert_eq!(theme.colors.on_surface, dark.on_surface);
        theme.derive(None, Scheme::Light, "test");
        assert!(theme.colors.surface_container.l > theme.colors.on_surface.l);
        assert_eq!(theme.followed, Followed::default());
    }

    #[test]
    fn a_theme_file_replaces_shapes_sizes_font_and_motion() {
        let file: ThemeFile = toml::from_str(
            r#"
            [shape]
            medium = 4
            [spacing]
            small = 6.5
            [typography]
            family = " Bahnschrift "
            label_size = 15
            [motion.spatial]
            stiffness = 200
            damping_ratio = 0.45
            [bar]
            pill_height = 40
            [dock]
            icon_size = 28
            [meter]
            amplitude = 4
            "#,
        )
        .unwrap();
        let mut theme = Theme::default();
        let built_in = Theme::default();
        assert!(file.apply_measures_to(&mut theme).is_empty());

        assert_eq!(theme.shape.medium, px(4.));
        assert_eq!(theme.shape.small, built_in.shape.small);
        assert_eq!(theme.spacing.small, px(6.5));
        assert_eq!(theme.typography.family, "Bahnschrift");
        assert_eq!(theme.typography.label_size, px(15.));
        assert_eq!(theme.typography.icon_size, built_in.typography.icon_size);
        assert_eq!(theme.motion.spatial.stiffness, 200.);
        assert_eq!(theme.motion.spatial.damping_ratio, 0.45);
        assert_eq!(
            theme.motion.effects.stiffness,
            built_in.motion.effects.stiffness
        );
        assert_eq!(theme.bar.pill_height, px(40.));
        assert_eq!(theme.dock.icon_size, px(28.));
        assert_eq!(theme.dock.item_size, built_in.dock.item_size);
        assert_eq!(theme.meter.amplitude, px(4.));
    }

    #[test]
    fn what_a_theme_file_gets_wrong_is_named_and_left_alone() {
        let file: ThemeFile = toml::from_str(
            r#"
            [shape]
            small = -1
            medium = 12
            [typography]
            family = ""
            label_size = 0
            [motion.spatial_fast]
            stiffness = 1e9
            damping_ratio = 0
            [dock]
            item_size = inf
            "#,
        )
        .unwrap();
        let mut theme = Theme::default();
        let built_in = Theme::default();
        let problems = file.apply_measures_to(&mut theme);

        let named: Vec<&str> = problems
            .iter()
            .map(|problem| problem.split('`').nth(1).unwrap())
            .collect();
        assert_eq!(
            named,
            [
                "shape.small",
                "typography.family",
                "typography.label_size",
                "motion.spatial_fast.stiffness",
                "motion.spatial_fast.damping_ratio",
                "dock.item_size"
            ]
        );
        assert_eq!(theme.shape.small, built_in.shape.small);
        assert_eq!(theme.shape.medium, px(12.));
        assert_eq!(theme.typography.family, built_in.typography.family);
        assert_eq!(theme.typography.label_size, built_in.typography.label_size);
        assert_eq!(
            theme.motion.spatial_fast.stiffness,
            built_in.motion.spatial_fast.stiffness
        );
        assert_eq!(theme.dock.item_size, built_in.dock.item_size);
        assert!(toml::from_str::<ThemeFile>("[shape]\nhuge = 1\n").is_err());
    }

    #[test]
    fn a_font_name_is_kept_once() {
        assert!(std::ptr::eq(kept("Some Font"), kept("Some Font")));
    }

    /// `text` as a theme file, which must be one.
    fn file(text: &str) -> ThemeFile {
        Theme::check(text).unwrap();
        toml::from_str(text).unwrap()
    }

    /// The theme `layers` make, with the values of their settings.
    fn stacked(layers: &[(&ThemeFile, Option<&str>)], legible: bool) -> Theme {
        let mut merge = Merge::default();
        for (index, (file, values)) in layers.iter().enumerate() {
            let values = values.map(|values| toml::from_str::<toml::Table>(values).unwrap());
            merge.layer(&format!("layer{index}"), file, values.as_ref());
        }
        merge.finish(legible)
    }

    #[test]
    fn a_layer_that_sets_a_seed_derives_every_color_again() {
        let base =
            file("seed = \"#ff0000\"\n[colors]\nprimary = \"#123456\"\noutline = \"#654321\"\n");
        let over = file("seed = \"#0000ff\"\n[colors]\nprimary = \"#abcdef\"\n");
        let theme = stacked(&[(&base, None), (&over, None)], false);
        assert_eq!(theme.colors.primary, rgb(0xabcdef).into());
        // The outline of the base came before the seed of the layer.
        let blue = tonal::roles([0, 0, 0xff], false);
        assert_eq!(theme.colors.outline, blue.outline);
        assert_eq!(theme.colors.surface_container, blue.surface_container);

        // A layer with only a scheme keeps the seed below it.
        let light = file("scheme = \"light\"\n");
        let theme = stacked(&[(&base, None), (&light, None)], false);
        let red = tonal::roles([0xff, 0, 0], true);
        assert_eq!(theme.colors.surface_container, red.surface_container);
    }

    #[test]
    fn a_dark_scheme_over_a_light_one_without_a_seed_is_dark_again() {
        let light = file("scheme = \"light\"\n");
        let dark = file("scheme = \"dark\"\n");
        let theme = stacked(&[(&light, None), (&dark, None)], false);
        let built_in = Theme::default().colors;
        assert_eq!(theme.colors.surface_container, built_in.surface_container);
        assert_eq!(theme.colors.on_surface, built_in.on_surface);
        // A seed below is kept and derived dark.
        let seeded = file("seed = \"#ff0000\"\nscheme = \"light\"\n");
        let theme = stacked(&[(&seeded, None), (&dark, None)], false);
        let red = tonal::roles([0xff, 0, 0], false);
        assert_eq!(theme.colors.surface_container, red.surface_container);
    }

    #[test]
    fn a_scale_multiplies_what_the_layers_below_made() {
        let base = file("[shape]\nsmall = 10\nfull = 500\n");
        let corners = file(concat!(
            "[shape]\nmedium = 20\n",
            "[[setting]]\nkey = \"corners\"\nkind = \"slider\"\nlabel = \"Corners\"\n",
            "min = 0\nmax = 3\nstep = 0.5\ndefault = 1.0\n",
            "scale = [\"shape.small\", \"shape.medium\"]\n",
        ));
        let theme = stacked(&[(&base, None), (&corners, Some("corners = 2.0"))], false);
        assert_eq!(theme.shape.small, px(20.));
        assert_eq!(theme.shape.medium, px(40.));
        assert_eq!(theme.shape.full, px(500.));
        // Its default leaves the lengths as they are.
        let theme = stacked(&[(&base, None), (&corners, None)], false);
        assert_eq!(theme.shape.small, px(10.));
        assert!(Theme::check("[[setting]]\nkey = \"c\"\nkind = \"slider\"\nlabel = \"C\"\nmin = 0\nmax = 2\nscale = [\"shape.full\"]\n").is_err());
    }

    #[test]
    fn a_choice_brings_its_tokens_and_a_value_goes_where_it_affects() {
        let theme_file = file(concat!(
            "seed = \"#ff0000\"\n",
            "[[setting]]\nkey = \"variant\"\nkind = \"choice\"\nlabel = \"Variant\"\n",
            "choices = [{ value = \"unset\", label = \"Red\" }, ",
            "{ value = \"blue\", overlay = { seed = \"#0000ff\", bar = { pill_height = 40 } } }]\n",
            "[[setting]]\nkey = \"font\"\nkind = \"font\"\nlabel = \"Font\"\noptional = true\n",
            "affects = [\"typography.family\"]\n",
            "[[setting]]\nkey = \"accent\"\nkind = \"color\"\nlabel = \"Accent\"\noptional = true\n",
            "affects = [\"colors.primary\"]\n",
        ));
        let theme = stacked(
            &[(
                &theme_file,
                Some("variant = \"blue\"\nfont = \"Bahnschrift\"\naccent = \"#11223380\""),
            )],
            false,
        );
        let blue = tonal::roles([0, 0, 0xff], false);
        assert_eq!(theme.colors.outline, blue.outline);
        assert_eq!(theme.colors.primary, rgb(0x112233).into());
        assert_eq!(theme.bar.pill_height, px(40.));
        assert_eq!(theme.typography.family, "Bahnschrift");
        let plain = stacked(&[(&theme_file, None)], false);
        assert_eq!(
            plain.colors.outline,
            tonal::roles([0xff, 0, 0], false).outline
        );
        let groups = affected_groups(&theme_file.declared().0.decls[0]);
        assert!(groups.len() == 2 && groups.contains(&"Colors") && groups.contains(&"Bar"));
    }

    #[test]
    fn contrast_is_corrected_once_at_the_end() {
        let base = file("[colors]\nsurface_container = \"#202020\"\non_surface = \"#303030\"\n");
        let over = file("[colors]\non_surface_variant = \"#282828\"\n");
        let legible = stacked(&[(&base, None), (&over, None)], true);
        let mut by_hand = stacked(&[(&base, None), (&over, None)], false).colors;
        by_hand.make_legible();
        assert_eq!(legible.colors.on_surface, by_hand.on_surface);
        assert_eq!(
            legible.colors.on_surface_variant,
            by_hand.on_surface_variant
        );
        assert!(legible.legible);
    }

    #[test]
    fn settings_that_name_no_token_are_refused() {
        let wrong = [
            "[[setting]]\nkey = \"a\"\nkind = \"color\"\nlabel = \"A\"\naffects = [\"colors.nothing\"]\n",
            "[[setting]]\nkey = \"a\"\nkind = \"toggle\"\nlabel = \"A\"\n",
            "[[setting]]\nkey = \"a\"\nkind = \"list\"\nlabel = \"A\"\naffects = [\"seed\"]\n",
            "[[setting]]\nkey = \"a\"\nkind = \"choice\"\nlabel = \"A\"\nchoices = [{ value = \"x\", overlay = { colours = 1 } }]\n",
        ];
        for text in wrong {
            assert!(Theme::check(text).is_err(), "{text} was taken");
        }
        let flat = "[[setting]]\nkey = \"flat\"\nkind = \"toggle\"\nlabel = \"Flat\"\naffects = [\"bar.flat\"]\n";
        Theme::check(flat).unwrap();
    }

    #[test]
    fn the_built_in_themes_declare_their_settings() {
        let builtin = ThemeFile::builtin(None);
        let (declared, problems) = builtin.declared();
        assert!(problems.is_empty(), "{problems:?}");
        assert!(declared.get("accent").is_some());
        let accent = ThemeFile::builtin(Some(ACCENT_SEED));
        assert!(accent.declared().0.get("accent").is_none());
        assert_eq!(accent.declared().0.decls.len(), declared.decls.len() - 1);
        let theme = stacked(
            &[(
                &builtin,
                Some("density = \"compact\"\ncorners = 0.5\ndock_size = 1.5"),
            )],
            false,
        );
        assert_eq!(theme.spacing.small, px(6.));
        assert_eq!(theme.shape.small, px(4.));
        assert_eq!(theme.dock.item_size, px(72.));
    }

    #[test]
    fn the_bundled_themes_are_valid() {
        for (name, text) in BUNDLED {
            Theme::check(text).unwrap_or_else(|error| panic!("{name}: {error:#}"));
            let file: ThemeFile = toml::from_str(text).unwrap();
            assert!(file.declared().1.is_empty(), "{name}");
        }
    }

    const BUNDLED: [(&str, &str); 23] = [
        ("arcade", include_str!("../../../themes/arcade.toml")),
        ("aurora", include_str!("../../../themes/aurora.toml")),
        ("citrus", include_str!("../../../themes/citrus.toml")),
        ("compact", include_str!("../../../themes/compact.toml")),
        ("contrast", include_str!("../../../themes/contrast.toml")),
        ("dense", include_str!("../../../themes/dense.toml")),
        ("ember", include_str!("../../../themes/ember.toml")),
        (
            "expressive",
            include_str!("../../../themes/expressive.toml"),
        ),
        ("forest", include_str!("../../../themes/forest.toml")),
        ("glacier", include_str!("../../../themes/glacier.toml")),
        ("lilac", include_str!("../../../themes/lilac.toml")),
        ("mocha", include_str!("../../../themes/mocha.toml")),
        ("nebula", include_str!("../../../themes/nebula.toml")),
        ("ocean", include_str!("../../../themes/ocean.toml")),
        ("oled", include_str!("../../../themes/oled.toml")),
        ("paper", include_str!("../../../themes/paper.toml")),
        ("round", include_str!("../../../themes/round.toml")),
        ("sakura", include_str!("../../../themes/sakura.toml")),
        ("sandstone", include_str!("../../../themes/sandstone.toml")),
        ("sunset", include_str!("../../../themes/sunset.toml")),
        ("synthwave", include_str!("../../../themes/synthwave.toml")),
        ("terminal", include_str!("../../../themes/terminal.toml")),
        ("void", include_str!("../../../themes/void.toml")),
    ];

    #[test]
    fn a_theme_file_replaces_only_the_roles_it_sets() {
        let file: ThemeFile = toml::from_str("[colors]\nprimary = \"#ff0000\"\n").unwrap();
        let mut colors = Theme::default().colors;
        let outline = colors.outline;
        file.colors.apply_to(&mut colors).unwrap();
        assert_eq!(colors.primary, rgb(0xff0000).into());
        assert_eq!(colors.outline, outline);
    }
}
