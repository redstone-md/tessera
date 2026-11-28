// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The view of a folder window as scripts see it: the `Document` of a
//! window of Tessera Files, and the background object of the desktop's
//! view (`IShellView::GetItemObject(SVGIO_BACKGROUND)`), an
//! `IShellFolderViewDual` over the folder the place shows.
//!
//! Its `Folder` is the Shell's own `Folder` of that place, from
//! `Shell.Application::NameSpace`, and its `Application` is that
//! `Shell.Application`, made in Tessera's process: an installer that
//! starts a program through `IShellDispatch2::ShellExecute` on the
//! desktop's view, so that it runs without elevation, has it started by
//! Tessera. `SelectItem` chooses an item in the window as `/select` does.
//! `SelectedItems` and `FocusedItem` give what programs chose through
//! these objects; what the user chose in the window is not known here.

use std::ffi::c_void;

use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, E_POINTER, HWND};
use windows::Win32::System::Com::IDispatch;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    _SVGIO, Folder, FolderItem, FolderItems, FolderItems_Impl, IShellFolder, IShellFolderViewDual,
    IShellFolderViewDual_Impl, SVGIO_BACKGROUND, SVGIO_SELECTION, SVGIO_TYPE_MASK,
};
use windows::core::{BSTR, GUID, HSTRING, IUnknown, Interface, Ref, implement};

use super::super::ExplorerSettings;
use super::Place;
use super::automation::{
    Given, Library, Objects, given, is_path, late_binding, path_of_text, shell_application,
};

/// The flags of `ViewOptions` (`ShellFolderViewOptions` of shldisp.h).
const SFVVO_SHOWALLOBJECTS: i32 = 0x1;
const SFVVO_SHOWEXTENSIONS: i32 = 0x2;
const SFVVO_SHOWSYSFILES: i32 = 0x20;

/// The view of the folder of a place.
#[implement(IShellFolderViewDual, Agile = false)]
pub(in super::super) struct FolderViewDual {
    place: Place,
}

impl FolderViewDual {
    pub(in super::super) fn new(place: Place) -> Self {
        Self { place }
    }
}

late_binding!(
    FolderViewDual_Impl,
    IShellFolderViewDual,
    Library::ShellControls
);

impl IShellFolderViewDual_Impl for FolderViewDual_Impl {
    fn Application(&self) -> windows::core::Result<IDispatch> {
        shell_application()?.cast()
    }

    fn Parent(&self) -> windows::core::Result<IDispatch> {
        shell_application()?.cast()
    }

    fn Folder(&self) -> windows::core::Result<Folder> {
        self.place.folder()
    }

    fn SelectedItems(&self) -> windows::core::Result<FolderItems> {
        Ok(ChosenItems { place: self.place }.into())
    }

    fn FocusedItem(&self) -> windows::core::Result<FolderItem> {
        let Some(first) = self.place.chosen().into_iter().next() else {
            // No item has the focus: S_OK and no item, as documented.
            return Err(windows::core::Error::empty());
        };
        // SAFETY: the name outlives the call.
        unsafe { self.place.folder()?.ParseName(&BSTR::from(first)) }
    }

    fn SelectItem(&self, item: *const VARIANT, flags: i32) -> windows::core::Result<()> {
        // SAFETY: the variant is the one the caller passed.
        let name = name_of(&unsafe { given(item) });
        self.place.choose(name, flags);
        Ok(())
    }

    fn PopupItemMenu(
        &self,
        _item: Ref<'_, FolderItem>,
        _x: &VARIANT,
        _y: &VARIANT,
    ) -> windows::core::Result<BSTR> {
        Err(E_NOTIMPL.into())
    }

    fn Script(&self) -> windows::core::Result<IDispatch> {
        Err(E_NOTIMPL.into())
    }

    fn ViewOptions(&self) -> windows::core::Result<i32> {
        let settings = ExplorerSettings::read();
        let mut options = 0;
        if settings.show_hidden {
            options |= SFVVO_SHOWALLOBJECTS;
        }
        if !settings.hide_extensions {
            options |= SFVVO_SHOWEXTENSIONS;
        }
        if settings.show_protected {
            options |= SFVVO_SHOWSYSFILES;
        }
        Ok(options)
    }
}

/// The name in its folder of the item a program names: a `FolderItem`,
/// a path, or a name.
fn name_of(given: &Given) -> Option<String> {
    let text = match given {
        Given::Text(text) => text.clone(),
        Given::Object(Some(object)) => {
            let item: FolderItem = object.cast().ok()?;
            // SAFETY: the call only reads the path of the item.
            unsafe { item.Path() }.ok()?.to_string()
        }
        _ => return None,
    };
    Some(name_in_folder(&text)).filter(|name| !name.is_empty())
}

/// The last part of a path or a parsing name, which names the item in its
/// folder.
pub(super) fn name_in_folder(text: &str) -> String {
    let text = text.trim();
    if is_path(text) {
        let path = path_of_text(text);
        return { path.file_name() }.map_or_else(
            || text.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
    }
    let trimmed = text.trim_end_matches('\\');
    trimmed.rsplit('\\').next().unwrap_or(trimmed).to_owned()
}

/// The items programs chose in a place, as `SelectedItems` gives them.
#[implement(FolderItems, Agile = false)]
struct ChosenItems {
    place: Place,
}

late_binding!(ChosenItems_Impl, FolderItems, Library::ShellControls);

impl ChosenItems_Impl {
    fn item(&self, name: &str) -> windows::core::Result<FolderItem> {
        // SAFETY: the name outlives the call.
        unsafe { self.place.folder()?.ParseName(&BSTR::from(name)) }
    }
}

impl FolderItems_Impl for ChosenItems_Impl {
    fn Count(&self) -> windows::core::Result<i32> {
        Ok(self.place.chosen().len() as i32)
    }

    fn Application(&self) -> windows::core::Result<IDispatch> {
        shell_application()?.cast()
    }

    fn Parent(&self) -> windows::core::Result<IDispatch> {
        self.place.folder()?.cast()
    }

    fn Item(&self, index: &VARIANT) -> windows::core::Result<FolderItem> {
        let chosen = self.place.chosen();
        // SAFETY: the variant is the one the caller passed.
        let name = match unsafe { given(index) } {
            Given::Number(index) => usize::try_from(index)
                .ok()
                .and_then(|index| chosen.get(index).cloned()),
            Given::Text(name) => { chosen.iter() }
                .find(|chosen| chosen.eq_ignore_ascii_case(&name))
                .cloned(),
            _ => None,
        };
        self.item(&name.ok_or(windows::core::Error::from(E_INVALIDARG))?)
    }

    fn _NewEnum(&self) -> windows::core::Result<IUnknown> {
        let chosen = self.place.chosen();
        let items = { chosen.iter() }
            .filter_map(|name| self.item(name).ok())
            .filter_map(|item| item.cast::<IDispatch>().ok())
            .collect();
        let objects: windows::Win32::System::Ole::IEnumVARIANT = Objects::new(items).into();
        objects.cast()
    }
}

/// Answers `IShellView::GetItemObject` for a view of `place` whose window
/// is `owner`: the background as its `IShellFolderViewDual`, or as the
/// folder's own object (`IShellFolder::CreateViewObject`, which gives the
/// menu of the background for `IContextMenu`), and the items chosen as
/// the folder's object of them (`GetUIObjectOf`).
pub(in super::super) fn item_object(
    place: &Place,
    owner: HWND,
    kind: &_SVGIO,
    iid: *const GUID,
    object: *mut *mut c_void,
) -> windows::core::Result<()> {
    // SAFETY: the caller passes the interface it asks for.
    let wanted = unsafe { iid.as_ref() }.ok_or(windows::core::Error::from(E_POINTER))?;
    match kind.0 & SVGIO_TYPE_MASK.0 {
        kind if kind == SVGIO_BACKGROUND.0 => {
            let automation = [IDispatch::IID, IShellFolderViewDual::IID];
            if automation.contains(wanted) {
                let view: IShellFolderViewDual = FolderViewDual::new(*place).into();
                // SAFETY: the caller gives where to put the interface.
                return unsafe { view.query(iid, object) }.ok();
            }
            let folder = place.shell_folder()?;
            // SAFETY: the call writes the object asked for into `object`.
            unsafe {
                (Interface::vtable(&folder).CreateViewObject)(folder.as_raw(), owner, iid, object)
            }
            .ok()
        }
        kind if kind == SVGIO_SELECTION.0 => {
            let folder = place.shell_folder()?;
            let names = place.chosen();
            if names.is_empty() {
                return Err(E_FAIL.into());
            }
            let children = children_of(&folder, owner, &names);
            let lists: Vec<*const ITEMIDLIST> = { children.iter() }
                .map(|child| child.0.cast_const())
                .collect();
            if lists.is_empty() {
                return Err(E_FAIL.into());
            }
            // SAFETY: the lists are alive for the call; the call writes the
            // object asked for into `object`.
            unsafe {
                (Interface::vtable(&folder).GetUIObjectOf)(
                    folder.as_raw(),
                    owner,
                    lists.len() as u32,
                    lists.as_ptr(),
                    iid,
                    std::ptr::null_mut(),
                    object,
                )
            }
            .ok()
        }
        _ => Err(E_NOTIMPL.into()),
    }
}

/// An item list of a child of a folder, freed when dropped.
pub(in super::super) struct Child(pub *mut ITEMIDLIST);

impl Drop for Child {
    fn drop(&mut self) {
        // SAFETY: the list was allocated by the Shell for this owner.
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(self.0.cast_const().cast())) }
    }
}

/// The item lists of the items called `names` in `folder`, as the folder
/// knows them; a name it does not know is left out.
pub(in super::super) fn children_of(
    folder: &IShellFolder,
    owner: HWND,
    names: &[String],
) -> Vec<Child> {
    let mut children = Vec::new();
    for name in names {
        let mut list = std::ptr::null_mut();
        // SAFETY: the name outlives the call, which writes a list owned
        // here from then on.
        let parsed = unsafe {
            folder.ParseDisplayName(
                owner,
                None,
                &HSTRING::from(name.as_str()),
                None,
                &raw mut list,
                std::ptr::null_mut(),
            )
        };
        if parsed.is_ok() && !list.is_null() {
            children.push(Child(list));
        }
    }
    children
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_item_is_named_by_the_last_part_of_its_path() {
        assert_eq!(name_in_folder(r"C:\Users\Me\a.txt"), "a.txt");
        assert_eq!(name_in_folder("file:///C:/Users/Me/b%20c.txt"), "b%20c.txt");
        assert_eq!(name_in_folder("a.txt"), "a.txt");
        assert_eq!(
            name_in_folder(r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\C:\"),
            "C:"
        );
    }
}
