// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The parts of a toast that a banner and the list of notifications draw
//! alike, below its texts: the wide picture at the top, the first picture
//! of the content, the progress bars, the input fields (a line of text
//! with its send button, or a row of choices) and up to five buttons.
//! Every part has a fixed height, so that a notification can grow in and
//! shrink away, and the list can tell how tall it is before drawing it.
//!
//! What the user types and chooses is kept in [`ToastControls`], which the
//! view of the notification owns; a press is handed to the view, which
//! has the store do it.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement, IntoElement,
    MouseButton, ObjectFit, ParentElement, Pixels, SharedString, StatefulInteractiveElement,
    Styled, StyledImage, Window, div, img, px, relative,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::notifications_store::NotificationStore;
use crate::theme::Theme;
use crate::toast::{InputKind, Toast, ToastAction};
use crate::ui::{icon, one_line};

/// The height of the wide picture at the top.
const HERO: f32 = 112.;

/// The height of the picture in the content.
const INLINE: f32 = 96.;

/// The height of one progress bar with its lines.
const PROGRESS: f32 = 40.;

/// The height of the bar itself.
const BAR: f32 = 4.;

/// The height of a row of choices, and of a row of buttons.
const CHOICES: f32 = 30.;
const BUTTONS: f32 = 32.;

/// The space between two parts.
const GAP: f32 = 6.;

/// How many progress bars are drawn, at most.
const MOST_BARS: usize = 2;

/// The glyph of the button that sends what was typed.
const SEND_GLYPH: &str = "\u{E724}";

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// What the user can press or type into on a toast, in the order the
/// Tab key goes through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// The text field of the input at this place.
    Field(usize),
    /// The choices of the input at this place.
    Choices(usize),
    /// The action at this place, as a button.
    Action(usize),
}

/// What was pressed on the parts of a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastPress {
    /// The action at this place.
    Action(usize),
    /// The choice `choice` of the input at `input`.
    Choose { input: usize, choice: usize },
}

/// What a view does with a press: it is given the press, the window and
/// its own context.
pub type OnPress<V> = Rc<dyn Fn(&mut V, ToastPress, &mut Window, &mut Context<V>)>;

/// The fields and the choices of one toast, as the user left them.
pub struct ToastControls {
    toast: Arc<Toast>,
    /// The text fields, by the place of their input.
    fields: Vec<(usize, Entity<TextField>)>,
    /// The choice of each input of choices, by its place.
    chosen: HashMap<usize, usize>,
}

impl ToastControls {
    pub fn new(toast: Arc<Toast>, cx: &mut App) -> Self {
        let mut fields = Vec::new();
        let mut chosen = HashMap::new();
        for (at, input) in toast.inputs.iter().enumerate() {
            match &input.kind {
                InputKind::Text { placeholder } => {
                    let placeholder = SharedString::from(placeholder.clone());
                    let default = input.default.clone().unwrap_or_default();
                    let field = cx.new(|cx| {
                        let mut field = TextField::new(placeholder, cx);
                        if !default.is_empty() {
                            field.set_text(&default, false, cx);
                        }
                        field
                    });
                    fields.push((at, field));
                }
                InputKind::Selection { choices } => {
                    let first = { input.default.as_ref() }
                        .and_then(|default| choices.iter().position(|(id, _)| id == default))
                        .unwrap_or(0);
                    chosen.insert(at, first);
                }
            }
        }
        Self {
            toast,
            fields,
            chosen,
        }
    }

    /// The toast they are of.
    pub fn toast(&self) -> &Arc<Toast> {
        &self.toast
    }

    /// The values of the inputs, by their identifiers: what each field
    /// holds and the identifier of each choice.
    pub fn values(&self, cx: &App) -> HashMap<String, String> {
        let mut values = HashMap::new();
        for (at, field) in &self.fields {
            values.insert(self.toast.inputs[*at].id.clone(), field.read(cx).text());
        }
        for (&at, &choice) in &self.chosen {
            let input = &self.toast.inputs[at];
            if let InputKind::Selection { choices } = &input.kind
                && let Some((id, _)) = choices.get(choice)
            {
                values.insert(input.id.clone(), id.clone());
            }
        }
        values
    }

    /// Chooses `choice` of the input at `input`.
    pub fn choose(&mut self, input: usize, choice: usize) {
        self.chosen.insert(input, choice);
    }

    /// Chooses the next choice of the input at `input`, after the last the
    /// first.
    pub fn choose_next(&mut self, input: usize) {
        let count = match &self.toast.inputs.get(input).map(|input| &input.kind) {
            Some(InputKind::Selection { choices }) => choices.len(),
            _ => return,
        };
        let next = self
            .chosen
            .get(&input)
            .map_or(0, |at| (at + 1) % count.max(1));
        self.chosen.insert(input, next);
    }

    /// The field of the input at `input`.
    pub fn field(&self, input: usize) -> Option<&Entity<TextField>> {
        { self.fields.iter() }
            .find(|(at, _)| *at == input)
            .map(|(_, field)| field)
    }
}

/// The action that sends the field `input` of `toast`: the button whose
/// `hint-inputId` names it.
pub fn send_action(toast: &Toast, input: usize) -> Option<usize> {
    let id = &toast.inputs.get(input)?.id;
    { toast.actions.iter() }
        .position(|action| !action.context_menu && action.input_id.as_ref() == Some(id))
}

/// Whether the action at `at` stands beside a text field as its send
/// button rather than in the row of buttons.
fn beside_a_field(toast: &Toast, at: usize) -> bool {
    let Some(action) = toast.actions.get(at) else {
        return false;
    };
    let Some(id) = &action.input_id else {
        return false;
    };
    toast
        .input(id)
        .is_some_and(|input| matches!(input.kind, InputKind::Text { .. }))
}

/// The actions in the row of buttons, by their places.
fn row_buttons(toast: &Toast) -> Vec<usize> {
    { toast.actions.iter().enumerate() }
        .filter(|(at, action)| !action.context_menu && !beside_a_field(toast, *at))
        .map(|(at, _)| at)
        .collect()
}

/// What can be pressed or typed into on `toast`, in the order of the Tab
/// key.
pub fn controls(toast: &Toast) -> Vec<Control> {
    let inputs = { toast.inputs.iter().enumerate() }.map(|(at, input)| match input.kind {
        InputKind::Text { .. } => Control::Field(at),
        InputKind::Selection { .. } => Control::Choices(at),
    });
    inputs
        .chain(row_buttons(toast).into_iter().map(Control::Action))
        .collect()
}

/// What the button of `action` says.
pub fn action_label(action: &ToastAction, language: Language) -> String {
    match action.content.trim() {
        "" if action.is_snooze() => language.say("Snooze").to_owned(),
        "" if action.is_dismiss() => language.say("Dismiss").to_owned(),
        content => content.to_owned(),
    }
}

/// How much taller than one of plain texts a notification is with the
/// parts of `toast`.
pub fn extra_height(toast: &Toast, theme: &Theme) -> Pixels {
    let mut height = 0.;
    if toast.hero.is_some() {
        height += HERO + GAP;
    }
    if !toast.images.is_empty() {
        height += INLINE + GAP;
    }
    height += toast.progress.len().min(MOST_BARS) as f32 * (PROGRESS + GAP);
    for input in &toast.inputs {
        height += GAP
            + match input.kind {
                InputKind::Text { .. } => f32::from(theme.launcher.field_height),
                InputKind::Selection { .. } => CHOICES,
            };
    }
    if !row_buttons(toast).is_empty() {
        height += BUTTONS + GAP;
    }
    px(height)
}

/// The wide picture at the top of `toast`, of the application `app_id`,
/// once it was read; where it is still being read, its place stays empty.
pub fn hero(toast: &Toast, app_id: &str, theme: &Theme, cx: &App) -> Option<AnyElement> {
    let image = toast.hero.as_ref()?;
    let picture = { cx.try_global::<NotificationStore>() }
        .and_then(|store| store.picture(app_id, &image.src));
    let frame = div()
        .flex_none()
        .w_full()
        .h(px(HERO))
        .mb(px(GAP))
        .overflow_hidden()
        .rounded(theme.shape.medium)
        .bg(theme.colors.surface_container);
    Some(match picture {
        Some(picture) => frame
            .child(img(picture).size_full().object_fit(ObjectFit::Cover))
            .into_any_element(),
        None => frame.into_any_element(),
    })
}

/// The parts of `toast` below its texts, with `focused` the control the
/// keys are on, if any; `id` tells the elements of this notification from
/// those of others.
pub fn parts<V: 'static>(
    id: u64,
    controls: &ToastControls,
    app_id: &str,
    focused: Option<Control>,
    theme: &Theme,
    on_press: OnPress<V>,
    cx: &mut Context<V>,
) -> Vec<AnyElement> {
    let toast = controls.toast().clone();
    let language = Language::of(cx);
    let caption = theme.typography.label_size * CAPTION_SHARE;
    let store = cx.try_global::<NotificationStore>();
    let mut parts = Vec::new();
    let ring = |element: gpui::Stateful<gpui::Div>, on: bool| match on {
        true => element.border_2().border_color(theme.colors.primary),
        false => element,
    };

    if let Some(image) = toast.images.first() {
        let picture = store.and_then(|store| store.picture(app_id, &image.src));
        let frame = div()
            .flex_none()
            .w_full()
            .h(px(INLINE))
            .overflow_hidden()
            .rounded(theme.shape.small);
        parts.push(match picture {
            Some(picture) => frame
                .child(img(picture).size_full().object_fit(ObjectFit::Contain))
                .into_any_element(),
            None => frame.bg(theme.colors.surface_container).into_any_element(),
        });
    }

    for progress in toast.progress.iter().take(MOST_BARS) {
        let share = progress.value.share();
        let right = match (&progress.value_text, share) {
            (Some(text), _) => text.clone(),
            (None, Some(share)) => format!("{:.0} %", share * 100.),
            (None, None) => String::new(),
        };
        let fill = match share {
            Some(share) => div().h_full().w(relative(share)).bg(theme.colors.primary),
            // It moves without telling how far: a part of the bar is lit.
            None => div()
                .h_full()
                .w(relative(0.3))
                .bg(theme.colors.primary)
                .opacity(0.6),
        };
        let line = |left: String, right: String| {
            div()
                .flex()
                .gap(theme.spacing.small)
                .text_size(caption)
                .text_color(theme.colors.on_surface_variant)
                .child(div().flex_1().min_w_0().child(one_line(left)))
                .child(div().flex_none().child(one_line(right)))
        };
        parts.push(
            div()
                .flex_none()
                .h(px(PROGRESS))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(3.))
                .child(line(
                    progress.title.clone().unwrap_or_default(),
                    String::new(),
                ))
                .child(
                    div()
                        .h(px(BAR))
                        .w_full()
                        .rounded(theme.shape.full)
                        .overflow_hidden()
                        .bg(theme.colors.surface_container)
                        .child(fill.rounded(theme.shape.full)),
                )
                .child(line(progress.status.clone(), right))
                .into_any_element(),
        );
    }

    for (at, input) in toast.inputs.iter().enumerate() {
        match &input.kind {
            InputKind::Text { .. } => {
                let Some(field) = controls.field(at).cloned() else {
                    continue;
                };
                let send = send_action(&toast, at).map(|action| {
                    let press = on_press.clone();
                    let button = div()
                        .id(SharedString::from(format!("toast-{id}-send-{at}")))
                        .flex_none()
                        .size(theme.launcher.field_height)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme.shape.full)
                        .text_color(theme.colors.primary)
                        .hover(|style| style.bg(theme.colors.surface_container))
                        .child(icon(theme, SEND_GLYPH))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            press(this, ToastPress::Action(action), window, cx);
                        }));
                    ring(button, focused == Some(Control::Field(at)))
                });
                parts.push(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(theme.spacing.small)
                        // The window of banners does not take the keys until
                        // the field is clicked.
                        .on_mouse_down(MouseButton::Left, |_, window, cx| {
                            cx.stop_propagation();
                            window.activate_window();
                        })
                        .child(div().flex_1().min_w_0().child(field))
                        .children(send)
                        .into_any_element(),
                );
            }
            InputKind::Selection { choices } => {
                let chosen = controls.chosen.get(&at).copied().unwrap_or(0);
                let chips: Vec<AnyElement> = { choices.iter().enumerate() }
                    .map(|(choice, (_, content))| {
                        let press = on_press.clone();
                        let ground = match choice == chosen {
                            true => theme.colors.secondary_container,
                            false => theme.colors.surface_container,
                        };
                        div()
                            .id(SharedString::from(format!(
                                "toast-{id}-choice-{at}-{choice}"
                            )))
                            .flex_none()
                            .h(px(CHOICES - 4.))
                            .px(theme.spacing.small)
                            .flex()
                            .items_center()
                            .rounded(theme.shape.full)
                            .bg(ground)
                            .text_size(caption)
                            .hover(|style| style.opacity(0.85))
                            .child(one_line(content.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                press(this, ToastPress::Choose { input: at, choice }, window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect();
                let row = div()
                    .id(SharedString::from(format!("toast-{id}-choices-{at}")))
                    .flex_none()
                    .h(px(CHOICES))
                    .flex()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .overflow_hidden()
                    .rounded(theme.shape.full)
                    .children(chips);
                parts.push(ring(row, focused == Some(Control::Choices(at))).into_any_element());
            }
        }
    }

    let buttons: Vec<AnyElement> = { row_buttons(&toast).into_iter() }
        .map(|at| {
            let action = &toast.actions[at];
            let press = on_press.clone();
            let picture = { action.image.as_ref() }
                .and_then(|src| store.and_then(|store| store.picture(app_id, src)));
            let button = div()
                .id(SharedString::from(format!("toast-{id}-button-{at}")))
                .flex_1()
                .min_w_0()
                .h(px(BUTTONS - 4.))
                .px(theme.spacing.small)
                .flex()
                .items_center()
                .justify_center()
                .gap(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .bg(theme.colors.surface_container)
                .hover(|style| style.bg(theme.colors.secondary_container))
                .text_color(theme.colors.primary)
                .font_weight(FontWeight::MEDIUM)
                .children(picture.map(|picture| img(picture).size(px(16.)).flex_none()))
                .child(one_line(action_label(action, language)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    press(this, ToastPress::Action(at), window, cx);
                }));
            ring(button, focused == Some(Control::Action(at))).into_any_element()
        })
        .collect();
    if !buttons.is_empty() {
        parts.push(
            div()
                .flex_none()
                .h(px(BUTTONS))
                .flex()
                .items_center()
                .gap(theme.spacing.small)
                .children(buttons)
                .into_any_element(),
        );
    }
    parts
        .into_iter()
        .map(|part| div().flex_none().mt(px(GAP)).child(part).into_any_element())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(xml: &str) -> Toast {
        Toast::parse(xml, &HashMap::new()).unwrap()
    }

    #[test]
    fn a_send_button_stands_beside_its_field_and_the_rest_in_the_row() {
        let chat = toast(
            r#"<toast><visual><binding template="ToastGeneric"><text>Hi</text></binding></visual>
<actions>
  <input id="reply" type="text" placeHolderContent="Reply"/>
  <input id="mood" type="selection"><selection id="a" content="A"/><selection id="b" content="B"/></input>
  <action content="Send" arguments="send" hint-inputId="reply"/>
  <action content="Like" arguments="like"/>
  <action content="Pick" arguments="pick" hint-inputId="mood"/>
  <action content="Mute" arguments="mute" placement="contextMenu"/>
</actions></toast>"#,
        );
        assert_eq!(send_action(&chat, 0), Some(0));
        assert_eq!(send_action(&chat, 1), Some(2));
        assert_eq!(row_buttons(&chat), [1, 2]);
        assert_eq!(
            controls(&chat),
            [
                Control::Field(0),
                Control::Choices(1),
                Control::Action(1),
                Control::Action(2)
            ]
        );
    }

    #[test]
    fn the_system_buttons_say_what_they_do() {
        let reminder = toast(
            r#"<toast><visual><binding template="ToastGeneric"><text>Hi</text></binding></visual><actions hint-systemCommands="SnoozeAndDismiss"/></toast>"#,
        );
        let english = Language::English;
        let labels: Vec<String> = reminder
            .actions
            .iter()
            .map(|a| action_label(a, english))
            .collect();
        assert_eq!(labels, ["Snooze", "Dismiss"]);
    }
}
