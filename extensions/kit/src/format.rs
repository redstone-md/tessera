// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Writing numbers and text for small spaces.

/// `value` with `decimals` decimals and its thousands apart, as
/// `67,012.35`.
pub fn grouped(value: f64, decimals: usize) -> String {
    let written = format!("{:.*}", decimals, value.abs());
    let (whole, fraction) = written.split_once('.').unwrap_or((&written, ""));
    let mut out = String::new();
    if value < 0. && written.bytes().any(|b| b.is_ascii_digit() && b != b'0') {
        out.push('-');
    }
    for (at, digit) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    if !fraction.is_empty() {
        out.push('.');
        out.push_str(fraction);
    }
    out
}

/// A price or rate with as many decimals as its size wants: none from
/// 1000 on, two from 1, and four significant digits below.
pub fn amount(value: f64) -> String {
    let size = value.abs();
    let decimals = if size >= 1000. {
        0
    } else if size >= 1. {
        2
    } else if size > 0. {
        (3 - size.log10().floor() as i32).clamp(2, 10) as usize
    } else {
        2
    };
    grouped(value, decimals)
}

/// A count as `999`, `1.2k`, `98k` or `3.4M`.
pub fn compact(count: u64) -> String {
    let short = |value: f64, unit: &str| match value < 10. {
        true => format!("{:.1}{unit}", value).replace(".0", ""),
        false => format!("{:.0}{unit}", value),
    };
    match count {
        0..1000 => count.to_string(),
        1000..999_500 => short(count as f64 / 1e3, "k"),
        _ => short(count as f64 / 1e6, "M"),
    }
}

/// A change in percent with its direction, as `▲1.2%` or `▼0.4%`.
pub fn change(percent: f64) -> String {
    let arrow = if percent < 0. { '▼' } else { '▲' };
    format!("{arrow}{:.1}%", percent.abs())
}

/// `text` cut to at most `limit` characters, ending in `…` when cut.
pub fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let kept: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// `text` with its first letter in upper case, and `-` and `_` as spaces,
/// as `Bitcoin cash` from `bitcoin-cash`.
pub fn title(text: &str) -> String {
    let spaced = text.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// `count` and the noun for it, as `1 day` or `3 days`.
pub fn plural(count: u64, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        _ => format!("{count} {many}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(grouped(67_012.346, 2), "67,012.35");
        assert_eq!(grouped(1_234_567., 0), "1,234,567");
        assert_eq!(grouped(999., 0), "999");
        assert_eq!(grouped(-1234.5, 1), "-1,234.5");
        assert_eq!(grouped(-0.001, 2), "0.00");
    }

    #[test]
    fn amounts_keep_their_significant_digits() {
        assert_eq!(amount(67_012.3), "67,012");
        assert_eq!(amount(4.56789), "4.57");
        assert_eq!(amount(0.000_123_4), "0.0001234");
        assert_eq!(amount(0.92), "0.9200");
    }

    #[test]
    fn counts_are_compact() {
        assert_eq!(compact(999), "999");
        assert_eq!(compact(1000), "1k");
        assert_eq!(compact(1234), "1.2k");
        assert_eq!(compact(98_123), "98k");
        assert_eq!(compact(3_400_000), "3.4M");
    }

    #[test]
    fn text_is_shortened_and_titled() {
        assert_eq!(shorten("abcdef", 6), "abcdef");
        assert_eq!(shorten("abc defg", 6), "abc d…");
        assert_eq!(title("bitcoin-cash"), "Bitcoin cash");
        assert_eq!(change(-0.44), "▼0.4%");
        assert_eq!(change(1.26), "▲1.3%");
        assert_eq!(plural(1, "day", "days"), "1 day");
    }
}
