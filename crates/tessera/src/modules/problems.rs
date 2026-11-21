// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What went wrong while the shell ran. Widgets: `problems`, a count that
//! shows only while there is something to tell, and `problem-list`, the
//! reports themselves, for a popup.

use std::time::Duration;

use gpui::{
    AnyElement, AppContext, AsyncApp, Context, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window,
    div, px,
};

use crate::config::{PopupConfig, WidgetSpec};
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring, staggered};
use crate::report::{self, Report};
use crate::theme::Theme;
use crate::ui::{icon, pill, state_layer};

/// How often it is checked whether something new was reported. The check
/// compares two numbers.
const CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// How far below its place a row starts when the list arrives.
const ROW_TRAVEL: f32 = 10.;

/// The popup a click on `problems` opens when the layout names none.
const POPUP: &str = "problems";

const ICON_WARNING: &str = "\u{e7ba}";

pub struct ProblemsModule;

impl Module for ProblemsModule {
    fn name(&self) -> &'static str {
        "problems"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("problems", |_, cx| cx.new(Indicator::new).into())?;
        widgets.register("problem-list", |_, cx| cx.new(List::new).into())?;
        widgets.register_popup(PopupConfig {
            place: Default::default(),
            name: POPUP.to_owned(),
            width: 380.,
            height: 320.,
            side: Vec::new(),
            content: vec![WidgetSpec {
                widget: "problem-list".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
        })?;
        widgets.open_with("problems", POPUP);
        Ok(())
    }
}

/// Calls `changed` on `this` whenever something was reported or forgotten,
/// for as long as `this` lives. Call it before the first reading, so that
/// nothing reported in between is missed.
fn follow<V: 'static>(cx: &mut Context<V>, changed: fn(&mut V, &mut Context<V>)) {
    let mut seen = report::changes();
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(CHECK_INTERVAL).await;
            let now = report::changes();
            if now == seen {
                if this.upgrade().is_none() {
                    break;
                }
                continue;
            }
            seen = now;
            if this.update(cx, changed).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// How many things went wrong, counting repeats.
fn total(reports: &[Report]) -> u32 {
    let times = reports.iter().map(|report| report.times);
    times.fold(0, u32::saturating_add)
}

/// The count in the bar; nothing while nothing was reported.
struct Indicator {
    count: u32,
    /// 1 while there is something to tell.
    presence: Spring,
}

impl Indicator {
    fn new(cx: &mut Context<Self>) -> Self {
        follow(cx, Self::read);
        let count = total(&report::reports());
        let mut presence = Spring::new(cx.global::<Theme>().motion.spatial, 0.);
        presence.animate_to(if count > 0 { 1. } else { 0. });
        Self { count, presence }
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let count = total(&report::reports());
        if count > self.count {
            // Each new report shows.
            self.presence.kick(4.);
        }
        if count > 0 {
            self.count = count;
        }
        self.presence.animate_to(if count > 0 { 1. } else { 0. });
        cx.notify();
    }
}

impl Render for Indicator {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let presence = self.presence.value(window);
        if self.presence.target() == 0. && self.presence.is_settled() {
            self.count = 0;
            return div();
        }
        pill(&theme)
            .relative()
            .top(px(ROW_TRAVEL * (1. - presence)))
            .opacity(presence.clamp(0., 1.))
            .font_weight(FontWeight::MEDIUM)
            .child(icon(&theme, ICON_WARNING).text_color(theme.colors.primary))
            .child(SharedString::from(self.count.to_string()))
    }
}

/// The reports, the latest first.
struct List {
    reports: Vec<Report>,
    arrival: Spring,
    clear: Interaction,
}

impl List {
    fn new(cx: &mut Context<Self>) -> Self {
        follow(cx, Self::read);
        let motion = cx.global::<Theme>().motion;
        let mut arrival = Spring::new(motion.spatial, 0.);
        arrival.animate_to(1.);
        Self {
            reports: latest_first(),
            arrival,
            clear: Interaction::new(&motion),
        }
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let reports = latest_first();
        if reports.is_empty() != self.reports.is_empty() {
            self.arrival.jump_to(0.);
            self.arrival.animate_to(1.);
        }
        self.reports = reports;
        cx.notify();
    }

    fn row(&self, report: &Report, arrived: f32, theme: &Theme) -> AnyElement {
        let times = (report.times > 1).then(|| format!(", {} times", report.times));
        div()
            .relative()
            .top(px(ROW_TRAVEL * (1. - arrived)))
            .opacity(arrived)
            .flex_none()
            .px(theme.spacing.medium)
            .py(theme.spacing.small)
            .flex()
            .flex_col()
            .rounded(theme.shape.medium)
            .bg(theme.colors.surface_container_highest)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.colors.on_surface_variant)
                    .child(SharedString::from(format!(
                        "{}{}",
                        report.at,
                        times.unwrap_or_default()
                    ))),
            )
            .child(SharedString::from(report.text.clone()))
            .into_any_element()
    }
}

fn latest_first() -> Vec<Report> {
    let mut reports = report::reports();
    reports.reverse();
    reports
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let state = self.clear.state(window);
        let clear = div()
            .id("clear-problems")
            .px(theme.spacing.small + theme.spacing.tiny)
            .py(theme.spacing.tiny)
            .rounded(theme.shape.full)
            .text_color(theme.colors.primary)
            .bg(state_layer(&theme, theme.colors.surface_container, state))
            .on_click(cx.listener(|_, _, _, _| report::clear()))
            .child("Clear all");
        let clear = Interaction::track(clear, cx, |this: &mut Self| Some(&mut this.clear));
        let header = div()
            .h(theme.bar.pill_height)
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .text_color(theme.colors.on_surface)
            .child("Problems")
            .children((!self.reports.is_empty()).then_some(clear));
        let count = self.reports.len();
        let rows: Vec<AnyElement> = self
            .reports
            .iter()
            .enumerate()
            .map(|(index, report)| self.row(report, staggered(arrival, index, count), &theme))
            .collect();
        let nothing = rows.is_empty().then(|| {
            div()
                .relative()
                .top(px(ROW_TRAVEL * (1. - arrival)))
                .opacity(arrival)
                .text_color(theme.colors.on_surface_variant)
                .child("No problem is on record")
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.tiny)
            .child(header)
            .child(
                div()
                    .id("problem-list")
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.tiny)
                    .overflow_y_scroll()
                    .children(rows)
                    .children(nothing),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_count_towards_the_total() {
        let report = |times| Report {
            text: "it".to_owned(),
            at: "10:00".to_owned(),
            times,
        };
        assert_eq!(total(&[]), 0);
        assert_eq!(total(&[report(1), report(3)]), 4);
        assert_eq!(total(&[report(u32::MAX), report(3)]), u32::MAX);
    }
}
