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

use async_channel::Sender;
use gpui::{
    App, AppContext, Bounds, Global, IntoElement, ParentElement, Render, Styled, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use serde::Deserialize;

use model::RECYCLE_BIN;
pub use model::{Grouping, Location, Look, size_text};
use session::{Opening, Session};
pub use thumbs::FileViewConfig;
use view::FilesView;

use crate::actions::{Action, Actions};
use crate::autoplay::{self, AutoPlayList, AutoPlayWhen};
use crate::banners::{Banners, Notice};
use crate::config::{PopupConfig, WidgetSpec};
use crate::explorer_args::{Destination, ExplorerArgs, KnownPlace, Places, explorer_command_line};
use crate::i18n::Language;
use crate::module::{Module, WidgetRegistry};
use crate::modules::devices::{Drives, letter_of as drive_letter_of};
use crate::modules::windows::FILES_APP;
use crate::platform::Place;
use crate::platform::{
    AppWindow, DriveDialog, FolderHandler, NetworkDrives, RecycleBin, SharePackage, ShellThread,
    ShellWindows, known_folder_path, reveal_with, shell_mode, start_explorer,
};
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
    /// When AutoPlay is done for drives that arrive: as the shell only
    /// (Explorer does it otherwise), always, or never.
    pub autoplay: AutoPlayWhen,
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
            autoplay: AutoPlayWhen::Shell,
        }
    }
}

/// The file manager: how it is configured.
#[derive(Default)]
pub struct Files {
    config: FilesConfig,
    /// Where `platform::reveal` hands its paths, once they are taken.
    reveals: Option<Sender<PathBuf>>,
    /// Whether the list of folder windows of the Shell is served.
    serving: bool,
}

impl Global for Files {}

impl Files {
    pub fn configure(config: &FilesConfig, cx: &mut App) {
        cx.default_global::<Self>().config = *config;
        Self::take_reveals(config.own, cx);
        Drives::play_arrivals(config.autoplay.active(shell_mode()), cx);
        Self::serve_shell_windows(cx);
    }

    /// Whether the folders the surfaces of Tessera open open here.
    pub fn opens_folders(cx: &App) -> bool {
        Self::config(cx).own
    }

    /// Has the files that `platform::reveal` shows in their folders shown
    /// here while `own`, and in Explorer otherwise.
    fn take_reveals(own: bool, cx: &mut App) {
        if !own {
            reveal_with(None);
            return;
        }
        let reveals = match &cx.global::<Self>().reveals {
            Some(reveals) => reveals.clone(),
            None => {
                let (reveals, revealed) = async_channel::unbounded::<PathBuf>();
                cx.spawn(async move |cx| {
                    while let Ok(path) = revealed.recv().await {
                        if cx.update(|cx| Self::open_at_item(&path, cx)).is_err() {
                            break;
                        }
                    }
                })
                .detach();
                cx.global_mut::<Self>().reveals = Some(reveals.clone());
                reveals
            }
        };
        reveal_with(Some(reveals));
    }

    /// Opens the share window of Windows for `paths`, over the window of
    /// Tessera in front, or else any of its windows. The files are read
    /// away from the user interface first.
    pub fn share(paths: Vec<PathBuf>, cx: &mut App) {
        let many = Language::of(cx).say("items");
        let prepared = cx
            .background_executor()
            .spawn(async move { SharePackage::prepare(&paths, many) });
        cx.spawn(async move |cx| {
            let package = match prepared.await {
                Ok(package) => package,
                Err(error) => return report!("nothing was shared: {error:#}"),
            };
            cx.update(|cx| {
                let window = cx.active_window().or_else(|| cx.windows().first().copied());
                let Some(window) = window else {
                    return report!("nothing was shared: Tessera has no window to share from");
                };
                let shown = window.update(cx, |_, window, _| package.show(window));
                if let Ok(Err(error)) | Err(error) = shown {
                    report!("nothing was shared: {error:#}");
                }
            })
            .ok();
        })
        .detach();
    }

    /// Makes Tessera Files the handler of the folders and drives of the
    /// file system with `on`, or gives that back, away from the user
    /// interface, and says in a notice how it went.
    fn set_default(on: bool, cx: &mut App) {
        let changed = cx.background_executor().spawn(async move {
            match on {
                true => FolderHandler::install(),
                false => FolderHandler::revert(),
            }
        });
        cx.spawn(async move |cx| {
            let changed = changed.await;
            cx.update(|cx| {
                let language = Language::of(cx);
                let (title, body) = match (&changed, on) {
                    (Ok(_), true) => (
                        language.say("Folders open in Tessera Files"),
                        language.say("Folders and drives opened by other programs come here; virtual folders such as Control Panel stay with Explorer.").to_owned(),
                    ),
                    (Ok(_), false) => (
                        language.say("Folders open as before"),
                        language.say("Other programs open folders as they did before Tessera Files took them.").to_owned(),
                    ),
                    (Err(error), _) => (
                        language.say("Folders were not changed"),
                        format!("{error:#}"),
                    ),
                };
                Banners::post(
                    Notice {
                        app: language.say("File manager").to_owned(),
                        title: title.to_owned(),
                        body,
                        glyph: Some(ICON_FOLDER),
                        ..Notice::default()
                    },
                    cx,
                );
            })
            .ok();
        })
        .detach();
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
            app_id: Some(FILES_APP.to_owned()),
            window_min_size: Some(size(px(LEAST_SIZE.0), px(LEAST_SIZE.1))),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            // The dock and the switcher list the window as an application's.
            AppWindow::mark_own(window, FILES_APP, None);
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

    /// Serves the list of folder windows of the Shell, as the shell of the
    /// session, once. The windows of the file manager are listed at the
    /// folder of their pane in front, which the list follows as they go
    /// elsewhere, and leave it when they close; a program that asks one of
    /// them to choose files brings it to the front with those files chosen
    /// once its folder was read.
    fn serve_shell_windows(cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.serving || !shell_mode() {
            return;
        }
        this.serving = true;
        let selections = ShellWindows::serve();
        for window in Self::windows(cx) {
            ShellWindows::opened(window.hwnd, window.folder);
        }
        Self::observe_windows(cx, |change, _| match change {
            WindowsChange::Opened(window) => {
                ShellWindows::opened(window.hwnd, window.folder.clone())
            }
            WindowsChange::Navigated(window) => {
                ShellWindows::navigated(window.hwnd, window.folder.clone());
            }
            WindowsChange::Closed(window) => ShellWindows::closed(window.hwnd),
        });
        cx.spawn(async move |cx| {
            while let Ok(selection) = selections.recv().await {
                let answered = cx.update(|cx| {
                    let window = { Self::windows(cx).into_iter() }
                        .find(|window| window.hwnd == selection.window);
                    if let Some(window) = window {
                        let _ =
                            { window.handle }.update(cx, |_, window, _| window.activate_window());
                        (window.select)(selection.names, cx);
                    }
                });
                if answered.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Opens the folder `path` in the file manager when the configuration
    /// gives folders to it, and says whether it did; the caller has the
    /// Shell open it otherwise.
    pub fn open_folder(path: &Path, cx: &mut App) -> bool {
        path.is_dir() && Self::open_owned(Location::Folder(path.to_owned()), cx)
    }

    /// Opens `location` in the file manager when the configuration gives
    /// folders to it, and says whether it did; the caller has the Shell
    /// open it otherwise.
    pub fn open_owned(location: Location, cx: &mut App) -> bool {
        if !Self::opens_folders(cx) {
            return false;
        }
        Self::open(location, cx);
        true
    }

    /// Opens the Recycle Bin: in the file manager when the configuration
    /// gives folders to it, and in Explorer otherwise; a failure is
    /// reported.
    pub fn open_recycle_bin(cx: &mut App) {
        if Self::open_owned(Location::RecycleBin, cx) {
            return;
        }
        if let Err(error) = RecycleBin::open() {
            report!("the Recycle Bin was not opened: {error:#}");
        }
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

    /// Opens the folder around `path` with `path` chosen in it, once the
    /// folder was read; a path with no folder around it, as the root of a
    /// drive, opens itself.
    pub fn open_at_item(path: &Path, cx: &mut App) {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                Self::open_selecting(parent.to_owned(), vec![name.to_owned()], cx);
            }
            _ => Self::open(Location::Folder(path.to_owned()), cx),
        }
    }

    /// Where a window opens that names no folder: the folder of the user.
    pub fn home() -> Location {
        Place::Home
            .path()
            .map_or(Location::Computer, Location::Folder)
    }

    /// The location a command or a key names: nothing or `~` for the
    /// folder of the user, `home` for Home (Quick access and the files
    /// used lately), `computer` for the drives, `recycle-bin` for the
    /// Recycle Bin, or a path, with a leading
    /// `~` standing for the folder of the user.
    pub fn named(name: &str) -> Location {
        let name = name.trim();
        match name {
            "" | "~" => Self::home(),
            "computer" => Location::Computer,
            "home" => Location::Home,
            RECYCLE_BIN => Location::RecycleBin,
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
        })?;
        // AutoPlay asks here what to do with a drive that arrived.
        widgets.register_panel(autoplay::POPUP, |_, cx| cx.new(AutoPlayList::new).into())?;
        widgets.register_popup(PopupConfig {
            name: autoplay::POPUP.to_owned(),
            place: Default::default(),
            width: 360.,
            height: 380.,
            content: vec![WidgetSpec {
                widget: autoplay::POPUP.to_owned(),
                popup: None,
                options: toml::Table::new(),
            }],
            side: Vec::new(),
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
        actions.register("files", open)?;
        let default =
            Action::new(|arg, cx| Files::set_default(arg == Some("on"), cx)).checking(|arg| {
                match arg {
                    Some("on" | "off") => Ok(()),
                    _ => Err("`files-default` takes `on` or `off`".to_owned()),
                }
            });
        actions.register("files-default", default)?;
        let share = Action::new(|arg, cx| Files::share(shared_paths(arg.unwrap_or_default()), cx))
            .checking(|arg| match arg.map(shared_paths) {
                Some(paths) if !paths.is_empty() => Ok(()),
                _ => Err("`share` takes the paths of files, separated by `|`".to_owned()),
            });
        actions.register("share", share)?;
        let map = Action::new(|_, cx| {
            let dialog = NetworkDrives::map();
            cx.background_executor()
                .spawn(async move { report_dialog(dialog.recv().await.ok()) })
                .detach();
        });
        actions.register("map-drive", map)?;
        let disconnect = Action::new(|arg, cx| {
            let dialog = NetworkDrives::disconnect(arg.and_then(drive_letter_of));
            cx.background_executor()
                .spawn(async move { report_dialog(dialog.recv().await.ok()) })
                .detach();
        })
        .checking(|arg| match arg {
            None => Ok(()),
            Some(arg) if drive_letter_of(arg).is_some() => Ok(()),
            Some(_) => Err("`disconnect-drive` takes the letter of a drive, or none".to_owned()),
        });
        actions.register("disconnect-drive", disconnect)?;
        let location = Action::new(|arg, cx| {
            let Some(arg) = arg.map(str::to_owned) else {
                if let Err(error) = NetworkDrives::add_location_wizard() {
                    report!("{error:#}");
                }
                return;
            };
            let made = cx.background_executor().spawn(async move {
                let (address, name) = match arg.split_once('|') {
                    Some((address, name)) => {
                        (address.trim().to_owned(), Some(name.trim().to_owned()))
                    }
                    None => (arg.trim().to_owned(), None),
                };
                NetworkDrives::add_location(&address, name.as_deref())
            });
            // This PC shows the network locations, the new one among them.
            cx.spawn(async move |cx| match made.await {
                Ok(_) => {
                    cx.update(|cx| Files::open(Location::Computer, cx)).ok();
                }
                Err(error) => report!("the network location was not added: {error:#}"),
            })
            .detach();
        })
        .taking_any_arg();
        actions.register("add-network-location", location)
    }
}

/// The paths the `arg` of `share` names, separated by `|`, which no path
/// holds.
fn shared_paths(arg: &str) -> Vec<PathBuf> {
    { arg.split('|') }
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Reports what a dialog of network drives came to where Windows refused.
fn report_dialog(dialog: Option<DriveDialog>) {
    if let Some(DriveDialog::Failed(why)) = dialog {
        report!("the network drive was not changed: {why}");
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
    pub fn observe_windows(cx: &mut App, observer: impl Fn(&WindowsChange, &mut App) + 'static) {
        cx.default_global::<OpenWindows>()
            .observers
            .push(std::rc::Rc::new(observer));
    }

    /// Opens a window of the file manager at `folder` with the items
    /// called `names` in it chosen, once it was read, as `/select` and
    /// `SHOpenFolderAndSelectItems` ask.
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

    #[test]
    fn the_drives_and_the_recycle_bin_have_names() {
        assert_eq!(Files::named("computer"), Location::Computer);
        assert_eq!(Files::named(" recycle-bin "), Location::RecycleBin);
        assert_eq!(Files::named("home"), Location::Home);
    }
}
