// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the icons of the desktop need of Windows: the two folders the
//! desktop shows (the user's and the one all users share), the items of
//! the Shell it shows beside them as `HideDesktopIcons\NewStartPanel`
//! says (This PC, the Recycle Bin, Network, Control Panel, the folder of
//! the user), a watch on both folders with `ReadDirectoryChangesW`, the
//! icons of the Recycle Bin, Explorer's own icons hidden while Tessera
//! draws them, and the window the icons are drawn in made part of the
//! desktop ([`DesktopLayer`]).
//!
//! The window of the icons is a window of GPUI. In [`LayerKind::Child`]
//! it becomes a child window: of Tessera's own `SHELLDLL_DefView` where
//! Tessera is the shell, so that it really is the desktop, and beside
//! Explorer of a window of the class [`LAYER_CLASS`] that Tessera keeps
//! at the bottom of the order of windows. In [`LayerKind::Window`] it
//! stays a top-level window, kept at the bottom by a subclass of its
//! procedure, and owned by Tessera's own desktop window where Tessera is
//! the shell. A window of another process is never its owner: that
//! would tie the input of Tessera to Explorer. Alt+F4 on either closes
//! nothing; it is told on [`DesktopLayer::close_requests`].
//!
//! The cards of the desktop are owned by the layer of their display
//! ([`NativeSurface::rest_on_desktop`]), so that they stay above the
//! icons; the layer gives them up before it goes, as Windows destroys the
//! windows an owner owns with it.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;

use anyhow::{Context as _, Result, bail};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WAIT_FAILED, WAIT_OBJECT_0, WPARAM,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, MapWindowPoints, PAINTSTRUCT};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThreadId, INFINITE, SetEvent, WaitForMultipleObjects,
};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::{
    DefSubclassProc, FOLDERID_Desktop, FOLDERID_PublicDesktop, RemoveWindowSubclass,
    SHGSI_ICONLOCATION, SHGetStockIconInfo, SHSTOCKICONINFO, SIGDN_NORMALDISPLAY, SIID_RECYCLER,
    SIID_RECYCLERFULL, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumThreadWindows, FindWindowExW, GA_ROOT,
    GW_CHILD, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GWLP_HWNDPARENT, GetAncestor, GetWindow,
    GetWindowLongPtrW, HWND_BOTTOM, HWND_NOTOPMOST, IDC_ARROW, IsWindowVisible, LoadCursorW,
    PostMessageW, RegisterClassW, SC_CLOSE, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetParent, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, ShowWindowAsync, WINDOWPOS, WM_CLOSE, WM_ERASEBKGND, WM_NCDESTROY, WM_PAINT,
    WM_SETFOCUS, WM_SIZE, WM_SYSCOMMAND, WM_WINDOWPOSCHANGING, WNDCLASSW, WS_CAPTION, WS_CHILD,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    WS_THICKFRAME, WindowFromPoint,
};
use windows::core::{BOOL, PCWSTR, w};

use super::registry::Key;
use super::shell_item::{IdList, ShellId, name_of_item, wide};
use super::shell_items::known_folder;
use super::{ExplorerSettings, IconPixels, NativeSurface, ScreenRect, ShellHost, Taskbar};

/// The class of the window Tessera keeps at the bottom beside Explorer,
/// which the icons of the desktop are drawn in. Windows that look for the
/// desktop by its class (Alt+F4 on the desktop, a display filled by an
/// application) know it by this name. It is short: one of them reads at
/// most fifteen letters of a class.
pub(super) const LAYER_CLASS: &str = "Tessera.Icons";

/// Where Explorer keeps which items of the Shell the desktop shows, by
/// their class identifiers: 0 shows one, 1 hides it.
const NEW_START_PANEL: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel";

/// The number of the subclass that keeps a top-level layer down.
const LAYER_SUBCLASS: usize = 0x7E55;

/// The folders the desktop shows: the user's own and the one all users
/// share, whose items stand on every user's desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopFolders {
    pub user: PathBuf,
    /// `None` where Windows names none or it cannot be read.
    pub public: Option<PathBuf>,
}

impl DesktopFolders {
    /// The two folders as Windows knows them; `None` where the user has
    /// no desktop folder.
    pub fn find() -> Option<Self> {
        let user = known_folder(&FOLDERID_Desktop)
            .inspect_err(|error| report!("the desktop folder is not known: {error:#}"))
            .ok()?;
        let public = known_folder(&FOLDERID_PublicDesktop)
            .ok()
            .filter(|public| public != &user && public.is_dir());
        Some(Self { user, public })
    }

    /// Both folders, the user's first.
    pub fn all(&self) -> Vec<PathBuf> {
        std::iter::once(self.user.clone())
            .chain(self.public.clone())
            .collect()
    }
}

/// An item of the Shell the desktop can show beside its files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VirtualKind {
    Computer,
    UserFiles,
    Network,
    RecycleBin,
    ControlPanel,
}

impl VirtualKind {
    /// In the order Explorer arranges them in.
    pub const ALL: [Self; 5] = [
        Self::Computer,
        Self::UserFiles,
        Self::Network,
        Self::RecycleBin,
        Self::ControlPanel,
    ];

    /// The class identifier the Shell names the item by.
    pub fn clsid(self) -> &'static str {
        match self {
            Self::Computer => "{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            Self::UserFiles => "{59031a47-3f72-44a7-89c5-5595fe6b30ee}",
            Self::Network => "{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}",
            Self::RecycleBin => "{645FF040-5081-101B-9F08-00AA002F954E}",
            Self::ControlPanel => "{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}",
        }
    }

    /// The name the Shell parses, `::{...}`, which opening it, its icon
    /// and its menu are asked by.
    pub fn parsing_name(self) -> String {
        format!("::{}", self.clsid())
    }

    /// Whether Windows shows it where the user changed nothing: only the
    /// Recycle Bin.
    fn shown_by_default(self) -> bool {
        self == Self::RecycleBin
    }

    /// The identifier list of the item, as a shortcut to it is made with.
    /// Asks the Shell; COM must be initialized on the calling thread.
    pub fn id(self) -> Option<ShellId> {
        IdList::of_path(Path::new(&self.parsing_name()))
            .ok()
            .map(|list| list.to_id())
    }
}

/// What the user chose in Windows for what the desktop shows: which
/// items of the Shell (Desktop icon settings) and Explorer's folder
/// options. It is read from the registry alone, cheaply, so it can be
/// asked every little while and the desktop read again when it changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopOptions {
    /// The value Explorer wrote for each of [`VirtualKind::ALL`], in its
    /// order: 0 shows it, 1 hides it.
    hidden: [Option<u32>; VirtualKind::ALL.len()],
    pub folders: ExplorerSettings,
}

impl DesktopOptions {
    /// The options as they are now.
    pub fn read() -> Self {
        let key = Key::read(HKEY_CURRENT_USER, NEW_START_PANEL, Default::default());
        Self {
            hidden: VirtualKind::ALL
                .map(|kind| key.as_ref().and_then(|key| key.number(kind.clsid()))),
            folders: ExplorerSettings::read(),
        }
    }
}

/// An item of the Shell on the desktop, as Windows has it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtualItem {
    pub kind: VirtualKind,
    /// What the Shell calls it, in the language of Windows.
    pub name: String,
    /// Whether the user has Windows show it on the desktop.
    pub shown: bool,
}

/// The items of the Shell the desktop may show, in Explorer's order, with
/// whether each is shown as `options` say and its name. Asks the Shell;
/// COM must be initialized on the calling thread, which is not the thread
/// of GPUI.
pub fn virtual_items(options: &DesktopOptions) -> Vec<VirtualItem> {
    { VirtualKind::ALL.into_iter().zip(options.hidden) }
        .map(|(kind, hidden)| {
            let name = IdList::of_path(Path::new(&kind.parsing_name()))
                .and_then(|list| list.item())
                .ok()
                .and_then(|item| name_of_item(&item, SIGDN_NORMALDISPLAY))
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            VirtualItem {
                kind,
                shown: shown(hidden, kind.shown_by_default()) && !name.is_empty(),
                name,
            }
        })
        .collect()
}

/// Whether an item is shown, as the value Explorer wrote for it says
/// (`hidden`), or as Windows has it by default where there is none.
fn shown(hidden: Option<u32>, by_default: bool) -> bool {
    hidden.map_or(by_default, |hidden| hidden == 0)
}

impl IconPixels {
    /// The icon of the Recycle Bin, `size` pixels high: full or empty.
    pub fn recycle_bin(full: bool, size: u32) -> Option<Self> {
        let mut info = SHSTOCKICONINFO {
            cbSize: size_of::<SHSTOCKICONINFO>() as u32,
            ..Default::default()
        };
        let id = if full {
            SIID_RECYCLERFULL
        } else {
            SIID_RECYCLER
        };
        // SAFETY: the structure carries its size and outlives the call,
        // which writes the place of the icon into it and gives no handle.
        unsafe { SHGetStockIconInfo(id, SHGSI_ICONLOCATION, &raw mut info) }.ok()?;
        let length = { info.szPath.iter() }
            .position(|&unit| unit == 0)
            .unwrap_or(info.szPath.len());
        let file = PathBuf::from(String::from_utf16_lossy(&info.szPath[..length]));
        Self::of_file(&file, info.iIcon, size)
    }
}

/// A watch on folders: something arrives on its receiver whenever what
/// one of them holds changes, until it is dropped. Changes come in bursts
/// (a copy of many files); the receiver holds at most one waiting.
pub struct FolderWatch {
    /// The event that ends the watching thread, which holds it too.
    stop: Arc<Event>,
}

/// An event of Windows, closed when the last holder lets go of it.
struct Event(HANDLE);

// SAFETY: an event is a kernel object that any thread may set and wait
// for; it is closed once, by the last holder.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: the handle was made for this value and is closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

impl FolderWatch {
    /// Watches `folders` on a thread of its own. A folder that cannot be
    /// opened is reported and left out; with none left, the watch fails.
    pub fn start(folders: &[PathBuf]) -> Result<(Self, Receiver<()>)> {
        // SAFETY: the event is made without a name, and closed when both
        // this and the thread let go of it.
        let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .context("the watch on the desktop cannot be stopped")?;
        let stop = Arc::new(Event(stop));
        let mut watched = Vec::new();
        for folder in folders {
            match Watched::open(folder) {
                Ok(one) => watched.push(one),
                Err(error) => report!("{} is not watched: {error:#}", folder.display()),
            }
        }
        if watched.is_empty() {
            bail!("no folder of the desktop can be watched");
        }
        let (changed, told) = async_channel::bounded(1);
        let stopping = stop.clone();
        thread::Builder::new()
            .name("desktop watch".to_owned())
            .spawn(move || watch(watched, &stopping, &changed))
            .context("the watch on the desktop does not start")?;
        Ok((Self { stop }, told))
    }
}

impl Drop for FolderWatch {
    fn drop(&mut self) {
        // SAFETY: the event is open while this holds it.
        let _ = unsafe { SetEvent(self.stop.0) };
    }
}

/// How many bytes of changes a folder gives at once; more is told as an
/// overflow, which is a change all the same.
const CHANGES_BUFFER: usize = 16 * 1024;

/// A folder being watched, with the reading that waits for its changes.
struct Watched {
    folder: HANDLE,
    /// Where the changes are written, aligned as Windows wants them.
    buffer: Box<[u32]>,
    overlapped: Box<OVERLAPPED>,
}

// SAFETY: the handles are kernel handles, which any thread may use; the
// buffer and the structure are owned, and only the watching thread
// touches them once it has them.
unsafe impl Send for Watched {}

impl Watched {
    fn open(folder: &Path) -> Result<Self> {
        let name = wide(folder.as_os_str());
        // SAFETY: the name is NUL-terminated and outlives the calls; the
        // handles are closed when this is dropped.
        unsafe {
            let handle = CreateFileW(
                PCWSTR(name.as_ptr()),
                FILE_LIST_DIRECTORY.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                None,
            )
            .context("the folder cannot be opened")?;
            let event = match CreateEventW(None, true, false, PCWSTR::null()) {
                Ok(event) => event,
                Err(error) => {
                    let _ = CloseHandle(handle);
                    return Err(error).context("the folder cannot be waited for");
                }
            };
            let overlapped = Box::new(OVERLAPPED {
                hEvent: event,
                ..Default::default()
            });
            Ok(Self {
                folder: handle,
                buffer: vec![0u32; CHANGES_BUFFER / 4].into_boxed_slice(),
                overlapped,
            })
        }
    }

    /// Asks for the next changes; they are waited for on the event.
    fn arm(&mut self) -> Result<()> {
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_ATTRIBUTES
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE;
        // SAFETY: the buffer and the structure are boxed and live until
        // the reading is over, which `Drop` waits for after cancelling.
        unsafe {
            let event = self.overlapped.hEvent;
            *self.overlapped = OVERLAPPED {
                hEvent: event,
                ..Default::default()
            };
            ReadDirectoryChangesW(
                self.folder,
                self.buffer.as_mut_ptr().cast(),
                (self.buffer.len() * 4) as u32,
                false,
                filter,
                None,
                Some(&raw mut *self.overlapped),
                None,
            )
        }
        .context("the changes of the folder cannot be read")
    }

    /// Ends the reading that arrived: whether it went well, with changes
    /// or with none written as more changed than fits; `false` where the
    /// folder went or cannot be reached.
    fn finish(&mut self) -> bool {
        let mut bytes = 0u32;
        // SAFETY: the structure is the one the reading was started with.
        unsafe {
            GetOverlappedResult(
                self.folder,
                &raw const *self.overlapped,
                &raw mut bytes,
                false,
            )
        }
        .is_ok()
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let mut bytes = 0u32;
        // SAFETY: the reading is cancelled and waited for before the
        // buffer and the structure it writes to go; the handles are
        // closed once.
        unsafe {
            if CancelIoEx(self.folder, Some(&raw const *self.overlapped)).is_ok() {
                let _ = GetOverlappedResult(
                    self.folder,
                    &raw const *self.overlapped,
                    &raw mut bytes,
                    true,
                );
            }
            let _ = CloseHandle(self.overlapped.hEvent);
            let _ = CloseHandle(self.folder);
        }
    }
}

/// The body of the watching thread: tells `changed` whenever a folder of
/// `watched` changes, until the event `stop` is set.
fn watch(mut watched: Vec<Watched>, stop: &Event, changed: &Sender<()>) {
    let stop = stop.0;
    watched.retain_mut(|one| {
        one.arm()
            .inspect_err(|error| report!("a folder of the desktop is not watched: {error:#}"))
            .is_ok()
    });
    loop {
        let mut handles = vec![stop];
        handles.extend(watched.iter().map(|one| one.overlapped.hEvent));
        // SAFETY: every handle is open for as long as the wait lasts.
        let woken = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if woken == WAIT_FAILED || woken == WAIT_OBJECT_0 || watched.is_empty() {
            break;
        }
        let index = (woken.0 - WAIT_OBJECT_0.0) as usize;
        let Some(one) = index.checked_sub(1).and_then(|at| watched.get_mut(at)) else {
            break;
        };
        // A change that waits to be read is enough.
        changed.try_send(()).ok();
        // A reading that failed tells that the folder went or cannot be
        // reached any more: it would fail again at once, round after
        // round. The folder is read once more above, and no longer
        // watched.
        if !one.finish() {
            report!("a folder of the desktop is no longer watched: it cannot be reached");
            watched.remove(index - 1);
        } else if one.arm().is_err() {
            watched.remove(index - 1);
        }
        if changed.is_closed() {
            break;
        }
    }
}

/// Explorer's own icons of the desktop, which are hidden while Tessera
/// draws them beside Explorer and shown again afterwards: when Tessera's
/// icons go, when Tessera quits, and by the watchdog that restores the
/// taskbar after Tessera ended in any way, which a mark on the disk tells
/// that they were hidden.
pub struct ExplorerIcons;

impl ExplorerIcons {
    /// Hides the list of icons of Explorer's desktop where it is shown,
    /// and says whether Explorer has one.
    pub fn hide() -> bool {
        let views = folder_views();
        let shown: Vec<HWND> = { views.iter().copied() }
            // SAFETY: the call only reads the state of a window.
            .filter(|&view| unsafe { IsWindowVisible(view) }.as_bool())
            .collect();
        if !shown.is_empty() {
            if let Some(mark) = hidden_mark() {
                if let Some(folder) = mark.parent() {
                    let _ = std::fs::create_dir_all(folder);
                }
                if let Err(error) = std::fs::write(&mark, b"") {
                    report!("Explorer's icons may stay hidden after a crash: {error}");
                }
            }
            for view in shown {
                // SAFETY: the window is Explorer's; the call only posts
                // that it be hidden and does not wait for Explorer, so a
                // hung Explorer never holds Tessera; one that went away
                // is ignored.
                unsafe {
                    let _ = ShowWindowAsync(view, SW_HIDE);
                }
            }
        }
        !views.is_empty()
    }

    /// Shows Explorer's icons again where Tessera hid them.
    pub fn restore() {
        let Some(mark) = hidden_mark().filter(|mark| mark.exists()) else {
            return;
        };
        for view in folder_views() {
            // SAFETY: as in `hide`: the showing is posted, not waited for.
            unsafe {
                let _ = ShowWindowAsync(view, SW_SHOWNOACTIVATE);
            }
        }
        let _ = std::fs::remove_file(mark);
    }
}

/// The file whose presence says that Tessera hid Explorer's icons.
fn hidden_mark() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("tessera")
            .join("explorer-icons-hidden"),
    )
}

/// The lists of icons of Explorer's desktop: the `SysListView32` in the
/// `SHELLDLL_DefView` of its `Progman` or of the `WorkerW` that holds it
/// after the wallpaper was animated.
fn folder_views() -> Vec<HWND> {
    let mut views = Vec::new();
    for class in [w!("Progman"), w!("WorkerW")] {
        for desktop in Taskbar::windows_of_class(class) {
            // SAFETY: the searches only read windows; one that went away
            // makes them fail.
            unsafe {
                let Ok(view) = FindWindowExW(Some(desktop), None, w!("SHELLDLL_DefView"), None)
                else {
                    continue;
                };
                if let Ok(list) = FindWindowExW(Some(view), None, w!("SysListView32"), None) {
                    views.push(list);
                }
            }
        }
    }
    views
}

/// How the window of the icons is made part of the desktop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerKind {
    /// A child window of the desktop: of Tessera's `SHELLDLL_DefView` as
    /// the shell, of a window Tessera keeps at the bottom beside Explorer.
    #[default]
    Child,
    /// A top-level window kept at the bottom of the order of windows.
    Window,
}

/// What Alt+F4 on the window of the icons tells.
fn close_requests() -> &'static (Sender<()>, Receiver<()>) {
    static REQUESTS: OnceLock<(Sender<()>, Receiver<()>)> = OnceLock::new();
    REQUESTS.get_or_init(|| async_channel::bounded(1))
}

/// The windows that own the cards of a display, by display: the window of
/// a layer that is Tessera's own and lives on the thread of GPUI.
static OWNERS: Mutex<Vec<(isize, isize)>> = Mutex::new(Vec::new());

fn owners() -> std::sync::MutexGuard<'static, Vec<(isize, isize)>> {
    OWNERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The windows of GPUI that are layers of the icons, as numbers.
static LAYERS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn layers() -> std::sync::MutexGuard<'static, Vec<isize>> {
    LAYERS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The window of the icons of one display, made part of the desktop,
/// until this is dropped. It is made on the thread of GPUI and stays
/// there.
#[derive(Debug)]
pub struct DesktopLayer {
    surface: NativeSurface,
    kind: LayerKind,
    /// The window the icons are a child of; `None` in [`LayerKind::Window`].
    parent: Option<isize>,
    /// The window Tessera made to hold the icons, which goes with this.
    host: Option<isize>,
}

impl NativeSurface {
    /// Makes the window the layer of the icons of the desktop over
    /// `frame`, as `kind` says, and shows it without taking the keyboard.
    pub fn adopt_as_desktop_layer(
        self,
        kind: LayerKind,
        frame: ScreenRect,
    ) -> Result<DesktopLayer> {
        let mut layer = DesktopLayer {
            surface: self,
            kind,
            parent: None,
            host: None,
        };
        match kind {
            LayerKind::Child => {
                let parent = match ShellHost::def_view_window() {
                    Some(view) => view.raw(),
                    None => {
                        let host = make_host(frame)?;
                        layer.host = Some(host);
                        host
                    }
                };
                layer.parent = Some(parent);
                self.become_child(HWND(parent as *mut c_void))?;
            }
            LayerKind::Window => self.stay_down()?,
        }
        layer.place(frame)?;
        self.reveal(false);
        if let Some(owner) = layer.card_owner() {
            owners().push((self.monitor(), owner));
        }
        layers().push(self.hwnd.0 as isize);
        Ok(layer)
    }

    /// Has the window, a card, owned by the layer of the icons of its
    /// display where there is one of Tessera's own, so that it stays above
    /// the icons; nothing changes where it is owned so already.
    pub fn rest_on_desktop(self) {
        let monitor = self.monitor();
        let kept = owners();
        let owner = { kept.iter() }
            .find(|(display, _)| *display == monitor)
            .map(|&(_, owner)| owner);
        drop(kept);
        // SAFETY: the calls only read and change the owner of a window of
        // this thread, which is the thread the owner lives on as well.
        unsafe {
            let now = GetWindow(self.hwnd, GW_OWNER).map_or(0, |owner| owner.0 as isize);
            let wanted = owner.unwrap_or(0);
            if now != wanted {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, wanted);
            }
        }
    }

    /// The display the window is on, as a number.
    fn monitor(self) -> isize {
        // SAFETY: the call only reads the state of the window.
        unsafe { MonitorFromWindow(self.hwnd, MONITOR_DEFAULTTONEAREST) }.0 as isize
    }

    /// Makes the window a child of `parent`, without a frame or the
    /// styles of a top-level window.
    fn become_child(self, parent: HWND) -> Result<()> {
        // SAFETY: the window belongs to this thread; the styles are
        // changed before the parent, as Windows asks.
        unsafe {
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
            let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE) as u32;
            let style = (style & !(WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0))
                | WS_CHILD.0
                | WS_CLIPSIBLINGS.0;
            SetWindowLongPtrW(self.hwnd, GWL_STYLE, style as isize);
            let dropped = WS_EX_TOPMOST
                | WS_EX_APPWINDOW
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT;
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32 & !dropped.0;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, extended as isize);
            SetParent(self.hwnd, Some(parent)).context("the icons are not let into the desktop")?;
        }
        Ok(())
    }

    /// Keeps the top-level window at the bottom of the order of windows,
    /// owned by Tessera's own desktop window where Tessera is the shell,
    /// and has Alt+F4 on it close nothing.
    fn stay_down(self) -> Result<()> {
        // SAFETY: the window belongs to this thread; the subclass is
        // removed when the window is destroyed or the layer dropped.
        unsafe {
            let extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            let extended = (extended | WS_EX_TOOLWINDOW.0)
                & !(WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_NOACTIVATE).0;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, extended as isize);
            if let Some(desktop) = ShellHost::desktop_window() {
                SetWindowLongPtrW(self.hwnd, GWLP_HWNDPARENT, desktop.raw());
            }
            if !SetWindowSubclass(self.hwnd, Some(keep_down), LAYER_SUBCLASS, 0).as_bool() {
                bail!("the icons cannot be kept below the windows");
            }
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            let _ = SetWindowPos(self.hwnd, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags);
            let _ = SetWindowPos(self.hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
        }
        Ok(())
    }
}

impl DesktopLayer {
    /// Where Alt+F4 on a window of the icons arrives, as the user asks how
    /// to end the session there.
    pub fn close_requests() -> Receiver<()> {
        close_requests().1.clone()
    }

    /// The window of GPUI the icons are drawn in.
    pub fn surface(&self) -> NativeSurface {
        self.surface
    }

    /// Whether the window at the screen point `at` is a layer of the
    /// icons, not covered there by another window.
    pub fn at_point(at: (i32, i32)) -> bool {
        // SAFETY: the call only reads which window is at a point.
        let window = unsafe { WindowFromPoint(POINT { x: at.0, y: at.1 }) };
        layers().contains(&(window.0 as isize))
    }

    /// Puts the layer over `frame`, in pixels of the screen.
    pub fn place(&self, frame: ScreenRect) -> Result<()> {
        let hwnd = self.surface.hwnd;
        let Some(parent) = self.parent.map(|parent| HWND(parent as *mut c_void)) else {
            return self.surface.place(frame);
        };
        let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
        // SAFETY: the windows are alive; the point outlives the call.
        unsafe {
            let mut origin = [POINT {
                x: frame.left,
                y: frame.top,
            }];
            if let Some(host) = self.host {
                let host = HWND(host as *mut c_void);
                SetWindowPos(
                    host,
                    None,
                    frame.left,
                    frame.top,
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOZORDER,
                )
                .context("the desktop could not be moved")?;
                origin[0] = POINT::default();
            } else {
                MapWindowPoints(None, Some(parent), &mut origin);
            }
            SetWindowPos(
                hwnd,
                None,
                origin[0].x,
                origin[0].y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
            .context("the icons could not be moved")?;
            // GPUI may drop the size it was told while it was busy, as
            // `NativeSurface::place` says; it is told once more.
            if (1..=0xffff).contains(&width) && (1..=0xffff).contains(&height) {
                let size = (height as isize) << 16 | width as isize;
                let _ = PostMessageW(Some(hwnd), WM_SIZE, WPARAM(0), LPARAM(size));
            }
        }
        Ok(())
    }

    /// Gives the keyboard to the icons, as a click on them does in
    /// Explorer.
    pub fn focus(&self) {
        let hwnd = self.surface.hwnd;
        // SAFETY: the windows are alive; a refusal changes nothing.
        unsafe {
            let root = GetAncestor(hwnd, GA_ROOT);
            let _ = SetForegroundWindow(if root.is_invalid() { hwnd } else { root });
            let _ = SetFocus(Some(hwnd));
        }
    }

    /// The window that owns the cards of the display: the window of the
    /// layer that is Tessera's own, on this thread.
    fn card_owner(&self) -> Option<isize> {
        match (self.kind, self.host) {
            (LayerKind::Window, _) => Some(self.surface.hwnd.0 as isize),
            (LayerKind::Child, Some(host)) => Some(host),
            // Within Tessera's own desktop the cards are above it anyway.
            (LayerKind::Child, None) => None,
        }
    }
}

impl Drop for DesktopLayer {
    fn drop(&mut self) {
        let hwnd = self.surface.hwnd;
        layers().retain(|&layer| layer != hwnd.0 as isize);
        if let Some(owner) = self.card_owner() {
            owners().retain(|&(_, kept)| kept != owner);
            give_up_owned(owner);
        }
        // SAFETY: the windows belong to this thread. The window of GPUI
        // is taken out of the host before the host goes, which would take
        // it along; GPUI closes it itself.
        unsafe {
            match self.kind {
                LayerKind::Window => {
                    let _ = RemoveWindowSubclass(hwnd, Some(keep_down), LAYER_SUBCLASS);
                }
                LayerKind::Child => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    let _ = SetParent(hwnd, None);
                    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
                    let style = (style & !WS_CHILD.0) | WS_POPUP.0;
                    SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
                }
            }
            if let Some(host) = self.host {
                let _ = DestroyWindow(HWND(host as *mut c_void));
            }
        }
    }
}

/// Has the windows of this thread that `owner` owns owned by nothing.
fn give_up_owned(owner: isize) {
    unsafe extern "system" fn give_up(window: HWND, owner: LPARAM) -> BOOL {
        // SAFETY: the window is one of this thread that the enumeration
        // gives; only its owner is read and changed.
        unsafe {
            if GetWindow(window, GW_OWNER).is_ok_and(|now| now.0 as isize == owner.0) {
                SetWindowLongPtrW(window, GWLP_HWNDPARENT, 0);
            }
        }
        true.into()
    }
    // SAFETY: the enumeration is synchronous and calls back on this thread.
    unsafe {
        let _ = EnumThreadWindows(GetCurrentThreadId(), Some(give_up), LPARAM(owner));
    }
}

/// Makes the window that holds the icons beside Explorer, over `frame`:
/// drawn by nothing itself, kept at the bottom, owned by Tessera's own
/// desktop window where Tessera is the shell.
fn make_host(frame: ScreenRect) -> Result<isize> {
    static REGISTERED: OnceLock<bool> = OnceLock::new();
    let class: Vec<u16> = LAYER_CLASS.encode_utf16().chain([0]).collect();
    // SAFETY: the class structure and the names outlive the calls; the
    // window belongs to this thread.
    unsafe {
        let module = GetModuleHandleW(None).context("the program has no module")?;
        let registered = *REGISTERED.get_or_init(|| {
            let description = WNDCLASSW {
                lpfnWndProc: Some(respond_host),
                hInstance: module.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&raw const description) != 0
        });
        if !registered {
            bail!("the class of the desktop could not be registered");
        }
        let owner = ShellHost::desktop_window().map(|desktop| HWND(desktop.raw() as *mut c_void));
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            PCWSTR(class.as_ptr()),
            w!(""),
            WS_POPUP | WS_CLIPCHILDREN,
            frame.left,
            frame.top,
            frame.right - frame.left,
            frame.bottom - frame.top,
            owner,
            None,
            Some(module.into()),
            None,
        )
        .context("the desktop of the icons could not be made")?;
        let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        let _ = SetWindowPos(host, Some(HWND_BOTTOM), 0, 0, 0, 0, flags);
        Ok(host.0 as isize)
    }
}

/// Tells that Alt+F4 was pressed on the desktop; a request that waits is
/// enough.
fn request_close() {
    close_requests().0.try_send(()).ok();
}

/// Has a window that moves in the order of windows stay at the bottom.
///
/// # Safety
/// `position` must be the `WINDOWPOS` of `WM_WINDOWPOSCHANGING`.
unsafe fn keep_at_bottom(position: LPARAM) {
    let position = position.0 as *mut WINDOWPOS;
    // SAFETY: the caller passes the structure Windows gave.
    unsafe {
        if let Some(position) = position.as_mut()
            && position.flags & SWP_NOZORDER != SWP_NOZORDER
        {
            position.hwndInsertAfter = HWND_BOTTOM;
        }
    }
}

/// Whether `message` with `w` asks the window to close, as Alt+F4 does.
fn asks_to_close(message: u32, w: WPARAM) -> bool {
    message == WM_CLOSE || (message == WM_SYSCOMMAND && w.0 as u32 & 0xFFF0 == SC_CLOSE)
}

/// The window procedure of the window that holds the icons beside
/// Explorer.
unsafe extern "system" fn respond_host(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
) -> LRESULT {
    match message {
        WM_WINDOWPOSCHANGING => {
            // SAFETY: `lParam` of this message is its `WINDOWPOS`.
            unsafe { keep_at_bottom(l) };
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            // SAFETY: the paint structure outlives both calls.
            unsafe {
                BeginPaint(window, &raw mut paint);
                let _ = EndPaint(window, &raw const paint);
            }
            return LRESULT(0);
        }
        // The keys go to the icons inside.
        WM_SETFOCUS => {
            // SAFETY: the child is a window of this thread.
            unsafe {
                if let Ok(icons) = GetWindow(window, GW_CHILD) {
                    let _ = SetFocus(Some(icons));
                }
            }
            return LRESULT(0);
        }
        _ if asks_to_close(message, w) => {
            request_close();
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: the arguments are passed on as Windows gave them.
    unsafe { DefWindowProcW(window, message, w, l) }
}

/// The subclass of a top-level window of the icons: it stays at the
/// bottom, and Alt+F4 closes nothing.
unsafe extern "system" fn keep_down(
    window: HWND,
    message: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    _: usize,
) -> LRESULT {
    match message {
        WM_WINDOWPOSCHANGING => {
            // SAFETY: `lParam` of this message is its `WINDOWPOS`.
            unsafe { keep_at_bottom(l) };
        }
        WM_NCDESTROY => {
            // SAFETY: the subclass is this one, on its own window.
            unsafe {
                let _ = RemoveWindowSubclass(window, Some(keep_down), LAYER_SUBCLASS);
            }
        }
        _ if asks_to_close(message, w) => {
            request_close();
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: the arguments are passed on as Windows gave them.
    unsafe { DefSubclassProc(window, message, w, l) }
}

/// The frame of the layer of a display whose room for windows is `work`
/// and whose whole is `display`: the room, one pixel shorter where it is
/// the whole display, so that Windows does not take the desktop in front
/// for an application shown full screen.
pub fn layer_frame(work: ScreenRect, display: ScreenRect) -> ScreenRect {
    let mut frame = work;
    if work == display && frame.bottom - frame.top > 1 {
        frame.bottom -= 1;
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_is_shown_as_explorer_wrote_or_as_windows_has_it() {
        assert!(shown(Some(0), false));
        assert!(!shown(Some(1), true));
        assert!(shown(None, true));
        assert!(!shown(None, false));
    }

    #[test]
    fn only_the_recycle_bin_is_shown_by_default() {
        let shown: Vec<_> = { VirtualKind::ALL.into_iter() }
            .filter(|kind| kind.shown_by_default())
            .collect();
        assert_eq!(shown, [VirtualKind::RecycleBin]);
    }

    #[test]
    fn a_layer_over_a_whole_display_is_a_pixel_shorter() {
        let display = ScreenRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(layer_frame(display, display).bottom, 1079);
        let work = ScreenRect {
            bottom: 1032,
            ..display
        };
        assert_eq!(layer_frame(work, display), work);
    }

    #[test]
    fn the_class_of_the_layer_is_short_enough_to_be_read_whole() {
        assert!(LAYER_CLASS.len() <= 15);
    }
}
