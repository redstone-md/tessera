// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Answers to what is typed into the launcher, the runner and the Run
//! dialog, in the manner of KRunner of KDE Plasma: each [`Runner`] looks
//! at the text and offers what it can do with it, such as the result of
//! a calculation, a conversion of units, a search on the web, a window to
//! switch to, a command to run, a page of the settings or an emoji.
//!
//! The runners are asked on the background executor, each on its own,
//! so that one that reads the disk or the network holds up neither the
//! others nor the thread of GPUI; their answers arrive one runner at a
//! time on the receiver [`Runners::ask`] gives. What the user chooses is
//! remembered by [`History`], which ranks it higher the next time.
//!
//! A widget chooses its runners with the option `runners`, a list of
//! their names ([`ALL`]), and tunes them with the options `web`, `engine`,
//! `currency`, `recent` and `settings`.

mod calc;
mod command;
mod emoji;
mod history;
mod recent;
mod system;
mod units;
mod web;
mod windows;

use std::path::PathBuf;
use std::sync::Arc;

use async_channel::Receiver;
use gpui::{App, BackgroundExecutor, ClipboardItem};

use crate::files::Files;
use crate::i18n::Language;
use crate::module::WidgetContext;
use crate::platform::{AppWindowId, PowerAction, RunningProcess};

pub use command::{Command, completions, terminal_launch};
pub use history::History;

/// The names of the runners, in the order they are asked.
pub const ALL: [&str; 11] = [
    "calculator",
    "units",
    "command",
    "web",
    "windows",
    "processes",
    "power",
    "settings",
    "emoji",
    "recent",
    "search",
];

/// The score from which an answer stands above the applications in the
/// launcher; those below it come after the applications and files.
pub const STRONG: u16 = 500;

/// What is typed, as the runners look at it.
#[derive(Clone, Debug)]
pub struct Query {
    text: String,
    lower: String,
    language: Language,
}

impl Query {
    pub fn new(text: &str, language: Language) -> Self {
        let text = text.trim().to_owned();
        Self {
            lower: text.to_lowercase(),
            text,
            language,
        }
    }

    /// What is typed, without spaces around it.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The same in lowercase letters.
    pub fn lower(&self) -> &str {
        &self.lower
    }

    /// How many letters are typed.
    pub fn letters(&self) -> usize {
        self.text.chars().count()
    }

    /// The language of the user interface.
    pub fn language(&self) -> Language {
        self.language
    }

    /// `text` in the language of the user interface.
    pub fn say(&self, text: &'static str) -> &'static str {
        self.language.say(text)
    }
}

/// What choosing an answer does.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerAction {
    /// Nothing: the answer only tells something.
    Nothing,
    /// Puts the text on the clipboard.
    Copy(String),
    /// Starts a program, opens a file, a folder or an address.
    Run(Command),
    /// Brings a window in front.
    Focus(AppWindowId),
    /// Asks a window to close.
    Close(AppWindowId),
    /// Ends the processes of a program at once, after the user confirms
    /// it.
    Kill { ids: Vec<u32>, name: String },
    /// Locks, sleeps, signs out, restarts or shuts down, after the user
    /// confirms it, but locking.
    Power(PowerAction),
}

impl AnswerAction {
    /// Whether the action asks to be confirmed before it is done, as it
    /// loses what is not saved.
    pub fn needs_confirmation(&self) -> bool {
        match self {
            Self::Kill { .. } => true,
            Self::Power(action) => *action != PowerAction::Lock,
            _ => false,
        }
    }

    /// Does the action. With `admin` a command runs as an administrator,
    /// and with `terminal` in a terminal; other actions do not heed them.
    pub fn perform(&self, admin: bool, terminal: bool, cx: &mut App) {
        match self {
            Self::Nothing => {}
            Self::Copy(text) => cx.write_to_clipboard(ClipboardItem::new_string(text.clone())),
            Self::Run(command) if command.terminal || (terminal && !command.is_address()) => {
                // Which terminal there is is read from the disk.
                let line = command.line();
                cx.background_executor()
                    .spawn(async move { terminal_launch(&line, admin).start() })
                    .detach();
            }
            Self::Run(command) => {
                if !(command.folder && !admin && Files::open_folder(&command.path(), cx)) {
                    command.launch(admin).start();
                }
            }
            Self::Focus(window) => window.activate(),
            Self::Close(window) => window.close(),
            Self::Kill { ids, .. } => {
                let ids = ids.clone();
                cx.background_executor()
                    .spawn(async move {
                        for id in ids {
                            if let Err(error) = RunningProcess::kill(id) {
                                report!("{error:#}");
                            }
                        }
                    })
                    .detach();
            }
            Self::Power(action) => action.perform(),
        }
    }
}

/// Something a runner offers for what is typed.
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub title: String,
    /// What the answer is, said under or beside its title.
    pub detail: String,
    /// The icon of the Segoe Fluent Icons font that stands for it.
    pub glyph: &'static str,
    /// A file whose icon stands for it instead, where it has one.
    pub icon: Option<PathBuf>,
    /// How sure the runner is that this is what is wanted, up to 1000;
    /// see [`STRONG`].
    pub score: u16,
    pub action: AnswerAction,
    /// A second thing to do, on a button of its own: its icon and its
    /// action.
    pub button: Option<(&'static str, AnswerAction)>,
    /// What Ctrl+C copies while the answer is chosen.
    pub copy: Option<String>,
    /// What the history remembers the answer by; empty for one not to
    /// remember.
    pub key: String,
    /// Whether the runner is still finding out, and is to be asked again
    /// in a moment.
    pub waiting: bool,
}

/// The icon of the button that copies.
const ICON_COPY: &str = "\u{e8c8}";

impl Answer {
    pub fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        glyph: &'static str,
        score: u16,
        action: AnswerAction,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            glyph,
            icon: None,
            score,
            action,
            button: None,
            copy: None,
            key: String::new(),
            waiting: false,
        }
    }

    /// The answer, with a button that copies `text`, as Ctrl+C does.
    pub fn copying(mut self, text: impl Into<String>) -> Self {
        let text = text.into();
        self.button = Some((ICON_COPY, AnswerAction::Copy(text.clone())));
        self.copy = Some(text);
        self
    }

    /// The answer, with a second action on a button showing `glyph`.
    pub fn with_button(mut self, glyph: &'static str, action: AnswerAction) -> Self {
        self.button = Some((glyph, action));
        self
    }

    /// The answer, remembered by the history as `key`.
    pub fn keyed(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// The answer, shown with the icon of `file`.
    pub fn with_icon(mut self, file: PathBuf) -> Self {
        self.icon = Some(file);
        self
    }

    /// The answer, to be asked for again in a moment.
    pub fn waiting(mut self) -> Self {
        self.waiting = true;
        self
    }
}

/// Something that answers what is typed.
pub trait Runner: Send + Sync {
    /// The name a layout calls it by, one of [`ALL`].
    fn id(&self) -> &'static str;

    /// What it offers for `query`, which may be empty. It runs on the
    /// background executor and may read the disk, but should answer
    /// within a few milliseconds where it can.
    fn answer(&self, query: &Query) -> Vec<Answer>;
}

/// How the runners of a widget are chosen by its options.
#[derive(Clone, Debug, Default)]
pub struct RunnerOptions {
    /// Whether the recent documents are offered while nothing is typed.
    pub recent_when_empty: bool,
}

/// The runners a widget asks.
#[derive(Clone)]
pub struct Runners {
    list: Arc<[Box<dyn Runner>]>,
}

impl Runners {
    /// The runners the options of a widget choose: those `runners` names,
    /// or all of them, set up as `web`, `engine`, `currency`, `recent` and
    /// `settings` say; `defaults` gives what holds where an option is
    /// not given. An unknown name is reported.
    pub fn of_widget(context: &WidgetContext, defaults: RunnerOptions) -> Self {
        let named = context.texts("runners");
        let mut chosen: Vec<&str> = match &named {
            Some(names) => { names.iter() }
                .filter_map(|name| {
                    let known = ALL.iter().find(|known| **known == name.as_str());
                    if known.is_none() {
                        report!(
                            "there is no runner `{name}`; there are `{}`",
                            ALL.join("`, `")
                        );
                    }
                    known.copied()
                })
                .collect(),
            None => ALL.to_vec(),
        };
        if context.flag("settings") == Some(false) {
            chosen.retain(|name| *name != "settings");
        }
        let web = web::Web::of_widget(context);
        let currency = context.flag("currency").unwrap_or(true);
        let recent = context.flag("recent").unwrap_or(defaults.recent_when_empty);
        let all: Vec<Box<dyn Runner>> = vec![
            Box::new(calc::Calculator),
            Box::new(units::Units::new(currency)),
            Box::new(command::Commands),
            Box::new(web.clone()),
            Box::new(windows::Windows),
            Box::new(windows::Processes),
            Box::new(system::Power),
            Box::new(system::Settings),
            Box::new(emoji::Emoji),
            Box::new(recent::Recent::new(recent)),
            Box::new(web.fallback()),
        ];
        let list: Vec<Box<dyn Runner>> = { all.into_iter() }
            .filter(|runner| chosen.contains(&runner.id()))
            .collect();
        Self { list: list.into() }
    }

    /// Asks every runner about `query` on `executor`. The receiver gives
    /// the answers of one runner at a time, as they come, each with the
    /// place of its runner among these, and closes when all have
    /// answered; dropping it leaves the answers unread.
    pub fn ask(
        &self,
        query: Query,
        executor: &BackgroundExecutor,
    ) -> Receiver<(usize, Vec<Answer>)> {
        let (send, answers) = async_channel::unbounded();
        let query = Arc::new(query);
        for index in 0..self.list.len() {
            let (list, query, send) = (self.list.clone(), query.clone(), send.clone());
            executor
                .spawn(async move {
                    if send.is_closed() {
                        return;
                    }
                    let answers = list[index].answer(&query);
                    send.try_send((index, answers)).ok();
                })
                .detach();
        }
        answers
    }
}

/// How well `name` answers `query`, which is lowercase; lower is better:
/// 0 where it starts with it, 1 where a word in it does, 2 where it only
/// contains it.
pub fn rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    let at = name.find(query)?;
    Some(if at == 0 {
        0
    } else if name[..at].ends_with(|before: char| !before.is_alphanumeric()) {
        1
    } else {
        2
    })
}

/// The score of an answer that `rank` gave the rank of, from `best` for
/// a name that starts with what is typed down by a tenth for each worse
/// rank.
pub fn ranked_score(best: u16, rank: u8) -> u16 {
    best.saturating_sub(u16::from(rank) * (best / 10))
}

/// `template` with each `{}` replaced by the next of `parts`, for texts
/// that are said in another language with something put into them.
pub fn fill(template: &str, parts: &[&str]) -> String {
    let mut filled = String::with_capacity(template.len());
    let mut parts = parts.iter();
    let mut rest = template;
    while let Some(at) = rest.find("{}") {
        filled.push_str(&rest[..at]);
        filled.push_str(parts.next().copied().unwrap_or_default());
        rest = &rest[at + 2..];
    }
    filled.push_str(rest);
    filled
}

/// What the history remembers an application of the Start menu by:
/// the name of its shortcut.
pub fn app_key(name: &str) -> String {
    format!("app:{}", name.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn better_matches_rank_lower() {
        assert_eq!(rank("Windows Terminal", "term"), Some(1));
        assert_eq!(rank("Terminal", "term"), Some(0));
        assert_eq!(rank("Determine", "term"), Some(2));
        assert_eq!(rank("Paint", "term"), None);
        assert_eq!(ranked_score(700, 0), 700);
        assert_eq!(ranked_score(700, 2), 560);
    }

    #[test]
    fn a_template_is_filled_in_order() {
        assert_eq!(
            fill("Search {} for “{}”", &["Google", "cats"]),
            "Search Google for “cats”"
        );
        assert_eq!(fill("{} and {}", &["one"]), "one and ");
        assert_eq!(fill("plain", &["unused"]), "plain");
    }

    #[test]
    fn only_what_loses_something_is_confirmed() {
        assert!(AnswerAction::Power(PowerAction::Restart).needs_confirmation());
        assert!(!AnswerAction::Power(PowerAction::Lock).needs_confirmation());
        let kill = AnswerAction::Kill {
            ids: vec![42],
            name: "notepad.exe".to_owned(),
        };
        assert!(kill.needs_confirmation());
        assert!(!AnswerAction::Copy("4".to_owned()).needs_confirmation());
    }

    #[test]
    fn every_runner_has_its_name() {
        let mut names = ALL.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }
}
