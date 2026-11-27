// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A reader of JSON for the files of other programs the runners look
//! into: the bookmarks of browsers, the settings of Windows Terminal and
//! the state of Visual Studio Code. It reads the whole of JSON, and also
//! the comments and trailing commas that the settings of Windows
//! Terminal and of Visual Studio Code may have.

use std::collections::BTreeMap;

/// How deep arrays and objects may nest before a file is taken as
/// unreadable, so that a hostile file cannot exhaust the stack.
const DEEPEST: usize = 128;

/// A value of JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    /// The value written in `text`, or `None` where it is not JSON.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut reader = Reader {
            text: text.as_bytes(),
            at: 0,
            depth: 0,
        };
        let value = reader.value()?;
        reader.blank();
        (reader.at == reader.text.len()).then_some(value)
    }

    /// The value of the key `key`, where this is an object that has it.
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(members) => members.get(key),
            _ => None,
        }
    }

    /// The value at the end of `keys`, followed from here.
    pub fn at(&self, keys: &[&str]) -> Option<&Self> {
        keys.iter().try_fold(self, |value, key| value.get(key))
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The entries, where this is an array; none otherwise.
    pub fn items(&self) -> &[Self] {
        match self {
            Self::Array(items) => items,
            _ => &[],
        }
    }

    /// The members, where this is an object.
    pub fn members(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    /// The text of the key `key`, where it is a string.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }
}

struct Reader<'a> {
    text: &'a [u8],
    at: usize,
    depth: usize,
}

impl Reader<'_> {
    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    /// Steps over spaces and comments.
    fn blank(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.at += 1,
                Some(b'/') if self.text.get(self.at + 1) == Some(&b'/') => {
                    while self.peek().is_some_and(|byte| byte != b'\n') {
                        self.at += 1;
                    }
                }
                Some(b'/') if self.text.get(self.at + 1) == Some(&b'*') => {
                    let rest = &self.text[self.at + 2..];
                    let end = rest.windows(2).position(|pair| pair == b"*/");
                    self.at = match end {
                        Some(end) => self.at + 2 + end + 2,
                        None => self.text.len(),
                    };
                }
                _ => return,
            }
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        self.blank();
        let there = self.peek() == Some(byte);
        self.at += usize::from(there);
        there
    }

    fn word(&mut self, word: &[u8], value: Json) -> Option<Json> {
        let there = self.text[self.at..].starts_with(word);
        there.then(|| {
            self.at += word.len();
            value
        })
    }

    fn value(&mut self) -> Option<Json> {
        self.blank();
        match self.peek()? {
            b'{' => self.nested(Self::object),
            b'[' => self.nested(Self::array),
            b'"' => self.string().map(Json::String),
            b't' => self.word(b"true", Json::Bool(true)),
            b'f' => self.word(b"false", Json::Bool(false)),
            b'n' => self.word(b"null", Json::Null),
            _ => self.number(),
        }
    }

    fn nested(&mut self, read: fn(&mut Self) -> Option<Json>) -> Option<Json> {
        if self.depth == DEEPEST {
            return None;
        }
        self.depth += 1;
        let value = read(self);
        self.depth -= 1;
        value
    }

    fn object(&mut self) -> Option<Json> {
        self.at += 1;
        let mut members = BTreeMap::new();
        loop {
            if self.eat(b'}') {
                return Some(Json::Object(members));
            }
            self.blank();
            let key = self.string()?;
            if !self.eat(b':') {
                return None;
            }
            members.insert(key, self.value()?);
            // A comma may stand before the brace.
            if !self.eat(b',') {
                return self.eat(b'}').then_some(Json::Object(members));
            }
        }
    }

    fn array(&mut self) -> Option<Json> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            if self.eat(b']') {
                return Some(Json::Array(items));
            }
            items.push(self.value()?);
            if !self.eat(b',') {
                return self.eat(b']').then_some(Json::Array(items));
            }
        }
    }

    fn number(&mut self) -> Option<Json> {
        let start = self.at;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || b"+-.eE".contains(&byte))
        {
            self.at += 1;
        }
        let written = std::str::from_utf8(&self.text[start..self.at]).ok()?;
        written.parse().ok().map(Json::Number)
    }

    fn string(&mut self) -> Option<String> {
        if self.peek() != Some(b'"') {
            return None;
        }
        self.at += 1;
        let mut bytes = Vec::new();
        loop {
            let byte = self.peek()?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(bytes).ok(),
                b'\\' => {
                    let escaped = self.peek()?;
                    self.at += 1;
                    let character = match escaped {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => self.unicode()?,
                        other => char::from(other),
                    };
                    let mut buffer = [0; 4];
                    bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                }
                _ => bytes.push(byte),
            }
        }
    }

    /// The character of a `\u` escape, which may be the first half of a
    /// pair that stands for one beyond the first plane.
    fn unicode(&mut self) -> Option<char> {
        let first = self.hex()?;
        if (0xd800..0xdc00).contains(&first) && self.text[self.at..].starts_with(b"\\u") {
            self.at += 2;
            let second = self.hex()?;
            let joined = 0x10000 + ((first - 0xd800) << 10) + (second.checked_sub(0xdc00)?);
            return char::from_u32(joined);
        }
        Some(char::from_u32(first).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    fn hex(&mut self) -> Option<u32> {
        let digits = self.text.get(self.at..self.at + 4)?;
        let digits = std::str::from_utf8(digits).ok()?;
        let value = u32::from_str_radix(digits, 16).ok()?;
        self.at += 4;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number(value: &Json) -> Option<f64> {
        match value {
            Json::Number(number) => Some(*number),
            _ => None,
        }
    }

    #[test]
    fn json_is_read_whole() {
        let value = Json::parse(r#" { "a": [1, -2.5e1, true, null], "b": { "c": "d" } } "#)
            .expect("it is JSON");
        let numbers: Vec<Option<f64>> = { value.get("a").unwrap().items().iter() }
            .map(number)
            .collect();
        assert_eq!(numbers, [Some(1.), Some(-25.), None, None]);
        assert_eq!(value.at(&["b", "c"]).and_then(Json::as_str), Some("d"));
        assert_eq!(Json::parse("[1, 2"), None);
        assert_eq!(Json::parse("{} x"), None);
        assert_eq!(Json::parse(""), None);
    }

    #[test]
    fn escapes_are_undone() {
        let value = Json::parse(r#""a\"b\\c\né😀\/""#).expect("a string");
        assert_eq!(value.as_str(), Some("a\"b\\c\né😀/"));
    }

    #[test]
    fn comments_and_trailing_commas_are_allowed() {
        let text = "\u{feff}{\n// a comment\n\"a\": [1, 2,], /* another */ \"b\": 3,\n}";
        let value = Json::parse(text).expect("lenient JSON");
        assert_eq!(value.get("a").map(|a| a.items().len()), Some(2));
        assert_eq!(value.get("b").and_then(number), Some(3.));
    }

    #[test]
    fn a_file_nested_too_deep_is_not_read() {
        let deep = format!("{}{}", "[".repeat(DEEPEST + 1), "]".repeat(DEEPEST + 1));
        assert_eq!(Json::parse(&deep), None);
        let fine = format!("{}{}", "[".repeat(DEEPEST), "]".repeat(DEEPEST));
        assert!(Json::parse(&fine).is_some());
    }
}
