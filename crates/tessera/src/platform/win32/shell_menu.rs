// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The context menu of the Shell for files, folders or the background of
//! a folder: the very menu Explorer shows, with the verbs of the kinds of
//! files and those that programs add (shell extensions), owner-drawn items
//! included, and the extended verbs where Shift is held.
//!
//! A menu is built on the [`ShellThread`] and kept there while its
//! [`ShellMenu`] lives. It can be used in two ways:
//!
//! - Drawn by Tessera in its own style: [`ShellMenu::verbs`] lists the
//!   items as data, and [`ShellMenu::invoke`] does the one the user chose.
//! - Shown as Windows draws it ("Show more options"): [`ShellMenu::show`]
//!   tracks it with `TrackPopupMenuEx` from the hidden window of the shell
//!   thread, which passes the messages of owner-drawn and lazily filled
//!   submenus on to the menu (`IContextMenu3::HandleMenuMsg2`).
//!
//! Some verbs only work inside Explorer's own view of a folder, which
//! Tessera does not have: rename, New > Folder, View, Sort by, Group by,
//! Refresh, Undo. Delete and Paste would work, but outside the
//! [`UndoJournal`](super::UndoJournal). These verbs are not done: invoking
//! one gives [`MenuOutcome::Intercepted`], for the caller to do itself, and
//! [`ShellVerb::intercept`] tells which they are beforehand.
//!
//! ```ignore
//! let built = ShellMenu::for_items(chosen, shift_held);
//! cx.spawn(async move |this, cx| {
//!     let Ok(Ok(menu)) = built.recv().await else { return };
//!     match menu.show(None, None).recv().await {
//!         Ok(Ok(MenuOutcome::Intercepted(Intercept::Rename))) => { /* start renaming */ }
//!         Ok(Err(error)) => report!("{error}"),
//!         _ => {}
//!     }
//! }).detach();
//! ```
//!
//! A shell extension that hangs keeps the shell thread with it. Nothing can
//! end it from outside; a caller that must not wait for long races the
//! receiver against a timer of its own and goes on without the menu.
#![allow(unsafe_code)]
#![allow(
    dead_code,
    reason = "the menus of the Shell are for the file manager and the desktop, which do not use all of them yet"
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsStr, c_void};
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, anyhow};
use async_channel::Receiver;
use gpui::Window;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetDIBits, GetObjectW,
    HBITMAP, ReleaseDC,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::Shell::{
    CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_ITEMMENU, CMF_NODEFAULT, CMF_NORMAL,
    CMF_OPTIMIZEFORINVOKE, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN,
    CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, DEFCONTEXTMENU, GCS_VERBW, IContextMenu,
    IContextMenu2, IContextMenu3, IQueryAssociations, IShellFolder, SHCreateDefaultContextMenu,
    SHGetDesktopFolder,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, GetMenuItemCount, GetMenuItemInfoW, HMENU,
    MENUITEMINFOW, MFS_CHECKED, MFS_DEFAULT, MFS_DISABLED, MFT_OWNERDRAW, MFT_SEPARATOR,
    MIIM_BITMAP, MIIM_FTYPE, MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, PostMessageW,
    SW_SHOWNORMAL, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};
use windows::core::{IUnknown, Interface, PCSTR, PCWSTR, PSTR, PWSTR};

use super::shell_item::{IdList, ItemGroup, wide};
use super::{IconPixels, NativeSurface, ShellThread};

/// The range of the numbers the menu gives its items.
const FIRST: u32 = 1;
const LAST: u32 = 0x7FFF;

/// Flags of `CMINVOKECOMMANDINFOEX` that the bindings do not name: the
/// texts are UTF-16, the verb may finish after the call returns, or it
/// may not.
const CMIC_MASK_UNICODE: u32 = 0x4000;
const CMIC_MASK_ASYNCOK: u32 = 0x10_0000;
const CMIC_MASK_NOASYNC: u32 = 0x100;

/// How deep submenus are listed.
const MENU_DEPTH: usize = 4;

thread_local! {
    /// The menus built and kept on the shell thread, by their numbers.
    static MENUS: RefCell<HashMap<u64, Rc<Built>>> = RefCell::new(HashMap::new());
}

/// A verb that only works inside Explorer's own view of a folder, or that
/// Tessera does itself so that it can be undone; see the module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intercept {
    Rename,
    NewFolder,
    Delete,
    Paste,
    PasteLink,
    Refresh,
    View,
    Sort,
    Group,
    Undo,
    Redo,
}

impl Intercept {
    /// The verb `verb` is, if Tessera does it itself.
    pub fn of(verb: &str) -> Option<Self> {
        let verb = verb.to_ascii_lowercase();
        Some(match verb.as_str() {
            "rename" => Self::Rename,
            "newfolder" => Self::NewFolder,
            "delete" => Self::Delete,
            "paste" => Self::Paste,
            "pastelink" => Self::PasteLink,
            "refresh" => Self::Refresh,
            "view" => Self::View,
            "arrange" | "sortby" => Self::Sort,
            "groupby" => Self::Group,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            _ => return None,
        })
    }
}

/// An item of a context menu of the Shell.
#[derive(Clone, Debug, Default)]
pub struct ShellVerb {
    /// What [`ShellMenu::invoke`] takes to do it; zero for a separator or
    /// a submenu.
    pub id: u32,
    /// What it says, without the `&` of its access key or a shortcut
    /// after a tab.
    pub label: String,
    /// The key that chooses it while the menu is open, as underlined.
    pub access_key: Option<char>,
    /// Its name that does not change with the language, such as `open`,
    /// `properties`, `runas` or `cut`, where it has one.
    pub verb: Option<String>,
    /// Whether invoking it gives it back for the caller to do.
    pub intercept: Option<Intercept>,
    /// Whether it is what a double click does.
    pub default: bool,
    pub disabled: bool,
    pub checked: bool,
    pub separator: bool,
    /// Whether it is drawn by the program that added it; such an item may
    /// have no label, and only looks right in [`ShellMenu::show`].
    pub owner_drawn: bool,
    /// Its picture, where it has one.
    pub icon: Option<IconPixels>,
    /// Whether it opens a submenu, and the items of that.
    pub submenu: bool,
    pub children: Vec<ShellVerb>,
}

/// What came of a menu or of a verb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuOutcome {
    /// The menu was closed without a choice.
    Dismissed,
    /// The verb was done (or started: some finish on their own later).
    Invoked { verb: Option<String> },
    /// The verb is one for the caller to do.
    Intercepted(Intercept),
}

/// What a command to a menu names: an item by its number, or a verb.
#[derive(Clone, Debug)]
enum Command {
    Id(u32),
    Verb(String),
}

/// What a menu is of.
enum Subject {
    Items(Vec<PathBuf>),
    Background(PathBuf),
}

/// A context menu of the Shell, kept on the shell thread until this is
/// dropped. It can be sent to any thread.
#[derive(Debug)]
pub struct ShellMenu {
    key: u64,
    verbs: Vec<ShellVerb>,
}

impl ShellMenu {
    /// Builds the menu of the files and folders `paths`, which may lie in
    /// different folders; `extended` adds the verbs shown with Shift held.
    /// The receiver yields it once it is built, or why it is not.
    pub fn for_items(paths: Vec<PathBuf>, extended: bool) -> Receiver<Result<Self, String>> {
        Self::build_later(Subject::Items(paths), extended)
    }

    /// Builds the menu of the background of `folder`: New, Paste, Open in
    /// Terminal and what programs add there.
    pub fn for_background(folder: PathBuf, extended: bool) -> Receiver<Result<Self, String>> {
        Self::build_later(Subject::Background(folder), extended)
    }

    /// Does the verb `verb` of the files and folders `paths`, such as
    /// `properties`, `openas`, `print` or `runas`, as their menu would.
    /// The receiver yields how it went.
    pub fn do_verb(paths: Vec<PathBuf>, verb: &str) -> Receiver<Result<MenuOutcome, String>> {
        let verb = verb.to_owned();
        call(move || {
            let built = Built::of(Subject::Items(paths), CMF_OPTIMIZEFORINVOKE)?;
            built.invoke(Command::Verb(verb), None)
        })
    }

    /// The items of the menu, as Windows has them.
    pub fn verbs(&self) -> &[ShellVerb] {
        &self.verbs
    }

    /// Does the item numbered `id`, as a click on it would, at the screen
    /// point `at` where it was chosen.
    pub fn invoke(&self, id: u32, at: Option<(i32, i32)>) -> Receiver<Result<MenuOutcome, String>> {
        self.invoke_later(Command::Id(id), at)
    }

    /// Does the verb `verb` of the menu, such as `open` or `properties`.
    pub fn invoke_verb(&self, verb: &str) -> Receiver<Result<MenuOutcome, String>> {
        self.invoke_later(Command::Verb(verb.to_owned()), None)
    }

    /// Shows the menu as Windows draws it, at the screen point `at` or at
    /// the pointer, and does what is chosen. The keyboard goes back to
    /// `window` when the menu closes.
    pub fn show(
        &self,
        at: Option<(i32, i32)>,
        window: Option<&Window>,
    ) -> Receiver<Result<MenuOutcome, String>> {
        let key = self.key;
        let give_back = { window.and_then(|window| NativeSurface::of(window).ok()) }
            .map(|surface| surface.hwnd.0 as isize);
        call(move || Built::kept(key)?.show(at, give_back))
    }

    fn build_later(subject: Subject, extended: bool) -> Receiver<Result<Self, String>> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let flags = match &subject {
            Subject::Items(_) => CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | CMF_ITEMMENU,
            Subject::Background(_) => CMF_NORMAL | CMF_EXPLORE | CMF_NODEFAULT,
        } | if extended { CMF_EXTENDEDVERBS } else { 0 };
        call(move || {
            let built = Built::of(subject, flags)?;
            let verbs = built.walk(built.popup, 0);
            let key = NEXT.fetch_add(1, Ordering::Relaxed);
            MENUS.with(|menus| menus.borrow_mut().insert(key, Rc::new(built)));
            Ok(Self { key, verbs })
        })
    }

    fn invoke_later(
        &self,
        command: Command,
        at: Option<(i32, i32)>,
    ) -> Receiver<Result<MenuOutcome, String>> {
        let key = self.key;
        call(move || Built::kept(key)?.invoke(command, at))
    }
}

impl Drop for ShellMenu {
    fn drop(&mut self) {
        let key = self.key;
        ShellThread::run(move || {
            MENUS.with(|menus| menus.borrow_mut().remove(&key));
        });
    }
}

/// Has `work` done on the shell thread, and gives its answer with the
/// error as a text, which crosses threads.
fn call<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Receiver<Result<T, String>> {
    ShellThread::call(move || work().map_err(|error| format!("{error:#}")))
}

/// A menu built on the shell thread.
struct Built {
    menu: IContextMenu,
    menu2: Option<IContextMenu2>,
    menu3: Option<IContextMenu3>,
    popup: HMENU,
    /// The folder of a menu of a background, which verbs run in.
    folder: Option<PathBuf>,
}

impl Built {
    /// Builds the menu of `subject` on this thread, which has COM
    /// initialized apartment-threaded, asking it with `flags`.
    fn of(subject: Subject, flags: u32) -> Result<Self> {
        let owner = ShellThread::owner().unwrap_or_default();
        let (menu, folder) = match subject {
            Subject::Items(paths) => (menu_of_items(&ItemGroup::of_paths(&paths)?, owner)?, None),
            Subject::Background(folder) => (menu_of_background(&folder, owner)?, Some(folder)),
        };
        Self::filled(menu, folder, flags)
    }

    /// Builds the menu of the items `lists`.
    fn of_lists(lists: Vec<IdList>, owner: HWND, flags: u32) -> Result<Self> {
        let menu = menu_of_items(&ItemGroup::of(lists)?, owner)?;
        Self::filled(menu, None, flags)
    }

    /// Has `menu` fill a new popup menu.
    fn filled(menu: IContextMenu, folder: Option<PathBuf>, flags: u32) -> Result<Self> {
        // SAFETY: the popup menu is owned by `Built` from here on and
        // destroyed with it; the context menu fills it on this thread.
        unsafe {
            let popup = CreatePopupMenu().context("no menu can be made")?;
            let built = Self {
                menu2: menu.cast().ok(),
                menu3: menu.cast().ok(),
                menu,
                popup,
                folder,
            };
            built
                .menu
                .QueryContextMenu(built.popup, 0, FIRST, LAST, flags)
                .ok()
                .context("the menu of the Shell could not be filled")?;
            Ok(built)
        }
    }

    /// The menu numbered `key`, kept on this thread.
    fn kept(key: u64) -> Result<Rc<Self>> {
        MENUS
            .with(|menus| menus.borrow().get(&key).cloned())
            .context("the menu is gone")
    }

    /// Passes a message of the menu window to the menu, for owner-drawn
    /// items and submenus that are filled when they open; its answer,
    /// where it handled the message.
    fn forward(&self, message: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
        // SAFETY: the menus are alive, and the message is passed on as it
        // came, on the thread the menu lives on.
        unsafe {
            if let Some(menu) = &self.menu3 {
                let mut answer = LRESULT(0);
                return { menu.HandleMenuMsg2(message, w, l, Some(&raw mut answer)) }
                    .ok()
                    .map(|()| answer);
            }
            if message == WM_MENUCHAR {
                return None;
            }
            let menu = self.menu2.as_ref()?;
            // What a window answers when it handled the message itself.
            let handled = LRESULT(isize::from(message != WM_INITMENUPOPUP));
            menu.HandleMenuMsg(message, w, l).ok().map(|()| handled)
        }
    }

    /// The canonical verb of the item numbered `id`.
    fn verb_of(&self, id: u32) -> Option<String> {
        if !(FIRST..=LAST).contains(&id) {
            return None;
        }
        let mut name = [0u16; 128];
        // SAFETY: the buffer is given with its length in characters, as
        // the Unicode form of the call takes it.
        unsafe {
            self.menu
                .GetCommandString(
                    (id - FIRST) as usize,
                    GCS_VERBW,
                    None,
                    PSTR(name.as_mut_ptr().cast()),
                    name.len() as u32,
                )
                .ok()?;
        }
        let length = name.iter().position(|&unit| unit == 0).unwrap_or(0);
        (length > 0).then(|| String::from_utf16_lossy(&name[..length]))
    }

    /// The items of `popup`, a submenu `depth` deep.
    fn walk(&self, popup: HMENU, depth: usize) -> Vec<ShellVerb> {
        // SAFETY: the menu is alive on this thread.
        let count = unsafe { GetMenuItemCount(Some(popup)) }.max(0) as u32;
        (0..count)
            .filter_map(|position| self.item(popup, position, depth))
            .collect()
    }

    /// The item at `position` in `popup`.
    fn item(&self, popup: HMENU, position: u32, depth: usize) -> Option<ShellVerb> {
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_ID | MIIM_STATE | MIIM_SUBMENU | MIIM_STRING | MIIM_BITMAP,
            ..Default::default()
        };
        // SAFETY: the first call only measures the text; the second has a
        // buffer of that length and one more for the NUL, which it is told.
        let text = unsafe {
            GetMenuItemInfoW(popup, position, true, &raw mut info).ok()?;
            let mut text = vec![0u16; info.cch as usize + 1];
            info.dwTypeData = PWSTR(text.as_mut_ptr());
            info.cch = text.len() as u32;
            GetMenuItemInfoW(popup, position, true, &raw mut info).ok()?;
            text.truncate(info.cch as usize);
            String::from_utf16_lossy(&text)
        };
        let separator = info.fType.0 & MFT_SEPARATOR.0 != 0;
        let submenu = !info.hSubMenu.is_invalid() && !info.hSubMenu.0.is_null();
        let mut children = Vec::new();
        if submenu && depth < MENU_DEPTH {
            // A submenu such as Send to or Open with fills itself when it
            // opens.
            let position = LPARAM(position as isize);
            self.forward(WM_INITMENUPOPUP, WPARAM(info.hSubMenu.0 as usize), position);
            children = self.walk(info.hSubMenu, depth + 1);
        }
        let id = match separator || submenu {
            true => 0,
            false => info.wID,
        };
        let verb = self.verb_of(id);
        let (label, access_key) = label_of(&text);
        Some(ShellVerb {
            id,
            label,
            access_key,
            intercept: verb.as_deref().and_then(Intercept::of),
            verb,
            default: info.fState.0 & MFS_DEFAULT.0 != 0,
            disabled: info.fState.0 & MFS_DISABLED.0 != 0,
            checked: info.fState.0 & MFS_CHECKED.0 != 0,
            separator,
            owner_drawn: info.fType.0 & MFT_OWNERDRAW.0 != 0,
            icon: bitmap_pixels(info.hbmpItem),
            submenu,
            children,
        })
    }

    /// Shows the menu at `at` or at the pointer, and does what is chosen;
    /// the keyboard goes back to the window numbered `give_back`.
    fn show(
        self: Rc<Self>,
        at: Option<(i32, i32)>,
        give_back: Option<isize>,
    ) -> Result<MenuOutcome> {
        let owner = ShellThread::owner().context("the shell thread has no window for menus")?;
        let point = match at {
            Some((x, y)) => POINT { x, y },
            None => pointer(),
        };
        let forwarding = self.clone();
        let interception = ShellThread::intercept(move |message, w, l| match message {
            WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR => {
                forwarding.forward(message, w, l)
            }
            _ => None,
        });
        // SAFETY: the menu and the owner window live on this thread. The
        // owner is brought in front so that the menu closes when the user
        // clicks elsewhere, and is sent a message after it, as Windows
        // asks of a menu of a window that is not in front.
        let chosen = unsafe {
            let _ = SetForegroundWindow(owner);
            let chosen = TrackPopupMenuEx(
                self.popup,
                TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0,
                point.x,
                point.y,
                owner,
                None,
            );
            let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
            chosen.0 as u32
        };
        drop(interception);
        if let Some(window) = give_back {
            // SAFETY: bringing a window in front has no other effect; one
            // that is gone is refused.
            let _ = unsafe { SetForegroundWindow(HWND(window as *mut c_void)) };
        }
        if chosen == 0 {
            return Ok(MenuOutcome::Dismissed);
        }
        self.invoke(Command::Id(chosen), Some((point.x, point.y)))
    }

    /// Does `command`, chosen at the screen point `at`, unless it is a
    /// verb for the caller; it may finish after this returns.
    fn invoke(&self, command: Command, at: Option<(i32, i32)>) -> Result<MenuOutcome> {
        let verb = match &command {
            Command::Id(id) => self.verb_of(*id),
            Command::Verb(verb) => Some(verb.clone()),
        };
        if let Some(intercept) = verb.as_deref().and_then(Intercept::of) {
            return Ok(MenuOutcome::Intercepted(intercept));
        }
        let owner = ShellThread::owner().unwrap_or_default();
        self.invoke_as(&command, at, owner, CMIC_MASK_ASYNCOK)?;
        Ok(MenuOutcome::Invoked { verb })
    }

    /// Has the menu do `command`, with the flags `mask` besides those
    /// every invocation has.
    fn invoke_as(
        &self,
        command: &Command,
        at: Option<(i32, i32)>,
        owner: HWND,
        mut mask: u32,
    ) -> Result<()> {
        let (verb, verb_wide) = match command {
            Command::Id(id) if (FIRST..=LAST).contains(id) => (None, None),
            Command::Id(id) => return Err(anyhow!("the menu has no item {id}")),
            Command::Verb(verb) => {
                let ascii: Vec<u8> = { verb.bytes() }.filter(u8::is_ascii).chain([0]).collect();
                (Some(ascii), Some(wide(OsStr::new(verb))))
            }
        };
        // An item is named by its offset, in place of a pointer.
        let offset = match command {
            Command::Id(id) => (id - FIRST) as usize,
            Command::Verb(_) => 0,
        };
        let directory = self.folder.as_ref().map(|folder| wide(folder.as_os_str()));
        mask |= CMIC_MASK_UNICODE;
        if at.is_some() {
            mask |= CMIC_MASK_PTINVOKE;
        }
        // SAFETY: the calls only read the state of the keys.
        unsafe {
            if GetAsyncKeyState(i32::from(VK_SHIFT.0)) < 0 {
                mask |= CMIC_MASK_SHIFT_DOWN;
            }
            if GetAsyncKeyState(i32::from(VK_CONTROL.0)) < 0 {
                mask |= CMIC_MASK_CONTROL_DOWN;
            }
        }
        let (x, y) = at.unwrap_or_default();
        let info = CMINVOKECOMMANDINFOEX {
            cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
            fMask: mask,
            hwnd: owner,
            lpVerb: verb
                .as_ref()
                .map_or(PCSTR(offset as *const u8), |verb| PCSTR(verb.as_ptr())),
            lpVerbW: verb_wide
                .as_ref()
                .map_or(PCWSTR(offset as *const u16), |verb| PCWSTR(verb.as_ptr())),
            lpDirectoryW: directory
                .as_ref()
                .map_or(PCWSTR::null(), |folder| PCWSTR(folder.as_ptr())),
            nShow: SW_SHOWNORMAL.0,
            ptInvoke: POINT { x, y },
            ..Default::default()
        };
        // SAFETY: the structure has its size and the extended form that
        // CMIC_MASK_UNICODE announces; every text it points to is
        // NUL-terminated and outlives the call.
        unsafe {
            self.menu
                .InvokeCommand((&raw const info).cast::<CMINVOKECOMMANDINFO>())
        }
        .context("the verb could not be done")
    }
}

impl Drop for Built {
    fn drop(&mut self) {
        // SAFETY: the popup menu is owned here and not used after this.
        unsafe {
            let _ = DestroyMenu(self.popup);
        }
    }
}

/// Does the verb `verb` of the items `lists` on this thread, which has
/// COM initialized apartment-threaded, and waits for it to finish. This is
/// for the platform layer itself, such as "undelete" to restore an item
/// of the Recycle Bin.
pub(super) fn invoke_verb_here(lists: Vec<IdList>, verb: &str, owner: HWND) -> Result<()> {
    let built = Built::of_lists(lists, owner, CMF_OPTIMIZEFORINVOKE)?;
    built.invoke_as(
        &Command::Verb(verb.to_owned()),
        None,
        owner,
        CMIC_MASK_NOASYNC,
    )
}

/// The context menu of the items of `group`: the one their folder gives,
/// as Explorer asks it, or for items of different folders the default
/// menu of the Shell, with the verbs of the kind of the first.
fn menu_of_items(group: &ItemGroup, owner: HWND) -> Result<IContextMenu> {
    let children = group.children();
    if group.parent.is_some() {
        // SAFETY: the items point into the lists of the group, which
        // outlive the call.
        return unsafe { group.folder.GetUIObjectOf(owner, &children, None) }
            .context("the Shell gives no menu of these items");
    }
    // SAFETY: as above; the folder and the associations are handed over
    // in fields that do not release them, which is done after the call.
    unsafe {
        let associations: Option<IUnknown> = group
            .folder
            .GetUIObjectOf::<IQueryAssociations>(owner, &children[..1], None)
            .ok()
            .and_then(|associations| associations.cast().ok());
        let mut menu = DEFCONTEXTMENU {
            hwnd: owner,
            psf: ManuallyDrop::new(Some(group.folder.clone())),
            cidl: children.len() as u32,
            apidl: children.as_ptr().cast_mut().cast(),
            punkAssociationInfo: ManuallyDrop::new(associations),
            ..Default::default()
        };
        let made = SHCreateDefaultContextMenu(&raw const menu);
        ManuallyDrop::drop(&mut menu.psf);
        ManuallyDrop::drop(&mut menu.punkAssociationInfo);
        made.context("the Shell gives no menu of these items")
    }
}

/// The context menu of the background of `folder`.
fn menu_of_background(folder: &std::path::Path, owner: HWND) -> Result<IContextMenu> {
    let list = IdList::of_path(folder)?;
    // SAFETY: the lists are alive for the calls, which return objects
    // owned here.
    unsafe {
        let shell_folder: IShellFolder = match list.parent() {
            Some(_) => {
                let (parent, last) = list.bind_to_parent()?;
                parent.BindToObject(last, None)?
            }
            None => SHGetDesktopFolder()?,
        };
        shell_folder
            .CreateViewObject(owner)
            .with_context(|| format!("the Shell gives no menu of {}", folder.display()))
    }
}

/// The pointer's position on the screen.
fn pointer() -> POINT {
    let mut point = POINT::default();
    // SAFETY: `point` outlives the call that fills it.
    let _ = unsafe { GetCursorPos(&raw mut point) };
    point
}

/// The label of a menu item as it is shown, and its access key: `&`
/// marks the key, `&&` is a `&`, and a shortcut after a tab is left out.
fn label_of(text: &str) -> (String, Option<char>) {
    let text = text.split('\t').next().unwrap_or_default();
    let mut label = String::with_capacity(text.len());
    let mut key = None;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '&' {
            label.push(character);
            continue;
        }
        match characters.next() {
            Some('&') => label.push('&'),
            Some(next) => {
                key.get_or_insert(next.to_ascii_lowercase());
                label.push(next);
            }
            None => {}
        }
    }
    (label, key)
}

/// The pixels of the picture of a menu item, where it is a bitmap and not
/// one of the system's own pictures, as straight-alpha BGRA.
fn bitmap_pixels(bitmap: HBITMAP) -> Option<IconPixels> {
    // The pictures of the system (HBMMENU_*) are small numbers, -1 to 11.
    if (-1..=11).contains(&(bitmap.0 as isize)) {
        return None;
    }
    // SAFETY: the structures have their sizes, the pixel buffer matches
    // the size given to GetDIBits, and the screen's device context is
    // released; the bitmap stays the menu's.
    unsafe {
        let mut described = BITMAP::default();
        let read = GetObjectW(
            bitmap.into(),
            size_of::<BITMAP>() as i32,
            Some((&raw mut described).cast()),
        );
        let (width, height) = (described.bmWidth, described.bmHeight.abs());
        if read == 0 || width <= 0 || height <= 0 {
            return None;
        }
        let mut header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (width * height * 4) as usize];
        let screen = GetDC(None);
        let copied = GetDIBits(
            screen,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &raw mut header,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, screen);
        if copied == 0 {
            return None;
        }
        straighten(&mut bgra);
        Some(IconPixels {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

/// Turns premultiplied BGRA into straight alpha; pixels without any alpha
/// at all are of a bitmap without it, and are made opaque.
fn straighten(bgra: &mut [u8]) {
    let pixels = bgra.as_chunks_mut::<4>().0;
    if pixels.iter().all(|pixel| pixel[3] == 0) {
        pixels.iter_mut().for_each(|pixel| pixel[3] = 0xff);
        return;
    }
    for pixel in pixels {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 || alpha == 0xff {
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * 0xff + alpha / 2) / alpha).min(0xff) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_loses_its_marks() {
        assert_eq!(label_of("&Open"), ("Open".to_owned(), Some('o')));
        assert_eq!(
            label_of("Save && &Close"),
            ("Save & Close".to_owned(), Some('c'))
        );
        assert_eq!(label_of("Cu&t\tCtrl+X"), ("Cut".to_owned(), Some('t')));
        assert_eq!(label_of("Plain"), ("Plain".to_owned(), None));
        assert_eq!(label_of("Trailing&"), ("Trailing".to_owned(), None));
    }

    #[test]
    fn the_verbs_of_a_view_are_intercepted() {
        assert_eq!(Intercept::of("Rename"), Some(Intercept::Rename));
        assert_eq!(Intercept::of("NewFolder"), Some(Intercept::NewFolder));
        assert_eq!(Intercept::of("delete"), Some(Intercept::Delete));
        assert_eq!(Intercept::of("arrange"), Some(Intercept::Sort));
        assert_eq!(Intercept::of("open"), None);
        assert_eq!(Intercept::of("properties"), None);
    }

    #[test]
    fn premultiplied_pixels_are_straightened() {
        let mut half = [50, 100, 0, 128, 10, 20, 30, 255];
        straighten(&mut half);
        assert_eq!(half, [100, 199, 0, 128, 10, 20, 30, 255]);
        let mut flat = [1, 2, 3, 0];
        straighten(&mut flat);
        assert_eq!(flat, [1, 2, 3, 255]);
    }
}
