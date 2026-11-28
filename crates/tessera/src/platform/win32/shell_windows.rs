// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The list of the folder windows of the Shell, `ShellWindows`, served by
//! Tessera where Explorer does not run, with the automation objects of
//! the windows and of the desktop.
//!
//! Programs that show a file in its folder ("Show in folder" of browsers,
//! editors and download managers) call `SHOpenFolderAndSelectItems`,
//! which asks the object of the class `CLSID_ShellWindows` for a window
//! that shows the folder (`IShellWindows::FindWindowSW`), and has that
//! window choose the files: through `IServiceProvider` to the
//! `IShellBrowser` of the window (`SID_STopLevelBrowser`), its active
//! `IShellView`, and `IShellView::SelectItem`. Scripts list and steer the
//! windows through `Shell.Application.Windows()`, which is this list too,
//! by late binding. Explorer serves that class from its process; without
//! Explorer nobody does.
//!
//! As the shell, Tessera registers a class object for `CLSID_ShellWindows`
//! on the shell thread (`CoRegisterClassObject`, local server, for many
//! uses) whose list holds:
//!
//! - Each window of Tessera Files, as an object with `IWebBrowser2` (its
//!   window, `LocationURL` as `file:///…` for a folder and the parsing
//!   name otherwise, `LocationName`, `Navigate` and `Navigate2`, which
//!   send the window elsewhere, `Refresh`, `Quit`, and `Document`, its
//!   `IShellFolderViewDual`), `IServiceProvider`, `IShellBrowser` and
//!   `IShellView`, whose `SelectItem` hands the names chosen to the file
//!   manager.
//! - The windows of other programs that register themselves.
//! - The desktop, for `FindWindowSW` with `SWC_DESKTOP`: its object leads
//!   through `IServiceProvider`, `IShellBrowser::QueryActiveShellView` and
//!   `IShellView::GetItemObject(SVGIO_BACKGROUND)` to an
//!   `IShellFolderViewDual` whose `Application` is the Shell's
//!   `Shell.Application` made in Tessera's process, which is how
//!   installers start programs without elevation. Its view is also the
//!   desktop's `IFolderView` (see desktop_view.rs).
//!
//! `IDispatch` of every object is that of its interface's type
//! information (see automation.rs), so `Item`, `_NewEnum`, `Count` and the
//! properties of the windows work from scripts. Navigation and choices
//! are posted to the thread of GPUI, never waited for there. Beside
//! Explorer nothing is registered: the list stays Explorer's.
//!
//! Whether `SHOpenFolderAndSelectItems` and `CoCreateInstance` of other
//! processes find a class object registered this way is not verified on
//! Windows: the class is registered by Explorer under `HKCR\CLSID`, and
//! whether the activation of Windows reaches a running object of a
//! process that is not the one registered there is the open question.
#![allow(unsafe_code)]

mod automation;
mod folder_view;

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, LRESULT, S_FALSE,
    SHANDLE_PTR, VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoRegisterClassObject, IClassFactory, IClassFactory_Impl, IDispatch,
    IServiceProvider, IServiceProvider_Impl, IStream, REGCLS_MULTIPLEUSE,
};
use windows::Win32::System::Ole::{
    IOleWindow_Impl, OLECMDEXECOPT, OLECMDF, OLECMDID, OLEMENUGROUPWIDTHS, READYSTATE,
    READYSTATE_COMPLETE,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    _SVGIO, FOLDERSETTINGS, Folder, IShellBrowser, IShellBrowser_Impl, IShellFolder, IShellView,
    IShellView_Impl, IShellWindows, IShellWindows_Impl, IWebBrowser_Impl, IWebBrowser2,
    IWebBrowser2_Impl, IWebBrowserApp_Impl, SHBindToObject, SHGDN_FORPARSING, SHGDN_INFOLDER,
    SHGetDesktopFolder, SID_STopLevelBrowser, SVSI_DESELECTOTHERS, SVSI_SELECT, SWC_DESKTOP,
    SWFO_COOKIEPASSED, SWFO_INCLUDEPENDING, ShellWindowFindWindowOptions, ShellWindowTypeConstants,
    ShellWindows as CLSID_SHELL_WINDOWS, StrRetToStrW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HMENU, IsWindowVisible, MSG, PostMessageW, WM_CLOSE,
};
use windows::core::{
    BOOL, BSTR, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, implement,
};

use super::shell_item::IdList;
use super::{ShellHost, ShellThread};
use automation::{
    Given, Library, Objects, OwnedVariant, boolean, display_name_of, file_url, folder_of, given,
    is_path, late_binding, parsing_of, path_of_text,
};
pub(super) use folder_view::{Child, children_of, item_object};

/// The names a program asked a window of Tessera Files to choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The window, by the number of its handle.
    pub window: isize,
    /// The names in its folder, the first one first.
    pub names: Vec<OsString>,
}

/// A place a program asked a window of Tessera Files to go to, or, for
/// the desktop's window, to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Navigation {
    /// The window, by the number of its handle; the desktop's where a
    /// new window is asked for.
    pub window: isize,
    /// The parsing name of the place: a path, or a name of the Shell.
    pub parsing: String,
    /// Whether the program asked for the desktop's window, which opens a
    /// new window rather than going elsewhere.
    pub new_window: bool,
}

/// The name an item has in its folder for parsing, as `SHGDN_INFOLDER |
/// SHGDN_FORPARSING`.
pub(super) const NAME_IN_FOLDER: windows::Win32::UI::Shell::SHGDNF =
    windows::Win32::UI::Shell::SHGDNF(SHGDN_INFOLDER.0 | SHGDN_FORPARSING.0);

/// Whether the list is served, which only the shell does.
static SERVING: AtomicBool = AtomicBool::new(false);

/// Where the selections asked for go.
static SELECTIONS: OnceLock<Sender<Selection>> = OnceLock::new();

/// Where the navigations asked for go, and where they are taken from.
static NAVIGATIONS: OnceLock<(Sender<Navigation>, Receiver<Navigation>)> = OnceLock::new();

/// A window in the list.
struct Entry {
    cookie: i32,
    /// The window, by the number of its handle; zero while it is pending.
    window: isize,
    folder: Option<PathBuf>,
    /// The parsing name of the place it shows, where the file manager
    /// told it; a folder's path otherwise.
    parsing: Option<String>,
    /// What the list gives for it: an object of Tessera's for its own
    /// windows, the one a program registered for others.
    dispatch: Option<IDispatch>,
    /// Whether it is one of Tessera Files.
    own: bool,
    /// Whether it was registered as pending, before its window exists.
    pending: bool,
    /// The names programs chose in it through its objects.
    chosen: Vec<String>,
}

impl Entry {
    /// The parsing name of the place it shows.
    fn parsing(&self) -> Option<String> {
        { self.parsing.clone() }.or_else(|| {
            { self.folder.as_ref() }.map(|folder| folder.to_string_lossy().into_owned())
        })
    }
}

thread_local! {
    /// The windows in the list, on the shell thread.
    static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    /// The cookie the next window gets.
    static NEXT_COOKIE: Cell<i32> = const { Cell::new(1) };
    /// The one list, which every client gets.
    static LIST: RefCell<Option<IShellWindows>> = const { RefCell::new(None) };
    /// The desktop's object, made the first time a program asks.
    static DESKTOP: RefCell<Option<IDispatch>> = const { RefCell::new(None) };
    /// The names programs chose on the desktop through its objects.
    static DESKTOP_CHOSEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A place whose view the automation objects describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Place {
    /// A window of Tessera Files, by the number of its handle.
    Window(isize),
    Desktop,
}

impl Place {
    /// The parsing name of what it shows; for the desktop, its folder.
    fn parsing(self) -> Option<String> {
        match self {
            Self::Window(window) => with_entry(window, Entry::parsing).flatten(),
            Self::Desktop => parsing_of(&Given::Number(0)),
        }
    }

    /// What `Shell.Application::NameSpace` takes for it.
    fn namespace(self) -> OwnedVariant {
        match (self, self.parsing()) {
            (Self::Desktop, _) => OwnedVariant::number(0),
            (Self::Window(_), Some(parsing)) => OwnedVariant::text(&parsing),
            (Self::Window(_), None) => OwnedVariant::empty(),
        }
    }

    /// The Shell's `Folder` of it.
    fn folder(self) -> windows::core::Result<Folder> {
        folder_of(&self.namespace())
    }

    /// The Shell's folder object of it.
    pub(super) fn shell_folder(self) -> windows::core::Result<IShellFolder> {
        // SAFETY: the call takes no arguments.
        let desktop = unsafe { SHGetDesktopFolder() }?;
        let parsing = match self {
            Self::Desktop => return Ok(desktop),
            Self::Window(_) => self
                .parsing()
                .ok_or(windows::core::Error::from(E_INVALIDARG))?,
        };
        let list = IdList::of_path(Path::new(&parsing))
            .map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        // SAFETY: the list is alive for the call.
        unsafe { SHBindToObject(&desktop, list.as_ptr(), None) }
    }

    /// The names programs chose in it.
    pub(super) fn chosen(self) -> Vec<String> {
        match self {
            Self::Window(window) => {
                with_entry(window, |entry| entry.chosen.clone()).unwrap_or_default()
            }
            Self::Desktop => DESKTOP_CHOSEN.with(|chosen| chosen.borrow().clone()),
        }
    }

    /// Chooses the item called `name` as the flags `SVSI_*` say, and hands
    /// what is chosen in a window to the file manager.
    pub(super) fn choose(self, name: Option<String>, flags: i32) {
        match self {
            Self::Window(window) => {
                let chosen = ENTRIES.with(|entries| {
                    let mut entries = entries.borrow_mut();
                    let entry =
                        { entries.iter_mut() }.find(|entry| entry.own && entry.window == window)?;
                    choose_in(&mut entry.chosen, name, flags);
                    Some(entry.chosen.clone())
                });
                if let (Some(chosen), Some(selections)) = (chosen, SELECTIONS.get()) {
                    let names = chosen.into_iter().map(OsString::from).collect();
                    let _ = selections.try_send(Selection { window, names });
                }
            }
            Self::Desktop => {
                DESKTOP_CHOSEN.with(|chosen| choose_in(&mut chosen.borrow_mut(), name, flags));
            }
        }
    }
}

/// Chooses `name` in `chosen` as the flags of `SelectItem` say: the others
/// are let go with `SVSI_DESELECTOTHERS`, the item is chosen with
/// `SVSI_SELECT` and let go without it.
fn choose_in(chosen: &mut Vec<String>, name: Option<String>, flags: i32) {
    if flags & SVSI_DESELECTOTHERS.0 != 0 {
        chosen.clear();
    }
    let Some(name) = name else {
        return;
    };
    chosen.retain(|other| !other.eq_ignore_ascii_case(&name));
    if flags & SVSI_SELECT.0 != 0 {
        chosen.push(name);
    }
}

/// What `read` gives of the entry of the window `window` of Tessera Files.
fn with_entry<T>(window: isize, read: impl FnOnce(&Entry) -> T) -> Option<T> {
    ENTRIES.with(|entries| {
        let entries = entries.borrow();
        { entries.iter() }
            .find(|entry| entry.own && entry.window == window)
            .map(read)
    })
}

/// The list of the folder windows of the Shell, as Tessera serves it.
pub struct ShellWindows;

impl ShellWindows {
    /// Serves the list from now on, on the shell thread, and gives the
    /// selections programs ask of the windows of Tessera Files. To be
    /// called once, where Tessera is the shell.
    pub fn serve() -> Receiver<Selection> {
        let (sender, selections) = async_channel::unbounded();
        if SELECTIONS.set(sender).is_err() {
            report!("the list of folder windows is served already");
            return selections;
        }
        SERVING.store(true, Ordering::SeqCst);
        ShellThread::run(|| {
            let factory: IClassFactory = ListFactory.into();
            // SAFETY: the class is a constant and the factory is a live
            // object of this thread, which pumps messages for as long as
            // the process lives; the registration is never revoked.
            let registered = unsafe {
                CoRegisterClassObject(
                    &CLSID_SHELL_WINDOWS,
                    &factory,
                    CLSCTX_LOCAL_SERVER,
                    REGCLS_MULTIPLEUSE,
                )
            };
            if let Err(error) = registered {
                report!("the list of folder windows is not served: {error}");
            }
        });
        selections
    }

    /// The places programs asked windows of Tessera Files to go to, and
    /// the windows they asked the desktop to open; one receiver takes
    /// each.
    pub fn navigations() -> Receiver<Navigation> {
        NAVIGATIONS.get_or_init(async_channel::unbounded).1.clone()
    }

    /// Puts the window `window` of Tessera Files, at `folder`, in the list.
    pub fn opened(window: isize, folder: Option<PathBuf>) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            let cookie = next_cookie();
            let object: IWebBrowser2 = FolderWindow {
                cookie,
                window,
                desktop: false,
            }
            .into();
            let dispatch = object.cast::<IDispatch>().ok();
            ENTRIES.with(|entries| {
                entries.borrow_mut().push(Entry {
                    cookie,
                    window,
                    folder,
                    parsing: None,
                    dispatch,
                    own: true,
                    pending: false,
                    chosen: Vec::new(),
                });
            });
        });
    }

    /// Notes that the window `window` of Tessera Files shows `folder` now.
    pub fn navigated(window: isize, folder: Option<PathBuf>) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            ENTRIES.with(|entries| {
                let mut entries = entries.borrow_mut();
                if let Some(entry) = { entries.iter_mut() }.find(|entry| entry.window == window) {
                    entry.folder = folder;
                    entry.chosen.clear();
                }
            });
        });
    }

    /// Notes the parsing name of the place the window `window` of Tessera
    /// Files shows, for the places that are no folder (This PC, the
    /// Recycle Bin, the places of the Shell).
    pub fn named(window: isize, parsing: Option<String>) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            ENTRIES.with(|entries| {
                let mut entries = entries.borrow_mut();
                let found =
                    { entries.iter_mut() }.find(|entry| entry.own && entry.window == window);
                if let Some(entry) = found {
                    entry.parsing = parsing;
                }
            });
        });
    }

    /// Takes the window `window` of Tessera Files out of the list.
    pub fn closed(window: isize) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            ENTRIES.with(|entries| {
                entries
                    .borrow_mut()
                    .retain(|entry| !(entry.own && entry.window == window));
            });
        });
    }
}

fn next_cookie() -> i32 {
    NEXT_COOKIE.with(|next| {
        let cookie = next.get();
        next.set(cookie.wrapping_add(1).max(1));
        cookie
    })
}

/// A path as the list compares it: without case, and without a backslash
/// at its end other than that of a root.
fn compared(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
    let trimmed = text.trim_end_matches('\\');
    match trimmed.ends_with(':') || trimmed.is_empty() {
        true => format!("{trimmed}\\"),
        false => trimmed.to_owned(),
    }
}

/// A parsing name as the list compares it: a path as [`compared`] does,
/// another name without case.
pub(super) fn key_of(parsing: &str) -> String {
    match is_path(parsing) {
        true => compared(&path_of_text(parsing)),
        false => parsing.trim().to_lowercase(),
    }
}

/// The desktop's automation object, made once on this thread.
fn desktop_object() -> windows::core::Result<IDispatch> {
    DESKTOP.with(|desktop| {
        if let Some(object) = desktop.borrow().as_ref() {
            return Ok(object.clone());
        }
        let window = ShellHost::desktop_window().map_or(0, |window| window.raw());
        let object: IWebBrowser2 = FolderWindow {
            cookie: 0,
            window,
            desktop: true,
        }
        .into();
        let object: IDispatch = object.cast()?;
        *desktop.borrow_mut() = Some(object.clone());
        Ok(object)
    })
}

/// Hands a navigation to the thread of GPUI.
fn navigate(window: isize, parsing: String, new_window: bool) {
    let (sender, _) = NAVIGATIONS.get_or_init(async_channel::unbounded);
    let _ = sender.try_send(Navigation {
        window,
        parsing,
        new_window,
    });
}

/// The class object of the list, which gives every client the one list.
#[implement(IClassFactory, Agile = false)]
struct ListFactory;

impl IClassFactory_Impl for ListFactory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let list = LIST.with(|list| {
            list.borrow_mut()
                .get_or_insert_with(|| FolderList.into())
                .clone()
        });
        // SAFETY: the caller gives the interface it wants and where to put
        // it, as `CreateInstance` is called.
        unsafe { list.query(iid, object) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The list: what `CLSID_ShellWindows` gives.
#[implement(IShellWindows, Agile = false)]
struct FolderList;

late_binding!(FolderList_Impl, IShellWindows, Library::InternetControls);

impl FolderList_Impl {
    /// The objects of the windows that are not pending, in order.
    fn objects(&self) -> Vec<IDispatch> {
        ENTRIES.with(|entries| {
            let entries = entries.borrow();
            { entries.iter() }
                .filter(|entry| !entry.pending)
                .filter_map(|entry| entry.dispatch.clone())
                .collect()
        })
    }
}

impl IShellWindows_Impl for FolderList_Impl {
    fn Count(&self) -> windows::core::Result<i32> {
        Ok(self.objects().len() as i32)
    }

    fn Item(&self, index: &VARIANT) -> windows::core::Result<IDispatch> {
        // SAFETY: the variant is the one the caller passed.
        let index = match unsafe { given(index) } {
            Given::Number(index) => index,
            Given::Nothing => 0,
            _ => return Err(E_INVALIDARG.into()),
        };
        let index = usize::try_from(index).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        self.objects()
            .into_iter()
            .nth(index)
            .ok_or_else(|| S_FALSE.into())
    }

    fn _NewEnum(&self) -> windows::core::Result<IUnknown> {
        let objects: windows::Win32::System::Ole::IEnumVARIANT =
            Objects::new(self.objects()).into();
        objects.cast()
    }

    fn Register(
        &self,
        dispatch: Ref<'_, IDispatch>,
        window: i32,
        _class: ShellWindowTypeConstants,
    ) -> windows::core::Result<i32> {
        let cookie = next_cookie();
        let dispatch = dispatch.cloned();
        let parsing = dispatch.as_ref().and_then(location_of);
        ENTRIES.with(|entries| {
            entries.borrow_mut().push(Entry {
                cookie,
                window: window as isize,
                folder: None,
                parsing,
                dispatch,
                own: false,
                pending: false,
                chosen: Vec::new(),
            });
        });
        Ok(cookie)
    }

    fn RegisterPending(
        &self,
        _thread: i32,
        location: *const VARIANT,
        _root: *const VARIANT,
        _class: ShellWindowTypeConstants,
    ) -> windows::core::Result<i32> {
        let cookie = next_cookie();
        // SAFETY: the variant is the one the caller passed.
        let parsing = parsing_of(&unsafe { given(location) });
        ENTRIES.with(|entries| {
            entries.borrow_mut().push(Entry {
                cookie,
                window: 0,
                folder: None,
                parsing,
                dispatch: None,
                own: false,
                pending: true,
                chosen: Vec::new(),
            });
        });
        Ok(cookie)
    }

    fn Revoke(&self, cookie: i32) -> windows::core::Result<()> {
        ENTRIES.with(|entries| {
            entries
                .borrow_mut()
                .retain(|entry| entry.own || entry.cookie != cookie);
        });
        Ok(())
    }

    fn OnNavigate(&self, cookie: i32, location: *const VARIANT) -> windows::core::Result<()> {
        // SAFETY: the variant is the one the caller passed.
        let parsing = parsing_of(&unsafe { given(location) });
        ENTRIES.with(|entries| {
            let mut entries = entries.borrow_mut();
            if let Some(entry) = { entries.iter_mut() }.find(|entry| entry.cookie == cookie) {
                entry.parsing = parsing;
            }
        });
        Ok(())
    }

    fn OnActivated(&self, _cookie: i32, _active: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn FindWindowSW(
        &self,
        location: *const VARIANT,
        _root: *const VARIANT,
        class: ShellWindowTypeConstants,
        window: *mut i32,
        options: ShellWindowFindWindowOptions,
    ) -> windows::core::Result<IDispatch> {
        let write_window = |handle: isize| {
            if !window.is_null() {
                // SAFETY: the caller gives where the window is to be written.
                unsafe { window.write(handle as i32) };
            }
        };
        // The desktop, as "ShellExecute from the desktop" asks for it.
        if class.0 & SWC_DESKTOP.0 != 0 {
            write_window(ShellHost::desktop_window().map_or(0, |window| window.raw()));
            return desktop_object();
        }
        let by_cookie = options.0 & SWFO_COOKIEPASSED.0 != 0;
        let pending = options.0 & SWFO_INCLUDEPENDING.0 != 0;
        // SAFETY: the variant is the one the caller passed.
        let location = unsafe { given(location) };
        let (cookie, key) = match (by_cookie, &location) {
            (true, Given::Number(number)) => (Some(*number as i32), None),
            (true, _) => (None, None),
            (false, location) => (None, parsing_of(location).map(|parsing| key_of(&parsing))),
        };
        let found = ENTRIES.with(|entries| {
            let entries = entries.borrow();
            let entry = { entries.iter() }
                .filter(|entry| pending || !entry.pending)
                .find(|entry| match (cookie, &key) {
                    (Some(cookie), _) => entry.cookie == cookie,
                    (None, Some(key)) => {
                        entry.parsing().map(|parsing| key_of(&parsing)).as_ref() == Some(key)
                    }
                    (None, None) => false,
                })?;
            Some((entry.window, entry.dispatch.clone()))
        });
        let Some((handle, dispatch)) = found else {
            return Err(S_FALSE.into());
        };
        write_window(handle);
        // A window registered as pending has no object yet.
        dispatch.ok_or_else(|| S_FALSE.into())
    }

    fn OnCreated(&self, cookie: i32, created: Ref<'_, IUnknown>) -> windows::core::Result<()> {
        let dispatch = created
            .cloned()
            .and_then(|object| object.cast::<IDispatch>().ok());
        ENTRIES.with(|entries| {
            let mut entries = entries.borrow_mut();
            if let Some(entry) = { entries.iter_mut() }.find(|entry| entry.cookie == cookie) {
                entry.pending = false;
                entry.dispatch = dispatch;
            }
        });
        Ok(())
    }

    fn ProcessAttachDetach(&self, _attach: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The place a window of another program shows, as its `LocationURL`
/// says.
fn location_of(dispatch: &IDispatch) -> Option<String> {
    let browser: windows::Win32::UI::Shell::IWebBrowserApp = dispatch.cast().ok()?;
    // SAFETY: the call only reads the address of the window.
    let url = unsafe { browser.LocationURL() }.ok()?;
    parsing_of(&Given::Text(url.to_string()))
}

/// A window of Tessera Files, or the desktop, as the list gives it.
#[implement(
    IWebBrowser2,
    IServiceProvider,
    IShellBrowser,
    IShellView,
    Agile = false
)]
struct FolderWindow {
    cookie: i32,
    /// The window, by the number of its handle.
    window: isize,
    /// Whether it is the desktop, whose view is desktop_view.rs's.
    desktop: bool,
}

impl FolderWindow {
    fn handle(&self) -> HWND {
        HWND(self.window as *mut core::ffi::c_void)
    }

    fn place(&self) -> Place {
        match self.desktop {
            true => Place::Desktop,
            false => Place::Window(self.window),
        }
    }

    /// The parsing name of the place the window shows now.
    fn parsing(&self) -> Option<String> {
        if self.desktop {
            return self.place().parsing();
        }
        ENTRIES.with(|entries| {
            let entries = entries.borrow();
            { entries.iter() }
                .find(|entry| entry.cookie == self.cookie)
                .and_then(Entry::parsing)
        })
    }

    /// Sends the window to the place `parsing` names; the desktop has a
    /// new window opened there.
    fn go(&self, parsing: Option<String>) -> windows::core::Result<()> {
        let parsing = parsing.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        navigate(self.window, parsing, self.desktop);
        Ok(())
    }
}

late_binding!(FolderWindow_Impl, IWebBrowser2, Library::InternetControls);

/// What a window does not do: move by a script, show bars of a browser.
fn not_done<T>() -> windows::core::Result<T> {
    Err(E_NOTIMPL.into())
}

impl IWebBrowser_Impl for FolderWindow_Impl {
    fn GoBack(&self) -> windows::core::Result<()> {
        not_done()
    }
    fn GoForward(&self) -> windows::core::Result<()> {
        not_done()
    }
    fn GoHome(&self) -> windows::core::Result<()> {
        self.go(Some("shell:Home".to_owned()))
    }
    fn GoSearch(&self) -> windows::core::Result<()> {
        not_done()
    }
    fn Navigate(
        &self,
        url: &BSTR,
        _flags: *const VARIANT,
        _frame: *const VARIANT,
        _post: *const VARIANT,
        _headers: *const VARIANT,
    ) -> windows::core::Result<()> {
        self.go(parsing_of(&Given::Text(url.to_string())))
    }
    fn Refresh(&self) -> windows::core::Result<()> {
        // The desktop shows what is in its folder anyway.
        if self.desktop {
            return Ok(());
        }
        self.go(self.parsing())
    }
    fn Refresh2(&self, _level: *const VARIANT) -> windows::core::Result<()> {
        IWebBrowser_Impl::Refresh(self)
    }
    fn Stop(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn Application(&self) -> windows::core::Result<IDispatch> {
        automation::shell_application()?.cast()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.to_interface::<IWebBrowser2>().cast()
    }
    fn Container(&self) -> windows::core::Result<IDispatch> {
        // A window at the top has no container: S_OK and no object, as
        // documented.
        Err(windows::core::Error::empty())
    }
    fn Document(&self) -> windows::core::Result<IDispatch> {
        let view: windows::Win32::UI::Shell::IShellFolderViewDual =
            folder_view::FolderViewDual::new(self.place()).into();
        view.cast()
    }
    fn TopLevelContainer(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Type(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("Tessera Files"))
    }
    fn Left(&self) -> windows::core::Result<i32> {
        not_done()
    }
    fn SetLeft(&self, _left: i32) -> windows::core::Result<()> {
        not_done()
    }
    fn Top(&self) -> windows::core::Result<i32> {
        not_done()
    }
    fn SetTop(&self, _top: i32) -> windows::core::Result<()> {
        not_done()
    }
    fn Width(&self) -> windows::core::Result<i32> {
        not_done()
    }
    fn SetWidth(&self, _width: i32) -> windows::core::Result<()> {
        not_done()
    }
    fn Height(&self) -> windows::core::Result<i32> {
        not_done()
    }
    fn SetHeight(&self, _height: i32) -> windows::core::Result<()> {
        not_done()
    }
    fn LocationName(&self) -> windows::core::Result<BSTR> {
        let parsing = self.parsing().unwrap_or_default();
        let name = display_name_of(&parsing).unwrap_or_else(|| {
            let path = path_of_text(&parsing);
            { path.file_name() }.map_or(parsing.clone(), |name| name.to_string_lossy().into_owned())
        });
        Ok(BSTR::from(name))
    }
    fn LocationURL(&self) -> windows::core::Result<BSTR> {
        let url = match self.parsing() {
            Some(parsing) if is_path(&parsing) => file_url(&path_of_text(&parsing)),
            Some(parsing) => parsing,
            None => String::new(),
        };
        Ok(BSTR::from(url))
    }
    fn Busy(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for FolderWindow_Impl {
    fn Quit(&self) -> windows::core::Result<()> {
        // The desktop is not closed by a script.
        if self.desktop {
            return not_done();
        }
        // SAFETY: posting to a window that is gone fails without harm.
        unsafe { PostMessageW(Some(self.handle()), WM_CLOSE, WPARAM(0), LPARAM(0)) }
    }
    fn ClientToWindow(&self, _width: *mut i32, _height: *mut i32) -> windows::core::Result<()> {
        not_done()
    }
    fn PutProperty(&self, _property: &BSTR, _value: &VARIANT) -> windows::core::Result<()> {
        not_done()
    }
    fn GetProperty(&self, _property: &BSTR) -> windows::core::Result<VARIANT> {
        not_done()
    }
    fn Name(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from("Tessera Files"))
    }
    fn HWND(&self) -> windows::core::Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.window))
    }
    fn FullName(&self) -> windows::core::Result<BSTR> {
        let program = std::env::current_exe().unwrap_or_default();
        Ok(BSTR::from(program.display().to_string()))
    }
    fn Path(&self) -> windows::core::Result<BSTR> {
        let program = std::env::current_exe().unwrap_or_default();
        let folder = program.parent().map(Path::to_owned).unwrap_or_default();
        Ok(BSTR::from(format!("{}\\", folder.display())))
    }
    fn Visible(&self) -> windows::core::Result<VARIANT_BOOL> {
        // SAFETY: the call tolerates a window that is gone.
        Ok(boolean(unsafe { IsWindowVisible(self.handle()) }.as_bool()))
    }
    fn SetVisible(&self, _visible: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn StatusBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetStatusBar(&self, _shown: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
    fn StatusText(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::new())
    }
    fn SetStatusText(&self, _text: &BSTR) -> windows::core::Result<()> {
        not_done()
    }
    fn ToolBar(&self) -> windows::core::Result<i32> {
        Ok(1)
    }
    fn SetToolBar(&self, _shown: i32) -> windows::core::Result<()> {
        not_done()
    }
    fn MenuBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetMenuBar(&self, _shown: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
    fn FullScreen(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetFullScreen(&self, _full: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
}

impl IWebBrowser2_Impl for FolderWindow_Impl {
    fn Navigate2(
        &self,
        url: *const VARIANT,
        _flags: *const VARIANT,
        _frame: *const VARIANT,
        _post: *const VARIANT,
        _headers: *const VARIANT,
    ) -> windows::core::Result<()> {
        // SAFETY: the variant is the one the caller passed; an item list
        // in it is turned into its parsing name first.
        self.go(parsing_of(&unsafe { given(url) }))
    }
    fn QueryStatusWB(&self, _command: OLECMDID) -> windows::core::Result<OLECMDF> {
        not_done()
    }
    fn ExecWB(
        &self,
        _command: OLECMDID,
        _option: OLECMDEXECOPT,
        _input: *const VARIANT,
        _output: *mut VARIANT,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn ShowBrowserBar(
        &self,
        _class: *const VARIANT,
        _show: *const VARIANT,
        _size: *const VARIANT,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn ReadyState(&self) -> windows::core::Result<READYSTATE> {
        Ok(READYSTATE_COMPLETE)
    }
    fn Offline(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetOffline(&self, _offline: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
    fn Silent(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetSilent(&self, _silent: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn RegisterAsBrowser(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetRegisterAsBrowser(&self, _register: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn RegisterAsDropTarget(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetRegisterAsDropTarget(&self, _register: VARIANT_BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn TheaterMode(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetTheaterMode(&self, _theater: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
    fn AddressBar(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(boolean(!self.desktop))
    }
    fn SetAddressBar(&self, _shown: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
    fn Resizable(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(boolean(!self.desktop))
    }
    fn SetResizable(&self, _resizable: VARIANT_BOOL) -> windows::core::Result<()> {
        not_done()
    }
}

impl IServiceProvider_Impl for FolderWindow_Impl {
    fn QueryService(
        &self,
        service: *const GUID,
        iid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        // SAFETY: the caller passes the service it asks for.
        let service =
            unsafe { service.as_ref() }.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        let browser = [SID_STopLevelBrowser, IShellBrowser::IID];
        if !browser.contains(service) {
            return Err(E_NOINTERFACE.into());
        }
        let unknown = self.to_interface::<IShellBrowser>();
        // SAFETY: the caller gives the interface it wants and where to put
        // it, as `QueryService` is called.
        unsafe { unknown.query(iid, object) }.ok()
    }
}

impl IOleWindow_Impl for FolderWindow_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        Ok(self.handle())
    }
    fn ContextSensitiveHelp(&self, _enter: BOOL) -> windows::core::Result<()> {
        not_done()
    }
}

/// The browser of a window, as views and programs ask it: the active view
/// and `BrowseObject` are answered, the rest of a browser's frame is not
/// there.
impl IShellBrowser_Impl for FolderWindow_Impl {
    fn InsertMenusSB(
        &self,
        _menu: HMENU,
        _widths: *mut OLEMENUGROUPWIDTHS,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn SetMenuSB(
        &self,
        _menu: HMENU,
        _resources: isize,
        _active: HWND,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn RemoveMenusSB(&self, _menu: HMENU) -> windows::core::Result<()> {
        not_done()
    }
    fn SetStatusTextSB(&self, _text: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn EnableModelessSB(&self, _enable: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn TranslateAcceleratorSB(&self, _message: *const MSG, _id: u16) -> windows::core::Result<()> {
        Err(S_FALSE.into())
    }
    fn BrowseObject(&self, list: *const ITEMIDLIST, _flags: u32) -> windows::core::Result<()> {
        if list.is_null() {
            return Err(E_INVALIDARG.into());
        }
        // SAFETY: the caller passes an absolute item list.
        let parsing = unsafe {
            automation::name_of_list(
                list,
                windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING,
            )
        };
        self.go(parsing)
    }
    fn GetViewStateStream(&self, _mode: u32) -> windows::core::Result<IStream> {
        not_done()
    }
    fn GetControlWindow(&self, _id: u32) -> windows::core::Result<HWND> {
        not_done()
    }
    fn SendControlMsg(
        &self,
        _id: u32,
        _message: u32,
        _wparam: WPARAM,
        _lparam: LPARAM,
        _answer: *mut LRESULT,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn QueryActiveShellView(&self) -> windows::core::Result<IShellView> {
        match self.desktop {
            true => super::desktop_view::DesktopView::shared(),
            false => Ok(self.to_interface::<IShellView>()),
        }
    }
    fn OnViewWindowActive(&self, _view: Ref<'_, IShellView>) -> windows::core::Result<()> {
        Ok(())
    }
    fn SetToolbarItems(
        &self,
        _buttons: *const TBBUTTON,
        _count: u32,
        _flags: u32,
    ) -> windows::core::Result<()> {
        not_done()
    }
}

/// The name of the kind `kind` (`SHGDN_*`) of the child `child` of
/// `folder`.
pub(super) fn name_of_child(
    folder: &IShellFolder,
    child: *const ITEMIDLIST,
    kind: windows::Win32::UI::Shell::SHGDNF,
) -> Option<String> {
    let mut name = STRRET::default();
    let mut text = windows::core::PWSTR::null();
    // SAFETY: the child is a list relative to the folder; the text the
    // Shell allocates is read and freed once.
    unsafe {
        folder.GetDisplayNameOf(child, kind, &raw mut name).ok()?;
        StrRetToStrW(&raw mut name, Some(child), &raw mut text).ok()?;
        let read = text.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(text.0.cast_const().cast()));
        read
    }
}

/// The view of a window of Tessera Files, as `SHOpenFolderAndSelectItems`
/// reaches it.
impl IShellView_Impl for FolderWindow_Impl {
    fn TranslateAccelerator(&self, _message: *const MSG) -> HRESULT {
        S_FALSE
    }
    fn EnableModeless(&self, _enable: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn UIActivate(&self, _state: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn Refresh(&self) -> windows::core::Result<()> {
        IWebBrowser_Impl::Refresh(self)
    }
    fn CreateViewWindow(
        &self,
        _previous: Ref<'_, IShellView>,
        _settings: *const FOLDERSETTINGS,
        _browser: Ref<'_, IShellBrowser>,
        _view: *const windows::Win32::Foundation::RECT,
    ) -> windows::core::Result<HWND> {
        not_done()
    }
    fn DestroyViewWindow(&self) -> windows::core::Result<()> {
        not_done()
    }
    fn GetCurrentInfo(&self) -> windows::core::Result<FOLDERSETTINGS> {
        not_done()
    }
    fn AddPropertySheetPages(
        &self,
        _reserved: u32,
        _add: LPFNSVADDPROPSHEETPAGE,
        _lparam: LPARAM,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn SaveViewState(&self) -> windows::core::Result<()> {
        Ok(())
    }

    /// Chooses the item `item` names, in the folder of the window: the
    /// names chosen so far start again with `SVSI_DESELECTOTHERS`, and
    /// each choice hands every name chosen to the file manager.
    fn SelectItem(&self, item: *const ITEMIDLIST, flags: u32) -> windows::core::Result<()> {
        let place = self.place();
        let name = match item.is_null() {
            true => None,
            false => Some(
                name_of_child(&place.shell_folder()?, item, NAME_IN_FOLDER)
                    .map(|name| folder_view::name_in_folder(&name))
                    .ok_or(windows::core::Error::from(E_INVALIDARG))?,
            ),
        };
        place.choose(name, flags as i32);
        Ok(())
    }

    fn GetItemObject(
        &self,
        kind: &_SVGIO,
        iid: *const GUID,
        object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        item_object(&self.place(), self.handle(), kind, iid, object)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        IFolderView, IShellDispatch2, IShellFolderViewDual, SVGIO_BACKGROUND, SWFO_NEEDDISPATCH,
    };

    #[test]
    fn paths_are_compared_without_case_and_end() {
        assert_eq!(compared(Path::new(r"C:\Users\Me\")), r"c:\users\me");
        assert_eq!(compared(Path::new(r"C:\")), r"c:\");
        assert_eq!(compared(Path::new("C:")), r"c:\");
        assert_eq!(compared(Path::new("D:/Data")), r"d:\data");
        assert_eq!(key_of("file:///C:/Users/Me/"), r"c:\users\me");
        assert_eq!(
            key_of("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"),
            "::{20d04fe0-3aea-1069-a2d8-08002b30309d}"
        );
    }

    #[test]
    fn choices_follow_the_flags_of_select_item() {
        let mut chosen = Vec::new();
        choose_in(&mut chosen, Some("a".to_owned()), SVSI_SELECT.0);
        choose_in(&mut chosen, Some("b".to_owned()), SVSI_SELECT.0);
        assert_eq!(chosen, ["a", "b"]);
        // Without SVSI_SELECT the item is let go.
        choose_in(&mut chosen, Some("A".to_owned()), 0);
        assert_eq!(chosen, ["b"]);
        choose_in(
            &mut chosen,
            Some("c".to_owned()),
            SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0,
        );
        assert_eq!(chosen, ["c"]);
        choose_in(&mut chosen, None, SVSI_DESELECTOTHERS.0);
        assert!(chosen.is_empty());
    }

    /// Walks the calls of the documented sample "ShellExecute from the
    /// desktop" (FindWindowSW, QueryService, QueryActiveShellView,
    /// GetItemObject, Application) in this process, on Windows.
    #[test]
    fn the_desktop_answers_the_chain_of_shell_execute_from_the_desktop() {
        // SAFETY: the test thread is made a single-threaded apartment, as
        // the shell thread is; the objects stay on it.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let list: IShellWindows = FolderList.into();
            let desktop = OwnedVariant::number(0);
            let empty = OwnedVariant::empty();
            let mut window = -1;
            let dispatch = list
                .FindWindowSW(
                    &raw const desktop.0,
                    &raw const empty.0,
                    SWC_DESKTOP,
                    &raw mut window,
                    SWFO_NEEDDISPATCH,
                )
                .expect("the desktop is found");
            assert_ne!(window, -1, "the desktop's window is written");
            let provider: IServiceProvider = dispatch.cast().expect("IServiceProvider");
            let browser: IShellBrowser = provider
                .QueryService(&SID_STopLevelBrowser)
                .expect("the top-level browser");
            let view = browser.QueryActiveShellView().expect("the active view");
            let background: IDispatch = view
                .GetItemObject(SVGIO_BACKGROUND)
                .expect("the background of the view");
            let folder_view: IShellFolderViewDual =
                background.cast().expect("IShellFolderViewDual");
            let shell: IShellDispatch2 = folder_view
                .Application()
                .expect("the application")
                .cast()
                .expect("IShellDispatch2");
            drop(shell);
            let _: IFolderView = view.cast().expect("the view is the desktop's IFolderView");
            assert!(folder_view.Folder().is_ok(), "the desktop's folder");
        }
    }
}
