// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Unicode characters: `U+2192` gives the arrow →, `char arrow` the
//! characters whose names have a word starting with each word typed, and
//! `char →` the code and the name of the character typed. Choosing one
//! copies it. The names are those of the Unicode standard, in English.

use std::sync::OnceLock;

use super::{Answer, AnswerAction, Query, Runner, fill};

const ICON_CHARACTER: &str = "\u{e8d2}";

/// The scores of a character asked for by its code and of one found by
/// its name, and how many found by name are shown.
const SCORE_CODE: u16 = 900;
const SCORE_NAME: u16 = 850;
const SHOWN: usize = 16;

/// How many letters a name search waits for.
const LETTERS: usize = 2;

/// What a search by name is typed after.
const NAMED: &str = "char ";

/// The highest code of a character.
const LAST: u32 = 0x10_ffff;

/// The names that are made from the code rather than told, which a
/// search by name leaves out: there are tens of thousands of them.
const MADE_NAMES: [&str; 6] = [
    "CJK UNIFIED IDEOGRAPH-",
    "CJK COMPATIBILITY IDEOGRAPH-",
    "HANGUL SYLLABLE ",
    "TANGUT IDEOGRAPH-",
    "KHITAN SMALL SCRIPT CHARACTER-",
    "NUSHU CHARACTER-",
];

/// What is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Asked {
    /// The characters of these codes, as `U+2192 U+20AC`.
    Codes(Vec<char>),
    /// The characters whose names have these words, uppercase.
    Named(Vec<String>),
}

/// What `text` asks for, if it asks for a character.
fn asked(text: &str) -> Option<Asked> {
    let text = text.trim();
    if let Some(named) = { text.get(..NAMED.len()) }
        .filter(|start| start.eq_ignore_ascii_case(NAMED))
        .map(|_| text[NAMED.len()..].trim())
    {
        let mut letters = named.chars();
        // One character alone is told of by its code and name.
        if let (Some(character), None) = (letters.next(), letters.next())
            && !character.is_ascii_alphanumeric()
        {
            return Some(Asked::Codes(vec![character]));
        }
        if named.chars().count() < LETTERS {
            return None;
        }
        let words = named.split_whitespace().map(str::to_uppercase).collect();
        return Some(Asked::Named(words));
    }
    let codes: Option<Vec<char>> = text.split_whitespace().map(code).collect();
    codes.filter(|codes| !codes.is_empty()).map(Asked::Codes)
}

/// The character `word` names by its code, as `U+2192` or `u+1f600`.
fn code(word: &str) -> Option<char> {
    let digits = word
        .strip_prefix("U+")
        .or_else(|| word.strip_prefix("u+"))?;
    if digits.is_empty() || digits.len() > 6 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    char::from_u32(value).filter(|_| value <= LAST)
}

/// The code of `character` as Unicode writes it.
fn written(character: char) -> String {
    format!("U+{:04X}", u32::from(character))
}

/// Every character with a name of its own and that name, read once.
fn named_characters() -> &'static [(char, String)] {
    static NAMED: OnceLock<Vec<(char, String)>> = OnceLock::new();
    NAMED.get_or_init(|| {
        { (0..=LAST).filter_map(char::from_u32) }
            .filter_map(|character| {
                let name = unicode_names2::name(character)?.to_string();
                let made = MADE_NAMES.iter().any(|made| name.starts_with(made));
                (!made).then_some((character, name))
            })
            .collect()
    })
}

/// How well `name` has `words`: `None` where a word of it does not start
/// with each of them; 0 for a name that is the words, 1 where the first
/// word starts it, 2 otherwise.
fn name_rank(name: &str, words: &[String]) -> Option<u8> {
    let parts: Vec<&str> = name.split([' ', '-']).collect();
    let all = words
        .iter()
        .all(|word| parts.iter().any(|part| part.starts_with(word.as_str())));
    if !all {
        return None;
    }
    Some(
        if parts.len() == words.len() && parts.iter().zip(words).all(|(p, w)| p == w) {
            0
        } else if parts
            .first()
            .is_some_and(|first| first.starts_with(words[0].as_str()))
        {
            1
        } else {
            2
        },
    )
}

/// The characters whose names have `words`, the best first; shorter
/// names first among equals.
fn find(characters: &[(char, String)], words: &[String]) -> Vec<(char, String)> {
    let mut found: Vec<(u8, usize, char, &String)> = { characters.iter() }
        .filter_map(|(character, name)| {
            Some((name_rank(name, words)?, name.len(), *character, name))
        })
        .collect();
    found.sort_by_key(|&(rank, length, character, _)| (rank, length, character));
    { found.into_iter().take(SHOWN) }
        .map(|(.., character, name)| (character, name.clone()))
        .collect()
}

/// The answer for `character`.
fn answer(character: char, name: Option<String>, score: u16, query: &Query) -> Answer {
    let shown = character.to_string();
    let name = name.unwrap_or_else(|| query.say("A character without a name").to_owned());
    let title = fill("{}  {}", &[&shown, &name]);
    let detail = fill(
        "{} · {}",
        &[&written(character), query.say("Copies the character")],
    );
    Answer::new(title, detail, ICON_CHARACTER, score, {
        AnswerAction::Copy(shown.clone())
    })
    .copying(shown)
    .keyed(format!("unicode:{}", written(character)))
}

pub struct Unicode;

impl Runner for Unicode {
    fn id(&self) -> &'static str {
        "unicode"
    }

    fn answer(&self, query: &Query) -> Vec<Answer> {
        match asked(query.text()) {
            None => Vec::new(),
            Some(Asked::Codes(codes)) => { codes.into_iter() }
                .map(|character| {
                    let name = unicode_names2::name(character).map(|name| name.to_string());
                    answer(character, name, SCORE_CODE, query)
                })
                .collect(),
            Some(Asked::Named(words)) => { find(named_characters(), &words).into_iter() }
                .enumerate()
                .map(|(index, (character, name))| {
                    let score = SCORE_NAME.saturating_sub(index as u16);
                    answer(character, Some(name), score, query)
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_character_is_asked_for_by_its_code_or_by_its_name() {
        assert_eq!(asked("U+2192"), Some(Asked::Codes(vec!['→'])));
        assert_eq!(asked("u+1F600 U+20ac"), Some(Asked::Codes(vec!['😀', '€'])));
        assert_eq!(asked("U+D800"), None);
        assert_eq!(asked("U+110000"), None);
        assert_eq!(asked("U+"), None);
        assert_eq!(asked("2192"), None);
        assert_eq!(
            asked("char right arrow"),
            Some(Asked::Named(vec!["RIGHT".to_owned(), "ARROW".to_owned()]))
        );
        assert_eq!(asked("Char →"), Some(Asked::Codes(vec!['→'])));
        assert_eq!(asked("char a"), None);
        assert_eq!(asked("charming"), None);
        assert_eq!(written('→'), "U+2192");
        assert_eq!(written('😀'), "U+1F600");
    }

    #[test]
    fn names_are_matched_by_the_start_of_their_words() {
        let words = |text: &str| -> Vec<String> {
            text.split_whitespace().map(str::to_uppercase).collect()
        };
        assert_eq!(name_rank("RIGHTWARDS ARROW", &words("arrow")), Some(2));
        assert_eq!(name_rank("RIGHTWARDS ARROW", &words("right arr")), Some(1));
        assert_eq!(name_rank("EURO SIGN", &words("euro sign")), Some(0));
        assert_eq!(
            name_rank("LATIN SMALL LETTER E-ACUTE", &words("acute")),
            Some(2)
        );
        assert_eq!(name_rank("EURO SIGN", &words("dollar")), None);
        let characters = vec![
            ('⇒', "RIGHTWARDS DOUBLE ARROW".to_owned()),
            ('→', "RIGHTWARDS ARROW".to_owned()),
            ('€', "EURO SIGN".to_owned()),
        ];
        let found = find(&characters, &words("arrow"));
        assert_eq!(found[0].0, '→');
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn the_names_are_those_of_the_standard() {
        let name = unicode_names2::name('→').map(|name| name.to_string());
        assert_eq!(name.as_deref(), Some("RIGHTWARDS ARROW"));
    }
}
