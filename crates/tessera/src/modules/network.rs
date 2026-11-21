// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The network connection in use. Widget: `network`.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, IntoElement, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window, div, px,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{Link, Network};
use crate::theme::Theme;
use crate::ui::{icon, pill};

/// How often it is checked whether Windows reported a change.
const CHECK_INTERVAL: Duration = Duration::from_secs(2);

/// How many checks pass before the connection is read again although
/// nothing was reported: the signal strength changes without a report.
/// Without reports at all, every check reads.
const CHECKS_WATCHED: u32 = 15;

/// How far to the left of its place a new text starts.
const TEXT_TRAVEL: f32 = 8.;

const ICON_WIRED: &str = "\u{e839}";
const ICON_OFFLINE: &str = "\u{f384}";
/// Wi-Fi with one to four arcs, and mobile signal with one to five bars.
const ICONS_WIRELESS: [&str; 4] = ["\u{e872}", "\u{e873}", "\u{e874}", "\u{e701}"];
const ICONS_CELLULAR: [&str; 5] = ["\u{e86c}", "\u{e86d}", "\u{e86e}", "\u{e86f}", "\u{e870}"];

pub struct NetworkModule;

impl Module for NetworkModule {
    fn name(&self) -> &'static str {
        "network"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("network", |context, cx| {
            let named = match context.text("show") {
                None | Some("name") => true,
                Some("icon") => false,
                Some(other) => {
                    eprintln!(
                        "tessera: `show` must be `name` or `icon`, `{other}` is taken as `name`"
                    );
                    true
                }
            };
            cx.new(|cx| Connection::new(named, cx)).into()
        })
    }
}

/// The icon for `network`.
fn glyph(network: Option<&Network>) -> &'static str {
    // Windows counts up to five bars; the icons of Wi-Fi have four arcs.
    let step = |bars: u8, steps: usize| usize::from(bars.clamp(1, 5) - 1) * steps / 5;
    match network.map(|network| network.link) {
        None => ICON_OFFLINE,
        Some(Link::Wired) => ICON_WIRED,
        Some(Link::Wireless(bars)) => ICONS_WIRELESS[step(bars, 4)],
        Some(Link::Cellular(bars)) => ICONS_CELLULAR[step(bars, 5)],
    }
}

/// What is written beside the icon.
fn caption(network: Option<&Network>) -> String {
    match network {
        None => "Offline".to_owned(),
        Some(network) if network.internet => network.name.clone(),
        Some(network) => format!("{}, no internet", network.name),
    }
}

/// Shows the connection in a pill and follows it.
struct Connection {
    network: Option<Network>,
    /// Whether the name is written beside the icon.
    named: bool,
    /// 0 when what is shown changes, 1 once the new text is in place.
    arrival: Spring,
}

impl Connection {
    fn new(named: bool, cx: &mut Context<Self>) -> Self {
        let patience = if Network::watch() {
            CHECKS_WATCHED
        } else {
            eprintln!("tessera: Windows does not report network changes, so they are polled");
            1
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut seen = Network::changes();
            let mut quiet = 0;
            loop {
                cx.background_executor().timer(CHECK_INTERVAL).await;
                let now = Network::changes();
                quiet += 1;
                if now == seen && quiet < patience {
                    if this.upgrade().is_none() {
                        break;
                    }
                    continue;
                }
                (seen, quiet) = (now, 0);
                // Windows may take its time to answer.
                let network = cx
                    .background_executor()
                    .spawn(async { Network::current() })
                    .await;
                if this.update(cx, |this, cx| this.show(network, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut arrival = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            network: Network::current(),
            named,
            arrival,
        }
    }

    fn show(&mut self, network: Option<Network>, cx: &mut Context<Self>) {
        if network == self.network {
            return;
        }
        // A signal that only got stronger or weaker changes the icon, and
        // the text stays where it is.
        if caption(network.as_ref()) != caption(self.network.as_ref()) {
            self.arrival.jump_to(0.);
            self.arrival.animate_to(1.);
        }
        self.network = network;
        cx.notify();
    }
}

impl Render for Connection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let network = self.network.as_ref();
        let reached = network.is_some_and(|network| network.internet);
        let arrived = self.arrival.value(window).clamp(0., 1.);
        let color = if reached {
            theme.colors.on_surface_variant
        } else {
            theme.colors.outline
        };
        pill(&theme)
            .child(icon(&theme, glyph(network)).text_color(color))
            .children(self.named.then(|| {
                div()
                    .relative()
                    .right(px(TEXT_TRAVEL * (1. - arrived)))
                    .opacity(arrived)
                    .child(SharedString::from(caption(network)))
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(link: Link, internet: bool) -> Network {
        Network {
            name: "Home".to_owned(),
            link,
            internet,
        }
    }

    #[test]
    fn the_icon_follows_the_link_and_its_signal() {
        assert_eq!(glyph(None), ICON_OFFLINE);
        assert_eq!(glyph(Some(&network(Link::Wired, true))), ICON_WIRED);
        let wireless = |bars| glyph(Some(&network(Link::Wireless(bars), true)));
        assert_eq!(wireless(0), ICONS_WIRELESS[0]);
        assert_eq!(wireless(1), ICONS_WIRELESS[0]);
        assert_eq!(wireless(3), ICONS_WIRELESS[1]);
        assert_eq!(wireless(4), ICONS_WIRELESS[2]);
        assert_eq!(wireless(5), ICONS_WIRELESS[3]);
        let cellular = |bars| glyph(Some(&network(Link::Cellular(bars), true)));
        assert_eq!(cellular(1), ICONS_CELLULAR[0]);
        assert_eq!(cellular(5), ICONS_CELLULAR[4]);
        assert_eq!(cellular(9), ICONS_CELLULAR[4]);
    }

    #[test]
    fn the_text_says_what_is_reached() {
        assert_eq!(caption(None), "Offline");
        assert_eq!(caption(Some(&network(Link::Wired, true))), "Home");
        assert_eq!(
            caption(Some(&network(Link::Wired, false))),
            "Home, no internet"
        );
    }
}
