// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The notifications Tessera keeps: those of the Windows notification
//! center and the history of those that left it. Widgets: `notifications`,
//! the list, grouped by application, for a popup; `notifications-button`,
//! a bell in the bar with the count of notifications that came since the
//! list was last open, which opens the popup `notifications`; and `focus`,
//! a switch for do not disturb. Actions: `quiet`, which switches do not
//! disturb, and `notify`, which posts a notice of Tessera's own.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, div, img, px,
};

use crate::actions::{Action, Actions};
use crate::banners::{APP_GLYPH, Banners, FocusChoice, Hush, Notice};
use crate::config::{PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, mix};
use crate::notifications_store::{Entry, NotificationStore, groups, unix_now};
use crate::platform::Occupation;
use crate::theme::Theme;
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
}

/// The buttons at the head of the group of an application.
struct GroupButtons {
    clear: Interaction,
    unroll: Interaction,
}

/// Lists the notifications by application, the application with the
/// newest one first.
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
        let entries = store.entries();
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
                if item.entry != *entry {
                    item.entry = entry.clone();
                }
                continue;
            }
            let at = { self.items.iter() }
                .position(|item| item.entry.posted < entry.posted)
                .unwrap_or(self.items.len());
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.items.insert(
                at,
                Item {
                    entry: entry.clone(),
                    presence,
                    dismiss: Interaction::new(&motion),
                    unroll: Interaction::new(&motion),
                    leaving: false,
                },
            );
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

    /// Opens the application of the notification `key`, which then leaves.
    fn open(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(item) = self.item(key) {
            item.leaving = true;
            item.presence.animate_to(0.);
        }
        cx.defer(move |cx| NotificationStore::open(key, cx));
        cx.notify();
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

    fn render_item(
        item: &mut Item,
        now: i64,
        unrolled: bool,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = item.presence.value(window);
        let dismiss_state = item.dismiss.state(window);
        let unroll_state = item.unroll.state(window);
        let entry = &item.entry;
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
        let openable = entry.id.is_some() && !entry.app_id.is_empty();
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
            .child(header)
            .children(body);
        let card = if openable {
            card.cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.open(key, cx)))
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
            slot.h(ITEM_HEIGHT * presence.max(0.))
                .child(card.h(ITEM_HEIGHT - theme.spacing.small))
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
        let now = unix_now();
        let store = cx.try_global::<NotificationStore>();
        let error = store.and_then(NotificationStore::error).map(str::to_owned);

        let mut cards: Vec<AnyElement> = Vec::new();
        let grouped = groups(&self.items, |item| &item.entry);
        for group in &grouped {
            let first = &self.items[group.members[0]].entry;
            let (app, app_id) = (first.app.clone(), first.app_id.clone());
            let head = self.render_group_head(
                &group.key,
                &app,
                &app_id,
                &group.members,
                theme,
                window,
                cx,
            );
            cards.push(head);
            let shown = if self.unrolled_groups.contains(&group.key) {
                group.members.len()
            } else {
                COLLAPSED
            };
            for &at in group.members.iter().take(shown) {
                let unrolled = self.unrolled.contains(&self.items[at].entry.key);
                let item = &mut self.items[at];
                cards.push(Self::render_item(item, now, unrolled, theme, window, cx));
            }
        }
        // A notification that has finished leaving was just drawn at no
        // height.
        self.items
            .retain(|item| !(item.leaving && item.presence.is_settled()));
        let waiting = self.items.iter().filter(|item| !item.leaving).count();
        let kept: HashSet<String> = grouped.into_iter().map(|group| group.key).collect();
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
        let quiet_note = hush.filter(|_| banners).map(|hush| {
            div()
                .flex_none()
                .pb(theme.spacing.small)
                .px(theme.spacing.tiny)
                .text_size(theme.typography.label_size * CAPTION_SHARE)
                .text_color(theme.colors.on_surface_variant)
                .child(one_line(format!(
                    "{} · {}",
                    language.say("Do not disturb"),
                    describe(hush, language)
                )))
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
        let list = match (error, cards.is_empty()) {
            (Some(error), true) => note(SharedString::from(error)),
            (_, true) => note(language.say("No notifications").into()),
            (_, false) => div()
                .id("notification-list")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(div().w_full().flex().flex_col().children(cards))
                .into_any_element(),
        };
        div()
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
