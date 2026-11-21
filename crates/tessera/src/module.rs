// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The seam between the shell and the features plugged into it.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::bail;
use gpui::{AnyView, App, Global};

use crate::config::WidgetSpec;
use crate::platform::DisplayKey;

/// A unit of shell functionality. Everything a surface shows comes from one.
pub trait Module {
    /// A short name for diagnostics.
    fn name(&self) -> &'static str;

    /// Contributes the module's widgets.
    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()>;
}

/// Where and how a widget instance is being placed.
#[derive(Clone, Debug)]
pub struct WidgetContext {
    /// The display of the hosting surface, when known.
    pub display: Option<DisplayKey>,
    /// What the layout says about this instance, beyond its name.
    pub options: toml::Table,
}

impl WidgetContext {
    /// The text option `key`. A value of another type is reported and
    /// treated as absent.
    pub fn text(&self, key: &str) -> Option<&str> {
        let value = self.options.get(key)?;
        let text = value.as_str();
        if text.is_none() {
            eprintln!("tessera: the widget option `{key}` must be text, `{value}` is ignored");
        }
        text
    }

    /// The number option `key`. A value of another type is reported and
    /// treated as absent.
    pub fn number(&self, key: &str) -> Option<f64> {
        let value = self.options.get(key)?;
        let number = value
            .as_float()
            .or_else(|| value.as_integer().map(|whole| whole as f64));
        if number.is_none() {
            eprintln!("tessera: the widget option `{key}` must be a number, `{value}` is ignored");
        }
        number
    }

    /// The option `key` as a list of texts. Anything else is reported and
    /// treated as absent.
    pub fn texts(&self, key: &str) -> Option<Vec<String>> {
        let value = self.options.get(key)?;
        let texts = value.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
        });
        if texts.is_none() {
            eprintln!(
                "tessera: the widget option `{key}` must be a list of texts, `{value}` is ignored"
            );
        }
        texts
    }
}

type WidgetFactory = Rc<dyn Fn(&WidgetContext, &mut App) -> AnyView>;

/// The widgets a layout can refer to, by identifier.
#[derive(Default)]
pub struct WidgetRegistry {
    factories: HashMap<String, WidgetFactory>,
}

impl WidgetRegistry {
    /// Collects the widgets of `modules`. A module that fails to register is
    /// reported and skipped; widgets it added before failing stay available.
    pub fn from_modules(modules: &[Box<dyn Module>]) -> Self {
        let mut registry = Self::default();
        for module in modules {
            if let Err(error) = module.register(&mut registry) {
                eprintln!(
                    "tessera: module `{}` is not fully loaded: {error:#}",
                    module.name()
                );
            }
        }
        registry
    }

    /// Makes `factory` available to layouts as `id`.
    pub fn register(
        &mut self,
        id: &str,
        factory: impl Fn(&WidgetContext, &mut App) -> AnyView + 'static,
    ) -> anyhow::Result<()> {
        if self.factories.contains_key(id) {
            bail!("a widget named `{id}` is already registered");
        }
        self.factories.insert(id.to_owned(), Rc::new(factory));
        Ok(())
    }

    /// Builds a new instance of the widget `spec` names for a surface on
    /// `display`. A name no module provides is reported.
    pub fn build(spec: &WidgetSpec, display: Option<DisplayKey>, cx: &mut App) -> Option<AnyView> {
        let Some(factory) = cx.global::<Self>().factories.get(&spec.widget).cloned() else {
            eprintln!("tessera: no module provides the widget `{}`", spec.widget);
            return None;
        };
        let context = WidgetContext {
            display,
            options: spec.options.clone(),
        };
        Some(factory(&context, cx))
    }
}

impl Global for WidgetRegistry {}
