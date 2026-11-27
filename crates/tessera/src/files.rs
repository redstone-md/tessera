// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The file manager of Tessera: windows of its own that show the drives
//! and folders of the computer, open what they hold and copy, move,
//! rename and delete it, through the Shell where Explorer would.

mod columns;
mod details_pane;
mod model;
mod pane;
mod preview_pane;
mod recycle_view;
mod rename;
mod search;
mod session;
mod sidebar;
mod tabs;
mod thumbs;
mod view;

use std::path::{Path, PathBuf};

use gpui::{
    App, AppContext, Bounds, Global, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use serde::Deserialize;

pub use model::{Grouping, Location, Look, size_text};
use session::{Opening, Session};
pub use thumbs::FileViewConfig;
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

    /// Opens a window of the file manager with the tabs of `opening`, and
    /// gives it, where it opened.
    fn open_with(opening: Opening, cx: &mut App) -> Option<gpui::WindowHandle<FilesView>> {
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
        match opened {
            Ok(opened) => Some(opened),
            Err(error) => {
                report!("the file manager did not open: {error:#}");
                None
            }
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
        let open = |folder: Option<&str>, cx: &mut App| {
            Files::open_named(folder.unwrap_or_default(), cx);
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

// files-previews: the windows of the file manager as others see them, for
// the windows of the Shell (`ShellWindows`) and `/select`, and opening a
// folder with items chosen.

/// What chooses items by their names in the folder of a window, once it
/// was read.
pub type SelectItems = std::rc::Rc<dyn Fn(Vec<std::ffi::OsString>, &mut App)>;

/// Who is told when the windows of the file manager change.
type WindowsObserver = std::rc::Rc<dyn Fn(&WindowsChange, &mut App)>;

/// A window of the file manager, as the windows of the Shell list it.
#[derive(Clone)]
#[allow(dead_code, reason = "for the windows of the Shell")]
pub struct FilesWindow {
    pub handle: gpui::AnyWindowHandle,
    /// The native window, as a number.
    pub hwnd: isize,
    /// The folder its pane in front shows, where that is a folder.
    pub folder: Option<PathBuf>,
    /// Chooses the items of those names in the folder of its pane in
    /// front, once the folder was read.
    pub select: SelectItems,
}

/// What changed among the windows of the file manager.
#[derive(Clone)]
#[allow(dead_code, reason = "for the windows of the Shell")]
pub enum WindowsChange {
    Opened(FilesWindow),
    /// Its pane in front went to another place, or another pane came in
    /// front.
    Navigated(FilesWindow),
    Closed(FilesWindow),
}

/// The windows of the file manager that are open, and who is told when
/// they change.
#[derive(Default)]
struct OpenWindows {
    windows: Vec<(gpui::AnyWindowHandle, gpui::WeakEntity<FilesView>)>,
    observers: Vec<WindowsObserver>,
}

impl Global for OpenWindows {}

impl Files {
    /// The windows of the file manager that are open, the oldest first.
    #[allow(dead_code, reason = "for the windows of the Shell")]
    pub fn windows(cx: &App) -> Vec<FilesWindow> {
        let Some(open) = cx.try_global::<OpenWindows>() else {
            return Vec::new();
        };
        { open.windows.iter() }
            .filter_map(|(_, view)| view.upgrade())
            .map(|view| view.read(cx).described(&view, cx))
            .collect()
    }

    /// Has `observer` told whenever a window of the file manager opens,
    /// goes somewhere else or closes.
    #[allow(dead_code, reason = "for the windows of the Shell")]
    pub fn observe_windows(cx: &mut App, observer: impl Fn(&WindowsChange, &mut App) + 'static) {
        cx.default_global::<OpenWindows>()
            .observers
            .push(std::rc::Rc::new(observer));
    }

    /// Opens a window of the file manager at `folder` with the items
    /// called `names` in it chosen, once it was read, as `/select` and
    /// `SHOpenFolderAndSelectItems` ask.
    #[allow(dead_code, reason = "for the command lines of Explorer")]
    pub fn open_selecting(folder: PathBuf, names: Vec<std::ffi::OsString>, cx: &mut App) {
        // Where no window opened, nothing is chosen.
        if let Some(opened) = Self::open_with(Opening::at(Location::Folder(folder)), cx) {
            { opened.update(cx, |view, _, cx| view.select_when_loaded(names, cx)) }.ok();
        }
    }

    /// Notes that the window of `view` opened.
    fn window_opened(
        handle: gpui::AnyWindowHandle,
        view: gpui::WeakEntity<FilesView>,
        cx: &mut App,
    ) {
        let open = cx.default_global::<OpenWindows>();
        open.windows.retain(|(_, each)| each.upgrade().is_some());
        open.windows.push((handle, view));
    }

    /// Tells those who observe the windows of `change`; a window that
    /// closed is forgotten.
    fn window_changed(change: WindowsChange, cx: &mut App) {
        let observers = { cx.default_global::<OpenWindows>().observers.clone() };
        if let WindowsChange::Closed(closed) = &change {
            let handle = closed.handle;
            let open = cx.default_global::<OpenWindows>();
            open.windows.retain(|(each, _)| *each != handle);
        }
        for observer in observers {
            observer(&change, cx);
        }
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
