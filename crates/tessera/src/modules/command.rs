// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The output of a program of the user's choice, run again and again.
//! Widget: `command`.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use gpui::{
    AppContext, AsyncApp, Context, FontWeight, IntoElement, ParentElement, Render, SharedString,
    Styled, WeakEntity, Window, div,
};

use crate::module::{Module, WidgetRegistry};
use crate::motion::Spring;
use crate::theme::Theme;
use crate::ui::{icon, pill};

/// The process creation flag that keeps a console program from opening a
/// console window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How long a run may take before it is stopped.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running program is checked for having finished.
const WAIT_STEP: Duration = Duration::from_millis(20);

/// The seconds between runs when the layout does not say.
const DEFAULT_INTERVAL: f64 = 10.;

/// The shortest time between runs, in seconds.
const SHORTEST_INTERVAL: f64 = 1.;

/// How much of the output is kept, in bytes; the rest is read and dropped.
const OUTPUT_LIMIT: usize = 4096;

/// The longest text shown, in characters.
const TEXT_LENGTH: usize = 64;

/// The glyph shown instead of the output of a run that failed.
const FAILURE_GLYPH: &str = "\u{E7BA}";

pub struct CommandModule;

impl Module for CommandModule {
    fn name(&self) -> &'static str {
        "command"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("command", |context, cx| {
            let command = context.texts("command").unwrap_or_default();
            if command.is_empty() {
                eprintln!(
                    "tessera: a `command` widget needs `command`, a program and its arguments"
                );
            }
            let interval = context
                .number("interval")
                .unwrap_or(DEFAULT_INTERVAL)
                .max(SHORTEST_INTERVAL);
            let icon = context.text("icon").map(SharedString::new);
            cx.new(|cx| Output::new(command, Duration::from_secs_f64(interval), icon, cx))
                .into()
        })
    }
}

/// Runs `command`, a program and its arguments, and returns the first
/// line it prints that is not blank.
fn run(command: &[String]) -> Result<String, String> {
    let (program, arguments) = command.split_first().ok_or("there is no program to run")?;
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| format!("`{program}` did not start: {error}"))?;
    // The output is read while the program runs, or a program that prints
    // more than the pipe holds would never finish.
    let mut pipe = child.stdout.take().ok_or("the output cannot be read")?;
    let reader = thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0; 1024];
        while let Ok(read @ 1..) = pipe.read(&mut chunk) {
            let room = OUTPUT_LIMIT.saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..read.min(room)]);
        }
        kept
    });

    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(WAIT_STEP),
            Ok(None) => {
                // Stopping can only fail if the program has just finished.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "`{program}` was stopped after {} seconds",
                    TIMEOUT.as_secs()
                ));
            }
            Err(error) => return Err(format!("`{program}` could not be waited for: {error}")),
        }
    };
    let output = reader.join().map_err(|_| "the output could not be read")?;
    if !status.success() {
        return Err(format!("`{program}` failed: {status}"));
    }
    Ok(first_line(&String::from_utf8_lossy(&output)))
}

/// The first line of `output` that is not blank, shortened to fit a bar.
fn first_line(output: &str) -> String {
    let line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.chars().count() <= TEXT_LENGTH {
        return line.to_owned();
    }
    let kept: String = line.chars().take(TEXT_LENGTH - 1).collect();
    format!("{}…", kept.trim_end())
}

/// Shows what a program printed the last time it ran.
struct Output {
    /// A character of the theme's icon font.
    icon: Option<SharedString>,
    /// What the last run printed, or why it failed; `None` before the
    /// first run has finished.
    result: Option<Result<SharedString, String>>,
    /// How far new text has faded in, from 0 to 1.
    fade: Spring,
}

impl Output {
    fn new(
        command: Vec<String>,
        interval: Duration,
        icon: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        if !command.is_empty() {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let command = command.clone();
                    let result = cx
                        .background_executor()
                        .spawn(async move { run(&command) })
                        .await;
                    if this.update(cx, |this, cx| this.take(result, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(interval).await;
                }
            })
            .detach();
        }
        Self {
            icon,
            result: None,
            fade: Spring::new(cx.global::<Theme>().motion.effects, 1.),
        }
    }

    fn take(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        let result = result.map(SharedString::from);
        if self.result.as_ref() == Some(&result) {
            return;
        }
        if let Err(error) = &result {
            eprintln!("tessera: a `command` widget shows no output: {error}");
        }
        self.result = Some(result);
        self.fade = Spring::new(cx.global::<Theme>().motion.effects, 0.);
        self.fade.animate_to(1.);
        cx.notify();
    }
}

impl Render for Output {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let fade = self.fade.value(window).clamp(0., 1.);
        let content = match &self.result {
            None => None,
            Some(Ok(text)) if text.is_empty() => None,
            Some(Ok(text)) => Some(div().child(text.clone())),
            Some(Err(_)) => Some(icon(&theme, FAILURE_GLYPH).text_color(theme.colors.outline)),
        };
        if self.icon.is_none() && content.is_none() {
            return div();
        }
        pill(&theme)
            .font_weight(FontWeight::MEDIUM)
            .children(
                self.icon
                    .clone()
                    .map(|glyph| icon(&theme, glyph).text_color(theme.colors.on_surface_variant)),
            )
            .children(content.map(|content| content.opacity(fade)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn the_first_line_that_is_not_blank_is_shown() {
        assert_eq!(first_line(""), "");
        assert_eq!(first_line("\r\n  \r\n 21 °C \r\nrain"), "21 °C");
        let long = "x".repeat(200);
        let shown = first_line(&long);
        assert_eq!(shown.chars().count(), TEXT_LENGTH);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn a_program_is_run_and_its_output_read() {
        assert_eq!(
            run(&command(&["cmd", "/C", "echo hello"])).unwrap(),
            "hello"
        );
    }

    #[test]
    fn a_program_that_fails_or_is_missing_is_an_error() {
        assert!(run(&command(&["cmd", "/C", "exit 3"])).is_err());
        assert!(run(&command(&["tessera-no-such-program"])).is_err());
        assert!(run(&[]).is_err());
    }
}
