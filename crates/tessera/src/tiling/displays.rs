// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Which display an entry of `[[tiling.display]]` names, and what the
//! displays are called.
//!
//! A display is named by the name its monitor gives itself (a pattern
//! with `*` and `?`, or a regular expression after `re:`), by its device
//! name as Windows has it (`\\.\DISPLAY2`), or by its place counted from 1
//! from the left: the [`DisplaySelector`] the `displays` of a `[[surface]]`
//! take, matched against the [`DisplayName`]s the shell reads. What is
//! said here is pure.

pub use crate::config::DisplaySelector;
pub use crate::platform::DisplayName;
use crate::platform::ScreenRect;

// The keys of tiling-state.toml are the tiling's own business, so they
// are given to the platform's name of a display here.
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
/// left to the right, out of `known`, the names the shell last read: the
/// one at the same place from the left. Where `known` does not list as
/// many displays, as while the displays change, a display is named by its
/// place alone, and it is the main one where its top left corner is where
/// the screen begins.
pub fn names(frames: &[ScreenRect], known: &[DisplayName]) -> Vec<DisplayName> {
    let current = known.len() == frames.len();
    { frames.iter().enumerate() }
        .map(|(index, frame)| {
            let position = index + 1;
            let named = known.iter().find(|name| name.position == position);
            match named.filter(|_| current) {
                Some(name) => name.clone(),
                None => DisplayName {
                    device: String::new(),
                    friendly: String::new(),
                    position,
                    primary: frame.left == 0 && frame.top == 0,
                },
            }
        })
        .collect()
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
        let frames = [rect(-1920, 0, 0, 1080), rect(0, 0, 2560, 1440)];
        let unread = names(&frames, &[]);
        assert_eq!(unread[0].position, 1);
        assert!(!unread[0].primary && unread[1].primary);
        assert_eq!(unread[1].key(), "#2");
        // The names read are those of the displays at the same place.
        let read = [
            DisplayName {
                device: r"\\.\DISPLAY3".to_owned(),
                friendly: "DELL U2720Q".to_owned(),
                position: 2,
                primary: true,
            },
            DisplayName {
                device: r"\\.\DISPLAY1".to_owned(),
                friendly: "LG HDR".to_owned(),
                position: 1,
                primary: false,
            },
        ];
        let named = names(&frames, &read);
        assert_eq!(named[1], read[0]);
        assert_eq!(named[1].key(), r"\\.\DISPLAY3");
        assert_eq!(named[1].position_key(), "#2");
        // While the displays change, the places name them.
        assert_eq!(names(&frames, &read[..1])[1].key(), "#2");
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
