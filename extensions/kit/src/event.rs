// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the user did to a widget, read from the action `activate` is
//! given.

/// Something the user did to a widget.
///
/// Tessera hands a field, a slider and a toggle to `activate` as an action
/// of the form `<event>:<id>:<value>`; anything else is the action of a
/// button. A button whose action starts like one of those forms is read as
/// that form, so give buttons other actions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event<'a> {
    /// The button with this action was pressed.
    Pressed(&'a str),
    /// The text of the field `id` changed to `text`.
    InputChanged { id: &'a str, text: &'a str },
    /// Enter was pressed in the field `id`, which holds `text`.
    Submitted { id: &'a str, text: &'a str },
    /// The slider `id` was moved to `value`.
    SliderChanged { id: &'a str, value: f32 },
    /// The toggle `id` was switched on or off.
    Toggled { id: &'a str, on: bool },
}

impl<'a> Event<'a> {
    /// What `action` says was done.
    pub fn parse(action: &'a str) -> Self {
        let pressed = Self::Pressed(action);
        let Some((kind, rest)) = action.split_once(':') else {
            return pressed;
        };
        let Some((id, value)) = rest.split_once(':') else {
            return pressed;
        };
        match kind {
            "input-changed" => Self::InputChanged { id, text: value },
            "submitted" => Self::Submitted { id, text: value },
            "slider-changed" => match value.parse() {
                Ok(value) => Self::SliderChanged { id, value },
                Err(_) => pressed,
            },
            "toggled" => match value {
                "true" => Self::Toggled { id, on: true },
                "false" => Self::Toggled { id, on: false },
                _ => pressed,
            },
            _ => pressed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_told_from_buttons() {
        assert_eq!(Event::parse("increment"), Event::Pressed("increment"));
        assert_eq!(
            Event::parse("input-changed:search:a: b"),
            Event::InputChanged {
                id: "search",
                text: "a: b"
            }
        );
        assert_eq!(
            Event::parse("submitted:search:"),
            Event::Submitted {
                id: "search",
                text: ""
            }
        );
        assert_eq!(
            Event::parse("slider-changed:volume:0.25"),
            Event::SliderChanged {
                id: "volume",
                value: 0.25
            }
        );
        assert_eq!(
            Event::parse("toggled:wifi:false"),
            Event::Toggled {
                id: "wifi",
                on: false
            }
        );
    }

    #[test]
    fn a_malformed_input_is_a_press() {
        for action in [
            "slider-changed:volume:loud",
            "toggled:wifi:yes",
            "other:a:b",
            "a:b",
        ] {
            assert_eq!(Event::parse(action), Event::Pressed(action));
        }
    }
}
