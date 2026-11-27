// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Extensions: modules loaded from WebAssembly files, at startup and again
//! whenever those files change.

mod access;
mod devices;
mod host;
pub mod http;
mod images;
mod nodes;
pub mod registry;
pub mod runner;
mod runtime;
mod system;
mod widget;
mod windows_api;

use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::AppContext;
use serde::Deserialize;

pub use self::access::DeviceKind;
use self::access::{Access, Permissions, WatchKind};
use self::host::{Loaded, Worker};
use self::runner::{ANSWER_ACTION, Answering};
use self::runtime::Runtime;
use self::widget::ExtensionWidget;
use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, config_dir};
use crate::module::{Module, WidgetRegistry};

const MANIFEST_FILE: &str = "extension.toml";
const WASM_FILE: &str = "extension.wasm";
const STATE_FILE: &str = "state.txt";

/// The directory extensions are installed in.
pub fn extensions_dir() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("extensions"))
}

/// What the installed extensions are made of: it differs after the
/// manifest or the WebAssembly file of any of them was written, added or
/// removed. What an extension remembers by itself does not count.
pub fn installed_stamp() -> u64 {
    let mut files = Vec::new();
    let entries = extensions_dir().and_then(|root| fs::read_dir(root).ok());
    for directory in entries.into_iter().flatten().flatten() {
        for name in [MANIFEST_FILE, WASM_FILE] {
            let path = directory.path().join(name);
            if let Ok(written) = fs::metadata(&path).and_then(|file| file.modified()) {
                files.push((path, written));
            }
        }
    }
    files.sort();
    let mut hasher = DefaultHasher::new();
    files.hash(&mut hasher);
    hasher.finish()
}

/// Loads every extension in the `extensions` configuration directory. Each
/// one is a directory holding a manifest and a WebAssembly file.
pub struct ExtensionsModule {
    /// The `[extension.<id>]` tables of the configuration.
    pub settings: HashMap<String, toml::Table>,
}

impl Module for ExtensionsModule {
    fn name(&self) -> &'static str {
        "extensions"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        // Those loaded before end once nothing shows them any more.
        Loaded::clear();
        runner::clear();
        let Some(root) = extensions_dir() else {
            return Ok(());
        };
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context(format!("cannot list {}", root.display())),
        };
        let runtime = Runtime::new().context("the WebAssembly runtime does not start")?;
        for path in entries.flatten().map(|entry| entry.path()) {
            if !path.is_dir() {
                continue;
            }
            if let Err(error) = self.load(&runtime, &path, widgets) {
                report!(
                    "the extension in {} is not loaded: {error:#}",
                    path.display()
                );
            }
        }
        Ok(())
    }

    /// The shortcuts of the installed extensions as the actions
    /// `<extension id>.<shortcut id>`, bound to their keys by default, and
    /// the action that calls an extension back when one of its answers is
    /// chosen in the launcher. Extensions installed later have their
    /// shortcuts from the next start.
    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        let chosen = Action::new(|arg, cx| match runner::chosen(arg) {
            Ok((extension, answer)) => match Loaded::get(extension) {
                Some(loaded) => {
                    let host = loaded.host(cx);
                    host.update(cx, |host, cx| host.run_answer(answer, cx));
                }
                None => report!("the extension `{extension}` is not loaded"),
            },
            Err(error) => report!("{error}"),
        })
        .checking(|arg| runner::chosen(arg).map(drop));
        actions.register(ANSWER_ACTION, chosen)?;
        for manifest in installed_manifests() {
            for shortcut in &manifest.shortcuts {
                let name = manifest.qualified(&shortcut.id);
                let (extension, id) = (manifest.id.clone(), shortcut.id.clone());
                let mut action = Action::new(move |_, cx| match Loaded::get(&extension) {
                    Some(loaded) => {
                        let host = loaded.host(cx);
                        host.update(cx, |host, cx| host.shortcut(&id, cx));
                    }
                    None => report!("the extension `{extension}` is not loaded"),
                });
                if let Some(keys) = &shortcut.keys {
                    action = action.by_default(keys, None);
                }
                if let Err(error) = actions.register(&name, action) {
                    report!("the shortcut `{name}` is left out: {error:#}");
                }
            }
        }
        Ok(())
    }
}

/// The manifests of the installed extensions that can be read.
fn installed_manifests() -> Vec<Manifest> {
    let entries = extensions_dir().and_then(|root| fs::read_dir(root).ok());
    let manifests = entries.into_iter().flatten().flatten().filter_map(|entry| {
        let text = fs::read_to_string(entry.path().join(MANIFEST_FILE)).ok()?;
        Manifest::parse(&text).ok()
    });
    manifests.collect()
}

impl ExtensionsModule {
    /// Starts the extension in `directory` on a thread of its own and
    /// registers its widgets as `<extension id>.<widget name>`; it answers
    /// the launcher where its manifest has a `[runner]` table.
    fn load(
        &self,
        runtime: &Runtime,
        directory: &Path,
        widgets: &mut WidgetRegistry,
    ) -> anyhow::Result<()> {
        let manifest = fs::read_to_string(directory.join(MANIFEST_FILE))
            .with_context(|| format!("cannot read {MANIFEST_FILE}"))?;
        let manifest =
            Manifest::parse(&manifest).with_context(|| format!("invalid {MANIFEST_FILE}"))?;
        let wasm = fs::read(directory.join(WASM_FILE))
            .with_context(|| format!("cannot read {WASM_FILE}"))?;
        let popups = manifest.popups();
        let access = Access {
            permissions: manifest.permissions.clone(),
            settings: self.settings_of(&manifest.id),
            state_file: Some(directory.join(STATE_FILE)),
            popups: manifest
                .popups
                .iter()
                .map(|popup| popup.name.clone())
                .collect(),
            #[cfg(test)]
            bluetooth: None,
        };
        let instance = runtime.instantiate(&manifest.id, &wasm, access)?;
        let worker = Worker::start(&manifest.id, instance)
            .context("the thread of the extension does not start")?;
        // The host is an entity, which only a running application can
        // create: the first widget to be placed, shortcut pressed or
        // answer chosen does.
        let loaded = Loaded::new(&manifest.id, worker.clone(), manifest.permissions.system());

        for popup in popups {
            widgets.register_popup(popup)?;
        }
        for widget in &manifest.widgets {
            let id = manifest.qualified(&widget.name);
            if let Some(popup) = &widget.popup {
                widgets.open_with(&id, &manifest.qualified(popup));
            }
            let refresh = widget
                .refresh_seconds
                .map(|seconds| Duration::from_secs(seconds.max(1)));
            let (loaded, name, watch) = (loaded.clone(), widget.name.clone(), widget.watch.clone());
            let registered_id = id.clone();
            widgets.register(&registered_id, move |_, cx| {
                let host = loaded.host(cx);
                let (name, id, watch) = (name.clone(), id.clone(), watch.clone());
                cx.new(|cx| ExtensionWidget::new(host, name, id, refresh, watch, cx))
                    .into()
            })?;
        }
        if let Some(answers) = &manifest.runner {
            runner::add(Answering {
                id: manifest.id.clone(),
                prefix: answers.prefix.clone(),
                worker,
            });
        }
        Loaded::add(loaded);
        Ok(())
    }

    /// What the configuration sets for the extension `id`, as text.
    fn settings_of(&self, id: &str) -> HashMap<String, String> {
        let table = self.settings.get(id).into_iter().flatten();
        table
            .map(|(key, value)| {
                let text = match value.as_str() {
                    Some(text) => text.to_owned(),
                    None => value.to_string(),
                };
                (key.clone(), text)
            })
            .collect()
    }
}

/// Describes an extension to the shell.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    /// Prefixes the extension's widget names in layouts.
    id: String,
    #[serde(default, rename = "widget")]
    widgets: Vec<WidgetManifest>,
    /// Popups the extension brings along, written as in a layout. Layouts
    /// know them as `<id>.<name>`.
    #[serde(default, rename = "popup")]
    popups: Vec<PopupConfig>,
    #[serde(default)]
    permissions: Permissions,
    /// Has the extension answer what is typed into the launcher.
    runner: Option<RunnerManifest>,
    /// Keys the extension is told of, as `<id>.<shortcut id>` actions.
    #[serde(default, rename = "shortcut")]
    shortcuts: Vec<ShortcutManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetManifest {
    name: String,
    /// How often the content is asked for again without user input.
    refresh_seconds: Option<u64>,
    /// The popup of this extension that a click on the widget opens,
    /// unless the layout says otherwise.
    popup: Option<String>,
    /// What the widget is drawn again for when it changes.
    #[serde(default)]
    watch: Vec<WatchKind>,
}

/// How an extension answers the launcher.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnerManifest {
    /// What a query starts with to be handed to the extension, without
    /// it; without a prefix every query is.
    prefix: Option<String>,
}

/// Keys an extension is told of when they are pressed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShortcutManifest {
    /// Handed to `shortcut`; made of a-z, 0-9 and `-`.
    id: String,
    /// What pressing it does, for whoever reads the manifest.
    label: String,
    /// The keys it has unless the user binds them otherwise, as a
    /// `[[key]]` writes them, such as `win+alt+m`.
    keys: Option<String>,
}

/// The longest prefix of a runner.
const PREFIX_LIMIT: usize = 16;

impl Manifest {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        let widgets = manifest.widgets.iter().map(|widget| &widget.name);
        let popups = manifest.popups.iter().map(|popup| &popup.name);
        for name in std::iter::once(&manifest.id).chain(widgets).chain(popups) {
            if !is_identifier(name) {
                bail!("`{name}` is not a name made of a-z, 0-9 and `-`");
            }
        }
        for popup in &manifest.popups {
            let sized = |length: f32| length.is_finite() && length >= 1.;
            if !sized(popup.width) || !sized(popup.height) {
                bail!("the popup `{}` has no usable size", popup.name);
            }
        }
        for widget in &manifest.widgets {
            if let Some(popup) = &widget.popup
                && !manifest.popups.iter().any(|known| known.name == *popup)
            {
                bail!(
                    "the widget `{}` opens `{popup}`, which is not a popup of the extension",
                    widget.name
                );
            }
        }
        manifest.check_answers_and_keys()?;
        Ok(manifest)
    }

    /// Fails where what the widgets watch is not permitted, or a runner or
    /// a shortcut is not usable.
    fn check_answers_and_keys(&self) -> anyhow::Result<()> {
        for widget in &self.widgets {
            for kind in &widget.watch {
                if let Err(error) = self.permissions.check_system(kind.needs()) {
                    bail!("the widget `{}` cannot watch: {error}", widget.name);
                }
            }
        }
        let prefix = self
            .runner
            .as_ref()
            .and_then(|runner| runner.prefix.as_ref());
        if let Some(prefix) = prefix {
            let usable = !prefix.is_empty()
                && prefix.chars().count() <= PREFIX_LIMIT
                && !prefix.contains(char::is_whitespace);
            if !usable {
                bail!(
                    "the runner prefix `{prefix}` is not up to {PREFIX_LIMIT} letters without spaces"
                );
            }
        }
        let mut ids = Vec::new();
        for shortcut in &self.shortcuts {
            if !is_identifier(&shortcut.id) {
                bail!("`{}` is not a name made of a-z, 0-9 and `-`", shortcut.id);
            }
            if ids.contains(&&shortcut.id) {
                bail!("two shortcuts are named `{}`", shortcut.id);
            }
            if shortcut.label.trim().is_empty() {
                bail!("the shortcut `{}` has no label", shortcut.id);
            }
            if { shortcut.keys.as_deref() }.is_some_and(|keys| keys.trim().is_empty()) {
                bail!("the shortcut `{}` has empty keys", shortcut.id);
            }
            ids.push(&shortcut.id);
        }
        Ok(())
    }

    /// The name layouts know the widget or popup `name` by.
    fn qualified(&self, name: &str) -> String {
        format!("{}.{name}", self.id)
    }

    /// The popups as layouts know them. A widget in their content that
    /// the extension provides is named in full; any other name is kept,
    /// so a popup can also hold widgets of the shell.
    fn popups(&self) -> Vec<PopupConfig> {
        let popups = self.popups.iter().cloned().map(|mut popup| {
            popup.name = self.qualified(&popup.name);
            for spec in popup.content.iter_mut().chain(&mut popup.side) {
                if self.widgets.iter().any(|own| own.name == spec.widget) {
                    spec.widget = self.qualified(&spec.widget);
                }
            }
            popup
        });
        popups.collect()
    }
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_lists_widgets() {
        let manifest = Manifest::parse(
            "id = \"counter\"\n[[widget]]\nname = \"clicks\"\nrefresh_seconds = 5\n",
        )
        .unwrap();
        assert_eq!(manifest.id, "counter");
        assert_eq!(manifest.widgets[0].name, "clicks");
        assert_eq!(manifest.widgets[0].refresh_seconds, Some(5));
    }

    #[test]
    fn a_manifest_brings_popups_that_its_widgets_open() {
        let manifest = Manifest::parse(concat!(
            "id = \"lights\"\n",
            "[[widget]]\nname = \"button\"\npopup = \"panel\"\n",
            "[[widget]]\nname = \"controls\"\n",
            "[[popup]]\nname = \"panel\"\nwidth = 300\nheight = 200\n",
            "content = [\"controls\", \"clock\", { widget = \"other.thing\", size = 2 }]\n",
        ))
        .unwrap();
        assert_eq!(manifest.widgets[0].popup.as_deref(), Some("panel"));
        let [popup] = manifest.popups().try_into().unwrap();
        assert_eq!(popup.name, "lights.panel");
        let names: Vec<&str> = popup.content.iter().map(|spec| &*spec.widget).collect();
        assert_eq!(names, ["lights.controls", "clock", "other.thing"]);
    }

    #[test]
    fn a_manifest_with_a_popup_that_cannot_open_is_rejected() {
        let missing = "id = \"a\"\n[[widget]]\nname = \"b\"\npopup = \"c\"\n";
        assert!(Manifest::parse(missing).is_err());
        let flat = "id = \"a\"\n[[popup]]\nname = \"c\"\nwidth = 300\nheight = 0\n";
        assert!(Manifest::parse(flat).is_err());
    }

    #[test]
    fn the_manifests_of_the_examples_are_valid() {
        for manifest in [
            include_str!("../../../extensions/counter/extension.toml"),
            include_str!("../../../extensions/ai-limits/extension.toml"),
            include_str!("../../../extensions/weather/extension.toml"),
            include_str!("../../../extensions/device-battery/extension.toml"),
            include_str!("../../../extensions/pomodoro/extension.toml"),
            include_str!("../../../extensions/crypto/extension.toml"),
            include_str!("../../../extensions/exchange/extension.toml"),
            include_str!("../../../extensions/hacker-news/extension.toml"),
            include_str!("../../../extensions/moon/extension.toml"),
            include_str!("../../../extensions/countdown/extension.toml"),
            include_str!("../../../extensions/world-clock/extension.toml"),
            include_str!("../../../extensions/habits/extension.toml"),
            include_str!("../../../extensions/github-stats/extension.toml"),
            include_str!("../../../extensions/air-quality/extension.toml"),
            include_str!("../../../extensions/dice/extension.toml"),
            include_str!("../../../extensions/breaks/extension.toml"),
        ] {
            Manifest::parse(manifest).unwrap();
        }
    }

    #[test]
    fn settings_reach_an_extension_as_text() {
        let settings = toml::from_str("[probe]\nname = \"value\"\ncount = 3\nshown = true\n");
        let module = ExtensionsModule {
            settings: settings.unwrap(),
        };
        let settings = module.settings_of("probe");
        assert_eq!(settings["name"], "value");
        assert_eq!(settings["count"], "3");
        assert_eq!(settings["shown"], "true");
        assert!(module.settings_of("other").is_empty());
    }

    #[test]
    fn a_manifest_of_0_4_watches_answers_and_has_keys() {
        let manifest = Manifest::parse(concat!(
            "id = \"monitor\"\n",
            "[[widget]]\nname = \"meters\"\nwatch = [\"meters\", \"windows\"]\n",
            "[runner]\nprefix = \"sys\"\n",
            "[[shortcut]]\nid = \"panel\"\nlabel = \"Open\"\nkeys = \"win+alt+m\"\n",
            "[[shortcut]]\nid = \"other\"\nlabel = \"Other\"\n",
            "[permissions]\nsystem = [\"meters\", \"windows\"]\n",
        ))
        .unwrap();
        assert_eq!(
            manifest.widgets[0].watch,
            [WatchKind::Meters, WatchKind::Windows]
        );
        let prefix = manifest
            .runner
            .as_ref()
            .and_then(|runner| runner.prefix.as_deref());
        assert_eq!(prefix, Some("sys"));
        assert_eq!(manifest.shortcuts[0].keys.as_deref(), Some("win+alt+m"));
        assert_eq!(manifest.shortcuts[1].keys, None);
    }

    #[test]
    fn what_a_manifest_of_0_4_cannot_use_is_rejected() {
        let watching = "id = \"a\"\n[[widget]]\nname = \"b\"\nwatch = [\"media\"]\n";
        assert!(Manifest::parse(watching).is_err(), "media is not permitted");
        let permitted = format!("{watching}[permissions]\nsystem = [\"media\"]\n");
        assert!(Manifest::parse(&permitted).is_ok());
        let unknown = "id = \"a\"\n[[widget]]\nname = \"b\"\nwatch = [\"registry\"]\n";
        assert!(Manifest::parse(unknown).is_err());
        for prefix in ["", "two words", "a-prefix-too-long-to-type"] {
            let text = format!("id = \"a\"\n[runner]\nprefix = \"{prefix}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{prefix:?} was accepted");
        }
        assert!(Manifest::parse("id = \"a\"\n[runner]\n").is_ok());
        let shortcut =
            |id: &str, label: &str| format!("[[shortcut]]\nid = \"{id}\"\nlabel = \"{label}\"\n");
        for shortcuts in [
            shortcut("Open", "Open"),
            shortcut("open", " "),
            format!("{}{}", shortcut("open", "A"), shortcut("open", "B")),
            format!("{}keys = \"\"\n", shortcut("open", "A")),
        ] {
            let text = format!("id = \"a\"\n{shortcuts}");
            assert!(Manifest::parse(&text).is_err(), "{shortcuts} was accepted");
        }
    }

    #[test]
    fn names_that_could_collide_or_escape_are_rejected() {
        for id in ["", "Counter", "a.b", "../x", "a b"] {
            let text = format!("id = \"{id}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{id:?} was accepted");
        }
    }
}
