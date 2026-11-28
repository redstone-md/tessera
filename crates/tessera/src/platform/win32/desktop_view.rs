// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The view of the desktop as other programs reach it, where Tessera is
//! the shell: one object that is both the desktop's `IShellView`, which
//! the desktop's automation object gives from `QueryActiveShellView`
//! (see shell_windows.rs), and its `IFolderView`, which tools that read
//! or save the places of the icons ask that view for.
//!
//! It answers over the desktop folder: `ItemCount`, `Item` and `Items`
//! give what the desktop folder holds (files of both Desktop folders and
//! the items of the Shell it shows), `GetFolder` gives the folder, and
//! `GetItemObject(SVGIO_BACKGROUND)` the desktop's
//! `IShellFolderViewDual`. Where the icons are, their spacing and whether
//! they arrange themselves come from the desktop of Tessera once it tells
//! them ([`DesktopView::set_layout`]); until then `GetItemPosition`,
//! `SelectAndPositionItems`, `GetSpacing` and `GetAutoArrange` answer
//! `E_NOTIMPL`. A program that places icons has the places handed to
//! the desktop ([`DesktopView::placements`]), never waited for.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock, PoisonError};

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{
    E_INVALIDARG, E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, POINT, RECT, S_FALSE,
};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::IOleWindow_Impl;
use windows::Win32::UI::Controls::LPFNSVADDPROPSHEETPAGE;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    _SVGIO, FOLDERSETTINGS, FVM_ICON, FWF_DESKTOP, FWF_NOCLIENTEDGE, IEnumIDList, IFolderView,
    IFolderView_Impl, ILClone, IShellBrowser, IShellFolder, IShellItemArray, IShellView,
    IShellView_Impl, SHCONTF_FOLDERS, SHCONTF_NONFOLDERS, SHCreateShellItemArray, SHGDN_FORPARSING,
    SVGIO_ALLVIEW, SVGIO_SELECTION, SVGIO_TYPE_MASK, SVSI_SELECT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, MSG, SM_CXICONSPACING, SM_CYICONSPACING,
};
use windows::core::{BOOL, GUID, HRESULT, Interface, Ref, implement};

use super::ShellHost;
use super::shell_windows::{Child, NAME_IN_FOLDER, Place, item_object, key_of, name_of_child};

/// Where the desktop of Tessera shows its icons, as it tells the view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopLayout {
    /// Each icon by the parsing name of its item, at its place in pixels
    /// of the desktop window.
    pub positions: Vec<(String, (i32, i32))>,
    /// The distance between icons, across and down, in pixels.
    pub spacing: (i32, i32),
    /// Whether the icons arrange themselves.
    pub auto_arrange: bool,
}

/// A place a program asked an icon of the desktop to be moved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// The parsing name of the item.
    pub parsing: String,
    /// The place, in pixels of the desktop window.
    pub at: (i32, i32),
}

static LAYOUT: Mutex<Option<DesktopLayout>> = Mutex::new(None);
static PLACEMENTS: OnceLock<(Sender<Placement>, Receiver<Placement>)> = OnceLock::new();

thread_local! {
    /// The view, made once on the thread that serves the folder windows.
    static VIEW: RefCell<Option<IShellView>> = const { RefCell::new(None) };
}

/// The view of the desktop.
pub struct DesktopView;

impl DesktopView {
    /// The view, as the desktop's browser gives it.
    pub(super) fn shared() -> windows::core::Result<IShellView> {
        VIEW.with(|view| {
            let mut view = view.borrow_mut();
            Ok(view.get_or_insert_with(|| DesktopFolderView.into()).clone())
        })
    }

    /// Tells the view where the icons are, or that it does not know
    /// (`None`), which is how it starts.
    #[allow(
        dead_code,
        reason = "called with DesktopIcons::positions once desktop-fidelity is merged"
    )]
    pub fn set_layout(layout: Option<DesktopLayout>) {
        *LAYOUT.lock().unwrap_or_else(PoisonError::into_inner) = layout;
    }

    /// The places programs asked icons to be moved to.
    #[allow(
        dead_code,
        reason = "handed to DesktopIcons::place once desktop-fidelity is merged"
    )]
    pub fn placements() -> Receiver<Placement> {
        PLACEMENTS.get_or_init(async_channel::unbounded).1.clone()
    }
}

/// The layout the desktop told, where it told one.
fn layout() -> Option<DesktopLayout> {
    LAYOUT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The window the view draws in: the desktop's `SHELLDLL_DefView`.
fn view_window() -> HWND {
    let window = ShellHost::def_view_window()
        .or_else(ShellHost::desktop_window)
        .map_or(0, |window| window.raw());
    HWND(window as *mut c_void)
}

/// The desktop folder.
fn desktop() -> windows::core::Result<IShellFolder> {
    Place::Desktop.shell_folder()
}

/// The items the desktop folder holds, as it knows them.
fn children(folder: &IShellFolder) -> Vec<Child> {
    let mut found = None::<IEnumIDList>;
    let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0) as u32;
    // SAFETY: the enumerator is written into `found`, which outlives the
    // call; each list it gives is owned here from then on.
    unsafe {
        if folder
            .EnumObjects(view_window(), flags, &raw mut found)
            .is_err()
        {
            return Vec::new();
        }
        let Some(items) = found else {
            return Vec::new();
        };
        let mut children = Vec::new();
        loop {
            let mut list = [std::ptr::null_mut::<ITEMIDLIST>()];
            let mut fetched = 0u32;
            if items.Next(&mut list, Some(&raw mut fetched)) != windows::core::HRESULT(0)
                || fetched == 0
            {
                break;
            }
            children.push(Child(list[0]));
        }
        children
    }
}

/// The items the flags `SVGIO_*` name: all of them, or those programs
/// chose.
fn items(folder: &IShellFolder, kind: &_SVGIO) -> windows::core::Result<Vec<Child>> {
    match kind.0 & SVGIO_TYPE_MASK.0 {
        kind if kind == SVGIO_ALLVIEW.0 => Ok(children(folder)),
        kind if kind == SVGIO_SELECTION.0 => Ok(super::shell_windows::children_of(
            folder,
            view_window(),
            &Place::Desktop.chosen(),
        )),
        _ => Err(E_INVALIDARG.into()),
    }
}

/// The parsing name of a child of the desktop.
fn parsing_of(folder: &IShellFolder, child: *const ITEMIDLIST) -> Option<String> {
    name_of_child(folder, child, SHGDN_FORPARSING)
}

/// The place of the item `parsing` names in `layout`.
fn position_in(layout: &DesktopLayout, parsing: &str) -> Option<(i32, i32)> {
    let key = key_of(parsing);
    { layout.positions.iter() }
        .find(|(other, _)| key_of(other) == key)
        .map(|(_, at)| *at)
}

/// The view of the desktop.
#[implement(IShellView, IFolderView, Agile = false)]
struct DesktopFolderView;

impl IOleWindow_Impl for DesktopFolderView_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        Ok(view_window())
    }
    fn ContextSensitiveHelp(&self, _enter: BOOL) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
}

impl IShellView_Impl for DesktopFolderView_Impl {
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
        _view: *const RECT,
    ) -> windows::core::Result<HWND> {
        Err(E_NOTIMPL.into())
    }
    fn DestroyViewWindow(&self) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn GetCurrentInfo(&self) -> windows::core::Result<FOLDERSETTINGS> {
        Ok(FOLDERSETTINGS {
            ViewMode: FVM_ICON.0 as u32,
            fFlags: (FWF_DESKTOP.0 | FWF_NOCLIENTEDGE.0) as u32,
        })
    }
    fn AddPropertySheetPages(
        &self,
        _reserved: u32,
        _add: LPFNSVADDPROPSHEETPAGE,
        _lparam: LPARAM,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn SaveViewState(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn SelectItem(&self, item: *const ITEMIDLIST, flags: u32) -> windows::core::Result<()> {
        let name = match item.is_null() {
            true => None,
            false => Some(
                name_of_child(&desktop()?, item, NAME_IN_FOLDER)
                    .ok_or(windows::core::Error::from(E_INVALIDARG))?,
            ),
        };
        Place::Desktop.choose(name, flags as i32);
        Ok(())
    }
    fn GetItemObject(
        &self,
        kind: &_SVGIO,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        item_object(&Place::Desktop, view_window(), kind, iid, object)
    }
}

impl IFolderView_Impl for DesktopFolderView_Impl {
    fn GetCurrentViewMode(&self) -> windows::core::Result<u32> {
        Ok(FVM_ICON.0 as u32)
    }

    fn SetCurrentViewMode(&self, _mode: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn GetFolder(&self, iid: *const GUID, object: *mut *mut c_void) -> windows::core::Result<()> {
        // SAFETY: the caller gives the interface it wants and where to put
        // it.
        unsafe { desktop()?.query(iid, object) }.ok()
    }

    fn Item(&self, index: i32) -> windows::core::Result<*mut ITEMIDLIST> {
        let children = children(&desktop()?);
        let child = usize::try_from(index)
            .ok()
            .and_then(|index| children.get(index))
            .ok_or(windows::core::Error::from(E_INVALIDARG))?;
        // SAFETY: the copy is the caller's to free.
        Ok(unsafe { ILClone(child.0) })
    }

    fn ItemCount(&self, kind: &_SVGIO) -> windows::core::Result<i32> {
        Ok(items(&desktop()?, kind)?.len() as i32)
    }

    fn Items(
        &self,
        kind: &_SVGIO,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        let folder = desktop()?;
        let items = items(&folder, kind)?;
        let lists: Vec<*const ITEMIDLIST> = items.iter().map(|item| item.0.cast_const()).collect();
        // SAFETY: the caller passes the interface it asks for.
        let wanted = unsafe { iid.as_ref() }.ok_or(windows::core::Error::from(E_INVALIDARG))?;
        if *wanted == IDataObject::IID {
            if lists.is_empty() {
                return Err(E_INVALIDARG.into());
            }
            // SAFETY: the lists are alive for the call, which writes the
            // object into `object`.
            return unsafe {
                (Interface::vtable(&folder).GetUIObjectOf)(
                    folder.as_raw(),
                    view_window(),
                    lists.len() as u32,
                    lists.as_ptr(),
                    iid,
                    std::ptr::null_mut(),
                    object,
                )
            }
            .ok();
        }
        // SAFETY: the lists are alive for the call, which copies them.
        let array: IShellItemArray =
            unsafe { SHCreateShellItemArray(None, &folder, Some(&lists)) }?;
        // SAFETY: the caller gives where to put the interface.
        let found = unsafe { array.query(iid, object) };
        if found.is_err() {
            return Err(E_NOINTERFACE.into());
        }
        Ok(())
    }

    fn GetSelectionMarkedItem(&self) -> windows::core::Result<i32> {
        self.GetFocusedItem()
    }

    fn GetFocusedItem(&self) -> windows::core::Result<i32> {
        let Some(first) = Place::Desktop.chosen().into_iter().next() else {
            return Ok(-1);
        };
        let folder = desktop()?;
        let children = children(&folder);
        let index = { children.iter() }.position(|child| {
            name_of_child(&folder, child.0, NAME_IN_FOLDER)
                .is_some_and(|name| name.eq_ignore_ascii_case(&first))
        });
        Ok(index.map_or(-1, |index| index as i32))
    }

    fn GetItemPosition(&self, item: *const ITEMIDLIST) -> windows::core::Result<POINT> {
        let layout = layout().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        let parsing =
            parsing_of(&desktop()?, item).ok_or(windows::core::Error::from(E_INVALIDARG))?;
        let (x, y) =
            position_in(&layout, &parsing).ok_or(windows::core::Error::from(E_INVALIDARG))?;
        Ok(POINT { x, y })
    }

    fn GetSpacing(&self, spacing: *mut POINT) -> windows::core::Result<()> {
        let layout = layout().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        if !spacing.is_null() {
            let (x, y) = layout.spacing;
            // SAFETY: the caller gives where the spacing is written.
            unsafe { spacing.write(POINT { x, y }) };
        }
        Ok(())
    }

    fn GetDefaultSpacing(&self) -> windows::core::Result<POINT> {
        // SAFETY: the calls only read metrics of the system.
        unsafe {
            Ok(POINT {
                x: GetSystemMetrics(SM_CXICONSPACING),
                y: GetSystemMetrics(SM_CYICONSPACING),
            })
        }
    }

    fn GetAutoArrange(&self) -> windows::core::Result<()> {
        match layout() {
            Some(layout) if layout.auto_arrange => Ok(()),
            Some(_) => Err(S_FALSE.into()),
            None => Err(E_NOTIMPL.into()),
        }
    }

    fn SelectItem(&self, index: i32, flags: u32) -> windows::core::Result<()> {
        let folder = desktop()?;
        let children = children(&folder);
        let child = usize::try_from(index)
            .ok()
            .and_then(|index| children.get(index))
            .ok_or(windows::core::Error::from(E_INVALIDARG))?;
        let name = name_of_child(&folder, child.0, NAME_IN_FOLDER);
        Place::Desktop.choose(name, flags as i32);
        Ok(())
    }

    fn SelectAndPositionItems(
        &self,
        count: u32,
        lists: *const *const ITEMIDLIST,
        points: *const POINT,
        flags: u32,
    ) -> windows::core::Result<()> {
        if layout().is_none() {
            return Err(E_NOTIMPL.into());
        }
        if count > 0 && lists.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let folder = desktop()?;
        let (sender, _) = PLACEMENTS.get_or_init(async_channel::unbounded);
        for index in 0..count as usize {
            // SAFETY: the caller passes `count` lists, and as many points
            // where it passes points.
            let (list, point) = unsafe {
                let point = (!points.is_null()).then(|| points.add(index).read());
                (lists.add(index).read(), point)
            };
            if let (Some(parsing), Some(point)) = (parsing_of(&folder, list), point) {
                let _ = sender.try_send(Placement {
                    parsing,
                    at: (point.x, point.y),
                });
            }
            if flags & SVSI_SELECT.0 as u32 != 0 {
                let name = name_of_child(&folder, list, NAME_IN_FOLDER);
                Place::Desktop.choose(name, flags as i32);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_is_found_by_its_parsing_name() {
        let layout = DesktopLayout {
            positions: vec![
                (r"C:\Users\Me\Desktop\a.txt".to_owned(), (10, 20)),
                (
                    "::{645FF040-5081-101B-9F08-00AA002F954E}".to_owned(),
                    (30, 40),
                ),
            ],
            spacing: (75, 100),
            auto_arrange: false,
        };
        assert_eq!(
            position_in(&layout, r"c:\users\me\desktop\A.TXT"),
            Some((10, 20))
        );
        assert_eq!(
            position_in(&layout, "::{645ff040-5081-101b-9f08-00aa002f954e}"),
            Some((30, 40))
        );
        assert_eq!(position_in(&layout, r"C:\elsewhere"), None);
    }
}
