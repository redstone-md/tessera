// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Extensions: modules loaded from WebAssembly files, at startup and again
//! whenever those files change.

mod access;
mod devices;
mod host;
pub mod http;
pub mod registry;
mod runtime;
mod widget;

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::{App, AppContext};
use serde::Deserialize;

pub use self::access::DeviceKind;
use self::access::{Access, Permissions};
use self::host::Host;
use self::runtime::Runtime;
use self::widget::ExtensionWidget;
use crate::config::{PopupConfig, config_dir};
use crate::module::{Module, WidgetRegistry};
use crate::options::{OptionDecl, OptionSet, text_of};

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

/// An installed extension as the settings show it: what it is called and
/// what it declares can be set.
pub struct InstalledExtension {
    pub id: String,
    pub settings: OptionSet,
    /// The names of its widgets.
    pub widgets: Vec<String>,
}

impl InstalledExtension {
    /// The names of the widgets the setting `key` affects: those it names,
    /// or every widget of the extension where it names none.
    pub fn affected(&self, key: &str) -> Vec<&str> {
        let named = self.settings.get(key).map(|setting| &setting.affects);
        match named {
            Some(names) if !names.is_empty() => names.iter().map(String::as_str).collect(),
            _ => self.widgets.iter().map(String::as_str).collect(),
        }
    }
}

/// The extensions in the `extensions` configuration directory, by
/// identifier, each with what it declares; one whose manifest cannot be
/// read is left out, and why is reported. It reads files, so it is not
/// to be called on the thread that draws.
pub fn installed_extensions() -> Vec<InstalledExtension> {
    let entries = extensions_dir().and_then(|root| fs::read_dir(root).ok());
    let mut installed = Vec::new();
    for directory in entries.into_iter().flatten().flatten() {
        let path = directory.path().join(MANIFEST_FILE);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        match Manifest::parse(&text) {
            Ok(manifest) => installed.push(InstalledExtension {
                settings: manifest.declared(),
                widgets: { manifest.widgets.iter() }
                    .map(|widget| widget.name.clone())
                    .collect(),
                id: manifest.id,
            }),
            Err(error) => report!("{} is not read: {error:#}", path.display()),
        }
    }
    installed.sort_by(|one, other| one.id.cmp(&other.id));
    installed
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
}

impl ExtensionsModule {
    /// Starts the extension in `directory` and registers its widgets as
    /// `<extension id>.<widget name>`.
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
            settings: self.settings_of(&manifest),
            state_file: Some(directory.join(STATE_FILE)),
            #[cfg(test)]
            bluetooth: None,
        };
        let instance = runtime.instantiate(&manifest.id, &wasm, access)?;
        // The host is an entity, which only a running application can
        // create: the first widget to be placed does.
        let instance = Rc::new(RefCell::new(Some(instance)));
        let host = Rc::new(OnceCell::new());

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
            let (instance, host) = (instance.clone(), host.clone());
            let (extension, name) = (manifest.id.clone(), widget.name.clone());
            let registered_id = id.clone();
            widgets.register(&registered_id, move |_, cx| {
                let host = host
                    .get_or_init(|| {
                        let instance = instance.borrow_mut().take();
                        let instance = instance.expect("the host is created once");
                        cx.new(|_| Host::new(extension.clone(), instance))
                    })
                    .clone();
                let (name, id) = (name.clone(), id.clone());
                cx.new(|cx| ExtensionWidget::new(host, name, id, refresh, cx))
                    .into()
            })?;
        }
        Ok(())
    }

    /// What the configuration sets for the extension of `manifest`, as
    /// text: each value it declares checked against its declaration, or
    /// the declared default where the configuration has none or one that
    /// does not fit, and the keys it does not declare as they are written.
    /// What it was started with is kept, for [`Self::settings_changed`].
    fn settings_of(&self, manifest: &Manifest) -> HashMap<String, String> {
        let declared = manifest.declared();
        let settings = snapshot(&manifest.id, &declared, self.settings.get(&manifest.id));
        let mut started = STARTED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let started = started.get_or_insert_with(HashMap::new);
        started.insert(manifest.id.clone(), (declared, settings.clone()));
        settings
    }

    /// Takes the settings `table` of the extension `id` over, as the
    /// configuration holds them now. Whether the extension running has
    /// them: it does where what it reads did not change, as when a value
    /// is set to its default. Otherwise the answer is `false`, and the
    /// widgets of the extension are to be made again: an extension that
    /// runs is not told of new settings yet.
    pub fn settings_changed(id: &str, table: Option<&toml::Table>, _cx: &mut App) -> bool {
        let started = STARTED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some((declared, settings)) = started.as_ref().and_then(|started| started.get(id))
        else {
            // No extension of that name runs, so none is to be told.
            return true;
        };
        snapshot(id, declared, table) == *settings
    }
}

/// The settings each extension that was loaded was started with, and
/// what it declares, by extension identifier.
type Started = HashMap<String, (OptionSet, HashMap<String, String>)>;
static STARTED: Mutex<Option<Started>> = Mutex::new(None);

/// The settings `table` of the extension `id` as it reads them: what
/// `declared` declares resolved, every value as text. A list is written
/// as its items apart by commas, which `parse_list` of the kit reads.
fn snapshot(
    id: &str,
    declared: &OptionSet,
    table: Option<&toml::Table>,
) -> HashMap<String, String> {
    let empty = toml::Table::new();
    let owner = format!("the extension `{id}`");
    let resolved = declared.resolve(table.unwrap_or(&empty), &owner);
    { resolved.iter() }
        .map(|(key, value)| (key.clone(), text_of(value)))
        .collect()
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
    /// The settings the extension reads, for the settings to show and
    /// to check.
    #[serde(default, rename = "setting")]
    settings: Vec<OptionDecl>,
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
}

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
        for problem in manifest.unknown_affected() {
            report!("the extension `{}`: {problem}", manifest.id);
        }
        Ok(manifest)
    }

    /// The settings that are usable; what is wrong with the others is
    /// reported, and they are left out.
    fn declared(&self) -> OptionSet {
        let (declared, problems) = OptionSet::checked(self.settings.clone());
        for problem in problems {
            report!("the extension `{}`: {problem}", self.id);
        }
        declared
    }

    /// What the settings say they affect that is not a widget of the
    /// extension, or not something a setting of an extension affects.
    fn unknown_affected(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for setting in &self.settings {
            for name in &setting.affects {
                if !self.widgets.iter().any(|widget| widget.name == *name) {
                    problems.push(format!(
                        "the setting `{}` affects `{name}`, which is not a widget of the extension",
                        setting.key
                    ));
                }
            }
            if !setting.scale.is_empty() {
                problems.push(format!(
                    "the setting `{}` scales tokens, which only a theme has",
                    setting.key
                ));
            }
        }
        problems
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
            let manifest = Manifest::parse(manifest).unwrap();
            let (_, problems) = OptionSet::checked(manifest.settings.clone());
            assert!(problems.is_empty(), "{}: {problems:?}", manifest.id);
            let unknown = manifest.unknown_affected();
            assert!(unknown.is_empty(), "{}: {unknown:?}", manifest.id);
        }
    }

    #[test]
    fn settings_reach_an_extension_as_text() {
        let settings = toml::from_str("[probe]\nname = \"value\"\ncount = 3\nshown = true\n");
        let module = ExtensionsModule {
            settings: settings.unwrap(),
        };
        let probe = Manifest::parse("id = \"probe\"\n").unwrap();
        let settings = module.settings_of(&probe);
        assert_eq!(settings["name"], "value");
        assert_eq!(settings["count"], "3");
        assert_eq!(settings["shown"], "true");
        let other = Manifest::parse("id = \"other\"\n").unwrap();
        assert!(module.settings_of(&other).is_empty());
    }

    const DECLARING: &str = concat!(
        "id = \"probe\"\n",
        "[[widget]]\nname = \"now\"\n[[widget]]\nname = \"forecast\"\n",
        "[[setting]]\nkey = \"unit\"\nkind = \"choice\"\nlabel = \"Unit\"\ndefault = \"celsius\"\n",
        "affects = [\"now\"]\n",
        "choices = [{ value = \"celsius\" }, { value = \"fahrenheit\" }]\n",
        "[[setting]]\nkey = \"coins\"\nkind = \"list\"\nlabel = \"Coins\"\n",
        "default = [\"bitcoin\", \"ethereum\"]\n",
        "[[setting]]\nkey = \"minutes\"\nkind = \"number\"\nlabel = \"Minutes\"\n",
        "min = 5\nmax = 60\ndefault = 20\naffects = [\"nowhere\"]\n",
    );

    #[test]
    fn declared_settings_answer_with_their_defaults_and_in_the_form_of_the_kit() {
        let manifest = Manifest::parse(DECLARING).unwrap();
        assert_eq!(manifest.unknown_affected().len(), 1);
        let module = ExtensionsModule {
            settings: toml::from_str("[probe]\nminutes = 900\nunit = \"kelvin\"\n").unwrap(),
        };
        let settings = module.settings_of(&manifest);
        assert_eq!(settings["unit"], "celsius");
        assert_eq!(settings["coins"], "bitcoin, ethereum");
        assert_eq!(settings["minutes"], "60");
        // A list written by hand as an array reaches the kit as its text.
        let listed: toml::Table = toml::from_str("coins = [\"a\", \"b\"]\n").unwrap();
        let declared = manifest.declared();
        assert_eq!(snapshot("probe", &declared, Some(&listed))["coins"], "a, b");
    }

    #[test]
    fn a_setting_affects_the_widgets_it_names_or_all_of_them() {
        let manifest = Manifest::parse(DECLARING).unwrap();
        let installed = InstalledExtension {
            id: manifest.id.clone(),
            settings: manifest.declared(),
            widgets: vec!["now".to_owned(), "forecast".to_owned()],
        };
        assert_eq!(installed.affected("unit"), ["now"]);
        assert_eq!(installed.affected("coins"), ["now", "forecast"]);
    }

    #[test]
    fn names_that_could_collide_or_escape_are_rejected() {
        for id in ["", "Counter", "a.b", "../x", "a b"] {
            let text = format!("id = \"{id}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{id:?} was accepted");
        }
    }
}
