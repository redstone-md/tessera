// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A wall clock that repaints once per minute. Widget: `clock`.

use std::time::Duration;

use chrono::{Local, Timelike};
use gpui::{
    AppContext, AsyncApp, Context, FontWeight, IntoElement, ParentElement, Render, Styled,
    WeakEntity, Window,
};

use crate::module::{Module, WidgetRegistry};
use crate::theme::Theme;
use crate::ui::pill;

pub struct ClockModule;

impl Module for ClockModule {
    fn name(&self) -> &'static str {
        "clock"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("clock", |_, cx| cx.new(Clock::new).into())
    }
}

/// Displays the local date and time to minute precision.
struct Clock;

impl Clock {
    fn new(cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor()
                    .timer(Self::until_next_minute())
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self
    }

    /// Time left until the displayed minute changes.
    fn until_next_minute() -> Duration {
        let now = Local::now();
        let elapsed = u64::from(now.second()) * 1_000 + u64::from(now.timestamp_subsec_millis());
        Duration::from_millis(60_000u64.saturating_sub(elapsed).max(50))
    }
}

impl Render for Clock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        pill(cx.global::<Theme>())
            .font_weight(FontWeight::MEDIUM)
            .child(Local::now().format("%a %-d %b  %H:%M").to_string())
    }
}
