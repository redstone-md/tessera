// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the automation objects of the folder windows share: late binding
//! through the type libraries of Windows, the `VARIANT`s scripts pass and
//! get, the Shell's own `Shell.Application` object, and an enumerator of
//! objects for `_NewEnum`.
//!
//! Scripts reach the objects through `IDispatch` alone (`Shell.Application`
//! in VBScript and PowerShell, `ShellWindows` in AutoHotkey). The objects
//! describe themselves by the type information Windows registers for the
//! interfaces they implement: `IShellWindows` and `IWebBrowser2` in the
//! library "Microsoft Internet Controls" (SHDocVw), `IShellFolderViewDual`
//! and `FolderItems` in "Microsoft Shell Controls And Automation"
//! (Shell32). `GetIDsOfNames` and `Invoke` are those of the type
//! information (`DispGetIDsOfNames`, `DispInvoke`) over the object's own
//! table of functions, which is laid out as the type information says.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{S_FALSE, S_OK, VARIANT_BOOL};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IDispatch, ITypeInfo, SAFEARRAY,
    TKIND_DISPATCH,
};
use windows::Win32::System::Ole::{
    IEnumVARIANT, IEnumVARIANT_Impl, LoadRegTypeLib, SafeArrayAccessData, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_ARRAY, VT_BOOL, VT_BSTR, VT_BYREF,
    VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_I2, VT_I4, VT_I8, VT_INT, VT_UI1, VT_UI2, VT_UI4, VT_UINT,
    VT_VARIANT, VariantClear,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    Folder, FolderItem, IShellDispatch, SHGetFolderLocation, SHGetNameFromIDList,
    SHParseDisplayName, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
    Shell as CLSID_SHELL,
};
use windows::core::{BSTR, GUID, HRESULT, HSTRING, Interface, implement};

/// The flag of a dual interface in `TYPEATTR::wTypeFlags`
/// (`TYPEFLAG_FDUAL` of oaidl.h).
const TYPEFLAG_FDUAL: u16 = 0x40;

/// A type library of Windows that describes automation interfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Library {
    /// "Microsoft Internet Controls", SHDocVw 1.1: `IShellWindows`,
    /// `IWebBrowser2`.
    InternetControls,
    /// "Microsoft Shell Controls And Automation", Shell32 1.0:
    /// `IShellFolderViewDual`, `FolderItems`, `Folder`.
    ShellControls,
}

impl Library {
    /// Its identifier and version, as registered under `HKCR\TypeLib`.
    fn registered(self) -> (GUID, u16, u16) {
        match self {
            Self::InternetControls => (
                GUID::from_u128(0xeab22ac0_30c1_11cf_a7eb_0000c05bae0b),
                1,
                1,
            ),
            Self::ShellControls => (
                GUID::from_u128(0x50a7e9b0_70ef_11d1_b75a_00a0c90564fe),
                1,
                0,
            ),
        }
    }
}

thread_local! {
    /// The type information found so far, by interface; `None` where the
    /// library or the interface is missing, so it is not looked for again.
    static TYPE_INFOS: RefCell<HashMap<u128, Option<ITypeInfo>>> = RefCell::new(HashMap::new());
    /// The Shell's own `Shell.Application`, made once per thread.
    static SHELL: RefCell<Option<IShellDispatch>> = const { RefCell::new(None) };
}

/// The type information of `interface` in `library`, as `DispInvoke`
/// needs it: for a dual interface, that of its table of functions rather
/// than of its dispatch part.
pub(super) fn type_info(library: Library, interface: &GUID) -> Option<ITypeInfo> {
    let key = interface.to_u128();
    if let Some(found) = TYPE_INFOS.with(|infos| infos.borrow().get(&key).cloned()) {
        return found;
    }
    let (id, major, minor) = library.registered();
    // SAFETY: the identifiers outlive the calls; the attributes are read
    // once and given back.
    let found = unsafe {
        LoadRegTypeLib(&id, major, minor, 0)
            .and_then(|library| library.GetTypeInfoOfGuid(interface))
            .ok()
            .and_then(|info| {
                let attributes = info.GetTypeAttr().ok()?;
                let dual = (*attributes).typekind == TKIND_DISPATCH
                    && (*attributes).wTypeFlags & TYPEFLAG_FDUAL != 0;
                info.ReleaseTypeAttr(attributes);
                if !dual {
                    return Some(info);
                }
                // The interface the dispatch part is the dual of.
                let reference = info.GetRefTypeOfImplType(u32::MAX).ok()?;
                info.GetRefTypeInfo(reference).ok()
            })
    };
    if found.is_none() {
        report!("Windows has no type information for {interface:?}; scripts cannot use it");
    }
    TYPE_INFOS.with(|infos| infos.borrow_mut().insert(key, found.clone()));
    found
}

/// Implements `IDispatch` for `$impl` through the type information of
/// `$interface` in `$library`.
macro_rules! late_binding {
    ($impl:ty, $interface:ty, $library:expr) => {
        impl windows::Win32::System::Com::IDispatch_Impl for $impl {
            fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
                let found = $crate::platform::win32::shell_windows::automation::type_info(
                    $library,
                    &<$interface as windows::core::Interface>::IID,
                );
                Ok(u32::from(found.is_some()))
            }

            fn GetTypeInfo(
                &self,
                index: u32,
                _locale: u32,
            ) -> windows::core::Result<windows::Win32::System::Com::ITypeInfo> {
                if index != 0 {
                    return Err(windows::Win32::Foundation::DISP_E_BADINDEX.into());
                }
                $crate::platform::win32::shell_windows::automation::type_info(
                    $library,
                    &<$interface as windows::core::Interface>::IID,
                )
                .ok_or_else(|| windows::Win32::Foundation::E_NOTIMPL.into())
            }

            fn GetIDsOfNames(
                &self,
                _iid: *const windows::core::GUID,
                names: *const windows::core::PCWSTR,
                count: u32,
                _locale: u32,
                ids: *mut i32,
            ) -> windows::core::Result<()> {
                let info = $crate::platform::win32::shell_windows::automation::type_info(
                    $library,
                    &<$interface as windows::core::Interface>::IID,
                )
                .ok_or(windows::core::Error::from(
                    windows::Win32::Foundation::DISP_E_UNKNOWNNAME,
                ))?;
                // SAFETY: the caller passes `count` names and room for as
                // many identifiers, as `GetIDsOfNames` is called.
                unsafe { windows::Win32::System::Ole::DispGetIDsOfNames(&info, names, count, ids) }
            }

            fn Invoke(
                &self,
                member: i32,
                _iid: *const windows::core::GUID,
                _locale: u32,
                flags: windows::Win32::System::Com::DISPATCH_FLAGS,
                parameters: *const windows::Win32::System::Com::DISPPARAMS,
                result: *mut windows::Win32::System::Variant::VARIANT,
                exception: *mut windows::Win32::System::Com::EXCEPINFO,
                argument: *mut u32,
            ) -> windows::core::Result<()> {
                let info = $crate::platform::win32::shell_windows::automation::type_info(
                    $library,
                    &<$interface as windows::core::Interface>::IID,
                )
                .ok_or(windows::core::Error::from(
                    windows::Win32::Foundation::DISP_E_MEMBERNOTFOUND,
                ))?;
                let this: $interface = windows::core::IUnknownImpl::to_interface(self);
                // SAFETY: the table of functions of `this` is laid out as
                // the type information describes it; the arguments are
                // passed on as the caller gave them.
                unsafe {
                    windows::Win32::System::Ole::DispInvoke(
                        windows::core::Interface::as_raw(&this),
                        &info,
                        member,
                        flags.0,
                        parameters.cast_mut(),
                        result,
                        exception,
                        argument,
                    )
                }
            }
        }
    };
}

pub(super) use late_binding;

/// What a `VARIANT` a program passed holds, as far as the folder windows
/// read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Given {
    /// Nothing, or an optional argument left out.
    Nothing,
    Number(i64),
    Text(String),
    /// An item list, in an array of bytes.
    IdList(Vec<u8>),
    /// An automation object, such as a `FolderItem`.
    Object(Option<IDispatch>),
    /// A kind the folder windows do not read.
    Other(u16),
}

/// What `variant` holds; one passed by reference is followed.
///
/// # Safety
/// `variant` is null or points to a valid `VARIANT`.
pub(super) unsafe fn given(variant: *const VARIANT) -> Given {
    // SAFETY: as the caller promises; each field is read for the kind the
    // variant says it holds.
    unsafe {
        let Some(variant) = variant.as_ref() else {
            return Given::Nothing;
        };
        let inner = &variant.Anonymous.Anonymous;
        let value = &inner.Anonymous;
        let kind = inner.vt;
        if kind.0 == VT_BYREF.0 | VT_VARIANT.0 {
            return given(value.pvarVal);
        }
        match kind {
            VT_EMPTY | VT_ERROR => Given::Nothing,
            VT_I2 => Given::Number(i64::from(value.iVal)),
            VT_I4 | VT_INT => Given::Number(i64::from(value.lVal)),
            VT_I8 => Given::Number(value.llVal),
            VT_UI1 => Given::Number(i64::from(value.bVal)),
            VT_UI2 => Given::Number(i64::from(value.uiVal)),
            VT_UI4 | VT_UINT => Given::Number(i64::from(value.ulVal)),
            VT_BOOL => Given::Number(i64::from(value.boolVal.0 != 0)),
            VT_BSTR => Given::Text(value.bstrVal.to_string()),
            VT_DISPATCH => Given::Object((*value.pdispVal).clone()),
            kind if kind.0 == VT_ARRAY.0 | VT_UI1.0 => bytes_of(value.parray)
                .map(Given::IdList)
                .unwrap_or(Given::Other(kind.0)),
            kind => Given::Other(kind.0),
        }
    }
}

/// The bytes of a one-dimensional array of bytes.
///
/// # Safety
/// `array` is null or a valid array of `VT_UI1`.
unsafe fn bytes_of(array: *const SAFEARRAY) -> Option<Vec<u8>> {
    if array.is_null() {
        return None;
    }
    // SAFETY: as the caller promises; the data is accessed and let go
    // once, and read within the bound the array gives.
    unsafe {
        let last = SafeArrayGetUBound(array, 1).ok()?;
        let length = usize::try_from(last + 1).ok()?;
        let mut data = std::ptr::null_mut();
        SafeArrayAccessData(array, &raw mut data).ok()?;
        let bytes = std::slice::from_raw_parts(data.cast::<u8>(), length).to_vec();
        let _ = SafeArrayUnaccessData(array);
        Some(bytes)
    }
}

/// A `VARIANT` owned here, cleared when dropped.
pub(super) struct OwnedVariant(pub VARIANT);

impl OwnedVariant {
    fn of(kind: VARENUM, value: VARIANT_0_0_0) -> Self {
        Self(VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: kind,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: value,
                }),
            },
        })
    }

    pub(super) fn empty() -> Self {
        Self(VARIANT::default())
    }

    pub(super) fn number(number: i32) -> Self {
        Self::of(VT_I4, VARIANT_0_0_0 { lVal: number })
    }

    pub(super) fn text(text: &str) -> Self {
        Self::of(
            VT_BSTR,
            VARIANT_0_0_0 {
                bstrVal: ManuallyDrop::new(BSTR::from(text)),
            },
        )
    }

    pub(super) fn object(object: IDispatch) -> Self {
        Self::of(
            VT_DISPATCH,
            VARIANT_0_0_0 {
                pdispVal: ManuallyDrop::new(Some(object)),
            },
        )
    }

    /// Gives the variant away, to a caller who clears it.
    pub(super) fn into_raw(self) -> VARIANT {
        let this = ManuallyDrop::new(self);
        // SAFETY: the variant is moved out once; `self` is not dropped.
        unsafe { std::ptr::read(&this.0) }
    }
}

impl Drop for OwnedVariant {
    fn drop(&mut self) {
        // SAFETY: the variant was made here with a value of its kind.
        let _ = unsafe { VariantClear(&raw mut self.0) };
    }
}

/// `path` as an address, as `LocationURL` gives it.
pub(super) fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    match text.strip_prefix("//") {
        Some(share) => format!("file://{share}"),
        None => format!("file:///{text}"),
    }
}

/// The path an address of `LocationURL` or a path names.
pub(super) fn path_of_text(text: &str) -> PathBuf {
    let lower = text.to_ascii_lowercase();
    match lower.strip_prefix("file:") {
        Some(rest) if rest.starts_with("///") => PathBuf::from(text[8..].replace('/', "\\")),
        Some(rest) if rest.starts_with("//") => {
            PathBuf::from(format!("\\\\{}", text[7..].replace('/', "\\")))
        }
        _ => PathBuf::from(text),
    }
}

/// Whether `text` names a place by an address of the file system,
/// `file:` or a path, rather than by a parsing name of the Shell.
pub(super) fn is_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || text.starts_with("\\\\") || text.to_ascii_lowercase().starts_with("file:")
}

/// The parsing name of a place as a program names it: a path or an
/// address, a parsing name of the Shell, a number of a special folder
/// (`ssfDESKTOP` = 0, `ssfDRIVES` = 17 …), an item list, or a
/// `FolderItem` or `Folder` object.
pub(super) fn parsing_of(given: &Given) -> Option<String> {
    match given {
        Given::Text(text) if text.trim().is_empty() => None,
        Given::Text(text) if is_path(text) => {
            Some(path_of_text(text.trim()).to_string_lossy().into_owned())
        }
        Given::Text(text) => Some(text.trim().to_owned()),
        Given::Number(number) => {
            let special = i32::try_from(*number).ok()?;
            // SAFETY: the list the Shell allocates is read and freed once.
            unsafe {
                let list = SHGetFolderLocation(None, special, None, 0).ok()?;
                let name = name_of_list(list, SIGDN_DESKTOPABSOLUTEPARSING);
                CoTaskMemFree(Some(list.cast()));
                name
            }
        }
        Given::IdList(bytes) => {
            super::super::shell_item::id_list_length(bytes)?;
            // SAFETY: the bytes hold a whole item list, with its ending.
            unsafe {
                name_of_list(
                    bytes.as_ptr().cast::<ITEMIDLIST>(),
                    SIGDN_DESKTOPABSOLUTEPARSING,
                )
            }
        }
        Given::Object(Some(object)) => {
            // SAFETY: the calls only read the path of the item.
            unsafe {
                if let Ok(item) = object.cast::<FolderItem>() {
                    return item.Path().ok().map(|path| path.to_string());
                }
                let folder = object.cast::<Folder>().ok()?;
                let item = folder.cast::<windows::Win32::UI::Shell::Folder2>().ok()?;
                item.Self_().ok()?.Path().ok().map(|path| path.to_string())
            }
        }
        Given::Object(None) | Given::Nothing | Given::Other(_) => None,
    }
}

/// The name of the kind `kind` of what an item list names.
///
/// # Safety
/// `list` is a valid absolute item list.
pub(super) unsafe fn name_of_list(list: *const ITEMIDLIST, kind: SIGDN) -> Option<String> {
    // SAFETY: as the caller promises; the text is read and freed once.
    unsafe {
        let text = SHGetNameFromIDList(list, kind).ok()?;
        let name = text.to_string().ok();
        CoTaskMemFree(Some(text.0.cast_const().cast()));
        name
    }
}

/// The name the Shell shows for the place `parsing` names.
pub(super) fn display_name_of(parsing: &str) -> Option<String> {
    let mut list = std::ptr::null_mut();
    // SAFETY: the name outlives the call; the list it gives is read and
    // freed once.
    unsafe {
        SHParseDisplayName(&HSTRING::from(parsing), None, &raw mut list, 0, None).ok()?;
        let name = name_of_list(list, SIGDN_NORMALDISPLAY);
        CoTaskMemFree(Some(list.cast_const().cast()));
        name
    }
}

/// The Shell's own `Shell.Application` of this thread, which programs
/// get as the `Application` of the folder windows: its `ShellExecute`
/// starts programs from Tessera's process.
pub(super) fn shell_application() -> windows::core::Result<IShellDispatch> {
    SHELL.with(|shell| {
        if let Some(shell) = shell.borrow().as_ref() {
            return Ok(shell.clone());
        }
        // SAFETY: the class is the Shell's, made in this process.
        let made: IShellDispatch =
            unsafe { CoCreateInstance(&CLSID_SHELL, None, CLSCTX_INPROC_SERVER) }?;
        *shell.borrow_mut() = Some(made.clone());
        Ok(made)
    })
}

/// The `Folder` of the place `place` names, from `Shell.Application`:
/// `NameSpace` takes a path, a parsing name or the number of a special
/// folder.
pub(super) fn folder_of(place: &OwnedVariant) -> windows::core::Result<Folder> {
    let shell = shell_application()?;
    // SAFETY: the variant is alive for the call, which copies what it needs.
    unsafe { shell.NameSpace(&place.0) }
}

/// A `VARIANT_BOOL` of `value`.
pub(super) fn boolean(value: bool) -> VARIANT_BOOL {
    VARIANT_BOOL(if value { -1 } else { 0 })
}

/// An enumerator of automation objects, for `_NewEnum`.
#[implement(IEnumVARIANT, Agile = false)]
pub(super) struct Objects {
    objects: Vec<IDispatch>,
    next: std::cell::Cell<usize>,
}

impl Objects {
    pub(super) fn new(objects: Vec<IDispatch>) -> Self {
        Self {
            objects,
            next: std::cell::Cell::new(0),
        }
    }
}

impl IEnumVARIANT_Impl for Objects_Impl {
    fn Next(&self, count: u32, variants: *mut VARIANT, fetched: *mut u32) -> HRESULT {
        let start = self.next.get();
        let given = { self.objects.iter().skip(start) }.take(count as usize);
        let mut written = 0u32;
        for object in given {
            // SAFETY: the caller gives room for `count` variants; each is
            // written once, with a reference of its own.
            unsafe {
                variants
                    .add(written as usize)
                    .write(OwnedVariant::object(object.clone()).into_raw());
            }
            written += 1;
        }
        self.next.set(start + written as usize);
        if !fetched.is_null() {
            // SAFETY: the caller gives where the number is written.
            unsafe { fetched.write(written) };
        }
        if written == count { S_OK } else { S_FALSE }
    }

    fn Skip(&self, count: u32) -> HRESULT {
        let next = self.next.get() + count as usize;
        self.next.set(next.min(self.objects.len()));
        if next <= self.objects.len() {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Reset(&self) -> windows::core::Result<()> {
        self.next.set(0);
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumVARIANT> {
        let copy = Objects {
            objects: self.objects.clone(),
            next: std::cell::Cell::new(self.next.get()),
        };
        Ok(copy.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn paths_and_parsing_names_are_told_apart() {
        assert!(is_path(r"C:\Users"));
        assert!(is_path(r"\\nas\share"));
        assert!(is_path("FILE:///C:/x"));
        assert!(!is_path("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
        assert!(!is_path("shell:Downloads"));
        assert_eq!(
            parsing_of(&Given::Text("file:///C:/a%20b".to_owned())),
            Some(r"C:\a%20b".to_owned())
        );
        assert_eq!(
            parsing_of(&Given::Text(
                " ::{645FF040-5081-101B-9F08-00AA002F954E} ".to_owned()
            )),
            Some("::{645FF040-5081-101B-9F08-00AA002F954E}".to_owned())
        );
        assert_eq!(parsing_of(&Given::Text("  ".to_owned())), None);
        assert_eq!(parsing_of(&Given::Nothing), None);
    }

    #[test]
    fn variants_are_read_by_their_kind() {
        let number = OwnedVariant::number(17);
        let text = OwnedVariant::text("C:\\x");
        let empty = OwnedVariant::empty();
        // SAFETY: the variants are alive and made with values of their kind.
        unsafe {
            assert_eq!(given(&raw const number.0), Given::Number(17));
            assert_eq!(given(&raw const text.0), Given::Text("C:\\x".to_owned()));
            assert_eq!(given(&raw const empty.0), Given::Nothing);
            assert_eq!(given(std::ptr::null()), Given::Nothing);
            // A variant passed by reference is followed.
            let by_reference = OwnedVariant::of(
                VARENUM(VT_BYREF.0 | VT_VARIANT.0),
                VARIANT_0_0_0 {
                    pvarVal: (&raw const text.0).cast_mut(),
                },
            );
            assert_eq!(
                given(&raw const by_reference.0),
                Given::Text("C:\\x".to_owned())
            );
            // It holds no value of its own to clear.
            std::mem::forget(by_reference);
        }
    }

    #[test]
    fn a_boolean_of_automation_is_all_bits() {
        assert_eq!(boolean(true).0, -1);
        assert_eq!(boolean(false).0, 0);
    }
}
