// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The wallpaper by hand. Widget `wallpaper`: a click has every display
//! show the next picture of its list at once, a right click the one
//! before. Action `wallpaper` does the same, with `next` (or no `arg`) and
//! `previous`. Either begins the interval anew; a display with one
//! picture, a video, a color or the wallpaper of Windows keeps it.

use gpui::{
    AppContext, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window,
};

use crate::actions::{Action, Actions};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};
use crate::wallpaper::Wallpaper;

const ICON_PICTURE: &str = "\u{e91b}";

pub struct WallpaperModule;

impl Module for WallpaperModule {
    fn name(&self) -> &'static str {
        "wallpaper"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("wallpaper", |_, cx| {
            let motion = cx.global::<Theme>().motion;
            cx.new(|_| WallpaperButton {
                interaction: Interaction::new(&motion),
                pressed: Spring::new(motion.effects, 0.),
            })
            .into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let step =
            Action::new(|arg, cx| Wallpaper::go_on(steps(arg), cx)).checking(|arg| match arg {
                None | Some("next" | "previous") => Ok(()),
                _ => Err("`wallpaper` takes `next` or `previous`".to_owned()),
            });
        actions.register("wallpaper", step)
    }
}

/// How many pictures the action `wallpaper` goes on by with `arg`.
fn steps(arg: Option<&str>) -> isize {
    if arg == Some("previous") { -1 } else { 1 }
}

/// A click goes on to the next picture, a right click back to the one
/// before.
struct WallpaperButton {
    interaction: Interaction,
    /// 1 at the moment of a click, then back to 0.
    pressed: Spring,
}

impl WallpaperButton {
    fn go_on(&mut self, by: isize, cx: &mut Context<Self>) {
        Wallpaper::go_on(by, cx);
        self.pressed.jump_to(1.);
        self.pressed.animate_to(0.);
        cx.notify();
    }
}

impl Render for WallpaperButton {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let pressed = self.pressed.value(window).clamp(0., 1.);
        let state = self.interaction.state(window);
        let ground = mix(
            theme.on_bar(colors.surface_container),
            colors.secondary_container,
            pressed,
        );
        let button = pill(&theme)
            .id("wallpaper")
            .bg(state_layer(&theme, ground, state))
            .text_color(colors.on_surface)
            .on_click(cx.listener(|this, _, _, cx| this.go_on(1, cx)))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| this.go_on(-1, cx)),
            )
            .child(icon(&theme, ICON_PICTURE));
        Interaction::track(button, cx, |this: &mut Self| Some(&mut this.interaction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_wallpaper_goes_on_or_back() {
        let mut actions = Actions::default();
        WallpaperModule.actions(&mut actions).unwrap();
        for arg in [None, Some("next"), Some("previous")] {
            assert_eq!(actions.check("wallpaper", arg), Ok(()));
        }
        assert!(actions.check("wallpaper", Some("random")).is_err());
        assert!(!actions.takes_reserved_keys("wallpaper"));
        assert_eq!(steps(None), 1);
        assert_eq!(steps(Some("next")), 1);
        assert_eq!(steps(Some("previous")), -1);
    }
}
