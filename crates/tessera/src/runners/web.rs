// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Web shortcuts: `g: cats` searches Google for cats, `wiki: Tallinn`
//! Wikipedia, and so on for the keywords of [`BUILTIN`] and those the
//! widget option `web` adds, as `web = { so =
//! "https://stackoverflow.com/search?q={}" }`, where `{}` stands for what
//! is searched and `{lang}` for the language of Tessera. The runner
//! `search` offers to search the web for anything typed, after every
//! other answer, with the engine the option `engine` names by its keyword
//! (`ddg` unless it names another).

use crate::module::WidgetContext;

use super::command::Command;
use super::{Answer, AnswerAction, Query, Runner, fill};

const ICON_WEB: &str = "\u{e774}";

/// The score of a search asked for by its keyword, and that of the
/// search offered for anything, which comes last.
const SCORE_KEYWORD: u16 = 950;
const SCORE_FALLBACK: u16 = 100;

/// How many letters the search offered for anything waits for.
const FALLBACK_LETTERS: usize = 2;

/// The keyword of the engine of the search offered for anything, unless
/// the option `engine` names another.
const DEFAULT_ENGINE: &str = "ddg";

/// The engines that come with Tessera: their keywords, their names and
/// their addresses.
const BUILTIN: [(&[&str], &str, &str); 8] = [
    (
        &["g", "gg", "google"],
        "Google",
        "https://www.google.com/search?q={}",
    ),
    (&["ddg"], "DuckDuckGo", "https://duckduckgo.com/?q={}"),
    (&["bing"], "Bing", "https://www.bing.com/search?q={}"),
    (
        &["wiki", "wp"],
        "Wikipedia",
        "https://{lang}.wikipedia.org/w/index.php?search={}",
    ),
    (&["gh"], "GitHub", "https://github.com/search?q={}"),
    (
        &["yt"],
        "YouTube",
        "https://www.youtube.com/results?search_query={}",
    ),
    (
        &["maps"],
        "Google Maps",
        "https://www.google.com/maps/search/{}",
    ),
    (&["ya"], "Yandex", "https://yandex.com/search/?text={}"),
];

/// A search engine, under a keyword.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Engine {
    keyword: String,
    name: String,
    /// The address, with `{}` for what is searched.
    address: String,
}

impl Engine {
    /// The address that searches for `text`, the language being `code`.
    fn address_for(&self, text: &str, code: &str) -> String {
        let code = match code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()) {
            true => code,
            false => "en",
        };
        { self.address.replace("{lang}", code) }.replace("{}", &encode(text))
    }

    /// The answer that searches with this engine for `text`.
    fn answer(&self, text: &str, score: u16, query: &Query) -> Answer {
        let address = self.address_for(text, query.language().code());
        let title = fill(query.say("Search {} for “{}”"), &[&self.name, text]);
        let detail = query.say("Search on the web");
        let command = Command {
            target: address.clone(),
            arguments: None,
            folder: false,
            terminal: false,
            typed: address,
        };
        Answer::new(title, detail, ICON_WEB, score, AnswerAction::Run(command))
    }
}

/// The web shortcuts of a widget.
#[derive(Clone, Debug)]
pub struct Web {
    engines: Vec<Engine>,
    /// The keyword of the engine of the search offered for anything.
    engine: String,
}

impl Web {
    /// The built-in engines and those of the option `web`, which replace
    /// a built-in one of the same keyword, and the engine the option
    /// `engine` names. An address without `{}`, or an engine that is not
    /// there, is reported.
    pub fn of_widget(context: &WidgetContext) -> Self {
        let mut engines: Vec<Engine> = { BUILTIN.iter() }
            .flat_map(|&(keywords, name, address)| {
                keywords.iter().map(move |keyword| Engine {
                    keyword: (*keyword).to_owned(),
                    name: name.to_owned(),
                    address: address.to_owned(),
                })
            })
            .collect();
        let added = context.options.get("web");
        let table = added.and_then(|added| {
            let table = added.as_table();
            if table.is_none() {
                report!("the widget option `web` must be a table of keywords and addresses");
            }
            table
        });
        for (keyword, address) in table.into_iter().flatten() {
            match address.as_str().filter(|address| address.contains("{}")) {
                Some(address) => {
                    let keyword = keyword.to_lowercase();
                    engines.retain(|engine| engine.keyword != keyword);
                    engines.push(Engine {
                        name: host(address).unwrap_or(&keyword).to_owned(),
                        keyword,
                        address: address.to_owned(),
                    });
                }
                None => report!(
                    "the web shortcut `{keyword}` must be an address with {{}} for what is searched"
                ),
            }
        }
        let engine = context
            .text("engine")
            .unwrap_or(DEFAULT_ENGINE)
            .to_lowercase();
        if !engines.iter().any(|known| known.keyword == engine) {
            report!("the widget option `engine` names no web shortcut `{engine}`");
        }
        Self { engines, engine }
    }

    /// The runner of the search offered for anything.
    pub fn fallback(&self) -> Fallback {
        let engine = { self.engines.iter() }
            .find(|engine| engine.keyword == self.engine)
            .or_else(|| self.engines.iter().find(|e| e.keyword == DEFAULT_ENGINE))
            .cloned();
        Fallback { engine }
    }

    /// The engine `text` asks for by its keyword, and what it searches.
    fn asked<'a>(&self, text: &'a str) -> Option<(&Engine, &'a str)> {
        let (keyword, rest) = text.split_once(':')?;
        let keyword = keyword.trim().to_lowercase();
        let rest = rest.trim();
        let engine = self
            .engines
            .iter()
            .find(|engine| engine.keyword == keyword)?;
        (!rest.is_empty()).then_some((engine, rest))
    }
}

impl Runner for Web {
    fn id(&self) -> &'static str {
        "web"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        match self.asked(query.text()) {
            Some((engine, text)) => {
                let key = format!("web:{}", engine.keyword);
                vec![engine.answer(text, SCORE_KEYWORD, query).keyed(key)]
            }
            None => Vec::new(),
        }
    }
}

/// The search offered for anything typed.
pub struct Fallback {
    engine: Option<Engine>,
}

impl Runner for Fallback {
    fn id(&self) -> &'static str {
        "search"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some(engine) = &self.engine else {
            return Vec::new();
        };
        if query.letters() < FALLBACK_LETTERS || query.text().starts_with(['>', ':', '=']) {
            return Vec::new();
        }
        let title = fill(query.say("Search the web for “{}”"), &[query.text()]);
        let answer = engine.answer(query.text(), SCORE_FALLBACK, query);
        vec![Answer {
            title,
            detail: engine.name.clone(),
            ..answer
        }]
    }
}

/// The host of `address`.
fn host(address: &str) -> Option<&str> {
    let (_, rest) = address.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

/// `text` as it can stand in an address: every byte of its UTF-8 but the
/// letters, digits and `-._~` as `%` and two hexadecimal digits.
pub fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;

    fn context(options: &str) -> WidgetContext {
        WidgetContext {
            display: None,
            options: toml::from_str(options).unwrap(),
        }
    }

    #[test]
    fn what_is_searched_is_encoded() {
        assert_eq!(encode("rust gpui"), "rust%20gpui");
        assert_eq!(encode("a&b=c?"), "a%26b%3Dc%3F");
        assert_eq!(encode("ёж"), "%D1%91%D0%B6");
        assert_eq!(encode("v1.2_x~y-z"), "v1.2_x~y-z");
    }

    #[test]
    fn a_keyword_names_its_engine() {
        let web = Web::of_widget(&context(""));
        let (engine, text) = web.asked("wiki: Tallinn").unwrap();
        assert_eq!(text, "Tallinn");
        assert_eq!(
            engine.address_for(text, "ru"),
            "https://ru.wikipedia.org/w/index.php?search=Tallinn"
        );
        assert_eq!(
            engine.address_for(text, "uk-old"),
            "https://en.wikipedia.org/w/index.php?search=Tallinn"
        );
        assert!(web.asked("g:").is_none());
        assert!(web.asked("nothing: here").is_none());
        let (google, _) = web.asked("GG:cats").unwrap();
        assert_eq!(google.name, "Google");
    }

    #[test]
    fn the_options_add_engines_and_choose_the_one_for_anything() {
        let web = Web::of_widget(&context(
            r#"
            engine = "so"
            web = { so = "https://stackoverflow.com/search?q={}", g = "https://example.org/?q={}", bad = "https://x.org" }
            "#,
        ));
        let (engine, _) = web.asked("so: gpui").unwrap();
        assert_eq!(engine.name, "stackoverflow.com");
        assert_eq!(
            web.asked("g: a").unwrap().0.address,
            "https://example.org/?q={}"
        );
        assert!(web.asked("bad: a").is_none());
        let query = Query::new("borrow checker", Language::English);
        let answers = web.fallback().answer(&query);
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].title, "Search the web for “borrow checker”");
        assert_eq!(answers[0].score, SCORE_FALLBACK);
        let short = Query::new("b", Language::English);
        assert!(web.fallback().answer(&short).is_empty());
    }
}
