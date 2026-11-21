// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Shell-wide appearance tokens, following Material 3 role names.

use anyhow::bail;
use gpui::{Global, Hsla, Pixels, px, rgb};
use serde::Deserialize;

use crate::config::read_user_file;
use crate::motion::SpringSpec;

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

/// The appearance shared by every shell surface.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub colors: ColorRoles,
    pub shape: Shape,
    pub spacing: Spacing,
    pub typography: Typography,
    pub motion: Motion,
    pub bar: BarMetrics,
    pub dock: DockMetrics,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
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
        if name.contains(['/', '\\', ':']) || name.starts_with('.') {
            eprintln!("tessera: `{name}` is not a theme name, the built-in theme is used");
            return theme;
        }
        match read_user_file::<ThemeFile>(&format!("themes/{name}.toml")) {
            Some(file) => {
                if let Err(error) = file.colors.apply_to(&mut theme.colors) {
                    eprintln!("tessera: theme `{name}` is only partly applied: {error:#}");
                }
            }
            None => eprintln!("tessera: theme `{name}` is not loaded, the built-in theme is used"),
        }
        theme
    }
}

/// A user theme: values to replace in the built-in theme.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
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
    let digits = text.strip_prefix('#').unwrap_or_default();
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("`{text}` is not a color of the form #rrggbb");
    }
    Ok(rgb(u32::from_str_radix(digits, 16)?).into())
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
    fn a_theme_file_replaces_only_the_roles_it_sets() {
        let file: ThemeFile = toml::from_str("[colors]\nprimary = \"#ff0000\"\n").unwrap();
        let mut colors = Theme::default().colors;
        let outline = colors.outline;
        file.colors.apply_to(&mut colors).unwrap();
        assert_eq!(colors.primary, rgb(0xff0000).into());
        assert_eq!(colors.outline, outline);
    }
}
