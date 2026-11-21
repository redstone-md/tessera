// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use std::collections::HashSet;
use std::sync::Mutex;

use anyhow::bail;
use gpui::{Global, Hsla, Pixels, px, rgb};
use serde::Deserialize;

use crate::config::{read_user_file, theme_file};
use crate::motion::SpringSpec;
use crate::platform::{accent_color, light_mode};

mod contrast;
mod tonal;

/// The theme name that stands for colors derived from the accent color of
/// Windows instead of for a file.
pub const ACCENT_THEME: &str = "accent";

/// What a theme file writes as its seed to follow the accent color.
const ACCENT_SEED: &str = "accent";

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

    /// Whether `text` is a theme file. What of it is out of range is not
    /// looked at: that is reported when the theme is applied.
    pub fn check(text: &str) -> anyhow::Result<()> {
        toml::from_str::<ThemeFile>(text)?;
        Ok(())
    }

    /// The built-in theme with the user's theme file `name` applied over it.
    /// A missing or unusable file is reported and leaves the built-in theme.
    /// With `legible`, colors that are read are then moved away from what
    /// they lie on until they have the contrast WCAG 2.1 asks for.
    pub fn load(name: Option<&str>, legible: bool) -> Self {
        let mut theme = Self::named(name);
        if legible {
            theme.colors.make_legible();
        }
        theme
    }

    fn named(name: Option<&str>) -> Self {
        let mut theme = Self::default();
        let Some(name) = name else {
            return theme;
        };
        if name == ACCENT_THEME {
            theme.derive(Some(ACCENT_SEED), Scheme::System, name);
            return theme;
        }
        if name.contains(['/', '\\', ':']) || name.starts_with('.') {
            report!("`{name}` is not a theme name, the built-in theme is used");
            return theme;
        }
        match read_user_file::<ThemeFile>(&theme_file(name)) {
            Some(file) => {
                theme.derive(file.seed.as_deref(), file.scheme, name);
                if let Err(error) = file.colors.apply_to(&mut theme.colors) {
                    report!("theme `{name}` is only partly applied: {error:#}");
                }
                for problem in file.apply_measures_to(&mut theme) {
                    report!("theme `{name}`: {problem}, which is left as it was");
                }
            }
            None => report!("theme `{name}` is not loaded, the built-in theme is used"),
        }
        theme
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
        let Followed { accent, light } = self.followed;
        accent.is_some_and(|accent| accent_color().unwrap_or(BUILT_IN_SEED) != accent)
            || light.is_some_and(|light| light_mode().unwrap_or(false) != light)
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
    /// what the Windows settings say for the taskbar.
    #[serde(default)]
    scheme: Scheme,
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
}

impl ThemeFile {
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
fn parse_rgb(text: &str) -> anyhow::Result<[u8; 3]> {
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
        assert_eq!(file.scheme, Scheme::Light);
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
