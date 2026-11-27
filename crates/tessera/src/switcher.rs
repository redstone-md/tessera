// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The window switcher: the application windows in the middle of a
//! display, gone through with a key combination while its keys to hold
//! stay down, and the chosen one brought in front once they are let go.
//! Opened with keys that hold nothing, or by `tessera do switcher`, it
//! stays open until a window is chosen or it is closed.
//!
//! `[switcher]` sets how it looks (`style`: tiles, large pictures, a list
//! or a row of icons, with `previews` giving the tiles pictures of their
//! windows, which Windows draws and keeps up to date), which windows it
//! offers (`scope`) and where minimized windows go (`minimized`). While
//! it is open, what is typed narrows the windows to those whose title or
//! program has it, the wheel moves the choice, and Delete, Ctrl+W or a
//! click with the middle button closes a window.
//!
//! The action `switcher` goes through the windows like a `[[key]]` with
//! `switch`, and with `arg = "app-next"` through the windows of one
//! application, as Alt+` does on other desktops.

mod choice;
mod layout;

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, App, AsyncApp, Bounds, Context, CursorStyle, Div, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render, ScrollDelta,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Subscription, WeakEntity,
    Window, WindowHandle, div, img, point, px, size,
};
use serde::Deserialize;

use crate::actions::{Action, Actions};
use crate::i18n::Language;
use crate::icons::{AppIcons, Icon};
use crate::motion::{Spring, mix, staggered};
use crate::platform::{
    AppWindow, AppWindowId, ChoiceKey, DisplayKey, Modifiers, NativeSurface, ScreenEdge,
    ScreenRect, TakenKeys, Thumbnail,
};
use crate::popup::{
    MARGIN, PopupAnchor, floating_panel, floating_travel, open_floating, retire_floating,
};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover};
use crate::workspaces::Workspaces;

use choice::{
    Minimized, Scope, Through, answers, choice_key, first_choice, notches, rechosen,
    same_application, step_of, stepped, typed_text,
};
use layout::{Layout, NAME_HEIGHT, Style};

/// What `[switcher]` of the configuration says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitcherConfig {
    /// How the windows are shown.
    #[serde(default)]
    style: Style,
    /// Which windows are offered.
    #[serde(default)]
    scope: Scope,
    /// Where minimized windows go.
    #[serde(default)]
    minimized: Minimized,
    /// Whether a tile of the `tiles` style shows a picture of its window.
    #[serde(default)]
    previews: bool,
}

/// How many windows are offered at most: the ones used last.
const SHOWN: usize = 40;

/// How often it is asked whether the keys to hold are still down.
const RELEASE_CHECK: Duration = Duration::from_millis(15);

/// How often, and how many times, it is looked whether a window asked
/// to close has gone; one that is still there after that asked the user
/// or refused, and stays offered.
const CLOSE_CHECK: Duration = Duration::from_millis(100);
const CLOSE_CHECKS: usize = 15;

/// How much of the width and the height of the display the switcher
/// takes at most.
const ROOM_SHARE: f32 = 0.9;

/// The width the panel has at least where a line of text stands above
/// or below the windows.
const LINE_WIDTH: f32 = 320.;

/// How many pixels of a scroll by pixels count as one notch of the wheel.
const WHEEL_LINE: f32 = 40.;

/// The side of the icon that stands in a picture's room for a window
/// that has no picture.
const ROOM_ICON: f32 = 48.;

/// Where a picture that is not to be seen is put, wholly transparent.
const NOWHERE: ScreenRect = ScreenRect {
    left: 0,
    top: 0,
    right: 0,
    bottom: 0,
};

/// The glyph of the line of what is typed.
const GLYPH_FILTER: &str = "\u{e721}";

/// How a step of the `switcher` action is described where it is
/// refused.
const STEPS: &str = "`switcher` takes `next`, `previous`, `app-next` or `app-previous`, or none";

/// The open switcher.
#[derive(Default)]
pub struct Switcher {
    open: Option<WindowHandle<SwitcherView>>,
    /// Whether the keys that move through the tiles are listened for.
    listening: bool,
    config: SwitcherConfig,
}

impl gpui::Global for Switcher {}

impl Switcher {
    /// Takes in the configuration; a switcher that is open stays as it is.
    /// The first time, the action `switcher` is added.
    pub fn configure(config: &SwitcherConfig, cx: &mut App) {
        if !cx.has_global::<Self>() {
            Actions::add("switcher", Self::action(), cx);
        }
        cx.default_global::<Self>().config = *config;
    }

    /// The action `switcher`: a step through the windows its `arg` names,
    /// until none of the keys to hold of the combination that did it is
    /// down (for `tessera do`, of the keys down when it is done). It takes
    /// its keys from Windows, and brings Alt+Tab, Alt+Shift+Tab, Alt+` and
    /// Alt+Shift+` where the keys of Windows are in use, as when Tessera
    /// is the shell and Explorer, whose switcher Windows opens for Alt+Tab,
    /// does not run.
    fn action() -> Action {
        Action::new(|arg, cx| {
            if let Some((back, through)) = step_of(arg) {
                let held = Actions::held(cx).unwrap_or_else(Modifiers::down_now);
                Self::go(back, through, held, cx);
            }
        })
        .taking_reserved_keys()
        .checking(|arg| step_of(arg).map(|_| ()).ok_or_else(|| STEPS.to_owned()))
        .by_default("alt+tab", Some("next"))
        .by_default("alt+shift+tab", Some("previous"))
        .by_default("alt+backquote", Some("app-next"))
        .by_default("alt+shift+backquote", Some("app-previous"))
    }

    /// Goes one window on in the open switcher, or back with `back`; opens
    /// it when it is not open, with the window used before the one in
    /// front chosen. The choice is taken once none of `held` is down; with
    /// nothing held, once it is made.
    pub fn step(back: bool, held: Modifiers, cx: &mut App) {
        Self::go(back, Through::All, held, cx);
    }

    /// The same, through the windows `through` names.
    fn go(back: bool, through: Through, held: Modifiers, cx: &mut App) {
        if let Some(open) = cx.default_global::<Self>().open {
            let stepped = open.update(cx, |view, _, cx| view.step(back, through, cx));
            if matches!(stepped, Ok(true)) {
                return;
            }
        }
        Self::open(back, through, held, cx);
    }

    fn open(back: bool, through: Through, held: Modifiers, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            report!("the window switcher has nowhere to open");
            return;
        };
        let config = cx.global::<Self>().config;
        let mut entries = gather(config, anchor.display, cx);
        if entries.is_empty() {
            return;
        }
        let front = AppWindowId::foreground();
        let front = entries.iter().position(|entry| Some(entry.id) == front);
        // The windows of one application are those of the window in
        // front, or else of the one used last.
        let app = (through == Through::App).then(|| Followed::of(&entries[front.unwrap_or(0)]));
        let shown = offered(&entries, app.as_ref(), "");
        let front = front.and_then(|front| shown.iter().position(|&index| index == front));
        let chosen = first_choice(shown.len(), front, back);
        let theme = *cx.global::<Theme>();
        let measures = Measures::of(&theme);
        let chrome = Chrome {
            typed: false,
            title: config.style == Style::Icons,
        };
        let layout = measures.layout(config, shown.len(), &anchor, chrome);
        let (width, height) = measures.panel(&layout, chrome);
        let frame = anchor.middle(width, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(width), px(height)),
        };
        let held = (held != Modifiers::default()).then_some(held);
        let opened = open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let native = NativeSurface::of(window);
            if let (Some(_), Ok(native)) = (layout.picture, &native) {
                for entry in entries.iter_mut().filter(|entry| entry.may_be_pictured()) {
                    let thumbnail = Thumbnail::of(entry.id, *native).ok();
                    entry.picture = thumbnail.and_then(|thumbnail| {
                        let source = thumbnail.source_size()?;
                        Some((thumbnail, source))
                    });
                }
            }
            let adopted = native.and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the window switcher is not where it belongs: {error:#}");
            }
            let opening = Opening {
                entries,
                shown,
                chosen,
                app,
                config,
                anchor,
                measures,
                layout,
                frame,
                held,
            };
            SwitcherView::new(opening, window, cx)
        });
        match opened {
            Ok(window) => {
                cx.global_mut::<Self>().open = Some(window);
                Self::take_keys(cx);
            }
            Err(error) => report!("the window switcher did not open: {error:#}"),
        }
    }

    /// Takes the arrow keys, Enter and Esc from the keyboard for the open
    /// switcher; its view gives them back when it closes.
    fn take_keys(cx: &mut App) {
        if let Err(error) = TakenKeys::take_choice(true) {
            report!("the arrow keys do not reach the window switcher: {error:#}");
            return;
        }
        if std::mem::replace(&mut cx.global_mut::<Self>().listening, true) {
            return;
        }
        let presses = TakenKeys::choice_presses();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(key) = presses.recv().await {
                let pressed = cx.update(|cx| {
                    if let Some(open) = cx.global::<Self>().open {
                        open.update(cx, |view, _, cx| view.key(key, cx)).ok();
                    }
                });
                if pressed.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

/// The windows `config` offers, in the order they are offered, for a
/// switcher on `display`: those of the workspace shown, used last first,
/// then, where the scope is `all`, those of the other workspaces of
/// Tessera's own, workspace by workspace.
fn gather(config: SwitcherConfig, display: Option<DisplayKey>, cx: &App) -> Vec<Entry> {
    let own = std::process::id();
    let on_display = |window: &AppWindow| {
        config.scope != Scope::Display || display.is_none_or(|display| window.display == display)
    };
    let mut found: Vec<Found> = AppWindow::enumerate()
        .into_iter()
        .filter(|window| window.id.process() != own && on_display(window))
        .map(|window| Found {
            id: window.id,
            program: window.program(),
            workspace: None,
        })
        .collect();
    if config.scope == Scope::All && cx.has_global::<Workspaces>() {
        found.extend(elsewhere(&found, display, cx));
    }
    let found: Vec<(Found, bool)> = found
        .into_iter()
        .map(|found| {
            let minimized = found.id.is_minimized();
            (found, minimized)
        })
        .collect();
    let found = config.minimized.arrange(found, |(_, minimized)| *minimized);
    let icons = cx.global::<AppIcons>();
    let language = Language::of(cx);
    { found.into_iter().take(SHOWN) }
        .map(|(found, minimized)| {
            let name = { found.program.as_deref() }
                .and_then(|program| program.file_stem())
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let place = found.workspace.map(|index| {
                let label = Workspaces::label(index, cx);
                match label.chars().all(|c| c.is_ascii_digit()) {
                    true => format!("{} {label}", language.say("Workspace")).into(),
                    false => label.into(),
                }
            });
            Entry {
                id: found.id,
                title: found.id.title().into(),
                icon: { found.program.as_deref() }.and_then(|program| icons.of_executable(program)),
                name: name.into(),
                program: found.program,
                workspace: found.workspace,
                place,
                minimized,
                closing: false,
                picture: None,
            }
        })
        .collect()
}

/// The windows of the workspaces of Tessera's own that are not shown, on
/// `display` where every display has its own, that are not among `found`.
fn elsewhere(found: &[Found], display: Option<DisplayKey>, cx: &App) -> Vec<Found> {
    let Some(residents) = Workspaces::residents(display, cx) else {
        return Vec::new();
    };
    let current = Workspaces::now(display, cx).current;
    let mut elsewhere: Vec<Found> = Vec::new();
    for (index, windows) in residents.into_iter().enumerate() {
        if index == current {
            continue;
        }
        for resident in windows {
            let known = |other: &Found| other.id == resident.id;
            if found.iter().any(known) || elsewhere.iter().any(known) || !resident.id.exists() {
                continue;
            }
            elsewhere.push(Found {
                id: resident.id,
                program: resident.program,
                workspace: Some(index),
            });
        }
    }
    elsewhere
}

/// A window found for the switcher, before it is described.
struct Found {
    id: AppWindowId,
    program: Option<PathBuf>,
    /// The workspace of Tessera's own it is on, where that is not the one
    /// shown.
    workspace: Option<usize>,
}

/// A window to switch to.
struct Entry {
    id: AppWindowId,
    title: SharedString,
    icon: Icon,
    /// What the icon is taken from and what tells the windows of one
    /// application, and the name of its file without the extension,
    /// which typing finds as well as the title.
    program: Option<PathBuf>,
    name: SharedString,
    /// The workspace of Tessera's own it is on, where it is not the one
    /// shown, and what the list says of it.
    workspace: Option<usize>,
    place: Option<SharedString>,
    minimized: bool,
    /// Whether it was asked to close and has not gone yet.
    closing: bool,
    /// The picture of the window and the physical size of the window,
    /// where the cells show pictures and Windows gives one.
    picture: Option<(Thumbnail, (i32, i32))>,
}

impl Entry {
    /// Whether Windows can draw a picture of the window: a minimized one
    /// and one hidden on another workspace have none to give, so their
    /// icon stands in its place.
    fn may_be_pictured(&self) -> bool {
        !self.minimized && self.workspace.is_none()
    }

    /// Hides the picture of the window, where it has one.
    fn hide_picture(&self) {
        if let Some((thumbnail, _)) = &self.picture {
            thumbnail.place(NOWHERE, 0.);
        }
    }
}

/// The application whose windows alone are offered.
#[derive(Clone, Debug)]
struct Followed {
    /// The window it was taken from, which belongs to it whatever its
    /// program is.
    id: AppWindowId,
    program: Option<PathBuf>,
}

impl Followed {
    fn of(entry: &Entry) -> Self {
        Self {
            id: entry.id,
            program: entry.program.clone(),
        }
    }

    fn has(&self, entry: &Entry) -> bool {
        entry.id == self.id || same_application(entry.program.as_deref(), self.program.as_deref())
    }
}

/// The places in `entries` of the windows that are offered: those of the
/// application `app` follows, where it follows one, that answer what was
/// `typed`.
fn offered(entries: &[Entry], app: Option<&Followed>, typed: &str) -> Vec<usize> {
    let offered = |entry: &Entry| {
        app.is_none_or(|app| app.has(entry)) && answers(&entry.title, &entry.name, typed)
    };
    { entries.iter().enumerate() }
        .filter(|(_, entry)| offered(entry))
        .map(|(index, _)| index)
        .collect()
}

/// What is shown around the cells: the line of what is typed above them,
/// and the title of the chosen window under them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chrome {
    typed: bool,
    title: bool,
}

impl Chrome {
    fn lines(self) -> usize {
        usize::from(self.typed) + usize::from(self.title)
    }
}

/// The distances of the theme the switcher is laid out with, in logical
/// pixels: between cells, from the border of the panel, and between the
/// lines of text and the cells.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Measures {
    gap: f32,
    padding: f32,
    between: f32,
}

impl Measures {
    fn of(theme: &Theme) -> Self {
        Self {
            gap: theme.spacing.small / px(1.),
            padding: theme.spacing.medium / px(1.),
            between: theme.spacing.medium / px(1.),
        }
    }

    /// The cells of `count` windows as `config` shows them, with
    /// `chrome`, on the display of `anchor`.
    fn layout(
        self,
        config: SwitcherConfig,
        count: usize,
        anchor: &PopupAnchor,
        chrome: Chrome,
    ) -> Layout {
        let screen = anchor.screen;
        let logical = |length: i32| length as f32 / anchor.scale;
        let display = (
            logical(screen.right - screen.left),
            logical(screen.bottom - screen.top),
        );
        let room = self.room(display, chrome);
        Layout::new(config.style, config.previews, count, room, self.gap)
    }

    /// The room the cells have on a display of `display` logical pixels,
    /// with `chrome`.
    fn room(self, (width, height): (f32, f32), chrome: Chrome) -> (f32, f32) {
        let usable = |length: f32| (length - MARGIN * 2.) * ROOM_SHARE - self.padding * 2.;
        let lines = chrome.lines() as f32 * (NAME_HEIGHT + self.between);
        (usable(width).max(0.), (usable(height) - lines).max(0.))
    }

    /// The logical width and height of the panel around the cells of
    /// `layout` that are seen at once, with `chrome`.
    fn panel(self, layout: &Layout, chrome: Chrome) -> (f32, f32) {
        let (width, height) = layout.grid();
        let width = match chrome.lines() {
            0 => width,
            _ => width.max(LINE_WIDTH),
        };
        let lines = chrome.lines() as f32 * (NAME_HEIGHT + self.between);
        (
            width + self.padding * 2.,
            height + lines + self.padding * 2.,
        )
    }
}

/// What a switcher opens with.
struct Opening {
    entries: Vec<Entry>,
    shown: Vec<usize>,
    chosen: usize,
    app: Option<Followed>,
    config: SwitcherConfig,
    anchor: PopupAnchor,
    measures: Measures,
    layout: Layout,
    frame: ScreenRect,
    /// The keys whose release takes the choice; `None` keeps the
    /// switcher open until a choice is made.
    held: Option<Modifiers>,
}

/// The root view of the switcher window.
pub struct SwitcherView {
    entries: Vec<Entry>,
    /// The places in `entries` of the windows offered, in order.
    shown: Vec<usize>,
    /// Which of those is brought in front when the keys are let go.
    chosen: usize,
    /// The application whose windows alone are offered, if one is.
    app: Option<Followed>,
    /// What was typed, which the windows offered answer.
    typed: String,
    config: SwitcherConfig,
    anchor: PopupAnchor,
    measures: Measures,
    layout: Layout,
    /// Where the window was put last, in physical pixels.
    frame: ScreenRect,
    /// The first row seen, where not all are.
    first_row: usize,
    /// The column and the row of the mark behind the chosen cell, which
    /// travels from cell to cell, and the row seen first, which moves as
    /// the rows scroll.
    column: Spring,
    row: Spring,
    scroll: Spring,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    /// What the wheel scrolled that makes no notch yet.
    wheel: f32,
    focus: FocusHandle,
    _activation: Subscription,
}

impl SwitcherView {
    fn new(opening: Opening, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut visibility = Spring::new(motion.spatial, 0.);
        visibility.animate_to(1.);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.was_active = true;
            } else if this.was_active {
                this.close(cx);
            }
        });
        if let Some(held) = opening.held {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(RELEASE_CHECK).await;
                    if !held.any_down() {
                        break;
                    }
                    if this.upgrade().is_none() {
                        return;
                    }
                }
                this.update(cx, |this, cx| this.choose(this.chosen, cx))
                    .ok();
            })
            .detach();
        }
        let focus = cx.focus_handle();
        window.focus(&focus);
        let layout = opening.layout;
        let (column, row) = layout.cell_of(opening.chosen);
        let first_row = layout.first_row(0, opening.chosen);
        Self {
            column: Spring::new(motion.spatial, column as f32),
            row: Spring::new(motion.spatial, row as f32),
            scroll: Spring::new(motion.spatial, first_row as f32),
            first_row,
            entries: opening.entries,
            shown: opening.shown,
            chosen: opening.chosen,
            app: opening.app,
            typed: String::new(),
            config: opening.config,
            anchor: opening.anchor,
            measures: opening.measures,
            layout,
            frame: opening.frame,
            visibility,
            closing: false,
            was_active: false,
            wheel: 0.,
            focus,
            _activation: activation,
        }
    }

    /// Chooses the window after the chosen one, or before it with `back`,
    /// among those `through` names: the windows of the application of the
    /// chosen one, or all again. `false` when the switcher is closing and
    /// takes no more of that.
    fn step(&mut self, back: bool, through: Through, cx: &mut Context<Self>) -> bool {
        if self.closing {
            return false;
        }
        let app = match through {
            Through::All => None,
            Through::App => self.app.clone().or_else(|| {
                let chosen = self.shown.get(self.chosen)?;
                Some(Followed::of(&self.entries[*chosen]))
            }),
        };
        if app.as_ref().map(|app| app.id) != self.app.as_ref().map(|app| app.id) {
            let before = self.shown_windows();
            self.app = app;
            self.refresh(&before, cx);
        }
        if !self.shown.is_empty() {
            self.mark(stepped(self.chosen, self.shown.len(), back), cx);
        }
        true
    }

    /// The windows offered, in order.
    fn shown_windows(&self) -> Vec<AppWindowId> {
        self.shown
            .iter()
            .map(|&index| self.entries[index].id)
            .collect()
    }

    /// Offers the windows anew after the entries, the application or what
    /// was typed changed, `before` being the windows offered until now:
    /// the choice stays on its window where it can, and the cells are
    /// laid out again.
    fn refresh(&mut self, before: &[AppWindowId], cx: &mut Context<Self>) {
        self.shown = offered(&self.entries, self.app.as_ref(), &self.typed);
        let chosen = rechosen(before, self.chosen, &self.shown_windows());
        let chrome = self.chrome();
        let layout = self
            .measures
            .layout(self.config, self.shown.len(), &self.anchor, chrome);
        self.layout = layout;
        self.first_row = self.first_row.min(layout.rows.saturating_sub(1));
        self.mark(chosen, cx);
    }

    fn chrome(&self) -> Chrome {
        Chrome {
            typed: !self.typed.is_empty(),
            title: self.config.style == Style::Icons,
        }
    }

    /// Has the window offered at `chosen` be the chosen one, and brings
    /// its row into sight.
    fn mark(&mut self, chosen: usize, cx: &mut Context<Self>) {
        self.chosen = chosen;
        let (column, row) = self.layout.cell_of(chosen);
        self.column.animate_to(column as f32);
        self.row.animate_to(row as f32);
        self.first_row = self.layout.first_row(self.first_row, chosen);
        self.scroll.animate_to(self.first_row as f32);
        cx.notify();
    }

    /// Takes a key pressed while the switcher is open: an arrow moves the
    /// choice, Enter takes it, and Esc forgets what was typed, or closes
    /// without bringing a window in front when nothing was.
    fn key(&mut self, key: ChoiceKey, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        match key {
            ChoiceKey::Escape if !self.typed.is_empty() => self.type_into(None, cx),
            ChoiceKey::Escape => self.close(cx),
            ChoiceKey::Enter => self.choose(self.chosen, cx),
            arrow => {
                let count = self.shown.len();
                if count > 0 {
                    self.mark(self.layout.moved(self.chosen, count, arrow), cx);
                }
            }
        }
    }

    /// Takes a key that reached the window rather than the keyboard hook:
    /// what is typed, Backspace, Tab, Delete and Ctrl+W, and the keys of a
    /// choice when the hook did not take them.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let keystroke = &event.keystroke;
        let held = keystroke.modifiers;
        let key = keystroke.key.as_str();
        if let Some(choice) = choice_key(key) {
            self.key(choice, cx);
        } else if key == "tab" {
            self.step(held.shift, Through::All, cx);
        } else if key == "delete" || (key == "w" && held.control) {
            self.close_window(self.chosen, cx);
        } else if key == "backspace" {
            let mut typed = self.typed.clone();
            typed.pop();
            self.type_into(Some(typed), cx);
        } else if let Some(text) =
            typed_text(key, keystroke.key_char.as_deref(), held.control, held.alt)
        {
            let typed = self.typed.clone() + &text;
            self.type_into(Some(typed), cx);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    /// Has `typed` be what was typed, or nothing.
    fn type_into(&mut self, typed: Option<String>, cx: &mut Context<Self>) {
        let typed = typed.unwrap_or_default();
        if typed == self.typed {
            return;
        }
        let before = self.shown_windows();
        self.typed = typed;
        self.refresh(&before, cx);
    }

    /// Moves the choice a notch of the wheel on for each notch scrolled
    /// down, and back for each one up.
    fn wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let distance = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / WHEEL_LINE,
        };
        let (moved, left) = notches(self.wheel, distance);
        self.wheel = left;
        let count = self.shown.len() as i64;
        if moved == 0 || count == 0 || self.closing {
            return;
        }
        let at = (self.chosen as i64 + i64::from(moved)).rem_euclid(count);
        self.mark(at as usize, cx);
    }

    /// Asks the window offered at `position` to close, as its close button
    /// does, and stops offering it once it is gone. The application may
    /// ask the user first, or refuse; the window then stays.
    fn close_window(&mut self, position: usize, cx: &mut Context<Self>) {
        let Some(&index) = self.shown.get(position) else {
            return;
        };
        let entry = &mut self.entries[index];
        if entry.closing {
            return;
        }
        entry.closing = true;
        let (id, shown) = (entry.id, entry.workspace.is_none());
        id.close();
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            for _ in 0..CLOSE_CHECKS {
                cx.background_executor().timer(CLOSE_CHECK).await;
                // A window that only hides when closed, as one that goes
                // to the notification area, is gone as well.
                if !id.exists() || (shown && !id.is_shown()) {
                    this.update(cx, |this, cx| this.forget(id, cx)).ok();
                    return;
                }
            }
            let stays = |this: &mut Self, cx: &mut Context<Self>| {
                let entry = this.entries.iter_mut().find(|entry| entry.id == id);
                if let Some(entry) = entry {
                    entry.closing = false;
                    cx.notify();
                }
            };
            this.update(cx, stays).ok();
        })
        .detach();
    }

    /// Stops offering the window `id`, which has gone; the switcher closes
    /// once no window is left.
    fn forget(&mut self, id: AppWindowId, cx: &mut Context<Self>) {
        let before = self.shown_windows();
        if let Some(index) = self.entries.iter().position(|entry| entry.id == id) {
            self.entries[index].hide_picture();
            self.entries.remove(index);
        }
        if self.entries.is_empty() {
            self.close(cx);
            return;
        }
        self.refresh(&before, cx);
    }

    /// Brings the window offered at `position` in front, going to its
    /// workspace first where it is on another one, and closes.
    fn choose(&mut self, position: usize, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        let entry = self.shown.get(position).map(|&index| &self.entries[index]);
        match entry.map(|entry| (entry.id, entry.workspace)) {
            Some((id, Some(_))) => Workspaces::show_window(id, cx),
            Some((id, None)) => id.activate(),
            None => {}
        }
        self.close(cx);
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        give_keys_back();
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// Puts the window where the panel of the cells laid out now stands,
    /// when that is another place than before.
    fn place_window(&mut self, window: &Window) {
        let (width, height) = self.measures.panel(&self.layout, self.chrome());
        let frame = self.anchor.middle(width, height);
        if frame == self.frame {
            return;
        }
        self.frame = frame;
        if let Err(error) = NativeSurface::of(window).and_then(|native| native.place(frame)) {
            report!("the window switcher is not where it belongs: {error:#}");
        }
    }
}

/// Gives the keys taken for the open switcher back to the programs.
fn give_keys_back() {
    if let Err(error) = TakenKeys::take_choice(false) {
        report!("the arrow keys stay with the window switcher: {error:#}");
    }
}

impl Drop for SwitcherView {
    /// However the view goes, the keys do not stay taken.
    fn drop(&mut self) {
        if !self.closing {
            give_keys_back();
        }
    }
}

/// What every cell of a frame is drawn with.
struct Drawing {
    theme: Theme,
    visibility: f32,
    /// The row seen first, a fraction while the rows scroll.
    scroll: f32,
    /// Where the first cell seen begins, in logical pixels of the window.
    origin: (f32, f32),
    /// Physical pixels per logical one.
    scale: f32,
}

impl Drawing {
    /// The icon of `entry`, `side` long, or a blank square where it has
    /// none.
    fn icon(&self, entry: &Entry, side: f32) -> AnyElement {
        match entry.icon.clone() {
            Some(icon) => img(icon).size(px(side)).flex_none().into_any_element(),
            None => div()
                .size(px(side))
                .flex_none()
                .rounded(self.theme.shape.small)
                .bg(self.theme.colors.outline)
                .into_any_element(),
        }
    }

    /// The line of what was typed, above the cells.
    fn typed(&self, typed: &str) -> Div {
        let theme = &self.theme;
        div()
            .flex_none()
            .w_full()
            .h(px(NAME_HEIGHT))
            .px(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .rounded(theme.shape.full)
            .bg(theme.colors.surface_container_highest)
            .child(
                icon(theme, GLYPH_FILTER)
                    .flex_none()
                    .text_color(theme.colors.on_surface_variant),
            )
            .child(div().min_w_0().child(one_line(typed.to_owned())))
    }

    /// The title of the chosen window, under a row of icons.
    fn title(&self, entry: Option<&Entry>) -> Div {
        div()
            .flex_none()
            .w_full()
            .h(px(NAME_HEIGHT))
            .flex()
            .items_center()
            .justify_center()
            .font_weight(FontWeight::MEDIUM)
            .children(entry.map(|entry| div().min_w_0().child(one_line(entry.title.clone()))))
    }
}

impl Render for SwitcherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        self.place_window(window);
        let layout = self.layout;
        let measures = self.measures;
        let chrome = self.chrome();
        let (grid_width, grid_height) = layout.grid();
        let (panel_width, _) = measures.panel(&layout, chrome);
        let scroll = self.scroll.value(window);
        let travel = floating_travel(ScreenEdge::Top, visibility) / px(1.);
        let above = match chrome.typed {
            true => NAME_HEIGHT + measures.between,
            false => 0.,
        };
        let drawing = Drawing {
            theme,
            visibility,
            scroll,
            origin: (
                MARGIN + measures.padding + (panel_width - measures.padding * 2. - grid_width) / 2.,
                MARGIN + travel + measures.padding + above,
            ),
            scale: window.scale_factor(),
        };
        let mut offered = vec![false; self.entries.len()];
        for &index in &self.shown {
            offered[index] = true;
        }
        for (entry, _) in { self.entries.iter().zip(offered) }.filter(|(_, offered)| !offered) {
            entry.hide_picture();
        }
        let cells: Vec<AnyElement> = (0..self.shown.len())
            .filter_map(|position| self.cell(position, &drawing, cx))
            .collect();
        let (column, row) = (self.column.value(window), self.row.value(window));
        let (left, top) = layout.place(column, row - scroll);
        let corner = match layout.style {
            Style::List => theme.shape.medium,
            _ => theme.shape.large,
        };
        let mark = (!self.shown.is_empty()).then(|| {
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(layout.cell.0))
                .h(px(layout.cell.1))
                .rounded(corner)
                .bg(theme.colors.secondary_container)
        });
        let nothing = self.shown.is_empty().then(|| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(Language::of(cx).say("Nothing matches"))
        });
        let grid = div()
            .relative()
            .flex_none()
            .w(px(grid_width))
            .h(px(grid_height))
            .overflow_hidden()
            .children(mark)
            .children(cells)
            .children(nothing);
        let chosen = self
            .shown
            .get(self.chosen)
            .map(|&index| &self.entries[index]);
        let typed = chrome.typed.then(|| drawing.typed(&self.typed));
        let title = chrome.title.then(|| drawing.title(chosen));
        let panel = floating_panel(&theme, ScreenEdge::Top, visibility, |panel| {
            panel
                .items_center()
                .children(typed)
                .child(grid)
                .children(title)
        });
        panel
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_scroll_wheel(cx.listener(|this, event, _, cx| this.wheel(event, cx)))
    }
}

impl SwitcherView {
    /// The cell of the window offered at `position`, where its row is in
    /// sight or comes into it; the picture of its window is placed over
    /// it, or hidden while the row is not wholly in sight, since Windows
    /// draws it over everything the switcher draws.
    fn cell(
        &self,
        position: usize,
        drawing: &Drawing,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let layout = &self.layout;
        let entry = &self.entries[self.shown[position]];
        let (column, row) = layout.cell_of(position);
        let row = row as f32 - drawing.scroll;
        if row <= -1. || row >= layout.seen_rows as f32 {
            entry.hide_picture();
            return None;
        }
        let theme = &drawing.theme;
        let count = self.shown.len();
        let arrival = staggered(drawing.visibility, position, count);
        let rise = 8. * (1. - arrival);
        let (left, top) = layout.place(column as f32, row);
        let chosen = position == self.chosen;
        let color = match chosen {
            true => theme.colors.on_surface,
            false => theme.colors.on_surface_variant,
        };
        let opacity = match entry.closing {
            true => arrival * 0.5,
            false => arrival,
        };
        if let (Some(room), Some((thumbnail, source))) = (layout.picture, &entry.picture) {
            let in_sight = row >= -0.01 && row <= layout.seen_rows as f32 - 0.99;
            match layout.fitted(*source, drawing.scale) {
                Some((width, height)) if in_sight => {
                    let physical = |length: f32| (length * drawing.scale).round() as i32;
                    let content = room.1 + theme.spacing.small / px(1.) + NAME_HEIGHT;
                    let left = drawing.origin.0 + left + (layout.cell.0 - width) / 2.;
                    let top = drawing.origin.1
                        + top
                        + rise
                        + (layout.cell.1 - content) / 2.
                        + (room.1 - height) / 2.;
                    let area = ScreenRect {
                        left: physical(left),
                        top: physical(top),
                        right: physical(left + width),
                        bottom: physical(top + height),
                    };
                    thumbnail.place(area, opacity * drawing.visibility.clamp(0., 1.));
                }
                _ => entry.hide_picture(),
            }
        }
        let cell = div()
            .id(("switcher-cell", position))
            .absolute()
            .left(px(left))
            .top(px(top + rise))
            .w(px(layout.cell.0))
            .h(px(layout.cell.1))
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .opacity(opacity)
            .cursor(CursorStyle::PointingHand)
            .text_color(mix(
                theme.colors.on_surface_variant,
                color,
                drawing.visibility,
            ))
            .when_weight(chosen)
            .on_click(cx.listener(move |this, _, _, cx| this.choose(position, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close_window(position, cx)),
            );
        let title = || {
            div()
                .min_w_0()
                .max_w_full()
                .child(one_line(entry.title.clone()))
        };
        let cell = match (layout.style, layout.picture) {
            (Style::List, _) => {
                let detail = match &entry.place {
                    Some(place) if entry.name.is_empty() => place.clone(),
                    Some(place) => format!("{} \u{b7} {place}", entry.name).into(),
                    None => entry.name.clone(),
                };
                cell.px(theme.spacing.medium)
                    .child(drawing.icon(entry, layout.icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(one_line(entry.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .max_w(px(layout.cell.0 / 3.))
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(detail)),
                    )
            }
            (Style::Icons, _) => cell
                .justify_center()
                .child(drawing.icon(entry, layout.icon)),
            (_, Some(room)) => {
                // Windows draws the picture over this room; a window it
                // gives none of shows its icon there.
                let pictured = entry.picture.is_some();
                let room = div()
                    .w(px(room.0))
                    .h(px(room.1))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .children((!pictured).then(|| drawing.icon(entry, ROOM_ICON)));
                let name = div()
                    .max_w_full()
                    .h(px(NAME_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .child(drawing.icon(entry, layout.icon))
                    .child(title());
                cell.flex_col().justify_center().child(room).child(name)
            }
            (_, None) => cell
                .px(theme.spacing.small)
                .flex_col()
                .justify_center()
                .child(drawing.icon(entry, layout.icon))
                .child(title()),
        };
        Some(cell.into_any_element())
    }
}

/// The weight of the title of the chosen tile.
trait Weighted: Styled {
    fn when_weight(self, chosen: bool) -> Self {
        match chosen {
            true => self.font_weight(FontWeight::MEDIUM),
            false => self,
        }
    }
}

impl<T: Styled> Weighted for T {}

#[cfg(test)]
mod tests {
    use super::*;

    const MEASURES: Measures = Measures {
        gap: 8.,
        padding: 12.,
        between: 12.,
    };

    #[test]
    fn the_panel_holds_the_cells_and_the_lines_of_text() {
        let plain = Chrome {
            typed: false,
            title: false,
        };
        let typed = Chrome {
            typed: true,
            title: false,
        };
        let tiles = Layout::new(Style::Tiles, false, 2, (1800., 1000.), 8.);
        assert_eq!(
            MEASURES.panel(&tiles, plain),
            (2. * 136. + 8. + 24., 112. + 24.)
        );
        // A line of text needs room of its own, and is never squeezed.
        assert_eq!(
            MEASURES.panel(&tiles, typed),
            (LINE_WIDTH + 24., 112. + 24. + NAME_HEIGHT + 12.)
        );
        let pictured = Layout::new(Style::Tiles, true, 2, (1800., 1000.), 8.);
        assert_eq!(
            MEASURES.panel(&pictured, plain),
            (2. * 208. + 8. + 24., 172. + 24.)
        );
    }

    #[test]
    fn the_cells_keep_to_most_of_the_display() {
        let plain = Chrome {
            typed: false,
            title: false,
        };
        let both = Chrome {
            typed: true,
            title: true,
        };
        let (width, height) = MEASURES.room((1920., 1080.), plain);
        assert_eq!(width, (1920. - 24.) * ROOM_SHARE - 24.);
        assert_eq!(height, (1080. - 24.) * ROOM_SHARE - 24.);
        let (_, lower) = MEASURES.room((1920., 1080.), both);
        assert_eq!(lower, height - 2. * (NAME_HEIGHT + 12.));
        assert_eq!(MEASURES.room((10., 10.), both), (0., 0.));
    }

    #[test]
    fn the_configuration_names_the_style_the_scope_and_the_minimized() {
        let config: SwitcherConfig = toml::from_str(
            "style = \"list\"\nscope = \"all\"\nminimized = \"hide\"\npreviews = true",
        )
        .unwrap();
        assert_eq!(config.style, Style::List);
        assert_eq!(config.scope, Scope::All);
        assert_eq!(config.minimized, Minimized::Hide);
        assert!(config.previews);
        let plain = SwitcherConfig::default();
        assert_eq!(
            (plain.style, plain.scope, plain.minimized, plain.previews),
            (Style::Tiles, Scope::Workspace, Minimized::Keep, false)
        );
        assert!(toml::from_str::<SwitcherConfig>("style = \"cover\"").is_err());
        assert!(toml::from_str::<SwitcherConfig>("filter = \"all\"").is_err());
    }

    #[test]
    fn the_action_refuses_what_is_no_step() {
        let action = Switcher::action();
        assert_eq!(action.accepts("switcher", None), Ok(()));
        assert_eq!(action.accepts("switcher", Some("app-next")), Ok(()));
        assert_eq!(
            action.accepts("switcher", Some("up")),
            Err(STEPS.to_owned())
        );
        assert!(action.takes_reserved_keys);
        let keys: Vec<&str> = { action.default_keys.iter() }
            .map(|(press, _)| press.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "alt+tab",
                "alt+shift+tab",
                "alt+backquote",
                "alt+shift+backquote"
            ]
        );
    }
}
