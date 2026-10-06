// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Native `FOLDERID_AppsFolder` enumeration, launch, class-icon copy, and
//! the foreground query. All COM/shell FFI for the application surface lives
//! here; the portable wrappers are in `apps.rs`.
//!
//! Type seam: typed `windows` 0.62.2 COM interfaces and shell enumeration
//! only (with explicit `.0` conversions to `windows-sys` handles at the
//! boundary); existing `windows-sys` GDI/window/kernel functions elsewhere.
//! COM pointers never enter Send DTOs and never cross threads.

use std::mem::size_of;
use std::ptr::null_mut;
use std::slice;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Foundation::SIZE;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory,
    SEE_MASK_FLAG_NO_UI, SEE_MASK_IDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    SHGetKnownFolderItem, SHParseDisplayName, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_NORMALDISPLAY,
    SIIGBF_ICONONLY, ShellExecuteExW,
};
use windows::core::Interface;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Gdi::{
    BITMAP, CreateCompatibleDC as sys_CreateCompatibleDC, CreateDIBSection as sys_CreateDIBSection,
    DeleteDC as sys_DeleteDC, DeleteObject as sys_DeleteObject, GetDIBits as sys_GetDIBits,
    GetObjectW as sys_GetObjectW, HBITMAP as SYS_HBITMAP, HGDIOBJ as SYS_HGDIOBJ,
    SelectObject as sys_SelectObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DI_NORMAL, GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, GetForegroundWindow, GetIconInfo,
    GetWindowThreadProcessId, HICON, ICONINFO, IsWindow,
};

use crate::apps::{
    Application, ApplicationError, IconPixels, MAX_CATALOG_ENTRIES, MAX_DISPLAY_NAME_LEN,
    MAX_PARSING_NAME_LEN,
};

/// Encapsulated COM apartment lifetime for one native call.
///
/// `Owned` balances a successful `CoInitializeEx` with `CoUninitialize`;
/// `Inherited` means the thread already runs a different apartment
/// (`RPC_E_CHANGED_MODE`) and is used read-only without uninitializing.
enum Apartment {
    Owned,
    Inherited,
}

impl Apartment {
    fn enter() -> Result<Self, ApplicationError> {
        // SAFETY: thread-scoped COM initialization, no reserved parameter.
        let coinit = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if coinit.is_ok() {
            return Ok(Self::Owned);
        }
        if coinit == RPC_E_CHANGED_MODE {
            return Ok(Self::Inherited);
        }
        Err(ApplicationError::Windows {
            operation: "CoInitializeEx",
            code: coinit.0 as u32,
        })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if matches!(self, Self::Owned) {
            // SAFETY: balances our successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// Encapsulated `CoTaskMem` allocation (display-name strings, PIDLs).
struct CoTaskMem(*mut core::ffi::c_void);

impl Drop for CoTaskMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from a shell API documenting
            // CoTaskMemFree as its deallocator, freed exactly once.
            unsafe { CoTaskMemFree(Some(self.0)) };
        }
    }
}

/// Encapsulated GDI bitmap; freed with `DeleteObject` exactly once.
struct OwnedBitmap(SYS_HBITMAP);

impl Drop for OwnedBitmap {
    fn drop(&mut self) {
        // SAFETY: the HBITMAP was returned by GetImage/CreateDIBSection,
        // which document DeleteObject as the releaser.
        unsafe { sys_DeleteObject(self.0.cast()) };
    }
}

/// Restore selection before the owned bitmap drops; deleting a selected
/// GDI object fails and leaks it even if its DC is deleted afterward.
struct BitmapSelection {
    context: windows_sys::Win32::Graphics::Gdi::HDC,
    previous: SYS_HGDIOBJ,
}

impl Drop for BitmapSelection {
    fn drop(&mut self) {
        // SAFETY: the DC and previous object outlive this selection guard.
        unsafe { sys_SelectObject(self.context, self.previous) };
    }
}

/// Copies one bounded NUL-terminated UTF-16 shell string, rejecting empty
/// (and optionally blank) values. The scan window bounds any overread.
///
/// # Safety
/// `pointer` must reference at least `max_chars + 1` readable units or be
/// NUL-terminated earlier; shell `CoTaskMem` strings satisfy both.
unsafe fn bounded_string(
    pointer: *const u16,
    max_chars: usize,
    reject_blank: bool,
) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    while length < max_chars && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == 0 || unsafe { *pointer.add(length) } != 0 {
        return None;
    }
    let text = String::from_utf16(unsafe { slice::from_raw_parts(pointer, length) }).ok()?;
    if reject_blank && text.trim().is_empty() {
        return None;
    }
    Some(text)
}

/// Reads one display-name flavor from `item` into a bounded `String`.
fn display_name(
    item: &IShellItem,
    sigdn: windows::Win32::UI::Shell::SIGDN,
    max_chars: usize,
    reject_blank: bool,
) -> Option<String> {
    // SAFETY: item is live; PWSTR allocation is transferred to CoTaskMem.
    let raw = unsafe { item.GetDisplayName(sigdn) }.ok()?;
    let owned = CoTaskMem(raw.as_ptr().cast());
    // SAFETY: owned keeps the allocation alive; the string is NUL-terminated.
    unsafe { bounded_string(owned.0.cast(), max_chars, reject_blank) }
}

/// Converts a GDI bitmap into top-down premultiplied RGBA bytes.
///
/// One-pass: `GetObjectW` supplies dimensions, then `GetDIBits` fills a
/// forced 32-bpp top-down buffer, avoiding the header round-trip.
fn bitmap_rgba(bitmap: SYS_HBITMAP) -> Option<IconPixels> {
    if bitmap.is_null() {
        return None;
    }
    let mut header = BITMAP::default();
    // SAFETY: header is a valid writable BITMAP for the queried handle.
    if unsafe {
        sys_GetObjectW(
            bitmap.cast(),
            size_of::<BITMAP>() as i32,
            (&mut header as *mut BITMAP).cast(),
        )
    } == 0
    {
        return None;
    }
    let (width, height) = (header.bmWidth, header.bmHeight);
    const MAX_EDGE: i32 = IconPixels::MAX_EDGE as i32;
    if width <= 0 || height == 0 || width > MAX_EDGE || height.unsigned_abs() > MAX_EDGE as u32 {
        return None;
    }
    let rows = height.unsigned_abs() as usize;
    // SAFETY: a memory DC without a referencing window; deleted on all paths.
    let context = unsafe { sys_CreateCompatibleDC(std::ptr::null_mut()) };
    if context.is_null() {
        return None;
    }
    let result = (|| {
        let stride = (width as usize * 4).next_multiple_of(4);
        let mut pixels = vec![0u8; stride * rows];
        let mut info = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: request top-down rows regardless of source order.
                biHeight: -(rows as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: pixels is exactly `stride * rows`; GetDIBits writes at most
        // that much for a full-height 32-bpp top-down copy.
        let copied = unsafe {
            sys_GetDIBits(
                context,
                bitmap,
                0,
                rows as u32,
                pixels.as_mut_ptr().cast(),
                &mut info,
                windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
            )
        };
        if copied != rows as i32 {
            return None;
        }
        let mut rgba = Vec::with_capacity(width as usize * rows * 4);
        for row in 0..rows {
            // GetDIBits already converted to requested top-down row order.
            let base = row * stride;
            for pixel in 0..width as usize {
                // 32-bpp BI_RGB memory order is B, G, R, (A).
                let offset = base + pixel * 4;
                rgba.push(pixels[offset + 2]);
                rgba.push(pixels[offset + 1]);
                rgba.push(pixels[offset]);
                rgba.push(pixels[offset + 3]);
            }
        }
        if rgba.chunks_exact(4).all(|pixel| pixel[3] == 0) {
            return None;
        }
        IconPixels::new(width as u32, rows as u32, rgba)
    })();
    // SAFETY: this thread created the memory DC above.
    unsafe { sys_DeleteDC(context) };
    result
}

/// Best-effort 48×48 icon for one shell item; `None` skips the item image.
fn item_icon(item: &IShellItem) -> Option<IconPixels> {
    // SAFETY: item is live; the cast is QueryInterface-based, not a handler
    // bind, and the returned HBITMAP is caller-owned.
    let factory: IShellItemImageFactory = item.cast().ok()?;
    // SAFETY: factory is live; GetImage is documented icon-only at this size.
    let bitmap = unsafe { factory.GetImage(SIZE { cx: 48, cy: 48 }, SIIGBF_ICONONLY) }.ok()?;
    let bitmap = OwnedBitmap(bitmap.0);
    bitmap_rgba(bitmap.0)
}

/// Per-item conversion: both names bounded and nonblank, icon best-effort.
fn item_application(item: &IShellItem) -> Option<Application> {
    let name = display_name(item, SIGDN_NORMALDISPLAY, MAX_DISPLAY_NAME_LEN, true)?;
    let id = display_name(
        item,
        SIGDN_DESKTOPABSOLUTEPARSING,
        MAX_PARSING_NAME_LEN,
        false,
    )?;
    if id.trim().is_empty() {
        return None;
    }
    Some(Application::new(id, name, item_icon(item)))
}

/// Enumerates the whole AppsFolder catalog; whole-catalog failures are
/// returned, per-item failures skipped. Results are deduped and sorted.
pub(crate) fn catalog() -> Result<Vec<Application>, ApplicationError> {
    let _apartment = Apartment::enter()?;
    // SAFETY: the GUID is a documented constant; the interface is released
    // by RAII when `folder` drops.
    let folder: IShellItem = unsafe {
        SHGetKnownFolderItem(
            &FOLDERID_AppsFolder,
            windows::Win32::UI::Shell::KF_FLAG_DEFAULT,
            None,
        )
    }
    .map_err(|code| ApplicationError::Windows {
        operation: "SHGetKnownFolderItem",
        code: code.code().0 as u32,
    })?;
    // SAFETY: folder is live; BHID_EnumItems is the documented enumerator.
    let items: IEnumShellItems = unsafe {
        folder.BindToHandler(
            None::<&windows::Win32::System::Com::IBindCtx>,
            &BHID_EnumItems,
        )
    }
    .map_err(|code| ApplicationError::Windows {
        operation: "BindToHandler(BHID_EnumItems)",
        code: code.code().0 as u32,
    })?;

    let mut applications = Vec::new();
    let mut batch = [const { None }; 16];
    loop {
        // SAFETY: items is live; batch is a valid out-slice of interfaces.
        let mut fetched = 0u32;
        unsafe { items.Next(&mut batch, Some(&mut fetched)) }.map_err(|code| {
            ApplicationError::Windows {
                operation: "IEnumShellItems::Next",
                code: code.code().0 as u32,
            }
        })?;
        if fetched == 0 {
            break;
        }
        for shell_item in batch[..fetched as usize].iter_mut() {
            let Some(item) = shell_item.take() else {
                continue;
            };
            if let Some(application) = item_application(&item) {
                applications.push(application);
                if applications.len() >= MAX_CATALOG_ENTRIES {
                    return Ok(sorted_deduped(applications));
                }
            }
        }
        if (fetched as usize) < batch.len() {
            break;
        }
    }
    Ok(sorted_deduped(applications))
}

/// Case-insensitive dedupe by parsing name, then case-insensitive name sort.
fn sorted_deduped(mut applications: Vec<Application>) -> Vec<Application> {
    applications.sort_by(|a, b| {
        a.id()
            .to_uppercase()
            .cmp(&b.id().to_uppercase())
            .then_with(|| a.name().to_uppercase().cmp(&b.name().to_uppercase()))
    });
    applications.dedup_by(|a, b| a.id().eq_ignore_ascii_case(b.id()));
    applications.sort_by(|a, b| {
        a.name()
            .to_uppercase()
            .cmp(&b.name().to_uppercase())
            .then_with(|| a.id().cmp(b.id()))
    });
    applications
}

/// Launches a freshly enumerated application through its trusted parsing
/// name, re-parsed by the shell into a PIDL.
pub(crate) fn launch(application: &Application) -> Result<(), ApplicationError> {
    let _apartment = Apartment::enter()?;
    // The caller already validated the id; encode NUL-terminated.
    let mut id_utf16: Vec<u16> = application.id().encode_utf16().collect();
    id_utf16.push(0);
    // SAFETY: parses only a trusted enumerated qualified name; the PIDL is
    // caller-freed below via CoTaskMem.
    let mut pidl: *mut windows::Win32::UI::Shell::Common::ITEMIDLIST = null_mut();
    unsafe {
        SHParseDisplayName(
            windows::core::PCWSTR::from_raw(id_utf16.as_ptr()),
            None::<&windows::Win32::System::Com::IBindCtx>,
            &mut pidl,
            0,
            None,
        )
    }
    .map_err(|code| ApplicationError::Windows {
        operation: "SHParseDisplayName",
        code: code.code().0 as u32,
    })?;
    let owned_pidl = CoTaskMem(pidl.cast());
    // SAFETY: the PIDL remains owned while the execute call consumes it.
    execute_pidl(owned_pidl.0)
}

/// `SW_SHOWNORMAL` (1): launch the target's default presentation.
const SW_SHOWNORMAL_RAW: i32 = 1;

/// Keep the COM apartment alive until shell dispatch completes. Third-party
/// shell handlers can still block; supervised mode detects a stalled UI.
/// `SEE_MASK_FLAG_NO_UI` keeps dispatch errors in our presentation.
fn execute_pidl(pidl: *mut core::ffi::c_void) -> Result<(), ApplicationError> {
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_IDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpIDList: pidl,
        nShow: SW_SHOWNORMAL_RAW,
        ..Default::default()
    };
    // SAFETY: info and the PIDL outlive the documented IDList execute call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|code| ApplicationError::Windows {
        operation: "ShellExecuteExW",
        code: code.code().0 as u32,
    })
}

/// Raw handle of the current foreground window, `None` when none exists.
pub(crate) fn foreground_window_id() -> Option<tessera_core::WindowId> {
    // SAFETY: pure query with no parameters.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_null()).then(|| tessera_core::WindowId::new(hwnd as usize as u64))
}

/// Revalidates the recorded target identity and copies its shared class icon
/// into bounded premultiplied RGBA pixels. The shared icon is never
/// destroyed; everything drawn from it is owned and released locally.
pub(crate) fn window_icon(target: crate::ActivationTarget) -> Option<IconPixels> {
    let raw = usize::try_from(target.window_id().value()).ok()?;
    let hwnd = raw as HWND;
    // SAFETY: user32 validates transient handles; these queries never send
    // synchronous messages to the target window.
    if unsafe { IsWindow(hwnd) } == 0 {
        return None;
    }
    let mut process_id = 0;
    // SAFETY: process_id is a writable DWORD output.
    if unsafe { GetWindowThreadProcessId(hwnd, &mut process_id) } == 0
        || process_id != target.process_id()
    {
        return None;
    }
    // Class-icon query: shared, process-owned icons; try the standard then
    // the small variant. Zero means none exists (query, not last-error).
    let class_icon = unsafe { GetClassLongPtrW(hwnd, GCLP_HICON) };
    let small_icon = unsafe { GetClassLongPtrW(hwnd, GCLP_HICONSM) };
    if class_icon == 0 && small_icon == 0 {
        return None;
    }
    icon_from_handle(
        (if class_icon != 0 {
            class_icon
        } else {
            small_icon
        }) as HICON,
    )
}

/// Icon dimension bound shared by both copy paths.
const MAX_ICON_EDGE: i32 = IconPixels::MAX_EDGE as i32;

/// Draws one icon into an owned 32-bpp DIB section with `DrawIconEx`.
///
/// The shared source icon is only ever read; the destination bitmap, DC, and
/// bits are owned and released locally on every path.
fn icon_from_handle(icon: HICON) -> Option<IconPixels> {
    if icon.is_null() {
        return None;
    }
    let (width, height) = width_height_from_icon(icon)?;
    if width <= 0 || height <= 0 || width > MAX_ICON_EDGE || height > MAX_ICON_EDGE {
        return None;
    }
    // SAFETY: a memory DC without a referencing window; deleted on all paths.
    let context = unsafe { sys_CreateCompatibleDC(std::ptr::null_mut()) };
    if context.is_null() {
        return None;
    }
    let result = (|| {
        let info = windows_sys::Win32::Graphics::Gdi::BITMAPINFO {
            bmiHeader: windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER {
                biSize: size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: windows_sys::Win32::Graphics::Gdi::BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: CreateDIBSection allocates `height` DWORD-aligned 32-bpp
        // rows; the bits pointer is valid until the section is deleted.
        let mut bits: *mut core::ffi::c_void = null_mut();
        let section = unsafe {
            sys_CreateDIBSection(
                context,
                &info,
                windows_sys::Win32::Graphics::Gdi::DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if section.is_null() {
            return None;
        }
        let section = OwnedBitmap(section);
        if bits.is_null() {
            return None;
        }
        // SAFETY: bounded 32-bpp allocation; initialize transparent pixels
        // before mask/alpha drawing so untouched pixels cannot show garbage.
        unsafe {
            std::ptr::write_bytes(bits.cast::<u8>(), 0, width as usize * height as usize * 4)
        };
        // SAFETY: select our own section into our own memory DC.
        let previous = unsafe { sys_SelectObject(context, section.0.cast()) };
        if previous.is_null() || previous == (-1isize as SYS_HGDIOBJ) {
            return None;
        }
        let _selection = BitmapSelection { context, previous };
        // SAFETY: DrawIconEx only reads the source icon and writes our DC;
        // DI_NORMAL draws both mask and color without a brush.
        let drawn = unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DrawIconEx(
                context,
                0,
                0,
                icon,
                width,
                height,
                0,
                std::ptr::null_mut(),
                DI_NORMAL,
            )
        } != 0;
        // SAFETY: GDI may defer writes; flush before reading the bits.
        unsafe { windows_sys::Win32::Graphics::Gdi::GdiFlush() };
        if !drawn {
            return None;
        }
        // SAFETY: read exactly `width * height` premultiplied pixels.
        let bytes = unsafe {
            slice::from_raw_parts(bits.cast::<u8>(), width as usize * height as usize * 4)
        };
        // GDI DIBs are BGRA. Older mask-only icons may leave every alpha
        // byte zero; a labeled control is safer than an invisible image.
        if bytes.chunks_exact(4).all(|pixel| pixel[3] == 0) {
            return None;
        }
        let rgba = bytes
            .chunks_exact(4)
            .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
            .collect();
        IconPixels::new(width as u32, height as u32, rgba)
    })();
    // SAFETY: this thread created the memory DC above.
    unsafe { sys_DeleteDC(context) };
    result
}

/// Resolves an icon's dimensions from its `GetIconInfo` color bitmap via
/// `GetObjectW`. Both member bitmaps are owned by `GetIconInfo` and deleted
/// exactly once here.
fn width_height_from_icon(icon: HICON) -> Option<(i32, i32)> {
    let mut info = ICONINFO {
        fIcon: 1,
        ..Default::default()
    };
    // SAFETY: info is a valid writable ICONINFO for the queried icon.
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    let color = OwnedBitmap(info.hbmColor);
    let _mask = OwnedBitmap(info.hbmMask);
    if color.0.is_null() {
        return None;
    }
    let mut header = BITMAP::default();
    // SAFETY: header is a valid writable BITMAP for the queried handle.
    if unsafe {
        sys_GetObjectW(
            color.0.cast(),
            size_of::<BITMAP>() as i32,
            (&mut header as *mut BITMAP).cast(),
        )
    } == 0
    {
        return None;
    }
    Some((header.bmWidth, header.bmHeight))
}

#[cfg(test)]
#[path = "native_apps_tests.rs"]
mod tests;
