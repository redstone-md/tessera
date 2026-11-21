// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Menus: a short list of things to do with what was clicked, which opens
//! next to a surface and closes once one is chosen or the user turns away.

use std::rc::Rc;

use gpui::{
    App, AppContext, Bounds, Context, Global, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, WindowHandle,
    div, point, px, size,
};

use crate::motion::{Interaction, Spring};
use crate::platform::{NativeSurface, ScreenEdge};
use crate::popup::{PopupAnchor, floating_panel, floating_window};
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// The width of a menu, in logical pixels.
const WIDTH: f32 = 208.;

/// The height of an entry.
const ROW_HEIGHT: f32 = 36.;

/// Something a menu offers.
#[derive(Clone)]
pub struct MenuEntry {
    /// A character of the theme's icon font.
    glyph: &'static str,
    label: SharedString,
    action: Rc<dyn Fn(&mut App)>,
}

impl MenuEntry {
    pub fn new(
        glyph: &'static str,
        label: impl Into<SharedString>,
        action: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self {
            glyph,
            label: label.into(),
            action: Rc::new(action),
        }
    }
}

/// The open menu.
#[derive(Default)]
pub struct Menus {
    open: Option<WindowHandle<MenuView>>,
}

impl Global for Menus {}

impl Menus {
    /// Opens a menu of `entries` at `anchor`; any other menu closes. A menu
    /// without entries does not open.
    pub fn open(entries: Vec<MenuEntry>, anchor: PopupAnchor, cx: &mut App) {
        Self::close(cx);
        if entries.is_empty() {
            return;
        }
        let theme = *cx.global::<Theme>();
        let padding = theme.spacing.small / px(1.);
        let gap = theme.spacing.tiny / px(1.);
        let (width, height) = extent(entries.len(), padding, gap);
        let frame = anchor.frame(width, height);
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(width), px(height)),
        };
        let edge = anchor.edge;
        let opened = cx.open_window(
            floating_window(rough, anchor.display_id, true),
            |window, cx| {
                let adopted = NativeSurface::of(window).and_then(|native| {
                    native.adopt_as_popup()?;
                    native.place(frame)?;
                    native.reveal(true);
                    Ok(())
                });
                if let Err(error) = adopted {
                    report!("a menu is not where it belongs: {error:#}");
                }
                cx.new(|cx| MenuView::new(entries, edge, window, cx))
            },
        );
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("a menu did not open: {error:#}"),
        }
    }

    /// Closes the open menu, if there is one.
    pub fn close(cx: &mut App) {
        if let Some(window) = cx.global_mut::<Self>().open.take() {
            // A menu that has closed by itself is gone, which is fine.
            window.update(cx, |view, _, cx| view.close(cx)).ok();
        }
    }
}

/// The logical width and height of the panel of a menu of `rows` entries,
/// `gap` apart and `padding` from its border.
fn extent(rows: usize, padding: f32, gap: f32) -> (f32, f32) {
    let rows = rows as f32;
    let height = rows * ROW_HEIGHT + (rows - 1.).max(0.) * gap + padding * 2.;
    (WIDTH, height)
}

/// The root view of a menu window.
pub struct MenuView {
    entries: Vec<MenuEntry>,
    /// The pointer feedback of each entry.
    rows: Vec<Interaction>,
    /// The edge the menu's surface is docked to; it moves in from there.
    edge: ScreenEdge,
    /// 0 while hidden, 1 when fully open.
    visibility: Spring,
    closing: bool,
    /// Whether the window has had keyboard focus; before that, not having
    /// it does not mean the user turned away.
    was_active: bool,
    _activation: Subscription,
}

impl MenuView {
    fn new(
        entries: Vec<MenuEntry>,
        edge: ScreenEdge,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        Self {
            rows: entries.iter().map(|_| Interaction::new(&motion)).collect(),
            entries,
            edge,
            visibility,
            closing: false,
            was_active: false,
            _activation: activation,
        }
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            window.remove_window();
        }
        let rows: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let state = self.rows[index].state(window);
                let action = entry.action.clone();
                let row = div()
                    .id(("menu-entry", index))
                    .h(px(ROW_HEIGHT))
                    .flex_none()
                    .px(theme.spacing.medium)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .rounded(theme.shape.medium)
                    .bg(state_layer(&theme, theme.colors.surface_container, state))
                    .on_click(move |_, _, cx| {
                        action(cx);
                        // This window cannot be told to close while it
                        // handles the click.
                        cx.defer(Menus::close);
                    })
                    .child(
                        icon(&theme, entry.glyph)
                            .text_size(theme.typography.label_size)
                            .text_color(theme.colors.on_surface_variant),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(one_line(entry.label.clone())),
                    );
                Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index))
            })
            .collect();
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel
                .p(theme.spacing.small)
                .gap(theme.spacing.tiny)
                .children(rows)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_is_as_tall_as_its_entries_and_what_is_between_them() {
        assert_eq!(extent(1, 8., 4.), (WIDTH, 52.));
        assert_eq!(extent(3, 8., 4.), (WIDTH, 132.));
    }
}
