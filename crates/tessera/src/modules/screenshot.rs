// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Screenshots from a bar and from the keys. Widgets: `screenshot`, a
//! button of a bar that opens the popup `screenshot`, or with `mode` takes
//! that screenshot at once; and `screenshot-modes`, the list the popup
//! holds: what to take, the seconds to wait first and what to do with it.
//! Action: `screenshot <mode>[,edit|,copy|,save][,delay=N]`, which brings
//! Win+PrtScn (all displays, saved, as Windows does), and where
//! `[screenshot] own_print_screen` is on PrtScn (the region tool) and
//! Alt+PrtScn (the window in front), and where `snip = "tessera"` is
//! Win+Shift+S (the region tool), all where the keys of Windows are in use.

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, Context, Global, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::modules::windows_keys::{SNIP, take_snip};
use crate::popup::{PopupAnchor, Popups};
use crate::screenshot::{
    ICON_SCREENSHOT, LONGEST_DELAY, Mode, Request, ScreenshotConfig, Screenshots, Snip, Then,
};
use crate::theme::Theme;
use crate::ui::{icon, pill};

/// The popup the button opens.
const POPUP: &str = "screenshot";

/// How long the popup is given to be gone before a picture is taken
/// without a delay.
const POPUP_GONE: Duration = Duration::from_millis(300);

const ICON_MINUS: &str = "\u{e738}";
const ICON_PLUS: &str = "\u{e710}";

/// The glyph of each mode.
fn glyph(mode: Mode) -> &'static str {
    match mode {
        Mode::All => "\u{e7f4}",
        Mode::Display => "\u{e7f8}",
        Mode::Window => "\u{e737}",
        Mode::UnderPointer => "\u{e7c9}",
        Mode::Region => "\u{f407}",
    }
}

pub struct ScreenshotModule {
    config: ScreenshotConfig,
}

impl ScreenshotModule {
    /// The module for `[screenshot]` as `config` says. It tells the keys of
    /// Windows whether Win+Shift+S is left to the region tool, before the
    /// actions are collected.
    pub fn of(config: &ScreenshotConfig) -> Self {
        take_snip(config.snip == Snip::Tessera);
        Self {
            config: config.clone(),
        }
    }

    /// The action `screenshot` with the keys `config` brings.
    fn action(config: &ScreenshotConfig) -> Action {
        let mut action = Action::new(|arg, cx| match Request::parse(arg) {
            Ok(request) => Screenshots::take(request, cx),
            Err(error) => report!("{error}"),
        })
        .taking_reserved_keys()
        .checking(|arg| Request::parse(arg).map(|_| ()))
        .by_default("win+printscreen", Some("all,save"));
        if config.own_print_screen {
            action = action
                .by_default("printscreen", Some("region"))
                .by_default("alt+printscreen", Some("window"));
        }
        if config.snip == Snip::Tessera {
            action = action.by_default(SNIP, Some("region"));
        }
        action
    }
}

impl Module for ScreenshotModule {
    fn name(&self) -> &'static str {
        "screenshot"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("screenshot", |spec, cx| {
            let button = ShotButton::of(spec);
            cx.new(|_| button).into()
        })?;
        widgets.register_panel("screenshot-modes", |spec, cx| {
            let delay = spec
                .number("delay")
                .map(|delay| delay.clamp(0., LONGEST_DELAY as f64) as u32);
            let then = spec.text("then").and_then(then_named);
            cx.new(|cx| ModeList::new(delay, then, cx)).into()
        })?;
        widgets.register_popup(PopupConfig {
            place: Default::default(),
            name: POPUP.to_owned(),
            width: 320.,
            height: 420.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "screenshot-modes".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        actions.register("screenshot", Self::action(&self.config))
    }
}

/// What `then` names: `edit`, `copy` or `save`.
fn then_named(name: &str) -> Option<Then> {
    match name.trim() {
        "edit" => Some(Then::Edit),
        "copy" => Some(Then::Copy),
        "save" => Some(Then::Save),
        _ => None,
    }
}

/// Takes `request` once the popup it came from is gone.
fn take_after_popup(request: Request, cx: &mut App) {
    Popups::close(cx);
    if request.delay > 0 {
        return Screenshots::take(request, cx);
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(POPUP_GONE).await;
        cx.update(|cx| Screenshots::take(request, cx)).ok();
    })
    .detach();
}

/// A button of a bar.
struct ShotButton {
    /// What it takes at once; without one it opens the popup.
    mode: Option<Mode>,
    delay: u32,
}

impl ShotButton {
    fn of(spec: &WidgetContext) -> Self {
        let mode = { spec.text("mode") }
            .filter(|mode| *mode != "popup")
            .and_then(|mode| Mode::all().find(|known| known.name() == mode));
        let delay = { spec.number("delay") }
            .map_or(0, |delay| delay.clamp(0., LONGEST_DELAY as f64) as u32);
        Self { mode, delay }
    }
}

impl Render for ShotButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        pill(theme)
            .id("screenshot-button")
            .cursor_pointer()
            .child(icon(theme, ICON_SCREENSHOT))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| match this.mode {
                    Some(mode) => {
                        let request = Request {
                            mode,
                            then: None,
                            delay: this.delay,
                        };
                        Screenshots::take(request, cx);
                    }
                    None => match PopupAnchor::at(window, window.mouse_position().x, cx) {
                        Ok(anchor) => Popups::toggle(POPUP, anchor, cx),
                        Err(error) => {
                            report!("the popup of screenshots has nowhere to open: {error:#}")
                        }
                    },
                }),
            )
    }
}

/// What the list chose last, kept while Tessera runs.
#[derive(Clone, Copy, Default)]
struct Chosen {
    delay: Option<u32>,
    then: Option<Then>,
}

impl Global for Chosen {}

/// The modes, the delay and what to do afterwards.
struct ModeList {
    delay: u32,
    then: Then,
}

impl ModeList {
    fn new(delay: Option<u32>, then: Option<Then>, cx: &mut Context<Self>) -> Self {
        let chosen = *cx.default_global::<Chosen>();
        let config = Screenshots::config(cx);
        let default_then = match config.editor_after {
            true => Then::Edit,
            false => Then::Save,
        };
        Self {
            delay: chosen.delay.or(delay).unwrap_or(0),
            then: chosen.then.or(then).unwrap_or(default_then),
        }
    }

    fn remember(&self, cx: &mut Context<Self>) {
        *cx.default_global::<Chosen>() = Chosen {
            delay: Some(self.delay),
            then: Some(self.then),
        };
        cx.notify();
    }

    fn row(
        &self,
        mode: Mode,
        theme: &Theme,
        language: Language,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(("screenshot-mode", mode as usize))
            .h(px(44.))
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .hover(|style| style.bg(theme.colors.secondary_container))
            .cursor_pointer()
            .child(icon(theme, glyph(mode)).text_color(theme.colors.primary))
            .child(div().flex_1().min_w_0().child(language.say(mode.title())))
            .on_click(cx.listener(move |this, _, _, cx| {
                let request = Request {
                    mode,
                    then: Some(this.then),
                    delay: this.delay,
                };
                take_after_popup(request, cx);
            }))
            .into_any_element()
    }

    fn step_button(
        &self,
        id: &'static str,
        glyph: &'static str,
        by: i32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .cursor_pointer()
            .child(icon(theme, glyph))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.delay = (this.delay as i32 + by).clamp(0, LONGEST_DELAY as i32) as u32;
                this.remember(cx);
            }))
            .into_any_element()
    }
}

impl Render for ModeList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let rows: Vec<AnyElement> = { Mode::all() }
            .map(|mode| self.row(mode, &theme, language, cx))
            .collect();
        let seconds = match self.delay {
            0 => language.say("No delay").to_owned(),
            delay => language.say("{} s").replace("{}", &delay.to_string()),
        };
        let delay = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(div().flex_1().child(language.say("Delay")))
            .child(self.step_button("delay-less", ICON_MINUS, -1, &theme, cx))
            .child(div().w(px(72.)).flex().justify_center().child(seconds))
            .child(self.step_button("delay-more", ICON_PLUS, 1, &theme, cx));
        let thens = [
            (Then::Save, "Save"),
            (Then::Copy, "Copy"),
            (Then::Edit, "Edit"),
        ];
        let chips: Vec<AnyElement> = { thens.into_iter() }
            .map(|(then, label)| {
                let chosen = self.then == then;
                div()
                    .id(("screenshot-then", then as usize))
                    .px(theme.spacing.medium)
                    .py(theme.spacing.tiny)
                    .rounded(theme.shape.full)
                    .border_1()
                    .border_color(theme.colors.outline)
                    .when_chosen(chosen, &theme)
                    .cursor_pointer()
                    .child(language.say(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.then = then;
                        this.remember(cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let afterwards = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(div().flex_1().child(language.say("Afterwards")))
            .children(chips);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .children(rows)
            .child(div().h(theme.spacing.small))
            .child(delay)
            .child(afterwards)
    }
}

/// The look of a chip that is chosen.
trait ChipLook {
    fn when_chosen(self, chosen: bool, theme: &Theme) -> Self;
}

impl<E: Styled> ChipLook for E {
    fn when_chosen(self, chosen: bool, theme: &Theme) -> Self {
        match chosen {
            true => self
                .bg(theme.colors.secondary_container)
                .border_color(theme.colors.primary)
                .text_color(theme.colors.primary),
            false => self,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions(config: &ScreenshotConfig) -> Actions {
        let mut actions = Actions::default();
        // Not `of`, which would tell the keys of Windows for every test.
        let module = ScreenshotModule {
            config: config.clone(),
        };
        module.actions(&mut actions).unwrap();
        actions
    }

    fn bound(actions: &Actions, press: &str) -> Option<Option<String>> {
        let keys = actions.default_keys();
        let key = keys.iter().find(|key| key.press == press)?;
        Some(key.arg.clone())
    }

    #[test]
    fn win_printscreen_saves_every_display_as_windows_does() {
        let actions = actions(&ScreenshotConfig::default());
        assert_eq!(
            bound(&actions, "win+printscreen"),
            Some(Some("all,save".to_owned()))
        );
        assert_eq!(bound(&actions, "printscreen"), None);
        assert_eq!(bound(&actions, SNIP), None);
        assert!(actions.takes_reserved_keys("screenshot"));
    }

    #[test]
    fn the_options_bring_the_other_keys() {
        let config = ScreenshotConfig {
            own_print_screen: true,
            snip: Snip::Tessera,
            ..ScreenshotConfig::default()
        };
        let actions = actions(&config);
        assert_eq!(
            bound(&actions, "printscreen"),
            Some(Some("region".to_owned()))
        );
        assert_eq!(
            bound(&actions, "alt+printscreen"),
            Some(Some("window".to_owned()))
        );
        assert_eq!(bound(&actions, SNIP), Some(Some("region".to_owned())));
        for key in actions.default_keys() {
            assert!(
                crate::keys::Press::parse(&key.press).is_some(),
                "{}",
                key.press
            );
        }
    }

    #[test]
    fn the_action_checks_its_argument() {
        let actions = actions(&ScreenshotConfig::default());
        assert_eq!(actions.check("screenshot", None), Ok(()));
        assert_eq!(
            actions.check("screenshot", Some("region,copy,delay=2")),
            Ok(())
        );
        assert!(actions.check("screenshot", Some("now")).is_err());
    }

    #[test]
    fn then_is_named_as_the_action_names_it() {
        assert_eq!(then_named("copy"), Some(Then::Copy));
        assert_eq!(then_named("print"), None);
    }
}
