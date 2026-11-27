// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Banners: a notification that arrives is shown for a few seconds in a
//! corner of a display, unless banners are held back by do not disturb.
//!
//! Windows tells a program like this one about notifications only through
//! the list of its notification center, which the notification store
//! reads every two seconds, so a banner may come that much after the
//! notification. Windows shows a banner of its own as well unless it is
//! told not to in its settings.
//!
//! Do not disturb holds while it is switched on (`quiet = true`, which
//! the moon button and the `focus` widget write), until a time chosen in
//! the list of notifications, while `schedule` says so, and while the user
//! is busy in one of the ways `quiet_when` names. The banners of an
//! application a `[[notification_rule]]` gives `priority = true` come all
//! the same, and so do urgent notices of Tessera itself.
//!
//! Other parts of Tessera post notices of their own with
//! [`Banners::post`]: they are listed with the others, and their banners
//! can carry buttons, which those of Windows cannot, as Windows does not
//! tell what the buttons of a notification are.

use std::rc::Rc;
use std::time::Duration;

use chrono::{
    Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Timelike, Weekday,
};
use gpui::{
    AnyElement, App, AsyncApp, Context, CursorStyle, FontWeight, Global, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement, Styled,
    WeakEntity, Window, WindowHandle, div, img, point, px, size,
};
use serde::Deserialize;

use crate::config::edit::ConfigFile;
use crate::motion::{Interaction, Spring};
use crate::notifications_store::{NotificationStore, unix_now};
use crate::platform::{NativeSurface, Occupation, ScreenRect};
use crate::popup::{MARGIN, open_floating, retire_floating};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line, release_hover, state_layer};

/// How often the time the banners have left is counted down.
const TICK: Duration = Duration::from_millis(200);

/// The width of a banner and the height of the place it takes, which
/// includes the gap below it, in logical pixels; a banner with buttons is
/// taller by a row of them.
const WIDTH: f32 = 360.;
const SLOT: f32 = 108.;
const BUTTON_ROW: f32 = 36.;
const GAP: f32 = 8.;

/// How far sideways a banner starts on its way in.
const TRAVEL: f32 = 48.;

/// How many banners are shown at once unless the configuration says
/// otherwise, and the fewest and the most it may say; older ones leave for
/// newer ones.
const STACK: usize = 4;
const STACK_RANGE: (usize, usize) = (1, 8);

/// The seconds a banner stays unless the configuration says otherwise,
/// and the fewest and the most it may say.
const STAY: u32 = 6;
const STAY_RANGE: (u32, u32) = (2, 60);

/// How many notifications that left the notification center are kept
/// unless the configuration says otherwise, and the most it may say.
const HISTORY: usize = 200;
const HISTORY_MOST: usize = 1000;

/// The size of captions, as a share of the label size.
const CAPTION_SHARE: f32 = 0.85;

/// The side of the logo of an application on a banner.
const LOGO: Pixels = px(16.);

/// The hour of the morning that "until morning" waits for.
const MORNING: u32 = 8;

/// The glyph of the button that puts a banner away.
const CLOSE_GLYPH: &str = "\u{E711}";

/// The glyph shown for an application without a logo.
pub const APP_GLYPH: &str = "\u{E7E7}";

/// The corner of the display the banners hang in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Corner {
    fn right(self) -> bool {
        matches!(self, Self::TopRight | Self::BottomRight)
    }

    fn bottom(self) -> bool {
        matches!(self, Self::BottomRight | Self::BottomLeft)
    }
}

/// A time of the day, written `"22:30"`, in minutes after midnight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct ClockTime(u32);

impl TryFrom<String> for ClockTime {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        let wrong = || format!("`{text}` is no time of the day such as \"22:30\"");
        let (hours, minutes) = text.trim().split_once(':').ok_or_else(wrong)?;
        let hours: u32 = hours.parse().map_err(|_| wrong())?;
        let minutes: u32 = minutes.parse().map_err(|_| wrong())?;
        if hours > 23 || minutes > 59 {
            return Err(wrong());
        }
        Ok(Self(hours * 60 + minutes))
    }
}

impl ClockTime {
    fn time(self) -> NaiveTime {
        NaiveTime::from_hms_opt(self.0 / 60, self.0 % 60, 0).unwrap_or_default()
    }
}

/// A day of the week, written as its English name or the first three
/// letters of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Day(Weekday);

impl TryFrom<String> for Day {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        { text.trim().parse::<Weekday>() }
            .map(Self)
            .map_err(|_| format!("`{text}` is no day of the week such as \"mon\""))
    }
}

/// The hours of each day that do not disturb holds: from `from` to `to`,
/// which may be on the next day, and only on the `days` it starts on, or
/// every day without them. `from` the same as `to` is the 24 hours from
/// `from`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuietSchedule {
    from: ClockTime,
    to: ClockTime,
    #[serde(default)]
    days: Vec<Day>,
}

impl QuietSchedule {
    /// Whether the schedule holds at `now`, and until when.
    fn holds(&self, now: NaiveDateTime) -> Option<NaiveDateTime> {
        let today = now.date();
        let starts_on = |date: NaiveDate| {
            self.days.is_empty() || self.days.iter().any(|day| day.0 == date.weekday())
        };
        let (from, to, minute) = (
            self.from.0,
            self.to.0,
            now.time().num_seconds_from_midnight() / 60,
        );
        let end_on = |date: NaiveDate| date.and_time(self.to.time());
        if from < to {
            return (starts_on(today) && (from..to).contains(&minute)).then(|| end_on(today));
        }
        // Over midnight, and `from` the same as `to` as the 24 hours from
        // it: in the evening of a day it starts on, or in the morning after
        // one.
        if minute >= from && starts_on(today) {
            return Some(end_on(today.succ_opt().unwrap_or(today)));
        }
        let yesterday = today.pred_opt().unwrap_or(today);
        (minute < to && starts_on(yesterday)).then(|| end_on(today))
    }
}

/// What the configuration says about notifications.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationsConfig {
    /// Whether a notification that arrives is shown as a banner.
    #[serde(default)]
    pub banners: bool,
    /// Whether do not disturb holds until it is switched off.
    #[serde(default)]
    pub quiet: bool,
    /// How many seconds a banner stays.
    seconds: Option<u32>,
    /// The corner of the display the banners hang in.
    #[serde(default)]
    corner: Corner,
    /// How many banners are shown at once.
    stack: Option<usize>,
    /// How many notifications that left the notification center are kept.
    history: Option<usize>,
    /// What keeps the user busy so that do not disturb holds by itself;
    /// every kind there is without the key.
    quiet_when: Option<Vec<Occupation>>,
    /// When do not disturb holds each day.
    schedule: Option<QuietSchedule>,
}

impl NotificationsConfig {
    /// How long a banner stays.
    fn stay(&self) -> Duration {
        let (least, most) = STAY_RANGE;
        Duration::from_secs(self.seconds.unwrap_or(STAY).clamp(least, most).into())
    }

    /// How many banners are shown at once.
    fn stack(&self) -> usize {
        let (least, most) = STACK_RANGE;
        self.stack.unwrap_or(STACK).clamp(least, most)
    }

    /// How many notifications that left the notification center are kept.
    pub fn history(&self) -> usize {
        self.history.unwrap_or(HISTORY).min(HISTORY_MOST)
    }

    /// The kinds of being busy that hold banners back.
    fn quiet_when(&self) -> &[Occupation] {
        const EVERY: [Occupation; 4] = [
            Occupation::Fullscreen,
            Occupation::Game,
            Occupation::Presenting,
            Occupation::Duplicating,
        ];
        self.quiet_when.as_deref().unwrap_or(&EVERY)
    }

    /// Whether what keeps the user busy is to be asked, which it is while
    /// banners are shown and any kind of it holds them back, and whether
    /// a display that is duplicated counts.
    pub fn occupations(&self) -> Option<bool> {
        let kinds = self.quiet_when();
        (self.banners && !kinds.is_empty()).then(|| kinds.contains(&Occupation::Duplicating))
    }
}

/// Why do not disturb holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hush {
    /// It was switched on, until it is switched off.
    On,
    /// It was switched on until then, in seconds since 1970.
    Until(i64),
    /// The schedule says so, until then.
    Scheduled(NaiveDateTime),
    /// The user is busy so.
    Busy(Occupation),
}

/// The reasons do not disturb holds by itself at `now`, where `config`
/// asks for them: the schedule, and the user being busy with `occupation`.
fn automatic(
    config: &NotificationsConfig,
    occupation: Option<Occupation>,
    now: NaiveDateTime,
) -> impl Iterator<Item = Hush> {
    let scheduled = { config.schedule.as_ref() }
        .and_then(|schedule| schedule.holds(now))
        .map(Hush::Scheduled);
    let busy = { occupation.filter(|occupation| config.quiet_when().contains(occupation)) }
        .map(Hush::Busy);
    scheduled.into_iter().chain(busy)
}

/// Why do not disturb holds at `now` (the same moment as `now_unix`, in
/// seconds since 1970), if it does: as `config` says, `until` a time it
/// was switched on for, or while the user is busy with `occupation`. A
/// reason that holds by itself and was `released`, as the user switched
/// do not disturb off while it held, does not count.
fn hush_of(
    config: &NotificationsConfig,
    until: Option<i64>,
    occupation: Option<Occupation>,
    released: &[Hush],
    now: NaiveDateTime,
    now_unix: i64,
) -> Option<Hush> {
    if config.quiet {
        return Some(Hush::On);
    }
    if let Some(until) = until.filter(|&until| until > now_unix) {
        return Some(Hush::Until(until));
    }
    automatic(config, occupation, now).find(|hush| !released.contains(hush))
}

/// How long do not disturb is switched on for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusChoice {
    /// It is switched off, and what holds it by itself is let go of.
    Off,
    /// Until it is switched off.
    On,
    /// For this many minutes.
    Minutes(u32),
    /// Until the next morning, at eight.
    Morning,
}

impl FocusChoice {
    /// The choice `arg` of the action `focus` names: `on`, `off`,
    /// `morning` or a number of minutes; `None` for `toggle` or nothing.
    pub fn parse(arg: Option<&str>) -> Result<Option<Self>, String> {
        match arg.map(str::trim) {
            None | Some("" | "toggle") => Ok(None),
            Some("on") => Ok(Some(Self::On)),
            Some("off") => Ok(Some(Self::Off)),
            Some("morning") => Ok(Some(Self::Morning)),
            Some(minutes) => match minutes.parse::<u32>() {
                Ok(minutes) if minutes > 0 => Ok(Some(Self::Minutes(minutes))),
                _ => Err(
                    "`focus` takes `toggle`, `on`, `off`, `morning`, a number of minutes or no `arg`"
                        .to_owned(),
                ),
            },
        }
    }
}

/// The first moment at `MORNING` o'clock after `now`.
fn next_morning(now: NaiveDateTime) -> NaiveDateTime {
    let morning = NaiveTime::from_hms_opt(MORNING, 0, 0).unwrap_or_default();
    let today = now.date().and_time(morning);
    if today > now {
        today
    } else {
        today + TimeDelta::days(1)
    }
}

/// Seconds since 1970 at the local time `time`.
fn unix_at(time: NaiveDateTime) -> i64 {
    Local
        .from_local_datetime(&time)
        .earliest()
        .map_or_else(|| time.and_utc().timestamp(), |time| time.timestamp())
}

/// A button of a notice and what it does.
#[derive(Clone)]
pub struct NoticeAction {
    label: SharedString,
    run: Rc<dyn Fn(&mut App)>,
}

impl NoticeAction {
    #[allow(
        dead_code,
        reason = "for the parts of Tessera that post notices with buttons"
    )]
    pub fn new(label: impl Into<SharedString>, run: impl Fn(&mut App) + 'static) -> Self {
        Self {
            label: label.into(),
            run: Rc::new(run),
        }
    }
}

/// A notice of Tessera itself, to post with [`Banners::post`].
#[derive(Clone, Default)]
pub struct Notice {
    /// Who tells: the part of Tessera, such as "Devices".
    pub app: String,
    pub title: String,
    pub body: String,
    /// A character of the icon font shown in place of a logo.
    pub glyph: Option<&'static str>,
    /// Its buttons; a click on the banner does what the first one does.
    /// They are on the banner only, not in the list of notifications.
    pub actions: Vec<NoticeAction>,
    /// Whether it comes as a banner even while banners are switched off
    /// or held back, as a warning of an empty battery should.
    pub urgent: bool,
}

/// What a banner shows, and what a click on it does.
#[derive(Clone)]
struct BannerItem {
    /// The notification in the store.
    key: u64,
    /// Whether it is a notification of Windows, which a click opens the
    /// application of.
    windows: bool,
    app: String,
    app_id: String,
    title: String,
    body: String,
    glyph: &'static str,
    actions: Vec<NoticeAction>,
}

/// The banners and do not disturb.
#[derive(Default)]
pub struct Banners {
    config: NotificationsConfig,
    /// What held do not disturb by itself when the user switched it off.
    released: Vec<Hush>,
    open: Option<WindowHandle<BannerView>>,
}

impl Global for Banners {}

impl Banners {
    /// Takes over what the configuration says.
    pub fn configure(config: &NotificationsConfig, cx: &mut App) {
        cx.default_global::<Self>().config = config.clone();
    }

    /// What the configuration says.
    pub fn config(cx: &App) -> Option<&NotificationsConfig> {
        cx.try_global::<Self>().map(|this| &this.config)
    }

    /// Why do not disturb holds now, if it does.
    pub fn hush(cx: &App) -> Option<Hush> {
        let this = cx.try_global::<Self>()?;
        let store = cx.try_global::<NotificationStore>();
        let now = Local::now();
        hush_of(
            &this.config,
            store.and_then(NotificationStore::quiet_until),
            store.and_then(NotificationStore::occupation),
            &this.released,
            now.naive_local(),
            now.timestamp(),
        )
    }

    /// Switches do not disturb as `choice` says, or the other way than it
    /// is now without one.
    pub fn focus(choice: Option<FocusChoice>, cx: &mut App) {
        let hush = Self::hush(cx);
        let choice = choice.unwrap_or(if hush.is_some() {
            FocusChoice::Off
        } else {
            FocusChoice::On
        });
        let now = Local::now().naive_local();
        let until = match choice {
            FocusChoice::Off | FocusChoice::On => None,
            FocusChoice::Minutes(minutes) => Some(unix_now() + i64::from(minutes) * 60),
            FocusChoice::Morning => Some(unix_at(next_morning(now))),
        };
        NotificationStore::set_quiet_until(until, cx);
        let quiet = cx.default_global::<Self>().config.quiet;
        if choice == FocusChoice::Off {
            // What holds it by itself now is let go of until it ends.
            let occupation =
                { cx.try_global::<NotificationStore>() }.and_then(NotificationStore::occupation);
            let released = automatic(&cx.global::<Self>().config, occupation, now).collect();
            cx.global_mut::<Self>().released = released;
        }
        let wanted = choice == FocusChoice::On;
        if quiet != wanted {
            Self::write_quiet(wanted, cx);
        }
    }

    /// Writes into the configuration whether do not disturb holds until it
    /// is switched off; the configuration is read again at once.
    fn write_quiet(quiet: bool, cx: &mut App) {
        let written = ConfigFile::open().and_then(|mut file| {
            let value = quiet.then_some(toml_edit::Value::from(true));
            if !file.set_in("notifications", "quiet", value) {
                return Err("`notifications` in the configuration is no table".to_owned());
            }
            file.save()
        });
        match written {
            // Until then it holds as asked.
            Ok(()) => {
                cx.default_global::<Self>().config.quiet = quiet;
                cx.defer(Shell::config_changed);
            }
            Err(error) => report!("{error}"),
        }
    }

    /// Forgets that do not disturb was switched off while the user was
    /// busy, once the user is busy another way or no longer.
    pub fn occupation_changed(occupation: Option<Occupation>, cx: &mut App) {
        let stale = |hush: &Hush| matches!(hush, Hush::Busy(busy) if occupation != Some(*busy));
        let released = cx.try_global::<Self>().map(|this| &this.released);
        if released.is_some_and(|released| released.iter().any(stale)) {
            cx.global_mut::<Self>().released.retain(|hush| !stale(hush));
        }
    }

    /// Shows as banners the notifications of the store `arrived` names,
    /// oldest first, where they are wanted: not while banners are off, and
    /// not while do not disturb holds unless their application has
    /// priority, nor for an application whose banners are switched off.
    pub fn arrive(arrived: &[u64], cx: &mut App) {
        let Some(this) = cx.try_global::<Self>().filter(|this| this.config.banners) else {
            return;
        };
        let stay = this.config.stay();
        let hush = Self::hush(cx);
        let store = cx.global::<NotificationStore>();
        let items: Vec<BannerItem> = { arrived.iter() }
            .filter_map(|key| store.entry(*key))
            .filter(|entry| {
                let rule = store.rule(entry);
                let muted = rule.is_some_and(|rule| !rule.banners);
                let priority = rule.is_some_and(|rule| rule.priority);
                !muted && (hush.is_none() || priority)
            })
            .map(|entry| BannerItem {
                key: entry.key,
                windows: true,
                app: entry.app.clone(),
                app_id: entry.app_id.clone(),
                title: entry.title.clone(),
                body: entry.body.clone(),
                glyph: APP_GLYPH,
                actions: Vec::new(),
            })
            .collect();
        Self::show(items, stay, cx);
    }

    /// Posts `notice`: it is listed with the notifications, and comes as a
    /// banner where banners are shown and not held back, or where it is
    /// urgent.
    pub fn post(notice: Notice, cx: &mut App) {
        let Notice {
            app,
            title,
            body,
            glyph,
            actions,
            urgent,
        } = notice;
        let key = NotificationStore::add_own(app.clone(), title.clone(), body.clone(), cx);
        let banners = cx.default_global::<Self>().config.banners;
        if !urgent && !(banners && Self::hush(cx).is_none()) {
            return;
        }
        let stay = cx.global::<Self>().config.stay();
        let item = BannerItem {
            key,
            windows: false,
            app,
            app_id: String::new(),
            title,
            body,
            glyph: glyph.unwrap_or(APP_GLYPH),
            actions,
        };
        Self::show(vec![item], stay, cx);
    }

    /// Shows `items` above the banners there are, opening the window of
    /// the banners if it is not open.
    fn show(items: Vec<BannerItem>, stay: Duration, cx: &mut App) {
        if items.is_empty() {
            return;
        }
        let stack = cx.global::<Self>().config.stack();
        if let Some(open) = cx.global::<Self>().open {
            let shown = open.update(cx, |view, window, cx| {
                view.show(items.clone(), stay, stack, window, cx)
            });
            if matches!(shown, Ok(true)) {
                return;
            }
        }
        Self::open(items, stay, stack, cx);
    }

    /// Opens the window of the banners with `items` in it, in the corner
    /// the configuration names of what the bars leave of the display the
    /// pointer is on.
    fn open(items: Vec<BannerItem>, stay: Duration, stack: usize, cx: &mut App) {
        let Some(anchor) = Shell::middle_anchor(cx) else {
            report!("the banners have nowhere to open");
            return;
        };
        let area = anchor.display.and_then(|display| display.work_area());
        let hanging = Hanging {
            corner: cx.global::<Self>().config.corner,
            area: area.unwrap_or(anchor.screen),
            scale: anchor.scale,
        };
        let height = slots(items.iter().take(stack));
        let frame = hanging.frame(height);
        let rough = gpui::Bounds {
            origin: point(
                px(frame.left as f32 / anchor.scale),
                px(frame.top as f32 / anchor.scale),
            ),
            size: size(px(WIDTH), px(height)),
        };
        let opened = open_floating(rough, anchor.display_id, false, cx, |window, cx| {
            let adopted = NativeSurface::of(window).and_then(|native| {
                native.adopt_as_popup()?;
                native.place(frame)?;
                native.reveal(false);
                Ok(())
            });
            if let Err(error) = adopted {
                report!("the banners are not where they belong: {error:#}");
            }
            BannerView::new(items, stay, stack, hanging, cx)
        });
        match opened {
            Ok(window) => cx.global_mut::<Self>().open = Some(window),
            Err(error) => report!("the banners did not open: {error:#}"),
        }
    }
}

/// The height of the places of `items`, in logical pixels.
fn slots<'a>(items: impl Iterator<Item = &'a BannerItem>) -> f32 {
    items.map(BannerItem::slot).sum::<f32>().max(SLOT)
}

impl BannerItem {
    /// The height of its place, the gap below it included.
    fn slot(&self) -> f32 {
        if self.actions.is_empty() {
            SLOT
        } else {
            SLOT + BUTTON_ROW
        }
    }
}

/// Where on a display the banners hang: the corner, the area of the
/// display they keep to, in physical pixels, and the physical pixels per
/// logical one there.
#[derive(Clone, Copy)]
struct Hanging {
    corner: Corner,
    area: ScreenRect,
    scale: f32,
}

impl Hanging {
    /// The frame of the window around banners `height` logical pixels
    /// high in all: it touches the corner and grows away from it.
    fn frame(self, height: f32) -> ScreenRect {
        let physical = |length: f32| (length * self.scale).round() as i32;
        let width = physical(WIDTH + MARGIN * 2.);
        let height = physical(height + MARGIN * 2.);
        let area = self.area;
        let (left, right) = if self.corner.right() {
            (area.right - width, area.right)
        } else {
            (area.left, area.left + width)
        };
        let (top, bottom) = if self.corner.bottom() {
            (area.bottom - height, area.bottom)
        } else {
            (area.top, area.top + height)
        };
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// One banner and the motion it is in.
struct Banner {
    item: BannerItem,
    /// 0 while absent, 1 when fully shown.
    presence: Spring,
    /// How long it still stays.
    left: Duration,
    leaving: bool,
    close: Interaction,
    /// The pointer feedback of its buttons, in their order.
    buttons: Vec<Interaction>,
}

/// The root view of the window of the banners.
pub struct BannerView {
    /// Newest first, including banners that are still leaving.
    banners: Vec<Banner>,
    hanging: Hanging,
    /// How many banners stay at once.
    stack: usize,
    /// For how many logical pixels of banners the window is sized.
    room: f32,
    /// While the pointer is on the banners, none of them leaves.
    hovered: bool,
    /// The window was put away and shows no more banners.
    gone: bool,
}

impl BannerView {
    fn new(
        items: Vec<BannerItem>,
        stay: Duration,
        stack: usize,
        hanging: Hanging,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                if !matches!(this.update(cx, |this, cx| this.tick(cx)), Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        let room = slots(items.iter().take(stack));
        let mut this = Self {
            banners: Vec::new(),
            hanging,
            stack,
            room,
            hovered: false,
            gone: false,
        };
        this.add(items, stay, cx);
        this
    }

    /// Shows `items` above the banners there are. `false` when the window
    /// was put away and another one is needed.
    fn show(
        &mut self,
        items: Vec<BannerItem>,
        stay: Duration,
        stack: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.gone {
            return false;
        }
        self.stack = stack;
        self.add(items, stay, cx);
        self.fit(window);
        true
    }

    fn add(&mut self, items: Vec<BannerItem>, stay: Duration, cx: &mut Context<Self>) {
        let motion = cx.global::<Theme>().motion;
        for item in items {
            let mut presence = Spring::new(motion.spatial, 0.);
            presence.animate_to(1.);
            self.banners.insert(
                0,
                Banner {
                    buttons: item
                        .actions
                        .iter()
                        .map(|_| Interaction::new(&motion))
                        .collect(),
                    item,
                    presence,
                    left: stay,
                    leaving: false,
                    close: Interaction::new(&motion),
                },
            );
        }
        let staying = self.banners.iter_mut().filter(|banner| !banner.leaving);
        for banner in staying.skip(self.stack) {
            banner.leave();
        }
        cx.notify();
    }

    /// Sizes the window for the banners there are.
    fn fit(&mut self, window: &mut Window) {
        if self.banners.is_empty() {
            return;
        }
        let room = slots(self.banners.iter().map(|banner| &banner.item));
        if room == self.room {
            return;
        }
        self.room = room;
        let placed =
            NativeSurface::of(window).and_then(|native| native.place(self.hanging.frame(room)));
        if let Err(error) = placed {
            report!("the banners keep their room: {error:#}");
        }
    }

    /// Counts down the time the banners have left. `false` once the
    /// window was put away.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        if self.gone {
            return false;
        }
        if self.hovered {
            return true;
        }
        for banner in self.banners.iter_mut().filter(|banner| !banner.leaving) {
            banner.left = banner.left.saturating_sub(TICK);
            if banner.left.is_zero() {
                banner.leave();
                cx.notify();
            }
        }
        true
    }

    fn banner(&mut self, key: u64) -> Option<&mut Banner> {
        self.banners
            .iter_mut()
            .find(|banner| banner.item.key == key)
    }

    /// Has the banner of the notification `key` leave.
    fn hide(&mut self, key: u64, cx: &mut Context<Self>) {
        if let Some(banner) = self.banner(key) {
            banner.leave();
            cx.notify();
        }
    }

    /// Does what a click on the banner `key` does: opens the application
    /// of a notification of Windows, which then leaves the notification
    /// center, or does what the first button of a notice does.
    fn activate(&mut self, key: u64, cx: &mut Context<Self>) {
        let Some(banner) = self.banner(key) else {
            return;
        };
        let item = banner.item.clone();
        self.hide(key, cx);
        if item.windows {
            cx.defer(move |cx| NotificationStore::open(key, cx));
        } else if let Some(action) = item.actions.first() {
            let run = action.run.clone();
            cx.defer(move |cx| run(cx));
        }
    }

    /// Does what the button `index` of the banner `key` does.
    fn press(&mut self, key: u64, index: usize, cx: &mut Context<Self>) {
        let Some(banner) = self.banner(key) else {
            return;
        };
        let Some(run) = banner
            .item
            .actions
            .get(index)
            .map(|action| action.run.clone())
        else {
            return;
        };
        self.hide(key, cx);
        cx.defer(move |cx| run(cx));
    }

    fn render_banner(
        banner: &mut Banner,
        travel: f32,
        theme: Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let presence = banner.presence.value(window);
        let item = &banner.item;
        let key = item.key;
        let caption_size = theme.typography.label_size * CAPTION_SHARE;
        let logo = cx
            .try_global::<NotificationStore>()
            .and_then(|store| store.logo(&item.app_id));
        let mark = match logo {
            Some(logo) => img(logo).size(LOGO).flex_none().into_any_element(),
            None => icon(&theme, item.glyph)
                .flex_none()
                .text_size(caption_size)
                .text_color(theme.colors.primary)
                .into_any_element(),
        };
        let state = banner.close.state(window);
        let close = icon(&theme, CLOSE_GLYPH)
            .id(("banner-close", key as usize))
            .size(theme.typography.icon_size + theme.spacing.tiny)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme.shape.full)
            .text_size(caption_size)
            .text_color(theme.colors.on_surface_variant)
            .bg(state_layer(
                &theme,
                theme.colors.surface_container_highest,
                state,
            ))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.hide(key, cx);
            }));
        let close = Interaction::track(close, cx, move |this: &mut Self| {
            Some(&mut this.banner(key)?.close)
        });
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(caption_size)
                    .text_color(theme.colors.primary)
                    .child(one_line(item.app.clone())),
            )
            .child(close);
        let title = (!item.title.is_empty()).then(|| {
            div()
                .flex_none()
                .min_w_0()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.on_surface)
                .child(one_line(item.title.clone()))
        });
        let body = (!item.body.is_empty()).then(|| {
            div()
                .line_clamp(if item.title.is_empty() { 3 } else { 2 })
                .text_size(caption_size)
                .text_color(theme.colors.on_surface_variant)
                .child(SharedString::from(item.body.clone()))
        });
        let buttons: Vec<AnyElement> = { item.actions.iter().zip(&mut banner.buttons) }
            .enumerate()
            .map(|(index, (action, interaction))| {
                let state = interaction.state(window);
                let button = div()
                    .id(SharedString::from(format!("banner-{key}-button-{index}")))
                    .flex_1()
                    .h(px(BUTTON_ROW - GAP))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme.shape.full)
                    .text_color(theme.colors.primary)
                    .bg(state_layer(&theme, theme.colors.surface_container, state))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.press(key, index, cx);
                    }))
                    .child(one_line(action.label.clone()));
                Interaction::track(button, cx, move |this: &mut Self| {
                    this.banner(key)?.buttons.get_mut(index)
                })
                .into_any_element()
            })
            .collect();
        let button_row = (!buttons.is_empty()).then(|| {
            div()
                .flex_none()
                .mt(theme.spacing.small)
                .flex()
                .gap(theme.spacing.small)
                .children(buttons)
        });
        let slot = item.slot();
        let card = div()
            .id(("banner", key as usize))
            .relative()
            .left(px(travel * TRAVEL * (1. - presence)))
            .w_full()
            .h(px(slot - GAP))
            .px(theme.spacing.medium)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .gap(px(2.))
            .justify_center()
            .overflow_hidden()
            .rounded(theme.shape.large)
            .bg(theme.colors.surface_container_highest)
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _, _, cx| this.activate(key, cx)))
            .child(header)
            .children(title)
            .children(body)
            .children(button_row);
        // The place includes the gap below the card, so that the cards
        // beside it move smoothly when one comes or goes.
        div()
            .w_full()
            .h(px(slot * presence.max(0.)))
            .flex_none()
            .overflow_hidden()
            .opacity(presence.clamp(0., 1.))
            .child(card)
            .into_any_element()
    }
}

impl Banner {
    fn leave(&mut self) {
        self.leaving = true;
        self.presence.animate_to(0.);
    }
}

impl Render for BannerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_hover(window);
        let theme = *cx.global::<Theme>();
        let corner = self.hanging.corner;
        let travel = if corner.right() { 1. } else { -1. };
        let mut banners = std::mem::take(&mut self.banners);
        let mut slots: Vec<AnyElement> = { banners.iter_mut() }
            .map(|banner| Self::render_banner(banner, travel, theme, window, cx))
            .collect();
        // A banner that has finished leaving was just drawn at no height.
        let before = banners.len();
        banners.retain(|banner| !(banner.leaving && banner.presence.is_settled()));
        // The newest banner is the one nearest the corner.
        if corner.bottom() {
            slots.reverse();
        }
        let after = banners.len();
        self.banners = banners;
        if self.banners.is_empty() {
            if !self.gone {
                self.gone = true;
                retire_floating(window, cx);
            }
        } else if after != before {
            cx.on_next_frame(window, |this, window, _| this.fit(window));
        }
        let column = div()
            .id("banners")
            .size_full()
            .p(px(MARGIN))
            .flex()
            .flex_col()
            .font_family(theme.typography.family)
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface)
            .on_hover(cx.listener(|this, hovered: &bool, _, _| this.hovered = *hovered))
            .children(slots);
        if corner.bottom() {
            column.justify_end()
        } else {
            column
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> NaiveDateTime {
        // 2 March 2026 is a Monday.
        NaiveDate::from_ymd_opt(2026, 3, day)
            .and_then(|date| date.and_hms_opt(hour, minute, 0))
            .unwrap()
    }

    fn schedule(text: &str) -> QuietSchedule {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_banner_stays_as_long_as_the_configuration_says_within_limits() {
        let stay = |seconds| {
            NotificationsConfig {
                seconds,
                ..Default::default()
            }
            .stay()
        };
        assert_eq!(stay(None), Duration::from_secs(6));
        assert_eq!(stay(Some(10)), Duration::from_secs(10));
        assert_eq!(stay(Some(0)), Duration::from_secs(2));
        assert_eq!(stay(Some(600)), Duration::from_secs(60));
        let stack = |stack| {
            NotificationsConfig {
                stack,
                ..Default::default()
            }
            .stack()
        };
        assert_eq!((stack(None), stack(Some(0)), stack(Some(20))), (4, 1, 8));
    }

    #[test]
    fn the_window_hangs_in_its_corner_and_grows_away_from_it() {
        let area = ScreenRect {
            left: 0,
            top: 40,
            right: 3000,
            bottom: 2000,
        };
        let hanging = |corner| Hanging {
            corner,
            area,
            scale: 1.,
        };
        let one = hanging(Corner::TopRight).frame(SLOT);
        assert_eq!(
            (one.left, one.top, one.right, one.bottom),
            (2616, 40, 3000, 172)
        );
        let three = hanging(Corner::TopRight).frame(SLOT * 3.);
        assert_eq!((three.top, three.bottom), (40, 388));
        let low = hanging(Corner::BottomLeft).frame(SLOT * 3.);
        assert_eq!(
            (low.left, low.top, low.right, low.bottom),
            (0, 1652, 384, 2000)
        );
    }

    #[test]
    fn the_configuration_names_corners_times_and_days() {
        let config: NotificationsConfig = toml::from_str(
            "corner = \"bottom-left\"\nquiet_when = [\"game\"]\n[schedule]\nfrom = \"22:00\"\nto = \"7:30\"\ndays = [\"fri\", \"saturday\"]",
        )
        .unwrap();
        assert_eq!(config.corner, Corner::BottomLeft);
        assert_eq!(config.quiet_when(), [Occupation::Game]);
        let schedule = config.schedule.unwrap();
        assert_eq!((schedule.from.0, schedule.to.0), (22 * 60, 7 * 60 + 30));
        assert!(toml::from_str::<QuietSchedule>("from = \"24:00\"\nto = \"1:00\"").is_err());
        assert!(toml::from_str::<QuietSchedule>("from = \"9\"\nto = \"1:00\"").is_err());
        assert!(
            toml::from_str::<QuietSchedule>("from = \"9:00\"\nto = \"1:00\"\ndays = [\"someday\"]")
                .is_err()
        );
    }

    #[test]
    fn a_schedule_over_midnight_holds_from_the_evening_of_its_days() {
        let night = schedule("from = \"22:00\"\nto = \"07:00\"\ndays = [\"fri\"]");
        // Friday 6 March, 23:00, until Saturday 7:00.
        assert_eq!(night.holds(at(6, 23, 0)), Some(at(7, 7, 0)));
        assert_eq!(night.holds(at(7, 6, 59)), Some(at(7, 7, 0)));
        assert_eq!(night.holds(at(7, 7, 0)), None);
        assert_eq!(night.holds(at(6, 21, 59)), None);
        // Thursday evening and Friday morning are not of it.
        assert_eq!(night.holds(at(5, 23, 0)), None);
        assert_eq!(night.holds(at(6, 6, 0)), None);
    }

    #[test]
    fn a_schedule_within_a_day_holds_between_its_times() {
        let work = schedule("from = \"09:00\"\nto = \"12:00\"");
        assert_eq!(work.holds(at(2, 9, 0)), Some(at(2, 12, 0)));
        assert_eq!(work.holds(at(2, 12, 0)), None);
        assert_eq!(work.holds(at(2, 8, 59)), None);
        let weekend = schedule("from = \"0:00\"\nto = \"0:00\"\ndays = [\"sat\", \"sun\"]");
        assert_eq!(weekend.holds(at(7, 15, 0)), Some(at(8, 0, 0)));
        assert_eq!(weekend.holds(at(6, 15, 0)), None);
    }

    #[test]
    fn a_schedule_from_a_time_to_the_same_time_holds_for_24_hours() {
        let day = schedule("from = \"08:00\"\nto = \"08:00\"\ndays = [\"mon\"]");
        // Monday 2 March, 8:00, until Tuesday 8:00.
        assert_eq!(day.holds(at(2, 7, 59)), None);
        assert_eq!(day.holds(at(2, 8, 0)), Some(at(3, 8, 0)));
        assert_eq!(day.holds(at(2, 23, 0)), Some(at(3, 8, 0)));
        assert_eq!(day.holds(at(3, 0, 0)), Some(at(3, 8, 0)));
        assert_eq!(day.holds(at(3, 7, 59)), Some(at(3, 8, 0)));
        assert_eq!(day.holds(at(3, 8, 0)), None);
    }

    #[test]
    fn do_not_disturb_holds_for_the_reason_that_comes_first() {
        let now = at(2, 10, 0);
        let unix = 1_000;
        let config = |quiet: bool| NotificationsConfig {
            quiet,
            schedule: Some(schedule("from = \"09:00\"\nto = \"12:00\"")),
            ..Default::default()
        };
        let busy = Some(Occupation::Game);
        assert_eq!(
            hush_of(&config(true), None, busy, &[], now, unix),
            Some(Hush::On)
        );
        assert_eq!(
            hush_of(&config(false), Some(2_000), busy, &[], now, unix),
            Some(Hush::Until(2_000))
        );
        // A time that has passed holds no more.
        let scheduled = Hush::Scheduled(at(2, 12, 0));
        let game = Hush::Busy(Occupation::Game);
        assert_eq!(
            hush_of(&config(false), Some(500), busy, &[], now, unix),
            Some(scheduled)
        );
        // Released, the schedule gives way to what keeps the user busy,
        // and then to nothing.
        assert_eq!(
            hush_of(&config(false), None, busy, &[scheduled], now, unix),
            Some(game)
        );
        assert_eq!(
            hush_of(&config(false), None, busy, &[scheduled, game], now, unix),
            None
        );
        // Switching off releases both.
        let reasons: Vec<Hush> = automatic(&config(false), busy, now).collect();
        assert_eq!(reasons, [scheduled, game]);
        // Being busy in a way `quiet_when` leaves out does not count.
        let games_only = NotificationsConfig {
            quiet_when: Some(vec![Occupation::Game]),
            ..Default::default()
        };
        let fullscreen = Some(Occupation::Fullscreen);
        assert_eq!(hush_of(&games_only, None, fullscreen, &[], now, unix), None);
    }

    #[test]
    fn being_busy_is_asked_only_while_it_holds_banners_back() {
        let config = |banners, quiet_when: Option<Vec<Occupation>>| NotificationsConfig {
            banners,
            quiet_when,
            ..Default::default()
        };
        assert_eq!(config(false, None).occupations(), None);
        assert_eq!(config(true, None).occupations(), Some(true));
        assert_eq!(
            config(true, Some(vec![Occupation::Game])).occupations(),
            Some(false)
        );
        assert_eq!(config(true, Some(Vec::new())).occupations(), None);
    }

    #[test]
    fn focus_takes_a_choice_or_a_number_of_minutes() {
        assert_eq!(FocusChoice::parse(None), Ok(None));
        assert_eq!(FocusChoice::parse(Some("toggle")), Ok(None));
        assert_eq!(FocusChoice::parse(Some("on")), Ok(Some(FocusChoice::On)));
        assert_eq!(
            FocusChoice::parse(Some("45")),
            Ok(Some(FocusChoice::Minutes(45)))
        );
        assert!(FocusChoice::parse(Some("0")).is_err());
        assert!(FocusChoice::parse(Some("soon")).is_err());
    }

    #[test]
    fn the_morning_is_the_next_one_at_eight() {
        assert_eq!(next_morning(at(2, 23, 0)), at(3, 8, 0));
        assert_eq!(next_morning(at(3, 2, 0)), at(3, 8, 0));
        assert_eq!(next_morning(at(3, 8, 0)), at(4, 8, 0));
    }
}
