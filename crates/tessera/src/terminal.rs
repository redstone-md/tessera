// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera's own terminal: a shell (PowerShell, cmd, WSL or another
//! program) in a pseudoconsole of Windows, its output parsed into a grid
//! by `alacritty_terminal`, drawn by GPUI. The file manager shows one
//! under its panes (F4), and the drop-down terminal shows several in tabs.
//! How it looks and which shell it runs are `[terminal]` of the
//! configuration.

mod element;
mod keys;
mod links;
mod osc;
mod palette;
mod screen;
mod session;
mod shell;
mod view;

use gpui::{App, AsyncApp, Global, SharedString};
use serde::Deserialize;

use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};

pub use shell::same_folder;
pub use view::{TerminalEvent, TerminalView};

use crate::platform::font_families;

/// The font a terminal draws with where the configuration names none.
const DEFAULT_FONT: &str = "Cascadia Mono";

/// The fonts taken, in order, where the font configured is not installed.
const FALLBACK_FONTS: [&str; 2] = ["Consolas", "Courier New"];

/// The shape of the cursor where programs set none, and whether it
/// blinks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorLook {
    #[default]
    Block,
    Underline,
    Beam,
    BlockSteady,
    UnderlineSteady,
    BeamSteady,
}

impl CursorLook {
    fn style(self) -> CursorStyle {
        let (shape, blinking) = match self {
            Self::Block => (CursorShape::Block, true),
            Self::Underline => (CursorShape::Underline, true),
            Self::Beam => (CursorShape::Beam, true),
            Self::BlockSteady => (CursorShape::Block, false),
            Self::UnderlineSteady => (CursorShape::Underline, false),
            Self::BeamSteady => (CursorShape::Beam, false),
        };
        CursorStyle { shape, blinking }
    }
}

/// What `[terminal]` of the configuration says.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TerminalConfig {
    /// What runs in the terminal: `pwsh`, `powershell`, `cmd`, `wsl`,
    /// `wsl:<distribution>` or a command line of its own; none for
    /// PowerShell 7 where it is installed and Windows PowerShell otherwise.
    pub shell: Option<String>,
    /// The family of the font, which falls back to Consolas where it is
    /// not installed.
    pub font: String,
    /// The size of the font, in logical pixels.
    pub font_size: f32,
    /// How high a line is, as a share of the size of the font.
    pub line_height: f32,
    /// How many lines that scrolled out of sight are kept.
    pub scrollback: usize,
    /// The 16 named colours, `#rrggbb` each in the order black, red,
    /// green, yellow, blue, magenta, cyan, white and their bright ones;
    /// those not given come from the theme.
    pub palette: Vec<String>,
    /// The cursor where programs set none.
    pub cursor: CursorLook,
    /// Whether what is selected goes to the clipboard at once.
    pub copy_on_select: bool,
    /// Whether the pane of the file manager goes where the terminal under
    /// it says its shell went.
    pub pane_follows: bool,
    /// How much of the width of the display the drop-down terminal takes.
    pub dropdown_width: f32,
    /// How much of the height of the display it takes.
    pub dropdown_height: f32,
    /// Whether it hides when another window takes the keys.
    pub dropdown_hides: bool,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: None,
            font: DEFAULT_FONT.to_owned(),
            font_size: 13.,
            line_height: 1.25,
            scrollback: 10_000,
            palette: Vec::new(),
            cursor: CursorLook::Block,
            copy_on_select: false,
            pane_follows: true,
            dropdown_width: 1.,
            dropdown_height: 0.5,
            dropdown_hides: true,
        }
    }
}

impl TerminalConfig {
    /// The configuration with what makes no sense in it reported and
    /// replaced: sizes within bounds, and only colours that are colours.
    fn checked(&self) -> Self {
        let defaults = Self::default();
        let within = |value: f32, least: f32, most: f32, default: f32, key: &str| {
            if value.is_finite() && (least..=most).contains(&value) {
                return value;
            }
            report!("`[terminal] {key}` must be from {least} to {most}, `{value}` is ignored");
            default
        };
        let palette = { self.palette.iter().take(16) }
            .map(|color| match palette::parse_hex(color) {
                Some(_) => color.clone(),
                None => {
                    report!(
                        "`[terminal] palette` takes colours as `#rrggbb`, `{color}` is ignored"
                    );
                    String::new()
                }
            })
            .collect();
        Self {
            font: match self.font.trim() {
                "" => defaults.font.clone(),
                font => font.to_owned(),
            },
            font_size: within(self.font_size, 6., 48., defaults.font_size, "font_size"),
            line_height: within(
                self.line_height,
                0.8,
                3.,
                defaults.line_height,
                "line_height",
            ),
            scrollback: self.scrollback.min(100_000),
            palette,
            dropdown_width: within(
                self.dropdown_width,
                0.2,
                1.,
                defaults.dropdown_width,
                "dropdown_width",
            ),
            dropdown_height: within(
                self.dropdown_height,
                0.1,
                1.,
                defaults.dropdown_height,
                "dropdown_height",
            ),
            ..self.clone()
        }
    }
}

/// The font family of `configured` that is installed, as `families` lists
/// them: it, else Consolas, else Courier New; it as it is where none is.
fn installed_family(configured: &str, families: &[String]) -> String {
    let installed = |name: &str| {
        families
            .iter()
            .any(|family| family.eq_ignore_ascii_case(name))
    };
    { std::iter::once(configured).chain(FALLBACK_FONTS) }
        .find(|name| installed(name))
        .unwrap_or(configured)
        .to_owned()
}

/// What every terminal of Tessera is set to.
#[derive(Default)]
pub struct Terminals {
    config: TerminalConfig,
    /// The family of the font, once it was looked for among those
    /// installed.
    family: Option<SharedString>,
}

impl Global for Terminals {}

impl Terminals {
    /// Takes `config` for every terminal; the font is looked for among
    /// those installed away from the user interface, and the terminals
    /// take it once it is found.
    pub fn configure(config: &TerminalConfig, cx: &mut App) {
        let config = config.checked();
        let font = config.font.clone();
        let terminals = cx.default_global::<Self>();
        if terminals.config == config && terminals.family.is_some() {
            return;
        }
        terminals.config = config;
        let found = cx
            .background_executor()
            .spawn(async move { installed_family(&font, font_families()) });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let family = found.await;
            cx.update(|cx| cx.global_mut::<Self>().family = Some(family.into()))
                .ok();
        })
        .detach();
    }

    /// What the terminals are set to.
    pub fn config(cx: &App) -> TerminalConfig {
        { cx.try_global::<Self>() }
            .map(|terminals| terminals.config.clone())
            .unwrap_or_default()
    }

    /// The family of the font the terminals draw with.
    pub fn family(cx: &App) -> SharedString {
        let terminals = cx.try_global::<Self>();
        match terminals.and_then(|terminals| terminals.family.clone()) {
            Some(family) => family,
            None => terminals
                .map_or(DEFAULT_FONT, |terminals| terminals.config.font.as_str())
                .to_owned()
                .into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configuration_reads_and_mends_what_makes_no_sense() {
        let config: TerminalConfig = toml::from_str(
            "shell = \"cmd\"\nfont_size = 200\npalette = [\"#000000\", \"red\"]\ncursor = \"beam-steady\"\n",
        )
        .expect("a terminal is configured");
        assert_eq!(config.shell.as_deref(), Some("cmd"));
        assert_eq!(config.cursor, CursorLook::BeamSteady);
        let checked = config.checked();
        assert_eq!(checked.font_size, 13.);
        assert_eq!(checked.palette, ["#000000", ""]);
        assert!(toml::from_str::<TerminalConfig>("colour = 1").is_err());
        assert_eq!(
            CursorLook::BeamSteady.style(),
            CursorStyle {
                shape: CursorShape::Beam,
                blinking: false
            }
        );
    }

    #[test]
    fn a_font_not_installed_falls_back_to_consolas() {
        let families = ["Arial".to_owned(), "consolas".to_owned()];
        assert_eq!(installed_family("Cascadia Mono", &families), "Consolas");
        assert_eq!(installed_family("Arial", &families), "Arial");
        assert_eq!(installed_family("Nothing", &[]), "Nothing");
    }
}
