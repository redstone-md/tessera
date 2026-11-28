// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Tessera keeps: those of the Windows notification
//! center and the history of those that left it, with what their toasts
//! hold (pictures, progress bars, input fields and buttons), and the jobs
//! on files that run. Widgets: `notifications`, the list, grouped by
//! application and by the headers of the toasts, for a popup, which takes
//! the keys when its popup is opened by a key; `notifications-button`,
//! a bell in the bar with the count of notifications that came since the
//! list was last open, which opens the popup `notifications`; and `focus`,
//! a switch for do not disturb. Actions: `quiet`, which switches do not
//! disturb, and `notify`, which posts a notice of Tessera's own.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, AsyncApp, Context, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ListAlignment, ListState, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window, div, img, list, point, px, relative,
};

use crate::actions::{Action, Actions};
use crate::banners::{APP_GLYPH, Banners, FocusChoice, Hush, Notice};
use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::menu::{MenuEntry, Menus};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::notifications_store::{Entry, NotificationStore, Press, groups, unix_now};
use crate::platform::{FileJobs, Launch, Occupation, QuietProfile, pointer_on_own_window, reveal};
use crate::popup::{PopupAnchor, Popups};
use crate::theme::Theme;
use crate::toast_view::{self, Control, OnPress, ToastControls, ToastPress};
use crate::ui::{icon, one_line, pill, state_layer, switch};

/// How often the ages of the notifications and the state of do not
/// disturb are looked at again, as time passes without a change.
const REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// The height of one notification while its text is not unrolled: a
/// title and two lines of text. It is fixed so that a notification can
/// grow in and shrink away.
const ITEM_HEIGHT: Pixels = px(84.);

/// How many notifications of an application are shown while its group is
/// rolled up.
const COLLAPSED: usize = 2;

/// A text longer than this, or one of several lines, may not fit in two
/// lines, so its notification can be unrolled.
const LONG_TEXT: usize = 80;

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// The side of the logo of an application in the list.
const LOGO: Pixels = px(20.);

/// The height of the row of a header of toasts.
const HEADER_ROW: f32 = 24.;

/// The height of the row of a job on files.
const JOB_ROW: f32 = 76.;

/// The height of the row of buttons of a job that ended.
const DESTINATION_ROW: f32 = 34.;

/// How far beyond what is in sight the list draws rows.
const LIST_OVERDRAW: f32 = 200.;

/// The glyph of an entry of the menu of a notification.
const MENU_GLYPH: &str = "\u{E8A7}";

/// The height of the row of the `focus` widget.
const FOCUS_ROW: f32 = 40.;

/// The popup a click on `notifications-button` opens when the layout
/// names none.
const POPUP: &str = "notifications";

/// The glyph of the button that dismisses a notification.
const DISMISS_GLYPH: &str = "\u{E711}";

/// The glyph of do not disturb.
const QUIET_GLYPH: &str = "\u{E708}";

/// The glyph of the button of the bar.
const BELL_GLYPH: &str = "\u{E91C}";

/// The glyphs that roll a group or a text down and up.
const MORE_GLYPH: &str = "\u{E70D}";
const LESS_GLYPH: &str = "\u{E70E}";

/// What the moon button offers while do not disturb does not hold.
const CHOICES: [(&str, FocusChoice); 3] = [
    ("For 1 hour", FocusChoice::Minutes(60)),
    ("Until morning", FocusChoice::Morning),
    ("Until turned off", FocusChoice::On),
];

pub struct NotificationsModule;

impl Module for NotificationsModule {
    fn name(&self) -> &'static str {
        "notifications"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("notifications", |_, cx| {
            cx.new(NotificationList::new).into()
        })?;
        widgets.register("notifications-button", |_, cx| cx.new(Bell::new).into())?;
        widgets.register_panel("focus", |_, cx| cx.new(Focus::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: Default::default(),
            width: 380.,
            height: 560.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "notifications".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })?;
        widgets.open_with("notifications-button", POPUP);
        Ok(())
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let quiet = Action::new(|arg, cx| match FocusChoice::parse(arg) {
            Ok(choice) => Banners::focus(choice, cx),
            Err(error) => report!("{error}"),
        })
        .checking(|arg| FocusChoice::parse(arg).map(drop));
        actions.register("quiet", quiet)?;
        let notify = Action::new(|arg, cx| {
            if let Some(notice) = notice_of(arg) {
                Banners::post(notice, cx);
            }
        })
        .checking(|arg| match notice_of(arg) {
            Some(_) => Ok(()),
            None => Err("`notify` takes the text of the notice as `arg`".to_owned()),
        });
        actions.register("notify", notify)
    }
}

/// The notice `tessera do notify <arg>` posts: the title, and after ` | `
/// the text under it.
fn notice_of(arg: Option<&str>) -> Option<Notice> {
    let arg = arg.map(str::trim).filter(|arg| !arg.is_empty())?;
    let (title, body) = arg.split_once(" | ").unwrap_or((arg, ""));
    Some(Notice {
        app: "Tessera".to_owned(),
        title: title.trim().to_owned(),
        body: body.trim().to_owned(),
        ..Notice::default()
    })
}

/// Calls `changed` on the view whenever the notifications or do not
/// disturb change, and `ticked` as time passes, for as long as it lives.
fn follow<V: 'static>(cx: &mut Context<V>, changed: fn(&mut V, &mut Context<V>)) {
    cx.observe_global::<NotificationStore>(changed).detach();
    cx.observe_global::<Banners>(changed).detach();
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(REFRESH_INTERVAL).await;
            if this.update(cx, changed).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// How long ago something happened `seconds` ago, in a few characters.
fn age(seconds: i64, language: Language) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    match seconds {
        ..MINUTE => language.say("now").to_owned(),
        MINUTE..HOUR => format!("{} {}", seconds / MINUTE, language.say("min")),
        HOUR..DAY => format!("{} {}", seconds / HOUR, language.say("h")),
        _ => format!("{} {}", seconds / DAY, language.say("d")),
    }
}

/// The time of the day of `unix`, seconds since 1970, as `15:30`.
fn clock(unix: i64) -> String {
    let time = Local.timestamp_opt(unix, 0).earliest();
    time.map_or_else(String::new, |time| time.format("%H:%M").to_string())
}

/// Why do not disturb holds, in a few words.
fn describe(hush: Hush, language: Language) -> String {
    match hush {
        Hush::On => language.say("Until turned off").to_owned(),
        Hush::Until(until) => format!("{} {}", language.say("On until"), clock(until)),
        Hush::Scheduled(end) => format!(
            "{} {}",
            language.say("As scheduled, until"),
            end.format("%H:%M")
        ),
        Hush::Busy(busy) => language
            .say(match busy {
                Occupation::Fullscreen => "While an app fills the screen",
                Occupation::Game => "While a game runs",
                Occupation::Presenting => "While presenting",
                Occupation::Duplicating => "While a display is duplicated",
            })
            .to_owned(),
        Hush::Windows(QuietProfile::AlarmsOnly) => language.say("Windows: alarms only").to_owned(),
        Hush::Windows(_) => language.say("Windows: Do not disturb").to_owned(),
    }
}

/// The mark of an application: its logo, or else a glyph.
fn mark(app_id: &str, side: Pixels, theme: &Theme, cx: &gpui::App) -> AnyElement {
    let logo = { cx.try_global::<NotificationStore>() }.and_then(|store| store.logo(app_id));
    match logo {
        Some(logo) => img(logo).size(side).flex_none().into_any_element(),
        None => icon(theme, APP_GLYPH)
            .flex_none()
            .text_color(theme.colors.primary)
            .into_any_element(),
    }
}

/// A small round button with `glyph`.
fn round_button(theme: &Theme, glyph: &'static str, ground: gpui::Hsla) -> gpui::Div {
    let caption_size = theme.typography.label_size * CAPTION_SHARE;
    icon(theme, glyph)
        .size(theme.typography.icon_size + theme.spacing.small)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.shape.full)
        .text_size(caption_size)
        .text_color(theme.colors.on_surface_variant)
        .bg(ground)
}

/// One notification of the list and the motion it is in.
struct Item {
    entry: Entry,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    dismiss: Interaction,
    /// Rolls its text down and up.
    unroll: Interaction,
    /// It was dismissed and is shrinking away.
    leaving: bool,
    /// The fields and the choices of its toast.
    controls: Option<ToastControls>,
}

impl Item {
    /// Takes the controls of the toast its entry has now, keeping what was
    /// typed while the toast stays the same.
    fn take_controls(&mut self, cx: &mut App) {
        let same = match (&self.controls, &self.entry.toast) {
            (Some(controls), Some(toast)) => Arc::ptr_eq(controls.toast(), toast),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.controls = { self.entry.toast.clone() }.map(|toast| ToastControls::new(toast, cx));
        }
    }

    /// Its height while its text is not unrolled.
    fn height(&self, theme: &Theme) -> Pixels {
        let toast = { self.entry.toast.as_deref() }
            .map_or(px(0.), |toast| toast_view::extra_height(toast, theme));
        let destination = match self.entry.destination.is_some() {
            true => px(DESTINATION_ROW),
            false => px(0.),
        };
        ITEM_HEIGHT + toast + destination
    }
}

/// The buttons at the head of the group of an application.
struct GroupButtons {
    clear: Interaction,
    unroll: Interaction,
}

/// One row of the list, which the list draws only while it is in sight.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Row {
    /// The heading of the jobs on files.
    Jobs,
    /// A job on files, by its number.
    Job(u64),
    /// The head of the group of an application, by its key.
    Group(String),
    /// The header of the toasts of an application that share it: the key
    /// of the group, the identifier of the header and the newest
    /// notification under it.
    Header(String, String, u64),
    /// A notification, by its key.
    Item(u64),
}

/// The places in `members` (of `items`, newest first) ordered by the
/// header of their toasts, as Windows groups them: the notifications of
/// one header stand together, the header with the newest first, and those
/// without one keep their places among them. Each comes with the
/// identifier of its header.
fn by_header<'a>(
    members: &[usize],
    header: impl Fn(usize) -> Option<&'a str>,
) -> Vec<(usize, Option<&'a str>)> {
    let mut order: Vec<Option<&str>> = Vec::new();
    for &at in members {
        let id = header(at);
        if id.is_none() || !order.contains(&id) {
            order.push(id);
        }
    }
    let mut placed = Vec::with_capacity(members.len());
    let mut taken = vec![false; members.len()];
    for id in order {
        for (place, &at) in members.iter().enumerate() {
            if !taken[place] && header(at) == id {
                taken[place] = true;
                placed.push((at, id));
                if id.is_none() {
                    break;
                }
            }
        }
    }
    placed
}

/// The span of rows that changed between `old` and `new`: where they
/// start to differ, how many old rows the span had and how many new ones
/// it has; `None` where they are the same.
fn changed_span<T: PartialEq>(old: &[T], new: &[T]) -> Option<(usize, usize, usize)> {
    let start = old
        .iter()
        .zip(new)
        .take_while(|(one, other)| one == other)
        .count();
    if start == old.len() && start == new.len() {
        return None;
    }
    let tail = { old[start..].iter().rev().zip(new[start..].iter().rev()) }
        .take_while(|(one, other)| one == other)
        .count();
    Some((start, old.len() - start - tail, new.len() - start - tail))
}

/// Lists the notifications by application, the application with the
/// newest one first, under the jobs on files that run. The list draws
/// only the rows in sight. Opened from the keyboard, the keys go to its
/// first notification.
struct NotificationList {
    /// In the order of the store, including notifications that are still
    /// leaving.
    items: Vec<Item>,
    groups: HashMap<String, GroupButtons>,
    /// The groups that show all their notifications.
    unrolled_groups: HashSet<String>,
    /// The notifications that show all their text.
    unrolled: HashSet<u64>,
    clear: Interaction,
    quiet: Interaction,
    /// How far the moon button is switched on.
    quietness: Spring,
    /// Whether the choices of how long do not disturb holds are shown.
    choosing: bool,
    choices: [Interaction; CHOICES.len()],
    /// The rows as they were drawn last.
    rows: Vec<Row>,
    /// The rows whose height may have changed since they were drawn.
    stale: HashSet<Row>,
    list: ListState,
    focus: FocusHandle,
    /// The notification the keys are on, and the control of it.
    selected: Option<u64>,
    control: Option<Control>,
    /// Hears when the window of the list becomes active.
    activation: Option<Subscription>,
}

impl NotificationList {
    fn new(cx: &mut Context<Self>) -> Self {
        follow(cx, Self::sync);
        let motion = cx.global::<Theme>().motion;
        let quiet = Banners::hush(cx).is_some();
        let mut this = Self {
            items: Vec::new(),
            groups: HashMap::new(),
            unrolled_groups: HashSet::new(),
            unrolled: HashSet::new(),
            clear: Interaction::new(&motion),
            quiet: Interaction::new(&motion),
            quietness: Spring::new(motion.effects, if quiet { 1. } else { 0. }),
            choosing: false,
            choices: std::array::from_fn(|_| Interaction::new(&motion)),
            rows: Vec::new(),
            stale: HashSet::new(),
            list: ListState::new(0, ListAlignment::Top, px(LIST_OVERDRAW)),
            focus: cx.focus_handle(),
            selected: None,
            control: None,
            activation: None,
        };
        this.sync(cx);
        this
    }

    /// Takes over what the store holds: known notifications stay, new ones
    /// grow in at their place, missing ones start leaving. Everything
    /// listed counts as seen.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(store) = cx.try_global::<NotificationStore>() else {
            cx.notify();
            return;
        };
        let motion = cx.global::<Theme>().motion;
        let now = unix_now();
        let entries: Vec<Entry> = { store.entries().iter() }
            .filter(|entry| entry.listed(now))
            .cloned()
            .collect();
        // The jobs move without the rows changing.
        let jobs: Vec<Row> = { store.jobs().iter() }
            .map(|job| Row::Job(job.event.id))
            .collect();
        self.stale.extend(jobs);
        for item in &mut self.items {
            let present = entries.iter().any(|entry| entry.key == item.entry.key);
            if !present && !item.leaving {
                item.leaving = true;
                item.presence.animate_to(0.);
            }
        }
        // The store is newest first, so a new notification goes before
        // the first older one that is shown.
        for entry in entries {
            if let Some(item) = self
                .items
                .iter_mut()
                .find(|item| item.entry.key == entry.key)
            {
                if item.entry != entry {
                    item.entry = entry;
                    item.take_controls(cx);
                    self.stale.insert(Row::Item(item.entry.key));
                }
                continue;
            }
            let at = { self.items.iter() }
                .position(|item| item.entry.posted < entry.posted)
                .unwrap_or(self.items.len());
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            let mut item = Item {
                entry,
                presence,
                dismiss: Interaction::new(&motion),
                unroll: Interaction::new(&motion),
                leaving: false,
                controls: None,
            };
            item.take_controls(cx);
            self.items.insert(at, item);
        }
        cx.defer(NotificationStore::see);
        cx.notify();
    }

    fn item(&mut self, key: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| item.entry.key == key)
    }

    /// Has the notifications `keys` leave at once and takes them away.
    fn dismiss(&mut self, keys: Vec<u64>, cx: &mut Context<Self>) {
        for item in self
            .items
            .iter_mut()
            .filter(|item| keys.contains(&item.entry.key))
        {
            item.leaving = true;
            item.presence.animate_to(0.);
        }
        cx.defer(move |cx| NotificationStore::dismiss(&keys, cx));
        cx.notify();
    }

    /// Does what `press` on the notification `key` asks, with what was
    /// typed into it; it leaves unless it was only a choice.
    fn press(&mut self, key: u64, press: Press, cx: &mut Context<Self>) {
        let Some(item) = self.item(key) else {
            return;
        };
        let values = { item.controls.as_ref() }
            .map(|controls| controls.values(cx))
            .unwrap_or_default();
        item.leaving = true;
        item.presence.animate_to(0.);
        cx.defer(move |cx| NotificationStore::activate(key, press, &values, cx));
        cx.notify();
    }

    /// Does what was pressed on the parts of the toast of `key`.
    fn press_toast(&mut self, key: u64, press: ToastPress, cx: &mut Context<Self>) {
        match press {
            ToastPress::Action(at) => self.press(key, Press::Action(at), cx),
            ToastPress::Choose { input, choice } => {
                if let Some(controls) = self.item(key).and_then(|item| item.controls.as_mut()) {
                    controls.choose(input, choice);
                    self.stale.insert(Row::Item(key));
                    cx.notify();
                }
            }
        }
    }

    /// Takes every notification away.
    fn clear(&mut self, cx: &mut Context<Self>) {
        for item in &mut self.items {
            item.leaving = true;
            item.presence.animate_to(0.);
        }
        cx.defer(NotificationStore::clear);
        cx.notify();
    }

    /// The moon button: switches do not disturb off where it holds, and
    /// otherwise offers how long it is to hold.
    fn press_quiet(&mut self, cx: &mut Context<Self>) {
        if Banners::hush(cx).is_some() {
            self.choosing = false;
            cx.defer(|cx| Banners::focus(Some(FocusChoice::Off), cx));
        } else {
            self.choosing = !self.choosing;
        }
        cx.notify();
    }

    fn choose(&mut self, choice: FocusChoice, cx: &mut Context<Self>) {
        self.choosing = false;
        cx.defer(move |cx| Banners::focus(Some(choice), cx));
        cx.notify();
    }

    /// Opens the menu of the notification `key` at `at`, a point of the
    /// window: the entries its toast puts there, and Dismiss.
    fn open_menu(&mut self, key: u64, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.iter().find(|item| item.entry.key == key) else {
            return;
        };
        let language = Language::of(cx);
        let this = cx.entity().downgrade();
        let mut entries: Vec<MenuEntry> = Vec::new();
        if let Some(toast) = &item.entry.toast {
            for (index, action) in toast
                .actions
                .iter()
                .enumerate()
                .filter(|(_, action)| action.context_menu)
            {
                let this = this.clone();
                let label = toast_view::action_label(action, language);
                entries.push(MenuEntry::new(MENU_GLYPH, label, move |cx| {
                    this.update(cx, |this, cx| this.press(key, Press::Action(index), cx))
                        .ok();
                }));
            }
        }
        let gone = this.clone();
        entries.push(MenuEntry::new(
            DISMISS_GLYPH,
            language.say("Dismiss"),
            move |cx| {
                gone.update(cx, |this, cx| this.dismiss(vec![key], cx)).ok();
            },
        ));
        match PopupAnchor::at_point(window, at, cx) {
            Ok(anchor) => Menus::open(entries, anchor, cx),
            Err(error) => report!("the menu of a notification has no place: {error:#}"),
        }
    }

    /// Gives the keys to the first notification, where the window became
    /// active while the pointer is not on Tessera, as it does when the
    /// popup of the list is opened by a key.
    fn take_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !window.is_window_active() || pointer_on_own_window() {
            return;
        }
        self.focus.focus(window);
        self.control = None;
        let first = self.rows.iter().find_map(|row| match row {
            Row::Item(key) => Some(*key),
            _ => None,
        });
        self.select(first, cx);
    }

    /// Puts the keys on the notification `key`, and has the list show it.
    fn select(&mut self, key: Option<u64>, cx: &mut Context<Self>) {
        for old in self.selected.iter().chain(key.iter()) {
            self.stale.insert(Row::Item(*old));
        }
        self.selected = key;
        if let Some(row) =
            key.and_then(|key| self.rows.iter().position(|row| *row == Row::Item(key)))
        {
            self.list.scroll_to_reveal_item(row);
        }
        cx.notify();
    }

    /// The notifications in the order they are drawn.
    fn shown_keys(&self) -> Vec<u64> {
        { self.rows.iter() }
            .filter_map(|row| match row {
                Row::Item(key) => Some(*key),
                _ => None,
            })
            .collect()
    }

    /// The field the keys are in, if one of the selected notification has
    /// them.
    fn typing_in(&self, window: &Window, cx: &App) -> Option<usize> {
        let key = self.selected?;
        let controls = self
            .items
            .iter()
            .find(|item| item.entry.key == key)?
            .controls
            .as_ref()?;
        (0..controls.toast().inputs.len()).find(|&input| {
            controls
                .field(input)
                .is_some_and(|field| field.read(cx).is_focused(window))
        })
    }

    /// Moves the keys to the next or the previous control of the selected
    /// notification, and back to the notification after the last.
    fn step_control(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected else {
            return;
        };
        let Some(item) = self.items.iter().find(|item| item.entry.key == key) else {
            return;
        };
        let controls = { item.entry.toast.as_deref() }
            .map(toast_view::controls)
            .unwrap_or_default();
        let at = self
            .control
            .and_then(|control| controls.iter().position(|c| *c == control));
        let next = match (at, back) {
            (None, false) => controls.first().copied(),
            (None, true) => controls.last().copied(),
            (Some(at), false) => controls.get(at + 1).copied(),
            (Some(0), true) => None,
            (Some(at), true) => controls.get(at - 1).copied(),
        };
        self.control = next;
        let field = match next {
            Some(Control::Field(input)) => {
                { item.controls.as_ref() }.and_then(|controls| controls.field(input).cloned())
            }
            _ => None,
        };
        match field {
            Some(field) => field.read(cx).focus(window),
            None => self.focus.focus(window),
        }
        self.stale.insert(Row::Item(key));
        cx.notify();
    }

    /// The keys of the list: Up and Down move between the notifications,
    /// Enter does what the one or its control in focus does, Delete
    /// dismisses it, Tab goes through its fields and buttons, the menu key
    /// opens its menu, and Escape closes the popup.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let typing = self.typing_in(window, cx);
        let shift = event.keystroke.modifiers.shift;
        let keys = self.shown_keys();
        let at = self
            .selected
            .and_then(|key| keys.iter().position(|k| *k == key));
        match event.keystroke.key.as_str() {
            "escape" if typing.is_some() => {
                self.control = None;
                self.focus.focus(window);
            }
            "escape" => cx.defer(Popups::close),
            "tab" => self.step_control(shift, window, cx),
            "down" if typing.is_none() => {
                let next = at.map_or(0, |at| (at + 1).min(keys.len().saturating_sub(1)));
                self.control = None;
                self.select(keys.get(next).copied(), cx);
            }
            "up" if typing.is_none() => {
                let next = at.map_or(0, |at| at.saturating_sub(1));
                self.control = None;
                self.select(keys.get(next).copied(), cx);
            }
            "delete" if typing.is_none() => {
                let Some(at) = at else {
                    return;
                };
                let next = keys
                    .get(at + 1)
                    .or_else(|| at.checked_sub(1).and_then(|before| keys.get(before)));
                let next = next.copied();
                self.dismiss(vec![keys[at]], cx);
                self.control = None;
                self.select(next, cx);
            }
            "enter" => self.enter(window, cx),
            "menu" => self.menu_from_keys(window, cx),
            "f10" if shift => self.menu_from_keys(window, cx),
            _ => {}
        }
    }

    /// Does what Enter does on the selected notification.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected else {
            return;
        };
        let typing = self.typing_in(window, cx);
        let toast = self
            .items
            .iter()
            .find(|item| item.entry.key == key)
            .and_then(|item| item.entry.toast.clone());
        let control = typing.map(Control::Field).or(self.control);
        match (control, toast) {
            (Some(Control::Field(input)), Some(toast)) => {
                if let Some(action) = toast_view::send_action(&toast, input) {
                    self.press(key, Press::Action(action), cx);
                }
            }
            (Some(Control::Choices(input)), _) => {
                if let Some(controls) = self.item(key).and_then(|item| item.controls.as_mut()) {
                    controls.choose_next(input);
                }
                self.stale.insert(Row::Item(key));
                cx.notify();
            }
            (Some(Control::Action(action)), _) => self.press(key, Press::Action(action), cx),
            _ => self.press(key, Press::Body, cx),
        }
    }

    /// Opens the menu of the selected notification at its lower left.
    fn menu_from_keys(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected else {
            return;
        };
        let Some(row) = self.rows.iter().position(|row| *row == Row::Item(key)) else {
            return;
        };
        let Some(bounds) = self.list.bounds_for_item(row) else {
            return;
        };
        let at = point(
            bounds.origin.x + px(16.),
            bounds.origin.y + bounds.size.height,
        );
        self.open_menu(key, at, window, cx);
    }

    /// The rows to draw, now.
    fn layout_rows(&self, cx: &App) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(store) = cx.try_global::<NotificationStore>()
            && !store.jobs().is_empty()
        {
            rows.push(Row::Jobs);
            rows.extend(store.jobs().iter().map(|job| Row::Job(job.event.id)));
        }
        for group in groups(&self.items, |item| &item.entry) {
            rows.push(Row::Group(group.key.clone()));
            let shown = match self.unrolled_groups.contains(&group.key) {
                true => group.members.len(),
                false => COLLAPSED,
            };
            let header = |at: usize| {
                let toast = self.items[at].entry.toast.as_deref()?;
                toast.header.as_ref().map(|header| header.id.as_str())
            };
            let mut last_header = None;
            for (at, id) in by_header(&group.members, header).into_iter().take(shown) {
                if let Some(id) = id.filter(|id| last_header != Some(*id)) {
                    rows.push(Row::Header(
                        group.key.clone(),
                        id.to_owned(),
                        self.items[at].entry.key,
                    ));
                }
                last_header = id;
                rows.push(Row::Item(self.items[at].entry.key));
            }
        }
        rows
    }

    /// Tells the list of the rows that changed since it last drew them,
    /// so that it measures them again.
    fn refresh_rows(&mut self, rows: Vec<Row>) {
        if let Some((start, old, new)) = changed_span(&self.rows, &rows) {
            self.list.splice(start..start + old, new);
        }
        self.rows = rows;
        let moving: Vec<Row> = { self.items.iter() }
            .filter(|item| !item.presence.is_settled())
            .map(|item| Row::Item(item.entry.key))
            .collect();
        self.stale.extend(moving);
        for (at, row) in self.rows.iter().enumerate() {
            if self.stale.contains(row) {
                self.list.splice(at..at + 1, 1);
            }
        }
        self.stale.clear();
    }

    /// Draws the row at `index`.
    fn render_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let Some(row) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        match row {
            Row::Jobs => div()
                .h(theme.bar.pill_height)
                .px(theme.spacing.tiny)
                .flex()
                .items_center()
                .font_weight(FontWeight::MEDIUM)
                .child(Language::of(cx).say("Jobs"))
                .into_any_element(),
            Row::Job(id) => self.render_job(id, theme, cx),
            Row::Group(key) => {
                let members: Vec<usize> = { self.items.iter().enumerate() }
                    .filter(|(_, item)| item.entry.group() == key)
                    .map(|(at, _)| at)
                    .collect();
                let Some(&first) = members.first() else {
                    return div().into_any_element();
                };
                let (app, app_id) = (
                    self.items[first].entry.app.clone(),
                    self.items[first].entry.app_id.clone(),
                );
                self.render_group_head(&key, &app, &app_id, &members, theme, window, cx)
            }
            Row::Header(_, _, newest) => {
                let title = { self.items.iter().find(|item| item.entry.key == newest) }
                    .and_then(|item| {
                        item.entry
                            .toast
                            .as_ref()?
                            .header
                            .as_ref()
                            .map(|header| header.title.clone())
                    })
                    .unwrap_or_default();
                div()
                    .id(("toast-header", newest as usize))
                    .h(px(HEADER_ROW))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .text_size(theme.typography.label_size * CAPTION_SHARE)
                    .text_color(theme.colors.primary)
                    .cursor_pointer()
                    .child(one_line(title))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.press(newest, Press::Header, cx)),
                    )
                    .into_any_element()
            }
            Row::Item(key) => {
                let now = unix_now();
                let unrolled = self.unrolled.contains(&key);
                let selected = self.selected == Some(key);
                let control = self.control.filter(|_| selected);
                let Some(at) = self.items.iter().position(|item| item.entry.key == key) else {
                    return div().into_any_element();
                };
                self.render_item(at, now, unrolled, selected, control, theme, window, cx)
            }
        }
    }

    /// A job on files: what it does, how far it is, and its Cancel.
    fn render_job(&mut self, id: u64, theme: Theme, cx: &mut Context<Self>) -> AnyElement {
        let language = Language::of(cx);
        let Some(job) = { cx.try_global::<NotificationStore>() }
            .and_then(|store| store.jobs().iter().find(|job| job.event.id == id).cloned())
        else {
            return div().into_any_element();
        };
        let caption = theme.typography.label_size * CAPTION_SHARE;
        let fill = match job.share() {
            Some(share) => div().h_full().w(relative(share)).bg(theme.colors.primary),
            None => div()
                .h_full()
                .w(relative(0.3))
                .bg(theme.colors.primary)
                .opacity(0.6),
        };
        let cancel = job.event.can_cancel.then(|| {
            div()
                .id(("job-cancel", id as usize))
                .flex_none()
                .px(theme.spacing.small)
                .py(theme.spacing.tiny)
                .rounded(theme.shape.full)
                .text_color(theme.colors.primary)
                .hover(|style| style.bg(theme.colors.surface_container))
                .child(language.say("Cancel"))
                .on_click(cx.listener(move |_, _, _, _| FileJobs::cancel(id)))
        });
        div()
            .w_full()
            .pb(theme.spacing.small)
            .child(
                div()
                    .h(px(JOB_ROW) - theme.spacing.small)
                    .px(theme.spacing.medium - theme.spacing.tiny)
                    .py(theme.spacing.small)
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(4.))
                    .rounded(theme.shape.medium)
                    .bg(theme.colors.surface_container_highest)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(theme.spacing.small)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(one_line(job.event.title.clone())),
                            )
                            .children(cancel),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .rounded(theme.shape.full)
                            .overflow_hidden()
                            .bg(theme.colors.surface_container)
                            .child(fill.rounded(theme.shape.full)),
                    )
                    .child(
                        div()
                            .text_size(caption)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(job.progress_text(language))),
                    ),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments, reason = "the parts of one card")]
    fn render_item(
        &mut self,
        at: usize,
        now: i64,
        unrolled: bool,
        selected: bool,
        control: Option<Control>,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let item = &mut self.items[at];
        let (presence, dismiss_state, unroll_state) = (
            item.presence.value(window),
            item.dismiss.state(window),
            item.unroll.state(window),
        );
        let (entry, height) = (item.entry.clone(), item.height(&theme));
        let key = entry.key;
        let language = Language::of(cx);
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let ground = theme.colors.surface_container_highest;

        let dismiss = round_button(
            &theme,
            DISMISS_GLYPH,
            state_layer(&theme, ground, dismiss_state),
        )
        .id(("dismiss", key as usize))
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.dismiss(vec![key], cx);
        }));
        let dismiss = Interaction::track(dismiss, cx, move |this: &mut Self| {
            Some(&mut this.item(key)?.dismiss)
        });
        let long = entry.body.chars().count() > LONG_TEXT || entry.body.contains('\n');
        let unroll = long.then(|| {
            let glyph = if unrolled { LESS_GLYPH } else { MORE_GLYPH };
            let button = round_button(&theme, glyph, state_layer(&theme, ground, unroll_state))
                .id(("unroll", key as usize))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    if !this.unrolled.remove(&key) {
                        this.unrolled.insert(key);
                    }
                    this.stale.insert(Row::Item(key));
                    cx.notify();
                }));
            Interaction::track(button, cx, move |this: &mut Self| {
                Some(&mut this.item(key)?.unroll)
            })
        });

        let (headline, text) = match (entry.title.as_str(), entry.body.as_str()) {
            ("", body) => (body.lines().next().unwrap_or_default(), ""),
            (title, body) => (title, body),
        };
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.on_surface)
                    .child(one_line(headline.to_owned())),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(caption_size)
                    .text_color(theme.colors.on_surface_variant)
                    .child(age(now - entry.posted, language)),
            )
            .children(unroll)
            .child(dismiss);
        let body = (!text.is_empty()).then(|| {
            let body = div()
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(SharedString::from(text.to_owned()));
            if unrolled { body } else { body.line_clamp(2) }
        });
        let toast = entry.toast.clone();
        let hero = { toast.as_deref() }
            .and_then(|toast| toast_view::hero(toast, &entry.app_id, &theme, cx));
        let parts = match &self.items[at].controls {
            Some(controls) => {
                let on_press: OnPress<Self> =
                    Rc::new(move |this: &mut Self, press, _, cx| this.press_toast(key, press, cx));
                toast_view::parts(key, controls, &entry.app_id, control, &theme, on_press, cx)
            }
            None => Vec::new(),
        };
        let destination = entry.destination.clone().map(|folder| {
            let shown = folder.clone();
            let button = |id: &'static str, label: &str| {
                div()
                    .id((id, key as usize))
                    .flex_none()
                    .h(px(DESTINATION_ROW - 6.))
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .rounded(theme.shape.full)
                    .bg(theme.colors.surface_container)
                    .hover(|style| style.bg(theme.colors.secondary_container))
                    .text_color(theme.colors.primary)
                    .child(one_line(label.to_owned()))
            };
            div()
                .flex_none()
                .mt(px(6.))
                .flex()
                .gap(theme.spacing.small)
                .child(
                    button("open-destination", language.say("Open destination")).on_click(
                        move |_, _, _| {
                            Launch::new(&folder).start();
                        },
                    ),
                )
                .child(
                    button("show-destination", language.say("Show in folder")).on_click(
                        move |_, _, _| {
                            if let Err(error) = reveal(&shown) {
                                report!("{error:#}");
                            }
                        },
                    ),
                )
        });
        let openable = entry.toast.is_some() || (entry.id.is_some() && !entry.app_id.is_empty());
        let card = div()
            .id(("notification", key as usize))
            .px(theme.spacing.medium - theme.spacing.tiny)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(px(2.))
            .overflow_hidden()
            .rounded(theme.shape.medium)
            .bg(ground)
            .when(selected, |card| {
                card.border_2().border_color(theme.colors.primary)
            })
            .children(hero)
            .child(header)
            .children(body)
            .children(parts)
            .children(destination)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.open_menu(key, event.position, window, cx);
                }),
            );
        let card = if openable {
            card.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.press(key, Press::Body, cx)))
        } else {
            card
        };
        // The slot includes the gap below the card, so that the cards
        // below move smoothly when one comes or goes. An unrolled text
        // takes the height it needs.
        let slot = div()
            .w_full()
            .flex_none()
            .pb(theme.spacing.small)
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.));
        if unrolled {
            slot.child(card).into_any_element()
        } else {
            slot.h(height * presence.max(0.))
                .child(card.h(height - theme.spacing.small))
                .into_any_element()
        }
    }

    /// The head of the group `key`: the logo and the name of the
    /// application, how many notifications it has, and buttons that roll
    /// the group down and that clear it.
    #[allow(clippy::too_many_arguments, reason = "the parts of one row")]
    fn render_group_head(
        &mut self,
        key: &str,
        app: &str,
        app_id: &str,
        members: &[usize],
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let motion = theme.motion;
        let buttons = { self.groups.entry(key.to_owned()) }.or_insert_with(|| GroupButtons {
            clear: Interaction::new(&motion),
            unroll: Interaction::new(&motion),
        });
        let (clear_state, unroll_state) =
            (buttons.clear.state(window), buttons.unroll.state(window));
        let language = Language::of(cx);
        let ground = theme.colors.surface_container;
        let name = if app.is_empty() {
            language.say("Unknown application").to_owned()
        } else {
            app.to_owned()
        };
        let count = members.len();
        let keys: Vec<u64> = { members.iter().map(|&at| &self.items[at]) }
            .filter(|item| !item.leaving)
            .map(|item| item.entry.key)
            .collect();
        let group = key.to_owned();
        let clear = round_button(
            &theme,
            DISMISS_GLYPH,
            state_layer(&theme, ground, clear_state),
        )
        .id(SharedString::from(format!("clear-group-{key}")))
        .on_click(cx.listener(move |this, _, _, cx| this.dismiss(keys.clone(), cx)));
        let found = group.clone();
        let clear = Interaction::track(clear, cx, move |this: &mut Self| {
            Some(&mut this.groups.get_mut(&found)?.clear)
        });
        let unroll = (count > COLLAPSED).then(|| {
            let unrolled = self.unrolled_groups.contains(key);
            let glyph = if unrolled { LESS_GLYPH } else { MORE_GLYPH };
            let toggled = group.clone();
            let button = round_button(&theme, glyph, state_layer(&theme, ground, unroll_state))
                .id(SharedString::from(format!("unroll-group-{key}")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.unrolled_groups.remove(&toggled) {
                        this.unrolled_groups.insert(toggled.clone());
                    }
                    cx.notify();
                }));
            let found = group.clone();
            Interaction::track(button, cx, move |this: &mut Self| {
                Some(&mut this.groups.get_mut(&found)?.unroll)
            })
        });
        div()
            .h(theme.bar.pill_height)
            .flex_none()
            .px(theme.spacing.tiny)
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(mark(app_id, LOGO, &theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.on_surface)
                    .child(one_line(name)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::from(count.to_string())),
            )
            .children(unroll)
            .child(clear)
            .into_any_element()
    }

    /// The choices of how long do not disturb holds.
    fn render_choices(
        &mut self,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let language = Language::of(cx);
        let chips: Vec<AnyElement> = CHOICES
            .iter()
            .enumerate()
            .map(|(index, &(label, choice))| {
                let state = self.choices[index].state(window);
                let chip = div()
                    .id(("quiet-choice", index))
                    .flex_1()
                    .min_w_0()
                    .h(theme.bar.pill_height - theme.spacing.tiny)
                    .px(theme.spacing.small)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.shape.full)
                    .text_size(theme.typography.label_size * CAPTION_SHARE)
                    .text_color(theme.colors.on_surface)
                    .bg(state_layer(
                        &theme,
                        theme.colors.surface_container_highest,
                        state,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(choice, cx)))
                    .child(one_line(language.say(label)));
                Interaction::track(chip, cx, move |this: &mut Self| this.choices.get_mut(index))
                    .into_any_element()
            })
            .collect();
        div()
            .flex_none()
            .pb(theme.spacing.small)
            .flex()
            .gap(theme.spacing.small)
            .children(chips)
            .into_any_element()
    }
}

impl Render for NotificationList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let store = cx.try_global::<NotificationStore>();
        let error = store.and_then(NotificationStore::error).map(str::to_owned);
        if self.activation.is_none() {
            let activation = cx.observe_window_activation(window, |this, window, cx| {
                this.take_keys(window, cx);
            });
            self.activation = Some(activation);
        }

        // A notification that has finished leaving was drawn at no height
        // the last time. Those out of sight move on with time too.
        let moment = Instant::now();
        for item in &mut self.items {
            item.presence.value_at(moment);
        }
        let before = self.items.len();
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));
        if self.items.len() != before
            && let Some(key) = self.selected
            && !self.items.iter().any(|item| item.entry.key == key)
        {
            self.selected = None;
            self.control = None;
        }
        let rows = self.layout_rows(cx);
        self.refresh_rows(rows);
        let waiting = self.items.iter().filter(|item| !item.leaving).count();
        let kept: HashSet<String> = { groups(&self.items, |item| &item.entry).into_iter() }
            .map(|group| group.key)
            .collect();
        self.groups.retain(|key, _| kept.contains(key));

        let state = self.clear.state(window);
        let clear = div()
            .id("clear-notifications")
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(cx.listener(|this, _, _, cx| this.clear(cx)))
            .child(language.say("Clear all"));
        let clear = Interaction::track(clear, cx, |this: &mut Self| Some(&mut this.clear));
        // Do not disturb is switched here while banners are shown, which
        // is all it holds back.
        let banners = Banners::config(cx).is_some_and(|config| config.banners());
        let hush = Banners::hush(cx);
        self.quietness
            .animate_to(if hush.is_some() { 1. } else { 0. });
        let quietness = self.quietness.value(window).clamp(0., 1.);
        let state = self.quiet.state(window);
        let ground = mix(
            theme.colors.surface_container,
            theme.colors.secondary_container,
            quietness,
        );
        let hold = icon(&theme, QUIET_GLYPH)
            .id("quiet-notifications")
            .size(theme.bar.pill_height - theme.spacing.tiny)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_color(mix(
                theme.colors.on_surface_variant,
                theme.colors.on_surface,
                quietness,
            ))
            .bg(state_layer(&theme, ground, state))
            .on_click(cx.listener(|this, _, _, cx| this.press_quiet(cx)));
        let hold = Interaction::track(hold, cx, |this: &mut Self| Some(&mut this.quiet));
        let header = div()
            .h(theme.bar.pill_height)
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .text_color(theme.colors.on_surface)
            .child(div().flex_1().child(language.say("Notifications")))
            .children((waiting > 0).then_some(clear))
            .children(banners.then_some(hold));
        // Windows' own do not disturb is told of even without banners, as
        // it holds back those of Windows.
        let quiet_note = hush
            .filter(|hush| banners || matches!(hush, Hush::Windows(_)))
            .map(|hush| {
                let text = match hush {
                    Hush::Windows(_) => describe(hush, language),
                    _ => format!(
                        "{} \u{b7} {}",
                        language.say("Do not disturb"),
                        describe(hush, language)
                    ),
                };
                div()
                    .flex_none()
                    .pb(theme.spacing.small)
                    .px(theme.spacing.tiny)
                    .text_size(theme.typography.label_size * CAPTION_SHARE)
                    .text_color(theme.colors.on_surface_variant)
                    .child(one_line(text))
            });
        let choices = (banners && self.choosing && hush.is_none())
            .then(|| self.render_choices(theme, window, cx));

        let note = |text: SharedString| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.colors.on_surface_variant)
                .child(text)
                .into_any_element()
        };
        let list = match (error, self.rows.is_empty()) {
            (Some(error), true) => note(SharedString::from(error)),
            (_, true) => note(language.say("No notifications").into()),
            (_, false) => {
                let this = cx.entity();
                list(self.list.clone(), move |index, window, cx| {
                    this.update(cx, |this, cx| this.render_row(index, window, cx))
                })
                .flex_1()
                .min_h(px(0.))
                .into_any_element()
            }
        };
        div()
            .id("notifications")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(theme.typography.label_size)
            .child(header)
            .children(quiet_note)
            .children(choices)
            .child(list)
    }
}

/// The bell in the bar: how many notifications came since the list was
/// last open, and a moon in its place while do not disturb holds.
struct Bell {
    unseen: usize,
    quiet: bool,
    /// 1 while there is a count to show.
    counted: Spring,
}

impl Bell {
    fn new(cx: &mut Context<Self>) -> Self {
        follow(cx, Self::read);
        let mut counted = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        let (unseen, quiet) = Self::state(cx);
        counted.jump_to(if unseen > 0 { 1. } else { 0. });
        Self {
            unseen,
            quiet,
            counted,
        }
    }

    fn state(cx: &gpui::App) -> (usize, bool) {
        let unseen = { cx.try_global::<NotificationStore>() }.map_or(0, NotificationStore::unseen);
        let banners = Banners::config(cx).is_some_and(|config| config.banners());
        (unseen, banners && Banners::hush(cx).is_some())
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let (unseen, quiet) = Self::state(cx);
        if (unseen, quiet) == (self.unseen, self.quiet) {
            return;
        }
        if unseen > self.unseen {
            // Each new notification shows.
            self.counted.kick(4.);
        }
        self.counted.animate_to(if unseen > 0 { 1. } else { 0. });
        // The count stays while it shrinks away.
        if unseen > 0 {
            self.unseen = unseen;
        }
        self.quiet = quiet;
        cx.notify();
    }
}

impl Render for Bell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let counted = self.counted.value(window);
        if self.counted.target() == 0. && self.counted.is_settled() {
            self.unseen = 0;
        }
        let glyph = if self.quiet { QUIET_GLYPH } else { BELL_GLYPH };
        let count = (self.unseen > 0).then(|| {
            div()
                .flex_none()
                .overflow_hidden()
                .max_w(px(48. * counted.max(0.)))
                .opacity(counted.clamp(0., 1.))
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(self.unseen.to_string()))
        });
        pill(&theme)
            .child(icon(&theme, glyph).text_color(if self.unseen > 0 {
                theme.colors.primary
            } else {
                theme.colors.on_surface
            }))
            .children(count)
    }
}

/// A row with a switch for do not disturb, and why it holds.
struct Focus {
    hush: Option<Hush>,
    /// 0 while off, 1 while on.
    travel: Spring,
    interaction: Interaction,
}

impl Focus {
    fn new(cx: &mut Context<Self>) -> Self {
        follow(cx, Self::read);
        let motion = cx.global::<Theme>().motion;
        let hush = Banners::hush(cx);
        Self {
            hush,
            travel: Spring::new(motion.spatial, if hush.is_some() { 1. } else { 0. }),
            interaction: Interaction::new(&motion),
        }
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let hush = Banners::hush(cx);
        if hush != self.hush {
            self.hush = hush;
            self.travel.animate_to(if hush.is_some() { 1. } else { 0. });
            cx.notify();
        }
    }
}

impl Render for Focus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let travel = self.travel.value(window);
        let state = self.interaction.state(window);
        let control = switch(&theme, "focus-switch", travel, state)
            .on_click(cx.listener(|_, _, _, cx| cx.defer(|cx| Banners::focus(None, cx))));
        let control =
            Interaction::track(control, cx, |this: &mut Self| Some(&mut this.interaction));
        let note = match self.hush {
            Some(hush) => describe(hush, language),
            None => language.say("Off").to_owned(),
        };
        // Without banners there is nothing for do not disturb to hold back.
        let banners = Banners::config(cx).is_some_and(|config| config.banners());
        let note = if banners {
            note
        } else {
            language.say("Banners are switched off").to_owned()
        };
        div()
            .h(px(FOCUS_ROW))
            .flex_none()
            .w_full()
            .px(theme.spacing.small)
            .flex()
            .items_center()
            .gap(theme.spacing.medium)
            .child(icon(&theme, QUIET_GLYPH).flex_none())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(one_line(language.say("Do not disturb"))),
                    )
                    .child(
                        div()
                            .text_size(theme.typography.label_size * CAPTION_SHARE)
                            .text_color(theme.colors.on_surface_variant)
                            .child(one_line(note)),
                    ),
            )
            .child(control)
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    #[test]
    fn an_age_is_told_in_the_largest_fitting_unit() {
        let age = |seconds| age(seconds, Language::English);
        assert_eq!(age(-5), "now");
        assert_eq!(age(59), "now");
        assert_eq!(age(60), "1 min");
        assert_eq!(age(3_599), "59 min");
        assert_eq!(age(7_200), "2 h");
        assert_eq!(age(3 * 86_400 + 5), "3 d");
        assert_eq!(super::age(7_200, Language::Russian), "2 ч");
    }

    #[test]
    fn do_not_disturb_says_why_it_holds() {
        let english = Language::English;
        assert_eq!(describe(Hush::On, english), "Until turned off");
        assert_eq!(
            describe(Hush::Busy(Occupation::Game), english),
            "While a game runs"
        );
        let end = NaiveDate::from_ymd_opt(2026, 3, 2)
            .and_then(|date| date.and_hms_opt(7, 0, 0))
            .unwrap();
        assert_eq!(
            describe(Hush::Scheduled(end), english),
            "As scheduled, until 07:00"
        );
    }

    #[test]
    fn toasts_of_one_header_stand_together() {
        // Newest first: 0 (inbox), 1 (none), 2 (news), 3 (inbox), 4 (none).
        let headers = [Some("inbox"), None, Some("news"), Some("inbox"), None];
        let members = [0, 1, 2, 3, 4];
        let placed = by_header(&members, |at| headers[at]);
        let order: Vec<usize> = placed.iter().map(|(at, _)| *at).collect();
        assert_eq!(order, [0, 3, 1, 2, 4]);
        assert_eq!(placed[1].1, Some("inbox"));
    }

    #[test]
    fn only_the_rows_that_changed_are_measured_again() {
        assert_eq!(changed_span(&[1, 2, 3], &[1, 2, 3]), None);
        assert_eq!(changed_span(&[1, 2, 3], &[1, 9, 3]), Some((1, 1, 1)));
        assert_eq!(changed_span(&[1, 3], &[1, 2, 3]), Some((1, 0, 1)));
        assert_eq!(changed_span(&[1, 2, 3], &[3]), Some((0, 2, 0)));
        assert_eq!(changed_span::<u8>(&[], &[]), None);
    }

    #[test]
    fn a_notice_has_a_title_and_a_text_after_a_bar() {
        let notice = notice_of(Some(" Backup | 3 files were copied ")).unwrap();
        assert_eq!(
            (notice.title.as_str(), notice.body.as_str()),
            ("Backup", "3 files were copied")
        );
        assert_eq!(notice_of(Some("Done")).unwrap().body, "");
        assert!(notice_of(Some("  ")).is_none() && notice_of(None).is_none());
    }
}
