// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A wall clock that repaints once per minute. Widget: `clock`.

use std::time::Duration;

use chrono::format::{Item, StrftimeItems};
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
        widgets.register("clock", |context, cx| {
            let format = context
                .text("format")
                .filter(|format| {
                    let valid = is_valid_format(format);
                    if !valid {
                        eprintln!("tessera: `{format}` is not a clock format, the default is used");
                    }
                    valid
                })
                .unwrap_or(DEFAULT_FORMAT)
                .to_owned();
            cx.new(|cx| Clock::new(format, cx)).into()
        })
    }
}

const DEFAULT_FORMAT: &str = "%a %-d %b  %H:%M";

/// Whether `format` is a `strftime` pattern that can be displayed.
fn is_valid_format(format: &str) -> bool {
    !StrftimeItems::new(format).any(|item| item == Item::Error)
}

/// Displays the local date and time to minute precision, in the `strftime`
/// pattern the `format` option gives.
struct Clock {
    format: String,
}

impl Clock {
    fn new(format: String, cx: &mut Context<Self>) -> Self {
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
        Self { format }
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
            .child(Local::now().format(&self.format).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_unknown_specifiers_are_rejected() {
        assert!(is_valid_format(DEFAULT_FORMAT));
        assert!(is_valid_format("%H:%M:%S"));
        assert!(!is_valid_format("%Q"));
    }
}
