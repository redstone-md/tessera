// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The seam between the shell and the features plugged into it.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::bail;
use gpui::{AnyView, App, Global};

use crate::platform::DisplayKey;

/// A unit of shell functionality. Everything a surface shows comes from one.
pub trait Module {
    /// A short name for diagnostics.
    fn name(&self) -> &'static str;

    /// Contributes the module's widgets.
    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()>;
}

/// Where a widget instance is being placed.
#[derive(Clone, Copy, Debug)]
pub struct WidgetContext {
    /// The display of the hosting surface, when known.
    pub display: Option<DisplayKey>,
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

    /// Builds a new instance of the widget `id`, if any module provides it.
    pub fn build(id: &str, context: &WidgetContext, cx: &mut App) -> Option<AnyView> {
        let factory = cx.global::<Self>().factories.get(id).cloned()?;
        Some(factory(context, cx))
    }
}

impl Global for WidgetRegistry {}
