// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine, and those in reach that
//! can be. Widgets: `bluetooth`, a list for a popup, in which a click
//! connects or disconnects a device that sound goes through, two clicks of
//! the right button remove a paired device, and a click on a device in
//! reach pairs it, asking for its PIN where it wants one; and
//! `bluetooth-button`, an icon that is there only on a machine with
//! Bluetooth. Windows is asked for devices in reach only while the window
//! of the list has the keyboard, as an open popup has.

use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, Div, Entity, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, WeakEntity, Window, div, px,
};

use crate::field::TextField;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix, staggered};
use crate::platform::{
    Bluetooth, BluetoothScan, FoundDevice, PairedDevice, Pairing, PairingFailure, PairingPrompt,
    Wireless,
};
use crate::theme::Theme;
use crate::ui::{
    LIST_MIN_HEIGHT, choice_button, icon, list_row, list_title, pill, state_layer, wave_bar,
};

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

/// How long the dot that tells of the search takes to pulse once.
const PULSE: Duration = Duration::from_millis(1200);

const ICON_BLUETOOTH: &str = "\u{e702}";
const ICON_ADD: &str = "\u{e710}";

pub struct BluetoothModule;

impl Module for BluetoothModule {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("bluetooth", |_, cx| cx.new(Devices::new).into())?;
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

/// What the user is told of a pairing that failed for `failure`.
fn failure_text(failure: &PairingFailure, language: Language) -> String {
    let text = match failure {
        PairingFailure::Refused => "The device refused the pairing",
        PairingFailure::TimedOut => "The device did not answer in time",
        PairingFailure::NotReady => "The device is not ready to pair; put it in pairing mode",
        PairingFailure::Cancelled => "The pairing was cancelled",
        PairingFailure::Other(reason) => {
            return format!("{}: {reason}", language.say("The device was not paired"));
        }
    };
    language.say(text).to_owned()
}

/// What the row of a device being paired says while the device asks
/// `prompt`, or while nothing is asked.
fn prompt_text(prompt: Option<&PairingPrompt>, language: Language) -> String {
    match prompt {
        None => language.say("Pairing").to_owned(),
        Some(PairingPrompt::Show(pin)) => {
            format!("{} {pin}", language.say("Type this PIN on the device:"))
        }
        Some(PairingPrompt::Confirm(pin)) => {
            format!("{} {pin}", language.say("Does the device show this PIN?"))
        }
        Some(PairingPrompt::Enter) => language.say("Type the PIN of the device").to_owned(),
    }
}

/// A paired device with what Windows names it by.
type Listed = (String, PairedDevice);

/// A device that was asked to connect or to disconnect and has not yet.
struct Asked {
    /// What Windows holds the device together by.
    container: u128,
    connect: bool,
    /// How many readings it has been waited for.
    reads: u32,
}

/// A device being paired.
struct Pairs {
    id: String,
    pairing: Rc<Pairing>,
    /// What the device asks of the user now.
    prompt: Option<PairingPrompt>,
    /// Whether the user said no to what the device asked.
    cancelled: bool,
}

/// The search for devices in reach, while it runs.
struct Search {
    /// `None` until it has started.
    scan: Option<BluetoothScan>,
    found: Vec<FoundDevice>,
    /// When it started, for the pulse of its dot.
    since: std::time::Instant,
}

/// Lists the devices and follows them while it is shown.
struct Devices {
    /// `None` until the first reading.
    devices: Option<Result<Vec<Listed>, String>>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
    /// One for each row of a paired device.
    rows: Vec<Interaction>,
    asked: Option<Asked>,
    /// 1 while a device is waited for, 0 otherwise.
    waiting: Spring,
    search: Option<Search>,
    /// Why the search could not start.
    search_failure: Option<String>,
    /// One for each row of a device in reach.
    found_rows: Vec<Interaction>,
    pairs: Option<Pairs>,
    pin: Entity<TextField>,
    /// Whether the field for the PIN is to take the keyboard when it is
    /// next drawn.
    focus_pin: bool,
    yes: Interaction,
    no: Interaction,
    /// The paired device one more click of the right button removes.
    removing: Option<String>,
    /// What the last thing done came to, when it did not work out, or a
    /// word on a device that cannot be connected from here.
    message: Option<String>,
    activation: Option<Subscription>,
}

impl Devices {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                let read = cx
                    .background_executor()
                    .spawn(async { Bluetooth::paired_devices() })
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
        let pin = SharedString::new(Language::of(cx).say("PIN"));
        let pin = cx.new(|cx| TextField::new(pin, cx));
        Self {
            devices: None,
            arrival: Spring::new(motion.spatial, 0.),
            rows: Vec::new(),
            asked: None,
            waiting: Spring::new(motion.effects, 0.),
            search: None,
            search_failure: None,
            found_rows: Vec::new(),
            pairs: None,
            pin,
            focus_pin: false,
            yes: Interaction::new(&motion),

            no: Interaction::new(&motion),
            removing: None,
            message: None,
            activation: None,
        }
    }

    /// Follows whether the window of the list has the keyboard, the first
    /// time the list is drawn: the search runs while it has.
    fn follow_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.activation.is_some() {
            return;
        }
        self.activation = Some(cx.observe_window_activation(window, |this, window, cx| {
            this.searching(window.is_window_active(), cx);
        }));
        self.searching(window.is_window_active(), cx);
    }

    /// Starts the search for devices in reach, or stops it.
    fn searching(&mut self, wanted: bool, cx: &mut Context<Self>) {
        match (wanted, self.search.is_some()) {
            (true, false) => self.start_search(cx),
            (false, true) => {
                // Stopping the watchers of Windows is left to a thread
                // that may wait.
                if let Some(scan) = self.search.take().and_then(|search| search.scan) {
                    cx.background_executor()
                        .spawn(async move { drop(scan) })
                        .detach();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.search = Some(Search {
            scan: None,
            found: Vec::new(),
            since: std::time::Instant::now(),
        });
        self.search_failure = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let started = cx
                .background_executor()
                .spawn(async { BluetoothScan::start() })
                .await;
            let changes = this.update(cx, |this, cx| {
                let scan = match started {
                    Ok(scan) => scan,
                    Err(error) => {
                        this.search = None;
                        this.search_failure = Some(error);
                        cx.notify();
                        return None;
                    }
                };
                let changes = scan.changes();
                match &mut this.search {
                    Some(search) => search.scan = Some(scan),
                    // The window lost the keyboard while the search started.
                    None => {
                        cx.background_executor()
                            .spawn(async move { drop(scan) })
                            .detach();
                        return None;
                    }
                }
                Some(changes)
            });
            let Ok(Some(changes)) = changes else {
                return;
            };
            while changes.recv().await.is_ok() {
                let going = this.update(cx, |this, cx| {
                    let Some(search) = &mut this.search else {
                        return false;
                    };
                    let Some(scan) = &search.scan else {
                        return false;
                    };
                    search.found = scan.found();
                    let motion = cx.global::<Theme>().motion;
                    let count = search.found.len();
                    this.found_rows
                        .resize_with(count, || Interaction::new(&motion));
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Pairs the device in reach `device`.
    fn pair(&mut self, device: &FoundDevice, cx: &mut Context<Self>) {
        if self.pairs.is_some() {
            return;
        }
        let pairing = Rc::new(Bluetooth::pair(device.id.clone()));
        self.pairs = Some(Pairs {
            id: device.id.clone(),
            pairing: pairing.clone(),
            prompt: None,
            cancelled: false,
        });
        self.message = None;
        self.removing = None;
        cx.notify();
        let prompts = pairing.prompts.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Ok(prompt) = prompts.recv().await {
                let shown = this.update(cx, |this, cx| {
                    if prompt == PairingPrompt::Enter {
                        this.pin.update(cx, |field, cx| field.clear(cx));
                        this.focus_pin = true;
                    }
                    if let Some(pairs) = &mut this.pairs {
                        pairs.prompt = Some(prompt);
                    }
                    cx.notify();
                });
                if shown.is_err() {
                    break;
                }
            }
        })
        .detach();
        let outcome = pairing.outcome.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(outcome) = outcome.recv().await else {
                return;
            };
            let read = outcome.is_ok().then(|| {
                cx.background_executor()
                    .spawn(async { Bluetooth::paired_devices() })
            });
            let _ = this.update(cx, |this, cx| {
                let cancelled = this.pairs.take().is_some_and(|pairs| pairs.cancelled);
                if let Err(failure) = &outcome {
                    let failure = match cancelled {
                        true => &PairingFailure::Cancelled,
                        false => failure,
                    };
                    this.message = Some(failure_text(failure, Language::of(cx)));
                }
                cx.notify();
            });
            if let Some(read) = read {
                let read = read.await;
                let _ = this.update(cx, |this, cx| this.take(read, cx));
            }
        })
        .detach();
    }

    /// Answers what the device being paired asks.
    fn answer(&mut self, answer: Option<String>, cx: &mut Context<Self>) {
        if let Some(pairs) = &mut self.pairs {
            pairs.cancelled |= answer.is_none();
            pairs.pairing.answer(answer);
            pairs.prompt = None;
            cx.notify();
        }
    }

    /// Answers the PIN that was typed.
    fn submit_pin(&mut self, cx: &mut Context<Self>) {
        let pin = self.pin.read(cx).text();
        let pin = pin.trim();
        if !pin.is_empty() {
            self.answer(Some(pin.to_owned()), cx);
        }
    }

    fn pin_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => self.submit_pin(cx),
            "escape" => self.answer(None, cx),
            _ => {}
        }
    }

    /// A click of the right button on the paired device `id`: the second
    /// one in a row removes it.
    fn right_clicked(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.removing.as_deref() != Some(id) {
            self.removing = Some(id.to_owned());
            cx.notify();
            return;
        }
        self.removing = None;
        self.message = None;
        cx.notify();
        let id = id.to_owned();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let removed = cx
                .background_executor()
                .spawn(async move {
                    Bluetooth::unpair(&id)?;
                    Bluetooth::paired_devices()
                })
                .await;
            let _ = this.update(cx, |this, cx| match removed {
                Ok(devices) => this.take(Ok(devices), cx),
                Err(error) => {
                    report!("{error}");
                    this.message = Some(error);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Asks `device` to connect if it is not connected, and to disconnect
    /// if it is. A device Windows plays no sound through cannot be asked,
    /// which the list then says.
    fn clicked(&mut self, device: &PairedDevice, cx: &mut Context<Self>) {
        self.removing = None;
        let Some(container) = device.audio else {
            let text = "Windows offers no way to connect this device from here; it connects by itself when it is switched on";
            self.message = Some(Language::of(cx).say(text).to_owned());
            cx.notify();
            return;
        };
        if self.asked.is_some() {
            return;
        }
        self.message = None;
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

    fn take(&mut self, read: Result<Vec<Listed>, String>, cx: &mut Context<Self>) {
        if let Some(asked) = &mut self.asked {
            asked.reads += 1;
            let listed = read.as_deref().unwrap_or_default();
            let device = listed
                .iter()
                .find(|(_, device)| device.audio == Some(asked.container));
            let done = device.is_none_or(|(_, device)| device.connected == asked.connect);
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
        let kind = |devices: &Option<Result<Vec<Listed>, String>>| match devices {
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
        (id, device): &Listed,
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
        let note = match self.removing.as_deref() == Some(id.as_str()) {
            true => language.say("Right-click again to remove it").to_owned(),
            false => note(device, asked, language),
        };
        let row = list_row(
            theme,
            ICON_BLUETOOTH,
            device.connected,
            device.name.clone(),
            note,
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
        let row = match (device.audio, self.rows.get_mut(index)) {
            (Some(_), Some(interaction)) => {
                row.bg(state_layer(theme, base, interaction.state(window)))
            }
            _ => row,
        };
        let row = Interaction::track(row, cx, move |this: &mut Self| this.rows.get_mut(index));
        let (clicked, removed) = (device.clone(), id.clone());
        row.on_click(cx.listener(move |this, _, _, cx| this.clicked(&clicked, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| this.right_clicked(&removed, cx)),
            )
            .into_any_element()
    }

    /// The title over the devices in reach, with a dot that pulses while
    /// Windows looks for them.
    fn search_title(&self, theme: &Theme, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let language = Language::of(cx);
        let title = list_title(theme, language.say("Available devices"));
        let Some(search) = &self.search else {
            return title;
        };
        // The pulse needs a frame after this one.
        window.request_animation_frame();
        let phase = search.since.elapsed().as_secs_f32() / PULSE.as_secs_f32();
        let pulse = 0.5 + 0.5 * (phase * std::f32::consts::TAU).sin();
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(title)
            .child(
                div()
                    .size(px(6.))
                    .rounded(theme.shape.full)
                    .bg(theme.colors.primary)
                    .opacity(0.3 + 0.7 * pulse),
            )
            .child(
                div()
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::new(language.say("Searching…"))),
            )
    }

    fn found_row(
        &mut self,
        index: usize,
        device: &FoundDevice,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let pairing = self.pairs.as_ref().filter(|pairs| pairs.id == device.id);
        let note = match pairing {
            Some(pairs) => prompt_text(pairs.prompt.as_ref(), language),
            None => language.say("Ready to pair").to_owned(),
        };
        let prompt = pairing.and_then(|pairs| pairs.prompt.clone());
        let lit = pairing.is_some();
        let row = list_row(theme, ICON_ADD, lit, device.name.clone(), note)
            .id(("bluetooth-found", index));
        let row = match self.found_rows.get_mut(index) {
            Some(interaction) if self.pairs.is_none() => row.bg(state_layer(
                theme,
                theme.colors.surface_container_highest,
                interaction.state(window),
            )),
            _ => row,
        };
        let row = Interaction::track(row, cx, move |this: &mut Self| {
            this.found_rows.get_mut(index)
        });
        let clicked = device.clone();
        let row = row.on_click(cx.listener(move |this, _, _, cx| this.pair(&clicked, cx)));
        let answer = match prompt {
            Some(PairingPrompt::Confirm(_)) => Some(self.confirm_buttons(theme, window, cx)),
            Some(PairingPrompt::Enter) => Some(self.pin_line(theme, window, cx)),
            _ => None,
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(row)
            .children(answer)
            .into_any_element()
    }

    /// Yes and No, for a PIN the device shows too.
    fn confirm_buttons(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let yes = choice_button(theme, language.say("Pair"), true, self.yes.state(window))
            .id("bluetooth-yes");
        let yes = Interaction::track(yes, cx, |this: &mut Self| Some(&mut this.yes))
            .on_click(cx.listener(|this, _, _, cx| this.answer(Some(String::new()), cx)));
        let no = choice_button(theme, language.say("Cancel"), false, self.no.state(window))
            .id("bluetooth-no");
        let no = Interaction::track(no, cx, |this: &mut Self| Some(&mut this.no))
            .on_click(cx.listener(|this, _, _, cx| this.answer(None, cx)));
        div()
            .flex_none()
            .flex()
            .justify_end()
            .gap(theme.spacing.small)
            .child(no)
            .child(yes)
            .into_any_element()
    }

    /// The field for a PIN the device wants, with its buttons.
    fn pin_line(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        if std::mem::take(&mut self.focus_pin) {
            self.pin.read(cx).focus(window);
        }
        let go = choice_button(theme, language.say("Pair"), true, self.yes.state(window))
            .id("bluetooth-pin-go");
        let go = Interaction::track(go, cx, |this: &mut Self| Some(&mut this.yes))
            .on_click(cx.listener(|this, _, _, cx| this.submit_pin(cx)));
        let no = choice_button(theme, language.say("Cancel"), false, self.no.state(window))
            .id("bluetooth-pin-no");
        let no = Interaction::track(no, cx, |this: &mut Self| Some(&mut this.no))
            .on_click(cx.listener(|this, _, _, cx| this.answer(None, cx)));
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .on_key_down(cx.listener(|this, event, _, cx| this.pin_key(event, cx)))
            .child(self.pin.clone())
            .child(no)
            .child(go)
            .into_any_element()
    }

    /// The devices in reach, under their title, while the search runs or
    /// could not start.
    fn found_rows(
        &mut self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if let Some(failure) = self.search_failure.clone() {
            rows.push(
                list_title(theme, Language::of(cx).say("Available devices")).into_any_element(),
            );
            rows.push(self.message(theme, &failure, window).into_any_element());
            return rows;
        }
        if self.search.is_none() {
            return rows;
        }
        rows.push(self.search_title(theme, window, cx).into_any_element());
        let found = self
            .search
            .as_ref()
            .map(|search| search.found.clone())
            .unwrap_or_default();
        for (index, device) in found.iter().enumerate() {
            rows.push(self.found_row(index, device, theme, window, cx));
        }
        rows
    }
}

impl Render for Devices {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_window(window, cx);
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let frame = div()
            .id("bluetooth-list")
            .w_full()
            .flex_1()
            .min_h(LIST_MIN_HEIGHT)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll();
        let paired: Vec<AnyElement> = match self.devices.clone() {
            None => {
                let text = language.say("Looking for devices");
                vec![self.message(&theme, text, window).into_any_element()]
            }
            Some(Err(error)) => vec![self.message(&theme, &error, window).into_any_element()],
            Some(Ok(devices)) if devices.is_empty() => {
                let text = language.say("No Bluetooth device is paired");
                vec![self.message(&theme, text, window).into_any_element()]
            }
            Some(Ok(devices)) => {
                let count = devices.len();
                { devices.iter().enumerate() }
                    .map(|(index, device)| self.row(index, count, device, &theme, window, cx))
                    .collect()
            }
        };
        let found = self.found_rows(&theme, window, cx);
        frame
            .children(paired)
            .children(self.message.clone().map(|message| {
                div()
                    .flex_none()
                    .pt(theme.spacing.small)
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::from(message))
            }))
            .children(found)
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

    #[test]
    fn a_pairing_says_what_the_device_asks_and_why_it_failed() {
        let english = Language::English;
        assert_eq!(prompt_text(None, english), "Pairing");
        assert_eq!(
            prompt_text(Some(&PairingPrompt::Show("123456".into())), english),
            "Type this PIN on the device: 123456"
        );
        assert!(prompt_text(Some(&PairingPrompt::Confirm("42".into())), english).ends_with(" 42"));
        assert_eq!(
            failure_text(&PairingFailure::Other("status 19".into()), english),
            "The device was not paired: status 19"
        );
        assert_eq!(
            failure_text(&PairingFailure::Cancelled, english),
            "The pairing was cancelled"
        );
    }
}
