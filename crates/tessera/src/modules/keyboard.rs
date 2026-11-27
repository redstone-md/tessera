// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `keyboard` widget: the language the window in front is typed in,
//! and a click that asks that window for the next one.
//!
//! The widget `keyboard-list`, in the popup of the same name, lists the
//! keyboard layouts with the names Windows gives them, the one in use
//! marked; choosing one asks the window that was in front for it. The
//! action `input-language` asks the window in front for the next layout
//! (Win+Space where the keys of Windows are in use) or for the one before
//! (`previous`, Win+Shift+Space); where a window of Tessera is in front,
//! as the launcher is while its search field is typed into, Tessera takes
//! on that layout itself. `list` opens the list. The switcher Windows
//! shows while Win+Space is held is part of Explorer; in its place the
//! layout flyout lists the layouts with the one switched to lit, where
//! `[flyout]` wants layouts shown. Ctrl+Shift and Alt+Shift are switched
//! by Windows itself and stay with it.
//!
//! Where the window in front is typed in a Japanese, Korean or Chinese
//! layout, the widget shows the mode of its input method beside the
//! letters, as the input indicator of Explorer does (A, あ, カ, 가, 中, 英),
//! read on the background executor while the widget is shown; a click on
//! the mode switches the input method on or off.
//!
//! The widget `touch-keyboard` is the button of the taskbar of Explorer
//! that shows and hides the touch keyboard of Windows.

use std::time::{Duration, Instant};

use gpui::{
    App, AppContext, AsyncApp, Context, FontWeight, Global, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::flyout::{Flyouts, with_names};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::platform::{ImeState, InputLanguage, InputName, OwnInput, TouchKeyboard, Typing};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{Chooser, Pick, choice_row_of, icon, one_line, pill, state_layer};

const ICON_KEYBOARD: &str = "\u{e765}";
const ICON_TOUCH_KEYBOARD: &str = "\u{e92e}";
const ICON_CHECK: &str = "\u{e73e}";

/// The name of the popup of the layouts and of its widget.
const LIST: &str = "keyboard-list";

/// The name of the action.
const ACTION: &str = "input-language";

/// How high a row of the list is: a name over a note.
const LAYOUT_ROW: f32 = 52.;

/// How often the language is looked at: Windows announces a change only to
/// the window it happens in.
const CHECK_INTERVAL: Duration = Duration::from_millis(400);

/// How long a layout asked for is taken to be the one of its window:
/// Windows answers the request a moment later, and a second press in
/// between goes on from the layout asked for, not from the one before.
const ASKED_FOR: Duration = Duration::from_millis(500);

pub struct KeyboardModule;

impl Module for KeyboardModule {
    fn name(&self) -> &'static str {
        "keyboard"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("keyboard", |_, cx| cx.new(Keyboard::new).into())?;
        widgets.register("touch-keyboard", |_, cx| {
            let interaction = Interaction::new(&cx.global::<Theme>().motion);
            cx.new(|_| TouchKeyboardButton { interaction }).into()
        })?;
        widgets.register_panel(LIST, |_, cx| {
            let typing = TypedInto::window(cx);
            cx.new(|cx| LayoutList::new(typing, cx)).into()
        })?;
        widgets.register_popup(PopupConfig {
            name: LIST.to_owned(),
            place: Default::default(),
            width: 340.,
            height: 360.,
            content: vec![WidgetSpec {
                widget: LIST.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Explorer keeps Win+Space to itself while it runs.
        let action = Action::new(input_language)
            .taking_reserved_keys()
            .checking(|arg| match arg {
                None | Some("next" | "previous" | "list") => Ok(()),
                Some(_) => Err(format!(
                    "`{ACTION}` takes `next`, `previous`, `list` or no `arg`"
                )),
            })
            .by_default("win+space", None)
            .by_default("win+shift+space", Some("previous"));
        actions.register(ACTION, action)
    }
}

/// The window last seen in front that is typed into and is no window of
/// Tessera: the one a choice of the list is for, once a popup of Tessera
/// is in front.
#[derive(Default)]
struct TypedInto {
    typing: Option<Typing>,
    /// When the layout of `typing` was asked for, if it was.
    asked: Option<Instant>,
}

impl Global for TypedInto {}

impl TypedInto {
    /// The window in front, or where one of Tessera is, the one seen
    /// before it.
    fn window(cx: &mut App) -> Option<Typing> {
        Self::seen(Typing::in_front(), cx);
        cx.default_global::<Self>().typing
    }

    /// Takes in the window seen in front. While a layout asked for this
    /// very window may still be on its way, the one asked for stands.
    fn seen(typing: Option<Typing>, cx: &mut App) {
        let Some(typing) = typing else {
            return;
        };
        let this = cx.default_global::<Self>();
        let waiting = this
            .asked
            .is_some_and(|asked| may_be_unanswered(asked, Instant::now()));
        if waiting && this.typing.is_some_and(|known| same_window(known, typing)) {
            return;
        }
        this.typing = Some(typing);
    }

    /// Remembers that `typing` was asked for `language` just now.
    fn asked(mut typing: Typing, language: InputLanguage, cx: &mut App) {
        typing.language = language;
        let this = cx.default_global::<Self>();
        (this.typing, this.asked) = (Some(typing), Some(Instant::now()));
    }
}

/// Whether a window asked for a layout at `asked` may not have answered
/// by `now`.
fn may_be_unanswered(asked: Instant, now: Instant) -> bool {
    now.saturating_duration_since(asked) < ASKED_FOR
}

/// Whether `one` and `other` are the same window, whatever their layouts.
fn same_window(one: Typing, mut other: Typing) -> bool {
    other.language = one.language;
    one == other
}

/// The layout after `language` among `languages`, or before it when
/// `back`, going round at the ends.
fn step(language: InputLanguage, languages: &[InputLanguage], back: bool) -> Option<InputLanguage> {
    if back {
        let reversed: Vec<InputLanguage> = languages.iter().rev().copied().collect();
        language.next_of(&reversed)
    } else {
        language.next_of(languages)
    }
}

/// Asks `typing` for `language`, and remembers it was asked.
fn switch(typing: Typing, language: InputLanguage, cx: &mut App) {
    match typing.switch_to(language) {
        Ok(()) => TypedInto::asked(typing, language, cx),
        Err(error) => report!("the input language was not switched: {error:#}"),
    }
}

/// Asks `typing` for the layout after its own, or before it when `back`,
/// and gives the layouts with the one asked for.
fn switch_next(
    typing: Typing,
    back: bool,
    cx: &mut App,
) -> Option<(Vec<InputLanguage>, InputLanguage)> {
    let installed = InputLanguage::installed();
    let next = step(typing.language, &installed, back)?;
    switch(typing, next, cx);
    Some((installed, next))
}

/// Does the action `input-language` with `arg`: the next or the previous
/// layout go to the window in front, which may be one of Tessera.
fn input_language(arg: Option<&str>, cx: &mut App) {
    if arg == Some("list") {
        Shell::toggle_popup(LIST, cx);
        return;
    }
    let back = arg == Some("previous");
    if OwnInput::in_front() {
        if let Err(error) = OwnInput::step(back) {
            report!("the input language was not switched: {error:#}");
        }
        return;
    }
    // The list of the layouts shows while the keys are held, as the one
    // of Windows does.
    if let Some((installed, next)) =
        TypedInto::window(cx).and_then(|typing| switch_next(typing, back, cx))
    {
        Flyouts::show_layouts(installed, next, cx);
    }
}

/// Shows the language of the window that was last seen in front, and the
/// mode of its input method.
struct Keyboard {
    typing: Option<Typing>,
    /// The input method of that window, as last read.
    ime: Option<ImeState>,
    interaction: Interaction,
    /// Pointer feedback of the mode of the input method.
    ime_interaction: Interaction,
    /// 1 at the moment the language changes, then back to 0.
    changed: Spring,
}

impl Keyboard {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                // The input method is asked through the window in front,
                // which may take a moment to answer.
                let ime = cx
                    .background_executor()
                    .spawn(async { ImeState::in_front() });
                let ime = ime.await;
                let looked = this.update(cx, |this, cx| {
                    this.look(cx);
                    this.take_ime(ime, cx);
                });
                if looked.is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            typing: Typing::in_front(),
            ime: None,
            interaction: Interaction::new(&motion),
            ime_interaction: Interaction::new(&motion),
            changed: Spring::new(motion.effects, 0.),
        }
    }

    /// Takes in the input method read; a window of Tessera in front, which
    /// tells none, changes nothing.
    fn take_ime(&mut self, ime: Option<ImeState>, cx: &mut Context<Self>) {
        if ime.is_some() && ime != self.ime {
            self.ime = ime;
            cx.notify();
        }
    }

    /// Switches the input method of the window on or off, and reads it
    /// again once that is done.
    fn toggle_ime(&mut self, cx: &mut Context<Self>) {
        let Some(ime) = self.ime else {
            return;
        };
        // The click shows at once; the next reading tells what came of it.
        self.ime = Some(ime.switched());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let toggled = cx.background_executor().spawn(async move {
                let toggled = ime.toggle();
                (toggled, ImeState::in_front())
            });
            let (toggled, read) = toggled.await;
            if let Err(error) = toggled {
                report!("the input method was not switched: {error:#}");
            }
            this.update(cx, |this, cx| this.take_ime(read, cx)).ok();
        })
        .detach();
    }

    /// Takes in the window in front. A window of Tessera itself in front
    /// changes nothing, so that a click here still finds the other one.
    fn look(&mut self, cx: &mut Context<Self>) {
        let Some(typing) = Typing::in_front() else {
            return;
        };
        TypedInto::seen(Some(typing), cx);
        if self.typing == Some(typing) {
            return;
        }
        let shown = self.typing.map(|typing| typing.language);
        if shown.is_some_and(|shown| shown != typing.language) {
            self.changed.jump_to(1.);
            self.changed.animate_to(0.);
        }
        self.typing = Some(typing);
        cx.notify();
    }

    /// Asks the window for the next layout. A click on a bar leaves the
    /// window in front, which may not have answered the click before.
    fn switch(&self, cx: &mut App) {
        if let Some(typing) = TypedInto::window(cx).or(self.typing) {
            switch_next(typing, false, cx);
        }
    }
}

impl Render for Keyboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let changed = self.changed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            changed,
        );
        let code = self
            .typing
            .map_or_else(|| "--".to_owned(), |typing| typing.language.code());
        let mode = self.ime.and_then(|ime| ime.mode());
        let ime_state = self.ime_interaction.state(window);
        let mode = mode.map(|mode| {
            let glyph = div()
                .id("keyboard-ime")
                .px(theme.spacing.tiny)
                .rounded(theme.shape.small)
                .bg(state_layer(&theme, ground, ime_state))
                .font_weight(FontWeight::MEDIUM)
                .child(mode.glyph())
                .on_click(cx.listener(|this, _, _, cx| {
                    // The click is the input method's, not the layout's.
                    cx.stop_propagation();
                    this.toggle_ime(cx);
                }));
            Interaction::track(glyph, cx, |this: &mut Self| Some(&mut this.ime_interaction))
        });
        let button = pill(&theme)
            .id("keyboard")
            .bg(state_layer(&theme, ground, state))
            .on_click(cx.listener(|this, _, _, cx| this.switch(cx)))
            .child(icon(&theme, ICON_KEYBOARD))
            .child(div().font_weight(FontWeight::MEDIUM).child(code))
            .children(mode);
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

/// The button that shows and hides the touch keyboard of Windows.
struct TouchKeyboardButton {
    interaction: Interaction,
}

impl Render for TouchKeyboardButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let state = self.interaction.state(window);
        let ground = theme.on_bar(theme.colors.surface_container);
        let button = pill(&theme)
            .id("touch-keyboard")
            .bg(state_layer(&theme, ground, state))
            .on_click(|_, _, _| TouchKeyboard::toggle())
            .child(icon(&theme, ICON_TOUCH_KEYBOARD));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

/// A layout the list offers: what it is and what Windows calls it.
#[derive(Clone, Debug, PartialEq)]
struct Layout {
    language: InputLanguage,
    name: InputName,
}

/// The layouts of Windows with their names, as [`with_names`] pairs them.
fn layouts(languages: Vec<InputLanguage>, names: Vec<InputName>) -> Vec<Layout> {
    { with_names(languages, names).into_iter() }
        .map(|(language, name)| Layout { language, name })
        .collect()
}

/// The layouts to choose from for the window that was in front.
struct LayoutList {
    typing: Option<Typing>,
    /// `None` until the names have been read.
    layouts: Option<Vec<Layout>>,
    chooser: Chooser,
}

impl LayoutList {
    fn new(typing: Option<Typing>, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx
                .background_executor()
                .spawn(async { layouts(InputLanguage::installed(), InputName::installed()) });
            let layouts = read.await;
            this.update(cx, |this, cx| {
                let current = this.typing.map(|typing| typing.language);
                let index = { layouts.iter() }.position(|layout| Some(layout.language) == current);
                this.chooser.chosen = index.unwrap_or(0);
                this.layouts = Some(layouts);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            typing,
            layouts: None,
            chooser: Chooser::new(cx),
        }
    }

    fn count(&self) -> usize {
        self.layouts.as_ref().map_or(0, Vec::len)
    }

    /// Asks the window for the layout `index` and closes the list.
    fn take(&mut self, index: usize, cx: &mut Context<Self>) {
        let layout = { self.layouts.as_ref() }.and_then(|layouts| layouts.get(index));
        if let (Some(typing), Some(layout)) = (self.typing, layout) {
            switch(typing, layout.language, cx);
        }
        cx.defer(Popups::close);
    }

    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match Pick::of(event) {
            Some(Pick::Take) => self.take(self.chooser.chosen, cx),
            Some(Pick::Leave) => cx.defer(Popups::close),
            Some(Pick::Letter(_)) | None => return,
            Some(pick) => self.chooser.pick(pick, self.count()),
        }
        cx.notify();
    }
}

impl Render for LayoutList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        self.chooser.take_focus(window);
        let current = self.typing.map(|typing| typing.language);
        let layouts = self.layouts.clone().unwrap_or_default();
        let mut rows = Vec::with_capacity(layouts.len());
        for (index, layout) in layouts.iter().enumerate() {
            let state = self.chooser.state(index, &theme, window);
            let chosen = index == self.chooser.chosen;
            let code = layout.language.code();
            let title = match layout.name.language.is_empty() {
                true => code.clone(),
                false => layout.name.language.clone(),
            };
            let note = (!layout.name.layout.is_empty()).then(|| {
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(layout.name.layout.clone()))
            });
            let label = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(one_line(title))
                .children(note);
            let row = choice_row_of(&theme, ICON_KEYBOARD, label, None, chosen, state)
                .h(px(LAYOUT_ROW))
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.on_surface_variant)
                        .child(code),
                )
                .children((Some(layout.language) == current).then(|| {
                    icon(&theme, ICON_CHECK)
                        .flex_none()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.primary)
                }))
                .id(("keyboard-layout", index))
                .on_click(cx.listener(move |this, _, _, cx| this.take(index, cx)));
            rows.push(Chooser::track(row, index, cx, |this: &mut Self| {
                &mut this.chooser
            }));
        }
        let note = match (&self.layouts, self.typing) {
            (None, _) => Some(language.say("Reading…")),
            (_, None) => Some(language.say("No window is typed into.")),
            _ => None,
        };
        div()
            .id(LIST)
            .track_focus(self.chooser.focus())
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .pb(theme.spacing.tiny)
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(language.say("Keyboard layout"))),
            )
            .child(
                div()
                    .id("keyboard-layouts")
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.tiny)
                    .overflow_y_scroll()
                    .children(rows),
            )
            .children(note.map(|note| {
                div()
                    .flex_none()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(note))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_goes_either_way_or_opens_the_list() {
        let mut actions = Actions::default();
        KeyboardModule.actions(&mut actions).unwrap();
        for arg in [None, Some("next"), Some("previous"), Some("list")] {
            assert_eq!(actions.check(ACTION, arg), Ok(()));
        }
        assert!(actions.check(ACTION, Some("ru")).is_err());
        let presses: Vec<String> = { actions.default_keys().into_iter() }
            .map(|key| key.press)
            .collect();
        assert_eq!(presses, ["win+space", "win+shift+space"]);
    }

    #[test]
    fn a_layout_asked_for_stands_only_for_a_moment() {
        let asked = Instant::now();
        assert!(may_be_unanswered(asked, asked));
        assert!(may_be_unanswered(asked, asked + ASKED_FOR / 2));
        assert!(!may_be_unanswered(asked, asked + ASKED_FOR));
        assert!(
            may_be_unanswered(asked + ASKED_FOR, asked),
            "a clock that went back"
        );
    }

    #[test]
    fn names_are_paired_with_layouts_only_when_both_lists_agree() {
        let name = |language: &str| InputName {
            language: language.to_owned(),
            layout: String::new(),
        };
        let languages = InputLanguage::installed();
        let names: Vec<InputName> = languages.iter().map(|_| name("x")).collect();
        let paired = layouts(languages.clone(), names);
        assert!(paired.iter().all(|layout| layout.name.language == "x"));
        let unpaired = layouts(languages.clone(), vec![name("x"); languages.len() + 1]);
        assert!(
            unpaired
                .iter()
                .all(|layout| layout.name.language.is_empty())
        );
    }
}
