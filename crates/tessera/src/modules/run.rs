// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Run dialog, as Win+R opens it in Windows: a field that runs a
//! program, opens a folder, a document or an address, with the history
//! of the Run dialog of Explorer (`RunMRU`), which it reads and writes the
//! same way, completions of paths, Browse, and running as an
//! administrator with Ctrl+Shift+Enter. What it runs is understood as
//! crate::runners::Command understands it: `%VARIABLES%`, App Paths and
//! `PATH`, `shell:` places, `::{CLSID}` and `ms-settings:` pages.
//! Widget: `run`, popup `run`, action `run`, which takes Win+R from
//! Windows.

use std::path::Path;
use std::time::Duration;

use gpui::{
    App, AppContext, AsyncApp, Context, Entity, FontWeight, Global, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, PathPromptOptions, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, WeakEntity, Window, div, px,
};

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::field::TextField;
use crate::files::Files;
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::platform::RunHistory;
use crate::popup::Popups;
use crate::runners::{Command, History, completions, fill};
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::{icon, one_line};

/// The name of the popup and of the action that opens it.
const POPUP: &str = "run";

/// The size of the popup.
const WIDTH: f32 = 480.;
const HEIGHT: f32 = 400.;

const ICON_RUN: &str = "\u{e756}";
const ICON_HISTORY: &str = "\u{e81c}";
const ICON_FOLDER: &str = "\u{e8b7}";

/// How many suggestions are shown at the most, and how high each is.
const SUGGESTIONS: usize = 8;
const SUGGESTION_HEIGHT: Pixels = px(32.);

/// How long Browse waits for the popup to have gone before it shows the
/// dialog of Windows, which would otherwise belong to the popup and close
/// with it.
const BROWSE_AFTER: Duration = Duration::from_millis(350);

pub struct RunModule;

impl Module for RunModule {
    fn name(&self) -> &'static str {
        "run"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("run", |_, cx| cx.new(RunDialog::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: PopupPlace::Surface,
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: "run".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let open = Action::new(|_, cx| Shell::toggle_popup(POPUP, cx))
            .taking_reserved_keys()
            .by_default("win+r", None);
        actions.register("run", open)
    }
}

/// What the Run dialog shows when it opens next: the file chosen with
/// Browse, or what was typed before it.
#[derive(Default)]
struct Prefill(Option<String>);

impl Global for Prefill {}

/// `path` as the Run dialog writes it: in quotes where it has a space.
fn quoted(path: &Path) -> String {
    let path = path.to_string_lossy();
    match path.contains(char::is_whitespace) {
        true => format!("\"{path}\""),
        false => path.into_owned(),
    }
}

/// The commands of `history` that start with `typed` but are not just
/// that, whatever the case, the newest first, and then `completed`, at
/// most [`SUGGESTIONS`] of them, none twice.
fn suggested(history: &[String], typed: &str, completed: Vec<String>) -> Vec<String> {
    let lower = typed.to_lowercase();
    let mut suggestions: Vec<String> = Vec::new();
    let from_history = { history.iter() }
        .filter(|command| command.to_lowercase().starts_with(&lower))
        .cloned();
    for suggestion in from_history.chain(completed) {
        let lowered = suggestion.to_lowercase();
        let known = suggestions
            .iter()
            .any(|known| known.to_lowercase() == lowered);
        if lowered != lower && !known {
            suggestions.push(suggestion);
        }
    }
    suggestions.truncate(SUGGESTIONS);
    suggestions
}

/// Why something did not run: what is said of it, and what Windows said.
struct Failure {
    said: String,
    error: String,
}

/// The Run dialog.
struct RunDialog {
    field: Entity<TextField>,
    /// The commands run before, the newest first.
    history: Vec<String>,
    /// What the list under the field offers, and which of it is chosen
    /// with the arrow keys.
    suggestions: Vec<String>,
    chosen: Option<usize>,
    /// What was typed before a suggestion was chosen, which Up past the
    /// first one brings back.
    typed: String,
    /// What the field said when the suggestions were made.
    written: String,
    /// The completions being read, and the command being started.
    completing: Option<Task<()>>,
    running: Option<Task<()>>,
    failure: Option<Failure>,
    /// Whether the keys were given to the field yet.
    focused: bool,
}

impl RunDialog {
    fn new(cx: &mut Context<Self>) -> Self {
        let language = Language::of(cx);
        let placeholder = language.say("A program, a folder, a document or an address");
        let field = cx.new(|cx| TextField::new(SharedString::new(placeholder), cx));
        cx.observe(&field, |this, _, cx| this.changed(cx)).detach();
        History::read(cx);
        let prefill = cx
            .try_global::<Prefill>()
            .and_then(|prefill| prefill.0.clone());
        if prefill.is_some() {
            cx.set_global(Prefill(None));
        }
        let prefilled = prefill.is_some();
        if let Some(text) = prefill {
            field.update(cx, |field, cx| field.put_text(&text, cx));
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let history = cx.background_executor().spawn(async { RunHistory::read() });
            let history = history.await;
            this.update(cx, |this, cx| this.take_history(history, prefilled, cx))
                .ok();
        })
        .detach();
        Self {
            field,
            history: Vec::new(),
            suggestions: Vec::new(),
            chosen: None,
            typed: String::new(),
            written: String::new(),
            completing: None,
            running: None,
            failure: None,
            focused: false,
        }
    }

    /// Has `history` be the commands run before; the field shows the
    /// newest, all of it selected to be typed over, as in Windows, unless
    /// something is in it already.
    fn take_history(&mut self, history: Vec<String>, prefilled: bool, cx: &mut Context<Self>) {
        self.history = history;
        let newest = self.history.first().cloned();
        match newest {
            Some(newest) if !prefilled && self.text(cx).is_empty() => {
                self.written = newest.clone();
                self.typed = newest.clone();
                self.field
                    .update(cx, |field, cx| field.set_text(&newest, false, cx));
                self.suggest(cx);
            }
            _ => self.suggest(cx),
        }
    }

    fn text(&self, cx: &App) -> String {
        self.field.read(cx).text()
    }

    /// The field says something else: the suggestions start over.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text == self.written {
            return;
        }
        self.written = text.clone();
        self.typed = text;
        self.chosen = None;
        self.failure = None;
        self.suggest(cx);
    }

    /// Offers the commands of the history that start like what is typed
    /// at once, and the entries of the folder it is typed into once they
    /// are read.
    fn suggest(&mut self, cx: &mut Context<Self>) {
        let typed = self.typed.clone();
        self.suggestions = suggested(&self.history, &typed, Vec::new());
        cx.notify();
        if !typed.contains(['\\', '/']) {
            self.completing = None;
            return;
        }
        let completing = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let completed = cx.background_executor().spawn({
                let typed = typed.clone();
                async move { completions(&typed, SUGGESTIONS) }
            });
            let completed = completed.await;
            this.update(cx, |this, cx| {
                if this.typed == typed {
                    this.suggestions = suggested(&this.history, &typed, completed);
                    this.chosen = None;
                    cx.notify();
                }
            })
            .ok();
        });
        self.completing = Some(completing);
    }

    /// Puts `text` in the field as a suggestion is chosen, without making
    /// new suggestions for it.
    fn show_chosen(&mut self, text: &str, cx: &mut Context<Self>) {
        self.written = text.to_owned();
        self.field.update(cx, |field, cx| field.put_text(text, cx));
        cx.notify();
    }

    /// Chooses the suggestion `steps` away from the one chosen; before the
    /// first one is what was typed.
    fn step(&mut self, steps: isize, cx: &mut Context<Self>) {
        if self.suggestions.is_empty() {
            return;
        }
        let at = self.chosen.map_or(-1, |chosen| chosen as isize) + steps;
        let last = self.suggestions.len() as isize - 1;
        self.chosen = (at >= 0).then_some(at.min(last) as usize);
        let text = match self.chosen {
            Some(chosen) => self.suggestions[chosen].clone(),
            None => self.typed.clone(),
        };
        self.show_chosen(&text, cx);
    }

    /// Takes the suggestion `index` as if it was typed, which completes a
    /// folder with what is in it.
    fn take(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.suggestions.get(index).cloned() else {
            return;
        };
        self.field.update(cx, |field, cx| field.put_text(&text, cx));
        self.field.read(cx).focus(window);
    }

    /// Runs what the field says, as an administrator with `admin`. The
    /// dialog closes once it has started; what fails is said under the
    /// field and the dialog stays.
    fn run(&mut self, admin: bool, cx: &mut Context<Self>) {
        let typed = self.text(cx).trim().to_owned();
        if typed.is_empty() || self.running.is_some() {
            return;
        }
        self.failure = None;
        cx.notify();
        let running = cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let understood =
                cx.background_executor().spawn({
                    let typed = typed.clone();
                    async move {
                        Command::understand(&typed).unwrap_or_else(|| Command::as_typed(&typed))
                    }
                });
            let command = understood.await;
            // A folder opens in the file manager of Tessera, where it is
            // the one that opens folders.
            let opened = command.folder
                && !admin
                && cx
                    .update(|cx| Files::open_folder(&command.path(), cx))
                    .unwrap_or(false);
            let outcome = match opened {
                true => Ok(()),
                false => {
                    // The Shell is not to show its own message for what
                    // is not found: the dialog says it.
                    let launch = match admin {
                        true => command.launch(true),
                        false => command.launch(false).quiet(),
                    };
                    match launch.started().recv().await {
                        Ok(Ok(_)) => Ok(()),
                        Ok(Err(error)) => Err(error),
                        Err(error) => Err(error.to_string()),
                    }
                }
            };
            this.update(cx, |this, cx| this.ran(&typed, &command, outcome, cx))
                .ok();
        });
        self.running = Some(running);
    }

    /// What running `typed` as `command` came to.
    fn ran(
        &mut self,
        typed: &str,
        command: &Command,
        outcome: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        self.running = None;
        match outcome {
            Ok(()) => {
                let typed = typed.to_owned();
                cx.background_executor()
                    .spawn(async move {
                        if let Err(error) = RunHistory::add(&typed) {
                            report!("{error:#}");
                        }
                    })
                    .detach();
                History::chose(&format!("command:{}", command.typed.to_lowercase()), cx);
                cx.defer(Popups::close);
            }
            Err(error) => {
                let said = Language::of(cx).say("Windows cannot open “{}”.");
                self.failure = Some(Failure {
                    said: fill(said, &[typed]),
                    error,
                });
                cx.notify();
            }
        }
    }

    /// Closes the dialog and shows that of Windows that chooses a file;
    /// the dialog opens again with the file chosen, or with what was
    /// typed where none is.
    fn browse(&mut self, cx: &mut Context<Self>) {
        let kept = self.text(cx);
        let prompt = Language::of(cx).say("Open");
        cx.defer(Popups::close);
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            cx.background_executor().timer(BROWSE_AFTER).await;
            let options = PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(SharedString::new_static(prompt)),
            };
            let Ok(chosen) = cx.update(|cx| cx.prompt_for_paths(options)) else {
                return;
            };
            let text = match chosen.await {
                Ok(Ok(Some(paths))) => paths.first().map_or(kept, |path| quoted(path)),
                Ok(Err(error)) => {
                    report!("no file can be chosen: {error:#}");
                    kept
                }
                _ => kept,
            };
            cx.update(|cx| {
                cx.set_global(Prefill(Some(text)));
                Shell::toggle_popup(POPUP, cx);
            })
            .ok();
        })
        .detach();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke.modifiers;
        match event.keystroke.key.as_str() {
            "escape" => cx.defer(Popups::close),
            "enter" => self.run(keys.control && keys.shift, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "tab" if !self.suggestions.is_empty() => {
                self.take(self.chosen.unwrap_or(0), window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn render_suggestions(&self, theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self
            .suggestions
            .iter()
            .enumerate()
            .map(|(index, suggestion)| {
                let chosen = self.chosen == Some(index);
                let glyph = match suggestion.ends_with(['\\', '/']) {
                    true => ICON_FOLDER,
                    false => ICON_HISTORY,
                };
                let background = match chosen {
                    true => theme.colors.secondary_container,
                    false => gpui::transparent_black(),
                };
                div()
                    .id(("run-suggestion", index))
                    .flex_none()
                    .h(SUGGESTION_HEIGHT)
                    .px(theme.spacing.medium)
                    .flex()
                    .items_center()
                    .gap(theme.spacing.small)
                    .rounded(theme.shape.medium)
                    .bg(background)
                    .hover(|style| style.bg(theme.colors.surface_container_highest))
                    .child(icon(theme, glyph).text_color(theme.colors.on_surface_variant))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(one_line(SharedString::new(suggestion.as_str()))),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.take(index, window, cx)))
            });
        div()
            .id("run-suggestions")
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .children(rows)
    }

    fn render_failure(&self, theme: &Theme) -> Option<impl IntoElement> {
        let failure = self.failure.as_ref()?;
        Some(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_color(theme.colors.primary)
                        .child(SharedString::new(failure.said.as_str())),
                )
                .child(
                    div()
                        .text_size(theme.typography.label_size)
                        .text_color(theme.colors.on_surface_variant)
                        .child(SharedString::new(failure.error.as_str())),
                ),
        )
    }
}

/// A button of the dialog; `main` is the one Enter presses.
fn button(
    id: &'static str,
    label: &'static str,
    main: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let background = match main {
        true => theme.colors.secondary_container,
        false => theme.colors.surface_container_highest,
    };
    div()
        .id(id)
        .flex_none()
        .px(theme.spacing.medium)
        .py(theme.spacing.small)
        .rounded(theme.shape.full)
        .bg(background)
        .text_color(theme.colors.on_surface)
        .text_size(theme.typography.label_size)
        .hover(|style| style.opacity(0.85))
        .child(label)
}

impl Render for RunDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let language = Language::of(cx);
        if !self.focused {
            self.focused = true;
            self.field.read(cx).focus(window);
        }
        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .text_color(theme.colors.on_surface)
            .font_weight(FontWeight::MEDIUM)
            .child(icon(&theme, ICON_RUN).text_color(theme.colors.primary))
            .child(language.say("Run a command"));
        let hint = div()
            .flex_none()
            .text_size(theme.typography.label_size)
            .text_color(theme.colors.on_surface_variant)
            .child(language.say(
                "Type the name of a program, a folder, a document or an address to open it. Ctrl+Shift+Enter runs it as an administrator.",
            ));
        let busy = self.running.is_some();
        let buttons = div()
            .flex_none()
            .flex()
            .justify_end()
            .gap(theme.spacing.small)
            .child(
                button("run-admin", language.say("As administrator"), false, &theme)
                    .on_click(cx.listener(|this, _, _, cx| this.run(true, cx))),
            )
            .child(
                button(
                    "run-browse",
                    language.say("Browse for a file…"),
                    false,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
            )
            .child(
                button("run-cancel", language.say("Cancel"), false, &theme)
                    .on_click(cx.listener(|_, _, _, cx| cx.defer(Popups::close))),
            )
            .child(
                button("run-open", language.say("Open"), true, &theme)
                    .opacity(if busy { 0.6 } else { 1. })
                    .on_click(cx.listener(|this, _, _, cx| this.run(false, cx))),
            );
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .child(header)
            .child(hint)
            .child(div().flex_none().flex().child(self.field.clone()))
            .child(self.render_suggestions(&theme, cx))
            .children(self.render_failure(&theme))
            .child(buttons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_is_offered_before_the_completions() {
        let history = [
            "notepad".to_owned(),
            "cmd".to_owned(),
            "Notepad todo.txt".to_owned(),
        ];
        let completed = vec![r"C:\Users\".to_owned(), "notepad todo.txt".to_owned()];
        assert_eq!(
            suggested(&history, "note", completed),
            ["notepad", "Notepad todo.txt", r"C:\Users\"]
        );
        // What is typed already is not offered again.
        assert_eq!(suggested(&history, "CMD", Vec::new()), Vec::<String>::new());
        assert_eq!(suggested(&history, "", Vec::new()).len(), 3);
    }

    #[test]
    fn a_path_with_a_space_is_quoted() {
        assert_eq!(
            quoted(Path::new(r"C:\Windows\notepad.exe")),
            r"C:\Windows\notepad.exe"
        );
        assert_eq!(
            quoted(Path::new(r"C:\Program Files\App\app.exe")),
            r#""C:\Program Files\App\app.exe""#
        );
    }
}
