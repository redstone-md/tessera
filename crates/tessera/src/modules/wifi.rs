// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Wi-Fi networks in reach. Widget: `wifi`, a list for a popup, in
//! which a click joins a network: at once where Windows has its settings
//! or anyone may join, and after its password was typed otherwise. Two
//! clicks of the right button have Windows forget the settings of one.
//! The connected network has a button that leaves it, and a row under the
//! networks joins one that hides its name.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, Entity, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, WeakEntity, Window, div, px,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::platform::{Launch, Wifi, WifiNetwork, WifiReach, WifiSecurity, password_fits};
use crate::theme::Theme;
use crate::ui::{LIST_MIN_HEIGHT, choice_button, icon, list_row, state_layer};

/// How often the list is read while it is shown.
const READ_INTERVAL: Duration = Duration::from_secs(3);

/// How many readings pass between the times the adapters are asked to
/// look anew.
const READS_PER_SCAN: u32 = 5;

/// How many readings a network that was asked to join is waited for.
const READS_TO_JOIN: u32 = 10;

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// How much of a row shows while it cannot be joined here.
const UNKNOWN_OPACITY: f32 = 0.6;

/// Wi-Fi with one to four arcs.
const ICONS_SIGNAL: [&str; 4] = ["\u{e872}", "\u{e873}", "\u{e874}", "\u{e701}"];
const ICON_LOCK: &str = "\u{e72e}";
const ICON_CONNECTED: &str = "\u{e73e}";
const ICON_SHOW: &str = "\u{e7b3}";
const ICON_HIDE: &str = "\u{ed1a}";
const ICON_HIDDEN_NETWORK: &str = "\u{e710}";

/// Where the Wi-Fi settings of Windows are, for networks joined there.
const WIFI_SETTINGS: &str = "ms-settings:network-wifi";

/// The ways a network that hides its name can be secured here, each with
/// what its choice says.
const HIDDEN_SECURITY: [(WifiSecurity, &str); 3] = [
    (WifiSecurity::Password, "WPA2-Personal"),
    (WifiSecurity::Password3, "WPA3-Personal"),
    (WifiSecurity::Open, "Open network"),
];

/// Why a network that hides its name cannot be joined with `name`,
/// `security` and `password`; `None` where it can be tried.
fn hidden_refusal(name: &str, security: WifiSecurity, password: &str) -> Option<&'static str> {
    let name = name.trim();
    if name.is_empty() || name.len() > 32 {
        return Some("The name of a network has 1 to 32 Latin characters");
    }
    (security != WifiSecurity::Open && !password_fits(password))
        .then_some("A password has 8 to 63 Latin characters")
}

pub struct WifiModule;

impl Module for WifiModule {
    fn name(&self) -> &'static str {
        "wifi"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("wifi", |_, cx| cx.new(WifiList::new).into())
    }
}

/// The icon for a signal of `bars`, of which Windows counts up to five.
fn signal_glyph(bars: u8) -> &'static str {
    ICONS_SIGNAL[usize::from(bars.clamp(1, 5) - 1) * ICONS_SIGNAL.len() / 5]
}

/// What a row says under the name of `network`; `joining` is the network
/// being joined, and `forgetting` the one a click more forgets.
fn note(network: &WifiNetwork, joining: Option<&str>, forgetting: Option<&str>) -> &'static str {
    let name = Some(network.name.as_str());
    if network.connected {
        "Connected"
    } else if joining == name {
        "Joining"
    } else if network.profile.is_some() {
        if forgetting == name {
            "Right-click again to forget it"
        } else {
            "Saved"
        }
    } else {
        match network.security {
            WifiSecurity::Open => "Open network",
            WifiSecurity::Password | WifiSecurity::Password3 => "Asks for a password",
            WifiSecurity::Other => "Not saved; join it in the Windows settings first",
        }
    }
}

/// A network that was asked to join and is not connected yet.
struct Joining {
    network: String,
    /// Whether its settings were saved for this attempt; they are
    /// forgotten again if it does not join, so that the password can be
    /// typed anew.
    made: bool,
    /// How many readings it has been waited for.
    reads: u32,
}

/// Lists the networks and follows them while it is shown.
struct WifiList {
    /// `None` until the first reading.
    reach: Option<Result<WifiReach, String>>,
    /// The network a click asked to join, until it is connected.
    joining: Option<Joining>,
    /// Why the last attempt to join failed.
    failure: Option<String>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
    rows: Vec<Interaction>,
    /// The network whose password is asked for, or was until the field
    /// folded away.
    asking: Option<String>,
    /// Whether the field for the password is open.
    asked: bool,
    /// 1 while the field for the password is open, 0 while it is not.
    ask: Spring,
    password: Entity<TextField>,
    /// The buttons beside the field: showing the password, and joining.
    eye: Interaction,
    go: Interaction,
    /// The saved network one more click of the right button forgets.
    forgetting: Option<String>,
    /// The button of the connected network that leaves it, and whether it
    /// was clicked and Windows has not answered yet.
    leave: Interaction,
    leaving: bool,
    /// The row that unfolds the form for a network that hides its name,
    /// whether it is unfolded, and how far.
    hidden_row: Interaction,
    hidden_open: bool,
    hidden_unfold: Spring,
    hidden_name: Entity<TextField>,
    hidden_password: Entity<TextField>,
    hidden_security: WifiSecurity,
    /// The choices of the security, the button that joins, and the one
    /// that opens the Wi-Fi settings of Windows.
    hidden_choices: [Interaction; 3],
    hidden_go: Interaction,
    settings: Interaction,
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
                let taken = this.update(cx, |this, cx| {
                    this.take(read, cx);
                    this.wait(cx);
                });
                if taken.is_err() {
                    break;
                }
                cx.background_executor().timer(READ_INTERVAL).await;
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        let password = SharedString::new(Language::of(cx).say("Password"));
        let hidden_password = cx.new(|cx| TextField::new(password.clone(), cx).hiding());
        let password = cx.new(|cx| TextField::new(password, cx).hiding());
        let name = SharedString::new(Language::of(cx).say("Network name"));
        let hidden_name = cx.new(|cx| TextField::new(name, cx));
        Self {
            reach: None,
            joining: None,
            failure: None,
            arrival: Spring::new(motion.spatial, 0.),
            rows: Vec::new(),
            asking: None,
            asked: false,
            ask: Spring::new(motion.spatial_fast, 0.),
            password,
            eye: Interaction::new(&motion),
            go: Interaction::new(&motion),
            forgetting: None,
            leave: Interaction::new(&motion),
            leaving: false,
            hidden_row: Interaction::new(&motion),
            hidden_open: false,
            hidden_unfold: Spring::new(motion.spatial_fast, 0.),
            hidden_name,
            hidden_password,
            hidden_security: WifiSecurity::Password,
            hidden_choices: [(); 3].map(|()| Interaction::new(&motion)),
            hidden_go: Interaction::new(&motion),
            settings: Interaction::new(&motion),
        }
    }

    /// Leaves the network the machine is connected to.
    fn disconnect(&mut self, cx: &mut Context<Self>) {
        if self.leaving {
            return;
        }
        self.leaving = true;
        self.failure = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // Windows may join the network again at once, as its profile
            // says it does by itself, so the list is read right after and
            // the button no longer waits for the network to drop out.
            let left = cx
                .background_executor()
                .spawn(async { Wifi::disconnect().map(|()| Wifi::reach()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.leaving = false;
                match left {
                    Ok(read) => this.take(read, cx),
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Unfolds the form for a network that hides its name, or folds it.
    fn toggle_hidden(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hidden_open = !self.hidden_open;
        self.hidden_unfold
            .animate_to(if self.hidden_open { 1. } else { 0. });
        if self.hidden_open {
            self.fold(cx);
            self.failure = None;
            self.hidden_name.update(cx, |field, cx| {
                field.clear(cx);
                field.focus(window);
            });
            self.hidden_password.update(cx, |field, cx| {
                field.clear(cx);
                field.hide(true, cx);
            });
        }
        cx.notify();
    }

    /// Joins the network that hides its name with what the form says.
    fn submit_hidden(&mut self, cx: &mut Context<Self>) {
        let name = self.hidden_name.read(cx).text().trim().to_owned();
        let password = self.hidden_password.read(cx).text();
        let security = self.hidden_security;
        if let Some(refusal) = hidden_refusal(&name, security, &password) {
            self.failure = Some(Language::of(cx).say(refusal).to_owned());
            cx.notify();
            return;
        }
        self.hidden_open = false;
        self.hidden_unfold.animate_to(0.);
        self.joining = Some(Joining {
            network: name.clone(),
            made: true,
            reads: 0,
        });
        self.failure = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let joined = cx
                .background_executor()
                .spawn(async move { Wifi::join_hidden(&name, security, &password) })
                .await;
            if let Err(error) = joined {
                let _ = this.update(cx, |this, cx| this.fail(error, cx));
            }
        })
        .detach();
    }

    /// The row that unfolds the form for a network that hides its name,
    /// and the form as far as it is unfolded.
    fn hidden(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let state = self.hidden_row.state(window);
        let row = list_row(
            theme,
            ICON_HIDDEN_NETWORK,
            self.hidden_open,
            language.say("Join a hidden network"),
            language.say("For a network that does not show its name"),
        )
        .id("wifi-hidden")
        .bg(state_layer(
            theme,
            theme.colors.surface_container_highest,
            state,
        ));
        let row = Interaction::track(row, cx, |this: &mut Self| Some(&mut this.hidden_row))
            .on_click(cx.listener(|this, _, window, cx| this.toggle_hidden(window, cx)));
        let shown = self.hidden_unfold.value(window).clamp(0., 1.);
        if shown < 0.01 {
            return row.into_any_element();
        }
        let choices: Vec<AnyElement> = HIDDEN_SECURITY
            .iter()
            .enumerate()
            .map(|(index, &(security, label))| {
                let chosen = self.hidden_security == security;
                let state = self.hidden_choices[index].state(window);
                let choice = choice_button(theme, language.say(label), chosen, state)
                    .id(("wifi-hidden-security", index));
                Interaction::track(choice, cx, move |this: &mut Self| {
                    this.hidden_choices.get_mut(index)
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.hidden_security = security;
                    cx.notify();
                }))
                .into_any_element()
            })
            .collect();
        let secured = self.hidden_security != WifiSecurity::Open;
        let go = choice_button(
            theme,
            language.say("Join"),
            true,
            self.hidden_go.state(window),
        )
        .id("wifi-hidden-go");
        let go = Interaction::track(go, cx, |this: &mut Self| Some(&mut this.hidden_go))
            .on_click(cx.listener(|this, _, _, cx| this.submit_hidden(cx)));
        let settings = choice_button(
            theme,
            language.say("Wi-Fi settings"),
            false,
            self.settings.state(window),
        )
        .id("wifi-settings");
        let settings = Interaction::track(settings, cx, |this: &mut Self| Some(&mut this.settings))
            .on_click(|_, _, _| Launch::new(WIFI_SETTINGS).start());
        let height = theme.launcher.field_height;
        let form = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .pt(theme.spacing.small)
            .opacity(shown)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.submit_hidden(cx),
                    "escape" => this.toggle_hidden(window, cx),
                    _ => {}
                }
            }))
            .child(div().h(height).flex().child(self.hidden_name.clone()))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.spacing.small)
                    .children(choices),
            )
            .children(secured.then(|| div().h(height).flex().child(self.hidden_password.clone())))
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::new(language.say(
                    "WPA-Enterprise and WEP networks are joined in the Wi-Fi settings of Windows",
                ))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(theme.spacing.small)
                    .child(settings)
                    .child(go),
            );
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(row)
            .child(form)
            .into_any_element()
    }

    /// The networks of the last reading.
    fn networks(&self) -> &[WifiNetwork] {
        match &self.reach {
            Some(Ok(WifiReach::Networks(networks))) => networks,
            _ => &[],
        }
    }

    /// Counts a reading for the network being joined, and gives up on it
    /// after enough of them.
    fn wait(&mut self, cx: &mut Context<Self>) {
        let Some(joining) = &mut self.joining else {
            return;
        };
        joining.reads += 1;
        if joining.reads <= READS_TO_JOIN {
            return;
        }
        let language = Language::of(cx);
        let Some(joining) = self.joining.take() else {
            return;
        };
        let failure = if joining.made {
            self.forget(joining.network, cx);
            language.say("The network was not joined; check the password")
        } else {
            language.say("The network was not joined")
        };
        self.failure = Some(failure.to_owned());
        cx.notify();
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
                .any(|network| network.connected && network.name == joining.network)
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

    /// Asks Windows to join `network`: with the settings called `profile`,
    /// or else with settings made for it with `password`.
    fn join(
        &mut self,
        network: WifiNetwork,
        profile: Option<String>,
        password: String,
        cx: &mut Context<Self>,
    ) {
        self.joining = Some(Joining {
            network: network.name.clone(),
            made: profile.is_none(),
            reads: 0,
        });
        self.failure = None;
        self.forgetting = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let joined = cx
                .background_executor()
                .spawn(async move {
                    match profile {
                        Some(profile) => Wifi::connect(&profile),
                        None => Wifi::join(&network, &password),
                    }
                })
                .await;
            if let Err(error) = joined {
                let _ = this.update(cx, |this, cx| this.fail(error, cx));
            }
        })
        .detach();
    }

    /// Shows why something did not work out.
    fn fail(&mut self, error: String, cx: &mut Context<Self>) {
        report!("{error}");
        self.joining = None;
        self.failure = Some(error);
        cx.notify();
    }

    /// Has Windows forget the settings called `profile`.
    fn forget(&mut self, profile: String, cx: &mut Context<Self>) {
        self.forgetting = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let forgotten = cx
                .background_executor()
                .spawn(async move { Wifi::forget(&profile) })
                .await;
            if let Err(error) = forgotten {
                let _ = this.update(cx, |this, cx| this.fail(error, cx));
            }
        })
        .detach();
    }

    /// A click on the row of `network`, which is not connected.
    fn clicked(&mut self, network: &WifiNetwork, window: &mut Window, cx: &mut Context<Self>) {
        self.forgetting = None;
        if network.profile.is_some() || network.security == WifiSecurity::Open {
            self.fold(cx);
            self.join(network.clone(), network.profile.clone(), String::new(), cx);
        } else if self.asked && self.asking.as_deref() == Some(network.name.as_str()) {
            self.fold(cx);
        } else {
            // The field opens under the row, and under another row anew.
            if self.asking.as_deref() != Some(network.name.as_str()) {
                self.ask.jump_to(0.);
                self.asking = Some(network.name.clone());
            }
            self.asked = true;
            self.ask.animate_to(1.);
            self.failure = None;
            self.password.update(cx, |field, cx| {
                field.clear(cx);
                field.hide(true, cx);
                field.focus(window);
            });
            cx.notify();
        }
    }

    /// A click of the right button on the row of the network with the
    /// saved settings `profile`: the second one in a row forgets them.
    fn right_clicked(&mut self, network: &str, profile: &str, cx: &mut Context<Self>) {
        if self.forgetting.as_deref() == Some(network) {
            self.forget(profile.to_owned(), cx);
        } else {
            self.forgetting = Some(network.to_owned());
            cx.notify();
        }
    }

    /// Folds the field for the password away, empty.
    fn fold(&mut self, cx: &mut Context<Self>) {
        if self.asked {
            self.asked = false;
            self.ask.animate_to(0.);
            self.password.update(cx, |field, cx| field.clear(cx));
            cx.notify();
        }
    }

    /// Joins the network the password was asked for with what was typed.
    fn submit(&mut self, cx: &mut Context<Self>) {
        let asking = self.asking.as_deref();
        let network = self
            .networks()
            .iter()
            .find(|one| Some(one.name.as_str()) == asking);
        let Some(network) = network.filter(|_| self.asked).cloned() else {
            return;
        };
        let password = self.password.read(cx).text();
        if !password_fits(&password) {
            let text = Language::of(cx).say("A password has 8 to 63 Latin characters");
            self.failure = Some(text.to_owned());
            cx.notify();
            return;
        }
        self.fold(cx);
        self.join(network, None, password, cx);
    }

    /// The keys of the field that are no text.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.submit(cx),
            "escape" => self.fold(cx),
            _ => {}
        }
    }

    /// The field for the password with its buttons, as far as it is open.
    fn field(&mut self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let shown = self.ask.value(window).clamp(0., 1.);
        let height = theme.launcher.field_height;
        let hides = self.password.read(cx).hides();
        let eye = div()
            .id("wifi-eye")
            .size(height)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .bg(state_layer(
                theme,
                theme.colors.surface_container_highest,
                self.eye.state(window),
            ))
            .child(
                icon(theme, if hides { ICON_SHOW } else { ICON_HIDE })
                    .text_color(theme.colors.on_surface_variant),
            );
        let eye = Interaction::track(eye, cx, |this: &mut Self| Some(&mut this.eye));
        let eye = eye.on_click(cx.listener(|this, _, _, cx| {
            this.password.update(cx, |field, cx| {
                let hides = field.hides();
                field.hide(!hides, cx);
            });
        }));
        let go = div()
            .id("wifi-go")
            .h(height)
            .px(theme.spacing.medium)
            .flex_none()
            .flex()
            .items_center()
            .rounded(theme.shape.full)
            .bg(state_layer(
                theme,
                theme.colors.primary,
                self.go.state(window),
            ))
            .text_color(theme.colors.surface_container)
            .child(SharedString::new(Language::of(cx).say("Join")));
        let go = Interaction::track(go, cx, |this: &mut Self| Some(&mut this.go));
        let go = go.on_click(cx.listener(|this, _, _, cx| this.submit(cx)));
        let line = div()
            .h(height)
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(self.password.clone())
            .child(eye)
            .child(go);
        // The field unfolds downwards from under its row.
        div()
            .h((height + theme.spacing.small) * shown)
            .pt(theme.spacing.small * shown)
            .overflow_hidden()
            .opacity(shown)
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .child(line)
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
        let known = network.profile.is_some();
        let joinable = !network.connected && (known || network.security != WifiSecurity::Other);
        let base = if network.connected {
            theme.colors.secondary_container
        } else {
            theme.colors.surface_container_highest
        };
        let reachable = network.connected || joinable;
        let lit = network.connected;
        let row = list_row(
            theme,
            signal_glyph(network.bars),
            lit,
            network.name.clone(),
            Language::of(cx).say(note(
                network,
                self.joining
                    .as_ref()
                    .map(|joining| joining.network.as_str()),
                self.forgetting.as_deref(),
            )),
        )
        .id(("wifi-row", index))
        .relative()
        .top(px(ROW_TRAVEL * (1. - arrived)))
        .opacity(arrived * if reachable { 1. } else { UNKNOWN_OPACITY })
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
        let row = match joinable {
            true => row.bg(state_layer(theme, base, state)),
            false => row,
        };
        let row = Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index));
        if network.connected {
            let language = Language::of(cx);
            let label = match self.leaving {
                true => "Disconnecting",
                false => "Disconnect",
            };
            let leave = choice_button(theme, language.say(label), false, self.leave.state(window))
                .id("wifi-disconnect");
            let leave = Interaction::track(leave, cx, |this: &mut Self| Some(&mut this.leave))
                .on_click(cx.listener(|this, _, _, cx| this.disconnect(cx)));
            return row.child(leave).into_any_element();
        }
        if !joinable {
            return row.into_any_element();
        }
        let clicked = network.clone();
        let row = row.on_click(cx.listener(move |this, _, window, cx| {
            this.clicked(&clicked, window, cx);
        }));
        let row = match &network.profile {
            Some(profile) => {
                let (name, profile) = (network.name.clone(), profile.clone());
                row.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _, _, cx| this.right_clicked(&name, &profile, cx)),
                )
            }
            None => row,
        };
        let asked = self.asking.as_deref() == Some(network.name.as_str());
        if !asked || known || self.ask.value(window) < 0.01 {
            return row.into_any_element();
        }
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(row)
            .child(self.field(theme, window, cx))
            .into_any_element()
    }
}

impl Render for WifiList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let frame = div()
            .id("wifi-list")
            .w_full()
            .flex_1()
            .min_h(LIST_MIN_HEIGHT)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll();
        let networks = match self.reach.clone() {
            None => {
                let text = language.say("Looking for networks");
                return frame.child(self.message(&theme, text, window));
            }
            Some(Err(error)) => return frame.child(self.message(&theme, &error, window)),
            Some(Ok(WifiReach::NoAdapter)) => {
                let text = language.say("This machine has no Wi-Fi adapter");
                return frame.child(self.message(&theme, text, window));
            }
            Some(Ok(WifiReach::Off)) => {
                let text = language.say("Wi-Fi is switched off");
                return frame.child(self.message(&theme, text, window));
            }
            Some(Ok(WifiReach::Networks(networks))) => networks,
        };
        let count = networks.len();
        let mut rows: Vec<AnyElement> = networks
            .iter()
            .enumerate()
            .map(|(index, network)| self.row(index, count, network, &theme, window, cx))
            .collect();
        if networks.is_empty() {
            let text = language.say("No network is in reach");
            rows.push(self.message(&theme, text, window).into_any_element());
        }
        if !networks.iter().any(|network| network.connected) {
            self.leaving = false;
        }
        rows.push(self.hidden(&theme, window, cx));
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
            ssid: b"Home".to_vec(),
            security: match secured {
                true => WifiSecurity::Password,
                false => WifiSecurity::Open,
            },
            bars: 3,
            secured,
            connected,
            profile: known.then(|| "Home".to_owned()),
        }
    }

    #[test]
    fn a_hidden_network_needs_a_name_and_a_password_that_fits() {
        assert!(hidden_refusal("", WifiSecurity::Open, "").is_some());
        assert!(hidden_refusal(&"x".repeat(33), WifiSecurity::Open, "").is_some());
        assert_eq!(hidden_refusal("Home", WifiSecurity::Open, ""), None);
        assert!(hidden_refusal("Home", WifiSecurity::Password, "short").is_some());
        assert_eq!(
            hidden_refusal(" Home ", WifiSecurity::Password3, "longenough"),
            None
        );
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
        let home = Some("Home");
        assert_eq!(note(&network(true, true, true), None, None), "Connected");
        assert_eq!(note(&network(false, true, true), home, None), "Joining");
        assert_eq!(
            note(&network(false, true, true), Some("Other"), None),
            "Saved"
        );
        let armed = note(&network(false, true, true), None, home);
        assert!(armed.starts_with("Right-click again"));
        let asks = note(&network(false, false, true), None, home);
        assert_eq!(asks, "Asks for a password");
        assert_eq!(
            note(&network(false, false, false), None, None),
            "Open network"
        );
        let mut work = network(false, false, true);
        work.security = WifiSecurity::Other;
        assert!(note(&work, None, None).starts_with("Not saved"));
    }
}
