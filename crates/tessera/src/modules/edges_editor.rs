// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The screen edges, as a page of the settings in their section Screen
//! edges, after the Screen Edges page of KDE Plasma: widget
//! `edges-editor`. It draws each display as it stands among the others,
//! in proportion, with its name and its four corners and four edges; a
//! click on one shows what the pointer resting there does, the
//! `[[corner]]` of that spot: the action (with the titles the Shortcuts
//! page gives them) and what it is done with, or a program to start,
//! whether it is for this display or every one, how long the pointer
//! rests there first and how long it waits after it was used. A spot
//! that does nothing gets a `[[corner]]` once an action is chosen for it.
//!
//! What it shows is read from the configuration the shell holds; what is
//! changed is written to the file off the thread of the interface, with
//! the rest of the file and its comments as they were, and the shell
//! takes it over at once. The parts the page of the snap zones draws as
//! well are here, for both pages.

use gpui::{
    AppContext, AsyncApp, Context, Div, Entity, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Stateful, StatefulInteractiveElement,
    Styled, Subscription, WeakEntity, Window, div, px,
};

use super::action_titles;
use super::quick::chip;
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::actions::Actions;
use crate::config::edit::ConfigFile;
use crate::corners::{CornerConfig, DEFAULT_DELAY, LONGEST_DELAY, Spot, command_line, words};
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::{Display, ScreenRect};
use crate::runners::fill;
use crate::shell::{KnownDisplays, Shell};
use crate::theme::Theme;
use crate::ui::{list_title, one_line};

/// The name of the page.
const NAME: &str = "edges-editor";

/// The size of the drawing of the displays, and the room around them in
/// it, in logical pixels.
const CANVAS: (f32, f32) = (520., 200.);
const CANVAS_MARGIN: f32 = 12.;

/// The side of the square of a corner, and how thick the bar of an edge
/// is and how much of the edge it takes, in the drawing.
const CORNER_SIDE: f32 = 16.;
const EDGE_THICKNESS: f32 = 8.;
const EDGE_SHARE: f32 = 0.4;

/// A number of milliseconds of a corner, as a row of the page steps it.
#[derive(Clone, Copy)]
struct Wait {
    key: &'static str,
    /// What the row says, in English.
    label: &'static str,
    /// How much a step changes it.
    step: u32,
    read: fn(&CornerConfig) -> u32,
    set: fn(&mut CornerConfig, u32),
}

/// How long the pointer rests in the corner first.
const DELAY: Wait = Wait {
    key: "edge-delay",
    label: "Rests there first",
    step: 50,
    read: |corner| corner.delay.unwrap_or(DEFAULT_DELAY),
    set: |corner, value| corner.delay = Some(value),
};

/// How long the corner waits after it was used.
const COOLDOWN: Wait = Wait {
    key: "edge-cooldown",
    label: "Waits after a use",
    step: 100,
    read: |corner| corner.cooldown.unwrap_or(0),
    set: |corner, value| corner.cooldown = Some(value),
};

/// How high the list of actions is at most, in logical pixels.
const LIST_HEIGHT: f32 = 240.;

pub struct EdgesEditorModule;

impl Module for EdgesEditorModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(EdgesEditor::new).into())
    }
}

/// What a spot does: an action, or a program to start.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Deed {
    Action(String),
    Program,
}

/// What `corner` does, with what it is done with as the field shows it;
/// a popup is the action `popup` with its name.
fn deed_of(corner: &CornerConfig) -> Option<(Deed, String)> {
    if let Some(action) = &corner.action {
        return Some((
            Deed::Action(action.clone()),
            corner.arg.clone().unwrap_or_default(),
        ));
    }
    if let Some(popup) = &corner.popup {
        return Some((Deed::Action("popup".to_owned()), popup.clone()));
    }
    { corner.run.as_deref() }.map(|run| (Deed::Program, command_line(run)))
}

/// `corner` doing `deed` with `arg` instead of what it did.
fn with_deed(mut corner: CornerConfig, deed: &Deed, arg: &str) -> CornerConfig {
    corner.action = None;
    corner.arg = None;
    corner.popup = None;
    corner.run = None;
    let arg = arg.trim();
    match deed {
        Deed::Action(name) => {
            corner.action = Some(name.clone());
            corner.arg = (!arg.is_empty()).then(|| arg.to_owned());
        }
        Deed::Program => corner.run = Some(words(arg)),
    }
    corner
}

/// The `[[corner]]` of `corners` that the spot `spot` of the display
/// numbered `display` from 1 uses: one of that display first, then one
/// of every display.
fn entry_for(corners: &[CornerConfig], display: usize, spot: Spot) -> Option<usize> {
    let at = |only: Option<usize>| {
        { corners.iter() }.position(|corner| corner.at == spot && corner.display == only)
    };
    at(Some(display)).or_else(|| at(None))
}

/// A value of milliseconds a step up or down, within what a corner may
/// wait.
fn stepped(value: u32, step: u32, up: bool) -> u32 {
    match up {
        true => (value + step).min(LONGEST_DELAY),
        false => value.saturating_sub(step),
    }
}

/// How the displays fit the drawing: the factor from physical pixels to
/// the drawing, and where the leftmost and topmost of them begin in it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    factor: f32,
    left: f32,
    top: f32,
    origin: (i32, i32),
}

impl Fit {
    /// The fit of `frames` into the drawing, centered in it.
    fn of(frames: &[ScreenRect]) -> Self {
        let left = frames.iter().map(|frame| frame.left).min().unwrap_or(0);
        let top = frames.iter().map(|frame| frame.top).min().unwrap_or(0);
        let right = frames.iter().map(|frame| frame.right).max().unwrap_or(1);
        let bottom = frames.iter().map(|frame| frame.bottom).max().unwrap_or(1);
        let (width, height) = ((right - left).max(1) as f32, (bottom - top).max(1) as f32);
        let factor =
            ((CANVAS.0 - CANVAS_MARGIN * 2.) / width).min((CANVAS.1 - CANVAS_MARGIN * 2.) / height);
        Self {
            factor,
            left: (CANVAS.0 - width * factor) / 2.,
            top: (CANVAS.1 - height * factor) / 2.,
            origin: (left, top),
        }
    }

    /// Where `frame` is in the drawing: left, top, width and height.
    fn place(self, frame: ScreenRect) -> [f32; 4] {
        [
            self.left + (frame.left - self.origin.0) as f32 * self.factor,
            self.top + (frame.top - self.origin.1) as f32 * self.factor,
            (frame.right - frame.left) as f32 * self.factor,
            (frame.bottom - frame.top) as f32 * self.factor,
        ]
    }
}

/// Where the mark of `spot` lies in a display drawn `width` by `height`:
/// left, top, width and height.
fn mark_of(spot: Spot, width: f32, height: f32) -> [f32; 4] {
    let (side, thick) = (CORNER_SIDE, EDGE_THICKNESS);
    let (along_x, along_y) = (width * EDGE_SHARE, height * EDGE_SHARE);
    match spot {
        Spot::TopLeft => [0., 0., side, side],
        Spot::TopRight => [width - side, 0., side, side],
        Spot::BottomLeft => [0., height - side, side, side],
        Spot::BottomRight => [width - side, height - side, side, side],
        Spot::Top => [(width - along_x) / 2., 0., along_x, thick],
        Spot::Bottom => [(width - along_x) / 2., height - thick, along_x, thick],
        Spot::Left => [0., (height - along_y) / 2., thick, along_y],
        Spot::Right => [width - thick, (height - along_y) / 2., thick, along_y],
    }
}

/// A card of a page of the settings, with its title.
pub(super) fn page_card(theme: &Theme, title: impl Into<SharedString>) -> Div {
    div()
        .p(theme.spacing.medium)
        .flex()
        .flex_col()
        .gap(theme.spacing.small + theme.spacing.tiny)
        .rounded(theme.shape.large)
        .bg(theme.colors.surface_container_highest)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.primary)
                .child(one_line(title)),
        )
}

/// A field framed so that it stands out from the card.
pub(super) fn framed(theme: &Theme, field: &Entity<TextField>) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .rounded(theme.shape.full)
        .border_1()
        .border_color(theme.colors.outline)
        .child(field.clone())
}

/// A page of the settings that writes the configuration file and shows
/// why a change was not written.
pub(super) trait ConfigPage: Sized + 'static {
    fn feedback(&mut self) -> &mut Feedback;

    /// Shows `failure`, or that nothing failed.
    fn fail(&mut self, failure: Option<SharedString>);

    /// A chip of the page under `key`, chosen or not, that does `act`.
    fn button(
        &mut self,
        key: (&'static str, usize),
        label: impl Into<SharedString>,
        chosen: bool,
        act: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = *cx.global::<Theme>();
        let state = self.feedback().state(key, window);
        let element = chip(&theme, key, label, chosen, state)
            .on_click(cx.listener(move |this, _, _, cx| act(this, cx)));
        Feedback::track(element, key, cx, |this: &mut Self| this.feedback())
    }

    /// Makes `change` to the file away from the thread of the interface
    /// and has the shell take it over; a change that cannot be made, as
    /// when the file changed meanwhile, is shown.
    fn write(
        &mut self,
        change: impl FnOnce(&mut ConfigFile) -> bool + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.fail(None);
        let written = cx.background_executor().spawn(async move {
            ConfigFile::change(|file| match change(file) {
                true => Ok(()),
                false => Err("the file has changed in the meantime".to_owned()),
            })
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let written = written.await;
            this.update(cx, |this, cx| {
                match written {
                    Ok(()) => cx.defer(Shell::config_changed),
                    Err(error) => {
                        report!("the settings are not written: {error}");
                        this.fail(Some(error.into()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// The page.
struct EdgesEditor {
    /// The displays, from the left, as they were when the page opened.
    displays: Vec<ScreenRect>,
    /// The display and the spot chosen.
    display: usize,
    spot: Spot,
    /// Whether the list of actions is open.
    choosing: bool,
    /// A deed chosen that waits for what it is done with before it is
    /// written.
    pending: Option<Deed>,
    filter: Entity<TextField>,
    arg: Entity<TextField>,
    /// What the field of the argument was filled from: the entry, the
    /// display and the spot.
    filled_for: Option<(Option<usize>, usize, Spot)>,
    failure: Option<SharedString>,
    feedback: Feedback,
    _filtering: Subscription,
}

impl ConfigPage for EdgesEditor {
    fn feedback(&mut self) -> &mut Feedback {
        &mut self.feedback
    }

    fn fail(&mut self, failure: Option<SharedString>) {
        self.failure = failure;
    }
}

impl EdgesEditor {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let language = Language::of(cx);
        let filter = cx.new(|cx| TextField::new(language.say("Search actions").into(), cx));
        let arg = cx.new(|cx| TextField::new(language.say("What it is done with").into(), cx));
        // The list follows what is typed into the field above it.
        let filtering = cx.observe(&filter, |_, _, cx| cx.notify());
        let displays = Display::all()
            .into_iter()
            .map(|display| display.frame)
            .collect();
        Self {
            displays,
            display: 0,
            spot: Spot::TopLeft,
            choosing: false,
            pending: None,
            filter,
            arg,
            filled_for: None,
            failure: None,
            feedback: Feedback::new(motion),
            _filtering: filtering,
        }
    }

    /// What the display at `index` is called.
    fn display_name(index: usize, cx: &Context<Self>) -> String {
        let known = KnownDisplays::all(cx).iter();
        let named = { known }.find(|name| name.position == index + 1);
        match named {
            Some(name) => name.friendly.clone(),
            None => fill(
                Language::of(cx).say("Display {}"),
                &[&(index + 1).to_string()],
            ),
        }
    }

    /// The entry the chosen spot uses, with the corner it holds.
    fn entry(&self, cx: &Context<Self>) -> Option<(usize, CornerConfig)> {
        let corners = &Shell::config(cx).corners;
        let index = entry_for(corners, self.display + 1, self.spot)?;
        Some((index, corners[index].clone()))
    }

    fn select(&mut self, display: usize, spot: Spot, cx: &mut Context<Self>) {
        self.display = display;
        self.spot = spot;
        self.choosing = false;
        self.pending = None;
        self.failure = None;
        cx.notify();
    }

    /// Why `deed` cannot be done with `arg`, if it cannot.
    fn refusal(deed: &Deed, arg: &str, cx: &Context<Self>) -> Option<String> {
        let arg = arg.trim();
        match deed {
            Deed::Action(name) => {
                let arg = (!arg.is_empty()).then_some(arg);
                cx.global::<Actions>().check(name, arg).err()
            }
            Deed::Program if words(arg).is_empty() => Some(
                Language::of(cx)
                    .say("Type the program to start and its arguments.")
                    .to_owned(),
            ),
            Deed::Program => None,
        }
    }

    /// Has the chosen spot do `deed`, now if it needs nothing more, or
    /// once what it is done with is typed.
    fn choose(&mut self, deed: Deed, window: &mut Window, cx: &mut Context<Self>) {
        self.choosing = false;
        let current = self.entry(cx).and_then(|(_, corner)| deed_of(&corner));
        if current.as_ref().is_none_or(|(had, _)| *had != deed) {
            self.arg.update(cx, |field, cx| field.clear(cx));
        }
        let arg = self.arg.read(cx).text();
        match Self::refusal(&deed, &arg, cx) {
            None => self.commit(deed, &arg, cx),
            Some(_) => {
                self.pending = Some(deed);
                self.arg.read(cx).focus(window);
                cx.notify();
            }
        }
    }

    /// Writes what the field says the deed waiting, or the one the spot
    /// has, is done with.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let deed = { self.pending.clone() }.or_else(|| {
            self.entry(cx)
                .and_then(|(_, corner)| deed_of(&corner))
                .map(|(deed, _)| deed)
        });
        let Some(deed) = deed else {
            return;
        };
        let arg = self.arg.read(cx).text();
        if let Some(refusal) = Self::refusal(&deed, &arg, cx) {
            self.failure = Some(refusal.into());
            cx.notify();
            return;
        }
        self.commit(deed, &arg, cx);
    }

    /// Writes the chosen spot doing `deed` with `arg`: into its entry, or
    /// as a new one, for this display where there are more.
    fn commit(&mut self, deed: Deed, arg: &str, cx: &mut Context<Self>) {
        self.pending = None;
        let entry = self.entry(cx);
        let several = self.displays.len() > 1;
        let corner = match &entry {
            Some((_, corner)) => with_deed(corner.clone(), &deed, arg),
            None => {
                let display = several.then_some(self.display + 1);
                with_deed(CornerConfig::new(self.spot, display, "", None), &deed, arg)
            }
        };
        let table = corner.to_table();
        match entry {
            Some((index, _)) => self.write(move |file| file.set_corner(index, &table), cx),
            None => self.write(move |file| file.add_corner(&table), cx),
        }
    }

    /// Changes the entry of the chosen spot as `change` does.
    fn change(&mut self, change: impl FnOnce(&mut CornerConfig), cx: &mut Context<Self>) {
        let Some((index, mut corner)) = self.entry(cx) else {
            return;
        };
        change(&mut corner);
        let table = corner.to_table();
        self.write(move |file| file.set_corner(index, &table), cx);
    }

    fn remove(&mut self, cx: &mut Context<Self>) {
        if let Some((index, _)) = self.entry(cx) {
            self.pending = None;
            self.write(move |file| file.remove_corner(index), cx);
        }
    }

    /// What the deed is called.
    fn deed_title(deed: &Deed, cx: &Context<Self>) -> String {
        let language = Language::of(cx);
        match deed {
            Deed::Program => language.say("Run a program").to_owned(),
            Deed::Action(name) => {
                match { action_titles().iter() }.find(|title| title.name == *name) {
                    Some(title) => language.translate(&title.title),
                    None => name.clone(),
                }
            }
        }
    }

    /// The drawing of the displays with their corners and edges.
    fn canvas(&mut self, theme: &Theme, corners: &[CornerConfig], cx: &mut Context<Self>) -> Div {
        let fit = Fit::of(&self.displays);
        let mut canvas = div()
            .flex_none()
            .relative()
            .w(px(CANVAS.0))
            .h(px(CANVAS.1))
            .mx_auto()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container);
        for (display, &frame) in self.displays.iter().enumerate() {
            let [left, top, width, height] = fit.place(frame);
            let mut block = div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width))
                .h(px(height))
                .rounded(theme.shape.small)
                .border_1()
                .border_color(theme.colors.outline)
                .bg(theme.colors.surface_container_highest)
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(
                    div()
                        .max_w(px((width - CORNER_SIDE * 2.).max(0.)))
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(Self::display_name(display, cx))),
                );
            for (place, spot) in Spot::ALL.into_iter().enumerate() {
                let [x, y, w, h] = mark_of(spot, width, height);
                let used = entry_for(corners, display + 1, spot).is_some();
                let chosen = display == self.display && spot == self.spot;
                let ground = match (chosen, used) {
                    (true, _) => theme.colors.primary,
                    (false, true) => theme.colors.primary.opacity(0.55),
                    (false, false) => theme.colors.outline.opacity(0.35),
                };
                let lit = theme.colors.primary.opacity(0.8);
                block = block.child(
                    div()
                        .id(("edge-spot", display * Spot::ALL.len() + place))
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .h(px(h))
                        .rounded(theme.shape.small)
                        .bg(ground)
                        .hover(move |style| style.bg(lit))
                        .cursor_pointer()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select(display, spot, cx)),
                        ),
                );
            }
            canvas = canvas.child(block);
        }
        canvas
    }

    /// The list of the actions there are, as the field above it narrows
    /// them, to choose what the spot does.
    fn action_list(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let wanted = self.filter.read(cx).text().trim().to_lowercase();
        let language = Language::of(cx);
        let names = cx.global::<Actions>().names();
        let deeds = std::iter::once(Deed::Program).chain(names.into_iter().map(Deed::Action));
        let shown: Vec<(Deed, String)> = deeds
            .map(|deed| {
                let title = Self::deed_title(&deed, cx);
                (deed, title)
            })
            .filter(|(deed, title)| {
                let name = match deed {
                    Deed::Action(name) => name.as_str(),
                    Deed::Program => "",
                };
                wanted.is_empty()
                    || title.to_lowercase().contains(&wanted)
                    || name.to_lowercase().contains(&wanted)
            })
            .collect();
        let mut list = div()
            .id("edge-actions")
            .flex()
            .flex_col()
            .gap(px(2.))
            .max_h(px(LIST_HEIGHT))
            .overflow_y_scroll();
        for (index, (deed, title)) in shown.into_iter().enumerate() {
            let key = ("edge-action", index);
            let state = self.feedback.state(key, window);
            let name = match &deed {
                Deed::Action(name) if *name != title => Some(name.clone()),
                _ => None,
            };
            let row = div()
                .id(key)
                .flex_none()
                .px(theme.spacing.medium)
                .py(theme.spacing.tiny)
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .rounded(theme.shape.medium)
                .bg(crate::ui::state_layer(
                    theme,
                    theme.colors.surface_container,
                    state,
                ))
                .child(div().flex_1().min_w_0().child(one_line(title)))
                .children(name.map(|name| {
                    div()
                        .flex_none()
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(name))
                }))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.choose(deed.clone(), window, cx)),
                );
            list = list.child(Feedback::track(row, key, cx, |this: &mut Self| {
                &mut this.feedback
            }));
        }
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(framed(theme, &self.filter))
            .child(list)
            .child(note_line(theme, language.say("The titles are those of the Shortcuts page; an action without one is shown by its name.")))
    }

    /// A row that steps `wait` of the entry `corner`.
    fn stepper(
        &mut self,
        wait: Wait,
        corner: &CornerConfig,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Wait {
            key,
            label,
            step,
            read,
            set,
        } = wait;
        let value = read(corner);
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut row = div().flex().items_center().gap(theme.spacing.small).child(
            div()
                .min_w(px(160.))
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(language.say(label))),
        );
        for up in [false, true] {
            let next = stepped(value, step, up);
            let glyph = if up { "+" } else { "\u{2212}" };
            let button = self.button(
                (key, usize::from(up)),
                glyph,
                false,
                move |this, cx| this.change(|corner| set(corner, next), cx),
                window,
                cx,
            );
            row = row.child(button);
            if !up {
                let shown = fill(language.say("{} ms"), &[&value.to_string()]);
                row = row.child(div().min_w(px(72.)).flex().justify_center().child(shown));
            }
        }
        row
    }

    /// What the chosen spot does, and its settings.
    fn details(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let title = format!(
            "{} \u{b7} {}",
            language.say(self.spot.title()),
            Self::display_name(self.display, cx)
        );
        let mut card = page_card(&theme, title);
        let entry = self.entry(cx);
        let key = (
            entry.as_ref().map(|(index, _)| *index),
            self.display,
            self.spot,
        );
        if self.filled_for != Some(key) {
            self.filled_for = Some(key);
            let arg = { entry.as_ref() }
                .and_then(|(_, corner)| deed_of(corner))
                .map(|(_, arg)| arg)
                .unwrap_or_default();
            self.arg
                .update(cx, |field, cx| field.set_text(&arg, false, cx));
        }
        let deed = { entry.as_ref() }
            .and_then(|(_, corner)| deed_of(corner))
            .map(|(deed, _)| deed);
        let shown = self.pending.clone().or_else(|| deed.clone());
        match (&entry, &shown) {
            (Some((_, corner)), _) => {
                if self.displays.len() > 1 {
                    let own = corner.display.is_some();
                    let display = self.display + 1;
                    let this_one = self.button(
                        ("edge-scope", 0),
                        language.say("This display"),
                        own,
                        move |this, cx| this.change(|corner| corner.display = Some(display), cx),
                        window,
                        cx,
                    );
                    let every = self.button(
                        ("edge-scope", 1),
                        language.say("All displays"),
                        !own,
                        |this, cx| this.change(|corner| corner.display = None, cx),
                        window,
                        cx,
                    );
                    card = card.child(chip_row(&theme).child(this_one).child(every));
                }
            }
            (None, None) => {
                card = card.child(note_line(&theme, language.say("Nothing happens here.")));
            }
            (None, Some(_)) => {}
        }
        card = card.child(list_title(&theme, language.say("Action")));
        let label = match &shown {
            Some(deed) => Self::deed_title(deed, cx),
            None => language.say("Choose an action").to_owned(),
        };
        let choosing = self.choosing;
        let toggle = self.button(
            ("edge-choose", 0),
            label,
            choosing,
            |this, cx| {
                this.choosing = !this.choosing;
                cx.notify();
            },
            window,
            cx,
        );
        card = card.child(chip_row(&theme).child(toggle));
        if choosing {
            card = card.child(self.action_list(&theme, window, cx));
        }
        if let Some(deed) = &shown {
            let described = match deed {
                Deed::Program => Some(
                    language
                        .say("The program and its arguments, a part with spaces in double quotes.")
                        .to_owned(),
                ),
                Deed::Action(name) => { action_titles().iter() }
                    .find(|title| title.name == *name)
                    .and_then(|title| title.arg.as_deref())
                    .map(|arg| language.translate(arg)),
            };
            if let Some(described) = described {
                card = card.child(note_line(&theme, described));
            }
            let apply = self.button(
                ("edge-apply", 0),
                language.say("Apply"),
                false,
                |this, cx| this.apply(cx),
                window,
                cx,
            );
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "enter" {
                            this.apply(cx);
                        }
                    }))
                    .child(framed(&theme, &self.arg))
                    .child(apply),
            );
        }
        if let Some((_, corner)) = &entry {
            card = card
                .child(self.stepper(DELAY, corner, window, cx))
                .child(self.stepper(COOLDOWN, corner, window, cx));
            let remove = self.button(
                ("edge-remove", 0),
                language.say("Remove"),
                false,
                |this, cx| this.remove(cx),
                window,
                cx,
            );
            card = card.child(chip_row(&theme).child(remove));
        }
        if let Some(failure) = &self.failure {
            card = card.child(note_line(&theme, failure.clone()));
        }
        card
    }
}

impl Render for EdgesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let corners = Shell::config(cx).corners.clone();
        let mut overview = page_card(&theme, language.say("Screen edges")).child(note_line(
            &theme,
            language.say("Choose a corner or an edge of a display to say what the pointer resting there does. Nothing is done while a button is held or a window fills the display."),
        ));
        if self.displays.is_empty() {
            overview = overview.child(note_line(&theme, language.say("Windows names no display.")));
        } else {
            overview = overview.child(self.canvas(&theme, &corners, cx));
        }
        let mut page = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .child(overview);
        if !self.displays.is_empty() {
            self.display = self.display.min(self.displays.len() - 1);
            page = page.child(self.details(window, cx));
        }
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner(text: &str) -> CornerConfig {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_spot_uses_the_entry_of_its_display_before_one_of_every_display() {
        let corners = [
            corner("at = \"top-left\"\npopup = \"a\""),
            corner("at = \"top-left\"\ndisplay = 2\npopup = \"b\""),
            corner("at = \"right\"\ndisplay = 1\npopup = \"c\""),
        ];
        assert_eq!(entry_for(&corners, 1, Spot::TopLeft), Some(0));
        assert_eq!(entry_for(&corners, 2, Spot::TopLeft), Some(1));
        assert_eq!(entry_for(&corners, 1, Spot::Right), Some(2));
        assert_eq!(entry_for(&corners, 2, Spot::Right), None);
        assert_eq!(entry_for(&corners, 1, Spot::Bottom), None);
    }

    #[test]
    fn a_deed_is_read_and_written_in_one_of_three_ways() {
        let popup = corner("at = \"top\"\npopup = \"launcher\"\ndelay = 200");
        assert_eq!(
            deed_of(&popup),
            Some((Deed::Action("popup".to_owned()), "launcher".to_owned()))
        );
        let program = with_deed(popup.clone(), &Deed::Program, "wt.exe -d \"C:\\A B\"");
        assert_eq!(program.popup, None);
        assert_eq!(program.delay, Some(200));
        assert_eq!(
            deed_of(&program),
            Some((Deed::Program, "wt.exe -d \"C:\\A B\"".to_owned()))
        );
        let action = with_deed(program, &Deed::Action("taskview".to_owned()), "  ");
        assert_eq!(action.run, None);
        assert_eq!(action.arg, None);
        assert_eq!(
            deed_of(&action),
            Some((Deed::Action("taskview".to_owned()), String::new()))
        );
    }

    #[test]
    fn the_displays_fit_the_drawing_and_keep_their_proportions() {
        let left = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let right = ScreenRect {
            left: 1920,
            top: -200,
            right: 3360,
            bottom: 2360,
        };
        let fit = Fit::of(&[left, right]);
        let [x, y, width, height] = fit.place(left);
        let [x2, y2, width2, height2] = fit.place(right);
        assert!(x >= CANVAS_MARGIN - 0.01 && y2 >= CANVAS_MARGIN - 0.01);
        assert!(x2 + width2 <= CANVAS.0 - CANVAS_MARGIN + 0.01);
        assert!(y2 + height2 <= CANVAS.1 - CANVAS_MARGIN + 0.01);
        assert!((width / height - 1920. / 1080.).abs() < 0.01);
        assert!((x + width - x2).abs() < 0.01);
        assert!(y > y2);
        assert_eq!(stepped(4990, 50, true), LONGEST_DELAY);
        assert_eq!(stepped(20, 50, false), 0);
        let [_, top, _, thick] = mark_of(Spot::Bottom, 200., 100.);
        assert_eq!(top + thick, 100.);
    }
}
