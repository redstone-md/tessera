// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use anyhow::bail;
use gpui::{Global, Hsla, Pixels, px, rgb};
use serde::Deserialize;

use crate::config::{read_user_file, theme_file};
use crate::motion::SpringSpec;
use crate::platform::{accent_color, light_mode};

mod tonal;

/// The theme name that stands for colors derived from the accent color of
/// Windows instead of for a file.
pub const ACCENT_THEME: &str = "accent";

/// What a theme file writes as its seed to follow the accent color.
const ACCENT_SEED: &str = "accent";

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
    /// The built-in theme with the user's theme file `name` applied over it.
    /// A missing or unusable file is reported and leaves the built-in theme.
    pub fn load(name: Option<&str>) -> Self {
        let mut theme = Self::default();
        let Some(name) = name else {
            return theme;
        };
        if name == ACCENT_THEME {
            theme.derive(Some(ACCENT_SEED), Scheme::System, name);
            return theme;
        }
        if name.contains(['/', '\\', ':']) || name.starts_with('.') {
            eprintln!("tessera: `{name}` is not a theme name, the built-in theme is used");
            return theme;
        }
        match read_user_file::<ThemeFile>(&theme_file(name)) {
            Some(file) => {
                theme.derive(file.seed.as_deref(), file.scheme, name);
                if let Err(error) = file.colors.apply_to(&mut theme.colors) {
                    eprintln!("tessera: theme `{name}` is only partly applied: {error:#}");
                }
            }
            None => eprintln!("tessera: theme `{name}` is not loaded, the built-in theme is used"),
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
                    eprintln!("tessera: Windows does not tell whether it is light or dark");
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
                    eprintln!("tessera: Windows does not tell its accent color");
                }
                self.followed.accent = Some(accent.unwrap_or(BUILT_IN_SEED));
                accent
            }
            Some(seed) => parse_rgb(seed)
                .inspect_err(|error| eprintln!("tessera: theme `{name}`: {error:#}"))
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
    fn a_theme_file_replaces_only_the_roles_it_sets() {
        let file: ThemeFile = toml::from_str("[colors]\nprimary = \"#ff0000\"\n").unwrap();
        let mut colors = Theme::default().colors;
        let outline = colors.outline;
        file.colors.apply_to(&mut colors).unwrap();
        assert_eq!(colors.primary, rgb(0xff0000).into());
        assert_eq!(colors.outline, outline);
    }
}
