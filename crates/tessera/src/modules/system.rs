// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How busy the machine is and how much charge it has left.
//! Widgets: `cpu`, `memory`, `battery`.

use std::time::Duration;

use gpui::{
    AppContext, AsyncApp, Context, Div, FontWeight, IntoElement, ParentElement, Render,
    SharedString, Styled, WeakEntity, Window, div,
};

use crate::module::{Module, WidgetContext, WidgetRegistry};
use crate::motion::Spring;
use crate::platform::{Battery, MemoryLoad, ProcessorLoad};
use crate::theme::Theme;
use crate::ui::{icon, pill, wave_meter};

/// How wide the value is, in label sizes: room for "100%", so that the
/// pill keeps its width while the value changes.
const VALUE_WIDTH: f32 = 2.4;

/// The same for a value in gigabytes, such as "31.4 GB".
const AMOUNT_WIDTH: f32 = 3.8;

const BYTES_PER_GIGABYTE: f64 = 1_073_741_824.;

pub struct SystemModule;

impl Module for SystemModule {
    fn name(&self) -> &'static str {
        "system"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("cpu", |context, cx| {
            let gauge = ProcessorGauge(ProcessorLoad::default());
            cx.new(|cx| Meter::new(Box::new(gauge), context, cx)).into()
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
    /// How long a reading stays good enough.
    fn interval(&self) -> Duration;

    /// The state now; `None` when there is nothing to show.
    fn read(&mut self) -> Option<Reading>;
}

struct ProcessorGauge(ProcessorLoad);

impl Gauge for ProcessorGauge {
    fn interval(&self) -> Duration {
        Duration::from_secs(2)
    }

    fn read(&mut self) -> Option<Reading> {
        let level = self.0.read()?;
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
    fn interval(&self) -> Duration {
        Duration::from_secs(2)
    }

    fn read(&mut self) -> Option<Reading> {
        let memory = MemoryLoad::read()?;
        let level = (memory.used as f64 / memory.total.max(1) as f64) as f32;
        Some(Reading {
            level,
            glyph: "\u{E964}",
            text: if self.amount {
                format!("{:.1} GB", memory.used as f64 / BYTES_PER_GIGABYTE)
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
    fn interval(&self) -> Duration {
        Duration::from_secs(20)
    }

    fn read(&mut self) -> Option<Reading> {
        let battery = Battery::read()?;
        Some(Reading {
            level: f32::from(battery.percent) / 100.,
            glyph: battery_glyph(battery),
            text: format!("{}%", battery.percent),
            text_width: VALUE_WIDTH,
        })
    }
}

fn percent(level: f32) -> String {
    format!("{:.0}%", level * 100.)
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
    reading: Option<Reading>,
    /// The level drawn, which springs after the level read.
    level: Spring,
    style: Style,
}

impl Meter {
    fn new(mut gauge: Box<dyn Gauge>, context: &WidgetContext, cx: &mut Context<Self>) -> Self {
        let interval = gauge.interval();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(interval).await;
                if this.update(cx, Self::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();
        let reading = gauge.read();
        let mut level = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        level.animate_to(reading.as_ref().map_or(0., |reading| reading.level));
        Self {
            gauge,
            reading,
            level,
            style: match one_of(context, "style", &["wave", "text"]) {
                0 => Style::Wave,
                _ => Style::Text,
            },
        }
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        // A gauge that has nothing new keeps what it showed.
        let Some(reading) = self.gauge.read() else {
            return;
        };
        if self.reading.as_ref() == Some(&reading) {
            return;
        }
        let change = (reading.level - self.level.target()).abs();
        if change < SMALL_CHANGE && self.level.is_settled() {
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
    fn a_level_is_shown_as_a_whole_percentage() {
        assert_eq!(percent(0.), "0%");
        assert_eq!(percent(0.376), "38%");
        assert_eq!(percent(1.), "100%");
    }
}
