// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Draws the content an extension describes, with the shell's theme.
//!
//! The content is asked for on the thread of the extension and drawn once
//! it arrives; what the user does to a button, a field, a slider or a
//! toggle goes back to the extension in order, the moves of a slider and
//! the letters typed into a field that came while the extension was busy
//! taken together.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, AnyWindowHandle, AppContext, AsyncApp, Bounds, Context, Div, Entity,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, Render, RenderImage,
    StatefulInteractiveElement, Styled, WeakEntity, Window, canvas, div, img, px, relative,
};

use super::access::WatchKind;
use super::host::{Host, Place};
use super::images::Pictures;
use super::nodes::Content;
use super::runtime::{NodeKind, Slider as SliderNode, TextInput, Toggle};
use crate::field::TextField;
use crate::motion::{Interaction, Spring, mix};
use crate::theme::Theme;
use crate::ui::{Slider, icon, pill, state_layer, switch};

/// A window fewer pill heights high than this has one line of room.
const FLAT_BELOW: f32 = 3.;
/// How much of the text color the track of a progress bar takes on.
const TRACK_TINT: f32 = 0.12;
/// How wide a field and a slider are in a row, in pill heights.
const FIELD_IN_ROW: f32 = 5.;
const SLIDER_IN_ROW: f32 = 3.;
/// How long after the user moved a slider what the extension says of its
/// value is not taken over, so that the handle does not jump back.
const SLIDER_SETTLES: Duration = Duration::from_millis(400);

/// How a node is laid out, which depends on what contains it.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// The first node: it is the whole widget.
    Root,
    InRow,
    InColumn,
}

/// A field the extension described, by the name it gave it.
struct Field {
    field: Entity<TextField>,
    /// The value the extension gave it last.
    given: String,
    /// The text the extension was told of last.
    told: String,
}

/// A slider the extension described, drawn by a view of its own.
pub struct SliderControl {
    slider: Slider,
    /// The share of the range the handle is at.
    share: f32,
    /// Whether the user moved it since the extension was told.
    moved: bool,
    /// When the user moved it last.
    moved_at: Option<Instant>,
}

impl SliderControl {
    fn set(&mut self, share: f32, cx: &mut Context<Self>) {
        self.share = share;
        self.moved = true;
        self.moved_at = Some(Instant::now());
        cx.notify();
    }
}

impl Render for SliderControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let share = self.share;
        let track = self.slider.track(
            "extension-slider",
            share,
            window,
            cx,
            |this: &mut Self| &mut this.slider,
            |this, share, cx| this.set(share, cx),
        );
        div().w_full().flex().child(track)
    }
}

/// One placed instance of a widget that an extension provides.
pub struct ExtensionWidget {
    host: Entity<Host>,
    /// The widget's name inside its extension.
    widget: String,
    /// The name layouts use, for messages.
    id: String,
    /// The last content the extension returned, until it is first asked.
    content: Option<Rc<Content>>,
    /// Whether the content is being asked for, and whether it is to be
    /// asked for again once it comes.
    rendering: bool,
    stale: bool,
    /// Pointer feedback of the buttons and toggles, by node index.
    buttons: HashMap<usize, Interaction>,
    /// The shown fill of the progress bars, by node index.
    progress: HashMap<usize, Spring>,
    /// The shown position of the toggles, by their name.
    toggles: HashMap<String, Spring>,
    fields: HashMap<String, Field>,
    sliders: HashMap<String, Entity<SliderControl>>,
    pictures: Pictures,
    /// Whether the widget stands where there is one line of room, as on a
    /// bar: columns are laid out as rows there.
    flat: bool,
    /// The window the widget was drawn in last, and the middle of the
    /// widget in it: what the extension opens by itself opens there.
    window: Option<AnyWindowHandle>,
    middle: Rc<Cell<Option<Pixels>>>,
    /// What is still to be told to the extension, in order: the action,
    /// what tells a later one of the same control to take its place, and
    /// where what it opens is anchored.
    queue: VecDeque<(String, Option<String>, Place)>,
    /// Whether the extension is being told.
    telling: bool,
}

impl ExtensionWidget {
    pub fn new(
        host: Entity<Host>,
        widget: String,
        id: String,
        refresh: Option<Duration>,
        watch: Vec<WatchKind>,
        cx: &mut Context<Self>,
    ) -> Self {
        if let Some(interval) = refresh {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor().timer(interval).await;
                    if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe(&host, |this, _, cx| this.refresh(cx)).detach();
        let this = cx.weak_entity();
        host.update(cx, |host, cx| host.add_widget(this, &watch, cx));
        let mut widget = Self {
            host,
            widget,
            id,
            content: None,
            rendering: false,
            stale: false,
            buttons: HashMap::new(),
            progress: HashMap::new(),
            toggles: HashMap::new(),
            fields: HashMap::new(),
            sliders: HashMap::new(),
            pictures: Pictures::default(),
            flat: false,
            window: None,
            middle: Rc::default(),
            queue: VecDeque::new(),
            telling: false,
        };
        widget.refresh(cx);
        widget
    }

    /// Where the widget was drawn last: its window and its middle there.
    pub fn place(&self) -> Option<(AnyWindowHandle, Pixels)> {
        Some((self.window?, self.middle.get()?))
    }

    /// The pictures of the widget.
    pub(super) fn pictures(&mut self) -> &mut Pictures {
        &mut self.pictures
    }

    /// Where what the extension opens while drawing this widget opens.
    fn anchor(&self) -> Place {
        match self.place() {
            Some((window, x)) => Place::Widget(window, x),
            None => Place::Pointer,
        }
    }

    /// Asks the extension for the content again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.rendering {
            self.stale = true;
            return;
        }
        if self.host.read(cx).failed() {
            cx.notify();
            return;
        }
        self.rendering = true;
        let rendered = self.host.read(cx).render(&self.widget);
        let (host, place) = (self.host.clone(), self.anchor());
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = rendered.await;
            // Not within an update of the widget: what the extension asked
            // for meanwhile may look at its widgets.
            let rendered = cx.update(|cx| Host::finish(&host, outcome, place, cx));
            this.update(cx, |this, cx| {
                this.rendering = false;
                if let Ok(Some((content, problems))) = rendered {
                    let widget = this.widget.clone();
                    this.host
                        .update(cx, |host, _| host.complain(&widget, problems));
                    this.take(content, cx);
                }
                cx.notify();
                if std::mem::take(&mut this.stale) {
                    this.refresh(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows `content`, with fields and sliders as it describes them.
    fn take(&mut self, content: Content, cx: &mut Context<Self>) {
        let theme = *cx.global::<Theme>();
        let mut fields = std::mem::take(&mut self.fields);
        let mut sliders = std::mem::take(&mut self.sliders);
        for (index, node) in content.nodes.iter().enumerate() {
            if !content.shows(index) {
                continue;
            }
            match &node.kind {
                NodeKind::TextInput(input) => {
                    let field = match fields.remove(&input.id) {
                        Some(field) => self.keep_field(field, input, cx),
                        None => self.new_field(input, cx),
                    };
                    self.fields.insert(input.id.clone(), field);
                }
                NodeKind::Slider(slider) => {
                    let share = share_of(slider);
                    let control = sliders.remove(&slider.id).unwrap_or_else(|| {
                        let control = cx.new(|_| SliderControl {
                            slider: Slider::new(&theme, share),
                            share,
                            moved: false,
                            moved_at: None,
                        });
                        let id = slider.id.clone();
                        cx.observe(&control, move |this, control, cx| {
                            let moved = control.update(cx, |control, _| {
                                std::mem::take(&mut control.moved).then_some(control.share)
                            });
                            // The range as the extension describes it now.
                            let range = this.slider_range(&id);
                            if let (Some(share), Some((min, max))) = (moved, range) {
                                let value = min + (max - min) * share;
                                let action = format!("slider-changed:{id}:{value}");
                                this.tell(action, Some(format!("slider:{id}")), cx);
                            }
                        })
                        .detach();
                        control
                    });
                    control.update(cx, |control, cx| {
                        let settled = { control.moved_at }
                            .is_none_or(|moved| moved.elapsed() > SLIDER_SETTLES);
                        if settled && control.share != share {
                            control.share = share;
                            cx.notify();
                        }
                    });
                    self.sliders.insert(slider.id.clone(), control);
                }
                _ => {}
            }
        }
        self.pictures.keep_only(&content);
        self.content = Some(Rc::new(content));
    }

    /// The range of the slider `id` of the content shown.
    fn slider_range(&self, id: &str) -> Option<(f32, f32)> {
        let content = self.content.as_ref()?;
        content.nodes.iter().find_map(|node| match &node.kind {
            NodeKind::Slider(slider) if slider.id == id => Some((slider.min, slider.max)),
            _ => None,
        })
    }

    /// A field for `input`, which tells the extension what is typed.
    fn new_field(&mut self, input: &TextInput, cx: &mut Context<Self>) -> Field {
        let placeholder = input.placeholder.clone().into();
        let field = cx.new(|cx| {
            let mut field = TextField::new(placeholder, cx);
            field.put_text(&input.value, cx);
            field
        });
        let id = input.id.clone();
        cx.observe(&field, move |this, field, cx| {
            let text = field.read(cx).text();
            let Some(known) = this.fields.get_mut(&id) else {
                return;
            };
            if known.told == text {
                return;
            }
            known.told = text.clone();
            let action = format!("input-changed:{id}:{text}");
            this.tell(action, Some(format!("input:{id}")), cx);
        })
        .detach();
        Field {
            field,
            given: input.value.clone(),
            told: input.value.clone(),
        }
    }

    /// `field` as `input` describes it now: a value other than the one
    /// given before is put into it.
    fn keep_field(&mut self, mut field: Field, input: &TextInput, cx: &mut Context<Self>) -> Field {
        if field.given != input.value {
            field.given = input.value.clone();
            field.told = input.value.clone();
            let value = input.value.clone();
            field
                .field
                .update(cx, |field, cx| field.put_text(&value, cx));
        }
        field
    }

    /// Tells the extension `action`, after what it is still being told. An
    /// action with a `key` takes the place of one with the same key that
    /// waits.
    fn tell(&mut self, action: String, key: Option<String>, cx: &mut Context<Self>) {
        let place = self.anchor();
        self.tell_at(action, key, place, cx);
    }

    fn tell_at(
        &mut self,
        action: String,
        key: Option<String>,
        place: Place,
        cx: &mut Context<Self>,
    ) {
        if key.is_some()
            && let Some(waiting) = self
                .queue
                .iter_mut()
                .find(|(_, waiting, _)| *waiting == key)
        {
            *waiting = (action, key, place);
            return;
        }
        self.queue.push_back((action, key, place));
        self.tell_next(cx);
    }

    /// Tells the extension what waits first, once it is not being told.
    fn tell_next(&mut self, cx: &mut Context<Self>) {
        if self.telling || self.host.read(cx).failed() {
            return;
        }
        let Some((action, _, place)) = self.queue.pop_front() else {
            return;
        };
        self.telling = true;
        let widget = self.widget.clone();
        let told = { self.host.read(cx) }.call(move |instance| instance.activate(&widget, &action));
        let host = self.host.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = told.await;
            cx.update(|cx| {
                Host::finish(&host, outcome, place, cx);
                // Every widget of the extension may show something else now.
                host.update(cx, |_, cx| cx.notify());
            })
            .ok();
            this.update(cx, |this, cx| {
                this.telling = false;
                this.tell_next(cx);
            })
            .ok();
        })
        .detach();
    }

    fn children(
        &mut self,
        content: &Content,
        index: usize,
        placement: Placement,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Children must come later in the list, which rules out cycles.
        content.nodes[index]
            .children
            .iter()
            .map(|&child| child as usize)
            .filter(|&child| child > index)
            .filter_map(|child| self.element(content, child, placement, theme, window, cx))
            .collect()
    }

    /// A text. In a row it stays on one line and keeps its width, so that
    /// a full bar does not break it into lines; in a column it wraps.
    fn words(text: &str, placement: Placement) -> Div {
        let words = div().child(text.to_owned());
        match placement {
            Placement::Root | Placement::InRow => words.flex_none().whitespace_nowrap(),
            // Not squeezed where a column has more than fits.
            Placement::InColumn => words.flex_none(),
        }
    }

    /// What finds the middle of the whole widget in its window.
    fn measure(&self) -> impl IntoElement + use<> {
        let middle = self.middle.clone();
        let seen = move |bounds: Bounds<Pixels>, _: &mut Window, _: &mut gpui::App| {
            middle.set(Some(bounds.center().x));
        };
        canvas(seen, |_, (), _, _| {}).absolute().size_full()
    }

    fn element(
        &mut self,
        content: &Content,
        index: usize,
        placement: Placement,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let node = content.nodes.get(index)?;
        if !content.shows(index) {
            return None;
        }
        Some(match &node.kind {
            NodeKind::Text(text) => Self::words(text, placement).into_any_element(),
            NodeKind::Caption(text) => Self::words(text, placement)
                .text_color(theme.colors.on_surface_variant)
                .into_any_element(),
            NodeKind::Column | NodeKind::Scroll(_) if !self.flat => {
                let children =
                    self.children(content, index, Placement::InColumn, theme, window, cx);
                let column = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.small)
                    .children(children);
                if let NodeKind::Scroll(height) = node.kind {
                    // Its own height at most, and what does not fit is
                    // scrolled to, wherever it stands.
                    return Some(
                        column
                            .id(("extension-scroll", index))
                            .flex_none()
                            .max_h(px(height))
                            .overflow_y_scroll()
                            .into_any_element(),
                    );
                }
                match placement {
                    // The whole widget: what does not fit is scrolled to.
                    Placement::Root => column
                        .id("extension-content")
                        .relative()
                        .size_full()
                        .overflow_y_scroll()
                        .child(self.measure())
                        .into_any_element(),
                    Placement::InRow => column.into_any_element(),
                    // A column within a column is a card of its own.
                    Placement::InColumn => column
                        .flex_none()
                        .p(theme.spacing.medium)
                        .rounded(theme.shape.large)
                        .bg(theme.colors.surface_container_highest)
                        .into_any_element(),
                }
            }
            // Where there is one line of room a column is laid out as a row.
            NodeKind::Row | NodeKind::Column | NodeKind::Scroll(_) => {
                let children = self.children(content, index, Placement::InRow, theme, window, cx);
                match placement {
                    Placement::Root => pill(theme)
                        .flex_none()
                        .relative()
                        .children(children)
                        .child(self.measure())
                        .into_any_element(),
                    Placement::InRow => div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(theme.spacing.small)
                        .children(children)
                        .into_any_element(),
                    Placement::InColumn => div()
                        .w_full()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_between()
                        .gap(theme.spacing.small)
                        .children(children)
                        .into_any_element(),
                }
            }
            NodeKind::Progress(share) => {
                let share = if share.is_nan() {
                    0.
                } else {
                    share.clamp(0., 1.)
                };
                let shown = self
                    .progress
                    .entry(index)
                    .or_insert_with(|| Spring::new(theme.motion.effects, 0.));
                shown.animate_to(share);
                let shown = shown.value(window).clamp(0., 1.);
                // Kept at its height where a column has more than fits,
                // and told apart from a card it lies on.
                let track = div()
                    .h(theme.spacing.small)
                    .flex_none()
                    .rounded(theme.shape.full)
                    .overflow_hidden()
                    .bg(mix(
                        theme.colors.surface_container_highest,
                        theme.colors.on_surface,
                        TRACK_TINT,
                    ))
                    .child(
                        div()
                            .h_full()
                            .w(relative(shown))
                            .rounded(theme.shape.full)
                            .bg(theme.colors.primary),
                    );
                match placement {
                    Placement::InRow => track.w(theme.bar.pill_height * 1.5),
                    Placement::Root | Placement::InColumn => track.w_full(),
                }
                .into_any_element()
            }
            NodeKind::Button(button) => {
                let action = button.action.clone();
                let state = self
                    .buttons
                    .entry(index)
                    .or_insert_with(|| Interaction::new(&theme.motion))
                    .state(window);
                let element = div()
                    .id(("extension-button", index))
                    .flex_none()
                    .whitespace_nowrap()
                    .px(theme.spacing.small)
                    .rounded(theme.shape.full)
                    .bg(state_layer(theme, theme.colors.surface_container, state))
                    .text_color(theme.colors.primary)
                    .when_icon(&button.label, theme)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let place =
                            Place::Widget(window.window_handle(), window.mouse_position().x);
                        this.tell_at(action.clone(), None, place, cx);
                    }))
                    .child(button.label.clone());
                Interaction::track(element, cx, move |this: &mut Self| {
                    this.buttons.get_mut(&index)
                })
                .into_any_element()
            }
            NodeKind::Image(image) => {
                let height = match image.height {
                    0. => theme.typography.icon_size * 1.25,
                    height => px(height),
                };
                let picture = self.pictures.get(content, index, cx);
                picture_element(picture, height).into_any_element()
            }
            NodeKind::Icon(glyph) => icon(theme, glyph.clone()).flex_none().into_any_element(),
            NodeKind::TextInput(input) => {
                let field = self.fields.get(&input.id)?.field.clone();
                let id = input.id.clone();
                let holder = div()
                    .flex()
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key != "enter" {
                            return;
                        }
                        let Some(known) = this.fields.get(&id) else {
                            return;
                        };
                        let text = known.field.read(cx).text();
                        cx.stop_propagation();
                        this.tell(format!("submitted:{id}:{text}"), None, cx);
                    }))
                    .child(field);
                match placement {
                    Placement::InRow => holder.flex_none().w(theme.bar.pill_height * FIELD_IN_ROW),
                    Placement::Root | Placement::InColumn => holder.w_full(),
                }
                .into_any_element()
            }
            NodeKind::Slider(slider) => {
                let control = self.sliders.get(&slider.id)?.clone();
                let holder = div().flex().child(control);
                match placement {
                    Placement::InRow => holder.flex_none().w(theme.bar.pill_height * SLIDER_IN_ROW),
                    Placement::Root | Placement::InColumn => holder.w_full(),
                }
                .into_any_element()
            }
            NodeKind::Toggle(toggle) => self.toggle(toggle, index, theme, window, cx),
            NodeKind::Spacer => div().flex_1().into_any_element(),
        })
    }

    /// A switch that tells the extension when it is flipped.
    fn toggle(
        &mut self,
        toggle: &Toggle,
        index: usize,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on = |on: bool| if on { 1. } else { 0. };
        let travel = self
            .toggles
            .entry(toggle.id.clone())
            .or_insert_with(|| Spring::new(theme.motion.spatial, on(toggle.on)));
        travel.animate_to(on(toggle.on));
        let travel = travel.value(window);
        let state = self
            .buttons
            .entry(index)
            .or_insert_with(|| Interaction::new(&theme.motion))
            .state(window);
        let (id, flipped) = (toggle.id.clone(), !toggle.on);
        let element = switch(theme, ("extension-toggle", index), travel, state).on_click(
            cx.listener(move |this, _, _, cx| {
                // It moves at once, before the extension says so.
                if let Some(travel) = this.toggles.get_mut(&id) {
                    travel.animate_to(on(flipped));
                }
                this.tell(format!("toggled:{id}:{flipped}"), None, cx);
                cx.notify();
            }),
        );
        Interaction::track(element, cx, move |this: &mut Self| {
            this.buttons.get_mut(&index)
        })
        .into_any_element()
    }
}

/// The share of its range a slider is at.
fn share_of(slider: &SliderNode) -> f32 {
    let range = slider.max - slider.min;
    if range > 0. {
        ((slider.value - slider.min) / range).clamp(0., 1.)
    } else {
        0.
    }
}

/// A picture `height` high, as wide as it is in proportion, or an empty
/// space as high while it is not there.
fn picture_element(picture: Option<Arc<RenderImage>>, height: Pixels) -> AnyElement {
    let Some(picture) = picture else {
        return div().flex_none().size(height).into_any_element();
    };
    let size = picture.size(0);
    let (width, tall) = (size.width.0 as f32, size.height.0 as f32);
    let width = match tall > 0. {
        true => height * (width / tall),
        false => height,
    };
    img(picture)
        .flex_none()
        .w(width)
        .h(height)
        .into_any_element()
}

/// Draws a label of one character of the icon font, as the label of a
/// button that plays or skips, in that font.
trait IconLabel {
    fn when_icon(self, label: &str, theme: &Theme) -> Self;
}

impl<E: Styled> IconLabel for E {
    fn when_icon(self, label: &str, theme: &Theme) -> Self {
        let mut letters = label.chars();
        let private = |letter: char| ('\u{e000}'..='\u{f8ff}').contains(&letter);
        match (letters.next(), letters.next()) {
            (Some(letter), None) if private(letter) => {
                self.font_family(theme.typography.icon_family)
            }
            _ => self,
        }
    }
}

impl Render for ExtensionWidget {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        self.window = Some(window.window_handle());
        if self.host.read(cx).failed() {
            return pill(&theme)
                .text_color(theme.colors.on_surface_variant)
                .child(format!("{} stopped working", self.id))
                .into_any_element();
        }
        let Some(content) = self.content.clone() else {
            // Nothing is shown until the extension first answers.
            return div().into_any_element();
        };
        self.flat = window.viewport_size().height < theme.bar.pill_height * FLAT_BELOW;
        self.element(&content, 0, Placement::Root, &theme, window, cx)
            .unwrap_or_else(|| div().into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slider_is_drawn_at_its_share_of_the_range() {
        let slider = |min, max, value| SliderNode {
            id: "level".to_owned(),
            min,
            max,
            value,
        };
        assert_eq!(share_of(&slider(0., 10., 2.5)), 0.25);
        assert_eq!(share_of(&slider(-1., 1., 1.)), 1.);
        assert_eq!(share_of(&slider(5., 5., 5.)), 0.);
        assert_eq!(share_of(&slider(0., 1., 9.)), 1.);
    }
}
