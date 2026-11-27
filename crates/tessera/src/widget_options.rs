// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The options each built-in widget takes, declared as settings, so that the
//! settings can show a control for each and write it into the widget's entry.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::options::{Choice, OptionDecl};

/// What one file of widget options holds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetOptionsFile {
    #[serde(default)]
    widget: Vec<WidgetOptions>,
}

/// The options of one widget.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetOptions {
    /// The widget's name, or `*` for options every widget takes.
    name: String,
    #[serde(default)]
    setting: Vec<OptionDecl>,
}

/// The name that stands for every widget.
const EVERY_WIDGET: &str = "*";

/// Every file, by the package that fills it. Lines are only added.
const FILES: &[(&str, &str)] = &[
    (
        "widget-options",
        include_str!("widget_options/widget-options.toml"),
    ),
    ("dock-pins", include_str!("widget_options/dock-pins.toml")),
    (
        "theme-styles",
        include_str!("widget_options/theme-styles.toml"),
    ),
    (
        "desktop-fidelity",
        include_str!("widget_options/desktop-fidelity.toml"),
    ),
    (
        "files-terminal",
        include_str!("widget_options/files-terminal.toml"),
    ),
    (
        "spectacle-klipper",
        include_str!("widget_options/spectacle-klipper.toml"),
    ),
];

/// The widgets of every file that parses, in the order of [`FILES`]; a
/// file that does not parse is reported, once, and left out.
fn declared() -> &'static [WidgetOptions] {
    static DECLARED: OnceLock<Vec<WidgetOptions>> = OnceLock::new();
    DECLARED.get_or_init(|| {
        let mut widgets = Vec::new();
        for (package, text) in FILES {
            match toml::from_str::<WidgetOptionsFile>(text) {
                Ok(file) => widgets.extend(file.widget),
                Err(error) => {
                    report!("the widget options of {package} are not read: {error}");
                }
            }
        }
        widgets
    })
}

/// The declared options of `widget`: its own, then those of `*`, parsed once
/// and kept.
#[allow(
    dead_code,
    reason = "for the settings of widgets, which settings-core draws"
)]
pub fn of(widget: &str) -> Vec<OptionDecl> {
    let own = { declared().iter() }.filter(|options| options.name == widget);
    let every = { declared().iter() }.filter(|options| options.name == EVERY_WIDGET);
    let own = own.filter(|_| widget != EVERY_WIDGET);
    { own.chain(every) }
        .flat_map(|options| options.setting.iter().cloned())
        .collect()
}

/// Choices of an option that only the running shell knows (runner ids, popup
/// names, time zones), where the declaration's own list is empty. None here.
#[allow(
    dead_code,
    reason = "for the settings of widgets, which settings-core draws"
)]
pub fn dynamic(_widget: &str, _key: &str, _cx: &gpui::App) -> Option<Vec<Choice>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_widget_options_file_parses() {
        for (package, text) in FILES {
            let file: WidgetOptionsFile = toml::from_str(text).unwrap_or_else(|error| {
                panic!("the widget options of {package} do not parse: {error}")
            });
            for widget in &file.widget {
                assert!(!widget.name.is_empty(), "{package}: a widget has no name");
                for setting in &widget.setting {
                    setting.check().unwrap_or_else(|error| {
                        panic!("{package}: `{}` of `{}` {error}", setting.key, widget.name)
                    });
                }
            }
        }
    }
}
