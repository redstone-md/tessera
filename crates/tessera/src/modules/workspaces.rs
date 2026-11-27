// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `workspaces` widget: a button for each workspace, the one shown
//! standing out, with its icon, name or number. A click goes to that
//! workspace, the wheel over the widget to the next or previous one, and
//! a click with the other button opens a menu to rename the workspace,
//! give it an icon, remove it or send the window in front there. With
//! `add_button` the widget ends with a button that makes a workspace;
//! with `rows` the buttons are laid out in rows where the bar is high
//! enough, and on a bar at the left or right edge they stand in a
//! column.
//!
//! The module also brings the actions `workspace`, `send-window` and
//! `sticky`, with the keys of the virtual desktops of Windows where the
//! keys of Windows are in use.

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Render, ScrollDelta, ScrollWheelEvent,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{AppWindowId, DisplayKey, VirtualDesktops};
use crate::popup::PopupAnchor;
use crate::theme::Theme;
use crate::ui::{icon, state_layer, text_width};
use crate::workspaces::{Goal, Grid, RenamePrompt, Workspaces as Kept, columns};

/// How often the workspaces are looked at; Windows announces no change
/// of its desktops.
const CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// The height of a button, as a share of the height of a pill.
const BUTTON_SHARE: f32 = 0.66;

/// The room beside a name in its button, in button heights.
const NAME_MARGIN: f32 = 0.7;

/// How much wider than high the button of the shown desktop is.
const SHOWN_STRETCH: f32 = 0.8;

/// The lowest a row of buttons may be for the workspaces to be laid out
/// in rows on a bar; on a lower bar they stay in one row.
const LOWEST_ROW: Pixels = px(10.);

/// How many pixels of scrolling make a notch of the wheel.
const WHEEL_LINE: f32 = 40.;

/// The characters of the icon font offered for a workspace.
const GLYPHS: [&str; 12] = [
    "\u{e80f}", // home
    "\u{e774}", // globe
    "\u{e943}", // code
    "\u{e8bd}", // chat
    "\u{e715}", // mail
    "\u{e8a5}", // document
    "\u{e8b7}", // folder
    "\u{e8d6}", // music
    "\u{e714}", // video
    "\u{e7fc}", // game
    "\u{e713}", // settings
    "\u{e734}", // star
];

const ICON_ADD: &str = "\u{e710}";
const ICON_RENAME: &str = "\u{e8ac}";
const ICON_ICON: &str = "\u{e790}";
const ICON_NO_ICON: &str = "\u{e894}";
const ICON_REMOVE: &str = "\u{e74d}";
const ICON_SEND: &str = "\u{e8a9}";

pub struct WorkspacesModule;

impl Module for WorkspacesModule {
    fn name(&self) -> &'static str {
        "workspaces"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("workspaces", |context, cx| {
            let display = context.display;
            cx.new(|cx| Workspaces::new(display, cx)).into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let mut errors = Vec::new();
        for (name, action) in Self::all() {
            if let Err(error) = actions.register(name, action) {
                errors.push(format!("{error:#}"));
            }
        }
        match errors.is_empty() {
            true => Ok(()),
            false => Err(anyhow::anyhow!(errors.join("; "))),
        }
    }
}

impl WorkspacesModule {
    /// The actions of the module, by name. Explorer keeps the keys of its
    /// virtual desktops to itself while it runs, so they are taken from
    /// Windows; they are bound by default only where the keys of Windows
    /// are in use, which beside Explorer leaves them to Explorer.
    fn all() -> [(&'static str, Action); 3] {
        let workspace = Action::new(|arg, cx| {
            if let Some(goal) = arg.and_then(|arg| Goal::parse(arg, false)) {
                Kept::pursue(goal, cx);
            }
        })
        .taking_reserved_keys()
        .checking(|arg| Goal::check(arg, false))
        .by_default("win+ctrl+right", Some("next"))
        .by_default("win+ctrl+left", Some("previous"))
        .by_default("win+ctrl+up", Some("up"))
        .by_default("win+ctrl+down", Some("down"))
        .by_default("win+ctrl+d", Some("add"))
        .by_default("win+ctrl+f4", Some("remove"));
        let send = Action::new(|arg, cx| {
            if let Some(goal) = arg.and_then(|arg| Goal::parse(arg, true)) {
                Kept::send_window(goal, cx);
            }
        })
        .taking_reserved_keys()
        .checking(|arg| Goal::check(arg, true))
        .by_default("win+ctrl+shift+right", Some("next"))
        .by_default("win+ctrl+shift+left", Some("previous"));
        let sticky = Action::new(|_, cx| {
            let front = AppWindowId::foreground().filter(|id| id.process() != std::process::id());
            if let Some(id) = front {
                Kept::toggle_sticky(id, cx);
            }
        });
        [
            ("workspace", workspace),
            ("send-window", send),
            ("sticky", sticky),
        ]
    }
}

/// The button of one desktop and the motion it is in.
struct Workspace {
    /// 0 while absent, 1 when fully there.
    presence: Spring,
    /// 1 while this is the desktop shown.
    shown: Spring,
    interaction: Interaction,
}

/// What a button shows.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Face {
    /// Its name, or its number.
    label: SharedString,
    /// A character of the icon font, drawn beside the name, or instead
    /// of the number.
    glyph: Option<SharedString>,
    /// Whether the label is a name, not a number.
    named: bool,
}

impl Face {
    fn of(index: usize, cx: &App) -> Self {
        Self {
            label: Kept::label(index, cx).into(),
            glyph: Kept::icon(index, cx).map(SharedString::from),
            named: Kept::is_named(index, cx),
        }
    }

    /// The label shown beside the icon, or alone: none where an icon
    /// stands for a number.
    fn text(&self) -> Option<SharedString> {
        (self.named || self.glyph.is_none()).then(|| self.label.clone())
    }
}

/// How the buttons are laid out on the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// One row, as on a bar at the top or bottom.
    Row,
    /// `rows` rows of `columns` buttons each.
    Grid { rows: usize, columns: usize },
    /// One column, as on a bar at the left or right edge.
    Column,
}

struct Workspaces {
    /// The display of the bar, whose workspaces are shown where every
    /// display has its own.
    display: Option<DisplayKey>,
    desktops: VirtualDesktops,
    /// What each button shows.
    faces: Vec<Face>,
    /// One for each desktop, and for those that are still going away.
    buttons: Vec<Workspace>,
    grid: Grid,
    /// The generation of the workspaces the buttons were made for; a
    /// workspace made or removed gives every button after it another face.
    generation: u64,
    /// Whether the button that makes a workspace is shown, and its
    /// pointer feedback.
    adding: bool,
    adder: Interaction,
    /// What was scrolled and has not made a notch of the wheel yet.
    wheel: f32,
}

impl Workspaces {
    fn new(display: Option<DisplayKey>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                if this.update(cx, Self::look).is_err() {
                    break;
                }
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            display,
            desktops: Kept::now(display, cx),
            faces: Vec::new(),
            buttons: Vec::new(),
            grid: Kept::grid(cx),
            generation: Kept::generation(cx),
            adding: false,
            adder: Interaction::new(&motion),
            wheel: 0.,
        };
        this.adding = this.offers_add(cx);
        this.follow(cx);
        // What is there at the start does not arrive.
        for (index, button) in this.buttons.iter_mut().enumerate() {
            button.presence.jump_to(1.);
            let shown = index == this.desktops.current;
            button.shown.jump_to(if shown { 1. } else { 0. });
        }
        this
    }

    /// Whether the button that makes a workspace is to be shown.
    fn offers_add(&self, cx: &App) -> bool {
        Kept::offers_add(cx) && Kept::can_add(self.display, cx)
    }

    fn look(&mut self, cx: &mut Context<Self>) {
        let desktops = Kept::now(self.display, cx);
        let faces: Vec<Face> = (0..desktops.count)
            .map(|index| Face::of(index, cx))
            .collect();
        // The faces of buttons that are going away stay as they were.
        let changed = faces.iter().zip(&self.faces).any(|(now, was)| now != was);
        let (grid, adding) = (Kept::grid(cx), self.offers_add(cx));
        let generation = Kept::generation(cx);
        if desktops != self.desktops
            || changed
            || grid != self.grid
            || adding != self.adding
            || generation != self.generation
        {
            self.desktops = desktops;
            (self.grid, self.adding, self.generation) = (grid, adding, generation);
            self.follow(cx);
            cx.notify();
        }
    }

    /// What the button at `index` shows.
    fn face(&self, index: usize) -> Face {
        self.faces.get(index).cloned().unwrap_or_else(|| Face {
            label: (index + 1).to_string().into(),
            glyph: None,
            named: false,
        })
    }

    /// Sends the buttons where the desktops are: new ones come, the ones
    /// over the count go, and the shown one stands out.
    fn follow(&mut self, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        for index in 0..self.desktops.count {
            let face = Face::of(index, cx);
            match self.faces.get_mut(index) {
                Some(was) => *was = face,
                None => self.faces.push(face),
            }
        }
        while self.buttons.len() < self.desktops.count {
            self.buttons.push(Workspace {
                presence: Spring::new(motion.spatial, 0.),
                shown: Spring::new(motion.spatial, 0.),
                interaction: Interaction::new(&motion),
            });
        }
        for (index, button) in self.buttons.iter_mut().enumerate() {
            let there = index < self.desktops.count;
            let shown = there && index == self.desktops.current;
            button.presence.animate_to(if there { 1. } else { 0. });
            button.shown.animate_to(if shown { 1. } else { 0. });
        }
    }

    /// Goes a workspace on for each notch scrolled down, and back for
    /// each one up, past the ends where `wrap` says.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let distance = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / WHEEL_LINE,
        };
        let (notches, left) = notches(self.wheel, distance);
        self.wheel = left;
        if notches == 0 {
            return;
        }
        let Some(index) = stepped(
            self.desktops.current,
            self.desktops.count,
            notches,
            self.grid.wrap,
        ) else {
            return;
        };
        Kept::show(index, self.display, cx);
        self.look(cx);
    }

    /// Opens the menu of the workspace at `index` where it was clicked.
    fn open_menu(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let anchor = match vertical(window) {
            // Beside a column, the menu hangs from the pointer.
            true => PopupAnchor::under(window, event.position, cx),
            false => PopupAnchor::at(window, event.position.x, cx),
        };
        match anchor {
            Ok(anchor) => {
                let entries = menu_entries(index, self.display, anchor, cx);
                Menus::open(entries, anchor, cx);
            }
            Err(error) => report!("the menu of a workspace has no place: {error:#}"),
        }
    }

    fn render_button(
        &mut self,
        index: usize,
        layout: Layout,
        height: Pixels,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let face = self.face(index);
        let button = &mut self.buttons[index];
        let presence = button.presence.value(window);
        let shown = button.shown.value(window);
        let state = button.interaction.state(window);
        let colors = theme.colors;
        let typography = theme.typography;
        let text_size = typography.label_size.min(height * 0.8);
        let text = face.text();
        // A name takes the room it needs; a number or an icon, that of a
        // circle.
        let measured = text.as_ref().map_or(px(0.), |text| {
            text_width(text, typography.family, text_size, window)
        });
        let glyph_room = match (&face.glyph, &text) {
            (Some(_), Some(_)) => typography.icon_size.min(height) + theme.spacing.tiny,
            (Some(_), None) => typography.icon_size.min(height),
            _ => px(0.),
        };
        let room = (measured + glyph_room + height * NAME_MARGIN).max(height);
        let stretch = match layout {
            Layout::Row => SHOWN_STRETCH,
            // Rows and columns stay aligned.
            Layout::Grid { .. } | Layout::Column => 0.,
        };
        let width = match layout {
            Layout::Column => height,
            _ => room + height * stretch * shown.max(0.),
        };
        let lit = shown.clamp(0., 1.);
        let ground = mix(
            theme.on_bar(colors.surface_container_highest),
            colors.secondary_container,
            lit,
        );
        // A column has room for a short name or a number alone.
        let text = match layout {
            Layout::Column if face.glyph.is_some() => None,
            _ => text,
        };
        let face_element =
            div()
                .id(("workspace", index))
                .w(width)
                .h(height)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .gap(theme.spacing.tiny)
                .overflow_hidden()
                .rounded(theme.shape.full)
                .bg(state_layer(&theme, ground, state))
                .text_size(text_size)
                .text_color(mix(colors.on_surface_variant, colors.on_surface, lit))
                .font_weight(FontWeight::MEDIUM)
                .on_click(cx.listener(move |this, _, _, cx| {
                    Kept::show(index, this.display, cx);
                    this.look(cx);
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        this.open_menu(index, event, window, cx);
                    }),
                )
                .children(face.glyph.map(|glyph| {
                    icon(&theme, glyph).text_size(typography.icon_size.min(height * 0.8))
                }))
                .children(text);
        let face_element = Interaction::track(face_element, cx, move |this: &mut Self| {
            Some(&mut this.buttons.get_mut(index)?.interaction)
        });
        let gap = theme.spacing.tiny;
        let holder = match layout {
            Layout::Column => div().w(width).h((height + gap) * presence.max(0.)),
            _ => div().w((width + gap) * presence.max(0.)).h(height),
        };
        holder
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(face_element)
            .into_any_element()
    }

    /// The button that makes a workspace.
    fn render_adder(
        &mut self,
        height: Pixels,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.adder.state(window);
        let ground = theme.on_bar(theme.colors.surface_container_highest);
        let button = div()
            .id("workspace-add")
            .size(height)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, ground, state))
            .text_color(theme.colors.on_surface_variant)
            .child(icon(&theme, ICON_ADD).text_size(theme.typography.icon_size.min(height * 0.8)))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(index) = Kept::add(this.display, cx) {
                    Kept::show(index, this.display, cx);
                }
                this.look(cx);
            }));
        let button = Interaction::track(button, cx, |this: &mut Self| Some(&mut this.adder));
        div()
            .flex_none()
            .p(theme.spacing.tiny / 2.)
            .child(button)
            .into_any_element()
    }
}

/// Whether the window of the widget is higher than wide, as a bar at the
/// left or right edge is.
fn vertical(window: &Window) -> bool {
    let size = window.bounds().size;
    size.height > size.width
}

/// How the buttons of `count` workspaces in `rows` rows are laid out on a
/// surface that is `vertical`, or one whose pill is `pill` high: in rows
/// only where each row has room for a button of `lowest` height or more.
fn layout_of(
    count: usize,
    rows: usize,
    vertical: bool,
    pill: Pixels,
    gap: Pixels,
    lowest: Pixels,
) -> Layout {
    if vertical {
        return Layout::Column;
    }
    let rows = rows.min(count.max(1));
    if rows <= 1 {
        return Layout::Row;
    }
    let height = (pill - gap * (rows + 1) as f32) / rows as f32;
    match height >= lowest {
        true => Layout::Grid {
            rows,
            columns: columns(count, rows),
        },
        false => Layout::Row,
    }
}

/// The notches of the wheel that `distance` more of scrolling makes after
/// `left` was scrolled, scrolling down counting forward, and what is left
/// of it.
fn notches(left: f32, distance: f32) -> (i32, f32) {
    // GPUI gives scrolling up as a positive distance.
    let total = left - distance;
    let whole = total.trunc();
    (whole as i32, total - whole)
}

/// The workspace `notches` on from `current` among `count`, wrapping
/// around where `wrap` says and stopping at the ends otherwise; `None`
/// where that is `current`.
fn stepped(current: usize, count: usize, notches: i32, wrap: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let at = current as i64 + i64::from(notches);
    let at = match wrap {
        true => at.rem_euclid(count as i64),
        false => at.clamp(0, count as i64 - 1),
    } as usize;
    (at != current).then_some(at)
}

/// The entries of the menu of the workspace at `index` of `display`:
/// renaming it and giving it an icon, removing it, and sending the window
/// in front there, as far as the workspaces allow each. `anchor` is where
/// the menu of the icons opens.
fn menu_entries(
    index: usize,
    display: Option<DisplayKey>,
    anchor: PopupAnchor,
    cx: &App,
) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let own = Kept::own(cx);
    let mut entries = Vec::new();
    // The virtual desktops of Windows cannot be renamed by a program.
    // What opens from an entry opens once the menu has closed, which it
    // does right after the entry is chosen.
    if own {
        entries.push(MenuEntry::new(
            ICON_RENAME,
            language.say("Rename"),
            move |cx| {
                cx.defer(move |cx| cx.defer(move |cx| RenamePrompt::open(index, anchor, cx)));
            },
        ));
    }
    entries.push(MenuEntry::new(
        ICON_ICON,
        format!("{} \u{25b8}", language.say("Icon")),
        move |cx| {
            cx.defer(move |cx| {
                cx.defer(move |cx| Menus::open(icon_entries(index, cx), anchor, cx));
            });
        },
    ));
    if Kept::can_remove(index, display, cx) {
        entries.push(MenuEntry::new(
            ICON_REMOVE,
            language.say("Remove"),
            move |cx| {
                Kept::remove(index, display, cx);
            },
        ));
    }
    if own && index != Kept::now(display, cx).current {
        let label = language.say("Send window here");
        entries.push(MenuEntry::new(ICON_SEND, label, move |cx| {
            Kept::send_front(index, display, cx);
        }));
    }
    entries
}

/// The entries of the menu of icons for the workspace at `index`: each
/// glyph offered, and none.
fn icon_entries(index: usize, cx: &App) -> Vec<MenuEntry> {
    let language = Language::of(cx);
    let current = Kept::icon(index, cx);
    let mut entries: Vec<MenuEntry> = { GLYPHS.into_iter() }
        .filter(|glyph| current.as_deref() != Some(*glyph))
        .map(|glyph| {
            let label = format!("{} {}", language.say("Icon"), glyph);
            MenuEntry::new(glyph, label, move |cx| {
                Kept::set_icon(index, Some(glyph), cx)
            })
        })
        .collect();
    if current.is_some() {
        entries.insert(
            0,
            MenuEntry::new(ICON_NO_ICON, language.say("No icon"), move |cx| {
                Kept::set_icon(index, None, cx);
            }),
        );
    }
    entries
}

impl Render for Workspaces {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let count = self.desktops.count;
        let pill = theme.bar.pill_height;
        let gap = theme.spacing.tiny;
        let layout = layout_of(
            count,
            self.grid.rows,
            vertical(window),
            pill,
            gap,
            LOWEST_ROW,
        );
        let height = match layout {
            Layout::Grid { rows, .. } => (pill - gap * (rows + 1) as f32) / rows as f32,
            _ => pill * BUTTON_SHARE,
        };
        let buttons: Vec<AnyElement> = (0..self.buttons.len())
            .map(|index| self.render_button(index, layout, height, theme, window, cx))
            .collect();
        // A button over the count that has finished going was just drawn
        // at no width.
        while self.buttons.len() > count
            && self
                .buttons
                .last()
                .is_some_and(|button| button.presence.is_settled())
        {
            self.buttons.pop();
        }
        self.faces.truncate(self.buttons.len());
        let adder = self
            .adding
            .then(|| self.render_adder(height, theme, window, cx));
        let ground = div()
            .flex()
            .rounded(theme.shape.full)
            .bg(theme.on_bar(theme.colors.surface_container))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.wheel(event, cx)));
        match layout {
            Layout::Row => ground
                .h(pill)
                .px(theme.spacing.small - gap / 2.)
                .items_center()
                .children(buttons)
                .children(adder),
            Layout::Column => ground
                .w(pill)
                .py(theme.spacing.small - gap / 2.)
                .flex_col()
                .items_center()
                .children(buttons)
                .children(adder),
            Layout::Grid { columns, .. } => {
                let mut buttons = buttons.into_iter();
                let mut lines = Vec::new();
                while buttons.len() > 0 {
                    let line: Vec<AnyElement> = buttons.by_ref().take(columns).collect();
                    lines.push(div().flex().items_center().children(line));
                }
                ground
                    .h(pill)
                    .px(theme.spacing.small - gap / 2.)
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(gap)
                            .children(lines),
                    )
                    .children(adder)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_steps_down_forward_and_up_back() {
        assert_eq!(notches(0., -1.), (1, 0.));
        assert_eq!(notches(0., 2.), (-2, 0.));
        let (moved, left) = notches(0., -0.5);
        assert_eq!(moved, 0);
        assert_eq!(notches(left, -0.5).0, 1);
    }

    #[test]
    fn a_step_of_the_wheel_stops_at_the_ends_unless_it_wraps() {
        assert_eq!(stepped(0, 3, 1, false), Some(1));
        assert_eq!(stepped(2, 3, 1, false), None);
        assert_eq!(stepped(2, 3, 1, true), Some(0));
        assert_eq!(stepped(0, 3, -1, true), Some(2));
        assert_eq!(stepped(1, 3, -5, false), Some(0));
        assert_eq!(stepped(0, 0, 1, true), None);
        assert_eq!(stepped(0, 1, 1, true), None);
    }

    #[test]
    fn the_buttons_are_in_rows_only_where_the_bar_has_room() {
        let lay = |count, rows, vertical, pill| {
            layout_of(count, rows, vertical, px(pill), px(4.), px(10.))
        };
        assert_eq!(lay(4, 1, false, 36.), Layout::Row);
        assert_eq!(
            lay(4, 2, false, 36.),
            Layout::Grid {
                rows: 2,
                columns: 2
            }
        );
        assert_eq!(
            lay(5, 2, false, 36.),
            Layout::Grid {
                rows: 2,
                columns: 3
            }
        );
        // Three rows of a 30 pixels pill would be 7 pixels each.
        assert_eq!(lay(6, 3, false, 30.), Layout::Row);
        // A single workspace is one row however many are asked for.
        assert_eq!(lay(1, 2, false, 36.), Layout::Row);
        assert_eq!(lay(4, 2, true, 36.), Layout::Column);
    }

    #[test]
    fn the_actions_say_what_they_take() {
        let mut actions = Actions::default();
        WorkspacesModule.actions(&mut actions).unwrap();
        assert_eq!(actions.check("workspace", Some("next")), Ok(()));
        assert_eq!(actions.check("workspace", Some("rename")), Ok(()));
        assert!(actions.check("workspace", None).is_err());
        assert_eq!(actions.check("send-window", Some("4")), Ok(()));
        assert!(actions.check("send-window", Some("add")).is_err());
        assert_eq!(actions.check("sticky", None), Ok(()));
        assert!(actions.takes_reserved_keys("workspace"));
        let keys = actions.default_keys();
        let bound = |press: &str| {
            let key = keys.iter().find(|key| key.press == press)?;
            Some((key.action.clone()?, key.arg.clone()?))
        };
        assert_eq!(
            bound("win+ctrl+right"),
            Some(("workspace".to_owned(), "next".to_owned()))
        );
        assert_eq!(
            bound("win+ctrl+f4"),
            Some(("workspace".to_owned(), "remove".to_owned()))
        );
        assert_eq!(
            bound("win+ctrl+shift+left"),
            Some(("send-window".to_owned(), "previous".to_owned()))
        );
        for key in &keys {
            assert!(
                crate::keys::Press::parse(&key.press).is_some(),
                "`{}` is no combination",
                key.press
            );
        }
    }
}
