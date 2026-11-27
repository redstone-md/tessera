// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The list of the folder windows of the Shell, `ShellWindows`, served by
//! Tessera where Explorer does not run.
//!
//! Programs that show a file in its folder ("Show in folder" of browsers,
//! editors and download managers) call `SHOpenFolderAndSelectItems`,
//! which asks the object of the class `CLSID_ShellWindows` for a window
//! that shows the folder (`IShellWindows::FindWindowSW`), and has that
//! window choose the files: through `IServiceProvider` to the
//! `IShellBrowser` of the window (`SID_STopLevelBrowser`), its active
//! `IShellView`, and `IShellView::SelectItem`. Explorer serves that class
//! from its process; without Explorer nobody does.
//!
//! As the shell, Tessera registers a class object for `CLSID_ShellWindows`
//! on the shell thread (`CoRegisterClassObject`, local server, for many
//! uses) whose object lists the windows of Tessera Files, each as a small
//! object with `IWebBrowserApp` (its window, its folder as
//! `LocationURL`), `IServiceProvider`, `IShellBrowser` and `IShellView`,
//! whose `SelectItem` hands the names chosen to the file manager. Windows
//! of other programs that register themselves are kept in the list too.
//! Beside Explorer nothing is registered: the list stays Explorer's.
//!
//! What is not served: late binding (`IDispatch::Invoke`, as scripts use
//! through `Shell.Application`), `_NewEnum`, `Document` and navigation
//! through the objects. Whether `SHOpenFolderAndSelectItems` finds a class
//! object registered this way is not verified on Windows: the class is
//! registered by Explorer under `HKCR\CLSID`, and whether the activation
//! of Windows reaches a running object of a process that is not the one
//! registered there is the open question.
#![allow(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_channel::{Receiver, Sender};
use gpui::Window;
use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, DISP_E_MEMBERNOTFOUND, DISP_E_UNKNOWNNAME, E_INVALIDARG, E_NOINTERFACE,
    E_NOTIMPL, HWND, LPARAM, LRESULT, S_FALSE, SHANDLE_PTR, VARIANT_BOOL, VARIANT_FALSE,
    VARIANT_TRUE, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoRegisterClassObject, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO,
    IClassFactory, IClassFactory_Impl, IDispatch, IDispatch_Impl, IServiceProvider,
    IServiceProvider_Impl, IStream, ITypeInfo, REGCLS_MULTIPLEUSE, SAFEARRAY,
};
use windows::Win32::System::Ole::{
    IOleWindow_Impl, OLEMENUGROUPWIDTHS, SafeArrayAccessData, SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VARIANT, VT_ARRAY, VT_BSTR, VT_I2, VT_I4, VT_UI1};
use windows::Win32::UI::Controls::{LPFNSVADDPROPSHEETPAGE, TBBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    _SVGIO, FOLDERSETTINGS, GPFIDL_DEFAULT, ILCombine, ILCreateFromPathW, ILFree, IShellBrowser,
    IShellBrowser_Impl, IShellView, IShellView_Impl, IShellWindows, IShellWindows_Impl,
    IWebBrowser_Impl, IWebBrowserApp, IWebBrowserApp_Impl, SHGetPathFromIDListEx,
    SID_STopLevelBrowser, SVSI_DESELECTOTHERS, SVSI_SELECT, SWFO_COOKIEPASSED, SWFO_INCLUDEPENDING,
    ShellWindowFindWindowOptions, ShellWindowTypeConstants, ShellWindows as CLSID_SHELL_WINDOWS,
};
use windows::Win32::UI::WindowsAndMessaging::{HMENU, MSG, PostMessageW, WM_CLOSE};
use windows::core::{
    BOOL, BSTR, GUID, HRESULT, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, implement,
};

use super::{NativeSurface, ShellThread};

/// The names a program asked a window of Tessera Files to choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The window, by the number of its handle.
    pub window: isize,
    /// The names in its folder, the first one first.
    pub names: Vec<OsString>,
}

/// Whether the list is served, which only the shell does.
static SERVING: AtomicBool = AtomicBool::new(false);

/// Where the selections asked for go.
static SELECTIONS: OnceLock<Sender<Selection>> = OnceLock::new();

/// A window in the list.
struct Entry {
    cookie: i32,
    /// The window, by the number of its handle; zero while it is pending.
    window: isize,
    folder: Option<PathBuf>,
    /// What the list gives for it: an object of Tessera's for its own
    /// windows, the one a program registered for others.
    dispatch: Option<IDispatch>,
    /// Whether it is one of Tessera Files.
    own: bool,
    /// Whether it was registered as pending, before its window exists.
    pending: bool,
}

thread_local! {
    /// The windows in the list, on the shell thread.
    static ENTRIES: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    /// The cookie the next window gets.
    static NEXT_COOKIE: Cell<i32> = const { Cell::new(1) };
    /// The one list, which every client gets.
    static LIST: RefCell<Option<IShellWindows>> = const { RefCell::new(None) };
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

    /// The number of the handle of `window`, by which the list knows it.
    pub fn number_of(window: &Window) -> Option<isize> {
        NativeSurface::of(window)
            .ok()
            .map(|surface| surface.hwnd.0 as isize)
    }

    /// Puts the window `window` of Tessera Files, at `folder`, in the list.
    pub fn opened(window: isize, folder: Option<PathBuf>) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            let cookie = next_cookie();
            let object: IWebBrowserApp = FolderWindow {
                cookie,
                window,
                chosen: RefCell::new(Vec::new()),
            }
            .into();
            let dispatch = object.cast::<IDispatch>().ok();
            ENTRIES.with(|entries| {
                entries.borrow_mut().push(Entry {
                    cookie,
                    window,
                    folder,
                    dispatch,
                    own: true,
                    pending: false,
                });
            });
        });
    }

    /// Notes that the window `window` of Tessera Files shows `folder` now.
    #[allow(
        dead_code,
        reason = "for the file manager, once it tells of navigation"
    )]
    pub fn navigated(window: isize, folder: Option<PathBuf>) {
        if !SERVING.load(Ordering::SeqCst) {
            return;
        }
        ShellThread::run(move || {
            ENTRIES.with(|entries| {
                let mut entries = entries.borrow_mut();
                if let Some(entry) = { entries.iter_mut() }.find(|entry| entry.window == window) {
                    entry.folder = folder;
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

/// `path` as an address, as `LocationURL` gives it.
fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    match text.strip_prefix("//") {
        Some(share) => format!("file://{share}"),
        None => format!("file:///{text}"),
    }
}

/// The path an address of `LocationURL` or a path names.
fn path_of_text(text: &str) -> PathBuf {
    let lower = text.to_ascii_lowercase();
    match lower.strip_prefix("file:") {
        Some(rest) if rest.starts_with("///") => PathBuf::from(text[8..].replace('/', "\\")),
        Some(rest) if rest.starts_with("//") => {
            PathBuf::from(format!("\\\\{}", text[7..].replace('/', "\\")))
        }
        _ => PathBuf::from(text),
    }
}

/// The path of the folder a location of the list names: a path or an
/// address as text, or an item list in an array of bytes.
///
/// # Safety
///
/// `location` is null or points to a valid `VARIANT`.
unsafe fn path_of(location: *const VARIANT) -> Option<PathBuf> {
    // SAFETY: as the caller promises; each field is read for the kind the
    // variant says it holds, and the array is accessed and let go once.
    unsafe {
        let inner = &location.as_ref()?.Anonymous.Anonymous;
        let kind = inner.vt;
        if kind == VT_BSTR {
            return Some(path_of_text(&inner.Anonymous.bstrVal.to_string()));
        }
        if kind.0 == VT_ARRAY.0 | VT_UI1.0 {
            let array: *const SAFEARRAY = inner.Anonymous.parray;
            let mut data = std::ptr::null_mut();
            SafeArrayAccessData(array, &raw mut data).ok()?;
            let path = path_of_list(data.cast());
            let _ = SafeArrayUnaccessData(array);
            return path;
        }
        None
    }
}

/// The path of the folder of the file system an item list names.
///
/// # Safety
///
/// `list` is null or a valid item list.
unsafe fn path_of_list(list: *const ITEMIDLIST) -> Option<PathBuf> {
    if list.is_null() {
        return None;
    }
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: as the caller promises; the buffer carries its length.
    let found = unsafe { SHGetPathFromIDListEx(list, &mut buffer, GPFIDL_DEFAULT) };
    found
        .as_bool()
        .then(|| PathBuf::from(super::files::text_in(&buffer)))
}

/// The number a location of the list holds, for a cookie.
///
/// # Safety
///
/// `location` is null or points to a valid `VARIANT`.
unsafe fn number_of(location: *const VARIANT) -> Option<i32> {
    // SAFETY: as the caller promises; the number is read for its kind.
    unsafe {
        let inner = &location.as_ref()?.Anonymous.Anonymous;
        match inner.vt {
            VT_I4 => Some(inner.Anonymous.lVal),
            VT_I2 => Some(i32::from(inner.Anonymous.iVal)),
            _ => None,
        }
    }
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

/// Late binding is not served: every name is unknown.
macro_rules! no_late_binding {
    ($impl:ty) => {
        impl IDispatch_Impl for $impl {
            fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
                Ok(0)
            }

            fn GetTypeInfo(&self, _info: u32, _locale: u32) -> windows::core::Result<ITypeInfo> {
                Err(E_NOTIMPL.into())
            }

            fn GetIDsOfNames(
                &self,
                _iid: *const GUID,
                _names: *const PCWSTR,
                _count: u32,
                _locale: u32,
                _ids: *mut i32,
            ) -> windows::core::Result<()> {
                Err(DISP_E_UNKNOWNNAME.into())
            }

            fn Invoke(
                &self,
                _member: i32,
                _iid: *const GUID,
                _locale: u32,
                _flags: DISPATCH_FLAGS,
                _parameters: *const DISPPARAMS,
                _result: *mut VARIANT,
                _exception: *mut EXCEPINFO,
                _argument: *mut u32,
            ) -> windows::core::Result<()> {
                Err(DISP_E_MEMBERNOTFOUND.into())
            }
        }
    };
}

/// The list: what `CLSID_ShellWindows` gives.
#[implement(IShellWindows, Agile = false)]
struct FolderList;

no_late_binding!(FolderList_Impl);

impl IShellWindows_Impl for FolderList_Impl {
    fn Count(&self) -> windows::core::Result<i32> {
        Ok(ENTRIES.with(|entries| {
            entries
                .borrow()
                .iter()
                .filter(|entry| !entry.pending)
                .count()
        }) as i32)
    }

    fn Item(&self, index: &VARIANT) -> windows::core::Result<IDispatch> {
        // SAFETY: the variant is the one the caller passed.
        let index = unsafe { number_of(index) }.unwrap_or(0);
        let index = usize::try_from(index).map_err(|_| windows::core::Error::from(E_INVALIDARG))?;
        let found = ENTRIES.with(|entries| {
            let entries = entries.borrow();
            { entries.iter() }
                .filter(|entry| !entry.pending)
                .nth(index)
                .and_then(|entry| entry.dispatch.clone())
        });
        found.ok_or_else(|| S_FALSE.into())
    }

    fn _NewEnum(&self) -> windows::core::Result<IUnknown> {
        Err(E_NOTIMPL.into())
    }

    fn Register(
        &self,
        dispatch: Ref<'_, IDispatch>,
        window: i32,
        _class: ShellWindowTypeConstants,
    ) -> windows::core::Result<i32> {
        let cookie = next_cookie();
        let dispatch = dispatch.cloned();
        let folder = dispatch.as_ref().and_then(location_of);
        ENTRIES.with(|entries| {
            entries.borrow_mut().push(Entry {
                cookie,
                window: window as isize,
                folder,
                dispatch,
                own: false,
                pending: false,
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
        let folder = unsafe { path_of(location) };
        ENTRIES.with(|entries| {
            entries.borrow_mut().push(Entry {
                cookie,
                window: 0,
                folder,
                dispatch: None,
                own: false,
                pending: true,
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
        let folder = unsafe { path_of(location) };
        ENTRIES.with(|entries| {
            let mut entries = entries.borrow_mut();
            if let Some(entry) = { entries.iter_mut() }.find(|entry| entry.cookie == cookie) {
                entry.folder = folder;
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
        _class: ShellWindowTypeConstants,
        window: *mut i32,
        options: ShellWindowFindWindowOptions,
    ) -> windows::core::Result<IDispatch> {
        let by_cookie = options.0 & SWFO_COOKIEPASSED.0 != 0;
        let pending = options.0 & SWFO_INCLUDEPENDING.0 != 0;
        // SAFETY: the variant is the one the caller passed.
        let (cookie, folder) = unsafe {
            match by_cookie {
                true => (number_of(location), None),
                false => (None, path_of(location).map(|path| compared(&path))),
            }
        };
        let found = ENTRIES.with(|entries| {
            let entries = entries.borrow();
            let entry = { entries.iter() }
                .filter(|entry| pending || !entry.pending)
                .find(|entry| match (cookie, &folder) {
                    (Some(cookie), _) => entry.cookie == cookie,
                    (None, Some(folder)) => {
                        entry.folder.as_deref().map(compared).as_ref() == Some(folder)
                    }
                    (None, None) => false,
                })?;
            Some((entry.window, entry.dispatch.clone()))
        });
        let Some((handle, dispatch)) = found else {
            return Err(S_FALSE.into());
        };
        if !window.is_null() {
            // SAFETY: the caller gives where the window is to be written.
            unsafe { window.write(handle as i32) };
        }
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

/// The folder a window of another program shows, as its `LocationURL`
/// says.
fn location_of(dispatch: &IDispatch) -> Option<PathBuf> {
    let browser: IWebBrowserApp = dispatch.cast().ok()?;
    // SAFETY: the call only reads the address of the window.
    let url = unsafe { browser.LocationURL() }.ok()?;
    Some(path_of_text(&url.to_string()))
}

/// A window of Tessera Files, as the list gives it.
#[implement(
    IWebBrowserApp,
    IServiceProvider,
    IShellBrowser,
    IShellView,
    Agile = false
)]
struct FolderWindow {
    cookie: i32,
    /// The window, by the number of its handle.
    window: isize,
    /// The names chosen so far by `SelectItem`.
    chosen: RefCell<Vec<OsString>>,
}

impl FolderWindow {
    fn handle(&self) -> HWND {
        HWND(self.window as *mut core::ffi::c_void)
    }

    /// The folder the window shows now.
    fn folder(&self) -> Option<PathBuf> {
        ENTRIES.with(|entries| {
            let entries = entries.borrow();
            { entries.iter() }
                .find(|entry| entry.cookie == self.cookie)
                .and_then(|entry| entry.folder.clone())
        })
    }
}

no_late_binding!(FolderWindow_Impl);

/// What a window does not do: navigate, move, show bars of a browser.
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
        not_done()
    }
    fn GoSearch(&self) -> windows::core::Result<()> {
        not_done()
    }
    fn Navigate(
        &self,
        _url: &BSTR,
        _flags: *const VARIANT,
        _frame: *const VARIANT,
        _post: *const VARIANT,
        _headers: *const VARIANT,
    ) -> windows::core::Result<()> {
        not_done()
    }
    fn Refresh(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn Refresh2(&self, _level: *const VARIANT) -> windows::core::Result<()> {
        Ok(())
    }
    fn Stop(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn Application(&self) -> windows::core::Result<IDispatch> {
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn Container(&self) -> windows::core::Result<IDispatch> {
        not_done()
    }
    fn Document(&self) -> windows::core::Result<IDispatch> {
        not_done()
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
        let folder = self.folder().unwrap_or_default();
        let name = folder.file_name().map_or_else(
            || folder.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Ok(BSTR::from(name))
    }
    fn LocationURL(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from(
            self.folder()
                .map(|folder| file_url(&folder))
                .unwrap_or_default(),
        ))
    }
    fn Busy(&self) -> windows::core::Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for FolderWindow_Impl {
    fn Quit(&self) -> windows::core::Result<()> {
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
        Ok(VARIANT_TRUE)
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
    fn BrowseObject(&self, _list: *const ITEMIDLIST, _flags: u32) -> windows::core::Result<()> {
        not_done()
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
        Ok(self.to_interface::<IShellView>())
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
        Ok(())
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
        let folder = self
            .folder()
            .ok_or(windows::core::Error::from(E_INVALIDARG))?;
        let flags = flags as i32;
        let mut chosen = self.chosen.borrow_mut();
        if flags & SVSI_DESELECTOTHERS.0 != 0 {
            chosen.clear();
        }
        if flags & SVSI_SELECT.0 != 0 && !item.is_null() {
            let wide: Vec<u16> = folder
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain([0])
                .collect();
            // SAFETY: the path is NUL-terminated; the lists the Shell
            // makes are freed once, after the path is read from them; the
            // item is the relative list the caller passed.
            let path = unsafe {
                let parent = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
                if parent.is_null() {
                    return Err(E_INVALIDARG.into());
                }
                let whole = ILCombine(Some(parent), Some(item));
                let path = path_of_list(whole);
                ILFree(Some(whole));
                ILFree(Some(parent));
                path
            };
            if let Some(name) = path.as_deref().and_then(Path::file_name) {
                chosen.push(name.to_owned());
            }
        }
        if let Some(selections) = SELECTIONS.get() {
            let selection = Selection {
                window: self.window,
                names: chosen.clone(),
            };
            let _ = selections.try_send(selection);
        }
        Ok(())
    }

    fn GetItemObject(
        &self,
        _item: &_SVGIO,
        _iid: *const GUID,
        _object: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        not_done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_compared_without_case_and_end() {
        assert_eq!(compared(Path::new(r"C:\Users\Me\")), r"c:\users\me");
        assert_eq!(compared(Path::new(r"C:\")), r"c:\");
        assert_eq!(compared(Path::new("C:")), r"c:\");
        assert_eq!(compared(Path::new("D:/Data")), r"d:\data");
    }

    #[test]
    fn a_folder_is_an_address_and_back() {
        assert_eq!(file_url(Path::new(r"C:\a b")), "file:///C:/a b");
        assert_eq!(file_url(Path::new(r"\\nas\share")), "file://nas/share");
        assert_eq!(path_of_text("file:///C:/a b"), PathBuf::from(r"C:\a b"));
        assert_eq!(
            path_of_text("file://nas/share"),
            PathBuf::from(r"\\nas\share")
        );
        assert_eq!(path_of_text(r"D:\x"), PathBuf::from(r"D:\x"));
    }
}
