// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The options each built-in widget takes, declared as settings, so that the
//! settings can show a control for each and write it into the widget's entry.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::module::WidgetRegistry;
use crate::modules::RunnerCatalog;
use crate::options::{Choice, Label, OptionDecl};
use crate::shell::Shell;

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

/// Choices of an option that only the running shell knows, where the
/// declaration's own list is empty: the runners of a launcher with what
/// the help calls them (their names until the Search page has asked the
/// help), the popups a dock's Start button can open, the time zones of
/// the clock, and the widgets a tile unfolds. None for any other option.
#[allow(
    dead_code,
    reason = "for the settings of widgets, which settings-core draws"
)]
pub fn dynamic(widget: &str, key: &str, cx: &gpui::App) -> Option<Vec<Choice>> {
    // A choice of `value`, shown as `label` where it has one.
    fn choice(value: String, label: Option<String>) -> Choice {
        Choice {
            value: value.into(),
            label: label.map(Label::Plain),
            overlay: None,
        }
    }
    // Choices that are shown as what they write.
    fn named(values: impl Iterator<Item = String>) -> Vec<Choice> {
        values.map(|value| choice(value, None)).collect()
    }
    match (widget, key) {
        ("launcher" | "runner", "runners") => Some(RunnerCatalog::choices(cx)),
        ("windows", "start") => {
            // Those the modules bring and those of the configuration.
            let mut names = { cx.try_global::<WidgetRegistry>() }
                .map(WidgetRegistry::popup_names)
                .unwrap_or_default();
            if cx.has_global::<Shell>() {
                let popups = Shell::config(cx).popups.iter();
                names.extend(popups.map(|popup| popup.name.clone()));
            }
            names.sort();
            names.dedup();
            Some(named(names.into_iter()))
        }
        ("clock", "zones") => Some(
            { crate::time_zones::all().into_iter() }
                .map(|zone| {
                    let value = zone.iana.clone().unwrap_or_else(|| zone.windows_id.clone());
                    choice(value, Some(zone.display))
                })
                .collect(),
        ),
        (_, "detail") => {
            let registry = cx.try_global::<WidgetRegistry>()?;
            let panels = { registry.names().into_iter() }.filter(|name| registry.is_panel(name));
            Some(named(panels))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::config::Config;

    /// The sources of the widgets of Tessera that read options, by file:
    /// every file of a module that asks its `WidgetContext`, and their
    /// parts. A module added later that reads options is added here.
    const READERS: &[(&str, &str)] = &[
        ("files.rs", include_str!("files.rs")),
        ("runners.rs", include_str!("runners.rs")),
        ("runners/web.rs", include_str!("runners/web.rs")),
        ("modules/airplane.rs", include_str!("modules/airplane.rs")),
        (
            "modules/brightness.rs",
            include_str!("modules/brightness.rs"),
        ),
        ("modules/calendar.rs", include_str!("modules/calendar.rs")),
        ("modules/clock.rs", include_str!("modules/clock.rs")),
        ("modules/command.rs", include_str!("modules/command.rs")),
        (
            "modules/connections.rs",
            include_str!("modules/connections.rs"),
        ),
        ("modules/focused.rs", include_str!("modules/focused.rs")),
        ("modules/hdr.rs", include_str!("modules/hdr.rs")),
        ("modules/hotspot.rs", include_str!("modules/hotspot.rs")),
        ("modules/label.rs", include_str!("modules/label.rs")),
        ("modules/launcher.rs", include_str!("modules/launcher.rs")),
        (
            "modules/launcher/views.rs",
            include_str!("modules/launcher/views.rs"),
        ),
        (
            "modules/marketplace.rs",
            include_str!("modules/marketplace.rs"),
        ),
        ("modules/media.rs", include_str!("modules/media.rs")),
        ("modules/network.rs", include_str!("modules/network.rs")),
        ("modules/overview.rs", include_str!("modules/overview.rs")),
        ("modules/plan.rs", include_str!("modules/plan.rs")),
        (
            "modules/popup_settings.rs",
            include_str!("modules/popup_settings.rs"),
        ),
        ("modules/quick.rs", include_str!("modules/quick.rs")),
        ("modules/runner.rs", include_str!("modules/runner.rs")),
        (
            "modules/runner_settings.rs",
            include_str!("modules/runner_settings.rs"),
        ),
        ("modules/sound.rs", include_str!("modules/sound.rs")),
        ("modules/spacer.rs", include_str!("modules/spacer.rs")),
        ("modules/start.rs", include_str!("modules/start.rs")),
        (
            "modules/start/recommended.rs",
            include_str!("modules/start/recommended.rs"),
        ),
        ("modules/system.rs", include_str!("modules/system.rs")),
        ("modules/tray.rs", include_str!("modules/tray.rs")),
        ("modules/update.rs", include_str!("modules/update.rs")),
        ("modules/user.rs", include_str!("modules/user.rs")),
        ("modules/windows.rs", include_str!("modules/windows.rs")),
        (
            "modules/windows/options.rs",
            include_str!("modules/windows/options.rs"),
        ),
        (
            "modules/workspaces.rs",
            include_str!("modules/workspaces.rs"),
        ),
    ];

    /// The keys a widget reads that are no option of the layout: what the
    /// settings give a page of theirs (`Settings::open_at`).
    const NOT_OPTIONS: &[&str] = &["item"];

    /// The keys `source` asks a `WidgetContext` for: those of
    /// `.text("…")`, `.flag("…")`, `.number("…")` and `.texts("…")`, and
    /// of `one_of(context, "…"`, with any spaces and line breaks between.
    fn keys_read(source: &str) -> Vec<String> {
        let mut keys = Vec::new();
        let mut take = |rest: &str| {
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('"') else {
                return;
            };
            if let Some(end) = rest.find('"') {
                keys.push(rest[..end].to_owned());
            }
        };
        for call in [".text(", ".flag(", ".number(", ".texts("] {
            for (at, _) in source.match_indices(call) {
                take(&source[at + call.len()..]);
            }
        }
        for (at, _) in source.match_indices("one_of(") {
            let rest = source[at + "one_of(".len()..].trim_start();
            if let Some(rest) = rest.strip_prefix("context")
                && let Some(rest) = rest.trim_start().strip_prefix(',')
            {
                take(rest);
            }
        }
        keys.sort();
        keys.dedup();
        keys
    }

    /// Every declaration of every file, with the widget it is of.
    fn every_declaration() -> Vec<(String, OptionDecl)> {
        let mut all = Vec::new();
        for (_, text) in FILES {
            let file: WidgetOptionsFile = toml::from_str(text).expect("the file parses");
            for widget in file.widget {
                for setting in widget.setting {
                    all.push((widget.name.clone(), setting));
                }
            }
        }
        all
    }

    #[test]
    fn the_scanner_finds_the_keys_a_widget_reads() {
        let source = r#"
            let a = context.text("format").unwrap_or(DEFAULT);
            let b = context
                .flag( "week_numbers")
                .unwrap_or(false);
            let c = one_of(
                context,
                "style",
                &["wave", "text"],
            );
            let d = one_of(other, "not_this", &[]);
            let e = context.texts(KEY);
        "#;
        assert_eq!(keys_read(source), ["format", "style", "week_numbers"]);
    }

    #[test]
    fn every_declared_widget_is_one_tessera_registers() {
        let config: Config = toml::from_str(include_str!("default_config.toml"))
            .expect("the built-in layout parses");
        // The extensions installed on the machine are not Tessera's own.
        let modules: Vec<_> = { crate::modules::builtin(&config).into_iter() }
            .filter(|module| module.name() != "extensions")
            .collect();
        let names: HashSet<String> = WidgetRegistry::from_modules(&modules)
            .names()
            .into_iter()
            .collect();
        for (widget, setting) in every_declaration() {
            assert!(
                widget == EVERY_WIDGET || names.contains(&widget),
                "`{}` is declared for `{widget}`, which no module registers",
                setting.key
            );
        }
    }

    #[test]
    fn every_default_is_valid() {
        for (widget, setting) in every_declaration() {
            if let Some(default) = &setting.default {
                let valid = setting.validate(default);
                assert_eq!(
                    valid.as_ref(),
                    Ok(default),
                    "the default of `{}` of `{widget}` is not what it would write",
                    setting.key
                );
            }
        }
    }

    #[test]
    fn no_widget_declares_one_option_twice() {
        let declarations = every_declaration();
        let widgets: HashSet<&str> = { declarations.iter() }
            .map(|(widget, _)| widget.as_str())
            .collect();
        for widget in widgets {
            let declared = of(widget);
            let mut seen = HashSet::new();
            for setting in &declared {
                assert!(
                    seen.insert(setting.key.as_str()),
                    "`{}` is declared twice for `{widget}`, with those of `*`",
                    setting.key
                );
            }
        }
    }

    #[test]
    fn every_option_a_widget_reads_is_declared() {
        let declared: HashSet<String> = { every_declaration().into_iter() }
            .map(|(_, setting)| setting.key)
            .collect();
        for (file, source) in READERS {
            for key in keys_read(source) {
                assert!(
                    declared.contains(&key) || NOT_OPTIONS.contains(&key.as_str()),
                    "{file} reads the option `{key}`, which no widget declares"
                );
            }
        }
    }

    #[test]
    fn every_text_is_translated() {
        // A language file gives exactly the texts of the table, which a
        // test of i18n.rs checks.
        let file: toml::Table =
            toml::from_str(include_str!("../../../languages/de.toml")).expect("it parses");
        let texts = file["texts"].as_table().expect("it has texts");
        let plain = |label: &Option<Label>| match label {
            Some(Label::Plain(text)) => Some(text.clone()),
            _ => None,
        };
        for (widget, setting) in every_declaration() {
            let mut said = vec![plain(&Some(setting.label.clone()))];
            said.extend([&setting.description, &setting.unset_label].map(plain));
            said.extend(setting.choices.iter().map(|choice| plain(&choice.label)));
            for text in said.into_iter().flatten() {
                assert!(
                    texts.contains_key(&text),
                    "`{text}` of `{}` of `{widget}` is not in the table of texts",
                    setting.key
                );
            }
        }
    }

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
