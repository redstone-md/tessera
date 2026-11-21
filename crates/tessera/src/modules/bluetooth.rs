// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine. Widget: `bluetooth`, a
//! list for a popup.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Spring, staggered};
use crate::platform::{Bluetooth, PairedDevice};
use crate::theme::Theme;
use crate::ui::{icon, wave_bar};

/// How often the list is read while it is shown.
const READ_INTERVAL: Duration = Duration::from_secs(5);

/// How high a row is.
const ROW_HEIGHT: Pixels = px(44.);

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
        widgets.register("bluetooth", |_, cx| cx.new(Devices::new).into())
    }
}

/// What a row says under the name of `device`.
fn note(device: &PairedDevice) -> String {
    let state = if device.connected {
        "Connected"
    } else {
        "Not connected"
    };
    match (device.battery, device.connected) {
        (Some(charge), true) => format!("{state}, {charge}%"),
        (Some(charge), false) => format!("{state}, was {charge}%"),
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
        window: &mut Window,
    ) -> AnyElement {
        let arrived = staggered(self.arrival.value(window).clamp(0., 1.), index, count);
        let presence = if device.connected { 1. } else { AWAY_OPACITY };
        div()
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived * presence)
            .h(ROW_HEIGHT)
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(if device.connected {
                theme.colors.secondary_container
            } else {
                theme.colors.surface_container_highest
            })
            .child(
                icon(theme, ICON_BLUETOOTH)
                    .flex_none()
                    .text_color(if device.connected {
                        theme.colors.primary
                    } else {
                        theme.colors.on_surface_variant
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(SharedString::from(device.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.colors.on_surface_variant)
                            .child(SharedString::from(note(device))),
                    ),
            )
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
        let frame = div()
            .id("bluetooth-list")
            .size_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .overflow_y_scroll();
        let devices = match self.devices.clone() {
            None => return frame.child(self.message(&theme, "Looking for devices", window)),
            Some(Err(error)) => return frame.child(self.message(&theme, &error, window)),
            Some(Ok(devices)) => devices,
        };
        if devices.is_empty() {
            return frame.child(self.message(&theme, "No Bluetooth device is paired", window));
        }
        let count = devices.len();
        let rows: Vec<AnyElement> = devices
            .iter()
            .enumerate()
            .map(|(index, device)| self.row(index, count, device, &theme, window))
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
        assert_eq!(note(&device(true, None)), "Connected");
        assert_eq!(note(&device(false, None)), "Not connected");
        assert_eq!(note(&device(true, Some(80))), "Connected, 80%");
        assert_eq!(note(&device(false, Some(80))), "Not connected, was 80%");
    }
}
