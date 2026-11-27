// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Drives and devices that can be taken out of the machine, as the "Safely
//! Remove Hardware" icon of Explorer offers them. Widgets: `devices`, an
//! icon that is there only while something can be taken out, and
//! `devices-list`, for a popup, a row for each drive with how full it is
//! and buttons that open it and eject it, and a row for each other device
//! to eject. The popup `devices` holds the list, and a click on the icon
//! opens it. The action `eject` ejects the drive its `arg` names by its
//! letter.
//!
//! The list is read once and again whenever Windows tells of a change, and
//! every few seconds while it is shown, for the free space. A drive that
//! arrives is told of with a notice where `[notices]` says so; so is an
//! ejection, which says when the device may be pulled out or why Windows
//! refused.

use std::collections::HashMap;
use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, Div, Entity, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::banners::{Banners, Notice, NoticeAction};
use crate::config::{PopupConfig, WidgetSpec};
use crate::files::{Files, size_text};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, InteractionState, Spring, staggered};
use crate::platform::{
    DeviceChanges, DriveKind, EjectRefusal, Ejection, Launch, RemovableDevice, RemovableDrive,
    Removables,
};
use crate::popup::Popups;
use crate::theme::Theme;
use crate::ui::{LIST_MIN_HEIGHT, icon, list_row, list_title, pill, state_layer, wave_bar};

/// How long after Windows told of a change the list is read, so that the
/// changes of one device are read together and its drive has its letter.
const SETTLE: Duration = Duration::from_millis(400);

/// How often the list is read while it is shown, for the free space.
const SHOWN_INTERVAL: Duration = Duration::from_secs(5);

/// The popup a click on `devices` opens when the layout names none.
const POPUP: &str = "devices";

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// How wide the line that shows how full a drive is.
const FULLNESS_WIDTH: Pixels = px(48.);

const ICON_USB: &str = "\u{e88e}";
const ICON_DISK: &str = "\u{eda2}";
const ICON_DISC: &str = "\u{e958}";
const ICON_CARD: &str = "\u{e7f1}";

pub struct DevicesModule;

impl Module for DevicesModule {
    fn name(&self) -> &'static str {
        "devices"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("devices", |_, cx| cx.new(Button::new).into())?;
        widgets.register_panel("devices-list", |_, cx| cx.new(List::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: Default::default(),
            width: 380.,
            height: 360.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "devices-list".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })?;
        widgets.open_with("devices", POPUP);
        Ok(())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let eject = Action::new(|arg, cx| {
            if let Some(letter) = arg.and_then(letter_of) {
                Drives::eject_letter(letter, cx);
            }
        })
        .checking(|arg| match arg.and_then(letter_of) {
            Some(_) => Ok(()),
            None => Err("`eject` takes the letter of a drive, such as `E`".to_owned()),
        });
        actions.register("eject", eject)
    }
}

/// The drive letter `arg` names: `E`, `e`, `E:` or `E:\`.
fn letter_of(arg: &str) -> Option<char> {
    let arg = arg.trim();
    let rest = arg.trim_end_matches(['\\', '/']);
    let rest = rest.strip_suffix(':').unwrap_or(rest);
    let mut letters = rest.chars();
    let letter = letters.next()?.to_ascii_uppercase();
    (letters.next().is_none() && letter.is_ascii_uppercase()).then_some(letter)
}

/// Where an ejection that was asked for stands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    Ejecting,
    Refused(EjectRefusal),
}

/// What can be taken out, shared by every widget and the notices.
#[derive(Default)]
struct DeviceState {
    /// `None` until the first reading.
    removables: Option<Removables>,
    /// The ejections asked for, until the next one of the same thing.
    asked: HashMap<Ejection, Asked>,
}

impl DeviceState {
    /// Whether something is being ejected. The list is not read then: a
    /// reading opens every drive, which would keep its volume from being
    /// locked and have Windows refuse the ejection because of Tessera.
    fn ejecting(&self) -> bool {
        self.asked.values().any(|asked| *asked == Asked::Ejecting)
    }
}

/// The one list, made when the first widget or the notices ask for it, and
/// whether drives that arrive are told of.
#[derive(Default)]
pub struct Drives {
    state: Option<Entity<DeviceState>>,
    tell: bool,
    /// The number of the last reading started and of the last one taken:
    /// readings take their time, and one that ends after a later one is
    /// not taken.
    started: u64,
    taken: u64,
}

impl Global for Drives {}

impl Drives {
    /// Tells of drives that arrive from now on if `on`, and not otherwise.
    pub fn tell_arrivals(on: bool, cx: &mut App) {
        cx.default_global::<Self>().tell = on;
        if on {
            Self::state(cx);
        }
    }

    /// The list, which starts to be followed the first time it is asked
    /// for.
    fn state(cx: &mut App) -> Entity<DeviceState> {
        if let Some(state) = &cx.default_global::<Self>().state {
            return state.clone();
        }
        let state = cx.new(|cx: &mut Context<DeviceState>| {
            let changes = DeviceChanges::watch();
            cx.spawn(async move |_, cx: &mut AsyncApp| {
                if cx.update(Self::refresh).is_err() {
                    return;
                }
                while changes.recv().await.is_ok() {
                    cx.background_executor().timer(SETTLE).await;
                    if cx.update(Self::refresh).is_err() {
                        break;
                    }
                }
            })
            .detach();
            DeviceState::default()
        });
        cx.global_mut::<Self>().state = Some(state.clone());
        state
    }

    /// Whether something is being ejected, while which nothing is read.
    fn ejecting(cx: &mut App) -> bool {
        Self::state(cx).read(cx).ejecting()
    }

    /// Reads the list again, away from the user interface, unless
    /// something is being ejected; the end of the ejection reads it.
    fn refresh(cx: &mut App) {
        if Self::ejecting(cx) {
            return;
        }
        let (number, read) = Self::read(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let read = read.await;
            cx.update(|cx| Self::take(number, read, cx)).ok();
        })
        .detach();
    }

    /// Starts a reading of the list, with its number.
    fn read(cx: &mut App) -> (u64, Task<Removables>) {
        let this = cx.default_global::<Self>();
        this.started += 1;
        let number = this.started;
        let read = cx.background_executor().spawn(async { Removables::read() });
        (number, read)
    }

    /// Reads again how full the drives in the list are, which is far
    /// quicker than reading the list, unless something is being ejected.
    fn refresh_space(cx: &mut App) {
        let state = Self::state(cx);
        let state = state.read(cx);
        let Some(mut removables) = state.removables.clone().filter(|_| !state.ejecting()) else {
            return;
        };
        let read = cx.background_executor().spawn(async move {
            removables.read_space();
            removables
        });
        cx.spawn(async move |cx: &mut AsyncApp| {
            let read = read.await;
            cx.update(|cx| Self::take_space(read, cx)).ok();
        })
        .detach();
    }

    /// Takes how full the drives of `read` are into the list, for those
    /// drives that are still in it.
    fn take_space(read: Removables, cx: &mut App) {
        Self::state(cx).update(cx, |state, cx| {
            let Some(removables) = &mut state.removables else {
                return;
            };
            let mut changed = false;
            for drive in &mut removables.drives {
                let same = |known: &&RemovableDrive| {
                    known.drive.root == drive.drive.root && known.device == drive.device
                };
                let Some(known) = read.drives.iter().find(same) else {
                    continue;
                };
                let space = (known.drive.total, known.drive.free);
                if (drive.drive.total, drive.drive.free) != space {
                    (drive.drive.total, drive.drive.free) = space;
                    changed = true;
                }
            }
            if changed {
                cx.notify();
            }
        });
    }

    /// Takes the reading with `number`, unless a later one was taken.
    fn take(number: u64, read: Removables, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if number < this.taken {
            return;
        }
        this.taken = number;
        let state = Self::state(cx);
        let arrived = state.update(cx, |state, cx| {
            let arrived: Vec<RemovableDrive> = arrived(state.removables.as_ref(), &read)
                .into_iter()
                .cloned()
                .collect();
            // A refusal is forgotten once what was refused is gone, so a
            // device plugged in again starts afresh.
            let before = state.asked.len();
            state
                .asked
                .retain(|ejection, asked| *asked == Asked::Ejecting || holds(&read, ejection));
            let forgot = state.asked.len() != before;
            if state.removables.as_ref() != Some(&read) || forgot {
                state.removables = Some(read);
                cx.notify();
            }
            arrived
        });
        if !cx.global::<Self>().tell {
            return;
        }
        let language = Language::of(cx);
        for drive in arrived {
            Banners::post(arrival_notice(&drive, language), cx);
        }
    }

    /// Ejects what `ejection` names, which the user knows as `name`, and
    /// tells how it went.
    fn eject(ejection: Ejection, name: String, cx: &mut App) {
        let state = Self::state(cx);
        let asked = state.update(cx, |state, cx| {
            if state.asked.get(&ejection) == Some(&Asked::Ejecting) {
                return false;
            }
            state.asked.insert(ejection.clone(), Asked::Ejecting);
            cx.notify();
            true
        });
        if !asked {
            return;
        }
        let answered = ejection.start();
        cx.spawn(async move |cx: &mut AsyncApp| {
            let ejected = answered.recv().await;
            let ejected = ejected.unwrap_or_else(|_| Err(EjectRefusal::Refused(String::new())));
            cx.update(|cx| Self::ejected(ejection, name, ejected, cx))
                .ok();
        })
        .detach();
    }

    fn ejected(ejection: Ejection, name: String, ejected: Result<(), EjectRefusal>, cx: &mut App) {
        let language = Language::of(cx);
        let notice = match &ejected {
            Ok(()) => Notice {
                app: language.say("Removable devices").to_owned(),
                title: language.say("Safe to remove").to_owned(),
                body: format!("{name} {}", language.say("can now be taken out.")),
                glyph: Some(ICON_USB),
                ..Notice::default()
            },
            Err(refusal) => Notice {
                app: language.say("Removable devices").to_owned(),
                title: language.say("The device was not ejected").to_owned(),
                body: format!("{name}: {}", refusal_text(refusal, language)),
                glyph: Some(ICON_USB),
                // The user is waiting to pull the device out, and must
                // not miss that it is not safe yet.
                urgent: true,
                ..Notice::default()
            },
        };
        let state = Self::state(cx);
        state.update(cx, |state, cx| {
            match ejected {
                Ok(()) => state.asked.remove(&ejection),
                Err(refusal) => state.asked.insert(ejection, Asked::Refused(refusal)),
            };
            cx.notify();
        });
        Banners::post(notice, cx);
        Self::refresh(cx);
    }

    /// Ejects the drive with `letter`, as the action `eject` does. The
    /// list is read first, as it may not be followed yet, but not while
    /// something else is being ejected.
    fn eject_letter(letter: char, cx: &mut App) {
        if Self::ejecting(cx) {
            let state = Self::state(cx);
            let known = state.read(cx).removables.clone().unwrap_or_default();
            Self::eject_found(letter, &known, cx);
            return;
        }
        let (number, read) = Self::read(cx);
        cx.spawn(async move |cx: &mut AsyncApp| {
            let read = read.await;
            cx.update(|cx| {
                Self::eject_found(letter, &read, cx);
                Self::take(number, read, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Ejects the drive with `letter` among `removables`.
    fn eject_found(letter: char, removables: &Removables, cx: &mut App) {
        let found = removables
            .drives
            .iter()
            .find(|drive| drive.letter() == letter);
        match found {
            Some(drive) => Self::eject(drive.ejection(), drive.drive.title(), cx),
            None => report!("there is no drive {letter}: to eject"),
        }
    }
}

/// The drives of `now` that were not there `before`; none at the first
/// reading, whose drives were there before Tessera looked.
fn arrived<'a>(before: Option<&Removables>, now: &'a Removables) -> Vec<&'a RemovableDrive> {
    let Some(before) = before else {
        return Vec::new();
    };
    now.drives
        .iter()
        .filter(|drive| {
            !before
                .drives
                .iter()
                .any(|known| known.drive.root == drive.drive.root)
        })
        .collect()
}

/// Whether `removables` hold what `ejection` names.
fn holds(removables: &Removables, ejection: &Ejection) -> bool {
    let drives = removables.drives.iter().map(RemovableDrive::ejection);
    let devices = removables.devices.iter();
    let mut devices = devices.map(|device| Ejection::Device(device.instance.clone()));
    { drives }.any(|held| held == *ejection) || devices.any(|held| held == *ejection)
}

/// The notice of a drive that arrived, with buttons that open and eject
/// it.
fn arrival_notice(drive: &RemovableDrive, language: Language) -> Notice {
    let root = drive.drive.root.clone();
    let open = NoticeAction::new(language.say("Open"), move |cx| open_drive(&root, cx));
    let ejection = drive.ejection();
    let name = drive.drive.title();
    let eject = NoticeAction::new(language.say("Eject"), move |cx| {
        Drives::eject(ejection.clone(), name.clone(), cx);
    });
    let title = match drive.device {
        Some(_) => "A drive was plugged in",
        None => "A disc or a card was put in",
    };
    let body = match &drive.device {
        Some(device) => format!("{}, {}", drive.drive.title(), device.name),
        None => drive.drive.title(),
    };
    Notice {
        app: language.say("Removable devices").to_owned(),
        title: language.say(title).to_owned(),
        body,
        glyph: Some(drive_glyph(drive)),
        actions: vec![open, eject],
        urgent: false,
        ..Notice::default()
    }
}

/// Opens the drive at `root` in the file manager of Tessera, or else in
/// Explorer.
fn open_drive(root: &std::path::Path, cx: &mut App) {
    if !Files::open_folder(root, cx) {
        Launch::new(root).start();
    }
}

/// What tells the user why Windows did not eject something.
fn refusal_text(refusal: &EjectRefusal, language: Language) -> String {
    match refusal {
        EjectRefusal::InUse(None) => language
            .say("A program is using it; close its files and try again.")
            .to_owned(),
        EjectRefusal::InUse(Some(holder)) => {
            format!("{} {holder}.", language.say("It is in use by"))
        }
        EjectRefusal::NotAllowed => language.say("This account may not eject it.").to_owned(),
        EjectRefusal::Refused(detail) if detail.is_empty() => {
            language.say("Windows refused to eject it.").to_owned()
        }
        EjectRefusal::Refused(detail) => {
            format!(
                "{} ({detail})",
                language.say("Windows refused to eject it.")
            )
        }
    }
}

fn drive_glyph(drive: &RemovableDrive) -> &'static str {
    match (drive.drive.kind, &drive.device) {
        (DriveKind::Optical, _) => ICON_DISC,
        (DriveKind::Removable, None) => ICON_CARD,
        (DriveKind::Fixed, _) => ICON_DISK,
        _ => ICON_USB,
    }
}

/// What the row of a drive says under its name: how the ejection that was
/// asked for stands, or how much of it is free.
fn drive_note(drive: &RemovableDrive, asked: Option<&Asked>, language: Language) -> String {
    match asked {
        Some(Asked::Ejecting) => return language.say("Ejecting…").to_owned(),
        Some(Asked::Refused(refusal)) => return refusal_text(refusal, language),
        None => {}
    }
    let space = match (drive.drive.free, drive.drive.total) {
        (Some(free), Some(total)) => Some(format!(
            "{} {} {}",
            size_text(free),
            language.say("free of"),
            size_text(total)
        )),
        _ => None,
    };
    let device = drive.device.as_ref().map(|device| device.name.clone());
    match (space, device) {
        (Some(space), Some(device)) => format!("{space}, {device}"),
        (Some(text), None) | (None, Some(text)) => text,
        (None, None) => String::new(),
    }
}

/// How full a drive is, from 0 to 1, where it says.
fn fullness(drive: &RemovableDrive) -> Option<f32> {
    let (free, total) = (drive.drive.free?, drive.drive.total?);
    (total > 0).then(|| 1. - free as f32 / total as f32)
}

/// The icon in the bar, there only while something can be taken out.
struct Button {
    state: Entity<DeviceState>,
    /// 0 while nothing can be taken out, 1 while the icon is shown.
    presence: Spring,
    _changes: Subscription,
}

impl Button {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = Drives::state(cx);
        let motion = cx.global::<Theme>().motion;
        let mut this = Self {
            _changes: cx.observe(&state, |this: &mut Self, _, cx| this.follow(cx)),
            state,
            presence: Spring::new(motion.spatial, 0.),
        };
        this.follow(cx);
        this
    }

    fn follow(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let present = state
            .removables
            .as_ref()
            .is_some_and(|removables| !removables.is_empty());
        self.presence.animate_to(if present { 1. } else { 0. });
        cx.notify();
    }
}

impl Render for Button {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window).clamp(0., 1.);
        if presence == 0. {
            return div().into_any_element();
        }
        let state = self.state.read(cx);
        let busy = state.asked.values().any(|asked| *asked == Asked::Ejecting);
        pill(&theme)
            .opacity(presence)
            .child(icon(&theme, ICON_USB).text_color(if busy {
                theme.colors.primary
            } else {
                theme.colors.on_surface
            }))
            .into_any_element()
    }
}

/// The buttons of one row.
struct RowButtons {
    open: Interaction,
    eject: Interaction,
}

/// The list for a popup, read again every few seconds while it is shown.
struct List {
    state: Entity<DeviceState>,
    /// 0 when what is shown changes in kind, 1 once it is in place.
    arrival: Spring,
    rows: Vec<RowButtons>,
    _changes: Subscription,
}

impl List {
    fn new(cx: &mut Context<Self>) -> Self {
        let state = Drives::state(cx);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if cx.update(Drives::refresh).is_err() {
                return;
            }
            loop {
                cx.background_executor().timer(SHOWN_INTERVAL).await;
                if this.upgrade().is_none() || cx.update(Drives::refresh_space).is_err() {
                    break;
                }
            }
        })
        .detach();
        let motion = cx.global::<Theme>().motion;
        let mut arrival = Spring::new(motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            _changes: cx.observe(&state, |_, _, cx| cx.notify()),
            state,
            arrival,
            rows: Vec::new(),
        }
    }

    /// A button of the row at `index`; `eject` says which of the two.
    fn button(
        index: usize,
        eject: bool,
        label: &'static str,
        state: InteractionState,
        theme: &Theme,
        cx: &mut Context<Self>,
        click: impl Fn(&mut App) + 'static,
    ) -> AnyElement {
        let base = if eject {
            theme.colors.secondary_container
        } else {
            theme.colors.surface_container
        };
        let button = div()
            .id((label, index))
            .flex_none()
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(theme, base, state))
            .on_click(move |_, _, cx| click(cx))
            .child(Language::of(cx).say(label));
        Interaction::track(button, cx, move |this: &mut Self| {
            let row = this.rows.get_mut(index)?;
            Some(if eject { &mut row.eject } else { &mut row.open })
        })
        .into_any_element()
    }

    fn message(&self, theme: &Theme, text: &str, arrived: f32) -> Div {
        div()
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .text_color(theme.colors.on_surface_variant)
            .child(SharedString::from(text.to_owned()))
    }
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let state = self.state.read(cx);
        let removables = state.removables.clone();
        let asked = state.asked.clone();
        let frame = div()
            .id("devices-list")
            .w_full()
            .flex_1()
            .min_h(LIST_MIN_HEIGHT)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .overflow_y_scroll()
            .child(list_title(&theme, language.say("Safely remove")));
        let Some(removables) = removables else {
            let text = language.say("Looking for devices");
            return frame.child(self.message(&theme, text, arrival));
        };
        if removables.is_empty() {
            let text = language.say("Nothing to take out");
            return frame.child(self.message(&theme, text, arrival));
        }
        let count = removables.drives.len() + removables.devices.len();
        let motion = theme.motion;
        self.rows.resize_with(count, || RowButtons {
            open: Interaction::new(&motion),
            eject: Interaction::new(&motion),
        });
        let mut rows: Vec<AnyElement> = Vec::with_capacity(count);
        for (index, drive) in removables.drives.iter().enumerate() {
            let arrived = staggered(arrival, index, count);
            let ejection = drive.ejection();
            let note = drive_note(drive, asked.get(&ejection), language);
            let ejecting = asked.get(&ejection) == Some(&Asked::Ejecting);
            let open_state = self.rows[index].open.state(window);
            let eject_state = self.rows[index].eject.state(window);
            let root = drive.drive.root.clone();
            let open = Self::button(index, false, "Open", open_state, &theme, cx, move |cx| {
                open_drive(&root, cx);
                Popups::close(cx);
            });
            let name = drive.drive.title();
            let eject = (!ejecting).then(|| {
                Self::button(index, true, "Eject", eject_state, &theme, cx, move |cx| {
                    Drives::eject(ejection.clone(), name.clone(), cx);
                })
            });
            let full = fullness(drive).map(|level| {
                div()
                    .flex_none()
                    .w(FULLNESS_WIDTH)
                    .child(wave_bar(&theme, level * arrived))
            });
            let row = list_row(&theme, drive_glyph(drive), false, drive.drive.title(), note)
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrived)))
                .opacity(arrived)
                .children(full)
                .child(open)
                .children(eject);
            rows.push(row.into_any_element());
        }
        let first = removables.drives.len();
        for (offset, device) in removables.devices.iter().enumerate() {
            let index = first + offset;
            let arrived = staggered(arrival, index, count);
            rows.push(self.device_row(index, device, &asked, arrived, window, cx));
        }
        frame.children(rows)
    }
}

impl List {
    /// The row of a device without a drive: its name and a button that
    /// ejects it.
    fn device_row(
        &mut self,
        index: usize,
        device: &RemovableDevice,
        asked: &HashMap<Ejection, Asked>,
        arrived: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let ejection = Ejection::Device(device.instance.clone());
        let note = match asked.get(&ejection) {
            Some(Asked::Ejecting) => language.say("Ejecting…").to_owned(),
            Some(Asked::Refused(refusal)) => refusal_text(refusal, language),
            None => String::new(),
        };
        let ejecting = asked.get(&ejection) == Some(&Asked::Ejecting);
        let eject_state = self.rows[index].eject.state(window);
        let name = device.name.clone();
        let eject = (!ejecting).then(|| {
            Self::button(index, true, "Eject", eject_state, &theme, cx, move |cx| {
                Drives::eject(ejection.clone(), name.clone(), cx);
            })
        });
        list_row(&theme, ICON_USB, false, device.name.clone(), note)
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .children(eject)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::platform::Drive;

    fn drive(letter: char, device: bool) -> RemovableDrive {
        RemovableDrive {
            drive: Drive {
                root: PathBuf::from(format!("{letter}:\\")),
                label: "Stick".to_owned(),
                kind: DriveKind::Removable,
                total: Some(8 << 30),
                free: Some(2 << 30),
            },
            device: device.then(|| RemovableDevice {
                instance: format!("USB\\{letter}"),
                name: "SanDisk Cruzer".to_owned(),
            }),
        }
    }

    fn removables(drives: Vec<RemovableDrive>) -> Removables {
        Removables {
            drives,
            devices: Vec::new(),
        }
    }

    #[test]
    fn a_drive_is_named_by_its_letter() {
        assert_eq!(letter_of("E"), Some('E'));
        assert_eq!(letter_of("e:"), Some('E'));
        assert_eq!(letter_of(" F:\\ "), Some('F'));
        assert_eq!(letter_of("EF"), None);
        assert_eq!(letter_of("1"), None);
        assert_eq!(letter_of(""), None);
    }

    #[test]
    fn what_was_there_at_first_did_not_arrive() {
        let first = removables(vec![drive('E', true)]);
        assert!(arrived(None, &first).is_empty());
        let next = removables(vec![drive('E', true), drive('F', true)]);
        let came: Vec<char> = arrived(Some(&first), &next)
            .iter()
            .map(|drive| drive.letter())
            .collect();
        assert_eq!(came, ['F']);
        assert!(arrived(Some(&next), &first).is_empty());
    }

    #[test]
    fn a_refusal_is_kept_while_what_was_refused_is_there() {
        let now = removables(vec![drive('E', true), drive('G', false)]);
        assert!(holds(&now, &Ejection::Device("USB\\E".to_owned())));
        assert!(holds(&now, &Ejection::Medium('G')));
        assert!(!holds(&now, &Ejection::Medium('E')));
        assert!(!holds(&now, &Ejection::Device("USB\\F".to_owned())));
    }

    #[test]
    fn a_row_says_how_full_the_drive_is_or_how_its_ejection_stands() {
        let english = Language::English;
        let stick = drive('E', true);
        assert_eq!(
            drive_note(&stick, None, english),
            "2.0 GB free of 8.0 GB, SanDisk Cruzer"
        );
        assert_eq!(
            drive_note(&stick, Some(&Asked::Ejecting), english),
            "Ejecting…"
        );
        let held = Asked::Refused(EjectRefusal::InUse(Some("winword.exe".to_owned())));
        assert_eq!(
            drive_note(&stick, Some(&held), english),
            "It is in use by winword.exe."
        );
        assert_eq!(fullness(&stick), Some(0.75));
        assert_eq!(
            drive_note(&stick, None, Language::Russian),
            "2.0 GB свободно из 8.0 GB, SanDisk Cruzer"
        );
    }

    #[test]
    fn a_refusal_says_what_to_do() {
        let english = Language::English;
        assert_eq!(
            refusal_text(&EjectRefusal::InUse(None), english),
            "A program is using it; close its files and try again."
        );
        assert_eq!(
            refusal_text(&EjectRefusal::Refused("refusal 6".to_owned()), english),
            "Windows refused to eject it. (refusal 6)"
        );
    }

    #[test]
    fn an_arrival_offers_to_open_and_to_eject() {
        let notice = arrival_notice(&drive('E', true), Language::English);
        assert_eq!(notice.title, "A drive was plugged in");
        assert_eq!(notice.body, "Stick (E:), SanDisk Cruzer");
        assert_eq!(notice.actions.len(), 2);
        let card = arrival_notice(&drive('G', false), Language::English);
        assert_eq!(card.title, "A disc or a card was put in");
        assert_eq!(card.body, "Stick (G:)");
    }
}
