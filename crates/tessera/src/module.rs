// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The seam between the shell and the features plugged into it.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::bail;
use gpui::{AnyView, App, Global};

use crate::config::{PopupConfig, WidgetSpec};
use crate::platform::DisplayKey;

/// A unit of shell functionality. Everything a surface shows comes from one.
pub trait Module {
    /// A short name for diagnostics.
    fn name(&self) -> &'static str;

    /// Contributes the module's widgets and popups.
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
            report!("the widget option `{key}` must be text, `{value}` is ignored");
        }
        text
    }

    /// The yes-or-no option `key`. A value of another type is reported and
    /// treated as absent.
    pub fn flag(&self, key: &str) -> Option<bool> {
        let value = self.options.get(key)?;
        let flag = value.as_bool();
        if flag.is_none() {
            report!("the widget option `{key}` must be true or false, `{value}` is ignored");
        }
        flag
    }

    /// The number option `key`. A value of another type is reported and
    /// treated as absent.
    pub fn number(&self, key: &str) -> Option<f64> {
        let value = self.options.get(key)?;
        let number = value
            .as_float()
            .or_else(|| value.as_integer().map(|whole| whole as f64));
        if number.is_none() {
            report!("the widget option `{key}` must be a number, `{value}` is ignored");
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
            report!("the widget option `{key}` must be a list of texts, `{value}` is ignored");
        }
        texts
    }
}

type WidgetFactory = Rc<dyn Fn(&WidgetContext, &mut App) -> AnyView>;

/// What modules contribute: the widgets a layout can refer to, by
/// identifier, and the popups that come with them.
#[derive(Default)]
pub struct WidgetRegistry {
    factories: HashMap<String, WidgetFactory>,
    popups: HashMap<String, PopupConfig>,
    /// The popup a click on a widget opens when the layout names none, by
    /// widget identifier.
    opened: HashMap<String, String>,
}

impl WidgetRegistry {
    /// Collects the widgets of `modules`. A module that fails to register is
    /// reported and skipped; widgets it added before failing stay available.
    pub fn from_modules(modules: &[Box<dyn Module>]) -> Self {
        let mut registry = Self::default();
        for module in modules {
            if let Err(error) = module.register(&mut registry) {
                report!("module `{}` is not fully loaded: {error:#}", module.name());
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

    /// Makes `popup` available to layouts under its name. A popup of the
    /// same name in the user's configuration is used instead of it.
    pub fn register_popup(&mut self, popup: PopupConfig) -> anyhow::Result<()> {
        if self.popups.contains_key(&popup.name) {
            bail!("a popup named `{}` is already registered", popup.name);
        }
        self.popups.insert(popup.name.clone(), popup);
        Ok(())
    }

    /// Makes a click on the widget `widget` open `popup` wherever the
    /// layout names no popup for it.
    pub fn open_with(&mut self, widget: &str, popup: &str) {
        self.opened.insert(widget.to_owned(), popup.to_owned());
    }

    /// The popup a module registered as `name`.
    pub fn popup(&self, name: &str) -> Option<&PopupConfig> {
        self.popups.get(name)
    }

    /// The popup `spec` opens: the one the layout names, or else the one
    /// the widget's module chose. An empty name in the layout means none.
    pub fn popup_of(&self, spec: &WidgetSpec) -> Option<String> {
        let popup = match &spec.popup {
            Some(popup) => popup,
            None => self.opened.get(&spec.widget)?,
        };
        (!popup.is_empty()).then(|| popup.clone())
    }

    /// The identifiers of the widgets a layout can name, in order.
    /// The names of the popups that come with the widgets.
    pub fn popup_names(&self) -> Vec<String> {
        self.popups.keys().cloned().collect()
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.factories.keys().cloned().collect();
        names.sort();
        names
    }

    /// Builds a new instance of the widget `spec` names for a surface on
    /// `display`. A name no module provides is reported.
    pub fn build(spec: &WidgetSpec, display: Option<DisplayKey>, cx: &mut App) -> Option<AnyView> {
        let Some(factory) = cx.global::<Self>().factories.get(&spec.widget).cloned() else {
            report!("no module provides the widget `{}`", spec.widget);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(widget: &str, popup: Option<&str>) -> WidgetSpec {
        WidgetSpec {
            widget: widget.to_owned(),
            popup: popup.map(str::to_owned),
            options: toml::Table::new(),
        }
    }

    #[test]
    fn the_layout_decides_which_popup_a_widget_opens() {
        let mut registry = WidgetRegistry::default();
        registry.open_with("lights.button", "lights.panel");
        let opened = |spec: &WidgetSpec| registry.popup_of(spec);
        assert_eq!(
            opened(&spec("lights.button", None)).as_deref(),
            Some("lights.panel")
        );
        assert_eq!(
            opened(&spec("lights.button", Some("mine"))).as_deref(),
            Some("mine")
        );
        assert_eq!(opened(&spec("lights.button", Some(""))), None);
        assert_eq!(opened(&spec("clock", None)), None);
    }
}
