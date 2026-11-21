// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The `focused` widget: what the window in front is called.

use std::path::Path;
use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, FontWeight, IntoElement, ParentElement, Pixels, Render, Styled,
    WeakEntity, Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{AppWindow, AppWindowId, WindowEvents};
use crate::theme::Theme;
use crate::ui::{one_line, pill};

/// How wide the widget gets at most, in logical pixels, unless the layout
/// says otherwise.
const WIDTH: f32 = 260.;

/// How often it is asked whether something happened to a window.
const CHECK_INTERVAL: Duration = Duration::from_millis(150);

/// After how many quiet checks the window in front is looked at anyway.
const PATIENCE: u32 = 14;

pub struct FocusedModule;

impl Module for FocusedModule {
    fn name(&self) -> &'static str {
        "focused"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("focused", |context, cx| {
            let naming = match context.text("show") {
                None | Some("title") => Naming::Title,
                Some("program") => Naming::Program,
                Some(other) => {
                    report!("`focused` shows \"title\" or \"program\", not `{other}`");
                    Naming::Title
                }
            };
            let width = context.number("width").map_or(WIDTH, |width| width as f32);
            cx.new(|cx| Focused::new(naming, px(width.max(0.)), cx))
                .into()
        })
    }
}

/// What of the window in front is shown.
#[derive(Clone, Copy)]
enum Naming {
    /// The title of the window.
    Title,
    /// The name of its program file, without the ending.
    Program,
}

impl Naming {
    /// What `window` is called, or nothing where it has no name.
    fn of(self, window: &AppWindow) -> Option<String> {
        let name = match self {
            Self::Title => window.id.title(),
            Self::Program => program_name(window.executable.as_deref()?)?,
        };
        Some(name).filter(|name| !name.trim().is_empty())
    }
}

/// The name of the program file at `path` without its ending, with the
/// first letter a capital one, as programs are spoken of.
fn program_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let mut letters = stem.chars();
    let first = letters.next()?;
    Some(first.to_uppercase().chain(letters).collect())
}

/// Shows the name of the application window in front; shrinks away while
/// the desktop or another window without a place in the dock is in front.
struct Focused {
    naming: Naming,
    width: Pixels,
    /// The name shown last, kept while the widget leaves.
    shown: Option<String>,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// 0 at the moment the name changes, then up to 1.
    arrival: Spring,
}

impl Focused {
    fn new(naming: Naming, width: Pixels, cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let patience = if WindowEvents::watch() { PATIENCE } else { 1 };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut seen = WindowEvents::generation();
            let mut quiet = 0;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let now = WindowEvents::generation();
                quiet += 1;
                if now == seen && quiet < patience {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                (seen, quiet) = (now, 0);
                if this.update(cx, Self::look).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = Self {
            naming,
            width,
            shown: None,
            presence: Spring::new(motion.spatial, 0.),
            arrival: Spring::new(motion.effects, 1.),
        };
        this.look(cx);
        this
    }

    /// Takes in the window in front. A window of Tessera itself in front
    /// changes nothing: a click on a popup is not a change of application.
    fn look(&mut self, cx: &mut Context<Self>) {
        let Some(front) = AppWindowId::foreground() else {
            return;
        };
        if front.process() == std::process::id() {
            return;
        }
        let windows = AppWindow::enumerate();
        let name = windows
            .iter()
            .find(|window| window.id == front)
            .and_then(|window| self.naming.of(window));
        let present = if name.is_some() { 1. } else { 0. };
        if name == self.shown && self.presence.target() == present {
            return;
        }
        self.presence.animate_to(present);
        if name.is_some() {
            if self.shown.is_some() && self.shown != name {
                self.arrival.jump_to(0.);
                self.arrival.animate_to(1.);
            }
            self.shown = name;
        }
        cx.notify();
    }
}

impl Render for Focused {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window);
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let gone = self.presence.is_settled() && presence <= 0.;
        let Some(name) = self.shown.as_ref().filter(|_| !gone) else {
            return div();
        };
        div()
            .overflow_hidden()
            .max_w(self.width * presence.max(0.))
            .opacity(presence.clamp(0., 1.))
            .child(
                pill(&theme)
                    .max_w(self.width)
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .min_w_0()
                            .opacity(arrival)
                            .child(one_line(name.clone())),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_named_after_its_file() {
        let name = |path: &str| program_name(Path::new(path));
        assert_eq!(
            name(r"C:\Program Files\Editor\code.exe").as_deref(),
            Some("Code")
        );
        assert_eq!(name("").as_deref(), None);
    }
}
