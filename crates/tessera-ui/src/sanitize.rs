// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

/// Plain display text, never commands/markup. Preserve language-shaping joiners.
pub(crate) fn bounded_text(raw: &str, limit: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|character| {
            !matches!(
                *character,
                '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}' | '\u{feff}'
            ) && (!character.is_control() || character.is_whitespace())
        })
        .collect();
    let normalized = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut characters = normalized.chars();
    let mut result: String = characters.by_ref().take(limit).collect();
    if characters.next().is_some() {
        result.push('…');
    }
    result
}

pub(crate) fn caption(raw: &str) -> String {
    let caption = bounded_text(raw, 128);
    if caption.is_empty() {
        "(untitled window)".into()
    } else {
        caption
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_display_controls_without_merging_words_or_damaging_unicode() {
        assert_eq!(caption("  a \n\r\t b\u{0007}\u{202e}  "), "a b");
        assert_eq!(
            caption("\u{20000}\u{200d}\u{20000}"),
            "\u{20000}\u{200d}\u{20000}"
        );
        assert_eq!(caption("\u{061c}abc\u{2066}\u{2069}"), "abc");
    }

    #[test]
    fn empty_and_long_captions_remain_bounded_valid_text() {
        assert_eq!(caption(" \u{0007}\u{202e} "), "(untitled window)");
        let long = "\u{20000}".repeat(140);
        assert_eq!(caption(&long), format!("{}…", "\u{20000}".repeat(128)));
        assert_eq!(bounded_text("abcdef", 3), "abc…");
    }
}
