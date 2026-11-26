// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Reading the settings of `[extension.<id>]` in the shell's
//! configuration.
//!
//! The functions taking a key ask Tessera; the `parse_` functions under
//! them read text alone and can be tested anywhere.

use std::fmt::Display;
use std::ops::RangeInclusive;
use std::str::FromStr;

use crate::log;

/// The setting `key`, trimmed, unless it is missing or blank.
pub fn text(key: &str) -> Option<String> {
    crate::setting(key)
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The setting `key` read as a number within `range`, or `default` when
/// it is missing. A number outside the range is moved to its nearest end;
/// one that cannot be read is logged and replaced by `default`.
pub fn number<T>(key: &str, default: T, range: RangeInclusive<T>) -> T
where
    T: FromStr + PartialOrd + Copy + Display,
{
    let Some(text) = text(key) else {
        return default;
    };
    parse_number(&text, range).unwrap_or_else(|error| {
        log(&format!("the setting `{key}` {error}; {default} is used"));
        default
    })
}

/// The setting `key` read as `true` or `false`, or `default`.
pub fn flag(key: &str, default: bool) -> bool {
    match text(key).as_deref() {
        None => default,
        Some(text) => parse_flag(text).unwrap_or_else(|| {
            log(&format!(
                "the setting `{key}` is true or false, not `{text}`; {default} is used"
            ));
            default
        }),
    }
}

/// `text` read as a number, moved into `range` when it is outside.
pub fn parse_number<T>(text: &str, range: RangeInclusive<T>) -> Result<T, String>
where
    T: FromStr + PartialOrd + Copy + Display,
{
    let number: T = text
        .trim()
        .parse()
        .map_err(|_| format!("is not a number but `{}`", text.trim()))?;
    Ok(clamp(number, range))
}

fn clamp<T: PartialOrd + Copy>(number: T, range: RangeInclusive<T>) -> T {
    let (low, high) = range.into_inner();
    if number < low {
        low
    } else if number > high {
        high
    } else {
        number
    }
}

pub fn parse_flag(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// The items of a list such as `a, b; c`, trimmed, without blank ones.
pub fn parse_list(text: &str) -> Vec<String> {
    text.split([',', ';', '\n'])
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The entries of a list such as `Tokyo=+9; London=+0`: names and values
/// split at the first `=`, entries apart by `;` or a line break. An entry
/// without a name or a value is named in the error.
pub fn parse_pairs(text: &str) -> Result<Vec<(String, String)>, String> {
    text.split([';', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .map(|(name, value)| (name.trim(), value.trim()))
                .filter(|(name, value)| !name.is_empty() && !value.is_empty())
                .ok_or_else(|| format!("`{entry}` is not written as name=value"))?;
            Ok((name.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_kept_in_range() {
        assert_eq!(parse_number(" 25 ", 1..=120), Ok(25));
        assert_eq!(parse_number("0", 1..=120), Ok(1));
        assert_eq!(parse_number("500", 1..=120), Ok(120));
        assert_eq!(parse_number("2.5", 0.5..=10.), Ok(2.5));
        assert!(parse_number::<u32>("many", 1..=120).is_err());
    }

    #[test]
    fn flags_are_read_in_several_spellings() {
        assert_eq!(parse_flag("True"), Some(true));
        assert_eq!(parse_flag("off"), Some(false));
        assert_eq!(parse_flag("maybe"), None);
    }

    #[test]
    fn lists_are_split_and_trimmed() {
        assert_eq!(parse_list(" a, b;; c ,"), ["a", "b", "c"]);
        assert!(parse_list("  ").is_empty());
    }

    #[test]
    fn pairs_are_split_at_the_first_sign() {
        let pairs = parse_pairs("Tokyo=+9; New York = -5\nA=b=c").unwrap();
        let expected = [("Tokyo", "+9"), ("New York", "-5"), ("A", "b=c")];
        let pairs: Vec<(&str, &str)> = pairs.iter().map(|(a, b)| (&**a, &**b)).collect();
        assert_eq!(pairs, expected);
        assert!(parse_pairs("Tokyo").unwrap_err().contains("Tokyo"));
        assert!(parse_pairs("=3").is_err());
    }
}
