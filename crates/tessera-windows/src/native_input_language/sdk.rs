// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::layout::{self, LayoutRegistry};
use super::{Fetch, Identity, KEYBOARD_LAYOUT, LanguageNames, NativeCalls, NativeProfile};
use std::{marker::PhantomData, rc::Rc};
use windows::{
    Win32::{
        System::{
            Com::{
                CLSCTX_DISABLE_AAA, CLSCTX_INPROC_SERVER, COINIT, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize,
            },
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
                RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
            },
        },
        UI::{
            Input::KeyboardAndMouse::HKL,
            Shell::{
                SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, SHLoadIndirectString,
                ShellExecuteExW,
            },
            TextServices::{
                CLSID_TF_InputProcessorProfiles, CLSID_TF_ThreadMgr, IEnumTfInputProcessorProfiles,
                ITfInputProcessorProfileMgr, ITfInputProcessorProfiles, ITfThreadMgr,
                TF_INPUTPROCESSORPROFILE,
            },
        },
    },
    core::{GUID, Interface, PCWSTR, PWSTR},
};
use windows_sys::Win32::{
    Globalization::{
        GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SLOCALIZEDDISPLAYNAME, LOCALE_SNATIVELANGUAGENAME,
    },
    UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage, WM_QUIT,
    },
};

pub(super) struct WindowsCalls;

pub(super) struct Session {
    profiles: ITfInputProcessorProfiles,
    manager: ITfInputProcessorProfileMgr,
    _thread_bound: PhantomData<Rc<()>>,
}

/// Installed SDK returns a CoTaskMem allocation, not a Rust allocation. It is
/// wrapped immediately, including when the native HRESULT reports failure.
pub(super) struct Languages {
    pointer: *mut u16,
    count: usize,
}

impl AsRef<[u16]> for Languages {
    fn as_ref(&self) -> &[u16] {
        if self.count == 0 {
            return &[];
        }
        // SAFETY: GetLanguageList supplied count elements; construction rejected
        // null/nonzero and oversized output. The allocation remains owned here.
        unsafe { std::slice::from_raw_parts(self.pointer, self.count) }
    }
}

impl Drop for Languages {
    fn drop(&mut self) {
        // SAFETY: pointer is null or the exact allocation returned by TSF;
        // CoTaskMemFree accepts null. No allocator mixing or double ownership.
        unsafe { CoTaskMemFree(Some(self.pointer.cast())) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn decoded(buffer: &[u16], count: i32) -> Option<String> {
    let count = usize::try_from(count).ok()?;
    if count == 0 || count > buffer.len() || buffer[count - 1] != 0 {
        return None;
    }
    if buffer[..count - 1].contains(&0) {
        return None;
    }
    String::from_utf16(&buffer[..count - 1]).ok()
}

fn locale_field(locale: &[u16], field: u32) -> String {
    let mut buffer = [u16::MAX; 512];
    // SAFETY: validated NUL-terminated locale and bounded writable output.
    let count = unsafe {
        GetLocaleInfoEx(
            locale.as_ptr(),
            field,
            buffer.as_mut_ptr(),
            buffer.len() as i32,
        )
    };
    decoded(&buffer, count).unwrap_or_default()
}

impl NativeCalls for WindowsCalls {
    type Session = Session;
    type Languages = Languages;
    type Enumerator = IEnumTfInputProcessorProfiles;
    type ThreadManager = ITfThreadMgr;

    fn initialize(&self, model: i32) -> i32 {
        // SAFETY: request-local strict STA on the serialized native worker.
        unsafe { CoInitializeEx(None, COINIT(model)).0 }
    }

    fn uninitialize(&self) {
        // SAFETY: request guard balances S_OK/S_FALSE on the same thread, after
        // enumerators, BSTRs, thread manager and profile interfaces are released.
        unsafe { CoUninitialize() };
    }

    fn pump(&self) -> Result<(), u32> {
        // No idle apartment exists. Drain pending TSF messages at bounded request
        // checkpoints; COM itself pumps RPC during synchronous outgoing calls.
        // A flooded queue aborts rather than starving the bounded request queue.
        for _ in 0..256 {
            let mut message = MSG::default();
            // SAFETY: live MSG; current worker queue only, no generated input.
            if unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } == 0 {
                return Ok(());
            }
            if message.message == WM_QUIT {
                return Err(0x8007_04c7);
            }
            // SAFETY: dispatch only actual messages taken from this thread queue.
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        Err(0x8007_00aa)
    }

    fn session(&self) -> Result<Session, u32> {
        // SAFETY: live STA, fixed in-process class, no aggregation/remote server.
        let profiles: ITfInputProcessorProfiles = unsafe {
            CoCreateInstance(
                &CLSID_TF_InputProcessorProfiles,
                None,
                CLSCTX_INPROC_SERVER | CLSCTX_DISABLE_AAA,
            )
        }
        .map_err(|error| error.code().0 as u32)?;
        let manager = profiles.cast().map_err(|error| error.code().0 as u32)?;
        Ok(Session {
            profiles,
            manager,
            _thread_bound: PhantomData,
        })
    }

    fn languages(&self, session: &Session) -> Result<Languages, u32> {
        let mut pointer = std::ptr::null_mut();
        let mut count = 0;
        // SAFETY: correctly sized out parameters, live apartment-bound interface.
        let status = unsafe {
            (session.profiles.vtable().GetLanguageList)(
                session.profiles.as_raw(),
                &mut pointer,
                &mut count,
            )
            .0
        };
        let allocation = Languages {
            pointer,
            count: count as usize,
        };
        if status != 0 {
            return Err(status as u32);
        }
        if (allocation.count != 0 && allocation.pointer.is_null()) || allocation.count > 65_536 {
            return Err(0x8007_000d);
        }
        Ok(allocation)
    }

    fn names(&self, language: u16) -> LanguageNames {
        let mut locale = [u16::MAX; 85];
        // SAFETY: LANGID to default-sort LCID and bounded UTF-16 output buffer.
        let count = unsafe {
            LCIDToLocaleName(
                u32::from(language),
                locale.as_mut_ptr(),
                locale.len() as i32,
                0,
            )
        };
        let Some(code) = decoded(&locale, count) else {
            return LanguageNames::default();
        };
        LanguageNames {
            code,
            display: locale_field(&locale[..count as usize], LOCALE_SLOCALIZEDDISPLAYNAME),
            native: locale_field(&locale[..count as usize], LOCALE_SNATIVELANGUAGENAME),
        }
    }

    fn enumerate(&self, session: &Session, language: u16) -> Result<Self::Enumerator, u32> {
        // SAFETY: session and returned enumerator stay in this request's STA.
        unsafe { session.manager.EnumProfiles(language) }.map_err(|error| error.code().0 as u32)
    }

    fn next(&self, enumerator: &Self::Enumerator) -> Fetch {
        let mut row = TF_INPUTPROCESSORPROFILE::default();
        let mut fetched = 0;
        // SAFETY: pinned 0.62.2 vtable receives one initialized output slot and
        // a fetched count. Preserve raw HRESULT so S_FALSE is not erased.
        let status =
            unsafe { (enumerator.vtable().Next)(enumerator.as_raw(), 1, &mut row, &mut fetched).0 };
        Fetch {
            status,
            fetched,
            profile: NativeProfile {
                identity: Identity {
                    kind: row.dwProfileType,
                    language: row.langid,
                    class: row.clsid.to_u128(),
                    profile: row.guidProfile.to_u128(),
                    layout: row.hkl.0 as usize,
                },
                category: row.catid.to_u128(),
                flags: row.dwFlags,
            },
        }
    }

    fn description(&self, session: &Session, identity: Identity) -> String {
        if identity.kind == KEYBOARD_LAYOUT {
            return layout::description(&Registry, identity.layout);
        }
        // SAFETY: exact enabled TIP tuple copied from native enumeration.
        // Returned BSTR is owned and dropped before apartment teardown.
        unsafe {
            session.profiles.GetLanguageProfileDescription(
                &GUID::from_u128(identity.class),
                identity.language,
                &GUID::from_u128(identity.profile),
            )
        }
        .ok()
        .and_then(|value| String::from_utf16(&value).ok())
        .unwrap_or_default()
    }

    fn thread_manager(&self) -> Result<ITfThreadMgr, u32> {
        // SAFETY: fixed TSF class in this live STA; no object crosses threads.
        unsafe {
            CoCreateInstance(
                &CLSID_TF_ThreadMgr,
                None,
                CLSCTX_INPROC_SERVER | CLSCTX_DISABLE_AAA,
            )
        }
        .map_err(|error| error.code().0 as u32)
    }

    fn activate_manager(&self, manager: &ITfThreadMgr) -> Result<(), u32> {
        // SAFETY: same-thread activation is balanced exactly once by the guard.
        unsafe { manager.Activate() }
            .map(|_| ())
            .map_err(|error| error.code().0 as u32)
    }

    fn deactivate_manager(&self, manager: &ITfThreadMgr) -> Result<(), u32> {
        // SAFETY: guard owns exactly one successful same-thread activation.
        unsafe { manager.Deactivate() }.map_err(|error| error.code().0 as u32)
    }

    fn change_language(&self, session: &Session, language: u16) -> i32 {
        // SAFETY: active thread manager exists; raw result accepts only S_OK,
        // since ChangeCurrentLanguage documents no success-like no-change status.
        unsafe {
            (session.profiles.vtable().ChangeCurrentLanguage)(session.profiles.as_raw(), language).0
        }
    }

    fn activate(&self, session: &Session, identity: Identity, flags: u32) -> i32 {
        // SAFETY: pinned SDK vtable, exact freshly revalidated tuple, fixed
        // desktop scope. Do not use generated Result<()>: it erases S_FALSE,
        // which ActivateProfile documents as "profile not enabled".
        unsafe {
            (session.manager.vtable().ActivateProfile)(
                session.manager.as_raw(),
                identity.kind,
                identity.language,
                &GUID::from_u128(identity.class),
                &GUID::from_u128(identity.profile),
                HKL(identity.layout as *mut _),
                flags,
            )
            .0
        }
    }

    fn settings(&self, uri: &'static str, parameters: Option<&str>) -> Result<(), u32> {
        // The seam records operands, but production accepts only the fixed URI.
        if uri != super::SETTINGS_URI || parameters.is_some() {
            return Err(0x8007_0057);
        }
        let uri = wide(uri);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpFile: PCWSTR(uri.as_ptr()),
            nShow: 1,
            ..Default::default()
        };
        // SAFETY: fixed NUL-terminated Settings URI, null parameters/verb/path,
        // no interpreter; NOASYNC ensures request-local apartment is sufficient.
        unsafe { ShellExecuteExW(&mut info) }.map_err(|error| error.code().0 as u32)
    }
}

const LAYOUT_ROOT: &str = "SYSTEM\\CurrentControlSet\\Control\\Keyboard Layouts";
struct Registry;
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: uniquely owned registry handle from RegOpenKeyExW.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(path: &str) -> Result<Key, u32> {
    let path = wide(path);
    let mut handle = HKEY::default();
    // SAFETY: NUL-terminated trusted registry path, read-only access, live output.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut handle,
        )
    };
    if status.0 != 0 {
        return Err(status.0);
    }
    Ok(Key(handle))
}

impl LayoutRegistry for Registry {
    fn keys(&self) -> Result<Vec<String>, u32> {
        let root = open(LAYOUT_ROOT)?;
        let mut keys = Vec::new();
        for index in 0..16_384 {
            let mut buffer = [u16::MAX; 256];
            let mut count = buffer.len() as u32;
            // SAFETY: live read-only key and bounded writable subkey name buffer.
            let status = unsafe {
                RegEnumKeyExW(
                    root.0,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &mut count,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status.0 == 259 {
                return Ok(keys);
            }
            if status.0 != 0 {
                return Err(status.0);
            }
            let count = count as usize;
            if count > buffer.len() {
                return Err(13);
            }
            let key = String::from_utf16(&buffer[..count]).map_err(|_| 13u32)?;
            keys.push(key);
        }
        Err(234)
    }

    fn value(&self, key: &str, name: &str) -> Option<String> {
        if key.len() != 8 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let key = open(&format!("{LAYOUT_ROOT}\\{key}")).ok()?;
        let name = wide(name);
        let mut kind = REG_VALUE_TYPE::default();
        let mut bytes = 0;
        // SAFETY: read-only query with live count/type output; no data buffer.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut bytes),
            )
        };
        if status.0 != 0
            || (kind != REG_SZ && kind != REG_EXPAND_SZ)
            || !(2..=32_768).contains(&bytes)
            || bytes % 2 != 0
        {
            return None;
        }
        let mut buffer = vec![u16::MAX; bytes as usize / 2];
        let expected = bytes;
        // SAFETY: live u16 allocation is writable for exactly advertised bytes.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind),
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status.0 != 0 || bytes != expected || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
            return None;
        }
        decoded(&buffer, buffer.len() as i32)
    }

    fn indirect(&self, source: &str) -> Option<String> {
        if source.contains('\0') {
            return None;
        }
        let source = wide(source);
        let mut buffer = [u16::MAX; 4096];
        // SAFETY: registry resource reference, bounded UTF-16 output. This is a
        // label resolver, not process launch; no registry writes/enable actions.
        unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut buffer, None) }.ok()?;
        let end = buffer.iter().position(|&value| value == 0)?;
        String::from_utf16(&buffer[..end]).ok()
    }
}

#[cfg(test)]
#[path = "sdk_tests.rs"]
mod tests;
