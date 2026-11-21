// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The layout, the pinned applications and the theme, changed without
//! editing the configuration file by hand. Widget: `settings`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, AppContext, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, Window, div, img, px,
};

use crate::config::edit::{ConfigFile, Place, Slot};
use crate::config::{Config, Displays, SurfaceConfig, WidgetSpec, config_dir};
use crate::icons::AppIcons;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{AppWindow, ScreenEdge};
use crate::shell::Shell;
use crate::theme::{ACCENT_THEME, Theme};
use crate::ui::{icon, state_layer};

/// The directory of the configuration directory that holds theme files.
const THEMES_DIRECTORY: &str = "themes";

/// The height of a chip and the side of the round buttons in it.
const CHIP_HEIGHT: Pixels = px(28.);
const SMALL_BUTTON: Pixels = px(20.);

/// The width kept for the name of a slot.
const SLOT_LABEL_WIDTH: Pixels = px(52.);

/// The most a list of widgets to add takes while it opens.
const ADD_LIST_HEIGHT: Pixels = px(240.);

const ICON_BACK: &str = "\u{e76b}";
const ICON_FORWARD: &str = "\u{e76c}";
const ICON_REMOVE: &str = "\u{e711}";

pub struct SettingsModule;

impl Module for SettingsModule {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("settings", |_, cx| cx.new(Settings::new).into())
    }
}

/// Something to click, which also names its pointer feedback.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Control {
    /// Moves a widget toward the end of its surface, or toward the start.
    Shift(Place, bool),
    Remove(Place),
    /// Opens or closes the list of widgets to add to a slot.
    Adding(usize, Slot),
    /// Adds the widget at this place of the list to the slot being added to.
    Add(usize),
    /// Counts the pinned applications.
    Unpin(usize),
    /// Counts the running applications that are offered for pinning.
    Pin(usize),
    /// Counts the theme files; `None` is the built-in theme.
    Theme(Option<usize>),
}

/// How a control looks.
enum Face {
    /// A small round button with a character of the icon font.
    Glyph(&'static str),
    /// A chip with a text, tinted while `selected`.
    Chip { text: SharedString, selected: bool },
}

/// What a widget of the layout is called: its name, and its text where it
/// has one, which tells several of a kind apart.
fn widget_title(spec: &WidgetSpec) -> String {
    match spec.options.get("text").and_then(toml::Value::as_str) {
        Some(text) => format!("{}: {text}", spec.widget),
        None => spec.widget.clone(),
    }
}

/// What a surface is called.
fn surface_title(surface: &SurfaceConfig) -> String {
    let edge = match surface.edge {
        ScreenEdge::Top => "Top",
        ScreenEdge::Bottom => "Bottom",
    };
    let displays = match surface.displays {
        Displays::All => "every display",
        Displays::Primary => "the primary display",
    };
    format!("{edge} edge, on {displays}")
}

/// A path the way it is compared and written to the file.
fn plain(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// What a pinned application is called: the file name without its
/// extension for a path, the name itself otherwise.
fn pin_title(pin: &str) -> String {
    let path = Path::new(pin);
    match path.file_stem() {
        Some(stem) if path.is_absolute() => stem.to_string_lossy().into_owned(),
        _ => pin.to_owned(),
    }
}

/// The names of the theme files, in order.
fn themes() -> Vec<String> {
    let entries = config_dir()
        .and_then(|directory| fs::read_dir(directory.join(THEMES_DIRECTORY)).ok())
        .into_iter()
        .flatten();
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        })
        .filter_map(|path| Some(path.file_stem()?.to_str()?.to_owned()))
        .collect();
    names.retain(|name| name != ACCENT_THEME);
    names.sort();
    // The colors of Windows are a choice like a file.
    names.insert(0, ACCENT_THEME.to_owned());
    names
}

/// What the chip of the theme `name` says.
fn theme_title(name: &str) -> SharedString {
    if name == ACCENT_THEME {
        "Windows accent".into()
    } else {
        SharedString::new(name)
    }
}

/// Shows the configuration and changes its file.
struct Settings {
    /// What the file held when it was last read; `None` if it is unusable.
    config: Option<Config>,
    /// Why the last change was not made, or the file not read.
    failure: Option<SharedString>,
    /// The widgets a layout can name.
    available: Vec<String>,
    /// The programs with an open window that are not pinned.
    running: Vec<PathBuf>,
    themes: Vec<String>,
    /// The slot whose list of widgets to add is open, kept while it closes.
    adding: Option<(usize, Slot)>,
    /// 1 while that list is open, 0 while it is closed.
    add_list: Spring,
    interactions: HashMap<Control, Interaction>,
}

impl Settings {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            config: None,
            failure: None,
            available: Vec::new(),
            running: Vec::new(),
            themes: Vec::new(),
            adding: None,
            add_list: Spring::new(motion.spatial_fast, 0.),
            interactions: HashMap::new(),
        };
        match ConfigFile::open().and_then(|file| file.config()) {
            Ok(config) => this.config = Some(config),
            Err(error) => this.failure = Some(error.into()),
        }
        this.look_around(cx);
        this
    }

    /// Reads what the choices depend on besides the file.
    fn look_around(&mut self, cx: &mut Context<Self>) {
        self.available = cx.global::<WidgetRegistry>().names();
        self.themes = themes();
        let pins = self
            .config
            .as_ref()
            .and_then(Config::pinned)
            .unwrap_or_default();
        let pins: Vec<String> = pins.iter().map(|pin| pin.to_lowercase()).collect();
        let mut running: Vec<PathBuf> = Vec::new();
        for window in AppWindow::enumerate() {
            let Some(program) = window.executable else {
                continue;
            };
            let name = plain(&program).to_lowercase();
            let known = |other: &PathBuf| plain(other).to_lowercase() == name;
            if !pins.contains(&name) && !running.iter().any(known) {
                running.push(program);
            }
        }
        self.running = running;
    }

    /// Reads the file, applies `change` to it, writes it and has the shell
    /// take it over. A change that no longer fits the file is not made.
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut ConfigFile) -> bool) {
        let changed = ConfigFile::open().and_then(|mut file| {
            if !change(&mut file) {
                return Err("the configuration has changed in the meantime".to_owned());
            }
            file.save()?;
            file.config()
        });
        match changed {
            Ok(config) => {
                self.config = Some(config);
                self.failure = None;
                // The controls now stand for other things.
                self.interactions.clear();
                cx.defer(Shell::config_changed);
            }
            Err(error) => {
                eprintln!("tessera: {error}");
                self.failure = Some(error.into());
            }
        }
        self.look_around(cx);
        cx.notify();
    }

    fn act(&mut self, control: &Control, cx: &mut Context<Self>) {
        match *control {
            Control::Shift(place, forward) => self.change(cx, |file| file.shift(place, forward)),
            Control::Remove(place) => self.change(cx, |file| file.remove(place)),
            Control::Adding(surface, slot) => {
                let open = self.add_list.target() > 0.;
                if open && self.adding == Some((surface, slot)) {
                    self.add_list.animate_to(0.);
                } else {
                    if self.adding != Some((surface, slot)) {
                        self.add_list.jump_to(0.);
                    }
                    self.adding = Some((surface, slot));
                    self.add_list.animate_to(1.);
                }
                cx.notify();
            }
            Control::Add(index) => {
                let (Some((surface, slot)), Some(widget)) =
                    (self.adding, self.available.get(index).cloned())
                else {
                    return;
                };
                self.change(cx, |file| file.add(surface, slot, &widget));
            }
            Control::Unpin(index) => {
                let Some(mut pins) = self.config.as_ref().and_then(Config::pinned) else {
                    return;
                };
                if index < pins.len() {
                    pins.remove(index);
                    self.change(cx, |file| file.set_pinned(&pins));
                }
            }
            Control::Pin(index) => {
                let (Some(mut pins), Some(program)) = (
                    self.config.as_ref().and_then(Config::pinned),
                    self.running.get(index),
                ) else {
                    return;
                };
                pins.push(plain(program));
                self.change(cx, |file| file.set_pinned(&pins));
            }
            Control::Theme(choice) => {
                let theme = choice.and_then(|index| self.themes.get(index).cloned());
                if choice.is_some() && theme.is_none() {
                    return;
                }
                self.change(cx, |file| {
                    file.set_theme(theme.as_deref());
                    true
                });
            }
        }
    }

    /// A control that looks like `face` and follows the pointer.
    fn control(
        &mut self,
        control: Control,
        face: Face,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(control.clone())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let button = div()
            .id(SharedString::from(format!("{control:?}")))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full);
        let button = match face {
            Face::Glyph(glyph) => button
                .size(SMALL_BUTTON)
                .bg(state_layer(
                    &theme,
                    theme.colors.surface_container_highest,
                    state,
                ))
                .text_color(theme.colors.on_surface_variant)
                .child(icon(&theme, glyph).text_size(theme.typography.label_size)),
            Face::Chip { text, selected } => {
                let (base, color) = if selected {
                    (theme.colors.secondary_container, theme.colors.primary)
                } else {
                    (
                        theme.colors.surface_container_highest,
                        theme.colors.on_surface,
                    )
                };
                button
                    .h(CHIP_HEIGHT)
                    .px(theme.spacing.small + theme.spacing.tiny)
                    .bg(state_layer(&theme, base, state))
                    .text_color(color)
                    .child(text)
            }
        };
        let action = control.clone();
        let button = button.on_click(cx.listener(move |this, _, _, cx| this.act(&action, cx)));
        Interaction::track(button, cx, move |this: &mut Self| {
            this.interactions.get_mut(&control)
        })
        .into_any_element()
    }

    /// A widget of the layout: its name, and buttons to move and remove it.
    fn widget_chip(
        &mut self,
        place: Place,
        spec: &WidgetSpec,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let known = self.available.contains(&spec.widget);
        let buttons = [
            (Control::Shift(place, false), ICON_BACK),
            (Control::Shift(place, true), ICON_FORWARD),
            (Control::Remove(place), ICON_REMOVE),
        ]
        .map(|(control, glyph)| self.control(control, Face::Glyph(glyph), window, cx));
        div()
            .flex_none()
            .h(CHIP_HEIGHT)
            .pl(theme.spacing.small + theme.spacing.tiny)
            .pr(theme.spacing.tiny)
            .flex()
            .items_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            // A widget no module provides is shown, but dimmed.
            .text_color(if known {
                theme.colors.on_surface
            } else {
                theme.colors.outline
            })
            .child(div().pr(theme.spacing.tiny).child(widget_title(spec)))
            .children(buttons)
            .into_any_element()
    }

    fn surface(
        &mut self,
        index: usize,
        surface: &SurfaceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let add_list = self.add_list.value(window);
        let mut section = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(heading(&theme, surface_title(surface)));
        for slot in Slot::ALL {
            let specs = match slot {
                Slot::Start => &surface.start,
                Slot::Center => &surface.center,
                Slot::End => &surface.end,
            };
            let adding = self.adding == Some((index, slot));
            let mut chips = Vec::new();
            for (at, spec) in specs.iter().enumerate() {
                let place = Place {
                    surface: index,
                    slot,
                    index: at,
                };
                chips.push(self.widget_chip(place, spec, window, cx));
            }
            chips.push(self.control(
                Control::Adding(index, slot),
                Face::Chip {
                    text: "Add".into(),
                    selected: adding && self.add_list.target() > 0.,
                },
                window,
                cx,
            ));
            section = section.child(
                div()
                    .flex()
                    .items_start()
                    .gap(theme.spacing.small)
                    .child(
                        div()
                            .flex_none()
                            .w(SLOT_LABEL_WIDTH)
                            .h(CHIP_HEIGHT)
                            .flex()
                            .items_center()
                            .text_color(theme.colors.on_surface_variant)
                            .child(slot.label()),
                    )
                    .child(wrap(&theme).flex_1().children(chips)),
            );
            if adding && add_list > 0. {
                let mut offers = Vec::new();
                for at in 0..self.available.len() {
                    let face = Face::Chip {
                        text: self.available[at].clone().into(),
                        selected: true,
                    };
                    offers.push(self.control(Control::Add(at), face, window, cx));
                }
                section = section.child(
                    div()
                        .pl(SLOT_LABEL_WIDTH + theme.spacing.small)
                        .max_h(ADD_LIST_HEIGHT * add_list.max(0.))
                        .opacity(add_list.clamp(0., 1.))
                        .overflow_hidden()
                        .child(wrap(&theme).children(offers)),
                );
            }
        }
        section.into_any_element()
    }

    fn pins(&mut self, pins: &[String], window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let mut pinned = Vec::new();
        for (index, pin) in pins.iter().enumerate() {
            let remove = self.control(Control::Unpin(index), Face::Glyph(ICON_REMOVE), window, cx);
            let image = cx.global::<AppIcons>().of_executable(Path::new(pin));
            pinned.push(
                div()
                    .flex_none()
                    .h(CHIP_HEIGHT)
                    .pl(theme.spacing.small)
                    .pr(theme.spacing.tiny)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny + theme.spacing.tiny)
                    .rounded(theme.shape.full)
                    .bg(theme.colors.surface_container_highest)
                    .children(image.map(|image| img(image).size(SMALL_BUTTON).flex_none()))
                    .child(pin_title(pin))
                    .child(remove),
            );
        }
        let mut offers = Vec::new();
        for index in 0..self.running.len() {
            let name = self.running[index].file_stem().unwrap_or_default();
            let face = Face::Chip {
                text: name.to_string_lossy().into_owned().into(),
                selected: true,
            };
            offers.push(self.control(Control::Pin(index), face, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(heading(&theme, "Pinned applications"))
            .child(if pinned.is_empty() {
                note(&theme, "Nothing is pinned.").into_any_element()
            } else {
                wrap(&theme).children(pinned).into_any_element()
            })
            .children((!offers.is_empty()).then(|| note(&theme, "Running now, click to pin:")))
            .children((!offers.is_empty()).then(|| wrap(&theme).children(offers)))
            .into_any_element()
    }

    fn themes(
        &mut self,
        current: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let mut choices = vec![self.control(
            Control::Theme(None),
            Face::Chip {
                text: "Built-in".into(),
                selected: current.is_none(),
            },
            window,
            cx,
        )];
        for index in 0..self.themes.len() {
            let face = Face::Chip {
                text: theme_title(&self.themes[index]),
                selected: current == Some(self.themes[index].as_str()),
            };
            choices.push(self.control(Control::Theme(Some(index)), face, window, cx));
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(heading(&theme, "Theme"))
            .child(wrap(&theme).children(choices))
            .children((self.themes.len() == 1).then(|| {
                note(
                    &theme,
                    "Theme files are looked for in the themes directory of the configuration directory.",
                )
            }))
            .into_any_element()
    }
}

/// The title of a part of the settings.
fn heading(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.primary)
        .child(text.into())
}

/// A remark in the quieter color.
fn note(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_color(theme.colors.on_surface_variant)
        .child(text.into())
}

/// A row of chips that continues on the next line.
fn wrap(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.spacing.tiny + theme.spacing.tiny)
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let mut page = div()
            .id("settings")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .children(self.failure.clone().map(|failure| note(&theme, failure)));
        let Some(config) = self.config.clone() else {
            return page;
        };
        for (index, surface) in config.surfaces.iter().enumerate() {
            page = page.child(self.surface(index, surface, window, cx));
        }
        page = match config.pinned() {
            Some(pins) => page.child(self.pins(&pins, window, cx)),
            None => page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .child(heading(&theme, "Pinned applications"))
                    .child(note(
                        &theme,
                        "Applications are pinned to the `windows` widget, which the layout lacks.",
                    )),
            ),
        };
        page.child(self.themes(config.theme.as_deref(), window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_application_is_called_by_its_file_or_its_name() {
        assert_eq!(pin_title("C:/Windows/System32/charmap.exe"), "charmap");
        assert_eq!(pin_title("Command Prompt"), "Command Prompt");
        assert_eq!(pin_title("notes.v2"), "notes.v2");
    }

    #[test]
    fn paths_are_written_with_forward_slashes() {
        assert_eq!(plain(Path::new(r"C:\Apps\app.exe")), "C:/Apps/app.exe");
    }
}
