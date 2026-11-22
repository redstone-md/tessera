// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine. Widgets: `bluetooth`, a
//! list for a popup, and `bluetooth-button`, an icon that is there only on
//! a machine with Bluetooth.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Spring, staggered};
use crate::platform::{Bluetooth, PairedDevice, Wireless};
use crate::theme::Theme;
use crate::ui::{icon, list_row, pill, wave_bar};

/// How often the machine is asked whether it has Bluetooth: an adapter may
/// be plugged in and pulled out.
const PRESENCE_INTERVAL: Duration = Duration::from_secs(30);

/// How often the icon asks whether the radio is on.
const RADIO_INTERVAL: Duration = Duration::from_secs(5);

/// How many times the radio is asked for each time the machine is asked
/// whether it has Bluetooth.
const RADIO_READS: u32 = (PRESENCE_INTERVAL.as_secs() / RADIO_INTERVAL.as_secs()) as u32;

/// How often the list is read while it is shown.
const READ_INTERVAL: Duration = Duration::from_secs(5);

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// How much of a device shows while it is not connected.
const AWAY_OPACITY: f32 = 0.6;

/// How wide the line that shows a charge is.
const CHARGE_WIDTH: Pixels = px(48.);

const ICON_BLUETOOTH: &str = "\u{e702}";

pub struct BluetoothModule;

impl Module for BluetoothModule {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("bluetooth", |_, cx| cx.new(Devices::new).into())?;
        widgets.register("bluetooth-button", |_, cx| cx.new(Button::new).into())
    }
}

/// The Bluetooth icon, on a machine that has Bluetooth; paler while the
/// radio is off.
struct Button {
    /// 0 while the machine has no Bluetooth, 1 while the icon is shown.
    presence: Spring,
    /// 0 while the radio is off, 1 while it is on.
    power: Spring,
}

impl Button {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut present = false;
            for round in 0u32.. {
                let ask = round % RADIO_READS == 0;
                let read = cx.background_executor().spawn(async move {
                    let present = if ask { Bluetooth::present() } else { present };
                    // A radio that does not say is shown as one that is on.
                    (present, Wireless::Bluetooth.is_on() != Some(false))
                });
                let on;
                (present, on) = read.await;
                let shown = this.update(cx, |this, cx| {
                    this.presence.animate_to(if present { 1. } else { 0. });
                    this.power.animate_to(if on { 1. } else { 0. });
                    cx.notify();
                });
                if shown.is_err() {
                    break;
                }
                cx.background_executor().timer(RADIO_INTERVAL).await;
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            presence: Spring::new(motion.spatial, 0.),
            power: Spring::new(motion.spatial, 1.),
        }
    }
}

impl Render for Button {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window).clamp(0., 1.);
        if presence == 0. {
            return div().into_any_element();
        }
        let power = self.power.value(window).clamp(0., 1.);
        pill(&theme)
            .opacity(presence)
            .child(
                div()
                    .opacity(AWAY_OPACITY + (1. - AWAY_OPACITY) * power)
                    .child(icon(&theme, ICON_BLUETOOTH)),
            )
            .into_any_element()
    }
}

/// What a row says under the name of `device`.
fn note(device: &PairedDevice, language: Language) -> String {
    let state = language.say(if device.connected {
        "Connected"
    } else {
        "Not connected"
    });
    match (device.battery, device.connected) {
        (Some(charge), true) => format!("{state}, {charge}%"),
        (Some(charge), false) => format!("{state}, {} {charge}%", language.say("was")),
        (None, _) => state.to_owned(),
    }
}

/// Lists the devices and follows them while it is shown.
struct Devices {
    /// `None` until the first reading.
    devices: Option<Result<Vec<PairedDevice>, String>>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
}

impl Devices {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let read = cx
                    .background_executor()
                    .spawn(async { Bluetooth::devices() })
                    .await;
                if this.update(cx, |this, cx| this.take(read, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        })
        .detach();
        Self {
            devices: None,
            arrival: Spring::new(cx.global::<Theme>().motion.spatial, 0.),
        }
    }

    fn take(&mut self, read: Result<Vec<PairedDevice>, String>, cx: &mut Context<Self>) {
        if self.devices.as_ref() == Some(&read) {
            return;
        }
        // Devices that come and go within a list do not start it over.
        let kind = |devices: &Option<Result<Vec<PairedDevice>, String>>| match devices {
            None => 0,
            Some(Err(_)) => 1,
            Some(Ok(devices)) if devices.is_empty() => 2,
            Some(Ok(_)) => 3,
        };
        let read = Some(read);
        if kind(&read) != kind(&self.devices) {
            self.arrival.jump_to(0.);
            self.arrival.animate_to(1.);
        }
        self.devices = read;
        cx.notify();
    }

    /// One line of text that stands for the whole list.
    fn message(&mut self, theme: &Theme, text: &str, window: &mut Window) -> Div {
        let arrived = self.arrival.value(window).clamp(0., 1.);
        div()
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .text_color(theme.colors.on_surface_variant)
            .child(SharedString::from(text.to_owned()))
    }

    fn row(
        &mut self,
        index: usize,
        count: usize,
        device: &PairedDevice,
        theme: &Theme,
        language: Language,
        window: &mut Window,
    ) -> AnyElement {
        let arrived = staggered(self.arrival.value(window).clamp(0., 1.), index, count);
        let presence = if device.connected { 1. } else { AWAY_OPACITY };
        list_row(
            theme,
            ICON_BLUETOOTH,
            device.connected,
            device.name.clone(),
            note(device, language),
        )
        .relative()
        .top(px(ROW_TRAVEL * (1. - arrived)))
        .opacity(arrived * presence)
        .children(device.battery.map(|charge| {
            // The line fills as the row arrives.
            let level = f32::from(charge) / 100. * arrived;
            div()
                .flex_none()
                .w(CHARGE_WIDTH)
                .child(wave_bar(theme, level))
        }))
        .into_any_element()
    }
}

impl Render for Devices {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let frame = div()
            .id("bluetooth-list")
            .w_full()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll();
        let devices = match self.devices.clone() {
            None => {
                let text = language.say("Looking for devices");
                return frame.child(self.message(&theme, text, window));
            }
            Some(Err(error)) => return frame.child(self.message(&theme, &error, window)),
            Some(Ok(devices)) => devices,
        };
        if devices.is_empty() {
            let text = language.say("No Bluetooth device is paired");
            return frame.child(self.message(&theme, text, window));
        }
        let count = devices.len();
        let rows: Vec<AnyElement> = devices
            .iter()
            .enumerate()
            .map(|(index, device)| self.row(index, count, device, &theme, language, window))
            .collect();
        frame.children(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(connected: bool, battery: Option<u8>) -> PairedDevice {
        PairedDevice {
            name: "Mouse".to_owned(),
            connected,
            battery,
        }
    }

    #[test]
    fn a_row_says_whether_it_is_connected_and_how_charged() {
        assert_eq!(note(&device(true, None), Language::English), "Connected");
        assert_eq!(
            note(&device(false, None), Language::English),
            "Not connected"
        );
        assert_eq!(
            note(&device(true, Some(80)), Language::English),
            "Connected, 80%"
        );
        assert_eq!(
            note(&device(false, Some(80)), Language::English),
            "Not connected, was 80%"
        );
        assert_eq!(
            note(&device(false, Some(80)), Language::Russian),
            "Не подключено, было 80%"
        );
    }
}
