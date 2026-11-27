// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The snap zones and layouts, as a page of the settings in their section
//! Windows: widget `zones-editor`. It shows each `[[snap.layout]]` of the
//! file as a small picture with its name, or the six layouts of Windows
//! where the file has none, and the `[[snap.zone]]` entries a window
//! dragged with Shift snaps to, by display. **Edit on screen** opens the
//! editor of snap zones (the action `edit-zones`) over the display chosen
//! with the layout in it, **New layout** with one zone; a layout of the
//! file is removed here. The zones of Shift are written by hand in
//! `config.toml`; the page shows them.

use gpui::{
    AppContext, Context, Div, IntoElement, ParentElement, Render, SharedString, Styled, Window,
    div, px,
};

use super::edges_editor::{ConfigPage, page_card};
use super::settings_parts::{Feedback, chip_row, note_line};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::Display;
use crate::runners::fill;
use crate::shell::{KnownDisplays, Shell};
use crate::snap::{Snapper, Split, Zone};
use crate::theme::Theme;
use crate::ui::one_line;

/// The name of the page.
const NAME: &str = "zones-editor";

/// The size of the picture of a layout, in logical pixels, and the space
/// between its zones.
const PICTURE: (f32, f32) = (96., 60.);
const PICTURE_GAP: f32 = 3.;

pub struct ZonesEditorModule;

impl Module for ZonesEditorModule {
    fn name(&self) -> &'static str {
        NAME
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel(NAME, |_, cx| cx.new(ZonesEditor::new).into())
    }
}

/// The small picture of `zones`.
fn picture(theme: &Theme, zones: &[Zone]) -> Div {
    let blocks = zones.iter().map(|zone| {
        let [x, y, width, height] = zone.shares();
        div()
            .absolute()
            .left(px(x * PICTURE.0 + PICTURE_GAP / 2.))
            .top(px(y * PICTURE.1 + PICTURE_GAP / 2.))
            .w(px((width * PICTURE.0 - PICTURE_GAP).max(1.)))
            .h(px((height * PICTURE.1 - PICTURE_GAP).max(1.)))
            .rounded(theme.shape.small)
            .bg(theme.colors.secondary_container)
    });
    div()
        .relative()
        .flex_none()
        .w(px(PICTURE.0))
        .h(px(PICTURE.1))
        .rounded(theme.shape.medium)
        .bg(theme.colors.surface_container)
        .children(blocks)
}

/// The page.
struct ZonesEditor {
    /// The displays there are, from the left, by name, and the one the
    /// editor opens on.
    displays: Vec<String>,
    display: usize,
    failure: Option<SharedString>,
    feedback: Feedback,
}

impl ConfigPage for ZonesEditor {
    fn feedback(&mut self) -> &mut Feedback {
        &mut self.feedback
    }

    fn fail(&mut self, failure: Option<SharedString>) {
        self.failure = failure;
    }
}

impl ZonesEditor {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let language = Language::of(cx);
        let count = Display::all().len();
        let displays = (0..count)
            .map(|index| {
                let known = KnownDisplays::all(cx).iter();
                match { known }.find(|name| name.position == index + 1) {
                    Some(name) => name.friendly.clone(),
                    None => fill(language.say("Display {}"), &[&(index + 1).to_string()]),
                }
            })
            .collect();
        Self {
            displays,
            display: 0,
            failure: None,
            feedback: Feedback::new(motion),
        }
    }

    /// Opens the editor on the screen with the layout called `layout`, or
    /// a new one.
    fn edit(&mut self, layout: Option<String>, cx: &mut Context<Self>) {
        let display = self.display;
        cx.defer(move |cx| {
            Snapper::edit_zones(Some(display), layout.as_deref(), cx);
        });
    }

    /// The card of the layouts of the snap layouts.
    fn layouts(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let snap = Shell::config(cx).snap.clone();
        let mut card = page_card(&theme, language.say("Snap layouts"));
        let own = !snap.layouts.is_empty();
        if !own {
            card = card.child(note_line(
                &theme,
                language.say("These are the six layouts of Windows. Saving a layout of your own shows yours in their place."),
            ));
        }
        for (index, shown) in snap.shown_layouts().into_iter().enumerate() {
            let (key, zones) = (shown.key, shown.zones);
            let title = { shown.name }.unwrap_or_else(|| fill(language.say("Layout {}"), &[&key]));
            let editable = Split::from_zones(&zones).is_some();
            let count = fill(language.say("{} zones"), &[&zones.len().to_string()]);
            let mut buttons = chip_row(&theme);
            if editable {
                let layout = key.clone();
                buttons = buttons.child(self.button(
                    ("zones-edit", index),
                    language.say("Edit on screen"),
                    false,
                    move |this, cx| this.edit(Some(layout.clone()), cx),
                    window,
                    cx,
                ));
            }
            if own {
                let layout = key.clone();
                buttons = buttons.child(self.button(
                    ("zones-remove", index),
                    language.say("Remove"),
                    false,
                    move |this, cx| {
                        let layout = layout.clone();
                        this.write(move |file| file.remove_snap_layout(&layout), cx);
                    },
                    window,
                    cx,
                ));
            }
            let mut text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(one_line(title))
                .child(
                    div()
                        .text_color(theme.colors.on_surface_variant)
                        .child(one_line(count)),
                );
            if !editable {
                text = text.child(
                    div().text_color(theme.colors.on_surface_variant).child(language.say(
                        "Its zones are not cut by straight lines from side to side, so it is changed in config.toml.",
                    )),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .child(picture(&theme, &zones))
                    .child(text)
                    .child(buttons),
            );
        }
        if self.displays.len() > 1 {
            let mut chips = chip_row(&theme);
            for index in 0..self.displays.len() {
                let label = self.displays[index].clone();
                let chosen = index == self.display;
                chips = chips.child(self.button(
                    ("zones-display", index),
                    label,
                    chosen,
                    move |this, cx| {
                        this.display = index;
                        cx.notify();
                    },
                    window,
                    cx,
                ));
            }
            card = card
                .child(note_line(
                    &theme,
                    language.say("The editor opens on this display:"),
                ))
                .child(chips);
        }
        let new = self.button(
            ("zones-new", 0),
            language.say("New layout"),
            false,
            |this, cx| this.edit(None, cx),
            window,
            cx,
        );
        card = card.child(chip_row(&theme).child(new)).child(note_line(
            &theme,
            language.say("In the editor a click splits a zone, a line is dragged to move it, Enter saves and Esc cancels. Win+Alt+Z opens it where the keys of Windows are bound."),
        ));
        if let Some(failure) = &self.failure {
            card = card.child(note_line(&theme, failure.clone()));
        }
        card
    }

    /// The card of the zones a window dragged with Shift snaps to.
    fn shift_zones(&self, cx: &Context<Self>) -> Div {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let snap = &Shell::config(cx).snap;
        let mut card = page_card(&theme, language.say("Zones of Shift")).child(note_line(
            &theme,
            language.say("A window dragged with Shift held snaps to the zone under the pointer. They are [[snap.zone]] entries of config.toml."),
        ));
        let mut displays: Vec<Option<usize>> = snap.zones.iter().map(|zone| zone.display).collect();
        displays.sort();
        displays.dedup();
        if displays.is_empty() {
            return card.child(note_line(&theme, language.say("There are none.")));
        }
        for display in displays {
            let zones: Vec<Zone> = { snap.zones.iter() }
                .filter(|zone| zone.display == display)
                .filter_map(|zone| Zone::parse(zone.rect))
                .collect();
            let title = match display {
                Some(number) => fill(language.say("Display {}"), &[&number.to_string()]),
                None => language.say("All displays").to_owned(),
            };
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(theme.spacing.medium)
                    .child(picture(&theme, &zones))
                    .child(div().flex_1().min_w_0().child(one_line(title))),
            );
        }
        card
    }
}

impl Render for ZonesEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let layouts = self.layouts(window, cx);
        div()
            .flex()
            .flex_col()
            .gap(theme.spacing.medium)
            .child(layouts)
            .child(self.shift_zones(cx))
    }
}
