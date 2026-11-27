// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which display an entry of `[[tiling.display]]` names, and what the
//! displays are called.
//!
//! A display is named by the name its monitor gives itself (a pattern
//! with `*` and `?`, or a regular expression after `re:`), by its device
//! name as Windows has it (`\\.\DISPLAY2`), or by its place counted from 1
//! from the left; these are the names the `displays` of a `[[surface]]`
//! take. What is said here is pure.

use serde::Deserialize;

use crate::platform::ScreenRect;
use crate::rules::Pattern;

/// What a display is called. The names of the monitors and the device
/// names of Windows are not read yet, so both are empty and only the
/// place of a display names it for now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayName {
    /// The device name, as `\\.\DISPLAY1`.
    pub device: String,
    /// The name the monitor gives itself.
    pub friendly: String,
    /// The place from the left, counted from 1.
    pub position: usize,
    pub primary: bool,
}

impl DisplayName {
    /// What the display is kept under in `tiling-state.toml`: its device
    /// name, or its place where the name is not known.
    pub fn key(&self) -> String {
        match self.device.is_empty() {
            true => self.position_key(),
            false => self.device.clone(),
        }
    }

    /// What the display was kept under before its device name was read:
    /// its place. What was kept under it is taken over by the display now
    /// at that place, the first time it has nothing kept under its device
    /// name.
    pub fn position_key(&self) -> String {
        format!("#{}", self.position)
    }
}

/// The names of the displays whose frames are `frames`, given from the
/// left to the right: their places, and which is the main one, the one
/// whose top left corner is where the screen begins.
pub fn names(frames: &[ScreenRect]) -> Vec<DisplayName> {
    { frames.iter().enumerate() }
        .map(|(index, frame)| DisplayName {
            device: String::new(),
            friendly: String::new(),
            position: index + 1,
            primary: frame.left == 0 && frame.top == 0,
        })
        .collect()
}

/// A way to name displays.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplaySelector {
    /// The place from the left, counted from 1.
    Position(usize),
    /// The device name, in lowercase.
    Device(String),
    /// The name the monitor gives itself.
    Friendly(Pattern),
}

impl DisplaySelector {
    /// The selector `text` is, or why it is none.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a display is named by its name, its device name or its place".into());
        }
        if text.bytes().all(|byte| byte.is_ascii_digit()) {
            return match text.parse::<usize>() {
                Ok(position @ 1..) => Ok(Self::Position(position)),
                _ => Err(format!("the places of displays begin at 1, not at {text}")),
            };
        }
        if text.starts_with(r"\\.\") {
            return Ok(Self::Device(text.to_lowercase()));
        }
        Pattern::parse(text).map(Self::Friendly)
    }

    /// Whether `display` is one the selector names.
    pub fn matches(&self, display: &DisplayName) -> bool {
        match self {
            Self::Position(position) => display.position == *position,
            Self::Device(device) => display.device.eq_ignore_ascii_case(device),
            Self::Friendly(pattern) => pattern.matches(&display.friendly),
        }
    }
}

impl<'de> Deserialize<'de> for DisplaySelector {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn displays_are_named_by_their_place_until_their_names_are_read() {
        let names = names(&[rect(-1920, 0, 0, 1080), rect(0, 0, 2560, 1440)]);
        assert_eq!(names[0].position, 1);
        assert!(!names[0].primary && names[1].primary);
        assert_eq!(names[1].key(), "#2");
        let named = DisplayName {
            device: r"\\.\DISPLAY3".to_owned(),
            ..names[1].clone()
        };
        assert_eq!(named.key(), r"\\.\DISPLAY3");
        assert_eq!(named.position_key(), "#2");
    }

    #[test]
    fn a_selector_names_a_place_a_device_or_a_monitor() {
        let display = DisplayName {
            device: r"\\.\DISPLAY2".to_owned(),
            friendly: "DELL U3423WE".to_owned(),
            position: 2,
            primary: false,
        };
        let matches = |text: &str| DisplaySelector::parse(text).unwrap().matches(&display);
        assert!(matches("2"));
        assert!(!matches("1"));
        assert!(matches(r"\\.\display2"));
        assert!(!matches(r"\\.\DISPLAY1"));
        assert!(matches("dell*"));
        assert!(matches("DELL U????WE"));
        assert!(matches("re:^dell u34"));
        assert!(!matches("LG*"));
        for wrong in ["", "0", " ", "re:("] {
            assert!(DisplaySelector::parse(wrong).is_err(), "{wrong}");
        }
        // A monitor whose name is not known matches `*` only.
        let unnamed = DisplayName {
            friendly: String::new(),
            ..display
        };
        assert!(DisplaySelector::parse("*").unwrap().matches(&unnamed));
        assert!(!DisplaySelector::parse("dell*").unwrap().matches(&unnamed));
    }
}
