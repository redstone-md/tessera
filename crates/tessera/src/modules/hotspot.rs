// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The mobile hotspot, for the quick settings. Widgets: `hotspot`, a tile
//! that starts and stops sharing the internet connection over Wi-Fi, and
//! `hotspot-details`, which it unfolds: the name of the network, its
//! password (shown on a click), how many devices are on it, the band to
//! send it on and a QR code a phone joins the network with, which holds
//! the password and so is drawn only while the password is shown (option
//! qr, false to leave it out). Both share one reading of the hotspot, asked
//! every three seconds while one is shown. The name and the password are
//! changed in the Windows settings, which the details lead to.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, div, px,
};

use super::quick::{
    Face, Reading, SettingsPage, Source, TileView, chip, on_or_off, register_tile, sentence,
    unusable_note,
};
use crate::i18n::Language;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::{Hotspot, HotspotBand, HotspotInfo, HotspotState};
use crate::theme::Theme;
use crate::ui::qr::{QrPicture, qr_code};
use crate::ui::{TileFace, icon, list_row, state_layer};

const ICON_HOTSPOT: &str = "\u{e704}";
const ICON_NETWORK: &str = "\u{e701}";
const ICON_PASSWORD: &str = "\u{e72e}";
const ICON_DEVICES: &str = "\u{e772}";
const ICON_SHOW: &str = "\u{e7b3}";
const ICON_HIDE: &str = "\u{ed1a}";

/// How wide and high the QR code is drawn.
const QR_SIDE: f32 = 168.;

/// What a hidden password is shown as.
const HIDDEN: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

const SETTINGS: SettingsPage = SettingsPage {
    address: "ms-settings:network-mobilehotspot",
    text: "Mobile hotspot settings",
};

/// The hotspot, as the quick settings read it.
pub struct HotspotSource;

impl Source for HotspotSource {
    type State = Result<HotspotInfo, &'static str>;

    const INTERVAL: Duration = Duration::from_secs(3);

    fn read() -> Self::State {
        Hotspot::read()
    }
}

type HotspotReading = Reading<HotspotSource>;

/// What the hotspot was last read to be, where it can be had.
fn info_of(reading: &HotspotReading) -> Option<&HotspotInfo> {
    reading.state()?.as_ref().ok()
}

/// The text a phone reads from the QR code to join the network, in the
/// form the Wi-Fi Alliance and the camera applications of phones know:
/// `WIFI:T:WPA;S:<name>;P:<password>;;`, with `\`, `;`, `,`, `"` and `:`
/// escaped by a backslash.
fn join_text(ssid: &str, passphrase: &str) -> String {
    let escaped = |text: &str| {
        let mut out = String::with_capacity(text.len());
        for letter in text.chars() {
            if matches!(letter, '\\' | ';' | ',' | '"' | ':') {
                out.push('\\');
            }
            out.push(letter);
        }
        out
    };
    match passphrase.is_empty() {
        true => format!("WIFI:T:nopass;S:{};;", escaped(ssid)),
        false => format!("WIFI:T:WPA;S:{};P:{};;", escaped(ssid), escaped(passphrase)),
    }
}

/// What a chip of a band says.
fn band_label(band: HotspotBand, language: Language) -> &'static str {
    match band {
        HotspotBand::Any => language.say("Any band"),
        HotspotBand::Ghz2 => "2.4 GHz",
        HotspotBand::Ghz5 => "5 GHz",
        HotspotBand::Ghz6 => "6 GHz",
    }
}

/// The tile of the hotspot.
struct HotspotTile;

impl Face for HotspotTile {
    type Model = HotspotReading;

    fn model(&self, cx: &mut App) -> Entity<HotspotReading> {
        Reading::shared(cx)
    }

    fn follow(model: &Entity<HotspotReading>, cx: &mut Context<TileView<Self>>) -> Subscription {
        Reading::follow(model, cx)
    }

    fn face(&self, model: &HotspotReading, language: Language, _: &App) -> TileFace {
        let info = info_of(model);
        let note = match info {
            None => unusable_note(model.state().is_some(), language).to_owned(),
            Some(info) => match info.state {
                HotspotState::Changing => language.say("Switching…").to_owned(),
                HotspotState::On if info.clients > 0 => {
                    format!("{}: {}", language.say("Devices"), info.clients)
                }
                state => on_or_off(state == HotspotState::On, language).to_owned(),
            },
        };
        TileFace {
            glyph: ICON_HOTSPOT,
            title: language.say("Mobile hotspot").into(),
            note: note.into(),
            lit: info.is_some_and(|info| info.state != HotspotState::Off),
            usable: info.is_some_and(|info| info.state != HotspotState::Changing),
        }
    }

    fn click(&self, model: &Entity<HotspotReading>, cx: &mut App) {
        model.update(cx, |reading, cx| {
            let Some(info) = info_of(reading).cloned() else {
                return;
            };
            let on = info.state == HotspotState::Off;
            let shown = HotspotInfo {
                state: HotspotState::Changing,
                ..info
            };
            reading.change(Some(Ok(shown)), move || Hotspot::switch(on), |(), _| {}, cx);
        });
    }
}

/// The details of the hotspot.
struct HotspotDetails {
    reading: Entity<HotspotReading>,
    /// Whether the QR code is shown, while the password is.
    qr: bool,
    /// Whether the password is shown instead of dots.
    revealed: bool,
    /// The code last drawn, and the text it holds.
    code: Option<(String, Rc<QrPicture>)>,
    reveal: Interaction,
    bands: Vec<Interaction>,
    settings: Interaction,
    _follow: Subscription,
}

impl HotspotDetails {
    fn new(context: &WidgetContext, cx: &mut Context<Self>) -> Self {
        let reading = Reading::<HotspotSource>::shared(cx);
        let motion = cx.global::<Theme>().motion;
        Self {
            _follow: Reading::follow(&reading, cx),
            reading,
            qr: context.flag("qr").unwrap_or(true),
            revealed: false,
            code: None,
            reveal: Interaction::new(&motion),
            bands: HotspotBand::ALL.map(|_| Interaction::new(&motion)).to_vec(),
            settings: Interaction::new(&motion),
        }
    }

    /// The QR code of `info`, encoded again only when its text changed.
    fn code(&mut self, info: &HotspotInfo) -> Option<Rc<QrPicture>> {
        let text = join_text(&info.ssid, &info.passphrase);
        if let Some((held, picture)) = &self.code
            && *held == text
        {
            return Some(picture.clone());
        }
        let picture = Rc::new(QrPicture::of(&text)?);
        self.code = Some((text, picture.clone()));
        Some(picture)
    }

    fn set_band(&mut self, band: HotspotBand, cx: &mut Context<Self>) {
        self.reading.update(cx, |reading, cx| {
            let Some(info) = info_of(reading).cloned() else {
                return;
            };
            if info.band == Some(band) {
                return;
            }
            let shown = HotspotInfo {
                band: Some(band),
                ..info
            };
            reading.change(
                Some(Ok(shown)),
                move || Hotspot::set_band(band),
                |(), _| {},
                cx,
            );
        });
    }

    fn band_chips(
        &mut self,
        info: &HotspotInfo,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let mut chips = Vec::new();
        for (index, band) in HotspotBand::ALL.into_iter().enumerate() {
            if !info.bands.contains(&band) {
                continue;
            }
            let state = self.bands[index].state(window);
            let chosen = info.band == Some(band);
            let shown = chip(
                &theme,
                ("band", index),
                band_label(band, language),
                chosen,
                state,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_band(band, cx)));
            let shown =
                Interaction::track(shown, cx, move |this: &mut Self| this.bands.get_mut(index));
            chips.push(shown.into_any_element());
        }
        div()
            .flex()
            .flex_wrap()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .children(chips)
            .into_any_element()
    }
}

impl Render for HotspotDetails {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let frame = div().w_full().flex().flex_col().gap(theme.spacing.small);
        let state = self.settings.state(window);
        let settings = SETTINGS.link(&theme, language, state);
        let settings = Interaction::track(settings, cx, |this: &mut Self| Some(&mut this.settings))
            .into_any_element();
        let info = match self.reading.read(cx).state() {
            None => Err("Reading…"),
            Some(Err(reason)) => Err(*reason),
            Some(Ok(info)) => Ok(info.clone()),
        };
        let info = match info {
            Ok(info) => info,
            Err(reason) => {
                let reason = div()
                    .px(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(sentence(language.say(reason)));
                return frame.child(reason).child(settings).into_any_element();
            }
        };
        let on = info.state == HotspotState::On;
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            list_row(
                &theme,
                ICON_NETWORK,
                on,
                info.ssid.clone(),
                language.say("Network name"),
            )
            .into_any_element(),
        );
        let password = match self.revealed {
            true => info.passphrase.clone(),
            false => HIDDEN.to_owned(),
        };
        let reveal_state = self.reveal.state(window);
        let reveal = div()
            .id("reveal")
            .flex_none()
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(
                &theme,
                theme.colors.surface_container_highest,
                reveal_state,
            ))
            .child(icon(
                &theme,
                if self.revealed { ICON_HIDE } else { ICON_SHOW },
            ))
            .on_click(cx.listener(|this, _, _, cx| {
                this.revealed = !this.revealed;
                cx.notify();
            }));
        let reveal = Interaction::track(reveal, cx, |this: &mut Self| Some(&mut this.reveal));
        rows.push(
            list_row(
                &theme,
                ICON_PASSWORD,
                false,
                password,
                language.say("Password"),
            )
            .child(reveal)
            .into_any_element(),
        );
        let devices = match info.most_clients {
            0 => info.clients.to_string(),
            most => format!("{} / {most}", info.clients),
        };
        rows.push(
            list_row(
                &theme,
                ICON_DEVICES,
                info.clients > 0,
                devices,
                language.say("Devices on it"),
            )
            .into_any_element(),
        );
        if info.bands.len() > 1 {
            rows.push(self.band_chips(&info, window, cx));
        }
        // The code holds the password, so it is hidden along with it.
        if self.qr
            && self.revealed
            && let Some(picture) = self.code(&info)
        {
            rows.push(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(theme.spacing.tiny)
                    .text_color(theme.colors.on_surface_variant)
                    .child(qr_code(picture, px(QR_SIDE)))
                    .child(language.say("Scan with a phone to join"))
                    .into_any_element(),
            );
        }
        rows.push(settings);
        frame.children(rows).into_any_element()
    }
}

pub struct HotspotModule;

impl Module for HotspotModule {
    fn name(&self) -> &'static str {
        "hotspot"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        register_tile(widgets, "hotspot", |_| HotspotTile)?;
        widgets.register_panel("hotspot-details", |context, cx| {
            let context = context.clone();
            cx.new(|cx| HotspotDetails::new(&context, cx)).into()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_is_told_the_network_in_the_wifi_form() {
        assert_eq!(
            join_text("Home", "secret12"),
            "WIFI:T:WPA;S:Home;P:secret12;;"
        );
        assert_eq!(
            join_text("My;Net", r#"a:b,c"d\e"#),
            r#"WIFI:T:WPA;S:My\;Net;P:a\:b\,c\"d\\e;;"#
        );
        assert_eq!(join_text("Open", ""), "WIFI:T:nopass;S:Open;;");
    }

    #[test]
    fn the_code_of_a_network_can_be_drawn() {
        let picture =
            QrPicture::of(&join_text("Tessera hotspot", "correct horse battery")).unwrap();
        assert!(picture.side() >= 21);
    }
}
