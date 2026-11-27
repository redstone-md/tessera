// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Night light, for the quick settings. Widgets: `night-light`, a tile that
//! switches it, and `night-light-slider`, which the tile unfolds: how warm
//! the light is, and when it comes on by itself. The action `night-light`
//! does what the tile does from a `[[key]]` or `tessera do`, with `on`,
//! `off` or `toggle` (the default).
//!
//! Which night light they switch is chosen by `[night_light] source`: the
//! one of Windows, kept in the CloudStore of the account, or Tessera's
//! own, which tints the gamma ramps of the displays. With `auto`, the
//! default, it is the one of Windows unless Tessera is the shell, where
//! Windows may not apply what is written without Explorer, or Windows
//! keeps no state of it that Tessera can read. Windows keeps the schedule
//! of its own night light in its settings; Tessera's own follows
//! `[night_light] schedule`, from sunset to sunrise where the place is
//! given or between two times of day, and a click holds until the schedule
//! next changes its mind, as Windows does.

use std::f64::consts::PI;
use std::time::Duration;

use chrono::{DateTime, Days, Local, NaiveDate, NaiveTime, TimeZone, Utc};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, IntoElement, ParentElement,
    Render, Styled, Subscription, Window, div,
};
use serde::Deserialize;

use super::quick::{
    Face, Reading, SettingsPage, Source, TileView, on_or_off, register_tile, sentence,
    unusable_note,
};
use crate::actions::{Action, Actions};
use crate::config::config_dir;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::Interaction;
use crate::platform::{COLDEST, GammaNight, WARMEST, WindowsNight, WindowsNightLight, shell_mode};
use crate::theme::Theme;
use crate::ui::{Slider, TileFace, icon};

const ICON_NIGHT: &str = "\u{e708}";

/// How often the schedule is looked at, and the tint of Tessera's own
/// night light given again to displays that lost it when their mode
/// changed.
const TICK: Duration = Duration::from_secs(30);

/// The file of the configuration directory that keeps the colors the
/// displays had while Tessera's own night light tints them.
const KEPT_RAMPS: &str = "night-light-ramps";

const SETTINGS: SettingsPage = SettingsPage {
    address: "ms-settings:nightlight",
    text: "Night light settings",
};

/// Which night light the widgets switch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NightSource {
    /// That of Windows, unless Tessera is the shell or Windows keeps no
    /// state of it that can be read.
    #[default]
    Auto,
    Windows,
    Tessera,
}

/// When Tessera's own night light comes on by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NightSchedule {
    /// Never: only a click or the action switches it.
    #[default]
    Off,
    /// From sunset to sunrise at `latitude` and `longitude`.
    Sun,
    /// From `from` to `to`.
    Hours,
}

/// A time of day, written as "21:30".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct Clock(NaiveTime);

impl TryFrom<String> for Clock {
    type Error = String;

    fn try_from(text: String) -> Result<Self, String> {
        NaiveTime::parse_from_str(&text, "%H:%M")
            .map(Self)
            .map_err(|_| format!("`{text}` is no time of day such as \"21:30\""))
    }
}

/// What `[night_light]` says.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct NightLightConfig {
    pub source: NightSource,
    /// How warm Tessera's own night light is, in kelvins, from 1200 (the
    /// warmest) to 6500 (no tint).
    pub kelvins: u32,
    pub schedule: NightSchedule,
    pub from: Clock,
    pub to: Clock,
    /// The place whose sunset and sunrise `schedule = "sun"` follows, in
    /// degrees, north and east positive.
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Default for NightLightConfig {
    fn default() -> Self {
        let at = |hour| Clock(NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or_default());
        Self {
            source: NightSource::Auto,
            kelvins: 3400,
            schedule: NightSchedule::Off,
            from: at(21),
            to: at(7),
            latitude: None,
            longitude: None,
        }
    }
}

/// Where the sun is on a day at a place.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sun {
    Rises {
        rise: DateTime<Utc>,
        set: DateTime<Utc>,
    },
    /// It does not set, as in a summer near a pole.
    Up,
    /// It does not rise.
    Down,
}

/// When the sun rises and sets on `date` at `latitude` and `longitude`
/// (degrees, north and east positive), by the sunrise equation with the
/// refraction of the air and the size of the sun counted in; it is within
/// a minute or two of what almanacs give, except near the poles.
fn sun(date: NaiveDate, latitude: f64, longitude: f64) -> Sun {
    let radians = |degrees: f64| degrees * PI / 180.;
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap_or_default();
    let day = (date - epoch).num_days() as f64;
    // The mean solar noon, in days after noon on 1 January 2000.
    let noon = day - longitude / 360.;
    let anomaly = (357.5291 + 0.985_600_28 * noon).rem_euclid(360.);
    let m = radians(anomaly);
    let center = 1.9148 * m.sin() + 0.02 * (2. * m).sin() + 0.0003 * (3. * m).sin();
    let ecliptic = radians((anomaly + center + 180. + 102.9372).rem_euclid(360.));
    let transit = 2_451_545. + noon + 0.0053 * m.sin() - 0.0069 * (2. * ecliptic).sin();
    let declination = (ecliptic.sin() * radians(23.4397).sin()).asin();
    let latitude = radians(latitude);
    let hour = (radians(-0.833).sin() - latitude.sin() * declination.sin())
        / (latitude.cos() * declination.cos());
    if hour < -1. {
        return Sun::Up;
    }
    if hour > 1. {
        return Sun::Down;
    }
    let half = hour.acos() / (2. * PI);
    let moment = |julian: f64| {
        let seconds = ((julian - 2_440_587.5) * 86_400.).round() as i64;
        DateTime::from_timestamp(seconds, 0).unwrap_or_default()
    };
    Sun::Rises {
        rise: moment(transit - half),
        set: moment(transit + half),
    }
}

/// What the schedule says at a moment.
#[derive(Clone, Debug, PartialEq)]
struct Plan {
    /// Whether it has Tessera's own night light on.
    on: bool,
    /// The times after that moment when it switches, in order, with what
    /// it switches to.
    turns: Vec<(DateTime<Utc>, bool)>,
}

impl Plan {
    /// When the light next becomes other than `shown`: the next turn of
    /// the schedule, or with a click holding against it the first that
    /// agrees with the click no more.
    fn next_change(&self, shown: bool) -> Option<DateTime<Utc>> {
        let turn = self.turns.iter().find(|(_, on)| *on != shown);
        turn.map(|(at, _)| *at)
    }
}

impl NightLightConfig {
    /// What the schedule says at `now`; `None` without a schedule, or
    /// with a place that is not one.
    fn scheduled<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<Plan> {
        let zone = now.timezone();
        let today = now.date_naive();
        let mut turns: Vec<(DateTime<Utc>, bool)> = Vec::new();
        let mut polar = None;
        for offset in -1i64..=2 {
            let day = match offset {
                -1 => today.checked_sub_days(Days::new(1)),
                offset => today.checked_add_days(Days::new(offset as u64)),
            }?;
            match self.schedule {
                NightSchedule::Off => return None,
                NightSchedule::Hours => {
                    if self.from == self.to {
                        return None;
                    }
                    for (Clock(time), on) in [(self.from, true), (self.to, false)] {
                        let at = zone.from_local_datetime(&day.and_time(time)).earliest();
                        turns.extend(at.map(|at| (at.with_timezone(&Utc), on)));
                    }
                }
                NightSchedule::Sun => {
                    if self.place_mistake().is_some() {
                        return None;
                    }
                    let (Some(latitude), Some(longitude)) = (self.latitude, self.longitude) else {
                        return None;
                    };
                    match sun(day, latitude, longitude) {
                        Sun::Rises { rise, set } => turns.extend([(set, true), (rise, false)]),
                        Sun::Up if offset == 0 => polar = Some(false),
                        Sun::Down if offset == 0 => polar = Some(true),
                        Sun::Up | Sun::Down => {}
                    }
                }
            }
        }
        turns.sort_by_key(|(at, _)| *at);
        let now = now.with_timezone(&Utc);
        let last = turns.iter().rev().find(|(at, _)| *at <= now);
        let on = polar.or(last.map(|(_, on)| *on)).unwrap_or(false);
        turns.retain(|(at, _)| *at > now);
        Some(Plan { on, turns })
    }

    /// What is wrong with the place, for a report.
    fn place_mistake(&self) -> Option<&'static str> {
        if self.schedule != NightSchedule::Sun {
            return None;
        }
        match (self.latitude, self.longitude) {
            (Some(latitude), Some(longitude))
                if (-90. ..=90.).contains(&latitude) && (-180. ..=180.).contains(&longitude) =>
            {
                None
            }
            (Some(_), Some(_)) => Some("latitude is -90 to 90 and longitude -180 to 180"),
            _ => Some("schedule = \"sun\" needs latitude and longitude"),
        }
    }
}

/// A choice made by a click, which holds until the schedule changes what
/// it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Hand {
    on: bool,
    /// What the schedule said when the click came.
    scheduled: Option<bool>,
}

/// Whether night light is to be on: as the schedule says, unless a click
/// chose otherwise since it last changed; a click that no longer holds is
/// forgotten. Without a schedule or a click it stays as it `was`.
fn decide(scheduled: Option<bool>, hand: &mut Option<Hand>, was: bool) -> bool {
    if let Some(chosen) = *hand {
        if chosen.scheduled == scheduled {
            return chosen.on;
        }
        *hand = None;
    }
    scheduled.unwrap_or(was)
}

/// The share of the slider for `kelvins`: 0 for no tint, 1 for the
/// warmest.
fn strength(kelvins: u32) -> f32 {
    let kelvins = kelvins.clamp(WARMEST, COLDEST);
    (COLDEST - kelvins) as f32 / (COLDEST - WARMEST) as f32
}

/// The kelvins for the share `strength` of the slider.
fn kelvins_at(strength: f32) -> u32 {
    let strength = strength.clamp(0., 1.);
    COLDEST - ((COLDEST - WARMEST) as f32 * strength).round() as u32
}

/// The night light of Windows, as the quick settings read it.
pub struct WindowsNightSource;

impl Source for WindowsNightSource {
    type State = Result<WindowsNight, &'static str>;

    const INTERVAL: Duration = Duration::from_secs(3);

    fn read() -> Self::State {
        WindowsNightLight::read()
    }
}

type WindowsReading = Reading<WindowsNightSource>;

/// Which night light is switched, once it is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Light {
    Windows,
    Tessera,
}

/// Night light, whichever it is, shared by every widget that shows it.
pub struct Night {
    config: NightLightConfig,
    light: Light,
    windows: Entity<WindowsReading>,
    /// Whether Tessera's own tints the displays.
    on: bool,
    /// How warm Tessera's own is.
    kelvins: u32,
    hand: Option<Hand>,
    /// When Tessera's own next changes by itself, as the schedule and a
    /// click that holds against it have it.
    next: Option<DateTime<Utc>>,
    /// The warmth last asked of Windows that is not written yet, and
    /// whether one is being written.
    wanted: Option<u32>,
    writing: bool,
    _windows: Subscription,
}

struct SharedNight(Entity<Night>);

impl Global for SharedNight {}

impl Night {
    /// Starts night light as `config` says: the colors that a Tessera
    /// stopped without giving them back left on the displays are given
    /// back, the schedule is looked at now and every half minute, and the
    /// colors of the displays are given back when Tessera quits.
    pub fn start(config: &NightLightConfig, cx: &mut App) {
        if cx.has_global::<SharedNight>() {
            return Self::configure(config, cx);
        }
        if let Some(mistake) = config.place_mistake() {
            report!("[night_light] has no schedule: {mistake}");
        }
        if let Some(directory) = config_dir() {
            GammaNight::heal(directory.join(KEPT_RAMPS));
        }
        let windows = Reading::<WindowsNightSource>::shared(cx);
        let night = cx.new(|cx| Self::new(config.clone(), windows, cx));
        cx.set_global(SharedNight(night.clone()));
        cx.on_app_quit(|_| {
            GammaNight::restore();
            async {}
        })
        .detach();
        let ticking = night.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                let Some(night) = ticking.upgrade() else {
                    break;
                };
                if night.update(cx, Self::tick).is_err() {
                    break;
                }
            }
        })
        .detach();
        night.update(cx, Self::tick);
    }

    /// Takes over a configuration that was read again.
    pub fn configure(config: &NightLightConfig, cx: &mut App) {
        let Some(night) = cx
            .try_global::<SharedNight>()
            .map(|shared| shared.0.clone())
        else {
            return Self::start(config, cx);
        };
        if night.read(cx).config == *config {
            return;
        }
        if let Some(mistake) = config.place_mistake() {
            report!("[night_light] has no schedule: {mistake}");
        }
        night.update(cx, |night, cx| night.take(config.clone(), cx));
    }

    /// The one night light, before or after it was started.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(shared) = cx.try_global::<SharedNight>() {
            return shared.0.clone();
        }
        Self::start(&NightLightConfig::default(), cx);
        cx.global::<SharedNight>().0.clone()
    }

    fn new(
        config: NightLightConfig,
        windows: Entity<WindowsReading>,
        cx: &mut Context<Self>,
    ) -> Self {
        let follow = cx.observe(&windows, |night: &mut Self, windows, cx| {
            let unreadable = matches!(windows.read(cx).state(), Some(Err(_)));
            if night.config.source == NightSource::Auto
                && night.light == Light::Windows
                && unreadable
            {
                night.light = Light::Tessera;
                night.tick(cx);
            }
            // A warmth that waited for a switch to be done is written now.
            night.write(cx);
            cx.notify();
        });
        let light = Self::light_for(config.source);
        if light == Light::Windows {
            windows.update(cx, WindowsReading::refresh);
        }
        Self {
            kelvins: config.kelvins.clamp(WARMEST, COLDEST),
            config,
            light,
            windows,
            on: false,
            hand: None,
            next: None,
            wanted: None,
            writing: false,
            _windows: follow,
        }
    }

    fn light_for(source: NightSource) -> Light {
        match source {
            NightSource::Windows => Light::Windows,
            NightSource::Tessera => Light::Tessera,
            NightSource::Auto if shell_mode() => Light::Tessera,
            NightSource::Auto => Light::Windows,
        }
    }

    fn take(&mut self, config: NightLightConfig, cx: &mut Context<Self>) {
        let old = std::mem::replace(&mut self.config, config);
        if old.kelvins != self.config.kelvins {
            self.kelvins = self.config.kelvins.clamp(WARMEST, COLDEST);
            if self.on {
                GammaNight::show(Some(self.kelvins));
            }
        }
        if old.source != self.config.source {
            let light = Self::light_for(self.config.source);
            if light == Light::Windows {
                self.show_own(false);
                self.windows.update(cx, WindowsReading::refresh);
            }
            self.light = light;
        }
        if (old.schedule, old.from, old.to, old.latitude, old.longitude)
            != (
                self.config.schedule,
                self.config.from,
                self.config.to,
                self.config.latitude,
                self.config.longitude,
            )
        {
            self.hand = None;
        }
        self.tick(cx);
        cx.notify();
    }

    /// Follows the schedule of Tessera's own night light, and gives its
    /// tint again to displays that lost it.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.light != Light::Tessera {
            return;
        }
        let plan = self.config.scheduled(&Local::now());
        let on = decide(plan.as_ref().map(|plan| plan.on), &mut self.hand, self.on);
        let next = plan.and_then(|plan| plan.next_change(on));
        if (on, next) != (self.on, self.next) {
            cx.notify();
        }
        self.next = next;
        if on || self.on {
            self.show_own(on);
        }
    }

    /// Tints the displays or gives them their colors back.
    fn show_own(&mut self, on: bool) {
        if on || self.on {
            GammaNight::show(on.then_some(self.kelvins));
        }
        self.on = on;
    }

    /// Whether night light is on; `None` while it is not known.
    fn is_on(&self, cx: &App) -> Option<bool> {
        match self.light {
            Light::Tessera => Some(self.on),
            Light::Windows => match self.windows.read(cx).state() {
                Some(Ok(state)) => Some(state.on),
                _ => None,
            },
        }
    }

    /// Switches night light on or off, or over with `None`.
    fn switch(&mut self, on: Option<bool>, cx: &mut Context<Self>) {
        match self.light {
            Light::Tessera => {
                let on = on.unwrap_or(!self.on);
                let plan = self.config.scheduled(&Local::now());
                self.next = plan.as_ref().and_then(|plan| plan.next_change(on));
                self.hand = Some(Hand {
                    on,
                    scheduled: plan.map(|plan| plan.on),
                });
                self.show_own(on);
                cx.notify();
            }
            Light::Windows => self.windows.update(cx, |reading, cx| {
                let known = reading
                    .state()
                    .and_then(|state| state.as_ref().ok())
                    .copied();
                match known {
                    Some(state) => {
                        let on = on.unwrap_or(!state.on);
                        let shown = WindowsNight { on, ..state };
                        let work = move || WindowsNightLight::switch(on);
                        reading.change(Some(Ok(shown)), work, |(), _| {}, cx);
                    }
                    // Not read yet, as for a key pressed before any
                    // widget showed it: read and switch in one go.
                    None => {
                        let work = move || {
                            let now = WindowsNightLight::read().map_err(str::to_owned)?;
                            WindowsNightLight::switch(on.unwrap_or(!now.on))
                        };
                        reading.change(None, work, |(), _| {}, cx);
                    }
                }
            }),
        }
    }

    /// How warm the light is, in kelvins; `None` where Windows does not
    /// say.
    fn kelvins(&self, cx: &App) -> Option<u32> {
        match self.light {
            Light::Tessera => Some(self.kelvins),
            Light::Windows => self.wanted.or_else(|| match self.windows.read(cx).state() {
                Some(Ok(state)) => state.kelvins,
                _ => None,
            }),
        }
    }

    fn set_kelvins(&mut self, kelvins: u32, cx: &mut Context<Self>) {
        let kelvins = kelvins.clamp(WARMEST, COLDEST);
        match self.light {
            Light::Tessera => {
                self.kelvins = kelvins;
                if self.on {
                    GammaNight::show(Some(kelvins));
                }
            }
            Light::Windows => {
                self.wanted = Some(kelvins);
                self.write(cx);
            }
        }
        cx.notify();
    }

    /// Writes the warmth last asked for into the settings of Windows, and
    /// once that is done the one asked for meanwhile.
    fn write(&mut self, cx: &mut Context<Self>) {
        if self.writing {
            return;
        }
        let Some(kelvins) = self.wanted else {
            return;
        };
        let night = cx.entity();
        let started = self.windows.update(cx, |reading, cx| {
            let shown = match reading.state() {
                Some(Ok(state)) => Some(Ok(WindowsNight {
                    kelvins: Some(kelvins),
                    ..*state
                })),
                _ => None,
            };
            let work = move || Ok(WindowsNightLight::set_kelvins(kelvins));
            let then = move |written: Result<(), String>, cx: &mut App| {
                if let Err(error) = written {
                    report!("{error}");
                }
                night.update(cx, |night, cx| {
                    night.writing = false;
                    if night.wanted == Some(kelvins) {
                        night.wanted = None;
                    }
                    night.write(cx);
                    cx.notify();
                });
            };
            reading.change(shown, work, then, cx)
        });
        // Under way is a switch, after which the slider asks again.
        self.writing = started;
    }

    /// What the tile shows.
    fn face(&self, language: Language, cx: &App) -> TileFace {
        let on = self.is_on(cx);
        let note = match (on, self.light, self.next) {
            (None, _, _) => {
                let asked = self.windows.read(cx).state().is_some();
                unusable_note(asked, language).to_owned()
            }
            (Some(on), Light::Tessera, Some(next)) => {
                let at = next.with_timezone(&Local).format("%H:%M");
                format!("{} {} {at}", on_or_off(on, language), language.say("until"))
            }
            (Some(on), _, _) => on_or_off(on, language).to_owned(),
        };
        TileFace {
            glyph: ICON_NIGHT,
            title: language.say("Night light").into(),
            note: note.into(),
            lit: on == Some(true),
            usable: on.is_some(),
        }
    }

    /// What is said under the slider of how warm the light is.
    fn schedule_note(&self, language: Language) -> String {
        if self.light == Light::Windows {
            return language
                .say("Windows switches it by the schedule of its settings")
                .to_owned();
        }
        match self.config.schedule {
            NightSchedule::Off => language.say("Tessera's own; no schedule").to_owned(),
            NightSchedule::Sun if self.config.place_mistake().is_none() => language
                .say("Tessera's own, from sunset to sunrise")
                .to_owned(),
            NightSchedule::Sun => language
                .say("Tessera's own; the schedule needs the place")
                .to_owned(),
            NightSchedule::Hours => format!(
                "{}: {} \u{2013} {}",
                language.say("Tessera's own, by the hours"),
                self.config.from.0.format("%H:%M"),
                self.config.to.0.format("%H:%M")
            ),
        }
    }
}

/// The tile of night light.
struct NightTile;

impl Face for NightTile {
    type Model = Night;

    fn model(&self, cx: &mut App) -> Entity<Night> {
        Night::shared(cx)
    }

    fn follow(model: &Entity<Night>, cx: &mut Context<TileView<Self>>) -> Subscription {
        let windows = model.read(cx).windows.clone();
        Reading::poll(&windows, cx);
        cx.observe(model, |_, _, cx| cx.notify())
    }

    fn face(&self, model: &Night, language: Language, cx: &App) -> TileFace {
        model.face(language, cx)
    }

    fn click(&self, model: &Entity<Night>, cx: &mut App) {
        model.update(cx, |night, cx| night.switch(None, cx));
    }
}

/// How warm the light is, and when it comes on.
struct NightSlider {
    night: Entity<Night>,
    slider: Slider,
    settings: Interaction,
    _follow: Subscription,
}

impl NightSlider {
    fn new(cx: &mut Context<Self>) -> Self {
        let night = Night::shared(cx);
        let windows = night.read(cx).windows.clone();
        Reading::poll(&windows, cx);
        let theme = *cx.global::<Theme>();
        let share = night.read(cx).kelvins(cx).map_or(0., strength);
        Self {
            _follow: cx.observe(&night, |_, _, cx| cx.notify()),
            night,
            slider: Slider::new(&theme, share),
            settings: Interaction::new(&theme.motion),
        }
    }
}

impl Render for NightSlider {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        let night = self.night.read(cx);
        let kelvins = night.kelvins(cx);
        let on = night.is_on(cx);
        let light = night.light;
        let schedule = night.schedule_note(language);
        let reason = match night.windows.read(cx).state() {
            Some(Err(reason)) if light == Light::Windows => Some(*reason),
            _ => None,
        };
        let link_state = self.settings.state(window);
        let link = SETTINGS.link(&theme, language, link_state);
        let link = Interaction::track(link, cx, |this: &mut Self| Some(&mut this.settings));
        let note = |text: String| {
            div()
                .px(theme.spacing.small)
                .text_color(theme.colors.on_surface_variant)
                .child(text)
        };
        let frame = div().w_full().flex().flex_col().gap(theme.spacing.small);
        let Some(kelvins) = kelvins else {
            let text = match reason {
                Some(reason) => sentence(language.say(reason)),
                None if on.is_none() => language.say("Reading…").to_owned(),
                None => language
                    .say("Windows holds no warmth of night light yet; set it once in its settings")
                    .to_owned(),
            };
            return frame.child(note(text)).child(link).into_any_element();
        };
        self.slider.dim(on != Some(true));
        let share = strength(kelvins);
        let track = self.slider.track(
            "night-light-track",
            share,
            window,
            cx,
            |this: &mut Self| &mut this.slider,
            |this, share, cx| {
                this.night
                    .update(cx, |night, cx| night.set_kelvins(kelvins_at(share), cx));
            },
        );
        let slider = theme.slider;
        let row = div()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .child(
                div()
                    .size(slider.handle_height)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(&theme, ICON_NIGHT)),
            )
            .child(track)
            .child(
                div()
                    .flex_none()
                    .w(slider.value_width)
                    .flex()
                    .justify_end()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{kelvins} K")),
            );
        frame
            .child(row)
            .child(note(schedule))
            .child(link)
            .into_any_element()
    }
}

pub struct NightModule;

impl Module for NightModule {
    fn name(&self) -> &'static str {
        "night"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        register_tile(widgets, "night-light", |_| NightTile)?;
        widgets.register_panel("night-light-slider", |_, cx| {
            cx.new(NightSlider::new).into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let action = Action::new(|arg, cx| {
            let on = match arg {
                Some("on") => Some(true),
                Some("off") => Some(false),
                _ => None,
            };
            let night = Night::shared(cx);
            night.update(cx, |night, cx| night.switch(on, cx));
        })
        .checking(|arg| match arg {
            None | Some("on" | "off" | "toggle") => Ok(()),
            Some(_) => Err("`night-light` takes `on`, `off`, `toggle` or no `arg`".to_owned()),
        });
        actions.register("night-light", action)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, FixedOffset, Timelike};

    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn minutes(at: DateTime<Utc>) -> i64 {
        i64::from(at.hour() * 60 + at.minute())
    }

    #[test]
    fn the_sun_rises_and_sets_when_almanacs_say() {
        // London on the longest day: 03:43 and 20:21 UTC.
        let Sun::Rises { rise, set } = sun(date(2024, 6, 21), 51.5074, -0.1278) else {
            panic!("the sun sets in London");
        };
        assert!((minutes(rise) - (3 * 60 + 43)).abs() <= 3, "{rise}");
        assert!((minutes(set) - (20 * 60 + 21)).abs() <= 3, "{set}");
        // Moscow on the shortest day: 06:00 and 12:58 UTC.
        let Sun::Rises { rise, set } = sun(date(2024, 12, 21), 55.7558, 37.6173) else {
            panic!("the sun rises in Moscow");
        };
        assert!((minutes(rise) - 6 * 60).abs() <= 3, "{rise}");
        assert!((minutes(set) - (12 * 60 + 58)).abs() <= 3, "{set}");
    }

    #[test]
    fn beyond_the_polar_circle_the_sun_may_not_rise_or_set() {
        assert_eq!(sun(date(2024, 6, 21), 69.65, 18.96), Sun::Up);
        assert_eq!(sun(date(2024, 12, 21), 69.65, 18.96), Sun::Down);
    }

    fn hours(from: &str, to: &str) -> NightLightConfig {
        NightLightConfig {
            schedule: NightSchedule::Hours,
            from: Clock::try_from(from.to_owned()).unwrap(),
            to: Clock::try_from(to.to_owned()).unwrap(),
            ..NightLightConfig::default()
        }
    }

    #[test]
    fn fixed_hours_hold_across_midnight() {
        let zone = FixedOffset::east_opt(3 * 3600).unwrap();
        let at = |day, hour, minute| {
            zone.from_local_datetime(&date(2026, 11, day).and_hms_opt(hour, minute, 0).unwrap())
                .unwrap()
        };
        let config = hours("21:00", "07:00");
        let plan = config.scheduled(&at(27, 22, 15)).unwrap();
        assert!(plan.on);
        assert_eq!(
            plan.next_change(true),
            Some(at(28, 7, 0).with_timezone(&Utc))
        );
        let plan = config.scheduled(&at(27, 6, 59)).unwrap();
        assert!(plan.on);
        assert_eq!(
            plan.next_change(true),
            Some(at(27, 7, 0).with_timezone(&Utc))
        );
        let plan = config.scheduled(&at(27, 12, 0)).unwrap();
        assert!(!plan.on);
        assert_eq!(
            plan.next_change(false),
            Some(at(27, 21, 0).with_timezone(&Utc))
        );
        // The same hours in a day, and no schedule, say nothing.
        assert_eq!(hours("08:00", "08:00").scheduled(&at(27, 9, 0)), None);
        assert_eq!(NightLightConfig::default().scheduled(&at(27, 9, 0)), None);
    }

    #[test]
    fn the_sun_schedule_follows_sunset_and_sunrise() {
        let config = NightLightConfig {
            schedule: NightSchedule::Sun,
            latitude: Some(51.5074),
            longitude: Some(-0.1278),
            ..NightLightConfig::default()
        };
        let at = |hour, minute| {
            Utc.from_utc_datetime(&date(2024, 6, 21).and_hms_opt(hour, minute, 0).unwrap())
        };
        let plan = config.scheduled(&at(12, 0)).unwrap();
        assert!(!plan.on);
        assert!(
            plan.next_change(false)
                .is_some_and(|next| next.hour() == 20)
        );
        let plan = config.scheduled(&at(23, 0)).unwrap();
        assert!(plan.on);
        let next = plan.next_change(true);
        assert!(next.is_some_and(|next| next.day() == 22 && next.hour() == 3));
        // Without the place there is no schedule.
        let nowhere = NightLightConfig {
            latitude: None,
            ..config.clone()
        };
        assert_eq!(nowhere.scheduled(&at(23, 0)), None);
        assert!(nowhere.place_mistake().is_some());
        assert_eq!(config.place_mistake(), None);
        // Nor with a place that is not one.
        let beyond = NightLightConfig {
            latitude: Some(95.),
            ..config.clone()
        };
        assert_eq!(beyond.scheduled(&at(23, 0)), None);
        assert!(beyond.place_mistake().is_some());
    }

    #[test]
    fn a_click_against_the_schedule_holds_until_the_schedule_agrees_no_more() {
        let zone = FixedOffset::east_opt(0).unwrap();
        let at = |day, hour| {
            zone.from_local_datetime(&date(2026, 11, day).and_hms_opt(hour, 0, 0).unwrap())
                .unwrap()
        };
        // Switched off at 22:00 by hand: the schedule goes off at 07:00,
        // which changes nothing, and the light comes on at 21:00.
        let plan = hours("21:00", "07:00").scheduled(&at(27, 22)).unwrap();
        assert!(plan.on);
        assert_eq!(
            plan.next_change(false),
            Some(at(28, 21).with_timezone(&Utc))
        );
        // Switched on at noon by hand: it stays on until 07:00.
        let plan = hours("21:00", "07:00").scheduled(&at(27, 12)).unwrap();
        assert_eq!(plan.next_change(true), Some(at(28, 7).with_timezone(&Utc)));
    }

    #[test]
    fn a_click_holds_until_the_schedule_changes_its_mind() {
        let mut hand = Some(Hand {
            on: false,
            scheduled: Some(true),
        });
        // The evening the light was switched off by hand: it stays off.
        assert!(!decide(Some(true), &mut hand, false));
        assert!(hand.is_some());
        // The morning: the schedule says off, and the click is forgotten.
        assert!(!decide(Some(false), &mut hand, false));
        assert_eq!(hand, None);
        // The next evening it comes on by itself.
        assert!(decide(Some(true), &mut hand, false));
        // Without a schedule it stays as it was.
        assert!(decide(None, &mut None, true));
    }

    #[test]
    fn the_slider_runs_from_no_tint_to_the_warmest() {
        assert_eq!(strength(COLDEST), 0.);
        assert_eq!(strength(WARMEST), 1.);
        assert_eq!(kelvins_at(0.), COLDEST);
        assert_eq!(kelvins_at(1.), WARMEST);
        for kelvins in [1200, 2700, 3400, 4500, 6500] {
            assert_eq!(kelvins_at(strength(kelvins)), kelvins);
        }
    }

    #[test]
    fn the_configuration_reads_times_of_day() {
        let config: NightLightConfig =
            toml::from_str("schedule = \"hours\"\nfrom = \"22:30\"\nto = \"06:15\"\n").unwrap();
        assert_eq!(
            config.from,
            Clock(NaiveTime::from_hms_opt(22, 30, 0).unwrap())
        );
        assert!(toml::from_str::<NightLightConfig>("from = \"late\"\n").is_err());
        assert!(toml::from_str::<NightLightConfig>("color = 1\n").is_err());
    }
}
