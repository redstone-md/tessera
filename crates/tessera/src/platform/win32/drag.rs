// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Dragging files out of a window of Tessera into Explorer, onto the
//! desktop or into any program, as Explorer drags them, and what a drop
//! does: copy, move or make a shortcut, as Explorer decides it from the
//! keys held and the drives.
//!
//! A drag out ([`DragOut`]) gives the drop target a data object of the
//! Shell (`SHCreateDataObject`), which carries the files as paths
//! (`CF_HDROP`) and as items of the Shell, with Explorer's own drag image,
//! and runs the drag with `SHDoDragDrop`. That call runs a loop of its own
//! until the drop, in which GPUI keeps drawing and handling input: it is
//! started from a task of GPUI, outside any update, so that the windows of
//! Tessera the pointer passes over can take their events meanwhile. While
//! the pointer stays inside the window it started in, a view should use
//! GPUI's own `on_drag` and `on_drop`, and start the drag out only when
//! the pointer leaves the window.
#![allow(unsafe_code)]
#![allow(
    dead_code,
    reason = "dragging out is for the file manager and the desktop, which do not use it yet"
)]

use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::{Component, Path, PathBuf, Prefix};

use anyhow::{Context as _, Result};
use gpui::{App, Task, Window};
use windows::Win32::Foundation::{HGLOBAL, HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    IDropSource, ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    CFSTR_LOGICALPERFORMEDDROPEFFECT, CLSID_DragDropHelper, DSH_ALLOWDROPDESCRIPTIONTEXT,
    DragQueryFileW, HDROP, IDragSourceHelper2, SHCreateDataObject, SHDoDragDrop,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, PostMessageW, WM_LBUTTONUP, WM_RBUTTONUP,
};
use windows::core::PCWSTR;

use super::NativeSurface;
use super::file_op::{FileOperation, LinkTarget, same_path};
use super::shell_item::ItemGroup;

/// The key state of a drag that says Alt is held; the bindings name it
/// only as a number of another type.
const MK_ALT: u32 = 0x20;

/// What a drop does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DropEffect {
    /// Nothing: the drop is refused.
    #[default]
    None,
    Copy,
    Move,
    /// A shortcut is made to what is dropped.
    Link,
}

impl DropEffect {
    pub(super) fn raw(self) -> DROPEFFECT {
        match self {
            Self::None => DROPEFFECT_NONE,
            Self::Copy => DROPEFFECT_COPY,
            Self::Move => DROPEFFECT_MOVE,
            Self::Link => DROPEFFECT_LINK,
        }
    }

    /// The effect a target answered; where several are set, the one a
    /// drop does first.
    pub(super) fn of_raw(raw: u32) -> Self {
        match raw {
            raw if raw & DROPEFFECT_MOVE.0 != 0 => Self::Move,
            raw if raw & DROPEFFECT_COPY.0 != 0 => Self::Copy,
            raw if raw & DROPEFFECT_LINK.0 != 0 => Self::Link,
            _ => Self::None,
        }
    }
}

/// What a drag allows a drop to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropEffects {
    pub copy: bool,
    pub move_: bool,
    pub link: bool,
}

impl DropEffects {
    /// Copy, move and link: what files on a disk allow.
    pub const ALL: Self = Self {
        copy: true,
        move_: true,
        link: true,
    };

    /// Only copy: what a file that exists only in the program it comes
    /// from allows, such as an attachment of a mail.
    pub const COPY: Self = Self {
        copy: true,
        move_: false,
        link: false,
    };

    /// Only link: what an item without a path allows, such as This PC.
    pub const LINK: Self = Self {
        copy: false,
        move_: false,
        link: true,
    };

    pub fn allows(self, effect: DropEffect) -> bool {
        match effect {
            DropEffect::None => true,
            DropEffect::Copy => self.copy,
            DropEffect::Move => self.move_,
            DropEffect::Link => self.link,
        }
    }

    /// What both allow.
    pub fn and(self, other: Self) -> Self {
        Self {
            copy: self.copy && other.copy,
            move_: self.move_ && other.move_,
            link: self.link && other.link,
        }
    }

    pub(super) fn raw(self) -> DROPEFFECT {
        let mut raw = 0;
        for (allowed, effect) in [
            (self.copy, DROPEFFECT_COPY),
            (self.move_, DROPEFFECT_MOVE),
            (self.link, DROPEFFECT_LINK),
        ] {
            if allowed {
                raw |= effect.0;
            }
        }
        DROPEFFECT(raw)
    }

    pub(super) fn of_raw(raw: u32) -> Self {
        Self {
            copy: raw & DROPEFFECT_COPY.0 != 0,
            move_: raw & DROPEFFECT_MOVE.0 != 0,
            link: raw & DROPEFFECT_LINK.0 != 0,
        }
    }
}

/// The keys held during a drag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropKeys {
    pub control: bool,
    pub shift: bool,
    pub alt: bool,
}

impl DropKeys {
    /// The keys of the key state OLE gives a drop target.
    pub(super) fn of_key_state(state: MODIFIERKEYS_FLAGS) -> Self {
        Self {
            control: state.0 & MK_CONTROL.0 != 0,
            shift: state.0 & MK_SHIFT.0 != 0,
            alt: state.0 & MK_ALT != 0,
        }
    }

    /// The effect the keys ask for, as in Explorer: Ctrl copies, Shift
    /// moves, Alt or Ctrl+Shift makes a shortcut; `None` where no key
    /// asks for one.
    fn wanted(self) -> Option<DropEffect> {
        match (self.control, self.shift, self.alt) {
            (_, _, true) | (true, true, _) => Some(DropEffect::Link),
            (true, false, false) => Some(DropEffect::Copy),
            (false, true, false) => Some(DropEffect::Move),
            (false, false, false) => None,
        }
    }
}

impl From<gpui::Modifiers> for DropKeys {
    fn from(modifiers: gpui::Modifiers) -> Self {
        Self {
            control: modifiers.control,
            shift: modifiers.shift,
            alt: modifiers.alt,
        }
    }
}

/// What dropping `sources` into `folder` does, as in Explorer: what the
/// keys ask for, or else a move on the same drive and a copy to another,
/// where the drag allows it. A folder is never dropped into itself or into
/// a folder inside it, and a move into the folder the items already lie in
/// does nothing. Sources without a path (`sources` empty) are copied or
/// linked, as the drag allows.
pub fn drop_effect(
    sources: &[PathBuf],
    folder: &Path,
    keys: DropKeys,
    allowed: DropEffects,
) -> DropEffect {
    let into_itself =
        { sources.iter() }.any(|source| same_path(source, folder) || is_within(folder, source));
    if into_itself {
        return DropEffect::None;
    }
    let same_drive =
        !sources.is_empty() && sources.iter().all(|source| same_volume(source, folder));
    let wanted = keys.wanted().unwrap_or(match same_drive {
        true => DropEffect::Move,
        false => DropEffect::Copy,
    });
    let in_place = !sources.is_empty() && { sources.iter() }.all(|source| {
        source
            .parent()
            .is_some_and(|parent| same_path(parent, folder))
    });
    if wanted == DropEffect::Move && in_place {
        return DropEffect::None;
    }
    if allowed.allows(wanted) {
        return wanted;
    }
    // Without a key asking, what the drag allows is done instead.
    match keys.wanted() {
        Some(_) => DropEffect::None,
        None => [DropEffect::Copy, DropEffect::Move, DropEffect::Link]
            .into_iter()
            .find(|&effect| allowed.allows(effect) && !(effect == DropEffect::Move && in_place))
            .unwrap_or(DropEffect::None),
    }
}

/// The work that drops `paths` into `folder` with `effect`.
pub fn drop_into(paths: Vec<PathBuf>, folder: &Path, effect: DropEffect) -> FileOperation {
    let operation = FileOperation::new();
    match effect {
        DropEffect::None => operation,
        DropEffect::Copy => operation.copy(paths, folder),
        DropEffect::Move => operation.move_(paths, folder),
        DropEffect::Link => paths.into_iter().fold(operation, |operation, path| {
            operation.link(LinkTarget::Path(path), folder)
        }),
    }
}

/// Whether `path` lies inside the folder `ancestor`, at any depth.
fn is_within(path: &Path, ancestor: &Path) -> bool {
    let key = |path: &Path| {
        let text = path.to_string_lossy().replace('/', "\\").to_lowercase();
        text.trim_end_matches('\\').to_owned()
    };
    let (path, ancestor) = (key(path), key(ancestor));
    path.len() > ancestor.len()
        && path.starts_with(&ancestor)
        && path[ancestor.len()..].starts_with('\\')
}

/// Whether two paths lie on the same volume: the same drive letter, or
/// the same share of a server.
fn same_volume(one: &Path, other: &Path) -> bool {
    match (volume_of(one), volume_of(other)) {
        (Some(one), Some(other)) => one == other,
        _ => false,
    }
}

/// The volume a path lies on, as a key that compares without regard to
/// case.
fn volume_of(path: &Path) -> Option<String> {
    let Component::Prefix(prefix) = path.components().next()? else {
        return None;
    };
    let lower = |text: &std::ffi::OsStr| text.to_string_lossy().to_lowercase();
    Some(match prefix.kind() {
        Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
            format!("{}:", char::from(letter).to_ascii_lowercase())
        }
        Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
            format!(r"\\{}\{}", lower(server), lower(share))
        }
        Prefix::DeviceNS(name) | Prefix::Verbatim(name) => lower(name),
    })
}

/// Which button of the pointer a drag is made with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DragButton {
    #[default]
    Left,
    /// The drop then offers a menu of what to do, at the target.
    Right,
}

/// A drag of files out of a window of Tessera.
#[derive(Clone, Debug)]
pub struct DragOut {
    paths: Vec<PathBuf>,
    allowed: DropEffects,
    button: DragButton,
}

impl DragOut {
    /// A drag of `paths`, which may lie in different folders, that allows
    /// copy, move and link.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            allowed: DropEffects::ALL,
            button: DragButton::Left,
        }
    }

    /// Allows the drop only `allowed`.
    pub fn allowing(mut self, allowed: DropEffects) -> Self {
        self.allowed = allowed;
        self
    }

    /// Says which button the drag is made with.
    pub fn with_button(mut self, button: DragButton) -> Self {
        self.button = button;
        self
    }

    /// Starts the drag from `window`, where the button is held, and
    /// returns a task that ends with what the drop did. The drag runs in a
    /// task of GPUI with nothing of GPUI borrowed, so that GPUI draws and
    /// handles the events of its windows during it; the task is to be
    /// detached or kept, not awaited inside an update.
    ///
    /// The release of the button that drops ends the drag inside OLE, and
    /// GPUI never sees it; it is given the window afterwards, so that GPUI
    /// does not take the button as still held.
    pub fn start(self, window: &Window, cx: &App) -> Task<Result<DropEffect, String>> {
        let hwnd = NativeSurface::of(window)
            .ok()
            .map(|surface| surface.hwnd.0 as isize);
        cx.spawn(async move |_| {
            let dropped = self.run(hwnd).map_err(|error| format!("{error:#}"));
            if let Some(window) = hwnd {
                release_button(window, self.button);
            }
            dropped
        })
    }

    /// Runs the drag on this thread, the thread of GPUI, which has OLE
    /// initialized.
    fn run(&self, window: Option<isize>) -> Result<DropEffect> {
        let group = ItemGroup::of_paths(&self.paths)?;
        let children = group.children();
        let parent = group.parent.as_ref().map(|parent| parent.as_ptr());
        // SAFETY: the identifier lists outlive the calls; the data object
        // and the helper are COM objects of this thread.
        unsafe {
            let data: IDataObject =
                SHCreateDataObject(parent, Some(&children), None::<&IDataObject>)
                    .context("the files cannot be dragged")?;
            // Explorer's picture of the items, with room for what the
            // target says it does.
            let helper: Result<IDragSourceHelper2, _> =
                CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER);
            if let Ok(helper) = helper {
                let _ = helper.SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT.0 as u32);
                let _ = helper.InitializeFromWindow(None, None, &data);
            }
            let owner = window.map(|window| HWND(window as *mut c_void));
            let effect = SHDoDragDrop(owner, &data, None::<&IDropSource>, self.allowed.raw())
                .context("the files could not be dragged")?;
            // A target that moves the files itself answers that nothing is
            // left for the source to do, and tells what it did here.
            let logical =
                format(CFSTR_LOGICALPERFORMEDDROPEFFECT).and_then(|format| dword(&data, format));
            Ok(DropEffect::of_raw(logical.unwrap_or(effect.0)))
        }
    }
}

/// Tells the window numbered `window` that `button` was released at the
/// pointer, as OLE took that release for the drop.
fn release_button(window: isize, button: DragButton) {
    let window = HWND(window as *mut c_void);
    let mut point = POINT::default();
    // SAFETY: the calls only read the pointer, turn it into the window's
    // coordinates and post a message, which fails for a window that is
    // gone.
    unsafe {
        if GetCursorPos(&raw mut point).is_err() {
            return;
        }
        let _ = ScreenToClient(window, &raw mut point);
        let message = match button {
            DragButton::Left => WM_LBUTTONUP,
            DragButton::Right => WM_RBUTTONUP,
        };
        let position = ((point.y as u16 as u32) << 16) | point.x as u16 as u32;
        let _ = PostMessageW(Some(window), message, WPARAM(0), LPARAM(position as isize));
    }
}

/// The number of the clipboard format named `name`.
pub(super) fn format(name: PCWSTR) -> Option<u16> {
    // SAFETY: the name is a NUL-terminated constant.
    let format = unsafe { RegisterClipboardFormatW(name) };
    u16::try_from(format).ok().filter(|&format| format != 0)
}

/// What a data object should be asked for: `format` in global memory, of
/// the item numbered `index` (-1 for all).
pub(super) fn request(format: u16, index: i32, tymed: i32) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: index,
        tymed: tymed as u32,
    }
}

/// The bytes `data` holds in `format` in global memory, if it has it so.
pub(super) fn global_bytes(data: &IDataObject, format: u16, index: i32) -> Option<Vec<u8>> {
    let request = request(format, index, TYMED_HGLOBAL.0);
    // SAFETY: the medium is released after the memory is copied out of
    // it while it is locked.
    unsafe {
        let mut medium = data.GetData(&raw const request).ok()?;
        let bytes = (medium.tymed == TYMED_HGLOBAL.0 as u32)
            .then(|| read_global(medium.u.hGlobal))
            .flatten();
        ReleaseStgMedium(&raw mut medium);
        bytes
    }
}

/// A copy of what lies in global memory.
pub(super) fn read_global(memory: HGLOBAL) -> Option<Vec<u8>> {
    // SAFETY: the memory is locked while it is read, within its size.
    unsafe {
        let size = GlobalSize(memory);
        let start = GlobalLock(memory).cast::<u8>();
        if start.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(start, size).to_vec();
        let _ = GlobalUnlock(memory);
        Some(bytes)
    }
}

/// The number `data` holds in `format`, such as a drop effect.
fn dword(data: &IDataObject, format: u16) -> Option<u32> {
    let bytes = global_bytes(data, format, -1)?;
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

/// The paths of the files `data` carries as `CF_HDROP`.
pub(super) fn dropped_paths(data: &IDataObject) -> Vec<PathBuf> {
    let request = request(CF_HDROP.0, -1, TYMED_HGLOBAL.0);
    // SAFETY: the list is read while the medium is held, and the medium
    // is released after.
    unsafe {
        let Ok(mut medium) = data.GetData(&raw const request) else {
            return Vec::new();
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let paths = (0..count)
            .filter_map(|index| {
                let length = DragQueryFileW(drop, index, None) as usize;
                let mut name = vec![0u16; length + 1];
                let written = DragQueryFileW(drop, index, Some(&mut name)) as usize;
                (written > 0).then(|| PathBuf::from(OsString::from_wide(&name[..written])))
            })
            .collect();
        ReleaseStgMedium(&raw mut medium);
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(texts: &[&str]) -> Vec<PathBuf> {
        texts.iter().map(PathBuf::from).collect()
    }

    const NO_KEYS: DropKeys = DropKeys {
        control: false,
        shift: false,
        alt: false,
    };

    #[test]
    fn a_drop_moves_on_the_same_drive_and_copies_to_another() {
        let sources = paths(&[r"C:\a\x.txt"]);
        let folder = Path::new(r"c:\b");
        assert_eq!(
            drop_effect(&sources, folder, NO_KEYS, DropEffects::ALL),
            DropEffect::Move
        );
        assert_eq!(
            drop_effect(&sources, Path::new(r"D:\b"), NO_KEYS, DropEffects::ALL),
            DropEffect::Copy
        );
        let share = paths(&[r"\\server\share\x.txt"]);
        assert_eq!(
            drop_effect(
                &share,
                Path::new(r"\\SERVER\Share\y"),
                NO_KEYS,
                DropEffects::ALL
            ),
            DropEffect::Move
        );
    }

    #[test]
    fn the_keys_choose_the_effect() {
        let sources = paths(&[r"C:\a\x.txt"]);
        let folder = Path::new(r"D:\b");
        let keys = |control, shift, alt| DropKeys {
            control,
            shift,
            alt,
        };
        let effect = |keys| drop_effect(&sources, folder, keys, DropEffects::ALL);
        assert_eq!(effect(keys(true, false, false)), DropEffect::Copy);
        assert_eq!(effect(keys(false, true, false)), DropEffect::Move);
        assert_eq!(effect(keys(true, true, false)), DropEffect::Link);
        assert_eq!(effect(keys(false, false, true)), DropEffect::Link);
        assert_eq!(
            drop_effect(
                &sources,
                folder,
                keys(false, true, false),
                DropEffects::COPY
            ),
            DropEffect::None,
            "a key asks for what the drag does not allow"
        );
    }

    #[test]
    fn a_folder_is_not_dropped_into_itself() {
        let sources = paths(&[r"C:\a"]);
        for folder in [r"C:\a", r"C:\A\b\c"] {
            assert_eq!(
                drop_effect(&sources, Path::new(folder), NO_KEYS, DropEffects::ALL),
                DropEffect::None
            );
        }
        assert_eq!(
            drop_effect(&sources, Path::new(r"C:\ab"), NO_KEYS, DropEffects::ALL),
            DropEffect::Move
        );
    }

    #[test]
    fn a_move_into_the_same_folder_does_nothing_but_a_copy_does() {
        let sources = paths(&[r"C:\a\x.txt"]);
        let folder = Path::new(r"C:\a");
        assert_eq!(
            drop_effect(&sources, folder, NO_KEYS, DropEffects::ALL),
            DropEffect::None
        );
        let control = DropKeys {
            control: true,
            ..NO_KEYS
        };
        assert_eq!(
            drop_effect(&sources, folder, control, DropEffects::ALL),
            DropEffect::Copy
        );
    }

    #[test]
    fn what_has_no_path_is_copied_or_linked_as_allowed() {
        let folder = Path::new(r"C:\a");
        assert_eq!(
            drop_effect(&[], folder, NO_KEYS, DropEffects::COPY),
            DropEffect::Copy
        );
        assert_eq!(
            drop_effect(&[], folder, NO_KEYS, DropEffects::LINK),
            DropEffect::Link
        );
    }

    #[test]
    fn effects_cross_to_windows_and_back() {
        let all = DropEffects::ALL;
        assert_eq!(DropEffects::of_raw(all.raw().0), all);
        assert_eq!(all.and(DropEffects::COPY), DropEffects::COPY);
        assert_eq!(
            DropEffect::of_raw(DropEffect::Link.raw().0),
            DropEffect::Link
        );
        assert_eq!(DropEffect::of_raw(0), DropEffect::None);
    }
}
