// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What each runner answers and how it is asked, for the `help` runner,
//! which lists them when `?` is typed, and for the single-runner mode:
//! `calc: 2 + 2` asks only the calculator, `char: arrow` only the
//! Unicode runner, as the name or a short name of a runner and a colon
//! before the text limit the question to that runner.

use super::{Answer, AnswerAction, Query, Runner, fill, rank};

const ICON_HELP: &str = "\u{e897}";

/// The score of the rows of the help, which come before anything else.
const SCORE: u16 = 990;

/// What is typed for the help.
const ASKED: char = '?';

/// A runner as the help tells of it.
#[derive(Clone, Copy, Debug)]
pub struct About {
    /// Its name, one of [`super::ALL`].
    pub id: &'static str,
    /// Short names that limit a question to it, besides its name.
    pub aliases: &'static [&'static str],
    /// What it is called, in English, said in the language of Tessera.
    pub name: &'static str,
    /// What it answers, in English, said in the language of Tessera.
    pub what: &'static str,
    /// How it is asked, as it is typed, where it has a syntax of its own.
    pub example: Option<&'static str>,
    /// What the runner is asked with when a question is limited to it,
    /// `{}` standing for what follows the colon: the word a runner that
    /// waits for one is asked with.
    pub alone: &'static str,
}

/// Every runner, in the order of [`super::ALL`].
pub const ABOUT: [About; 19] = [
    About {
        id: "calculator",
        aliases: &["calc"],
        name: "Calculator",
        what: "Calculations",
        example: Some("2^10 / 3, sqrt(2) * pi, 0xff + 1, 255 to hex"),
        alone: "{}",
    },
    About {
        id: "units",
        aliases: &["convert"],
        name: "Unit conversion",
        what: "Conversions of units and currencies",
        example: Some("5 km to mi, 100 f in c, 100 usd to eur"),
        alone: "{}",
    },
    About {
        id: "command",
        aliases: &["run"],
        name: "Commands",
        what: "Programs, paths and addresses to run",
        example: Some("notepad todo.txt, %TEMP%, shell:Downloads, > ping 1.1.1.1"),
        alone: "{}",
    },
    About {
        id: "web",
        aliases: &[],
        name: "Web shortcuts",
        what: "Searches on the web by a keyword",
        example: Some("g: cats, wiki: Tallinn, yt: music"),
        alone: "{}",
    },
    About {
        id: "windows",
        aliases: &["window"],
        name: "Windows",
        what: "Open windows by their titles and programs",
        example: None,
        alone: "{}",
    },
    About {
        id: "processes",
        aliases: &["kill"],
        name: "Processes",
        what: "Ends the processes of a program",
        example: Some("kill notepad"),
        alone: "kill {}",
    },
    About {
        id: "power",
        aliases: &[],
        name: "Session and power",
        what: "Locking, sleep, signing out, restarting, shutting down",
        example: Some("lock, sleep, restart"),
        alone: "{}",
    },
    About {
        id: "settings",
        aliases: &[],
        name: "Settings pages",
        what: "Pages of the Settings app and the Control Panel",
        example: None,
        alone: "{}",
    },
    About {
        id: "emoji",
        aliases: &[],
        name: "Emoji",
        what: "Emoji and symbols to copy",
        example: Some(":heart"),
        alone: ":{}",
    },
    About {
        id: "recent",
        aliases: &[],
        name: "Recent documents",
        what: "The documents opened lately",
        example: None,
        alone: "{}",
    },
    About {
        id: "search",
        aliases: &[],
        name: "Search the web",
        what: "A search on the web for anything",
        example: None,
        alone: "{}",
    },
    About {
        id: "bookmarks",
        aliases: &["bm"],
        name: "Bookmarks",
        what: "Bookmarks of Edge, Chrome, Brave, Vivaldi and Firefox",
        example: None,
        alone: "{}",
    },
    About {
        id: "spell",
        aliases: &[],
        name: "Spelling",
        what: "Checks the spelling of a word",
        example: Some("spell necessary, necesary?"),
        alone: "spell {}",
    },
    About {
        id: "time",
        aliases: &["tz"],
        name: "Time zones",
        what: "The time in another zone, and conversions between zones",
        example: Some("time tokyo, time in UTC+3, 15:00 berlin in new york"),
        alone: "time {}",
    },
    About {
        id: "terminal",
        aliases: &["wt"],
        name: "Terminal profiles",
        what: "The profiles of Windows Terminal",
        example: None,
        alone: "{}",
    },
    About {
        id: "sessions",
        aliases: &["code"],
        name: "Editor sessions",
        what: "Folders and workspaces opened in Visual Studio Code and VSCodium",
        example: None,
        alone: "{}",
    },
    About {
        id: "unicode",
        aliases: &["char"],
        name: "Unicode characters",
        what: "Characters by their code or name, to copy",
        example: Some("U+2192, char arrow"),
        alone: "char {}",
    },
    About {
        id: "help",
        aliases: &[],
        name: "Help",
        what: "What each runner answers",
        example: Some("?"),
        alone: "?{}",
    },
    About {
        id: "extensions",
        // Not `ext`, which is a filter of the search for files.
        aliases: &[],
        name: "Extensions",
        what: "What the installed extensions answer",
        example: None,
        alone: "{}",
    },
];

/// The runner `text` limits the question to, of `chosen`, and the text
/// that runner is then asked with: `calc: 2 + 2` gives the calculator
/// and `2 + 2`. A word that is one of `keywords`, the keywords of the
/// web shortcuts, stays a web search, and so does any other word.
pub fn scope(text: &str, chosen: &[&str], keywords: &[String]) -> Option<(&'static str, String)> {
    let (word, rest) = text.split_once(':')?;
    let word = word.trim().to_lowercase();
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if keywords.contains(&word) {
        return None;
    }
    let about = { ABOUT.iter() }
        .find(|about| about.id == word || about.aliases.contains(&word.as_str()))?;
    if !chosen.contains(&about.id) {
        return None;
    }
    let rest = rest.trim();
    // A runner named with nothing after it says how it is asked.
    let asked = match rest.is_empty() && chosen.contains(&"help") {
        true => return Some(("help", format!("{ASKED}{}", about.id))),
        false => about.alone.replace("{}", rest),
    };
    Some((about.id, asked))
}

/// The text a runner is asked for its name with, as the help inserts it.
fn prefix(about: &About) -> String {
    format!("{}: ", about.aliases.first().copied().unwrap_or(about.id))
}

/// The runners `chosen` that `asked`, what follows the `?`, names by a
/// part of their names, of what they are called or of what they answer;
/// all of them for nothing.
fn matching<'a>(chosen: &[&str], asked: &str, query: &Query) -> Vec<&'a About> {
    let asked = asked.trim().to_lowercase();
    let mut found: Vec<(u8, &About)> = { ABOUT.iter() }
        .filter(|about| chosen.contains(&about.id))
        .filter_map(|about| {
            if asked.is_empty() {
                return Some((0, about));
            }
            let names = [about.id, query.say(about.name), query.say(about.what)];
            let names = names.into_iter().chain(about.aliases.iter().copied());
            let best = names.filter_map(|name| rank(name, &asked)).min()?;
            Some((best, about))
        })
        .collect();
    found.sort_by_key(|&(rank, _)| rank);
    found.into_iter().map(|(_, about)| about).collect()
}

/// Lists the runners of a widget with how each is asked.
pub struct Help {
    /// The runners the widget asks, by their names.
    chosen: Vec<&'static str>,
}

impl Help {
    pub fn new(chosen: Vec<&'static str>) -> Self {
        Self { chosen }
    }
}

impl Runner for Help {
    fn id(&self) -> &'static str {
        "help"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some(asked) = query.text().strip_prefix(ASKED) else {
            return Vec::new();
        };
        // A sentence after the mark is not a name of a runner.
        if asked.contains(char::is_whitespace) && !asked.trim().is_empty() {
            return Vec::new();
        }
        { matching(&self.chosen, asked, query).into_iter() }
            .enumerate()
            .map(|(index, about)| {
                let what = query.say(about.what);
                let detail = match about.example {
                    Some(example) => fill("{} · {}", &[example, what]),
                    None => what.to_owned(),
                };
                let shown = prefix(about);
                let title = fill("{} ({})", &[query.say(about.name), shown.trim_end()]);
                let score = SCORE.saturating_sub(index as u16);
                let action = AnswerAction::Insert(prefix(about));
                Answer::new(title, detail, ICON_HELP, score, action)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    #[test]
    fn every_runner_is_told_of_once_and_in_order() {
        let ids: Vec<&str> = ABOUT.iter().map(|about| about.id).collect();
        assert_eq!(ids, super::super::ALL);
        let mut short: Vec<&str> = { ABOUT.iter() }
            .flat_map(|about| about.aliases.iter().copied().chain([about.id]))
            .collect();
        let count = short.len();
        short.sort_unstable();
        short.dedup();
        assert_eq!(short.len(), count);
        assert!(ABOUT.iter().all(|about| about.alone.contains("{}")));
    }

    #[test]
    fn a_name_and_a_colon_limit_the_question_to_one_runner() {
        let chosen = ["calculator", "emoji", "help", "web", "unicode"];
        let keywords = ["g".to_owned(), "calc".to_owned()];
        assert_eq!(
            scope("calculator: 2 + 2", &chosen, &[]),
            Some(("calculator", "2 + 2".to_owned()))
        );
        assert_eq!(
            scope("calc:2+2", &chosen, &[]),
            Some(("calculator", "2+2".to_owned()))
        );
        // A runner that waits for a word gets it.
        assert_eq!(
            scope("emoji: heart", &chosen, &[]),
            Some(("emoji", ":heart".to_owned()))
        );
        assert_eq!(
            scope("CHAR: arrow", &chosen, &[]),
            Some(("unicode", "char arrow".to_owned()))
        );
        // Nothing after the colon asks the help about the runner.
        assert_eq!(
            scope("calc:", &chosen, &[]),
            Some(("help", "?calculator".to_owned()))
        );
        // A web keyword, a runner not chosen, a path and an address stay.
        assert_eq!(scope("calc: 1", &chosen, &keywords), None);
        assert_eq!(scope("g: cats", &chosen, &keywords), None);
        assert_eq!(scope("time: tokyo", &chosen, &[]), None);
        assert_eq!(scope(r"C:\Windows", &chosen, &[]), None);
        assert_eq!(scope("ms-settings:display", &chosen, &[]), None);
        assert_eq!(scope("https://example.org", &chosen, &[]), None);
        // A filter of the search for files is no name of a runner.
        assert_eq!(scope("ext:pdf", &crate::runners::ALL, &[]), None);
    }

    #[test]
    fn the_help_lists_the_runners_of_the_widget() {
        let help = Help::new(vec!["calculator", "emoji", "help"]);
        let all = help.answer(&Query::new("?", Language::English));
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].title, "Calculator (calc:)");
        assert_eq!(all[0].action, AnswerAction::Insert("calc: ".to_owned()));
        assert!(all[0].detail.contains("sqrt(2)"));
        let emoji = help.answer(&Query::new("?emo", Language::English));
        assert_eq!(emoji.len(), 1);
        assert_eq!(emoji[0].action, AnswerAction::Insert("emoji: ".to_owned()));
        assert!(
            help.answer(&Query::new("? is it", Language::English))
                .is_empty()
        );
        assert!(
            help.answer(&Query::new("2+2", Language::English))
                .is_empty()
        );
    }
}
