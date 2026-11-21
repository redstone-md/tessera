// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Extensions: modules loaded from WebAssembly files at startup.

mod access;
mod host;
mod http;
mod runtime;
mod widget;

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::AppContext;
use serde::Deserialize;

use self::access::{Access, Permissions};
use self::host::Host;
use self::runtime::Runtime;
use self::widget::ExtensionWidget;
use crate::config::config_dir;
use crate::module::{Module, WidgetRegistry};

const MANIFEST_FILE: &str = "extension.toml";
const WASM_FILE: &str = "extension.wasm";
const STATE_FILE: &str = "state.txt";

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
        let Some(root) = config_dir().map(|dir| dir.join("extensions")) else {
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
                eprintln!(
                    "tessera: the extension in {} is not loaded: {error:#}",
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
        let access = Access {
            permissions: manifest.permissions,
            settings: self.settings_of(&manifest.id),
            state_file: Some(directory.join(STATE_FILE)),
        };
        let instance = runtime.instantiate(&manifest.id, &wasm, access)?;
        // The host is an entity, which only a running application can
        // create: the first widget to be placed does.
        let instance = Rc::new(RefCell::new(Some(instance)));
        let host = Rc::new(OnceCell::new());

        for widget in manifest.widgets {
            let id = format!("{}.{}", manifest.id, widget.name);
            let refresh = widget
                .refresh_seconds
                .map(|seconds| Duration::from_secs(seconds.max(1)));
            let (instance, host) = (instance.clone(), host.clone());
            let extension = manifest.id.clone();
            let registered_id = id.clone();
            widgets.register(&registered_id, move |_, cx| {
                let host = host
                    .get_or_init(|| {
                        let instance = instance.borrow_mut().take();
                        let instance = instance.expect("the host is created once");
                        cx.new(|_| Host::new(extension.clone(), instance))
                    })
                    .clone();
                let (name, id) = (widget.name.clone(), id.clone());
                cx.new(|cx| ExtensionWidget::new(host, name, id, refresh, cx))
                    .into()
            })?;
        }
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
    #[serde(default)]
    permissions: Permissions,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WidgetManifest {
    name: String,
    /// How often the content is asked for again without user input.
    refresh_seconds: Option<u64>,
}

impl Manifest {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        let names = manifest.widgets.iter().map(|widget| &widget.name);
        for name in std::iter::once(&manifest.id).chain(names) {
            if !is_identifier(name) {
                bail!("`{name}` is not a name made of a-z, 0-9 and `-`");
            }
        }
        Ok(manifest)
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
    fn names_that_could_collide_or_escape_are_rejected() {
        for id in ["", "Counter", "a.b", "../x", "a b"] {
            let text = format!("id = \"{id}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{id:?} was accepted");
        }
    }
}
