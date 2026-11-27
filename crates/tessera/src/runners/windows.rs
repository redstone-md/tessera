// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Open windows and running programs: a window whose title or program
//! has what is typed in its name is offered to switch to, with a button
//! that closes it; `kill <name>` offers to end the processes of the
//! programs of that name, after a confirmation.

use std::path::Path;

use crate::platform::{AppWindow, RunningProcess};

use super::{Answer, AnswerAction, Query, Runner, fill, rank, ranked_score};

const ICON_WINDOW: &str = "\u{e737}";
const ICON_CLOSE: &str = "\u{e711}";
const ICON_END: &str = "\u{e7ba}";

/// How many letters the windows wait for.
const LETTERS: usize = 2;

/// The score of a window whose title starts with what is typed: below
/// the applications, so that typing the name of a program starts it
/// rather than going to a window it has open.
const SCORE_WINDOW: u16 = 480;

/// The score of a program that `kill` names, and how many are offered.
const SCORE_KILL: u16 = 900;
const KILLS_SHOWN: usize = 8;

/// What a `kill` is typed with, before the name of the program.
const KILL: &str = "kill ";

/// The file name of `program` without its extension, as the user knows
/// it.
fn program_name(program: &Path) -> Option<String> {
    Some(program.file_stem()?.to_string_lossy().into_owned())
}

pub struct Windows;

impl Runner for Windows {
    fn id(&self) -> &'static str {
        "windows"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        if query.letters() < LETTERS || query.lower().starts_with(KILL) {
            return Vec::new();
        }
        let mut answers = Vec::new();
        for window in AppWindow::enumerate() {
            let title = window.id.title();
            let program = window.executable.as_deref().and_then(program_name);
            let ranks = [Some(title.as_str()), program.as_deref()]
                .into_iter()
                .flatten()
                .filter_map(|name| rank(name, query.lower()));
            let Some(best) = ranks.min() else {
                continue;
            };
            let detail = match &program {
                Some(program) => fill("{} · {}", &[query.say("Switch to the window"), program]),
                None => query.say("Switch to the window").to_owned(),
            };
            let score = ranked_score(SCORE_WINDOW, best);
            let mut answer = Answer::new(title, detail, ICON_WINDOW, score, {
                AnswerAction::Focus(window.id)
            })
            .with_button(ICON_CLOSE, AnswerAction::Close(window.id));
            if let Some(executable) = window.executable {
                answer = answer.with_icon(executable);
            }
            answers.push(answer);
        }
        answers
    }
}

pub struct Processes;

impl Runner for Processes {
    fn id(&self) -> &'static str {
        "processes"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some(name) = query.lower().strip_prefix(KILL).map(str::trim) else {
            return Vec::new();
        };
        if name.is_empty() {
            return Vec::new();
        }
        let mut programs = programs_named(RunningProcess::all(), name);
        programs.truncate(KILLS_SHOWN);
        { programs.into_iter() }
            .map(|(program, ids, best)| {
                let count = ids.len().to_string();
                let detail = match ids.len() {
                    1 => fill(
                        query.say("Ends the process {} at once"),
                        &[&ids[0].to_string()],
                    ),
                    _ => fill(query.say("Ends its {} processes at once"), &[&count]),
                };
                let score = ranked_score(SCORE_KILL, best);
                let action = AnswerAction::Kill {
                    ids,
                    name: program.clone(),
                };
                Answer::new(program, detail, ICON_END, score, action)
            })
            .collect()
    }
}

/// The programs of `processes` whose names have `name` in them, each
/// with the numbers of its processes and its rank, the best first and
/// otherwise in the order of their names.
fn programs_named(processes: Vec<RunningProcess>, name: &str) -> Vec<(String, Vec<u32>, u8)> {
    let mut programs: Vec<(String, Vec<u32>, u8)> = Vec::new();
    for process in processes {
        let Some(best) = rank(&process.name, name) else {
            continue;
        };
        let known = { programs.iter_mut() }
            .find(|(program, ..)| program.eq_ignore_ascii_case(&process.name));
        match known {
            Some((_, ids, _)) => ids.push(process.id),
            None => programs.push((process.name, vec![process.id], best)),
        }
    }
    programs.sort_by(|(one, _, a), (other, _, b)| {
        a.cmp(b)
            .then_with(|| one.to_lowercase().cmp(&other.to_lowercase()))
    });
    programs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(id: u32, name: &str) -> RunningProcess {
        RunningProcess {
            id,
            name: name.to_owned(),
        }
    }

    #[test]
    fn the_processes_of_a_program_are_ended_together() {
        let running = vec![
            process(10, "chrome.exe"),
            process(11, "notepad.exe"),
            process(12, "Chrome.exe"),
            process(13, "googlechrome_proxy.exe"),
        ];
        let programs = programs_named(running, "chrome");
        assert_eq!(
            programs,
            [
                ("chrome.exe".to_owned(), vec![10, 12], 0),
                ("googlechrome_proxy.exe".to_owned(), vec![13], 2),
            ]
        );
    }

    #[test]
    fn a_program_is_named_without_its_extension() {
        assert_eq!(
            program_name(Path::new(r"C:\Windows\notepad.exe")).as_deref(),
            Some("notepad")
        );
    }
}
