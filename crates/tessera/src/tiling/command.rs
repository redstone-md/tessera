// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What a `tiling` key, command or action asks for, read from its words.

use super::Toward;
use super::declarative;
use super::layouts::{LayoutChoice, TileLayout};
use super::tree::Axis;

/// Where the next window that opens goes, in the place it opens in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reserve {
    /// Beside the window in front, on that side of it.
    Side(Toward),
    /// After the window in front, dividing its tile along this direction.
    Split(Axis),
    /// Out of the tiles.
    Float,
}

/// What is asked of the tiling.
#[derive(Clone, Debug, PartialEq)]
pub enum TilingCommand {
    /// The layout after the one in use on the display in front, after
    /// the last one no tiling.
    Next,
    /// No tiling.
    Off,
    /// Tiling, with this layout on the display in front.
    Layout(LayoutChoice),
    /// The window in front out of the tiles, or in again.
    Float,
    /// More of the width for the first window of `master`, or less, and
    /// in the other layouts for the window in front; in `rows` it is the
    /// height.
    Wider(bool),
    /// More of the width for the window in front, or less.
    Width(bool),
    /// More of the height for the window in front, or less.
    Height(bool),
    /// Every tile of the display in front alike again.
    Reset,
    /// Where the next window opens.
    Reserve(Reserve),
}

/// What `words` ask of the tiling, or why they ask nothing: `next`,
/// `off`, `float`, `wider`, `narrower`, `reset`, a layout of Tessera or the
/// name of a layout file (whether there is one is asked when it is
/// chosen), `reserve` with a side or `float`, `split` with
/// `horizontal` or `vertical`, or `width` or `height` with `more` or
/// `less`.
pub fn parse_command(words: &[&str]) -> Result<TilingCommand, String> {
    let more = |word: &str, what: &str| match word {
        "more" => Ok(true),
        "less" => Ok(false),
        _ => Err(format!(
            "`{what}` is followed by `more` or `less`, not `{word}`"
        )),
    };
    let words: Vec<String> = words.iter().map(|word| word.to_lowercase()).collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words[..] {
        ["next"] => Ok(TilingCommand::Next),
        ["off"] => Ok(TilingCommand::Off),
        ["float"] => Ok(TilingCommand::Float),
        ["wider"] => Ok(TilingCommand::Wider(true)),
        ["narrower"] => Ok(TilingCommand::Wider(false)),
        ["reset"] => Ok(TilingCommand::Reset),
        ["width", word] => more(word, "width").map(TilingCommand::Width),
        ["height", word] => more(word, "height").map(TilingCommand::Height),
        ["reserve", "float"] => Ok(TilingCommand::Reserve(Reserve::Float)),
        ["reserve", side] => match Toward::parse(side) {
            Some(side) => Ok(TilingCommand::Reserve(Reserve::Side(side))),
            None => Err(format!(
                "`reserve` is followed by `left`, `right`, `up`, `down` or `float`, not `{side}`"
            )),
        },
        ["split", "horizontal"] => Ok(TilingCommand::Reserve(Reserve::Split(Axis::Horizontal))),
        ["split", "vertical"] => Ok(TilingCommand::Reserve(Reserve::Split(Axis::Vertical))),
        ["split", other] => Err(format!(
            "`split` is followed by `horizontal` or `vertical`, not `{other}`"
        )),
        [name] => match TileLayout::named(name) {
            Some(layout) => Ok(TilingCommand::Layout(LayoutChoice::Builtin(layout))),
            None if declarative::may_name(name) => {
                Ok(TilingCommand::Layout(LayoutChoice::File(name.to_owned())))
            }
            None => Err(format!(
                "`tiling` is `next`, `off`, `float`, `wider`, `narrower`, `reset`, `reserve` with a side, `split` with a direction, `width` or `height` with `more` or `less`, or a layout, not `{name}`"
            )),
        },
        [] => Err("`tiling` says what to do".to_owned()),
        _ => Err(format!("`tiling` does not take `{}`", words.join(" "))),
    }
}

/// The words of a `tiling` key or action: what it says, split at spaces.
pub fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<TilingCommand, String> {
        parse_command(&words(text))
    }

    #[test]
    fn the_words_of_before_mean_what_they_meant() {
        assert_eq!(parse("next"), Ok(TilingCommand::Next));
        assert_eq!(parse("off"), Ok(TilingCommand::Off));
        assert_eq!(parse("float"), Ok(TilingCommand::Float));
        assert_eq!(parse("wider"), Ok(TilingCommand::Wider(true)));
        assert_eq!(parse("narrower"), Ok(TilingCommand::Wider(false)));
        assert_eq!(
            parse("Grid"),
            Ok(TilingCommand::Layout(LayoutChoice::Builtin(
                TileLayout::Grid
            )))
        );
        assert_eq!(
            parse("spiral"),
            Ok(TilingCommand::Layout(LayoutChoice::File(
                "spiral".to_owned()
            )))
        );
        assert!(parse("a/b").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn layouts_of_files_reserving_and_resizing_are_asked_for() {
        assert_eq!(
            parse("tall"),
            Ok(TilingCommand::Layout(LayoutChoice::File("tall".to_owned())))
        );
        assert_eq!(
            parse("bsp"),
            Ok(TilingCommand::Layout(LayoutChoice::Builtin(
                TileLayout::Bsp
            )))
        );
        assert_eq!(
            parse("reserve left"),
            Ok(TilingCommand::Reserve(Reserve::Side(Toward::Left)))
        );
        assert_eq!(
            parse("reserve float"),
            Ok(TilingCommand::Reserve(Reserve::Float))
        );
        assert_eq!(
            parse("split  vertical"),
            Ok(TilingCommand::Reserve(Reserve::Split(Axis::Vertical)))
        );
        assert_eq!(parse("width more"), Ok(TilingCommand::Width(true)));
        assert_eq!(parse("height less"), Ok(TilingCommand::Height(false)));
        assert_eq!(parse("reset"), Ok(TilingCommand::Reset));
        for wrong in [
            "reserve",
            "reserve back",
            "split diagonal",
            "width",
            "width wide",
            "next please",
            "grid now",
        ] {
            assert!(parse(wrong).is_err(), "{wrong}");
        }
    }
}
