// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Wi-Fi networks in reach. Widget: `wifi`, a list for a popup, in
//! which a network Windows has the settings of is joined with a click.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{Wifi, WifiNetwork, WifiReach};
use crate::theme::Theme;
use crate::ui::{icon, state_layer};

/// How often the list is read while it is shown.
const READ_INTERVAL: Duration = Duration::from_secs(3);

/// How many readings pass between the times the adapters are asked to
/// look anew.
const READS_PER_SCAN: u32 = 5;

/// How high a row is.
const ROW_HEIGHT: Pixels = px(40.);

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// How much of a row shows while it cannot be joined here.
const UNKNOWN_OPACITY: f32 = 0.6;

/// Wi-Fi with one to four arcs.
const ICONS_SIGNAL: [&str; 4] = ["\u{e872}", "\u{e873}", "\u{e874}", "\u{e701}"];
const ICON_LOCK: &str = "\u{e72e}";
const ICON_CONNECTED: &str = "\u{e73e}";

pub struct WifiModule;

impl Module for WifiModule {
    fn name(&self) -> &'static str {
        "wifi"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("wifi", |_, cx| cx.new(WifiList::new).into())
    }
}

/// The icon for a signal of `bars`, of which Windows counts up to five.
fn signal_glyph(bars: u8) -> &'static str {
    ICONS_SIGNAL[usize::from(bars.clamp(1, 5) - 1) * ICONS_SIGNAL.len() / 5]
}

/// What a row says under the name of `network`; `joining` is the network
/// being joined.
fn note(network: &WifiNetwork, joining: Option<&str>) -> &'static str {
    if network.connected {
        "Connected"
    } else if joining == Some(network.name.as_str()) {
        "Joining"
    } else if network.profile.is_some() {
        "Saved"
    } else if network.secured {
        "Not saved; join it in the Windows settings first"
    } else {
        "Open; join it in the Windows settings first"
    }
}

/// Lists the networks and follows them while it is shown.
struct WifiList {
    /// `None` until the first reading.
    reach: Option<Result<WifiReach, String>>,
    /// The network a click asked to join, until it is connected.
    joining: Option<String>,
    /// Why the last attempt to join failed.
    failure: Option<String>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
    rows: Vec<Interaction>,
}

impl WifiList {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut reads = 0;
            loop {
                let scan = reads % READS_PER_SCAN == 0;
                reads += 1;
                let read = cx.background_executor().spawn(async move {
                    if scan && let Err(error) = Wifi::scan() {
                        report!("{error}");
                    }
                    Wifi::reach()
                });
                let read = read.await;
                if this.update(cx, |this, cx| this.take(read, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        })
        .detach();
        Self {
            reach: None,
            joining: None,
            failure: None,
            arrival: Spring::new(cx.global::<Theme>().motion.spatial, 0.),
            rows: Vec::new(),
        }
    }

    fn take(&mut self, read: Result<WifiReach, String>, cx: &mut Context<Self>) {
        if self.reach.as_ref() == Some(&read) {
            return;
        }
        let networks = match &read {
            Ok(WifiReach::Networks(networks)) => networks.as_slice(),
            _ => &[],
        };
        if let Some(joining) = &self.joining
            && networks
                .iter()
                .any(|network| network.connected && &network.name == joining)
        {
            self.joining = None;
        }
        // Rows that come and go within a list do not start it over.
        let kind = |reach: &Option<Result<WifiReach, String>>| match reach {
            None => 0,
            Some(Err(_)) => 1,
            Some(Ok(WifiReach::NoAdapter)) => 2,
            Some(Ok(WifiReach::Off)) => 3,
            Some(Ok(WifiReach::Networks(_))) => 4,
        };
        let count = networks.len();
        let read = Some(read);
        if kind(&read) != kind(&self.reach) {
            self.arrival.jump_to(0.);
            self.arrival.animate_to(1.);
        }
        let motion = cx.global::<Theme>().motion;
        self.rows.resize_with(count, || Interaction::new(&motion));
        self.reach = read;
        cx.notify();
    }

    /// Asks Windows to join `network` with the settings called `profile`.
    fn join(&mut self, network: String, profile: String, cx: &mut Context<Self>) {
        self.joining = Some(network);
        self.failure = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let joined = cx
                .background_executor()
                .spawn(async move { Wifi::connect(&profile) })
                .await;
            if let Err(error) = joined {
                let _ = this.update(cx, |this, cx| {
                    report!("{error}");
                    this.joining = None;
                    this.failure = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
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
        network: &WifiNetwork,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let arrived = staggered(self.arrival.value(window).clamp(0., 1.), index, count);
        let state = self.rows[index].state(window);
        let joinable = !network.connected && network.profile.is_some();
        let base = if network.connected {
            theme.colors.secondary_container
        } else {
            theme.colors.surface_container_highest
        };
        let reachable = network.connected || network.profile.is_some();
        let row = div()
            .id(("wifi-row", index))
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived * if reachable { 1. } else { UNKNOWN_OPACITY })
            .h(ROW_HEIGHT)
            .flex_none()
            .px(theme.spacing.medium)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .rounded(theme.shape.medium)
            .bg(if joinable {
                state_layer(theme, base, state)
            } else {
                base
            })
            .child(
                icon(theme, signal_glyph(network.bars))
                    .flex_none()
                    .text_color(if network.connected {
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
                            .child(SharedString::from(network.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .text_color(theme.colors.on_surface_variant)
                            .child(note(network, self.joining.as_deref())),
                    ),
            )
            .children(network.connected.then(|| {
                icon(theme, ICON_CONNECTED)
                    .flex_none()
                    .text_color(theme.colors.primary)
            }))
            .children((network.secured && !network.connected).then(|| {
                icon(theme, ICON_LOCK)
                    .flex_none()
                    .text_color(theme.colors.outline)
            }));
        let row = Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index));
        match (joinable, &network.profile) {
            (true, Some(profile)) => {
                let (name, profile) = (network.name.clone(), profile.clone());
                row.on_click(cx.listener(move |this, _, _, cx| {
                    this.join(name.clone(), profile.clone(), cx);
                }))
                .into_any_element()
            }
            _ => row.into_any_element(),
        }
    }
}

impl Render for WifiList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let frame = div()
            .id("wifi-list")
            .size_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .overflow_y_scroll();
        let networks = match self.reach.clone() {
            None => return frame.child(self.message(&theme, "Looking for networks", window)),
            Some(Err(error)) => return frame.child(self.message(&theme, &error, window)),
            Some(Ok(WifiReach::NoAdapter)) => {
                let text = "This machine has no Wi-Fi adapter";
                return frame.child(self.message(&theme, text, window));
            }
            Some(Ok(WifiReach::Off)) => {
                let text = "Wi-Fi is switched off; it is switched on in the Windows settings";
                return frame.child(self.message(&theme, text, window));
            }
            Some(Ok(WifiReach::Networks(networks))) => networks,
        };
        if networks.is_empty() {
            return frame.child(self.message(&theme, "No network is in reach", window));
        }
        let count = networks.len();
        let rows: Vec<AnyElement> = networks
            .iter()
            .enumerate()
            .map(|(index, network)| self.row(index, count, network, &theme, window, cx))
            .collect();
        frame
            .children(rows)
            .children(self.failure.clone().map(|failure| {
                div()
                    .flex_none()
                    .pt(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::from(failure))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(connected: bool, known: bool, secured: bool) -> WifiNetwork {
        WifiNetwork {
            name: "Home".to_owned(),
            bars: 3,
            secured,
            connected,
            profile: known.then(|| "Home".to_owned()),
        }
    }

    #[test]
    fn the_signal_icon_has_four_steps_for_five_bars() {
        assert_eq!(signal_glyph(0), ICONS_SIGNAL[0]);
        assert_eq!(signal_glyph(1), ICONS_SIGNAL[0]);
        assert_eq!(signal_glyph(3), ICONS_SIGNAL[1]);
        assert_eq!(signal_glyph(5), ICONS_SIGNAL[3]);
        assert_eq!(signal_glyph(9), ICONS_SIGNAL[3]);
    }

    #[test]
    fn a_row_says_how_the_network_stands() {
        assert_eq!(note(&network(true, true, true), None), "Connected");
        assert_eq!(note(&network(false, true, true), Some("Home")), "Joining");
        assert_eq!(note(&network(false, true, true), Some("Other")), "Saved");
        assert!(note(&network(false, false, true), None).starts_with("Not saved"));
        assert!(note(&network(false, false, false), None).starts_with("Open"));
    }
}
