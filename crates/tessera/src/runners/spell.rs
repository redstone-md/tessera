// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Spelling: `spell necessary`, or a word with a question mark after it
//! (`necesary?`), is checked by the spell checker of Windows in the
//! languages of the user that it has dictionaries for. A word spelled
//! right says so; one spelled wrong is offered as the words it may be
//! meant as, each copied when chosen.

use crate::platform::SpellCheck;

use super::{Answer, AnswerAction, Query, Runner, fill};

const ICON_SPELLING: &str = "\u{f87b}";

/// The scores of what is said of a word asked for with `spell`, and of
/// one asked for with a question mark, which may be a question.
const SCORE_ASKED: u16 = 900;
const SCORE_MARKED: u16 = 520;

/// How many suggestions are shown for each language.
const SHOWN: usize = 5;

/// The words a check is asked with.
const ASKED: [&str; 2] = ["spell ", "орфография "];

/// The word `text` asks to check, and whether it was asked for by its
/// word rather than a question mark.
fn asked(text: &str) -> Option<(&str, bool)> {
    let text = text.trim();
    let lower = text.to_lowercase();
    let named = ASKED.iter().find(|word| lower.starts_with(*word));
    let (word, by_name) = match named {
        Some(named) => (text[named.len()..].trim(), true),
        None => (text.strip_suffix('?')?.trim_end(), false),
    };
    let letters = word
        .chars()
        .all(|c| c.is_alphabetic() || c == '\'' || c == '’' || c == '-');
    let fits = word.chars().filter(|c| c.is_alphabetic()).count() >= 2 && letters;
    fits.then_some((word, by_name))
}

pub struct Spell;

impl Runner for Spell {
    fn id(&self) -> &'static str {
        "spell"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        let Some((word, by_name)) = asked(query.text()) else {
            return Vec::new();
        };
        let score = if by_name { SCORE_ASKED } else { SCORE_MARKED };
        let said = match SpellCheck::word(word) {
            Ok(said) => said,
            Err(error) => {
                let title = query.say("Windows has no spell checker").to_owned();
                let detail = format!("{error:#}");
                return vec![Answer::new(
                    title,
                    detail,
                    ICON_SPELLING,
                    score,
                    AnswerAction::Nothing,
                )];
            }
        };
        if said.is_empty() {
            let title = query
                .say("No dictionary for the languages of the user")
                .to_owned();
            let detail = query.say("Spelling").to_owned();
            return vec![Answer::new(
                title,
                detail,
                ICON_SPELLING,
                score,
                AnswerAction::Nothing,
            )];
        }
        // Right in one of the languages is right.
        if let Some(right) = said.iter().find(|spelling| spelling.right) {
            let title = fill(query.say("“{}” is spelled right"), &[word]);
            let detail = fill("{} · {}", &[query.say("Spelling"), &right.language]);
            let answer = Answer::new(title, detail, ICON_SPELLING, score, {
                AnswerAction::Copy(word.to_owned())
            });
            return vec![answer.copying(word)];
        }
        let mut answers = Vec::new();
        for spelling in &said {
            let detail = fill("{} · {}", &[query.say("Spelling"), &spelling.language]);
            if spelling.suggestions.is_empty() {
                let title = fill(query.say("“{}” is not in the dictionary"), &[word]);
                answers.push(Answer::new(title, detail.clone(), ICON_SPELLING, score, {
                    AnswerAction::Nothing
                }));
                continue;
            }
            for suggestion in spelling.suggestions.iter().take(SHOWN) {
                // The likeliest first, and the first language first.
                let score = score.saturating_sub(answers.len() as u16);
                let answer =
                    Answer::new(suggestion.clone(), detail.clone(), ICON_SPELLING, score, {
                        AnswerAction::Copy(suggestion.clone())
                    });
                answers.push(answer.copying(suggestion.clone()));
            }
        }
        answers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_asked_for_by_name_or_with_a_question_mark() {
        assert_eq!(asked("spell necesary"), Some(("necesary", true)));
        assert_eq!(asked("Spell  O'Neil"), Some(("O'Neil", true)));
        assert_eq!(asked("necesary?"), Some(("necesary", false)));
        assert_eq!(asked("орфография корова"), Some(("корова", true)));
        assert_eq!(asked("what is it?"), None);
        assert_eq!(asked("?"), None);
        assert_eq!(asked("a?"), None);
        assert_eq!(asked("2+2?"), None);
        assert_eq!(asked("spelling"), None);
    }
}
