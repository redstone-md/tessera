// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The editor of snap zones on the screen, as Meta+T opens the editor of
//! the custom tiling in KWin: a window over the work area of a display
//! that shows the zones of a layout as they will lie, with the gap
//! between them. A click in a zone splits it at the pointer, side by side
//! or one above the other as chosen; a line between two zones is dragged
//! to move it, or clicked to merge the zones beside it. The layout is
//! saved under its name as a `[[snap.layout]]` of `config.toml`, or as a
//! layout file of the tiling in `%APPDATA%\tessera\tiling-layouts`, which
//! the tiling reads again by itself. Enter saves and Esc cancels.

use std::fs;
use std::path::PathBuf;

use gpui::{
    App, AppContext, AsyncApp, Context, Div, Entity, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Point, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, WeakEntity,
    Window, WindowHandle, black, div, px,
};
use toml_edit::Value;

use super::split::{Axis, Line, Split};
use super::{Kind, WIDEST_GAP, open_at};
use crate::config::config_dir;
use crate::config::edit::ConfigFile;
use crate::field::TextField;
use crate::i18n::Language;
use crate::motion::{InteractionState, Spring};
use crate::platform::{Display, ScreenRect};
use crate::popup::retire_floating;
use crate::runners::fill;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{choice_button, one_line, release_hover};

/// The directory of the layout files of the tiling, in the directory of
/// the configuration.
const TILING_LAYOUTS: &str = "tiling-layouts";

/// How thick the lines between zones are to be grabbed, in logical
/// pixels.
const GRIP: f32 = 12.;

/// How dark the work area is behind the zones.
const DIM: f32 = 0.45;

/// How opaque the zones are, and the zone under the pointer.
const ZONE_FILL: f32 = 0.22;
const ZONE_LIT: f32 = 0.38;

/// The buttons of the bar as they are drawn; they light up under the
/// pointer by their own hover style.
const RESTING: InteractionState = InteractionState {
    hover: 0.,
    press: 0.,
};

/// What the layout is saved as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveAs {
    /// A `[[snap.layout]]` of `config.toml`.
    SnapLayout,
    /// A layout file of the tiling.
    TilingFile,
}

/// What a click in a zone does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Split(Axis),
    Merge,
}

/// What the editor begins with.
#[derive(Clone, Debug)]
pub struct Draft {
    pub tree: Split,
    /// The name of the layout; with `replacing`, the name it had.
    pub name: String,
    /// The `[[snap.layout]]` that is changed, by its name, where one is.
    pub replacing: Option<String>,
    /// The room left between zones, as `[snap] gap` says.
    pub gap: u32,
}

/// The root view of the window of the editor.
pub struct ZoneEditor {
    display: Display,
    tree: Split,
    replacing: Option<String>,
    gap: u32,
    /// The gap the file has, which is written only when it changed.
    gap_before: u32,
    save_as: SaveAs,
    tool: Tool,
    name: Entity<TextField>,
    /// The line being dragged.
    dragging: Option<Line>,
    /// The zone under the pointer and where the pointer is, in logical
    /// pixels of the window.
    hovered: Option<(usize, Point<Pixels>)>,
    /// Why the layout was not saved.
    failure: Option<SharedString>,
    saving: bool,
    /// 0 while hidden, 1 when fully shown.
    visibility: Spring,
    closing: bool,
    was_active: bool,
    focus: FocusHandle,
    focused: bool,
    _activation: Subscription,
}

impl ZoneEditor {
    /// Opens the editor over the work area of `display`, with `draft`.
    pub fn open(
        display: Display,
        draft: Draft,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        open_at(
            display.work,
            &display,
            Kind::Popup,
            cx,
            move |window, cx| {
                let motion = cx.global::<Theme>().motion;
                let mut visibility = Spring::new(motion.effects, 0.);
                visibility.animate_to(1.);
                let language = Language::of(cx);
                let name = cx.new(|cx| {
                    let mut field = TextField::new(language.say("Name of the layout").into(), cx);
                    field.set_text(&draft.name, false, cx);
                    field
                });
                let activation =
                    cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                        if window.is_window_active() {
                            this.was_active = true;
                        } else if this.was_active && !this.saving {
                            this.close(cx);
                        }
                    });
                Self {
                    display,
                    tree: draft.tree,
                    replacing: draft.replacing,
                    gap: draft.gap,
                    gap_before: draft.gap,
                    save_as: SaveAs::SnapLayout,
                    tool: Tool::Split(Axis::Columns),
                    name,
                    dragging: None,
                    hovered: None,
                    failure: None,
                    saving: false,
                    visibility,
                    closing: false,
                    was_active: false,
                    focus: cx.focus_handle(),
                    focused: false,
                    _activation: activation,
                }
            },
        )
    }

    /// Starts closing without saving; the window goes away once it faded.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// The width and height of the work area in logical pixels.
    fn logical_size(&self) -> (f32, f32) {
        let scale = self.display.scale.max(0.1);
        let work = self.display.work;
        (
            (work.right - work.left) as f32 / scale,
            (work.bottom - work.top) as f32 / scale,
        )
    }

    /// `rect` of the screen in logical pixels of the window: left, top,
    /// width and height.
    fn logical(&self, rect: ScreenRect) -> [f32; 4] {
        let scale = self.display.scale.max(0.1);
        let work = self.display.work;
        [
            (rect.left - work.left) as f32 / scale,
            (rect.top - work.top) as f32 / scale,
            (rect.right - rect.left) as f32 / scale,
            (rect.bottom - rect.top) as f32 / scale,
        ]
    }

    /// Where `position` of the window lies in shares of the work area.
    fn share_at(&self, position: Point<Pixels>) -> (f32, f32) {
        let (width, height) = self.logical_size();
        (
            (position.x / px(1.) / width.max(1.)).clamp(0., 1.),
            (position.y / px(1.) / height.max(1.)).clamp(0., 1.),
        )
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => self.close(cx),
            "enter" => self.save(cx),
            _ => {}
        }
    }

    /// Does the tool chosen to the zone at `index`, clicked at `position`.
    fn click_zone(&mut self, index: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Tool::Split(axis) = self.tool else {
            return;
        };
        let (x, y) = self.share_at(position);
        let at = match axis {
            Axis::Columns => x,
            Axis::Rows => y,
        };
        if self.tree.split(index, axis, at) {
            self.failure = None;
            cx.notify();
        }
    }

    /// Merges the zones beside `line` with the tool to merge, or begins to
    /// drag it otherwise.
    fn press_line(&mut self, line: Line, cx: &mut Context<Self>) {
        match self.tool {
            Tool::Merge => {
                if self.tree.merge(&line) {
                    cx.notify();
                }
            }
            Tool::Split(_) => self.dragging = Some(line),
        }
    }

    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(line) = self.dragging.clone() else {
            return;
        };
        let (x, y) = self.share_at(event.position);
        let at = match line.axis {
            Axis::Columns => x,
            Axis::Rows => y,
        };
        if self.tree.drag(&line, at) {
            cx.notify();
        }
    }

    fn step_gap(&mut self, up: bool, cx: &mut Context<Self>) {
        self.gap = match up {
            true => (self.gap + 2).min(WIDEST_GAP),
            false => self.gap.saturating_sub(2),
        };
        cx.notify();
    }

    /// Saves the layout as chosen, off the thread of the interface, and
    /// closes the editor once it is saved.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.closing {
            return;
        }
        let language = Language::of(cx);
        let name = self.name.read(cx).text().trim().to_owned();
        if name.is_empty() {
            self.failure = Some(language.say("Give the layout a name.").into());
            cx.notify();
            return;
        }
        let taken = { Shell::config(cx).snap.shown_layouts().into_iter() }
            .filter(|shown| shown.own)
            .any(|shown| shown.key == name && Some(&shown.key) != self.replacing.as_ref());
        if self.save_as == SaveAs::SnapLayout && taken {
            let said = fill(
                language.say("There is a layout named {} already."),
                &[&name],
            );
            self.failure = Some(said.into());
            cx.notify();
            return;
        }
        let zones = self.tree.written();
        let tiling = self.tree.tiling_file(&name);
        let (save_as, replacing) = (self.save_as, self.replacing.clone());
        let gap = (self.gap != self.gap_before).then_some(self.gap);
        self.saving = true;
        self.failure = None;
        let saved = cx.background_executor().spawn(async move {
            match save_as {
                SaveAs::SnapLayout => ConfigFile::change(|file| {
                    if let Some(old) = replacing.filter(|old| *old != name) {
                        file.remove_snap_layout(&old);
                    }
                    if let Some(gap) = gap {
                        file.set_in("snap", "gap", Some(Value::from(i64::from(gap))));
                    }
                    match file.set_snap_layout(&name, &zones) {
                        true => Ok(true),
                        false => Err("the snap layouts of the file are written in a way that cannot be changed here".to_owned()),
                    }
                }),
                SaveAs::TilingFile => write_tiling_file(&name, &tiling).map(|()| false),
            }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let saved = saved.await;
            this.update(cx, |this, cx| {
                this.saving = false;
                match saved {
                    Ok(config) => {
                        if config {
                            cx.defer(Shell::config_changed);
                        }
                        this.close(cx);
                    }
                    Err(error) => {
                        report!("the layout of the zones is not saved: {error}");
                        this.failure = Some(error.into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// A button of the bar of the editor, chosen or not.
    fn chip(
        theme: &Theme,
        id: (&'static str, usize),
        label: impl Into<SharedString>,
        chosen: bool,
    ) -> gpui::Stateful<Div> {
        let ground = match chosen {
            true => theme.colors.secondary_container,
            false => theme.colors.surface_container_highest,
        };
        let lit = theme.colors.primary.opacity(0.18);
        div()
            .id(id)
            .flex_none()
            .h(px(28.))
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(ground)
            .hover(move |style| style.bg(lit))
            .text_color(match chosen {
                true => theme.colors.on_surface,
                false => theme.colors.on_surface_variant,
            })
            .child(one_line(label.into()))
    }

    /// The bar along the top: the name, the tools, the gap, what to save
    /// as, and the buttons.
    fn bar(&mut self, theme: &Theme, language: Language, cx: &mut Context<Self>) -> Div {
        let tools = [
            (Tool::Split(Axis::Columns), "Split side by side"),
            (Tool::Split(Axis::Rows), "Split one above the other"),
            (Tool::Merge, "Merge"),
        ];
        let mut tool_row = div().flex().flex_wrap().gap(theme.spacing.tiny);
        for (index, (tool, label)) in tools.into_iter().enumerate() {
            let chip = Self::chip(
                theme,
                ("zone-tool", index),
                language.say(label),
                self.tool == tool,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tool = tool;
                cx.notify();
            }));
            tool_row = tool_row.child(chip);
        }
        let kinds = [
            (SaveAs::SnapLayout, "Snap layout"),
            (SaveAs::TilingFile, "Tiling layout"),
        ];
        let mut kind_row = div().flex().flex_wrap().gap(theme.spacing.tiny);
        for (index, (kind, label)) in kinds.into_iter().enumerate() {
            let chip = Self::chip(
                theme,
                ("zone-save-as", index),
                language.say(label),
                self.save_as == kind,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.save_as = kind;
                cx.notify();
            }));
            kind_row = kind_row.child(chip);
        }
        let gap_row = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(one_line(language.say("Gap")))
            .child(
                Self::chip(theme, ("zone-gap", 0), "\u{2212}", false)
                    .on_click(cx.listener(|this, _, _, cx| this.step_gap(false, cx))),
            )
            .child(
                div()
                    .min_w(px(48.))
                    .flex()
                    .justify_center()
                    .child(format!("{} px", self.gap)),
            )
            .child(
                Self::chip(theme, ("zone-gap", 1), "+", false)
                    .on_click(cx.listener(|this, _, _, cx| this.step_gap(true, cx))),
            );
        let lit = theme.colors.primary.opacity(0.18);
        let cancel = choice_button(theme, language.say("Cancel"), false, RESTING)
            .id("zone-cancel")
            .hover(move |style| style.bg(lit))
            .on_click(cx.listener(|this, _, _, cx| this.close(cx)));
        let save = choice_button(theme, language.say("Save"), true, RESTING)
            .id("zone-save")
            .hover(|style| style.opacity(0.9))
            .opacity(if self.saving { 0.5 } else { 1. })
            .on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let hint = match self.tool {
            Tool::Split(_) => {
                "Click a zone to split it at the pointer; drag a line to move it. Enter saves, Esc cancels."
            }
            Tool::Merge => "Click a line to merge the zones beside it. Enter saves, Esc cancels.",
        };
        let gap_note = match self.save_as {
            SaveAs::SnapLayout => None,
            SaveAs::TilingFile => Some(language.say("The tiling keeps its own gap.")),
        };
        div()
            .absolute()
            .top(theme.spacing.medium)
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("zone-bar")
                    .max_w(px(760.))
                    .p(theme.spacing.medium)
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.large)
                    .bg(theme.colors.surface_container)
                    .text_color(theme.colors.on_surface)
                    .text_size(theme.typography.label_size)
                    // A press on the bar is not a press on the zones under it.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.small)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .rounded(theme.shape.full)
                                    .border_1()
                                    .border_color(theme.colors.outline)
                                    .child(self.name.clone()),
                            )
                            .child(cancel)
                            .child(save),
                    )
                    .child(tool_row)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(theme.spacing.medium)
                            .child(kind_row)
                            .child(gap_row),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(language.say(hint)),
                    )
                    .children(gap_note.map(|note| {
                        div()
                            .text_color(theme.colors.on_surface_variant)
                            .child(note)
                    }))
                    .children(self.failure.clone().map(|failure| {
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.primary)
                            .child(failure)
                    })),
            )
    }
}

impl Render for ZoneEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if !self.focused {
            self.focused = true;
            window.focus(&self.focus);
        }
        let visibility = self.visibility.value(window).clamp(0., 1.);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let work = self.display.work;
        let gap = self.gap as i32;
        let zones = self.tree.zones();
        let mut root = div()
            .id("zone-editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .size_full()
            .relative()
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .bg(black().opacity(DIM))
            .opacity(visibility)
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| this.pointer_moved(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.dragging = None),
            );
        for (index, zone) in zones.iter().enumerate() {
            let rect = zone.rect(work, gap);
            let [left, top, width, height] = self.logical(rect);
            let lit = self.hovered.is_some_and(|(hovered, _)| hovered == index);
            let fill = theme
                .colors
                .primary
                .opacity(if lit { ZONE_LIT } else { ZONE_FILL });
            let (pixels_wide, pixels_high) = (rect.right - rect.left, rect.bottom - rect.top);
            let mut block = div()
                .id(("zone", index))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .rounded(theme.shape.large)
                .bg(fill)
                .border_2()
                .border_color(theme.colors.primary.opacity(0.8))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(px(28.))
                        .child(SharedString::from((index + 1).to_string())),
                )
                .child(SharedString::from(format!(
                    "{pixels_wide} \u{d7} {pixels_high}"
                )))
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    if this.dragging.is_none() {
                        this.hovered = Some((index, event.position));
                        cx.notify();
                    }
                }))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if !*hovered && this.hovered.is_some_and(|(at, _)| at == index) {
                        this.hovered = None;
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.click_zone(index, event.position, cx)
                    }),
                );
            // Where a click splits the zone.
            if let (true, Tool::Split(axis), Some((_, at))) = (lit, self.tool, self.hovered) {
                let guide = div().absolute().bg(theme.colors.primary);
                let guide = match axis {
                    Axis::Columns => guide
                        .left(at.x - px(left) - px(1.))
                        .top_0()
                        .bottom_0()
                        .w(px(2.)),
                    Axis::Rows => guide
                        .top(at.y - px(top) - px(1.))
                        .left_0()
                        .right_0()
                        .h(px(2.)),
                };
                block = block.child(guide);
            }
            root = root.child(block);
        }
        let (width, height) = self.logical_size();
        for (index, line) in self.tree.lines().into_iter().enumerate() {
            let (left, top, line_width, line_height) = match line.axis {
                Axis::Columns => (
                    line.at * width - GRIP / 2.,
                    line.from * height,
                    GRIP,
                    (line.to - line.from) * height,
                ),
                Axis::Rows => (
                    line.from * width,
                    line.at * height - GRIP / 2.,
                    (line.to - line.from) * width,
                    GRIP,
                ),
            };
            let cursor = match (self.tool, line.axis) {
                (Tool::Merge, _) => gpui::CursorStyle::PointingHand,
                (_, Axis::Columns) => gpui::CursorStyle::ResizeLeftRight,
                (_, Axis::Rows) => gpui::CursorStyle::ResizeUpDown,
            };
            let mark = theme.colors.on_surface.opacity(0.85);
            let grip = div()
                .id(("zone-line", index))
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(line_width))
                .h(px(line_height))
                .flex()
                .items_center()
                .justify_center()
                .cursor(cursor)
                .child(match line.axis {
                    Axis::Columns => div()
                        .w(px(4.))
                        .h(px(48.))
                        .rounded(theme.shape.full)
                        .bg(mark),
                    Axis::Rows => div()
                        .w(px(48.))
                        .h(px(4.))
                        .rounded(theme.shape.full)
                        .bg(mark),
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.press_line(line.clone(), cx);
                    }),
                );
            root = root.child(grip);
        }
        root.child(self.bar(&theme, language, cx))
    }
}

/// What a layout called `name` is called as a file: its letters and
/// digits in lowercase, other characters as hyphens.
pub fn file_name(name: &str) -> String {
    let mut file = String::new();
    for character in name.trim().chars() {
        match character.is_alphanumeric() {
            true => file.extend(character.to_lowercase()),
            false if !file.ends_with('-') => file.push('-'),
            false => {}
        }
    }
    let file = file.trim_matches('-');
    match file.is_empty() {
        true => "layout".to_owned(),
        false => file.to_owned(),
    }
}

/// Writes `text` as the layout file of the tiling for `name`, beside
/// itself first so that the tiling never reads half of it.
fn write_tiling_file(name: &str, text: &str) -> Result<(), String> {
    crate::tiling::check_layout_file(text)?;
    let directory: PathBuf = config_dir()
        .ok_or("there is no place for a configuration")?
        .join(TILING_LAYOUTS);
    let path = directory.join(format!("{}.toml", file_name(name)));
    let failed = |error: std::io::Error| format!("{} cannot be written: {error}", path.display());
    fs::create_dir_all(&directory).map_err(failed)?;
    let draft = path.with_extension("toml.new");
    fs::write(&draft, text).map_err(failed)?;
    fs::rename(&draft, &path).map_err(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_is_saved_under_a_name_a_file_can_have() {
        assert_eq!(file_name("Two Thirds"), "two-thirds");
        assert_eq!(file_name("  wide / narrow!  "), "wide-narrow");
        assert_eq!(file_name("Широкий"), "широкий");
        assert_eq!(file_name("***"), "layout");
    }
}
