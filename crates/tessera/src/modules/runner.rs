// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The runner: a single field near the top of the display, as KRunner of
//! KDE Plasma is, that answers what is typed with applications, files and
//! everything the runners of crate::runners offer. Until something is
//! typed it shows the documents opened lately. Widget: `runner`, popup
//! `runner`, action `runner`.

use crate::actions::{Action, Actions};
use crate::config::{PopupConfig, PopupPlace, WidgetSpec};
use crate::module::{Module, WidgetRegistry};
use crate::shell::Shell;

use super::launcher::{Form, launcher_view};

/// The name of the popup and of the action that opens it.
const POPUP: &str = "runner";

/// The size of the popup.
const WIDTH: f32 = 640.;
const HEIGHT: f32 = 380.;

pub struct RunnerModule;

impl Module for RunnerModule {
    fn name(&self) -> &'static str {
        "runner"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register_panel("runner", |context, cx| {
            launcher_view(context, Form::Compact, cx)
        })?;
        widgets.register_popup(PopupConfig {
            name: POPUP.to_owned(),
            place: PopupPlace::Top,
            width: WIDTH,
            height: HEIGHT,
            content: vec![WidgetSpec {
                widget: "runner".to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Alt+Space is the window menu of Windows, which a user may want
        // to keep, so the runner brings no key of its own.
        let open = Action::new(|_, cx| Shell::toggle_popup(POPUP, cx));
        actions.register("runner", open)
    }
}
