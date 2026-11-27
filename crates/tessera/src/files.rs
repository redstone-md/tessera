// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The file manager of Tessera: windows of its own that show the drives
//! and folders of the computer, open what they hold and copy, move,
//! rename and delete it, through the Shell where Explorer would.

mod model;
mod pane;
mod rename;
mod search;
mod session;
mod sidebar;
mod tabs;
mod view;

use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, Bounds, Global, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use serde::Deserialize;

pub use model::{Grouping, Location, Look, size_text};
use session::{Opening, Session};
use view::FilesView;

use crate::actions::{Action, Actions};
use crate::explorer_args::{Destination, ExplorerArgs, KnownPlace, Places, explorer_command_line};
use crate::module::{Module, WidgetRegistry};
use crate::platform::Place;
use crate::platform::{ShellThread, known_folder_path, start_explorer};
use crate::theme::Theme;
use crate::ui::{icon, pill};

/// The size a window of the file manager opens at, and the least it can
/// be made.
const OPEN_SIZE: (f32, f32) = (1080., 680.);
const LEAST_SIZE: (f32, f32) = (560., 360.);

/// The folder character of the icon font.
const ICON_FOLDER: &str = "\u{e8b7}";

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
    /// What a folder is first grouped by.
    pub group: Grouping,
    /// Whether each folder keeps its own look, zoom, order and groups,
    /// as the user last set them there.
    pub per_folder: bool,
    /// Whether a window opened without a folder opens with the tabs of
    /// the last window closed.
    pub restore: bool,
    /// Whether the folder options of Explorer are followed: extensions of
    /// known types hidden, hidden and protected files shown as Explorer
    /// shows them, and the names a `desktop.ini` gives.
    pub explorer: bool,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            own: true,
            hidden: false,
            folders_first: true,
            view: Look::Details,
            group: Grouping::None,
            per_folder: false,
            restore: false,
            explorer: true,
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
        Self::open_with(Opening::at(location), cx);
    }

    /// Opens a window of the file manager at the folder `name` names, as
    /// `named` reads it; without a name, with the tabs of the last window
    /// closed where the configuration asks for that, and else at home.
    pub fn open_named(name: &str, cx: &mut App) {
        if !name.trim().is_empty() || !Self::config(cx).restore {
            Self::open(Self::named(name), cx);
            return;
        }
        cx.spawn(async move |cx| {
            let session = cx
                .background_executor()
                .spawn(async { Session::load() })
                .await;
            cx.update(|cx| {
                let opening = session
                    .opening(Self::named)
                    .unwrap_or_else(|| Opening::at(Self::home()));
                Self::open_with(opening, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens a window of the file manager with the tabs of `opening`.
    fn open_with(opening: Opening, cx: &mut App) {
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
            cx.new(|cx| FilesView::new(opening, config, window, cx))
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

    /// Opens what `words` name, read as Explorer reads its command line
    /// (see [`crate::explorer_args`]): a folder, `/select,<path>`,
    /// `shell:Downloads`, a `search-ms:` search and the like. What is no
    /// folder of the file system, such as Control Panel, is handed to
    /// Explorer with the same words. Known folders and paths are looked
    /// up away from the user interface.
    pub fn open_command(words: &[String], cx: &mut App) {
        let args = ExplorerArgs::parse(words);
        if args.target.is_none() && args.root.is_none() {
            Self::open_named("", cx);
            return;
        }
        let line = explorer_command_line(words);
        let found =
            ShellThread::call_alone("explorer words", move || args.destination(&SystemPlaces));
        cx.spawn(async move |cx| {
            let Ok(destination) = found.recv().await else {
                return;
            };
            cx.update(|cx| match destination {
                Destination::Default => Self::open_named("", cx),
                Destination::Open(location) => Self::open(location, cx),
                Destination::Select(path) => Self::open_at_item(&path, cx),
                Destination::Explorer => {
                    if let Err(error) = start_explorer(&line, false) {
                        report!("{error:#}");
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Opens the folder around `path`, where `path` is to be chosen.
    /// Choosing it waits for the panes to be able to choose an item by
    /// name; until then the folder opens with nothing chosen.
    pub fn open_at_item(path: &Path, cx: &mut App) {
        let folder = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => path,
        };
        Self::open(Location::Folder(folder.to_owned()), cx);
    }

    /// Where a window opens that names no folder: the folder of the user.
    pub fn home() -> Location {
        Place::Home
            .path()
            .map_or(Location::Computer, Location::Folder)
    }

    /// The location a command or a key names: nothing or `~` for the
    /// folder of the user, `home` for Home (Quick access and the files
    /// used lately), `computer` for the drives, or a path, with a leading
    /// `~` standing for the folder of the user.
    pub fn named(name: &str) -> Location {
        let name = name.trim();
        match name {
            "" | "~" => Self::home(),
            "computer" => Location::Computer,
            "home" => Location::Home,
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

/// The places of this computer as Explorer's command lines name them:
/// known folders through the Shell, paths through the file system. Asked
/// away from the user interface, as both may wait.
pub struct SystemPlaces;

impl Places for SystemPlaces {
    fn known(&self, name: &str) -> KnownPlace {
        known_folder_path(name).map_or(KnownPlace::Virtual, KnownPlace::Folder)
    }

    fn named(&self, word: &str) -> Location {
        Files::named(word)
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
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
        // Windows keeps Win+E, which opens Explorer, to itself; it is
        // taken where the keys of Windows are in use, as in shell mode,
        // where nothing else answers it.
        let open = |folder: Option<&str>, cx: &mut App| match folder {
            Some(folder) => Files::open_command(&[folder.to_owned()], cx),
            None => Files::open_named("", cx),
        };
        let open = { Action::new(open).taking_any_arg() }
            .taking_reserved_keys()
            .by_default("win+e", None);
        actions.register("files", open)
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
            .on_click(move |_, _, cx| Files::open_named(&folder, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_e_opens_the_file_manager_where_the_keys_of_windows_are_in_use() {
        let mut actions = Actions::default();
        FilesModule.actions(&mut actions).unwrap();
        let keys = actions.default_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].press, "win+e");
        assert_eq!(keys[0].action.as_deref(), Some("files"));
        assert_eq!(keys[0].arg, None);
        assert!(actions.takes_reserved_keys("files"));
    }
}
