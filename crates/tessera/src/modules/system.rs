// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How busy the machine is and how much charge it has left.
//! Widgets: `cpu`, `memory`, `battery`, `disk`, `network`, `gpu`.
//!
//! Each meter is read every `[system] interval` seconds (two by default),
//! or as often as its own option `interval` says; the battery and the
//! drives are read no more than every 20 and 30 seconds unless their own
//! option says otherwise. What is read is shared: meters of the same kind
//! read as often show one reading, taken once away from the thread that
//! draws.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Div, Entity, FontWeight, Global, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, WeakEntity, Window, div,
};
use serde::Deserialize;

use crate::files::size_text;
use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{
    Battery, GpuLoad, GpuUsage, MemoryLoad, NetworkTraffic, ProcessorLoad, disk_space,
};
use crate::theme::Theme;
use crate::ui::{icon, pill, wave_meter};

/// How wide the value is, in label sizes: room for "100%", so that the
/// pill keeps its width while the value changes.
const VALUE_WIDTH: f32 = 2.4;

/// The same for a value in gigabytes, such as "31.4 GB".
const AMOUNT_WIDTH: f32 = 3.8;

const BYTES_PER_GIGABYTE: f64 = 1_073_741_824.;

/// The fewest and the most seconds between two readings a configuration
/// may ask for.
const FEWEST_SECONDS: f64 = 1.;
const MOST_SECONDS: f64 = 60.;

/// `[system]`: how often the meters are read.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SystemConfig {
    /// Seconds between two readings of a meter that names none itself,
    /// from 1 to 60.
    pub interval: f64,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self { interval: 2. }
    }
}

/// `seconds` as the time between two readings, within the range a
/// configuration may ask for; a value out of it is reported.
fn interval_of(seconds: f64, whose: &str) -> Duration {
    let kept = match seconds.is_finite() {
        true => seconds.clamp(FEWEST_SECONDS, MOST_SECONDS),
        false => SystemConfig::default().interval,
    };
    if kept != seconds {
        report!("{whose} must be from 1 to 60 seconds, {seconds} is taken as {kept}");
    }
    Duration::from_secs_f64(kept)
}

/// How often the meters are read where they say nothing themselves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Meters {
    interval: Duration,
}

impl Global for Meters {}

impl Meters {
    /// Takes `[system]`; the meters that follow it are read as often
    /// from now on.
    pub fn configure(config: &SystemConfig, cx: &mut App) {
        let meters = Self {
            interval: interval_of(config.interval, "`[system] interval`"),
        };
        if cx.try_global::<Self>() != Some(&meters) {
            cx.set_global(meters);
        }
    }

    /// The time between two readings where a meter says nothing.
    fn interval(cx: &App) -> Duration {
        cx.try_global::<Self>().map_or_else(
            || Duration::from_secs_f64(SystemConfig::default().interval),
            |meters| meters.interval,
        )
    }
}

/// What a meter reads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Probe {
    Processor,
    Memory,
    Battery,
    /// The drive with this root.
    Disk(PathBuf),
    Network,
    Gpu,
}

impl Probe {
    /// The least time between two readings where the meter names none: a
    /// battery and a drive change slowly.
    fn least_interval(&self) -> Duration {
        match self {
            Self::Battery => Duration::from_secs(20),
            Self::Disk(_) => Duration::from_secs(30),
            _ => Duration::ZERO,
        }
    }

    fn sampler(&self) -> Sampler {
        match self {
            Self::Processor => Sampler::Processor(ProcessorLoad::default()),
            Self::Memory => Sampler::Memory,
            Self::Battery => Sampler::Battery,
            Self::Disk(root) => Sampler::Disk(root.clone()),
            Self::Network => Sampler::Network(NetworkTraffic::default()),
            Self::Gpu => Sampler::Gpu(None),
        }
    }
}

/// What a probe read at one moment.
#[derive(Clone, Debug, PartialEq)]
enum Sample {
    /// The busy share of the processors.
    Processor(f32),
    Memory(MemoryLoad),
    Battery(Battery),
    Disk {
        total: u64,
        free: u64,
    },
    /// Bytes per second.
    Network {
        received: f64,
        sent: f64,
    },
    Gpu(GpuUsage),
}

/// Reads a probe, keeping what the next reading is compared with. It is
/// moved to a thread of the background for each reading.
enum Sampler {
    Processor(ProcessorLoad),
    Memory,
    Battery,
    Disk(PathBuf),
    Network(NetworkTraffic),
    /// Opened at the first reading, where Windows has the counters.
    Gpu(Option<Option<GpuLoad>>),
}

impl Sampler {
    /// The state now; `None` when there is nothing new.
    fn sample(&mut self) -> Option<Sample> {
        match self {
            Self::Processor(load) => load.read().map(Sample::Processor),
            Self::Memory => MemoryLoad::read().map(Sample::Memory),
            Self::Battery => Battery::read().map(Sample::Battery),
            Self::Disk(root) => {
                let (total, free) = disk_space(root)?;
                Some(Sample::Disk { total, free })
            }
            Self::Network(traffic) => {
                let (received, sent) = traffic.read()?;
                Some(Sample::Network { received, sent })
            }
            Self::Gpu(Some(load)) => load.as_mut()?.read().map(Sample::Gpu),
            // The counters are rates: a reading right after the first
            // collection would cover no time, so the first comes at the next
            // interval, as the first of the processor does.
            Self::Gpu(load) => {
                let opened = GpuLoad::open();
                if opened.is_none() {
                    report!("Windows gives no counters of the graphics processors");
                }
                *load = Some(opened);
                None
            }
        }
    }
}

/// The readings of one probe, taken every so often while a meter shows
/// them, and shared by every meter that reads the same as often.
struct Feed {
    sample: Option<Sample>,
}

/// The feeds being read, by probe and interval.
#[derive(Default)]
struct Feeds(HashMap<(Probe, Duration), WeakEntity<Feed>>);

impl Global for Feeds {}

impl Feed {
    /// The feed of `probe` read every `interval`: the one being read
    /// already, or a new one, which is read until no meter holds it.
    fn of(probe: Probe, interval: Duration, cx: &mut App) -> Entity<Self> {
        let feeds = &mut cx.default_global::<Feeds>().0;
        feeds.retain(|_, feed| feed.upgrade().is_some());
        let key = (probe, interval);
        if let Some(feed) = feeds.get(&key).and_then(WeakEntity::upgrade) {
            return feed;
        }
        let feed = cx.new(|_| Self { sample: None });
        let weak = feed.downgrade();
        cx.default_global::<Feeds>()
            .0
            .insert(key.clone(), weak.clone());
        let mut sampler = key.0.sampler();
        cx.spawn(async move |cx: &mut AsyncApp| {
            loop {
                let (kept, sample) = cx
                    .background_executor()
                    .spawn(async move {
                        let sample = sampler.sample();
                        (sampler, sample)
                    })
                    .await;
                sampler = kept;
                let taken = weak.update(cx, |feed, cx| {
                    // A probe that has nothing new keeps what it showed.
                    if sample.is_some() && feed.sample != sample {
                        feed.sample = sample;
                        cx.notify();
                    }
                });
                if taken.is_err() {
                    break;
                }
                cx.background_executor().timer(interval).await;
            }
        })
        .detach();
        feed
    }
}

pub struct SystemModule;

impl Module for SystemModule {
    fn name(&self) -> &'static str {
        "system"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("cpu", |context, cx| {
            cx.new(|cx| Meter::new(Box::new(ProcessorGauge), context, cx))
                .into()
        })?;
        widgets.register("memory", |context, cx| {
            let gauge = MemoryGauge {
                amount: one_of(context, "show", &["percent", "used"]) == 1,
            };
            cx.new(|cx| Meter::new(Box::new(gauge), context, cx)).into()
        })?;
        widgets.register("battery", |context, cx| {
            cx.new(|cx| Meter::new(Box::new(BatteryGauge), context, cx))
                .into()
        })?;
        widgets.register("disk", |context, cx| {
            let gauge = DiskGauge {
                root: context.text("drive").unwrap_or("C:\\").into(),
                amount: one_of(context, "show", &["percent", "free"]) == 1,
            };
            cx.new(|cx| Meter::new(Box::new(gauge), context, cx)).into()
        })?;
        // Before the network meter, whose name is also that of the
        // connection widget of the network module: that module comes
        // first and keeps the name, and the refusal ends this list.
        widgets.register("gpu", |context, cx| {
            let gauge = GpuGauge {
                amount: one_of(context, "show", &["percent", "memory"]) == 1,
            };
            cx.new(|cx| Meter::new(Box::new(gauge), context, cx)).into()
        })?;
        widgets.register("network", |context, cx| {
            let megabits = context.number("megabits").unwrap_or(100.).max(1.);
            let gauge = NetworkGauge {
                full: megabits * 1_000_000. / 8.,
            };
            cx.new(|cx| Meter::new(Box::new(gauge), context, cx)).into()
        })
    }
}

/// The index in `choices` of the text option `key`; the first choice when
/// the option is absent. Anything else is reported and also the first.
fn one_of(context: &WidgetContext, key: &str, choices: &[&str]) -> usize {
    let Some(chosen) = context.text(key) else {
        return 0;
    };
    choices
        .iter()
        .position(|choice| *choice == chosen)
        .unwrap_or_else(|| {
            report!(
                "`{key}` must be one of {choices:?}, `{chosen}` is taken as `{}`",
                choices[0]
            );
            0
        })
}

/// What a gauge shows at one moment.
#[derive(PartialEq)]
struct Reading {
    /// From 0 to 1.
    level: f32,
    /// A character of the theme's icon font.
    glyph: &'static str,
    text: String,
    /// How many label sizes to keep for the text.
    text_width: f32,
}

/// Something about the machine that can be read as a level.
trait Gauge {
    /// What is read for it.
    fn probe(&self) -> Probe;

    /// What `sample` shows; `None` when there is nothing to show.
    fn show(&self, sample: &Sample) -> Option<Reading>;
}

struct ProcessorGauge;

impl Gauge for ProcessorGauge {
    fn probe(&self) -> Probe {
        Probe::Processor
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Processor(busy) = *sample else {
            return None;
        };
        let level = whole_percent(busy);
        Some(Reading {
            level,
            glyph: "\u{E950}",
            text: percent(level),
            text_width: VALUE_WIDTH,
        })
    }
}

struct MemoryGauge {
    /// Show gigabytes in use instead of a percentage.
    amount: bool,
}

impl Gauge for MemoryGauge {
    fn probe(&self) -> Probe {
        Probe::Memory
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Memory(memory) = sample else {
            return None;
        };
        let level = whole_percent((memory.used as f64 / memory.total.max(1) as f64) as f32);
        Some(Reading {
            level,
            glyph: "\u{E964}",
            text: if self.amount {
                gigabytes(memory.used)
            } else {
                percent(level)
            },
            text_width: if self.amount {
                AMOUNT_WIDTH
            } else {
                VALUE_WIDTH
            },
        })
    }
}

struct BatteryGauge;

impl Gauge for BatteryGauge {
    fn probe(&self) -> Probe {
        Probe::Battery
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Battery(battery) = *sample else {
            return None;
        };
        Some(Reading {
            level: f32::from(battery.percent) / 100.,
            glyph: battery_glyph(battery),
            text: format!("{}%", battery.percent),
            text_width: VALUE_WIDTH,
        })
    }
}

/// How full the drive with `root` is. Option `drive` (`C:\`), and
/// `show = "free"` for what is left instead of the share used.
struct DiskGauge {
    root: PathBuf,
    amount: bool,
}

impl Gauge for DiskGauge {
    fn probe(&self) -> Probe {
        Probe::Disk(self.root.clone())
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Disk { total, free } = *sample else {
            return None;
        };
        let level = whole_percent(1. - free as f32 / total.max(1) as f32);
        Some(Reading {
            level,
            glyph: "\u{EDA2}",
            text: match self.amount {
                true => size_text(free),
                false => percent(level),
            },
            text_width: if self.amount {
                AMOUNT_WIDTH
            } else {
                VALUE_WIDTH
            },
        })
    }
}

/// What goes through the network: the level is the busier way against
/// `megabits` per second (100), the text what is received each second.
struct NetworkGauge {
    /// Bytes per second that fill the meter.
    full: f64,
}

impl Gauge for NetworkGauge {
    fn probe(&self) -> Probe {
        Probe::Network
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Network { received, sent } = *sample else {
            return None;
        };
        let level = whole_percent((received.max(sent) / self.full).min(1.) as f32);
        Some(Reading {
            level,
            glyph: "\u{E896}",
            text: format!("{}/s", size_text(received as u64)),
            text_width: AMOUNT_WIDTH + 1.,
        })
    }
}

/// How busy the graphics processors are: the level and the text are the
/// busiest engine, as Task Manager shows it; `show = "memory"` has the
/// text the memory of its own in use and the level its share of it.
struct GpuGauge {
    amount: bool,
}

impl Gauge for GpuGauge {
    fn probe(&self) -> Probe {
        Probe::Gpu
    }

    fn show(&self, sample: &Sample) -> Option<Reading> {
        let Sample::Gpu(usage) = *sample else {
            return None;
        };
        let busy = whole_percent(usage.busy);
        let (level, text, text_width) = match (self.amount, usage.total) {
            (false, _) => (busy, percent(busy), VALUE_WIDTH),
            (true, Some(total)) if total > 0 => {
                let share = whole_percent((usage.used as f64 / total as f64).min(1.) as f32);
                (share, gigabytes(usage.used), AMOUNT_WIDTH)
            }
            (true, _) => (busy, gigabytes(usage.used), AMOUNT_WIDTH),
        };
        Some(Reading {
            level,
            glyph: "\u{E7F4}",
            text,
            text_width,
        })
    }
}

/// `level` at the nearest hundredth. A level wavers by less all the time,
/// and a reading that differs only so is the same reading: it has
/// nothing drawn again.
fn whole_percent(level: f32) -> f32 {
    (level * 100.).round() / 100.
}

fn percent(level: f32) -> String {
    format!("{:.0}%", level * 100.)
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / BYTES_PER_GIGABYTE)
}

/// The battery icon filled to the nearest tenth, with a bolt when the
/// machine is plugged in.
fn battery_glyph(battery: Battery) -> &'static str {
    const DISCHARGING: [&str; 11] = [
        "\u{E850}", "\u{E851}", "\u{E852}", "\u{E853}", "\u{E854}", "\u{E855}", "\u{E856}",
        "\u{E857}", "\u{E858}", "\u{E859}", "\u{E83F}",
    ];
    const CHARGING: [&str; 11] = [
        "\u{E85A}", "\u{E85B}", "\u{E85C}", "\u{E85D}", "\u{E85E}", "\u{E85F}", "\u{E860}",
        "\u{E861}", "\u{E862}", "\u{E83E}", "\u{EA93}",
    ];
    let tenths = usize::from((battery.percent.min(100) + 5) / 10);
    if battery.plugged_in {
        CHARGING[tenths]
    } else {
        DISCHARGING[tenths]
    }
}

/// The time between two readings of `probe`: the meter's own, or else
/// the default of `[system]` but no less than what the probe needs.
fn every(probe: &Probe, own: Option<Duration>, default: Duration) -> Duration {
    own.unwrap_or_else(|| default.max(probe.least_interval()))
}

/// The change of a level, as a share of the whole, below which the meter
/// moves in one step. Levels waver by less than this all the time, and
/// following each waver with a spring would keep every bar repainting.
const SMALL_CHANGE: f32 = 0.03;

/// How a meter draws its level.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// A wavy line and the value.
    Wave,
    /// The value alone.
    Text,
}

/// Shows a gauge in a pill and follows it.
struct Meter {
    gauge: Box<dyn Gauge>,
    /// The time between readings the widget's option `interval` asks for.
    own_interval: Option<Duration>,
    feed: Entity<Feed>,
    reading: Option<Reading>,
    /// The level drawn, which springs after the level read.
    level: Spring,
    style: Style,
    _feed: Subscription,
    /// Follows the default of `[system]`, for a meter without its own.
    _default: Option<Subscription>,
}

impl Meter {
    fn new(gauge: Box<dyn Gauge>, context: &WidgetContext, cx: &mut Context<Self>) -> Self {
        let own_interval = { context.number("interval") }
            .map(|seconds| interval_of(seconds, "the widget option `interval`"));
        let interval = every(&gauge.probe(), own_interval, Meters::interval(cx));
        let feed = Feed::of(gauge.probe(), interval, cx);
        let reading = feed.read(cx).sample.as_ref().and_then(|s| gauge.show(s));
        let mut level = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        level.animate_to(reading.as_ref().map_or(0., |reading| reading.level));
        let default = own_interval
            .is_none()
            .then(|| cx.observe_global::<Meters>(Self::rebind));
        Self {
            gauge,
            own_interval,
            _feed: cx.observe(&feed, |this, _, cx| this.refresh(cx)),
            feed,
            reading,
            level,
            style: match one_of(context, "style", &["wave", "text"]) {
                0 => Style::Wave,
                _ => Style::Text,
            },
            _default: default,
        }
    }

    /// Follows the feed read as often as `[system]` now says.
    fn rebind(&mut self, cx: &mut Context<Self>) {
        let interval = every(&self.gauge.probe(), self.own_interval, Meters::interval(cx));
        let feed = Feed::of(self.gauge.probe(), interval, cx);
        if feed == self.feed {
            return;
        }
        self._feed = cx.observe(&feed, |this, _, cx| this.refresh(cx));
        self.feed = feed;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let feed = self.feed.read(cx);
        // A gauge that has nothing new keeps what it showed.
        let Some(reading) = feed.sample.as_ref().and_then(|s| self.gauge.show(s)) else {
            return;
        };
        if self.reading.as_ref() == Some(&reading) {
            return;
        }
        let change = (reading.level - self.level.target()).abs();
        // Without the line there is nothing a spring would move.
        if self.style == Style::Text || (change < SMALL_CHANGE && self.level.is_settled()) {
            self.level.jump_to(reading.level);
        } else {
            self.level.animate_to(reading.level);
        }
        self.reading = Some(reading);
        cx.notify();
    }
}

impl Render for Meter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let Some(reading) = &self.reading else {
            return div();
        };
        let level = self.level.value(window);
        let value: Div = div()
            .min_w(theme.typography.label_size * reading.text_width)
            .flex()
            .justify_end()
            .child(SharedString::from(reading.text.clone()));
        pill(&theme)
            .font_weight(FontWeight::MEDIUM)
            .child(icon(&theme, reading.glyph).text_color(theme.colors.on_surface_variant))
            .children((self.style == Style::Wave).then(|| wave_meter(&theme, level)))
            .child(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_battery_icon_follows_charge_and_power() {
        let battery = |percent, plugged_in| Battery {
            percent,
            plugged_in,
        };
        assert_eq!(battery_glyph(battery(0, false)), "\u{E850}");
        assert_eq!(battery_glyph(battery(54, false)), "\u{E855}");
        assert_eq!(battery_glyph(battery(100, false)), "\u{E83F}");
        assert_eq!(battery_glyph(battery(96, true)), "\u{EA93}");
        assert_eq!(battery_glyph(battery(4, true)), "\u{E85A}");
    }

    #[test]
    fn a_level_that_wavers_within_a_percent_reads_the_same() {
        assert_eq!(whole_percent(0.4212), whole_percent(0.4188));
        assert_ne!(whole_percent(0.42), whole_percent(0.43));
        assert_eq!(percent(whole_percent(0.4188)), "42%");
    }

    #[test]
    fn a_level_is_shown_as_a_whole_percentage() {
        assert_eq!(percent(0.), "0%");
        assert_eq!(percent(0.376), "38%");
        assert_eq!(percent(1.), "100%");
    }

    #[test]
    fn a_meter_is_read_as_often_as_it_says_or_the_default_lets_it() {
        let two = Duration::from_secs(2);
        let five = Duration::from_secs(5);
        assert_eq!(every(&Probe::Processor, None, two), two);
        assert_eq!(every(&Probe::Processor, Some(five), two), five);
        assert_eq!(every(&Probe::Battery, None, two), Duration::from_secs(20));
        assert_eq!(every(&Probe::Battery, Some(five), two), five);
        let disk = Probe::Disk("C:\\".into());
        assert_eq!(
            every(&disk, None, Duration::from_secs(45)),
            Duration::from_secs(45)
        );
    }

    #[test]
    fn an_interval_is_kept_within_a_second_and_a_minute() {
        assert_eq!(interval_of(0.2, "test"), Duration::from_secs(1));
        assert_eq!(interval_of(90., "test"), Duration::from_secs(60));
        assert_eq!(interval_of(1.5, "test"), Duration::from_millis(1500));
        assert_eq!(interval_of(f64::NAN, "test"), Duration::from_secs(2));
    }

    #[test]
    fn the_graphics_meter_shows_the_busiest_engine_or_the_memory() {
        let sample = Sample::Gpu(GpuUsage {
            busy: 0.314,
            used: 2 * 1_073_741_824,
            total: Some(8 * 1_073_741_824),
        });
        let load = GpuGauge { amount: false }.show(&sample).unwrap();
        assert_eq!((load.level, load.text.as_str()), (0.31, "31%"));
        let memory = GpuGauge { amount: true }.show(&sample).unwrap();
        assert_eq!((memory.level, memory.text.as_str()), (0.25, "2.0 GB"));
        assert!(
            GpuGauge { amount: true }
                .show(&Sample::Processor(0.5))
                .is_none()
        );
    }

    #[test]
    fn the_configuration_reads_the_interval() {
        let config: SystemConfig = toml::from_str("interval = 5").unwrap();
        assert_eq!(config.interval, 5.);
        assert_eq!(
            toml::from_str::<SystemConfig>("").unwrap(),
            SystemConfig::default()
        );
        assert!(toml::from_str::<SystemConfig>("every = 5").is_err());
    }
}
