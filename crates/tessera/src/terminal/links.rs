// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The addresses and paths a terminal shows, which Ctrl+click opens: the
//! word under the pointer, without the punctuation of the sentence around
//! it, where it is an address of the web or of mail, or a path.

/// What stands under the pointer that can be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    /// An address with a scheme: `https://…`, `mailto:…`, `file://…`.
    Address(String),
    /// A path, of a drive (`C:\…`), of a share (`\\server\…`), or one that
    /// may lie in the folder of the shell (`src\main.rs:12`); without the
    /// line and column compilers write after it.
    Path(String),
}

/// The schemes of addresses that are opened.
const SCHEMES: [&str; 5] = ["https://", "http://", "ftp://", "file://", "mailto:"];

/// What ends a word that may be a link, besides spaces.
const BREAKS: [char; 6] = ['"', '\'', '<', '>', '`', '|'];

/// What may stand around a link in a sentence, and is not part of it.
const BEFORE: [char; 4] = ['(', '[', '{', '<'];
const AFTER: [char; 9] = ['.', ',', ';', ':', '!', '?', ')', ']', '}'];

/// The link at `column` of a line whose letters, one per column, are
/// `letters`.
pub fn link_at(letters: &[char], column: usize) -> Option<Link> {
    let breaks = |letter: &char| letter.is_whitespace() || BREAKS.contains(letter);
    if column >= letters.len() || breaks(&letters[column]) {
        return None;
    }
    let start = { letters[..column].iter() }
        .rposition(breaks)
        .map_or(0, |at| at + 1);
    let end = { letters[column..].iter() }
        .position(breaks)
        .map_or(letters.len(), |at| column + at);
    let word: String = letters[start..end].iter().collect();
    let word = word.trim_start_matches(BEFORE).trim_end_matches(AFTER);
    if word.is_empty() {
        return None;
    }
    let lower = word.to_lowercase();
    if SCHEMES.iter().any(|scheme| lower.starts_with(scheme)) {
        let bare = SCHEMES.iter().any(|scheme| lower == *scheme);
        return (!bare).then(|| Link::Address(word.to_owned()));
    }
    let path = without_position(word);
    let bytes = path.as_bytes();
    let drive = bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    let share = path.starts_with(r"\\") && path.len() > 2;
    let relative = path.contains(['\\', '/']) || path.contains('.');
    (drive || share || relative).then(|| Link::Path(path.to_owned()))
}

/// `path` without the `:line` or `:line:column` compilers write after a
/// file; a drive's colon stays.
fn without_position(path: &str) -> &str {
    let mut path = path;
    for _ in 0..2 {
        match path.rsplit_once(':') {
            Some((before, number))
                if !number.is_empty()
                    && number.chars().all(|digit| digit.is_ascii_digit())
                    && before.len() > 1 =>
            {
                path = before;
            }
            _ => break,
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, column: usize) -> Option<Link> {
        let letters: Vec<char> = line.chars().collect();
        link_at(&letters, column)
    }

    #[test]
    fn an_address_is_found_without_the_sentence_around_it() {
        let line = "See (https://example.com/a?b=1). Then";
        assert_eq!(
            at(line, 10),
            Some(Link::Address("https://example.com/a?b=1".to_owned()))
        );
        assert_eq!(
            at("mail mailto:me@example.com now", 8),
            Some(Link::Address("mailto:me@example.com".to_owned()))
        );
        assert_eq!(at("just https:// nothing", 7), None);
    }

    #[test]
    fn paths_are_found_with_the_place_a_compiler_names_left_out() {
        assert_eq!(
            at(r"error at C:\work\main.rs:12:5: oops", 12),
            Some(Link::Path(r"C:\work\main.rs".to_owned()))
        );
        assert_eq!(
            at(r"  \\server\share\file.txt", 5),
            Some(Link::Path(r"\\server\share\file.txt".to_owned()))
        );
        assert_eq!(
            at("--> src/view.rs:40", 6),
            Some(Link::Path("src/view.rs".to_owned()))
        );
        assert_eq!(at("'C:\\a b'", 2), Some(Link::Path("C:\\a".to_owned())));
    }

    #[test]
    fn plain_words_and_spaces_are_no_links() {
        assert_eq!(at("hello world", 2), None);
        assert_eq!(at("hello world", 5), None);
        assert_eq!(at("short", 40), None);
    }
}
