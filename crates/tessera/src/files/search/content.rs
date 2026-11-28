// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What files hold, for a search outside the index of Windows Search: a
//! file of text up to [`LARGEST`] is read whole and searched for the words
//! in any case. Text is told by a mark of its encoding (UTF-8, UTF-16 in
//! either order) or by holding no NUL in its first bytes; anything else,
//! as a document of Word or a picture, is not text Tessera can read, and
//! is said to be not searched rather than not to hold the words.

use std::fs;
use std::io::Read;
use std::path::Path;

/// The largest file read for what it holds, in bytes.
pub const LARGEST: u64 = 8 * 1024 * 1024;

/// How many bytes at the start of a file tell whether it is text.
const SNIFFED: usize = 4096;

/// The extensions of files that hold no text whatever their first bytes
/// look like, in small letters.
const NOT_TEXT: [&str; 33] = [
    "exe", "dll", "sys", "msi", "zip", "7z", "rar", "gz", "cab", "iso", "jpg", "jpeg", "png",
    "gif", "bmp", "webp", "heic", "mp3", "mp4", "mkv", "avi", "mov", "wav", "flac", "pdf", "doc",
    "docx", "xls", "xlsx", "ppt", "pptx", "odt", "lnk",
];

/// Whether the file at `path` holds every one of `words` (in small
/// letters); `None` where it is no text Tessera reads, or too large.
pub fn holds(path: &Path, words: &[String]) -> Option<bool> {
    let extension = { path.extension() }.map(|ext| ext.to_string_lossy().to_lowercase());
    if extension.is_some_and(|ext| NOT_TEXT.contains(&ext.as_str())) {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > LARGEST {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(LARGEST).read_to_end(&mut bytes).ok()?;
    let text = text_of(&bytes)?;
    Some(holds_words(&text, words))
}

/// The text `bytes` hold, where they are text.
pub fn text_of(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(rest).into_owned());
    }
    let utf16 = |rest: &[u8], little: bool| {
        let units: Vec<u16> = { rest.chunks_exact(2) }
            .map(|pair| match little {
                true => u16::from_le_bytes([pair[0], pair[1]]),
                false => u16::from_be_bytes([pair[0], pair[1]]),
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Some(utf16(rest, true));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Some(utf16(rest, false));
    }
    let start = &bytes[..bytes.len().min(SNIFFED)];
    (!start.contains(&0)).then(|| String::from_utf8_lossy(bytes).into_owned())
}

/// Whether `text` holds every one of `words` (in small letters), in any
/// case.
pub fn holds_words(text: &str, words: &[String]) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_told_by_its_mark_or_by_holding_no_nul() {
        assert_eq!(text_of(b"plain words").as_deref(), Some("plain words"));
        assert_eq!(
            text_of(b"\xEF\xBB\xBFwith a mark").as_deref(),
            Some("with a mark")
        );
        let little: Vec<u8> = [0xFF, 0xFE, b'h', 0, b'i', 0].to_vec();
        assert_eq!(text_of(&little).as_deref(), Some("hi"));
        let big: Vec<u8> = [0xFE, 0xFF, 0, b'h', 0, b'i'].to_vec();
        assert_eq!(text_of(&big).as_deref(), Some("hi"));
        assert_eq!(text_of(b"MZ\x90\0\x03\0"), None);
    }

    #[test]
    fn every_word_is_looked_for_in_any_case() {
        let words = ["budget".to_owned(), "q1 plan".to_owned()];
        assert!(holds_words("The BUDGET of the Q1 Plan", &words));
        assert!(!holds_words("The budget", &words));
        assert!(holds_words("anything", &[]));
    }
}
