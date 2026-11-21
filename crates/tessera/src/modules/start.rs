// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The parts of a Start menu besides the search: the pinned applications
//! and the buttons that end the session. Widgets: `apps`, `power`.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Entity, Global, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, WeakEntity, Window, div, img, px, relative,
};

use crate::config::edit::ConfigFile;
use crate::icons::AppIcons;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{PowerAction, Shortcut};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, state_layer};

/// How many pinned applications a row holds unless the layout says.
const DEFAULT_COLUMNS: usize = 5;

/// The side of the icon of a pinned application.
const TILE_ICON: Pixels = px(36.);

/// The height kept for the grid while it is empty, about that of a row of
/// tiles, so that the first pins do not move what is below the grid away
/// from the pointer that pins.
const EMPTY_HEIGHT: Pixels = px(76.);

/// How far below its place a tile starts when it appears.
const TILE_TRAVEL: f32 = 12.;

/// The part of the appearing that the last tile waits before it starts.
const STAGGER: f32 = 0.5;

/// The side of a power button, and the most its question adds to it.
const POWER_BUTTON: Pixels = px(40.);
const QUESTION_WIDTH: Pixels = px(120.);

/// How long a button waits for the click that confirms it.
const CONFIRMATION: Duration = Duration::from_secs(4);

pub struct StartModule;

impl Module for StartModule {
    fn name(&self) -> &'static str {
        "start"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("apps", |context, cx| {
            let pinned = context.texts("pinned").unwrap_or_default();
            let columns = context
                .number("columns")
                .map_or(DEFAULT_COLUMNS, |columns| (columns as usize).clamp(1, 12));
            let pins = Pins::shared(cx);
            pins.update(cx, |pins, cx| pins.show(pinned, cx));
            cx.new(|cx| Apps::new(pins, columns, cx)).into()
        })?;
        widgets.register("power", |context, cx| {
            let named = context.texts("actions");
            let actions = match named {
                Some(names) => names.iter().filter_map(|name| action_named(name)).collect(),
                None => ACTIONS.iter().map(|&(action, ..)| action).collect(),
            };
            cx.new(|_| Power::new(actions)).into()
        })
    }
}

/// `pinned` with `name` taken out if it is there, whatever its case, and
/// appended if it is not.
fn toggled(pinned: &[String], name: &str) -> Vec<String> {
    let same = |pin: &String| pin.to_lowercase() == name.to_lowercase();
    if pinned.iter().any(same) {
        pinned.iter().filter(|pin| !same(pin)).cloned().collect()
    } else {
        let mut pinned = pinned.to_vec();
        pinned.push(name.to_owned());
        pinned
    }
}

/// The applications pinned to the Start menu, by the name of their
/// shortcut. Shared by the `apps` widget and the search, which pins.
pub struct Pins {
    names: Vec<String>,
    /// Whether an `apps` widget has shown them, so that pinning has an
    /// effect to see.
    shown: bool,
}

struct SharedPins(Entity<Pins>);

impl Global for SharedPins {}

impl Pins {
    /// The one instance, created when the first widget asks for it.
    pub fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedPins>() {
            return shared.0.clone();
        }
        let pins = cx.new(|_| Self {
            names: Vec::new(),
            shown: false,
        });
        cx.set_global(SharedPins(pins.clone()));
        pins
    }

    /// Whether pinning shows anywhere.
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    pub fn holds(&self, name: &str) -> bool {
        let name = name.to_lowercase();
        self.names.iter().any(|pin| pin.to_lowercase() == name)
    }

    /// Takes over what the layout pins.
    fn show(&mut self, names: Vec<String>, cx: &mut Context<Self>) {
        self.names = names;
        self.shown = true;
        cx.notify();
    }

    /// Pins the application `name`, or unpins it if it is pinned, in the
    /// configuration file as well.
    pub fn toggle(&mut self, name: &str, cx: &mut Context<Self>) {
        let names = toggled(&self.names, name);
        let written = ConfigFile::open().and_then(|mut file| {
            if !file.set_start_pinned(&names) {
                return Err("no popup holds an `apps` widget to pin to".to_owned());
            }
            file.save()
        });
        match written {
            Ok(()) => {
                self.names = names;
                cx.defer(Shell::config_changed);
            }
            Err(error) => {
                report!("{error}");
                self.shown = false;
            }
        }
        cx.notify();
    }
}

/// The pinned applications as a grid of icons with names.
struct Apps {
    pins: Entity<Pins>,
    shortcuts: Vec<Shortcut>,
    columns: usize,
    /// 0 when the grid is created, 1 once every tile is in place.
    appear: Spring,
    /// Pointer feedback of the tiles, by the name they stand for.
    interactions: HashMap<String, Interaction>,
}

impl Apps {
    fn new(pins: Entity<Pins>, columns: usize, cx: &mut Context<Self>) -> Self {
        cx.observe(&pins, |_, _, cx| cx.notify()).detach();
        let mut appear = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        appear.animate_to(1.);
        Self {
            pins,
            shortcuts: Shortcut::start_menu(),
            columns,
            appear,
            interactions: HashMap::new(),
        }
    }

    fn shortcut(&self, name: &str) -> Option<&Shortcut> {
        let name = name.to_lowercase();
        let named = |shortcut: &&Shortcut| shortcut.name.to_lowercase() == name;
        self.shortcuts.iter().find(named)
    }

    fn tile(
        &mut self,
        index: usize,
        count: usize,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let state = self
            .interactions
            .entry(name.to_owned())
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        // Later tiles start later, and all have arrived at 1.
        let wait = STAGGER * index as f32 / count.max(1) as f32;
        let arrived = ((self.appear.value(window) - wait) / (1. - STAGGER)).clamp(0., 1.);
        let shortcut = self.shortcut(name).cloned();
        let image = shortcut
            .as_ref()
            .and_then(|shortcut| cx.global::<AppIcons>().of_shortcut(shortcut));
        let image = match image {
            Some(image) => img(image).size(TILE_ICON).flex_none().into_any_element(),
            None => div()
                .size(TILE_ICON)
                .flex_none()
                .rounded(theme.shape.small)
                .bg(theme.colors.outline)
                .into_any_element(),
        };
        let (found, unpinned, tracked) = (shortcut.is_some(), name.to_owned(), name.to_owned());
        let tile = div()
            .id(("pinned-app", index))
            .w(relative(1. / self.columns as f32))
            .relative()
            .top(px(TILE_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .py(theme.spacing.small)
            .px(theme.spacing.tiny)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.spacing.tiny)
            .rounded(theme.shape.medium)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            // A pin whose shortcut is gone is shown, but dimmed.
            .text_color(if found {
                theme.colors.on_surface
            } else {
                theme.colors.outline
            })
            .child(image)
            .child(
                // A long name takes a second line and is cut after it.
                div()
                    .w_full()
                    .text_center()
                    .overflow_hidden()
                    .line_clamp(2)
                    .child(SharedString::new(name)),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(shortcut) = &shortcut {
                    shortcut.launch();
                    cx.defer(Popups::close);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.interactions.remove(&unpinned);
                    this.pins.update(cx, |pins, cx| pins.toggle(&unpinned, cx));
                }),
            );
        Interaction::track(tile, cx, move |this: &mut Self| {
            this.interactions.get_mut(&tracked)
        })
        .into_any_element()
    }
}

impl Render for Apps {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let names = self.pins.read(cx).names.clone();
        if names.is_empty() {
            return div()
                .flex_none()
                .h(EMPTY_HEIGHT)
                .flex()
                .items_center()
                .text_color(theme.colors.on_surface_variant)
                // A text right in a row is as wide as its one line.
                .child(div().min_w_0().child(
                    "Nothing is pinned. Point at an application in the list and click its pin.",
                ));
        }
        let mut tiles = Vec::new();
        for (index, name) in names.iter().enumerate() {
            tiles.push(self.tile(index, names.len(), name, window, cx));
        }
        div().flex_none().flex().flex_wrap().children(tiles)
    }
}

/// The actions in the order their buttons stand, each with the name a
/// layout calls it by, its icon and what it asks before it acts.
const ACTIONS: [(PowerAction, &str, &str, &str); 5] = [
    (PowerAction::Lock, "lock", "\u{e72e}", ""),
    (PowerAction::Sleep, "sleep", "\u{e708}", "Sleep?"),
    (PowerAction::SignOut, "sign-out", "\u{f3b1}", "Sign out?"),
    (PowerAction::Restart, "restart", "\u{e777}", "Restart?"),
    (PowerAction::ShutDown, "shut-down", "\u{e7e8}", "Shut down?"),
];

fn action_named(name: &str) -> Option<PowerAction> {
    let known = ACTIONS.iter().find(|(_, known, ..)| *known == name);
    if known.is_none() {
        report!("`{name}` is not something the `power` widget does");
    }
    known.map(|&(action, ..)| action)
}

/// What a click on the button of `pressed` leads to while `armed` waits
/// for its confirmation: the action that waits afterwards, and whether
/// `pressed` is carried out. Only locking, which loses nothing, acts at
/// the first click.
fn press(armed: Option<PowerAction>, pressed: PowerAction) -> (Option<PowerAction>, bool) {
    if pressed == PowerAction::Lock || armed == Some(pressed) {
        (None, true)
    } else {
        (Some(pressed), false)
    }
}

/// A button's pointer feedback, and how far its question has opened.
struct PowerButton {
    interaction: Interaction,
    question: Spring,
}

/// Buttons that lock the session, put the computer to sleep, sign out,
/// restart and shut down.
struct Power {
    actions: Vec<PowerAction>,
    /// The action whose button was clicked once and asks to be clicked
    /// again.
    armed: Option<PowerAction>,
    /// Counts the clicks, so that a wait that was overtaken changes nothing.
    clicks: u64,
    buttons: HashMap<PowerAction, PowerButton>,
}

impl Power {
    fn new(actions: Vec<PowerAction>) -> Self {
        Self {
            actions,
            armed: None,
            clicks: 0,
            buttons: HashMap::new(),
        }
    }

    fn arm(&mut self, armed: Option<PowerAction>, cx: &mut Context<Self>) {
        self.armed = armed;
        for (action, button) in &mut self.buttons {
            let open = Some(*action) == armed;
            button.question.animate_to(if open { 1. } else { 0. });
        }
        cx.notify();
    }

    fn click(&mut self, action: PowerAction, cx: &mut Context<Self>) {
        let (armed, act) = press(self.armed, action);
        self.clicks += 1;
        self.arm(armed, cx);
        if act {
            action.perform();
            cx.defer(Popups::close);
            return;
        }
        let clicks = self.clicks;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(CONFIRMATION).await;
            let forget = |this: &mut Self, cx: &mut Context<Self>| {
                if this.clicks == clicks {
                    this.arm(None, cx);
                }
            };
            this.update(cx, forget).ok();
        })
        .detach();
    }

    fn button(
        &mut self,
        action: PowerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let (_, name, glyph, question) = ACTIONS
            .into_iter()
            .find(|&(known, ..)| known == action)
            .expect("every action is listed");
        let button = self.buttons.entry(action).or_insert_with(|| PowerButton {
            interaction: Interaction::new(&theme.motion),
            question: Spring::new(theme.motion.spatial_fast, 0.),
        });
        let state = button.interaction.state(window);
        let asked = button.question.value(window);
        let armed = self.armed == Some(action);
        let (base, color) = if armed {
            (theme.colors.primary, theme.colors.surface_container)
        } else {
            (
                theme.colors.surface_container_highest,
                theme.colors.on_surface,
            )
        };
        let element = div()
            .id(name)
            .flex_none()
            .h(POWER_BUTTON)
            .min_w(POWER_BUTTON)
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(&theme, base, state))
            .text_color(color)
            .child(
                div()
                    .w(POWER_BUTTON)
                    .flex()
                    .justify_center()
                    .child(icon(&theme, glyph)),
            )
            .child(
                div()
                    .max_w(QUESTION_WIDTH * asked.clamp(0., 1.))
                    .opacity(asked.clamp(0., 1.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(div().pr(theme.spacing.medium).child(question)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.click(action, cx)));
        Interaction::track(element, cx, move |this: &mut Self| {
            let button = this.buttons.get_mut(&action)?;
            Some(&mut button.interaction)
        })
        .into_any_element()
    }
}

impl Render for Power {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let mut buttons = Vec::new();
        for action in self.actions.clone() {
            buttons.push(self.button(action, window, cx));
        }
        div()
            .flex_none()
            .flex()
            .justify_end()
            .gap(theme.spacing.small)
            .children(buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_appends_and_unpinning_ignores_case() {
        let pinned = ["Paint".to_owned(), "Notepad".to_owned()];
        assert_eq!(
            toggled(&pinned, "Calculator"),
            ["Paint", "Notepad", "Calculator"]
        );
        assert_eq!(toggled(&pinned, "paint"), ["Notepad"]);
        assert_eq!(toggled(&[], "Paint"), ["Paint"]);
    }

    #[test]
    fn only_locking_acts_at_the_first_click() {
        use PowerAction::{Lock, Restart, ShutDown};
        assert_eq!(press(None, Lock), (None, true));
        assert_eq!(press(None, ShutDown), (Some(ShutDown), false));
        assert_eq!(press(Some(ShutDown), ShutDown), (None, true));
        // Another button takes the question over instead of acting.
        assert_eq!(press(Some(ShutDown), Restart), (Some(Restart), false));
        assert_eq!(press(Some(Restart), Lock), (None, true));
    }

    #[test]
    fn a_layout_names_the_actions() {
        assert_eq!(action_named("sign-out"), Some(PowerAction::SignOut));
        assert_eq!(action_named("explode"), None);
    }
}
