// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Tessera as the shell of the session: making it the shell and taking
//! that back (`tessera shell install|revert|status`), what it does before
//! anything else when Winlogon starts it, what it answers to Windows
//! while it runs, and how it gives the session to Explorer.
//!
//! Before GPUI starts, `main` asks this module, in order:
//! - [`hand_over_if_requested`] and [`run_command_if_requested`]: a
//!   process started only to start Explorer, or to run `tessera shell`;
//! - [`detect`] and [`guard`]: whether this is the shell, and whether it
//!   must give the session to Explorer instead of starting, because
//!   Windows is in Safe Mode, Ctrl+Shift are held, or Tessera started as
//!   the shell three times within a minute; the start is written down
//!   first, so that a start that fails anywhere later counts;
//! - [`adjust`], which makes the workspaces Tessera's own in shell mode,
//!   as Windows has no virtual desktops without Explorer;
//! - [`start_host`], which starts the thread that answers Windows
//!   ([`ShellHost`]).
//!
//! Once GPUI runs, `main` calls [`app_started`] before the surfaces are
//! opened, so that a start that fails still quits as below. Once the
//! surfaces are open, [`Shell::start`] calls [`surfaces_ready`], which
//! tells Windows the shell is ready and listens to the events of the
//! host. A Tessera that quits or panics as the shell of the session,
//! other than at the end of the session or to start again, starts
//! Explorer as the shell after it is gone, so the session is not left
//! without one.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AppContext, AsyncApp, Context, Global, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use serde::Deserialize;

use crate::actions::{Action, Actions};
use crate::config::{Config, PopupConfig, WidgetSpec};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::motion::{Interaction, Spring};
use crate::platform::{
    ExplorerShell, HostOptions, NextShell, SHELL_FLAG, ShellEvent, ShellHost, ShellRegistration,
    Taskbar, attach_console, is_safe_mode, own_program_name, recovery_keys_held, shell_mode,
    started_as_shell,
};
use crate::popup::Popups;
use crate::shell::Shell;
use crate::theme::Theme;
use crate::ui::state_layer;
use crate::workspaces::Workspaces;

/// How many starts as the shell within [`CRASH_WINDOW`] give the session
/// to Explorer.
const CRASH_LIMIT: usize = 3;

/// The time within which [`CRASH_LIMIT`] starts count as a crash loop.
const CRASH_WINDOW: Duration = Duration::from_secs(60);

/// The file of the starts as the shell, in the local data of Tessera.
const STARTS_FILE: &str = "shell-starts.txt";

/// The file that tells the next Tessera why the session went to Explorer.
const FALLBACK_FILE: &str = "shell-fallback.txt";

/// The flag of a start that takes over from another Tessera.
const SUCCESSOR_FLAG: &str = "--start-after";

/// How many workspaces of its own Tessera keeps as the shell where the
/// configuration names no number.
const SHELL_WORKSPACES: usize = 4;

/// The action that asks how to end the session, which another part of
/// Tessera may register.
const POWER_DIALOG: &str = "power-dialog";

/// The widget with the buttons that end the session, whose popup Alt+F4
/// on the desktop opens while there is no [`POWER_DIALOG`].
const POWER_WIDGET: &str = "power";

/// The popup that holds the `shell` widget.
const POPUP: &str = "shell";

/// What the configuration says of Tessera as the shell, `[shell]`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShellConfig {
    /// Whether Ctrl+Win+Alt+E (Task Manager and Explorer) and
    /// Ctrl+Win+Alt+R (Tessera again) are held as the shell.
    pub emergency_keys: bool,
    /// Whether a Tessera that quits as the shell starts Explorer as the
    /// shell, so that the session keeps one.
    pub quit_to_explorer: bool,
    /// Whether the keys of the volume and of media are answered as the
    /// shell, as Explorer does.
    pub media_keys: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            emergency_keys: true,
            quit_to_explorer: true,
            media_keys: true,
        }
    }
}

impl ShellConfig {
    fn options(&self) -> HostOptions {
        HostOptions {
            shell: shell_mode(),
            emergency_keys: self.emergency_keys,
            media_keys: self.media_keys,
        }
    }
}

/// The `[shell]` in use, for the panel.
struct ShellSettings(ShellConfig);

impl Global for ShellSettings {}

/// `quit_to_explorer` of the `[shell]` in use, where a panic can read it.
static QUIT_TO_EXPLORER: AtomicBool = AtomicBool::new(true);

/// What `tessera shell` is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellCommand {
    Install,
    Revert,
    Status,
}

impl ShellCommand {
    /// What the words of the command line ask for: `None` where they are
    /// not `shell ...`, and why not where the word after it is unknown.
    fn of(words: &[String]) -> Option<Result<Self, String>> {
        let (first, rest) = words.split_first()?;
        if first != "shell" {
            return None;
        }
        Some(match rest {
            [word] if word == "install" => Ok(Self::Install),
            [word] if word == "revert" => Ok(Self::Revert),
            [] => Ok(Self::Status),
            [word] if word == "status" => Ok(Self::Status),
            _ => Err("`tessera shell` takes `install`, `revert` or `status`".to_owned()),
        })
    }

    /// Does it and prints what came of it; gives the exit code.
    fn run(self) -> i32 {
        let program = own_program_name();
        let done = match self {
            Self::Install => ShellRegistration::install().inspect(|_| {
                println!("Tessera is written as the user's shell; it starts at the next sign-in.");
            }),
            Self::Revert => ShellRegistration::revert().inspect(|_| {
                println!(
                    "The user's shell is given back; Windows starts what the machine names at the next sign-in."
                );
            }),
            Self::Status => Ok(ShellRegistration::read()),
        };
        match done {
            Ok(registration) => {
                let now = if shell_mode() {
                    "Tessera is the shell of this session."
                } else {
                    "Tessera is not the shell of this session."
                };
                println!("{now}");
                for line in registration.describe(&program) {
                    println!("{line}");
                }
                0
            }
            Err(error) => {
                eprintln!("tessera: {error:#}");
                1
            }
        }
    }
}

/// Runs `tessera shell install|revert|status` if the arguments ask for it,
/// and gives the exit code; the caller exits then.
pub fn run_command_if_requested(arguments: impl Iterator<Item = String>) -> Option<i32> {
    let words: Vec<String> = arguments.collect();
    let command = ShellCommand::of(&words)?;
    attach_console();
    Some(match command {
        Ok(command) => command.run(),
        Err(why) => {
            eprintln!("tessera: {why}");
            2
        }
    })
}

/// Is the short process that starts Explorer as the shell once Tessera
/// is gone, if the arguments ask for it; says whether they did. The
/// windows the workspaces left hidden are shown first.
pub fn hand_over_if_requested(arguments: impl Iterator<Item = String>) -> bool {
    ExplorerShell::run_if_requested(arguments, Workspaces::recover)
}

/// Notes whether Tessera was started as the shell, with `--shell`, before
/// anything asks whether it is the shell.
pub fn detect(arguments: impl Iterator<Item = String>) {
    if arguments.into_iter().any(|word| word == SHELL_FLAG) {
        started_as_shell();
    }
}

/// What Tessera does before anything else as the shell: gives the session
/// to Explorer in Safe Mode, while Ctrl+Shift are held, and after
/// [`CRASH_LIMIT`] starts within [`CRASH_WINDOW`]. Says whether Tessera
/// goes on starting; the caller exits otherwise.
pub fn guard(arguments: impl Iterator<Item = String>) -> bool {
    tell_of_fallback();
    if !shell_mode() {
        return true;
    }
    if is_safe_mode() || recovery_keys_held() {
        if let Err(error) = ExplorerShell::start_now() {
            report!("Explorer was not started as the shell: {error:#}");
            return true;
        }
        return false;
    }
    // A start that takes over from another Tessera is no crash.
    if arguments.into_iter().any(|word| word == SUCCESSOR_FLAG) {
        return true;
    }
    if !record_start() {
        return true;
    }
    report!("Tessera started as the shell {CRASH_LIMIT} times within a minute");
    if let Err(error) = ShellRegistration::revert() {
        report!("Tessera stays the shell: {error:#}");
    }
    Workspaces::recover();
    let explorer = ExplorerShell::start_now();
    if let Err(error) = &explorer {
        report!("Explorer was not started as the shell: {error:#}");
    }
    if let Some(path) = local_file(FALLBACK_FILE) {
        let _ = fs::write(path, now_seconds().to_string());
    }
    // Without Explorer, Tessera goes on, which is better than nothing.
    explorer.is_err()
}

/// Has Tessera keep workspaces of its own as the shell where the
/// configuration names no number: the virtual desktops of Windows are
/// part of Explorer.
pub fn adjust(config: &mut Config) {
    if shell_mode() {
        config.workspaces = config.workspaces.owning(SHELL_WORKSPACES);
    }
}

/// Starts the thread that answers Windows, with the `[shell]` of the
/// configuration. The host shows the windows the workspaces hid by
/// itself at the end of the session, and as the shell a panic of the
/// thread of GPUI starts Explorer as the shell.
pub fn start_host(config: &ShellConfig) {
    QUIT_TO_EXPLORER.store(config.quit_to_explorer, Ordering::SeqCst);
    ShellHost::on_session_end(Workspaces::recover);
    if shell_mode() {
        leave_on_panic();
    }
    ShellHost::start(config.options());
}

/// Applies a new `[shell]`.
pub fn reconfigure(config: &ShellConfig, cx: &mut gpui::App) {
    QUIT_TO_EXPLORER.store(config.quit_to_explorer, Ordering::SeqCst);
    ShellHost::configure(config.options());
    cx.set_global(ShellSettings(config.clone()));
}

/// What the shell does once GPUI runs, before it opens anything: decides
/// what happens to the session when it quits, also when the surfaces
/// then fail to open.
pub fn app_started(config: &ShellConfig, cx: &mut gpui::App) {
    cx.set_global(ShellSettings(config.clone()));
    cx.on_app_quit(|_| {
        quitting();
        // GPUI drops the windows, and with them the space the surfaces
        // held, after every callback and before it polls what they give:
        // only then is the end of the session told that Tessera is done.
        async { ShellHost::session_restored() }
    })
    .detach();
}

/// What the shell does once its surfaces are open: tells Windows it is
/// ready and answers the events of the host.
pub fn surfaces_ready(cx: &mut gpui::App) {
    let events = ShellHost::events();
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = events.recv().await {
            if cx.update(|cx| answer(event, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
    ShellHost::signal_ready();
}

/// Answers an event of the host.
fn answer(event: ShellEvent, cx: &mut gpui::App) {
    match event {
        ShellEvent::StartRequested => Shell::start_requested(cx),
        ShellEvent::CloseRequested => ask_how_to_end(cx),
        ShellEvent::QuitRequested | ShellEvent::SessionEnding => cx.quit(),
        _ => {}
    }
}

/// Answers Alt+F4 on the desktop: runs the action [`POWER_DIALOG`]
/// where one exists, and otherwise opens the popup that holds the
/// [`POWER_WIDGET`], as the launcher does by default.
fn ask_how_to_end(cx: &mut gpui::App) {
    let known =
        { cx.try_global::<Actions>() }.is_some_and(|actions| actions.get(POWER_DIALOG).is_some());
    if known {
        Actions::run(POWER_DIALOG, None, cx);
    } else if let Some(popup) = Popups::holding(POWER_WIDGET, cx) {
        Shell::toggle_popup(&popup, cx);
    } else {
        report!(
            "Alt+F4 on the desktop opens nothing: there is no action `{POWER_DIALOG}` and no popup holds the `{POWER_WIDGET}` widget"
        );
    }
}

/// What the shell does as it quits: starts Explorer as the shell where
/// it should, see [`leave_to_explorer`].
fn quitting() {
    if let Err(error) = leave_to_explorer() {
        report!("the session is left without a shell: {error:#}");
    }
}

/// Starts Explorer as the shell once this Tessera is gone, where Tessera
/// is the shell, the configuration asks for it, nothing has handed the
/// session over yet, and the session goes on without a new Tessera.
fn leave_to_explorer() -> anyhow::Result<()> {
    let goes_on = !ShellHost::session_ending() && !Taskbar::successor_started();
    if shell_mode() && QUIT_TO_EXPLORER.load(Ordering::SeqCst) && goes_on {
        ExplorerShell::hand_over()?;
    }
    Ok(())
}

/// Has a panic of the thread of GPUI, which ends Tessera without its
/// callbacks of quitting, start Explorer as the shell all the same, after
/// what the hook before did. Panics of other threads are caught where they
/// happen, so they do not.
fn leave_on_panic() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        before(info);
        if thread::current().name() == Some("main") {
            // Nothing can be reported from here: the report may be what
            // panicked.
            let _ = leave_to_explorer();
        }
    }));
}

/// Gives the session to Explorer now: Explorer starts as the shell once
/// this Tessera is gone, and it quits.
fn hand_to_explorer(cx: &mut gpui::App) {
    if !shell_mode() {
        report!("Explorer is the shell of this session already");
        return;
    }
    match ExplorerShell::hand_over() {
        Ok(true) => cx.quit(),
        Ok(false) => {}
        Err(error) => report!("the session was not handed to Explorer: {error:#}"),
    }
}

/// A file of the local data of Tessera, `%LOCALAPPDATA%\tessera`, which
/// is made if it is missing.
fn local_file(name: &str) -> Option<PathBuf> {
    let directory = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("tessera");
    fs::create_dir_all(&directory).ok()?;
    Some(directory.join(name))
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Writes this start down and says whether it makes [`CRASH_LIMIT`]
/// within [`CRASH_WINDOW`].
fn record_start() -> bool {
    let Some(path) = local_file(STARTS_FILE) else {
        return false;
    };
    let now = now_seconds();
    let record = fs::read_to_string(&path).unwrap_or_default();
    let mut starts = recent_starts(&record, now);
    starts.push(now);
    let looping = starts.len() >= CRASH_LIMIT;
    // After a fallback the count starts again.
    let kept: Vec<String> = if looping {
        Vec::new()
    } else {
        starts.iter().map(u64::to_string).collect()
    };
    if let Err(error) = fs::write(&path, kept.join("\n")) {
        report!("{} was not written: {error}", path.display());
    }
    looping
}

/// The starts of `record`, one number of seconds per line, that lie
/// within [`CRASH_WINDOW`] before `now`.
fn recent_starts(record: &str, now: u64) -> Vec<u64> {
    let window = CRASH_WINDOW.as_secs();
    record
        .lines()
        .filter_map(|line| line.trim().parse::<u64>().ok())
        .filter(|&start| start <= now && now - start < window)
        .collect()
}

/// Reports, once, that an earlier Tessera gave the session to Explorer
/// after it failed to start.
fn tell_of_fallback() {
    let Some(path) = local_file(FALLBACK_FILE) else {
        return;
    };
    if fs::remove_file(&path).is_ok() {
        report!(
            "Tessera started as the shell {CRASH_LIMIT} times within a minute, so it gave the session to Explorer and stopped being the shell; `tessera shell install` makes it the shell again"
        );
    }
}

/// Making Tessera the shell from the user interface. Widget: `shell`, in
/// the popup `shell`; action: `shell`.
pub struct ShellModule;

impl Module for ShellModule {
    fn name(&self) -> &'static str {
        "shell"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("shell", |_, cx| cx.new(ShellPanel::new).into())?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: Default::default(),
            width: 400.,
            height: 300.,
            content: vec![WidgetSpec {
                widget: "shell".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    /// `shell` with `install` makes Tessera the shell of the next
    /// sessions, with `revert` gives that back to what the machine names,
    /// and with `explorer` hands this session to Explorer now.
    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let action = Action::new(|arg, cx| match arg {
            Some("install") => change_registration(true, cx),
            Some("revert") => change_registration(false, cx),
            Some("explorer") => hand_to_explorer(cx),
            _ => {}
        })
        .checking(|arg| match arg {
            Some("install" | "revert" | "explorer") => Ok(()),
            _ => Err("`shell` takes `install`, `revert` or `explorer` as `arg`".to_owned()),
        });
        actions.register("shell", action)
    }
}

/// Writes or removes Tessera as the user's shell away from the user
/// interface; a failure is reported.
fn change_registration(install: bool, cx: &mut gpui::App) {
    cx.background_executor()
        .spawn(async move {
            let done = if install {
                ShellRegistration::install()
            } else {
                ShellRegistration::revert()
            };
            if let Err(error) = done {
                report!("the shell of the next sessions was not changed: {error:#}");
            }
        })
        .detach();
}

/// The panel that tells which shell runs and which starts next, and
/// changes the second.
struct ShellPanel {
    /// What the registry said when it was last read; `None` before.
    registration: Option<ShellRegistration>,
    /// Why the last change failed.
    failure: Option<String>,
    /// Whether a change is under way.
    busy: bool,
    change: Interaction,
    hand: Interaction,
    arrival: Spring,
}

impl ShellPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let motion = cx.global::<Theme>().motion;
        let mut arrival = Spring::new(motion.spatial, 0.);
        arrival.animate_to(1.);
        Self::read(cx);
        Self {
            registration: None,
            failure: None,
            busy: false,
            change: Interaction::new(&motion),
            hand: Interaction::new(&motion),
            arrival,
        }
    }

    /// Reads the registry away from the user interface.
    fn read(cx: &mut Context<Self>) {
        let read = cx
            .background_executor()
            .spawn(async { ShellRegistration::read() });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let registration = read.await;
            this.update(cx, |this, cx| {
                this.registration = Some(registration);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes Tessera the shell of the next sessions, or gives that back.
    fn toggle(&mut self, cx: &mut Context<Self>) {
        let Some(registration) = &self.registration else {
            return;
        };
        if self.busy {
            return;
        }
        let install = registration.next_shell(&own_program_name()) != NextShell::Tessera;
        self.busy = true;
        self.failure = None;
        cx.notify();
        let change = cx.background_executor().spawn(async move {
            if install {
                ShellRegistration::install()
            } else {
                ShellRegistration::revert()
            }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let changed = change.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match changed {
                    Ok(registration) => this.registration = Some(registration),
                    Err(error) => this.failure = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The lines that say what Windows starts next and what may stand in
    /// its way.
    fn lines(&self, language: Language) -> Vec<SharedString> {
        let Some(registration) = &self.registration else {
            return vec![language.say("Reading…").into()];
        };
        let mut lines: Vec<SharedString> = Vec::new();
        let next = registration.next_shell(&own_program_name());
        lines.push(match &next {
            NextShell::Tessera => language
                .say("At the next sign-in Windows starts Tessera.")
                .into(),
            NextShell::Explorer => language
                .say("At the next sign-in Windows starts Explorer.")
                .into(),
            NextShell::Other(named) => format!(
                "{} {named}",
                language.say("At the next sign-in Windows starts another program:")
            )
            .into(),
        });
        if registration.policy.is_some() {
            lines.push(
                language
                    .say("A policy of this computer names the shell; only an administrator can change it.")
                    .into(),
            );
        } else if next == NextShell::Tessera
            && registration.user.is_some()
            && registration.user_is_mapped() != Some(true)
        {
            lines.push(language.say("Windows is told to read the shell of the computer. It is widely reported to read the one of the user all the same; that is not certain on this computer.").into());
        }
        lines.push(
            language
                .say("A change takes effect at the next sign-in.")
                .into(),
        );
        lines
    }
}

impl Render for ShellPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let colors = theme.colors;
        let language = Language::of(cx);
        let arrival = self.arrival.value(window).clamp(0., 1.);
        let now = if shell_mode() {
            language.say("Tessera is the shell of this session.")
        } else {
            language.say("Tessera runs beside Explorer.")
        };
        let next = self
            .registration
            .as_ref()
            .map(|registration| registration.next_shell(&own_program_name()));
        let label = match next {
            Some(NextShell::Tessera) => language.say("Give the shell back to Explorer"),
            _ => language.say("Make Tessera the shell"),
        };
        let governed = self
            .registration
            .as_ref()
            .is_none_or(|registration| registration.policy.is_some());
        let button = |id: &'static str, label: &'static str, ground, state| {
            div()
                .id(id)
                .px(theme.spacing.medium)
                .py(theme.spacing.small)
                .rounded(theme.shape.full)
                .bg(state_layer(&theme, ground, state))
                .text_color(colors.on_surface)
                .child(label)
        };
        let change_state = self.change.state(window);
        let change = (!governed && !self.busy).then(|| {
            let change = button(
                "shell-change",
                label,
                colors.secondary_container,
                change_state,
            )
            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));
            Interaction::track(change, cx, |this: &mut Self| Some(&mut this.change))
        });
        let hand_state = self.hand.state(window);
        let hand = shell_mode().then(|| {
            let hand = button(
                "shell-hand",
                language.say("Hand this session to Explorer now"),
                colors.surface_container_highest,
                hand_state,
            )
            .on_click(|_, _, cx| hand_to_explorer(cx));
            Interaction::track(hand, cx, |this: &mut Self| Some(&mut this.hand))
        });
        let keys = { cx.try_global::<ShellSettings>() }
            .is_some_and(|settings| settings.0.emergency_keys)
            && shell_mode();
        let small = |text: SharedString| {
            div()
                .text_size(px(12.))
                .text_color(colors.on_surface_variant)
                .child(text)
        };
        div()
            .size_full()
            .relative()
            .top(px(8. * (1. - arrival)))
            .opacity(arrival)
            .flex()
            .flex_col()
            .gap(theme.spacing.small)
            .child(
                div()
                    .h(theme.bar.pill_height)
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_color(colors.on_surface)
                    .child(language.say("Shell of the session")),
            )
            .child(div().text_color(colors.on_surface).child(now))
            .children(self.lines(language).into_iter().map(small))
            .children(keys.then(|| small(language.say("Ctrl+Win+Alt+E starts Task Manager and hands the session to Explorer; Ctrl+Win+Alt+R starts Tessera again.").into())))
            .children(self.failure.clone().map(|failure| {
                div()
                    .text_size(px(12.))
                    .text_color(colors.primary)
                    .child(SharedString::from(failure))
            }))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(theme.spacing.small)
                    .children(change)
                    .children(hand),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn the_command_line_asks_for_a_shell_command() {
        assert_eq!(ShellCommand::of(&words("files C:\\")), None);
        assert_eq!(ShellCommand::of(&[]), None);
        assert_eq!(
            ShellCommand::of(&words("shell install")),
            Some(Ok(ShellCommand::Install))
        );
        assert_eq!(
            ShellCommand::of(&words("shell revert")),
            Some(Ok(ShellCommand::Revert))
        );
        assert_eq!(
            ShellCommand::of(&words("shell")),
            Some(Ok(ShellCommand::Status))
        );
        assert!(matches!(
            ShellCommand::of(&words("shell remove now")),
            Some(Err(_))
        ));
    }

    #[test]
    fn only_starts_within_a_minute_count() {
        let record = "100\n150\nnot a number\n\n171\n400";
        assert_eq!(recent_starts(record, 200), [150, 171]);
        assert!(recent_starts("", 200).is_empty());
        // A start written later than now, as after the clock was turned
        // back, does not count.
        assert!(recent_starts("500", 200).is_empty());
    }

    #[test]
    fn the_shell_settings_have_defaults_and_refuse_unknown_keys() {
        let empty: ShellConfig = toml::from_str("").unwrap();
        assert_eq!(empty, ShellConfig::default());
        assert!(empty.emergency_keys && empty.quit_to_explorer && empty.media_keys);
        let quiet: ShellConfig = toml::from_str("media_keys = false").unwrap();
        assert!(!quiet.media_keys);
        assert!(toml::from_str::<ShellConfig>("startup = true").is_err());
    }
}
