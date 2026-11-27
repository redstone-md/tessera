// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The history of the clipboard to look through and paste from, as Win+V
//! opens it in Windows. Widgets: `clipboard`, the list with its search,
//! for a popup, and `clipboard-button`, a button of a bar that opens the
//! popup `clipboard`. Action: `clipboard`, which opens that popup and
//! takes Win+V from Windows.

use std::ops::Range;
use std::sync::Arc;

use chrono::{DateTime, Local};
use gpui::{
    AnyElement, App, AppContext, Context, ElementId, Entity, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ObjectFit, ParentElement, Pixels, Render, RenderImage,
    ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, StyledImage,
    UniformListScrollHandle, Window, div, img, px, uniform_list,
};

use crate::actions::{Action, Actions};
use crate::clipboard::{ClipHistory, Content, Entry, file_name};
use crate::config::{PopupConfig, WidgetSpec};
use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::AppWindowId;
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, pill};

/// The popup the action and the button open.
const POPUP: &str = "clipboard";

const ICON_SEARCH: &str = "\u{e721}";
const ICON_TEXT: &str = "\u{e8a5}";
const ICON_FILES: &str = "\u{e8b7}";
const ICON_LINK: &str = "\u{e71b}";
const ICON_PASTE: &str = "\u{e77f}";
const ICON_PIN: &str = "\u{e718}";
const ICON_PINNED: &str = "\u{e841}";
const ICON_DELETE: &str = "\u{e74d}";
const ICON_OPEN: &str = "\u{e8a7}";
const ICON_PICTURE: &str = "\u{e8b9}";

/// How high a row is, with the gap below its card, and how wide the
/// picture of a row is at the most.
const ROW_HEIGHT: Pixels = px(72.);
const PICTURE_WIDTH: Pixels = px(144.);

/// The side of a button of a row.
const BUTTON: Pixels = px(28.);

/// The size of the caption under what a row shows.
const CAPTION_SIZE: Pixels = px(11.);

pub struct ClipboardModule;

impl Module for ClipboardModule {
    fn name(&self) -> &'static str {
        "clipboard"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("clipboard", |_, cx| cx.new(ClipList::new).into())?;
        widgets.register("clipboard-button", |_, cx| cx.new(|_| ClipButton).into())?;
        widgets.register_popup(PopupConfig {
            place: Default::default(),
            name: POPUP.to_owned(),
            width: 440.,
            height: 560.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "clipboard".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })?;
        widgets.open_with("clipboard-button", POPUP);
        Ok(())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Windows keeps Win+V, which opens its own history, to itself.
        let open = Action::new(|_, cx| open(cx))
            .taking_reserved_keys()
            .by_default("win+v", None);
        actions.register("clipboard", open)
    }
}

/// Opens the history, noting the window in front to paste into, or
/// closes it when it is open.
fn open(cx: &mut App) {
    ClipHistory::aim(cx);
    Shell::toggle_popup(POPUP, cx);
}

/// A button of a bar that opens the history.
struct ClipButton;

impl Render for ClipButton {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        // The bar does not take the keyboard, so the window in front when
        // it is pressed is the one the user was in.
        pill(theme)
            .id("clipboard-button")
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| ClipHistory::aim(cx))
            .child(icon(theme, ICON_PASTE))
    }
}

/// The history, searched by what is written above it.
struct ClipList {
    history: Option<Entity<ClipHistory>>,
    field: Entity<TextField>,
    /// The positions in the history of the items that match the search,
    /// newest first.
    matches: Vec<usize>,
    /// The position in `matches` that Enter pastes.
    selected: usize,
    /// What the field said when the list was made.
    written: String,
    /// The window to paste into: the one in front when the history was
    /// asked for.
    target: Option<AppWindowId>,
    scroll: UniformListScrollHandle,
    /// Whether the keys were given to the field yet.
    focused: bool,
}

impl ClipList {
    fn new(cx: &mut Context<Self>) -> Self {
        let placeholder = Language::of(cx).say("Search the clipboard");
        let field = cx.new(|cx| TextField::new(placeholder.into(), cx).with_glyph(ICON_SEARCH));
        cx.observe(&field, |this, _, cx| this.typed(cx)).detach();
        let history = ClipHistory::shared(cx);
        if let Some(history) = &history {
            cx.observe(history, |this, _, cx| {
                this.find(cx);
                cx.notify();
            })
            .detach();
        }
        let target = history
            .as_ref()
            .and_then(|history| history.update(cx, |history, _| history.target()));
        let mut this = Self {
            history,
            field,
            matches: Vec::new(),
            selected: 0,
            written: String::new(),
            target,
            scroll: UniformListScrollHandle::new(),
            focused: false,
        };
        this.find(cx);
        this
    }

    /// The field says something else: the list starts over.
    fn typed(&mut self, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text();
        if text == self.written {
            return;
        }
        self.written = text;
        self.selected = 0;
        self.find(cx);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Looks for what the field says.
    fn find(&mut self, cx: &App) {
        self.matches = match &self.history {
            Some(history) => history.read(cx).history().matching(&self.written),
            None => Vec::new(),
        };
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }

    /// The item in `row` of the list.
    fn entry<'a>(&self, row: usize, cx: &'a App) -> Option<&'a Entry> {
        let history = self.history.as_ref()?.read(cx).history();
        history.entries().get(*self.matches.get(row)?)
    }

    fn id_of(&self, row: usize, cx: &App) -> Option<u64> {
        self.entry(row, cx).map(|entry| entry.id)
    }

    fn select(&mut self, row: usize, cx: &mut Context<Self>) {
        let row = row.min(self.matches.len().saturating_sub(1));
        if row != self.selected {
            self.selected = row;
            cx.notify();
        }
    }

    /// Selects the item `steps` away, if there is one, and scrolls it
    /// into view.
    fn step(&mut self, steps: isize, cx: &mut Context<Self>) {
        let Some(wanted) = self.selected.checked_add_signed(steps) else {
            return;
        };
        self.select(wanted, cx);
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Top);
    }

    /// Puts the item in `row` back on the clipboard and pastes it into the
    /// window the user was in, closing the popup.
    fn paste(&mut self, row: usize, cx: &mut Context<Self>) {
        let (Some(history), Some(id)) = (self.history.clone(), self.id_of(row, cx)) else {
            return;
        };
        let target = self.target;
        history.update(cx, |history, cx| history.paste(id, target, cx));
        cx.defer(Popups::close);
    }

    /// Does `change` with the item in `row`.
    fn change(
        &mut self,
        row: usize,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ClipHistory, u64, &mut Context<ClipHistory>),
    ) {
        let (Some(history), Some(id)) = (self.history.clone(), self.id_of(row, cx)) else {
            return;
        };
        history.update(cx, |history, cx| change(history, id, cx));
    }

    /// Handles the keys that are not text; text goes to the field.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Popups::close),
            "enter" => self.paste(self.selected, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            // With nothing written, Delete has nothing else to delete.
            "delete" if self.written.is_empty() => {
                self.change(self.selected, cx, |history, id, cx| history.remove(id, cx));
            }
            _ => {}
        }
    }

    /// A round button of a row with `glyph`, which does `press` with the
    /// item of the row.
    fn button(
        id: impl Into<ElementId>,
        glyph: &'static str,
        color: Hsla,
        theme: &Theme,
        row: usize,
        cx: &mut Context<Self>,
        press: fn(&mut ClipHistory, u64, &mut Context<ClipHistory>),
    ) -> AnyElement {
        div()
            .id(id)
            .size(BUTTON)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_color(color)
            .hover(|style| style.bg(theme.colors.surface_container))
            .child(icon(theme, glyph))
            .on_click(cx.listener(move |this, _, _, cx| {
                // The row under the button would paste the item.
                cx.stop_propagation();
                this.change(row, cx, press);
            }))
            .into_any_element()
    }

    fn render_rows(
        &self,
        range: Range<usize>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let language = Language::of(cx);
        let today = Local::now().date_naive();
        let mut rows = Vec::with_capacity(range.len());
        for row in range {
            let Some(shown) = self.entry(row, cx).map(Shown::of) else {
                continue;
            };
            let (id, lit) = (shown.id as usize, row == self.selected);
            let mut caption = match shown.copied.date_naive() == today {
                true => shown.copied.format("%H:%M").to_string(),
                false => language.moment(shown.copied.naive_local(), "%-d %b %H:%M"),
            };
            if let Look::Files { count, .. } = &shown.look
                && *count > 1
            {
                caption = format!("{caption} · {count} {}", language.say("items"));
            }
            if shown.pinned {
                caption = format!("{caption} · {}", language.say("Pinned"));
            }
            let quiet = theme.colors.on_surface_variant;
            let mut buttons = Vec::new();
            if lit && shown.link {
                let open: Press = |history, id, _| history.open_link(id);
                let button = (("clip-open", id), ICON_OPEN, quiet);
                buttons.push(Self::button(
                    button.0, button.1, button.2, theme, row, cx, open,
                ));
            }
            if lit || shown.pinned {
                let (glyph, color) = match shown.pinned {
                    true => (ICON_PINNED, theme.colors.primary),
                    false => (ICON_PIN, quiet),
                };
                let pin: Press = ClipHistory::toggle_pin;
                buttons.push(Self::button(
                    ("clip-pin", id),
                    glyph,
                    color,
                    theme,
                    row,
                    cx,
                    pin,
                ));
            }
            if lit {
                let remove: Press = ClipHistory::remove;
                let id = ("clip-delete", id);
                buttons.push(Self::button(id, ICON_DELETE, quiet, theme, row, cx, remove));
            }
            let ground = match lit {
                true => theme.colors.secondary_container,
                false => theme.colors.surface_container_highest,
            };
            let (leading, body) = shown.render(theme, language);
            let card = div()
                .size_full()
                .px(theme.spacing.medium)
                .flex()
                .items_center()
                .gap(theme.spacing.medium)
                .overflow_hidden()
                .rounded(theme.shape.medium)
                .bg(ground)
                .child(leading)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(body)
                        .child(
                            div()
                                .min_w_0()
                                .text_size(CAPTION_SIZE)
                                .text_color(theme.colors.on_surface_variant)
                                .child(one_line(caption)),
                        ),
                )
                .child(div().flex_none().flex().items_center().children(buttons));
            rows.push(
                div()
                    .id(("clip-row", id))
                    .h(ROW_HEIGHT)
                    .w_full()
                    .pb(theme.spacing.small)
                    .cursor_pointer()
                    .child(card)
                    .on_mouse_move(cx.listener(move |this, _, _, cx| this.select(row, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| this.paste(row, cx)))
                    .into_any_element(),
            );
        }
        rows
    }
}

/// What a button of a row does with the item of the row.
type Press = fn(&mut ClipHistory, u64, &mut Context<ClipHistory>);

/// What the row of an item shows of it, taken out of the history so that
/// a long text is not copied for each frame.
struct Shown {
    id: u64,
    pinned: bool,
    copied: DateTime<Local>,
    /// Whether the item is a web address.
    link: bool,
    look: Look,
}

/// What kind of item a row shows, and what of it.
enum Look {
    Text(SharedString),
    Files {
        count: usize,
        names: String,
    },
    Picture {
        thumbnail: Option<Arc<RenderImage>>,
        width: u32,
        height: u32,
    },
}

impl Shown {
    fn of(entry: &Entry) -> Self {
        let look = match &entry.content {
            Content::Text(_) => Look::Text(entry.preview.clone()),
            Content::Files(files) => Look::Files {
                count: files.len(),
                names: match files.as_slice() {
                    [one] => file_name(one),
                    _ => entry.preview.to_string(),
                },
            },
            Content::Image(picture) => Look::Picture {
                thumbnail: picture.thumbnail.clone(),
                width: picture.width,
                height: picture.height,
            },
        };
        Self {
            id: entry.id,
            pinned: entry.pinned,
            copied: entry.copied,
            link: entry.content.link().is_some(),
            look,
        }
    }

    /// What stands at the start of the row, and what the row shows.
    fn render(self, theme: &Theme, language: Language) -> (AnyElement, AnyElement) {
        let glyph = |glyph: &'static str| {
            icon(theme, glyph)
                .flex_none()
                .text_color(theme.colors.on_surface_variant)
                .into_any_element()
        };
        let line = |text: SharedString| {
            div()
                .min_w_0()
                .text_color(theme.colors.on_surface)
                .child(one_line(text))
                .into_any_element()
        };
        match self.look {
            Look::Text(preview) => {
                let leading = glyph(if self.link { ICON_LINK } else { ICON_TEXT });
                let body = div()
                    .min_w_0()
                    .line_clamp(2)
                    .text_color(theme.colors.on_surface)
                    .child(preview);
                (leading, body.into_any_element())
            }
            Look::Files { names, .. } => (glyph(ICON_FILES), line(names.into())),
            Look::Picture {
                thumbnail,
                width,
                height,
            } => {
                let tall = ROW_HEIGHT - theme.spacing.small * 3.;
                let wide = (tall * (width as f32 / height.max(1) as f32)).min(PICTURE_WIDTH);
                let leading = match thumbnail {
                    Some(thumbnail) => img(thumbnail)
                        .h(tall)
                        .w(wide)
                        .flex_none()
                        .object_fit(ObjectFit::Contain)
                        .rounded(theme.shape.small)
                        .into_any_element(),
                    None => glyph(ICON_PICTURE),
                };
                let text = format!("{} · {width} × {height}", language.say("Picture"));
                (leading, line(text.into()))
            }
        }
    }
}

impl Render for ClipList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if !self.focused {
            self.focused = true;
            self.field.read(cx).focus(window);
        }
        let (on, any, unpinned) = match &self.history {
            Some(history) => {
                let history = history.read(cx);
                let entries = history.history().entries();
                let unpinned = entries.iter().any(|entry| !entry.pinned);
                (history.is_on(), !entries.is_empty(), unpinned)
            }
            None => (false, false, false),
        };
        let clear = div()
            .id("clear-clipboard")
            .flex_none()
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .hover(|style| style.bg(theme.colors.surface_container_highest))
            .child(language.say("Clear all"))
            .on_click(cx.listener(|this, _, _, cx| {
                if let Some(history) = &this.history {
                    history.update(cx, |history, cx| history.clear(cx));
                }
            }));
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(div().flex_1().min_w_0().flex().child(self.field.clone()))
            .children((on && unpinned).then_some(clear));

        let note = |text: &'static str| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
                .into_any_element()
        };
        let list = if !on {
            note("Clipboard history is off")
        } else if !any {
            note("Nothing was copied yet")
        } else if self.matches.is_empty() {
            note("Nothing matches")
        } else {
            uniform_list(
                "clipboard-list",
                self.matches.len(),
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    let theme = *cx.global::<Theme>();
                    this.render_rows(range, &theme, cx)
                }),
            )
            .track_scroll(self.scroll.clone())
            .flex_1()
            .into_any_element()
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .text_size(theme.typography.label_size)
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(header)
            .child(list)
    }
}
