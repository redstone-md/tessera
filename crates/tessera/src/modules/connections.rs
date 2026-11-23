// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the machine is connected through. Widgets: `connections`, the
//! established connections and the VPN connections of Windows, for a
//! popup, and `radio-switch`, which switches Wi-Fi or Bluetooth.

use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};

use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{Carrier, Connection, Shortcut, Vpn, VpnState, Wireless};
use crate::theme::Theme;
use crate::ui::{icon, list_row, list_title, one_line, state_layer, switch};

/// How often what is shown is read again.
const READ_INTERVAL: Duration = Duration::from_secs(3);

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// How high the row of a switch and the row that opens the settings are.
const SLIM_ROW: f32 = 40.;

/// A page of the Windows settings a row leads to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Network,
    Vpn,
}

impl Page {
    /// What Windows opens the page by.
    fn address(self) -> &'static str {
        match self {
            Self::Network => "ms-settings:network",
            Self::Vpn => "ms-settings:network-vpn",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Network => ICON_SETTINGS,
            Self::Vpn => ICON_VPN,
        }
    }

    /// What the row that leads to it says.
    fn text(self) -> &'static str {
        match self {
            Self::Network => "Network settings",
            Self::Vpn => "Add a VPN in the Windows settings",
        }
    }
}

const ICON_CABLE: &str = "\u{e839}";
const ICON_WIFI: &str = "\u{e701}";
const ICON_OTHER: &str = "\u{e774}";
const ICON_VPN: &str = "\u{e705}";
const ICON_SETTINGS: &str = "\u{e713}";
const ICON_BLUETOOTH: &str = "\u{e702}";
const ICON_OPEN: &str = "\u{e76c}";

pub struct ConnectionsModule;

impl Module for ConnectionsModule {
    fn name(&self) -> &'static str {
        "connections"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("connections", |_, cx| cx.new(Links::new).into())?;
        widgets.register_panel("radio-switch", |context, cx| {
            let radio = match context.text("radio") {
                Some("bluetooth") => Wireless::Bluetooth,
                Some("wifi") | None => Wireless::Wifi,
                Some(other) => {
                    report!("the radio `{other}` is unknown, `wifi` is used");
                    Wireless::Wifi
                }
            };
            cx.new(|cx| RadioSwitch::new(radio, cx)).into()
        })
    }
}

/// Opens the page `page` of the Windows settings.
fn open_settings(page: &str) {
    let page = Shortcut {
        name: page.to_owned(),
        path: PathBuf::from(page),
    };
    page.launch();
}

/// How fast a connection that sends `bits` a second is, in words.
fn speed(bits: u64) -> String {
    let (amount, unit) = if bits >= 1_000_000_000 {
        (bits as f64 / 1e9, "Gbit/s")
    } else {
        (bits as f64 / 1e6, "Mbit/s")
    };
    if (amount - amount.round()).abs() < 0.05 {
        format!("{amount:.0} {unit}")
    } else {
        format!("{amount:.1} {unit}")
    }
}

/// What a row says under the name of `connection`.
fn note(connection: &Connection, language: Language) -> String {
    let reach = if connection.internet {
        "Internet"
    } else {
        "No internet"
    };
    let mut parts = vec![language.say(reach).to_owned()];
    parts.extend(connection.address.clone());
    parts.extend(connection.speed.map(speed));
    parts.join(" \u{b7} ")
}

/// What a row says under the name of a VPN in `state`.
fn vpn_note(state: VpnState) -> &'static str {
    match state {
        VpnState::Off => "Not connected",
        VpnState::Connecting => "Connecting",
        VpnState::On => "Connected",
        VpnState::Disconnecting => "Disconnecting",
    }
}

/// What was read at one time.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Reading {
    connections: Result<Vec<Connection>, String>,
    vpns: Vec<Vpn>,
}

impl Reading {
    fn take() -> Self {
        let vpns = Vpn::kept().unwrap_or_else(|error| {
            report!("{error}");
            Vec::new()
        });
        Self {
            connections: Connection::established(),
            vpns,
        }
    }
}

/// Lists the connections and follows them while it is shown.
struct Links {
    /// `None` until the first reading.
    reading: Option<Reading>,
    /// 0 when the first reading arrives, 1 once it is in place.
    arrival: Spring,
    vpns: Vec<Interaction>,
    add_vpn: Interaction,
    settings: Interaction,
}

impl Links {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let read = cx.background_executor().spawn(async { Reading::take() });
                let read = read.await;
                if this.update(cx, |this, cx| this.take(read, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            reading: None,
            arrival: Spring::new(motion.spatial, 0.),
            vpns: Vec::new(),
            add_vpn: Interaction::new(&motion),
            settings: Interaction::new(&motion),
        }
    }

    fn take(&mut self, read: Reading, cx: &mut Context<Self>) {
        if self.reading.as_ref() == Some(&read) {
            return;
        }
        if self.reading.is_none() {
            self.arrival.animate_to(1.);
        }
        let motion = cx.global::<Theme>().motion;
        self.vpns
            .resize_with(read.vpns.len(), || Interaction::new(&motion));
        self.reading = Some(read);
        cx.notify();
    }

    /// Connects or disconnects the VPN `name`, and reads again after.
    fn switch_vpn(&mut self, name: String, on: bool, cx: &mut Context<Self>) {
        if let Some(reading) = &mut self.reading
            && let Some(vpn) = reading.vpns.iter_mut().find(|vpn| vpn.name == name)
        {
            vpn.state = if on {
                VpnState::Connecting
            } else {
                VpnState::Disconnecting
            };
            cx.notify();
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async move {
                if let Err(error) = Vpn::switch(&name, on) {
                    report!("{error}");
                }
                Reading::take()
            });
            let read = read.await;
            this.update(cx, |this, cx| this.take(read, cx)).ok();
        })
        .detach();
    }

    /// A row that opens `page` of the Windows settings.
    fn settings_row(
        &mut self,
        page: Page,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let vpn = page == Page::Vpn;
        let interaction = if vpn {
            &mut self.add_vpn
        } else {
            &mut self.settings
        };
        let state = interaction.state(window);
        let row = div()
            .id(page.address())
            .h(px(SLIM_ROW))
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(state_layer(theme, theme.colors.surface_container, state))
            .text_color(theme.colors.on_surface_variant)
            .child(icon(theme, page.glyph()).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(one_line(Language::of(cx).say(page.text()))),
            )
            .child(icon(theme, ICON_OPEN).flex_none())
            .on_click(move |_, _, _| open_settings(page.address()));
        Interaction::track(row, cx, move |this: &mut Self| {
            Some(if vpn {
                &mut this.add_vpn
            } else {
                &mut this.settings
            })
        })
        .into_any_element()
    }
}

impl Render for Links {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let frame = div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.small);
        let Some(reading) = self.reading.clone() else {
            return frame;
        };
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let connections = reading.connections.clone().unwrap_or_default();
        let count = connections.len() + reading.vpns.len().max(1) + 3;
        let mut place = 0;
        // Each thing below arrives a little after the one above it.
        let mut arrive = |element: AnyElement| {
            let arrived = staggered(arrival, place, count);
            place += 1;
            div()
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrived)))
                .opacity(arrived)
                .child(element)
        };
        let mut rows = Vec::new();
        rows.push(arrive(
            list_title(&theme, language.say("Connections")).into_any_element(),
        ));
        if let Err(error) = &reading.connections {
            rows.push(arrive(list_title(&theme, error.clone()).into_any_element()));
        } else if connections.is_empty() {
            let text = language.say("Nothing is connected");
            rows.push(arrive(
                list_title(&theme, text)
                    .font_weight(FontWeight::NORMAL)
                    .into_any_element(),
            ));
        }
        for connection in &connections {
            let glyph = match connection.carrier {
                Carrier::Cable => ICON_CABLE,
                Carrier::Wifi => ICON_WIFI,
                Carrier::Other => ICON_OTHER,
            };
            let row = list_row(
                &theme,
                glyph,
                connection.internet,
                connection.name.clone(),
                note(connection, language),
            );
            rows.push(arrive(row.into_any_element()));
        }
        rows.push(arrive(list_title(&theme, "VPN").into_any_element()));
        for (index, vpn) in reading.vpns.iter().enumerate() {
            let state = self.vpns[index].state(window);
            let on = matches!(vpn.state, VpnState::On | VpnState::Connecting);
            let lit = vpn.state == VpnState::On;
            let ground = if lit {
                theme.colors.secondary_container
            } else {
                theme.colors.surface_container_highest
            };
            let name = vpn.name.clone();
            let row = list_row(
                &theme,
                ICON_VPN,
                lit,
                vpn.name.clone(),
                language.say(vpn_note(vpn.state)),
            )
            .id(("vpn-row", index))
            .bg(state_layer(&theme, ground, state))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.switch_vpn(name.clone(), !on, cx);
            }));
            let row = Interaction::track(row, cx, move |this: &mut Self| this.vpns.get_mut(index));
            rows.push(arrive(row.into_any_element()));
        }
        if reading.vpns.is_empty() {
            let row = self.settings_row(Page::Vpn, &theme, window, cx);
            rows.push(arrive(row));
        }
        let row = self.settings_row(Page::Network, &theme, window, cx);
        rows.push(arrive(row));
        frame.children(rows)
    }
}

/// The name of a radio and a switch for it; nothing on a machine without
/// the radio.
struct RadioSwitch {
    radio: Wireless,
    /// `None` until the radio said, and on a machine without it.
    on: Option<bool>,
    /// 0 while off, 1 while on.
    travel: Spring,
    /// 0 until the radio said, 1 once the row is in place.
    arrival: Spring,
    interaction: Interaction,
    /// Whether the radio is being switched now.
    busy: bool,
}

impl RadioSwitch {
    fn new(radio: Wireless, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let read = cx
                    .background_executor()
                    .spawn(async move { radio.is_on() })
                    .await;
                if this.update(cx, |this, cx| this.take(read, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        Self {
            radio,
            on: None,
            travel: Spring::new(motion.spatial, 0.),
            arrival: Spring::new(motion.spatial, 0.),
            interaction: Interaction::new(&motion),
            busy: false,
        }
    }

    fn take(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        if self.busy || self.on == on {
            return;
        }
        if self.on.is_none() {
            // The switch is where it belongs when the row arrives.
            self.travel.jump_to(if on == Some(true) { 1. } else { 0. });
        }
        self.show(on, cx);
    }

    fn show(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        self.on = on;
        self.arrival.animate_to(if on.is_some() { 1. } else { 0. });
        self.travel
            .animate_to(if on == Some(true) { 1. } else { 0. });
        cx.notify();
    }

    /// Switches the radio over, and shows what it says after.
    fn flip(&mut self, cx: &mut Context<Self>) {
        let Some(on) = self.on else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.show(Some(!on), cx);
        let radio = self.radio;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let read = cx.background_executor().spawn(async move {
                if let Err(error) = radio.switch(!on) {
                    report!("{error}");
                }
                radio.is_on()
            });
            let read = read.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.show(read, cx);
            })
            .ok();
        })
        .detach();
    }
}

impl Render for RadioSwitch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let arrived = self.arrival.value(window).clamp(0., 1.);
        if arrived == 0. {
            return div().into_any_element();
        }
        let (glyph, name) = match self.radio {
            Wireless::Wifi => (ICON_WIFI, "Wi-Fi"),
            Wireless::Bluetooth => (ICON_BLUETOOTH, "Bluetooth"),
        };
        let travel = self.travel.value(window);
        let state = self.interaction.state(window);
        let control = switch(&theme, "radio-switch", travel, state)
            .on_click(cx.listener(|this, _, _, cx| this.flip(cx)));
        let control =
            Interaction::track(control, cx, |this: &mut Self| Some(&mut this.interaction));
        div()
            .h(px(SLIM_ROW))
            .flex_none()
            .w_full()
            .px(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .opacity(arrived)
            .child(icon(&theme, glyph).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(one_line(name)),
            )
            .child(control)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_speed_is_said_in_the_unit_that_fits_it() {
        assert_eq!(speed(1_000_000_000), "1 Gbit/s");
        assert_eq!(speed(2_500_000_000), "2.5 Gbit/s");
        assert_eq!(speed(100_000_000), "100 Mbit/s");
        assert_eq!(speed(866_700_000), "866.7 Mbit/s");
    }

    #[test]
    fn a_row_says_what_the_connection_reaches_and_how() {
        let connection = Connection {
            name: "Home".to_owned(),
            carrier: Carrier::Cable,
            internet: true,
            address: Some("192.0.2.7".to_owned()),
            speed: Some(1_000_000_000),
        };
        assert_eq!(
            note(&connection, Language::English),
            "Internet \u{b7} 192.0.2.7 \u{b7} 1 Gbit/s"
        );
        let bare = Connection {
            internet: false,
            address: None,
            speed: None,
            ..connection
        };
        assert_eq!(note(&bare, Language::English), "No internet");
    }
}
