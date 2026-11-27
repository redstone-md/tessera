// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Menus: a short list of things to do with what was clicked, which opens
//! next to a surface and closes once one is chosen or the user turns away.
//! An entry can hold a further list, which takes the place of the menu's
//! entries when it is chosen, with a way back at its top.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement,
    Render, RenderImage, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    WindowHandle, div, img, point, px, size,
};

use crate::i18n::Language;
use crate::motion::{Interaction, Spring};
use crate::platform::{NativeSurface, ScreenEdge};
use crate::popup::{PopupAnchor, floating_panel, open_floating, retire_floating};
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// The width of a menu, in logical pixels.
const WIDTH: f32 = 208.;

/// The height of an entry.
const ROW_HEIGHT: f32 = 36.;

/// How many entries a menu shows at once; the others are scrolled to.
const ROWS_SHOWN: usize = 12;

/// What stands at the end of an entry that holds a further list, and at
/// the start of the entry that goes back from one.
const ICON_MORE: &str = "\u{e76c}";
const ICON_BACK: &str = "\u{e72b}";

/// What marks the entry that is chosen now among others.
const ICON_CHECKED: &str = "\u{e73e}";

/// How large the picture of an entry is, in parts of the size of its text.
const PICTURE_SIZE: f32 = 1.25;

/// What an entry does when it is chosen.
#[derive(Clone)]
enum Deed {
    Act(Rc<dyn Fn(&mut App)>),
    /// Shows these entries instead.
    Open(Rc<Vec<MenuEntry>>),
    /// Nothing: the entry names what follows.
    Name,
}

/// What the button at the end of an entry does.
type Aside = Rc<dyn Fn(&mut App)>;

/// Something a menu offers.
#[derive(Clone)]
pub struct MenuEntry {
    /// A character of the theme's icon font.
    glyph: &'static str,
    label: SharedString,
    deed: Deed,
    /// Whether it is marked as what is chosen now.
    checked: bool,
    /// A picture shown instead of the glyph, such as the icon of a file.
    picture: Option<Arc<RenderImage>>,
    /// A small button at the end of the entry, seen while the pointer is
    /// on it: its glyph and what it does.
    aside: Option<(&'static str, Aside)>,
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
            deed: Deed::Act(Rc::new(action)),
            checked: false,
            picture: None,
            aside: None,
        }
    }

    /// An entry that shows `entries` in the place of the menu's entries.
    /// One without entries is not offered.
    pub fn submenu(
        glyph: &'static str,
        label: impl Into<SharedString>,
        entries: Vec<MenuEntry>,
    ) -> Self {
        Self {
            deed: Deed::Open(Rc::new(entries)),
            ..Self::new(glyph, label, |_| {})
        }
    }

    /// A line that names what the entries after it are about, and does
    /// nothing.
    pub fn heading(label: impl Into<SharedString>) -> Self {
        Self {
            deed: Deed::Name,
            ..Self::new("", label, |_| {})
        }
    }

    /// The entry, marked as what is chosen now where `checked` says so.
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// Whether the entry is worth a line: a further list without entries
    /// is not.
    fn offered(&self) -> bool {
        match &self.deed {
            Deed::Open(entries) => !entries.is_empty(),
            Deed::Act(_) | Deed::Name => true,
        }
    }

    /// The entry, showing `picture` instead of its glyph where there is
    /// one.
    pub fn with_picture(mut self, picture: Option<Arc<RenderImage>>) -> Self {
        self.picture = picture;
        self
    }

    /// The entry, with a small button at its end that shows `glyph` while
    /// the pointer is on the entry and does `action` instead of the entry.
    pub fn with_aside(mut self, glyph: &'static str, action: impl Fn(&mut App) + 'static) -> Self {
        self.aside = Some((glyph, Rc::new(action)));
        self
    }
}

/// The open menu.
#[derive(Default)]
pub struct Menus {
    open: Option<WindowHandle<MenuView>>,
    /// Counts the menus opened, so that what reacts to a click can tell
    /// that something else opened one for it.
    opened: u64,
}

impl Global for Menus {}

impl Menus {
    /// Opens a menu of `entries` at `anchor`; any other menu closes. A menu
    /// without entries does not open.
    pub fn open(entries: Vec<MenuEntry>, anchor: PopupAnchor, cx: &mut App) {
        Self::close(cx);
        let entries: Vec<MenuEntry> = entries.into_iter().filter(MenuEntry::offered).collect();
        if entries.is_empty() {
            return;
        }
        cx.global_mut::<Self>().opened += 1;
        let frame = anchor.frame(WIDTH, height_of(entries.len(), cx));
        let rough = Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(height_of(entries.len(), cx))),
        };
        let opened = open_floating(rough, anchor.display_id, true, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(true);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("a menu is not where it belongs: {error:#}");
            }
            MenuView::new(entries, anchor, window, cx)
        });
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

    /// A number that grows each time a menu opens.
    pub fn opened(cx: &App) -> u64 {
        cx.try_global::<Self>().map_or(0, |menus| menus.opened)
    }
}

/// The logical height of the panel of a menu of `rows` entries in the
/// spacing of the theme in use.
fn height_of(rows: usize, cx: &App) -> f32 {
    let theme = cx.global::<Theme>();
    let padding = theme.spacing.small / px(1.);
    let gap = theme.spacing.tiny / px(1.);
    extent(rows, padding, gap).1
}

/// The logical width and height of the panel of a menu of `rows` entries,
/// `gap` apart and `padding` from its border.
fn extent(rows: usize, padding: f32, gap: f32) -> (f32, f32) {
    let rows = rows.min(ROWS_SHOWN) as f32;
    let height = rows * ROW_HEIGHT + (rows - 1.).max(0.) * gap + padding * 2.;
    (WIDTH, height)
}

/// The root view of a menu window.
pub struct MenuView {
    /// The entries shown first, then each further list chosen, the one
    /// shown last.
    levels: Vec<Rc<Vec<MenuEntry>>>,
    /// The pointer feedback of each line shown.
    rows: Vec<Interaction>,
    /// Where the menu was opened, to place it again when its height
    /// changes.
    anchor: PopupAnchor,
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
        anchor: PopupAnchor,
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
            levels: vec![Rc::new(entries)],
            edge: anchor.edge,
            anchor,
            visibility,
            closing: false,
            was_active: false,
            _activation: activation,
        }
    }

    /// The lines shown now: the way back from a further list, then its
    /// entries.
    fn lines(&self) -> usize {
        let shown = self.levels.last().map_or(0, |entries| entries.len());
        shown + usize::from(self.levels.len() > 1)
    }

    /// Shows `entries` instead of those shown, or goes back to the list
    /// before with `None`, and gives the window the height of what it
    /// shows now.
    fn go(&mut self, entries: Option<Rc<Vec<MenuEntry>>>, window: &Window, cx: &mut Context<Self>) {
        match entries {
            Some(entries) => {
                let entries = entries.iter().filter(|entry| entry.offered()).cloned();
                self.levels.push(Rc::new(entries.collect()));
            }
            None if self.levels.len() > 1 => {
                self.levels.pop();
            }
            None => return,
        }
        let motion = cx.global::<Theme>().motion;
        self.rows = (0..self.lines())
            .map(|_| Interaction::new(&motion))
            .collect();
        let frame = self.anchor.frame(WIDTH, height_of(self.lines(), cx));
        match NativeSurface::of(window) {
            // The window learns its new size only if it is given while
            // nothing of the user interface is being worked on.
            Ok(native) => cx
                .spawn(async move |_, _| {
                    if let Err(error) = native.place(frame) {
                        report!("a menu is not where it belongs: {error:#}");
                    }
                })
                .detach(),
            Err(error) => report!("a menu is not where it belongs: {error:#}"),
        }
        cx.notify();
    }

    /// Starts closing; the window goes away once the motion has finished.
    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.visibility.animate_to(0.);
        cx.notify();
    }

    /// The line at `index` of those shown, which shows `entry`, or the way
    /// back where `entry` is `None`.
    fn line(
        &mut self,
        index: usize,
        entry: Option<&MenuEntry>,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let language = Language::of(cx);
        let (glyph, label) = match entry {
            Some(entry) => (entry.glyph, entry.label.clone()),
            None => (ICON_BACK, language.say("Back").into()),
        };
        let row = div()
            .id(("menu-entry", index))
            .h(px(ROW_HEIGHT))
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium);
        if let Some(MenuEntry {
            deed: Deed::Name, ..
        }) = entry
        {
            return row
                .text_color(theme.colors.on_surface_variant)
                .font_weight(FontWeight::MEDIUM)
                .child(div().flex_1().min_w_0().child(one_line(label)))
                .into_any_element();
        }
        let state = self.rows[index].state(window);
        let deed = entry.map(|entry| entry.deed.clone());
        let trailing = match (&deed, entry.is_some_and(|entry| entry.checked)) {
            (Some(Deed::Open(_)), _) => Some(ICON_MORE),
            (_, true) => Some(ICON_CHECKED),
            _ => None,
        };
        let row = row.bg(state_layer(theme, theme.colors.surface_container, state));
        let row = match deed {
            // What the entry does runs outside this view, which it may
            // ask to close.
            Some(Deed::Act(action)) => row.on_click(move |_, _, cx| {
                action(cx);
                // This window cannot be told to close while it handles
                // the click.
                cx.defer(Menus::close);
            }),
            Some(Deed::Open(entries)) => row.on_click(cx.listener(move |this, _, window, cx| {
                this.go(Some(entries.clone()), window, cx);
            })),
            Some(Deed::Name) => row,
            None => row.on_click(cx.listener(|this, _, window, cx| this.go(None, window, cx))),
        };
        let lead = match entry.and_then(|entry| entry.picture.clone()) {
            Some(picture) => img(picture)
                .size(theme.typography.label_size * PICTURE_SIZE)
                .flex_none()
                .into_any_element(),
            None => icon(theme, glyph)
                .text_size(theme.typography.label_size)
                .text_color(theme.colors.on_surface_variant)
                .into_any_element(),
        };
        let aside = { entry.and_then(|entry| entry.aside.clone()) }.map(|(glyph, aside)| {
            div()
                .id(("menu-aside", index))
                .flex_none()
                .opacity(state.hover)
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    aside(cx);
                    cx.defer(Menus::close);
                })
                .child(
                    icon(theme, glyph)
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface_variant),
                )
        });
        let row = row
            .child(lead)
            .child(div().flex_1().min_w_0().child(one_line(label)))
            .children(trailing.map(|glyph| {
                icon(theme, glyph)
                    .text_size(theme.typography.label_size)
                    .text_color(theme.colors.primary)
            }))
            .children(aside);
        Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index))
            .into_any_element()
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let visibility = self.visibility.value(window);
        if self.closing && self.visibility.is_settled() {
            retire_floating(window, cx);
        }
        let shown = self.levels.last().cloned().unwrap_or_default();
        let back = self.levels.len() > 1;
        let mut rows = Vec::with_capacity(self.lines());
        if back {
            rows.push(self.line(0, None, &theme, window, cx));
        }
        for (at, entry) in shown.iter().enumerate() {
            let index = at + usize::from(back);
            rows.push(self.line(index, Some(entry), &theme, window, cx));
        }
        floating_panel(&theme, self.edge, visibility, |panel| {
            panel.p(theme.spacing.small).child(
                div()
                    .id("menu-entries")
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.tiny)
                    .overflow_y_scroll()
                    .children(rows),
            )
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
        assert_eq!(extent(40, 8., 4.), extent(ROWS_SHOWN, 8., 4.));
    }

    #[test]
    fn an_empty_further_list_is_not_offered() {
        let nothing = MenuEntry::submenu(ICON_MORE, "Nothing", Vec::new());
        let something = MenuEntry::submenu(
            ICON_MORE,
            "Something",
            vec![MenuEntry::new(ICON_MORE, "One", |_| {})],
        );
        assert!(!nothing.offered());
        assert!(something.offered());
        assert!(MenuEntry::heading("About").offered());
        assert!(
            MenuEntry::new(ICON_MORE, "Do", |_| {})
                .checked(true)
                .checked
        );
    }
}
