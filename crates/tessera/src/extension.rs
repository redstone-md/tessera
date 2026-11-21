// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Extensions: modules loaded from WebAssembly files at startup.

mod runtime;
mod widget;

use std::cell::RefCell;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, bail};
use gpui::AppContext;
use serde::Deserialize;

use self::runtime::Runtime;
use self::widget::ExtensionWidget;
use crate::config::config_dir;
use crate::module::{Module, WidgetRegistry};

const MANIFEST_FILE: &str = "extension.toml";
const WASM_FILE: &str = "extension.wasm";

/// Loads every extension in the `extensions` configuration directory. Each
/// one is a directory holding a manifest and a WebAssembly file.
pub struct ExtensionsModule;

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
            if let Err(error) = load(&runtime, &path, widgets) {
                eprintln!(
                    "tessera: the extension in {} is not loaded: {error:#}",
                    path.display()
                );
            }
        }
        Ok(())
    }
}

/// Describes an extension to the shell.
#[derive(Debug, Deserialize)]
struct Manifest {
    /// Prefixes the extension's widget names in layouts.
    id: String,
    #[serde(default, rename = "widget")]
    widgets: Vec<WidgetManifest>,
}

#[derive(Debug, Deserialize)]
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

/// Starts the extension in `directory` and registers its widgets as
/// `<extension id>.<widget name>`.
fn load(runtime: &Runtime, directory: &Path, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
    let manifest = fs::read_to_string(directory.join(MANIFEST_FILE))
        .with_context(|| format!("cannot read {MANIFEST_FILE}"))?;
    let manifest =
        Manifest::parse(&manifest).with_context(|| format!("invalid {MANIFEST_FILE}"))?;
    let wasm =
        fs::read(directory.join(WASM_FILE)).with_context(|| format!("cannot read {WASM_FILE}"))?;
    let instance = Rc::new(RefCell::new(runtime.instantiate(&manifest.id, &wasm)?));

    for widget in manifest.widgets {
        let id = format!("{}.{}", manifest.id, widget.name);
        let refresh = widget
            .refresh_seconds
            .map(|seconds| Duration::from_secs(seconds.max(1)));
        let instance = instance.clone();
        let registered_id = id.clone();
        widgets.register(&registered_id, move |_, cx| {
            let (instance, name, id) = (instance.clone(), widget.name.clone(), id.clone());
            cx.new(|cx| ExtensionWidget::new(instance, name, id, refresh, cx))
                .into()
        })?;
    }
    Ok(())
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
    fn names_that_could_collide_or_escape_are_rejected() {
        for id in ["", "Counter", "a.b", "../x", "a b"] {
            let text = format!("id = \"{id}\"\n");
            assert!(Manifest::parse(&text).is_err(), "{id:?} was accepted");
        }
    }
}
