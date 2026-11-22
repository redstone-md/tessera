// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine. Widgets: `bluetooth`, a
//! list for a popup, in which a click connects or disconnects a device
//! that sound goes through, and `bluetooth-button`, an icon that is there
//! only on a machine with Bluetooth.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, InteractiveElement, IntoElement, ParentElement,
    Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::platform::{Bluetooth, PairedDevice, Wireless};
use crate::theme::Theme;
use crate::ui::{icon, list_row, pill, state_layer, wave_bar};

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

/// How often the list is read while a device was asked to connect or to
/// disconnect and has not done so yet.
const ASKED_INTERVAL: Duration = Duration::from_secs(1);

/// How many readings a device that was asked is waited for.
const READS_TO_ANSWER: u32 = 20;

/// How far the row of a device that was asked goes towards the color of a
/// connected one while it is waited for.
const ASKED_TINT: f32 = 0.5;

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

/// What a row says under the name of `device`; `asked` is what the device
/// was asked for and has not done yet: to connect, or to disconnect.
fn note(device: &PairedDevice, asked: Option<bool>, language: Language) -> String {
    let state = language.say(match (asked, device.connected) {
        (Some(true), false) => "Connecting",
        (Some(false), true) => "Disconnecting",
        (_, true) => "Connected",
        (_, false) => "Not connected",
    });
    match (device.battery, device.connected) {
        (Some(charge), true) => format!("{state}, {charge}%"),
        (Some(charge), false) => format!("{state}, {} {charge}%", language.say("was")),
        (None, _) => state.to_owned(),
    }
}

/// Lists the devices and follows them while it is shown.
/// A device that was asked to connect or to disconnect and has not yet.
struct Asked {
    /// What Windows holds the device together by.
    container: u128,
    connect: bool,
    /// How many readings it has been waited for.
    reads: u32,
}

struct Devices {
    /// `None` until the first reading.
    devices: Option<Result<Vec<PairedDevice>, String>>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
    /// One for each row.
    rows: Vec<Interaction>,
    asked: Option<Asked>,
    /// 1 while a device is waited for, 0 otherwise.
    waiting: Spring,
}

impl Devices {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let read = cx
                    .background_executor()
                    .spawn(async { Bluetooth::devices() })
                    .await;
                let Ok(waiting) = this.update(cx, |this, cx| {
                    this.take(read, cx);
                    this.asked.is_some()
                }) else {
                    break;
                };
                let interval = match waiting {
                    true => ASKED_INTERVAL,
                    false => READ_INTERVAL,
                };
                cx.background_executor().timer(interval).await;
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            devices: None,
            arrival: Spring::new(motion.spatial, 0.),
            rows: Vec::new(),
            asked: None,
            waiting: Spring::new(motion.effects, 0.),
        }
    }

    /// Asks `device` to connect if it is not connected, and to disconnect
    /// if it is.
    fn clicked(&mut self, device: &PairedDevice, cx: &mut Context<Self>) {
        let Some(container) = device.audio else {
            return;
        };
        if self.asked.is_some() {
            return;
        }
        let connect = !device.connected;
        self.asked = Some(Asked {
            container,
            connect,
            reads: 0,
        });
        self.waiting.animate_to(1.);
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let asked = cx
                .background_executor()
                .spawn(async move { Bluetooth::link(container, connect) })
                .await;
            if let Err(error) = asked {
                report!("a Bluetooth device was not asked: {error}");
                let _ = this.update(cx, |this, cx| this.answered(cx));
            }
        })
        .detach();
    }

    /// Stops waiting for the device that was asked.
    fn answered(&mut self, cx: &mut Context<Self>) {
        self.asked = None;
        self.waiting.animate_to(0.);
        cx.notify();
    }

    fn take(&mut self, read: Result<Vec<PairedDevice>, String>, cx: &mut Context<Self>) {
        if let Some(asked) = &mut self.asked {
            asked.reads += 1;
            let listed = read.as_deref().unwrap_or_default();
            let device = listed
                .iter()
                .find(|device| device.audio == Some(asked.container));
            let done = device.is_none_or(|device| device.connected == asked.connect);
            if done || asked.reads >= READS_TO_ANSWER {
                self.answered(cx);
            }
        }
        if self.devices.as_ref() == Some(&read) {
            return;
        }
        let count = read.as_ref().map_or(0, Vec::len);
        let motion = cx.global::<Theme>().motion;
        self.rows.resize_with(count, || Interaction::new(&motion));
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let arrived = staggered(self.arrival.value(window).clamp(0., 1.), index, count);
        let asked = self.asked.as_ref();
        let asked = asked.filter(|asked| device.audio == Some(asked.container));
        let asked = asked.map(|asked| asked.connect);
        let waiting = match asked {
            Some(_) => self.waiting.value(window).clamp(0., 1.),
            None => 0.,
        };
        let presence = if device.connected {
            1.
        } else {
            AWAY_OPACITY + (1. - AWAY_OPACITY) * waiting
        };
        let colors = theme.colors;
        let base = if device.connected {
            colors.secondary_container
        } else {
            let tint = ASKED_TINT * waiting;
            mix(
                colors.surface_container_highest,
                colors.secondary_container,
                tint,
            )
        };
        let row = list_row(
            theme,
            ICON_BLUETOOTH,
            device.connected,
            device.name.clone(),
            note(device, asked, language),
        )
        .id(("bluetooth-row", index))
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
        }));
        if device.audio.is_none() {
            return row.into_any_element();
        }
        let row = match self.rows.get_mut(index) {
            Some(interaction) => row.bg(state_layer(theme, base, interaction.state(window))),
            None => row,
        };
        let row = Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index));
        let clicked = device.clone();
        row.on_click(cx.listener(move |this, _, _, cx| this.clicked(&clicked, cx)))
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
            .map(|(index, device)| self.row(index, count, device, &theme, window, cx))
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
            audio: None,
        }
    }

    #[test]
    fn a_row_says_whether_it_is_connected_and_how_charged() {
        assert_eq!(
            note(&device(true, None), None, Language::English),
            "Connected"
        );
        assert_eq!(
            note(&device(false, None), None, Language::English),
            "Not connected"
        );
        assert_eq!(
            note(&device(true, Some(80)), None, Language::English),
            "Connected, 80%"
        );
        assert_eq!(
            note(&device(false, Some(80)), None, Language::English),
            "Not connected, was 80%"
        );
        assert_eq!(
            note(&device(false, Some(80)), None, Language::Russian),
            "Не подключено, было 80%"
        );
    }

    #[test]
    fn a_row_says_what_its_device_was_asked_for_until_it_does_it() {
        let english = Language::English;
        assert_eq!(
            note(&device(false, None), Some(true), english),
            "Connecting"
        );
        assert_eq!(note(&device(true, None), Some(true), english), "Connected");
        assert_eq!(
            note(&device(true, Some(80)), Some(false), english),
            "Disconnecting, 80%"
        );
        assert_eq!(
            note(&device(false, None), Some(false), english),
            "Not connected"
        );
    }
}
