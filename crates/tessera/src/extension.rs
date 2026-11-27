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
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::{App, AppContext};
use serde::Deserialize;

use self::access::{Access, Permissions, WatchKind};
use self::host::{Loaded, Worker};
use self::runner::{ANSWER_ACTION, Answering};
use self::runtime::Runtime;
use self::widget::ExtensionWidget;
use crate::actions::{Action, Actions};
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
            settings: self.settings_of(&manifest),
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
            runner::add(Answering::new(
                manifest.id.clone(),
                answers.prefix.clone(),
                worker,
            ));
        }
        Loaded::add(loaded);
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
    /// configuration holds them now: they are kept as what it reads, and
    /// the extension, which keeps running, is told of them through
    /// [`host::Host::update_settings`], so its widgets show them at once.
    pub fn settings_changed(id: &str, table: Option<&toml::Table>, cx: &mut App) {
        let Some(settings) = keep_settings(id, table) else {
            return;
        };
        if let Some(host) = host::running(id, cx) {
            host.update(cx, |host, cx| host.update_settings(settings, cx));
        }
    }
}

/// Keeps the settings `table` as what the extension `id` that was loaded
/// reads, and gives them back; nothing where no extension of that name
/// was loaded.
fn keep_settings(id: &str, table: Option<&toml::Table>) -> Option<HashMap<String, String>> {
    let mut started = STARTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // No extension of that name runs, so none is to be told.
    let (declared, settings) = started.as_mut()?.get_mut(id)?;
    *settings = snapshot(id, declared, table);
    Some(settings.clone())
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
            if let Some(keys) = &shortcut.keys
                && crate::keys::Press::parse(keys).is_none()
            {
                bail!(
                    "the shortcut `{}` has keys `{keys}`, which are no combination Tessera can hold",
                    shortcut.id
                );
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
            include_str!("../../../extensions/system-monitor/extension.toml"),
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
    fn new_settings_are_kept_for_the_extension_that_was_loaded() {
        let manifest = Manifest::parse(&DECLARING.replace("\"probe\"", "\"kept\"")).unwrap();
        let module = ExtensionsModule {
            settings: HashMap::new(),
        };
        assert_eq!(module.settings_of(&manifest)["minutes"], "20");
        let table: toml::Table = toml::from_str("minutes = 30\n").unwrap();
        let told = keep_settings("kept", Some(&table)).unwrap();
        assert_eq!(told["minutes"], "30");
        assert!(keep_settings("not-loaded", Some(&table)).is_none());
        let started = STARTED.lock().unwrap();
        let started = started.as_ref().unwrap();
        assert_eq!(started["kept"].1["minutes"], "30");
        assert_eq!(started["kept"].1["unit"], "celsius");
        assert!(!started.contains_key("not-loaded"));
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
            format!("{}keys = \"win+banana\"\n", shortcut("open", "A")),
            format!("{}keys = \"m\"\n", shortcut("open", "A")),
        ] {
            let text = format!("id = \"a\"\n{shortcuts}");
            assert!(Manifest::parse(&text).is_err(), "{shortcuts} was accepted");
        }
        let keyed = format!(
            "id = \"a\"\n{}keys = \"win+alt+m\"\n",
            shortcut("open", "A")
        );
        assert!(Manifest::parse(&keyed).is_ok());
    }

    #[test]
    fn names_that_could_collide_or_escape_are_rejected() {
        for id in ["", "Counter", "a.b", "../x", "a b"] {
            let text = format!("id = \"{id}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{id:?} was accepted");
        }
    }
}
