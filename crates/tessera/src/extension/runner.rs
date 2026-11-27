// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The answers of extensions to what is typed into the launcher, as the
//! plugins of KRunner give theirs: one runner, `extensions`, asks every
//! loaded extension whose manifest has a `[runner]` table and puts their
//! answers together.
//!
//! Each extension answers on its own thread; what has not come within
//! [`BUDGET`] of the question is left out. An answer whose action is
//! `run` calls the extension back when it is chosen, through the action
//! `extension-answer`, which `tessera do` reaches from the launcher.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use super::host::{STOPPED, Worker};
use super::runtime::{Answer as Offered, AnswerAction as Offer};
use crate::runners::{Answer, AnswerAction, Command, Query, Runner};

/// How long the extensions have to answer, all together.
pub const BUDGET: Duration = Duration::from_millis(300);

/// The most of the score of an answer of an extension that counts.
const SCORE_CAP: u16 = 800;

/// The most answers taken from one extension.
const ANSWERS_EACH: usize = 20;

/// The longest identifier of an answer.
const ID_LIMIT: usize = 64;

/// The icon of an answer that names none.
const ICON_EXTENSION: &str = "\u{ea86}";

/// The name of the action that calls an extension back when one of its
/// answers is chosen; its argument is the extension and the answer.
pub const ANSWER_ACTION: &str = "extension-answer";

/// An extension that answers the launcher.
#[derive(Clone)]
pub struct Answering {
    pub id: String,
    /// What a query starts with to be handed to it, without that start.
    pub prefix: Option<String>,
    pub worker: Worker,
    /// How many questions it was asked, which tells a question that a
    /// later one replaced.
    questions: Arc<AtomicU64>,
}

impl Answering {
    pub fn new(id: String, prefix: Option<String>, worker: Worker) -> Self {
        Self {
            id,
            prefix,
            worker,
            questions: Arc::default(),
        }
    }

    /// Numbers a new question, which replaces those before it.
    fn ask(&self) -> Question {
        let number = self.questions.fetch_add(1, Ordering::Relaxed) + 1;
        Question {
            number,
            questions: self.questions.clone(),
            asked: Instant::now(),
        }
    }
}

/// A question to one extension, as its thread comes to it.
struct Question {
    number: u64,
    questions: Arc<AtomicU64>,
    asked: Instant,
}

impl Question {
    /// Whether answering it is of no use any more: another question
    /// replaced it, or its answers would come too late to be shown. Its
    /// thread then goes on to what waits after it, the drawing of its
    /// widgets among that, instead of working through stale questions
    /// one per key typed.
    fn is_stale(&self) -> bool {
        self.questions.load(Ordering::Relaxed) != self.number || self.asked.elapsed() > BUDGET
    }
}

/// The extensions that answer, as loaded last.
static ANSWERING: Mutex<Vec<Answering>> = Mutex::new(Vec::new());

/// Forgets the extensions that answered, before they are loaded again.
pub fn clear() {
    ANSWERING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Has `answering` answer the launcher too.
pub fn add(answering: Answering) {
    let mut all = ANSWERING.lock().unwrap_or_else(PoisonError::into_inner);
    all.retain(|known| known.id != answering.id);
    all.push(answering);
}

/// The runner of the extensions.
pub struct Extensions;

impl Runner for Extensions {
    fn id(&self) -> &'static str {
        "extensions"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let answering = ANSWERING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let (send, received) = mpsc::channel();
        let mut asked = 0;
        for extension in answering {
            let Some(text) = asked_of(query.text(), extension.prefix.as_deref()) else {
                continue;
            };
            let question = extension.ask();
            let (text, id, send) = (text.to_owned(), extension.id, send.clone());
            // Each answers on its own thread, all of them at once.
            extension.worker.send(
                move |instance| match question.is_stale() {
                    true => Ok(Vec::new()),
                    false => instance.answer(&text),
                },
                move |outcome| {
                    let answers = match outcome {
                        Ok(called) => taken(&id, called.value),
                        Err(error) => {
                            if error != STOPPED {
                                report!("the extension `{id}` did not answer: {error}");
                            }
                            Vec::new()
                        }
                    };
                    send.send(answers).ok();
                },
            );
            asked += 1;
        }
        let deadline = Instant::now() + BUDGET;
        let mut answers = Vec::new();
        for _ in 0..asked {
            let left = deadline.saturating_duration_since(Instant::now());
            match received.recv_timeout(left) {
                Ok(some) => answers.extend(some),
                Err(_) => break,
            }
        }
        answers
    }
}

/// What of `typed` an extension with `prefix` is asked about: all of it
/// without a prefix, what follows the prefix with one, and nothing where
/// it does not start with it.
fn asked_of<'a>(typed: &'a str, prefix: Option<&str>) -> Option<&'a str> {
    let Some(prefix) = prefix.filter(|prefix| !prefix.is_empty()) else {
        return Some(typed);
    };
    let start = typed.get(..prefix.len())?;
    if !start.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = &typed[prefix.len()..];
    // A word that only begins like the prefix is not meant for it.
    match rest.chars().next() {
        None => Some(rest),
        Some(next) if next.is_whitespace() || next == ':' => {
            Some(rest[next.len_utf8()..].trim_start())
        }
        Some(_) => None,
    }
}

/// The answers of the extension `id` that can be shown, as the launcher
/// shows answers.
fn taken(id: &str, offered: Vec<Offered>) -> Vec<Answer> {
    let mut seen = HashSet::new();
    let usable = offered.into_iter().filter(|offer| {
        !offer.title.trim().is_empty() && is_answer_id(&offer.id) && seen.insert(offer.id.clone())
    });
    let answers = usable.take(ANSWERS_EACH).filter_map(|offer| {
        let action = match &offer.action {
            Offer::Run => run_answer(id, &offer.id),
            Offer::Copy(text) => AnswerAction::Copy(text.clone()),
            Offer::Open(address) if address.starts_with("https://") => {
                AnswerAction::Run(Command::as_typed(address))
            }
            Offer::Open(_) => return None,
        };
        let score = offer.score.min(SCORE_CAP);
        let answer = Answer::new(offer.title, offer.detail, glyph(&offer.icon), score, action);
        let answer = answer.keyed(format!("extension:{id}:{}", offer.id));
        Some(match &offer.action {
            Offer::Copy(text) => answer.copying(text.clone()),
            _ => answer,
        })
    });
    answers.collect()
}

/// Whether `id` can name an answer: up to [`ID_LIMIT`] letters, digits
/// and `-`, `_` and `.`, which pass through a command line unquoted.
fn is_answer_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_LIMIT
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// What choosing the answer `answer` of the extension `id` does: it asks
/// the running Tessera to do [`ANSWER_ACTION`], as `tessera do` does.
fn run_answer(id: &str, answer: &str) -> AnswerAction {
    let program = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tessera.exe"));
    let arguments = format!("do {ANSWER_ACTION} {id} {answer}");
    AnswerAction::Run(Command {
        target: program.to_string_lossy().into_owned(),
        typed: format!("tessera {arguments}"),
        arguments: Some(arguments),
        folder: false,
        terminal: false,
    })
}

/// The extension and the answer the argument of [`ANSWER_ACTION`] names.
pub fn chosen(arg: Option<&str>) -> Result<(&str, &str), String> {
    let (id, answer) = arg
        .map(str::trim)
        .and_then(|arg| arg.split_once(' '))
        .ok_or_else(|| format!("`{ANSWER_ACTION}` takes an extension and an answer"))?;
    match is_answer_id(answer) && !id.is_empty() {
        true => Ok((id, answer)),
        false => Err(format!("`{answer}` does not name an answer")),
    }
}

/// The icon an extension names for an answer, kept for as long as Tessera
/// runs: one character of the icon font, or that of extensions.
fn glyph(named: &str) -> &'static str {
    static GLYPHS: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut letters = named.chars();
    let private = |letter: char| ('\u{e000}'..='\u{f8ff}').contains(&letter);
    let (Some(letter), None) = (letters.next(), letters.next()) else {
        return ICON_EXTENSION;
    };
    if !private(letter) {
        return ICON_EXTENSION;
    }
    // There are no more than some thousand of them, each kept once.
    let mut glyphs = GLYPHS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(known) = glyphs.iter().find(|known| **known == named) {
        return known;
    }
    let kept: &'static str = Box::leak(named.to_owned().into_boxed_str());
    glyphs.push(kept);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(id: &str, title: &str, score: u16, action: Offer) -> Offered {
        Offered {
            id: id.to_owned(),
            title: title.to_owned(),
            detail: "detail".to_owned(),
            icon: String::new(),
            score,
            action,
        }
    }

    #[test]
    fn an_extension_is_asked_about_what_follows_its_prefix() {
        assert_eq!(asked_of("weather", None), Some("weather"));
        assert_eq!(asked_of("sys", Some("sys")), Some(""));
        assert_eq!(asked_of("SYS note", Some("sys")), Some("note"));
        assert_eq!(asked_of("sys:  note", Some("sys")), Some("note"));
        assert_eq!(asked_of("system", Some("sys")), None);
        assert_eq!(asked_of("sy", Some("sys")), None);
        assert_eq!(asked_of("é", Some("e")), None);
        assert_eq!(asked_of("anything", Some("")), Some("anything"));
    }

    #[test]
    fn answers_are_taken_as_the_launcher_shows_them() {
        let answers = taken(
            "monitor",
            vec![
                offer("cpu", "Processor 12%", 2000, Offer::Copy("12%".to_owned())),
                offer("cpu", "Again", 100, Offer::Run),
                offer("bad id", "Bad", 100, Offer::Run),
                offer("empty", " ", 100, Offer::Run),
                offer(
                    "site",
                    "Site",
                    100,
                    Offer::Open("https://example.com".to_owned()),
                ),
                offer("file", "File", 100, Offer::Open("C:\\x.exe".to_owned())),
                offer("7", "Notes", 650, Offer::Run),
            ],
        );
        let titles: Vec<&str> = answers.iter().map(|answer| answer.title.as_str()).collect();
        assert_eq!(titles, ["Processor 12%", "Site", "Notes"]);
        assert_eq!(answers[0].score, SCORE_CAP);
        assert_eq!(answers[0].copy.as_deref(), Some("12%"));
        assert_eq!(answers[0].key, "extension:monitor:cpu");
        let AnswerAction::Run(command) = &answers[2].action else {
            panic!("a run answer runs a command");
        };
        assert_eq!(
            command.arguments.as_deref(),
            Some("do extension-answer monitor 7")
        );
        assert_eq!(answers[2].glyph, ICON_EXTENSION);
    }

    #[test]
    fn a_question_is_stale_once_another_is_asked_or_its_time_is_up() {
        let questions = Arc::new(AtomicU64::new(0));
        let ask = |asked| Question {
            number: questions.fetch_add(1, Ordering::Relaxed) + 1,
            questions: questions.clone(),
            asked,
        };
        let first = ask(Instant::now());
        assert!(!first.is_stale());
        let second = ask(Instant::now());
        assert!(first.is_stale() && !second.is_stale());
        let late = ask(Instant::now() - BUDGET - Duration::from_millis(1));
        assert!(late.is_stale());
    }

    #[test]
    fn the_chosen_answer_is_read_back() {
        assert_eq!(chosen(Some("monitor 7")), Ok(("monitor", "7")));
        assert!(chosen(Some("monitor")).is_err());
        assert!(chosen(Some("monitor a b")).is_err());
        assert!(chosen(None).is_err());
    }

    #[test]
    fn only_icons_of_the_icon_font_are_kept() {
        assert_eq!(glyph("\u{e768}"), "\u{e768}");
        let kept = glyph("\u{e768}");
        assert!(std::ptr::eq(kept, glyph("\u{e768}")));
        assert_eq!(glyph(""), ICON_EXTENSION);
        assert_eq!(glyph("A"), ICON_EXTENSION);
        assert_eq!(glyph("\u{e768}\u{e769}"), ICON_EXTENSION);
    }
}
