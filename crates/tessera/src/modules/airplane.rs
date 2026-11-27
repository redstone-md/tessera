// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Airplane mode and the radios of the machine, for the quick settings.
//! Widgets: `airplane`, a tile that switches every radio off and later
//! those back on that were on; `radio-tile`, a tile that switches Wi-Fi or
//! Bluetooth (option radio); and `radios`, every radio with a switch of its
//! own, which the airplane tile unfolds. All of them share one reading of
//! the radios, asked every three seconds while one is shown.
//!
//! The airplane mode flag of Windows itself is not set: Windows lets only
//! its own quick settings set it. With every radio off, Tessera counts the
//! machine as in airplane mode, whoever switched them off.

use std::time::Duration;

use gpui::{
    AppContext, Context, Entity, Global, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, Window, div,
};

use super::quick::{Face, Reading, Source, TileView, on_or_off, register_tile, unusable_note};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{RadioId, RadioInfo, RadioType, Radios, airplane_mode};
use crate::theme::Theme;
use crate::ui::{TileFace, list_row, switch};

const ICON_AIRPLANE: &str = "\u{e709}";
const ICON_WIFI: &str = "\u{e701}";
const ICON_BLUETOOTH: &str = "\u{e702}";
const ICON_MOBILE: &str = "\u{ec3b}";
const ICON_OTHER: &str = "\u{e774}";

/// The radios of the machine, as the quick settings read them.
pub struct RadioSource;

impl Source for RadioSource {
    type State = Result<Vec<RadioInfo>, String>;

    const INTERVAL: Duration = Duration::from_secs(3);

    fn read() -> Self::State {
        Radios::all()
    }
}

type RadioReading = Reading<RadioSource>;

/// The radios last read; none before the first reading or where they
/// cannot be read.
fn radios_of(reading: &RadioReading) -> &[RadioInfo] {
    match reading.state() {
        Some(Ok(radios)) => radios,
        _ => &[],
    }
}

/// The radios that were on when airplane mode was switched on, to be
/// switched back on when it is switched off; `None` where Tessera did not
/// switch it on, after which every radio is.
#[derive(Default)]
struct WereOn(Option<Vec<RadioId>>);

impl Global for WereOn {}

/// `radios` with each that `chosen` takes shown `on`, for a tile to show
/// at once what it was asked for.
fn shown_as(radios: &[RadioInfo], chosen: impl Fn(&RadioInfo) -> bool, on: bool) -> Vec<RadioInfo> {
    radios
        .iter()
        .map(|radio| RadioInfo {
            on: match radio.switchable && chosen(radio) {
                true => on,
                false => radio.on,
            },
            ..radio.clone()
        })
        .collect()
}

/// The tile of airplane mode.
struct Airplane;

impl Face for Airplane {
    type Model = RadioReading;

    fn model(&self, cx: &mut gpui::App) -> Entity<RadioReading> {
        Reading::shared(cx)
    }

    fn follow(model: &Entity<RadioReading>, cx: &mut Context<TileView<Self>>) -> Subscription {
        Reading::follow(model, cx)
    }

    fn face(&self, model: &RadioReading, language: Language, _: &gpui::App) -> TileFace {
        let radios = radios_of(model);
        let on = airplane_mode(radios);
        let usable = radios.iter().any(|radio| radio.switchable);
        let note = match usable {
            true => on_or_off(on, language),
            false => unusable_note(model.state().is_some(), language),
        };
        TileFace {
            glyph: ICON_AIRPLANE,
            title: language.say("Airplane mode").into(),
            note: note.into(),
            lit: on,
            usable,
        }
    }

    fn click(&self, model: &Entity<RadioReading>, cx: &mut gpui::App) {
        model.update(cx, |reading, cx| {
            let radios = radios_of(reading).to_vec();
            if airplane_mode(&radios) {
                let were_on = cx.default_global::<WereOn>().0.clone();
                let back = |radio: &RadioInfo| {
                    were_on
                        .as_ref()
                        .is_none_or(|were_on| were_on.contains(&radio.id()))
                };
                let shown = shown_as(&radios, back, true);
                reading.change(
                    Some(Ok(shown)),
                    move || Radios::airplane_off(were_on.as_deref()),
                    |(), cx| cx.set_global(WereOn(None)),
                    cx,
                );
            } else {
                let shown = shown_as(&radios, |_| true, false);
                reading.change(
                    Some(Ok(shown)),
                    Radios::airplane_on,
                    |were_on, cx| cx.set_global(WereOn(Some(were_on))),
                    cx,
                );
            }
        });
    }
}

/// The tile of one kind of radio.
struct RadioTile {
    kind: RadioType,
}

impl RadioTile {
    fn glyph_and_title(&self) -> (&'static str, &'static str) {
        match self.kind {
            RadioType::Bluetooth => (ICON_BLUETOOTH, "Bluetooth"),
            _ => (ICON_WIFI, "Wi-Fi"),
        }
    }
}

impl Face for RadioTile {
    type Model = RadioReading;

    fn model(&self, cx: &mut gpui::App) -> Entity<RadioReading> {
        Reading::shared(cx)
    }

    fn follow(model: &Entity<RadioReading>, cx: &mut Context<TileView<Self>>) -> Subscription {
        Reading::follow(model, cx)
    }

    fn face(&self, model: &RadioReading, language: Language, _: &gpui::App) -> TileFace {
        let mut radios = radios_of(model)
            .iter()
            .filter(|radio| radio.kind == self.kind);
        let on = radios.clone().any(|radio| radio.on);
        let usable = radios.any(|radio| radio.switchable);
        let note = match usable {
            true => on_or_off(on, language),
            false => unusable_note(model.state().is_some(), language),
        };
        let (glyph, title) = self.glyph_and_title();
        TileFace {
            glyph,
            title: language.say(title).into(),
            note: note.into(),
            lit: on,
            usable,
        }
    }

    fn click(&self, model: &Entity<RadioReading>, cx: &mut gpui::App) {
        let kind = self.kind;
        model.update(cx, |reading, cx| {
            let radios = radios_of(reading).to_vec();
            let on = radios.iter().any(|radio| radio.kind == kind && radio.on);
            let shown = shown_as(&radios, |radio| radio.kind == kind, !on);
            reading.change(
                Some(Ok(shown)),
                move || Radios::switch(kind, None, !on),
                |(), _| {},
                cx,
            );
        });
    }
}

/// The kind of radio the option `radio` names: `wifi`, the default, or
/// `bluetooth`.
fn radio_option(named: Option<&str>) -> RadioType {
    match named {
        Some("bluetooth") => RadioType::Bluetooth,
        Some("wifi") | None => RadioType::Wifi,
        Some(other) => {
            report!("the radio `{other}` is unknown, `wifi` is used");
            RadioType::Wifi
        }
    }
}

fn glyph_of(kind: RadioType) -> &'static str {
    match kind {
        RadioType::Wifi => ICON_WIFI,
        RadioType::Bluetooth => ICON_BLUETOOTH,
        RadioType::MobileBroadband => ICON_MOBILE,
        RadioType::Fm | RadioType::Other => ICON_OTHER,
    }
}

/// What a radio is called where its driver gives no name.
fn kind_name(kind: RadioType) -> &'static str {
    match kind {
        RadioType::Wifi => "Wi-Fi",
        RadioType::Bluetooth => "Bluetooth",
        RadioType::MobileBroadband => "Mobile network",
        RadioType::Fm => "FM radio",
        RadioType::Other => "Radio",
    }
}

/// Every radio with a switch of its own.
struct RadioList {
    reading: Entity<RadioReading>,
    /// The switch of each row, in the order of the radios.
    switches: Vec<(Interaction, Spring)>,
    _follow: Subscription,
}

impl RadioList {
    fn new(cx: &mut Context<Self>) -> Self {
        let reading = Reading::<RadioSource>::shared(cx);
        Self {
            _follow: Reading::follow(&reading, cx),
            reading,
            switches: Vec::new(),
        }
    }

    /// Switches the radio at `index` over.
    fn flip(&mut self, index: usize, cx: &mut Context<Self>) {
        self.reading.update(cx, |reading, cx| {
            let radios = radios_of(reading).to_vec();
            let Some(radio) = radios.get(index).filter(|radio| radio.switchable).cloned() else {
                return;
            };
            let on = !radio.on;
            let shown = shown_as(&radios, |other| other == &radio, on);
            reading.change(
                Some(Ok(shown)),
                move || Radios::switch(radio.kind, Some(&radio.name), on),
                |(), _| {},
                cx,
            );
        });
    }
}

impl Render for RadioList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let reading = self.reading.read(cx);
        let frame = div().w_full().flex().flex_col().gap(theme.spacing.tiny);
        let text = match reading.state() {
            None => Some("Reading…"),
            Some(Err(_)) => Some("The radios cannot be read"),
            Some(Ok(radios)) if radios.is_empty() => Some("This machine has no radio"),
            Some(Ok(_)) => None,
        };
        if let Some(text) = text {
            return frame
                .px(theme.spacing.small)
                .text_color(theme.colors.on_surface_variant)
                .child(language.say(text))
                .into_any_element();
        }
        let radios = radios_of(reading).to_vec();
        let motion = theme.motion;
        self.switches.resize_with(radios.len(), || {
            (Interaction::new(&motion), Spring::new(motion.spatial, 0.))
        });
        let mut rows = Vec::new();
        for (index, radio) in radios.iter().enumerate() {
            let (interaction, travel) = &mut self.switches[index];
            travel.animate_to(if radio.on { 1. } else { 0. });
            let travel = travel.value(window);
            let state = interaction.state(window);
            let name = match radio.name.is_empty() {
                true => language.say(kind_name(radio.kind)).to_owned(),
                false => radio.name.clone(),
            };
            let note = match radio.switchable {
                true => on_or_off(radio.on, language),
                false => language.say("Switched off on the machine itself"),
            };
            let row = list_row(&theme, glyph_of(radio.kind), radio.on, name, note);
            if !radio.switchable {
                rows.push(row.into_any_element());
                continue;
            }
            let control = switch(&theme, ("radio", index), travel, state)
                .on_click(cx.listener(move |this, _, _, cx| this.flip(index, cx)));
            let control = Interaction::track(control, cx, move |this: &mut Self| {
                this.switches
                    .get_mut(index)
                    .map(|(interaction, _)| interaction)
            });
            rows.push(row.child(control).into_any_element());
        }
        frame.children(rows).into_any_element()
    }
}

pub struct AirplaneModule;

impl Module for AirplaneModule {
    fn name(&self) -> &'static str {
        "airplane"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        register_tile(widgets, "airplane", |_| Airplane)?;
        register_tile(widgets, "radio-tile", |context| RadioTile {
            kind: radio_option(context.text("radio")),
        })?;
        widgets.register_panel("radios", |_, cx| cx.new(RadioList::new).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio(kind: RadioType, on: bool, switchable: bool) -> RadioInfo {
        RadioInfo {
            kind,
            name: format!("{kind:?}"),
            on,
            switchable,
        }
    }

    #[test]
    fn a_tile_shows_at_once_the_radios_it_switched() {
        let radios = [
            radio(RadioType::Wifi, true, true),
            radio(RadioType::Bluetooth, true, true),
            radio(RadioType::MobileBroadband, true, false),
        ];
        let shown = shown_as(&radios, |radio| radio.kind == RadioType::Wifi, false);
        let on: Vec<bool> = shown.iter().map(|radio| radio.on).collect();
        assert_eq!(on, [false, true, true]);
        // A radio the machine itself switched stays as it is.
        let all = shown_as(&radios, |_| true, false);
        assert!(!airplane_mode(&all));
        assert!(all[2].on);
    }

    #[test]
    fn the_option_radio_names_wifi_or_bluetooth() {
        assert_eq!(radio_option(None), RadioType::Wifi);
        assert_eq!(radio_option(Some("bluetooth")), RadioType::Bluetooth);
        assert_eq!(radio_option(Some("wifi")), RadioType::Wifi);
    }
}
