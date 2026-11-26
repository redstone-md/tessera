// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The file manager of Tessera: windows of its own that show the drives
//! and folders of the computer, open what they hold and copy, move,
//! rename and delete it, through the Shell where Explorer would.

mod model;
mod view;

use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, Bounds, Global, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use serde::Deserialize;

pub use model::{Location, size_text};
use view::FilesView;

use crate::actions::{Action, Actions};
use crate::module::{Module, WidgetRegistry};
use crate::platform::Place;
use crate::theme::Theme;
use crate::ui::{icon, pill};

/// The size a window of the file manager opens at, and the least it can
/// be made.
const OPEN_SIZE: (f32, f32) = (1080., 680.);
const LEAST_SIZE: (f32, f32) = (560., 360.);

/// The folder character of the icon font.
const ICON_FOLDER: &str = "\u{e8b7}";

/// How a folder is first shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Look {
    /// A row per item with its details.
    #[default]
    Details,
    /// Tiles of large icons.
    Tiles,
}

/// What `[files]` of the configuration says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FilesConfig {
    /// Whether the folders that the surfaces of Tessera open (the user
    /// menu, the launcher, the commands) open in its own file manager
    /// instead of in Explorer.
    pub own: bool,
    /// Whether hidden and system files are shown.
    pub hidden: bool,
    /// Whether folders stand before files.
    pub folders_first: bool,
    /// How a folder is first shown.
    pub view: Look,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            own: true,
            hidden: false,
            folders_first: true,
            view: Look::Details,
        }
    }
}

/// The file manager: how it is configured.
#[derive(Default)]
pub struct Files {
    config: FilesConfig,
}

impl Global for Files {}

impl Files {
    pub fn configure(config: &FilesConfig, cx: &mut App) {
        cx.default_global::<Self>().config = *config;
    }

    fn config(cx: &App) -> FilesConfig {
        cx.try_global::<Self>()
            .map(|files| files.config)
            .unwrap_or_default()
    }

    /// Opens a window of the file manager at `location`.
    pub fn open(location: Location, cx: &mut App) {
        let config = Self::config(cx);
        let bounds = Bounds::centered(None, size(px(OPEN_SIZE.0), px(OPEN_SIZE.1)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Files".into()),
                appears_transparent: true,
                traffic_light_position: None,
            }),
            focus: true,
            show: true,
            kind: WindowKind::Normal,
            is_movable: true,
            is_resizable: true,
            is_minimizable: true,
            app_id: Some("Tessera.Files".to_owned()),
            window_min_size: Some(size(px(LEAST_SIZE.0), px(LEAST_SIZE.1))),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| FilesView::new(location, config, window, cx))
        });
        if let Err(error) = opened {
            report!("the file manager did not open: {error:#}");
        }
    }

    /// Opens the folder `path` in the file manager when the configuration
    /// gives folders to it, and says whether it did; the caller has the
    /// Shell open it otherwise.
    pub fn open_folder(path: &Path, cx: &mut App) -> bool {
        if !Self::config(cx).own || !path.is_dir() {
            return false;
        }
        Self::open(Location::Folder(path.to_owned()), cx);
        true
    }

    /// Where a window opens that names no folder: the folder of the user.
    pub fn home() -> Location {
        Place::Home
            .path()
            .map_or(Location::Computer, Location::Folder)
    }

    /// The location a command or a key names: nothing or `~` for the
    /// folder of the user, `computer` for the drives, or a path, with a
    /// leading `~` standing for the folder of the user.
    pub fn named(name: &str) -> Location {
        let name = name.trim();
        match name {
            "" | "~" => Self::home(),
            "computer" => Location::Computer,
            _ => match name.strip_prefix("~\\").or(name.strip_prefix("~/")) {
                Some(rest) => match Place::Home.path() {
                    Some(home) => Location::Folder(home.join(rest)),
                    None => Location::Folder(PathBuf::from(name)),
                },
                None => Location::Folder(PathBuf::from(name)),
            },
        }
    }
}

/// The module of the file manager: the widget `files` opens a window of
/// it, at the folder its option `folder` names, and so does the action
/// `files`, at the folder of its argument.
pub struct FilesModule;

impl Module for FilesModule {
    fn name(&self) -> &'static str {
        "files"
    }

    fn register(&self, widgets: &mut WidgetRegistry) -> anyhow::Result<()> {
        widgets.register("files", |context, cx| {
            let folder = context.text("folder").unwrap_or_default().to_owned();
            cx.new(|_| FilesButton { folder }).into()
        })
    }

    fn actions(&self, actions: &mut Actions) -> anyhow::Result<()> {
        // Windows keeps Win+E, which opens Explorer, to itself.
        let open = |folder: Option<&str>, cx: &mut App| {
            Files::open(Files::named(folder.unwrap_or_default()), cx);
        };
        actions.register("files", Action::new(open).taking_reserved_keys())
    }
}

/// A button of a bar that opens the file manager.
struct FilesButton {
    folder: String,
}

impl Render for FilesButton {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        use gpui::{InteractiveElement, StatefulInteractiveElement};
        let theme = cx.global::<Theme>();
        let folder = self.folder.clone();
        pill(theme)
            .id("files")
            .cursor_pointer()
            .child(
                div()
                    .text_color(theme.colors.primary)
                    .child(icon(theme, ICON_FOLDER)),
            )
            .on_click(move |_, _, cx| Files::open(Files::named(&folder), cx))
    }
}
