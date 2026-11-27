// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Dropping into a window of Tessera what is dragged from Explorer, from
//! the desktop or from any program, as Explorer takes it.
//!
//! GPUI's own drop target takes only paths, always answers "copy" and
//! cannot refuse a drop over a row that is not a folder. A [`DropTarget`]
//! takes its place on one window (`RevokeDragDrop`, then
//! `RegisterDragDrop`): the effect follows the keys held and the drives
//! ([`drop_effect`](super::drop_effect)), the cursor and Explorer's drag
//! image say so (`IDropTargetHelper`), and only the places the view lists
//! as [`DropZones`] take a drop: the rows of folders, the background of the
//! folder shown. It takes:
//!
//! - files and folders, which are copied, moved or linked with a
//!   [`FileOperation`](super::FileOperation), into the [`UndoJournal`](super::UndoJournal);
//! - items of the Shell without a path, such as This PC or an item of the
//!   Control Panel, of which shortcuts are made;
//! - files that only exist in the program they come from (an attachment
//!   of a mail, a picture or a link from a browser), which are saved into
//!   the folder under free names.
//!
//! What happens is told on a channel as [`DropEvent`]s, for the view to
//! show where a drop would go and to read the folder after it.
//!
//! The view writes its zones in the coordinates of the window, as GPUI
//! lays them out, whenever it draws; the drop target reads them while
//! something is dragged over the window. OLE calls the drop target on the
//! thread of GPUI, from its message loop, outside any update.
//!
//! Once a window has a [`DropTarget`], GPUI's own drop events (`on_drop`
//! of external paths) no longer arrive there, also after it is dropped.
#![allow(unsafe_code)]
#![allow(
    dead_code,
    reason = "taking drops is for the file manager and the desktop, which do not use it yet"
)]

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use async_channel::{Receiver, Sender};
use gpui::{Bounds, Pixels, Point, Window, point, px};
use windows::Win32::Foundation::{HWND, POINT, POINTL, S_OK};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::StructuredStorage::StgCreateDocfile;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject, STGC_DEFAULT, STGM_CREATE, STGM_READWRITE,
    STGM_SHARE_EXCLUSIVE, TYMED_HGLOBAL, TYMED_ISTORAGE, TYMED_ISTREAM,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, CFSTR_SHELLIDLIST, CLSID_DragDropHelper,
    FD_ATTRIBUTES, FILEDESCRIPTORW, IDataObjectAsyncCapability, IDropTargetHelper,
};
use windows::core::{AgileReference, Interface, Ref, implement};

use super::NativeSurface;
use super::ShellThread;
use super::drag::{
    DropEffect, DropEffects, DropKeys, drop_effect, drop_into, dropped_paths, format, global_bytes,
    read_global, request,
};
use super::file_op::{
    FileDeed, FileOperation, LinkTarget, Made, Outcome, UndoJournal, unique_name,
};
use super::shell_item::{ShellId, id_list_length};

/// A place in a window that takes a drop: what is dropped there goes into
/// `folder`.
#[derive(Clone, Debug, PartialEq)]
pub struct DropZone {
    /// Where it is, in the coordinates of the window.
    pub bounds: Bounds<Pixels>,
    pub folder: PathBuf,
}

/// The places of a window that take a drop, written by its view as it
/// draws and read by its [`DropTarget`]. A later zone lies above an
/// earlier one: a row above the background of the list.
#[derive(Clone, Debug, Default)]
pub struct DropZones(Rc<RefCell<Vec<DropZone>>>);

impl DropZones {
    pub fn new() -> Self {
        Self::default()
    }

    /// Puts `zones` in place of the zones there were.
    pub fn set(&self, zones: Vec<DropZone>) {
        if let Ok(mut held) = self.0.try_borrow_mut() {
            *held = zones;
        }
    }

    /// Adds a zone above the others.
    pub fn push(&self, zone: DropZone) {
        if let Ok(mut held) = self.0.try_borrow_mut() {
            held.push(zone);
        }
    }

    pub fn clear(&self) {
        self.set(Vec::new());
    }

    /// The topmost zone at `at`, with its place in the list.
    fn hit(&self, at: Point<Pixels>) -> Option<(usize, PathBuf)> {
        let zones = self.0.try_borrow().ok()?;
        top_zone(&zones, at)
    }
}

/// The topmost of `zones` at `at`: the last one listed that holds it.
fn top_zone(zones: &[DropZone], at: Point<Pixels>) -> Option<(usize, PathBuf)> {
    { zones.iter().enumerate().rev() }
        .find(|(_, zone)| zone.bounds.contains(&at))
        .map(|(index, zone)| (index, zone.folder.clone()))
}

/// What happens to a drag over the window.
#[derive(Clone, Debug)]
pub enum DropEvent {
    /// Something is dragged over the window: over the zone at this place
    /// in the list, or none, and what a drop would do there. It is told
    /// when either changes.
    Over {
        zone: Option<usize>,
        effect: DropEffect,
    },
    /// The drag left the window, or was dropped where nothing takes it.
    Left,
    /// It was dropped into `folder`, and the work on it started.
    Dropped { folder: PathBuf, effect: DropEffect },
    /// The work of a drop into `folder` is over.
    Done { folder: PathBuf, outcome: Outcome },
}

/// Tessera's drop target on a window, in place of GPUI's, until this is
/// dropped.
#[derive(Debug)]
pub struct DropTarget {
    window: isize,
}

impl DropTarget {
    /// Takes the drops into `window` over the places `zones` lists, and
    /// tells of them on the channel returned. It is to be called on the
    /// thread of GPUI, which owns the window.
    pub fn register(window: &Window, zones: DropZones) -> Result<(Self, Receiver<DropEvent>)> {
        let hwnd = NativeSurface::of(window)?.hwnd;
        let (events, told) = async_channel::unbounded();
        // SAFETY: the helper is created on this thread, which has OLE
        // initialized, as GPUI does it.
        let helper: Option<IDropTargetHelper> =
            unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        let target: IDropTarget = Target {
            window: hwnd,
            zones,
            helper,
            events,
            drag: RefCell::new(None),
            shown: Cell::new(None),
        }
        .into();
        // SAFETY: the window belongs to this thread; OLE keeps the target
        // until it is revoked, which `Drop` does.
        unsafe {
            let _ = RevokeDragDrop(hwnd);
            RegisterDragDrop(hwnd, &target).context("the window takes no drops")?;
        }
        Ok((
            Self {
                window: hwnd.0 as isize,
            },
            told,
        ))
    }
}

impl Drop for DropTarget {
    fn drop(&mut self) {
        // SAFETY: revoking fails without harm for a window that is gone.
        let _ = unsafe { RevokeDragDrop(HWND(self.window as *mut c_void)) };
    }
}

/// What a drag carries, as the drop target takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Payload {
    /// Files and folders with paths.
    Files,
    /// Items of the Shell without paths, which get shortcuts.
    Items,
    /// Files that exist only in the program they come from.
    Virtual,
}

/// The drag over the window.
struct Dragged {
    data: IDataObject,
    payload: Payload,
    /// The paths dragged, for files.
    sources: Vec<PathBuf>,
    allowed: DropEffects,
}

impl Dragged {
    /// What `data` carries that the drop target takes, with what its
    /// source allows (`allowed`, as OLE gives it); `None` where it carries
    /// nothing that is taken.
    fn of(data: IDataObject, allowed: u32) -> Option<Self> {
        let offered = |format: u16| {
            let request = request(format, -1, TYMED_HGLOBAL.0);
            // SAFETY: the data object is alive; the call only asks.
            unsafe { data.QueryGetData(&raw const request) == S_OK }
        };
        let allowed = DropEffects::of_raw(allowed);
        let (payload, allowed) = if offered(CF_HDROP.0) {
            (Payload::Files, allowed)
        } else if format(CFSTR_SHELLIDLIST).is_some_and(offered) {
            (Payload::Items, allowed.and(DropEffects::LINK))
        } else if format(CFSTR_FILEDESCRIPTORW).is_some_and(offered) {
            (Payload::Virtual, allowed.and(DropEffects::COPY))
        } else {
            return None;
        };
        let sources = match payload {
            Payload::Files => dropped_paths(&data),
            _ => Vec::new(),
        };
        Some(Self {
            data,
            payload,
            sources,
            allowed,
        })
    }
}

#[implement(IDropTarget)]
struct Target {
    window: HWND,
    zones: DropZones,
    helper: Option<IDropTargetHelper>,
    events: Sender<DropEvent>,
    drag: RefCell<Option<Dragged>>,
    /// The zone and the effect last told.
    shown: Cell<Option<(Option<usize>, DropEffect)>>,
}

impl Target {
    /// The point of the screen `at` in the coordinates of the window, as
    /// GPUI lays it out.
    fn local(&self, at: &POINTL) -> Point<Pixels> {
        let mut client = POINT { x: at.x, y: at.y };
        // SAFETY: the calls only read the window's place and resolution.
        let dpi = unsafe {
            let _ = ScreenToClient(self.window, &raw mut client);
            GetDpiForWindow(self.window)
        };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        point(px(client.x as f32 / scale), px(client.y as f32 / scale))
    }

    /// The zone at `at` and what a drop of the drag there with `keys`
    /// does.
    fn aim(&self, keys: MODIFIERKEYS_FLAGS, at: &POINTL) -> (Option<(usize, PathBuf)>, DropEffect) {
        let drag = self.drag.borrow();
        let Some(drag) = drag.as_ref() else {
            return (None, DropEffect::None);
        };
        let zone = self.zones.hit(self.local(at));
        let effect = zone.as_ref().map_or(DropEffect::None, |(_, folder)| {
            drop_effect(
                &drag.sources,
                folder,
                DropKeys::of_key_state(keys),
                drag.allowed,
            )
        });
        (zone, effect)
    }

    /// Tells the view where the drag is, where that changed.
    fn tell_over(&self, zone: Option<usize>, effect: DropEffect) {
        if self.shown.replace(Some((zone, effect))) != Some((zone, effect)) {
            self.events.try_send(DropEvent::Over { zone, effect }).ok();
        }
    }

    /// The drag is over: tells the view so where it was told of it.
    fn tell_left(&self) {
        if self.shown.take().is_some() {
            self.events.try_send(DropEvent::Left).ok();
        }
    }

    /// Does the drop of `drag` into `folder` with `effect`; what OLE is to
    /// tell its source was done.
    fn perform(&self, drag: Dragged, folder: PathBuf, effect: DropEffect) -> DROPEFFECT {
        self.events
            .try_send(DropEvent::Dropped {
                folder: folder.clone(),
                effect,
            })
            .ok();
        let events = self.events.clone();
        let done = {
            let folder = folder.clone();
            move |outcome: Outcome| {
                events.try_send(DropEvent::Done { folder, outcome }).ok();
            }
        };
        match drag.payload {
            Payload::Files => {
                drop_into(drag.sources, &folder, effect).then(done);
                // The files are moved here, after the drop returns: the
                // source is told that nothing is left for it to do, so
                // that it deletes nothing itself.
                match effect {
                    DropEffect::Move => DROPEFFECT_NONE,
                    effect => effect.raw(),
                }
            }
            Payload::Items => {
                let items = { format(CFSTR_SHELLIDLIST) }
                    .and_then(|format| global_bytes(&drag.data, format, -1))
                    .map(|bytes| items_of_id_array(&bytes))
                    .unwrap_or_default();
                let operation = items
                    .into_iter()
                    .fold(FileOperation::new(), |operation, item| {
                        operation.link(LinkTarget::Item(item), &folder)
                    });
                operation.then(done);
                effect.raw()
            }
            Payload::Virtual => {
                save_virtual_later(drag.data, folder, done);
                DROPEFFECT_COPY
            }
        }
    }
}

#[allow(non_snake_case, reason = "the names of the methods of COM")]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let data = data.ok()?;
        // SAFETY: OLE gives a valid pointer to the effects the source
        // allows, which the answer is written over.
        let allowed = unsafe { (*effect).0 };
        *self.drag.borrow_mut() = Dragged::of(data.clone(), allowed);
        let (zone, chosen) = self.aim(keys, at);
        // SAFETY: as above; the helper draws the drag image over the
        // window, at the point of the screen.
        unsafe {
            *effect = chosen.raw();
            if let Some(helper) = &self.helper {
                let screen = POINT { x: at.x, y: at.y };
                let _ = helper.DragEnter(self.window, data, &raw const screen, chosen.raw());
            }
        }
        self.tell_over(zone.map(|(index, _)| index), chosen);
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let (zone, chosen) = self.aim(keys, at);
        // SAFETY: OLE gives a valid pointer for the answer.
        unsafe {
            *effect = chosen.raw();
            if let Some(helper) = &self.helper {
                let screen = POINT { x: at.x, y: at.y };
                let _ = helper.DragOver(&raw const screen, chosen.raw());
            }
        }
        self.tell_over(zone.map(|(index, _)| index), chosen);
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            // SAFETY: the helper only takes its drag image away.
            let _ = unsafe { helper.DragLeave() };
        }
        self.drag.borrow_mut().take();
        self.tell_left();
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<'_, IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        at: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let data = data.ok()?;
        if self.drag.borrow().is_none() {
            // SAFETY: OLE gives a valid pointer to the allowed effects.
            let allowed = unsafe { (*effect).0 };
            *self.drag.borrow_mut() = Dragged::of(data.clone(), allowed);
        }
        let (zone, chosen) = self.aim(keys, at);
        if let Some(helper) = &self.helper {
            let screen = POINT { x: at.x, y: at.y };
            // SAFETY: the helper takes its drag image away.
            let _ = unsafe { helper.Drop(data, &raw const screen, chosen.raw()) };
        }
        let drag = self.drag.borrow_mut().take();
        self.shown.set(None);
        let answer = match (drag, zone, chosen) {
            (Some(drag), Some((_, folder)), chosen) if chosen != DropEffect::None => {
                self.perform(drag, folder, chosen)
            }
            _ => {
                self.events.try_send(DropEvent::Left).ok();
                DROPEFFECT_NONE
            }
        };
        // SAFETY: OLE gives a valid pointer for the answer.
        unsafe { *effect = answer };
        Ok(())
    }
}

/// The absolute identifier lists in an array of them as the Shell hands
/// it over (`CIDA`): their number, the places of the parent folder and of
/// each item, then the lists themselves.
fn items_of_id_array(bytes: &[u8]) -> Vec<ShellId> {
    let word = |at: usize| -> Option<usize> {
        let word = bytes.get(at..at + 4)?;
        Some(u32::from_le_bytes(word.try_into().ok()?) as usize)
    };
    let list_at = |offset: usize| -> Option<&[u8]> {
        let rest = bytes.get(offset..)?;
        Some(&rest[..id_list_length(rest)?])
    };
    let Some(count) = word(0) else {
        return Vec::new();
    };
    let Some(parent) = word(4).and_then(list_at) else {
        return Vec::new();
    };
    // The parent's list without its ending, then the item's with its own.
    let parent = &parent[..parent.len() - 2];
    (1..=count.min(bytes.len() / 4))
        .filter_map(|index| {
            let child = list_at(word(4 + index * 4)?)?;
            ShellId::from_bytes([parent, child].concat())
        })
        .collect()
}

/// A file in a drag that exists only in the program it comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct VirtualFile {
    /// Its name, which may hold folders: `Album\Photo.jpg`.
    name: String,
    folder: bool,
}

/// The files that `data` describes (`FileGroupDescriptorW`).
fn virtual_files(data: &IDataObject) -> Vec<VirtualFile> {
    let Some(bytes) =
        format(CFSTR_FILEDESCRIPTORW).and_then(|format| global_bytes(data, format, -1))
    else {
        return Vec::new();
    };
    let Some(count) = bytes
        .get(..4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
    else {
        return Vec::new();
    };
    let size = size_of::<FILEDESCRIPTORW>();
    (0..count as usize)
        .map_while(|index| {
            let start = 4 + index * size;
            let slice = bytes.get(start..start + size)?;
            // SAFETY: the slice holds a whole descriptor, which is read
            // without regard to alignment, as it is packed.
            let descriptor =
                unsafe { std::ptr::read_unaligned(slice.as_ptr().cast::<FILEDESCRIPTORW>()) };
            let name = descriptor.cFileName;
            let length = name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(name.len());
            let has_attributes = descriptor.dwFlags & FD_ATTRIBUTES.0 as u32 != 0;
            let attributes = descriptor.dwFileAttributes;
            Some(VirtualFile {
                name: String::from_utf16_lossy(&name[..length]),
                folder: has_attributes && attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
            })
        })
        .collect()
}

/// A name a program gives a dragged file, as a path below the folder it
/// is dropped into; `None` where it would leave that folder or cannot be
/// a name of Windows.
fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in name.split(['\\', '/']).filter(|part| !part.is_empty()) {
        let bad = |character: char| "<>:\"|?*".contains(character) || character.is_control();
        if part == "." || part == ".." || part.chars().any(bad) || part.ends_with(['.', ' ']) {
            return None;
        }
        path.push(part);
    }
    match path
        .components()
        .all(|part| matches!(part, Component::Normal(_)))
    {
        true if path.components().next().is_some() => Some(path),
        _ => None,
    }
}

/// Where each of `files` goes below the folder dropped into: each first
/// part that `taken` says is taken, or that an earlier file took, gets a
/// free name, and the parts below a folder go with it. `None` for a name
/// that is refused.
fn plan_virtual(files: &[VirtualFile], taken: impl Fn(&str) -> bool) -> Vec<Option<PathBuf>> {
    // The free names given to the folders at the top, by their names.
    let mut renamed: HashMap<String, String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    files
        .iter()
        .map(|file| {
            let relative = safe_relative(&file.name)?;
            let mut parts = relative.components();
            let first = parts.next()?.as_os_str().to_string_lossy().into_owned();
            let key = first.to_lowercase();
            let in_folder = file.folder || parts.clone().next().is_some();
            let known = renamed.get(&key).filter(|_| in_folder).cloned();
            let top = match known {
                Some(top) => top,
                None => {
                    let top = unique_name(&first, |candidate| {
                        taken(candidate) || used.contains(&candidate.to_lowercase())
                    });
                    used.insert(top.to_lowercase());
                    if in_folder {
                        renamed.insert(key, top.clone());
                    }
                    top
                }
            };
            Some(Path::new(&top).join(parts.as_path()))
        })
        .collect()
}

/// Saves the files of `data` that exist only in their program into
/// `folder`, and has `done` told what was made: on a thread of its own
/// where the source lets the drop finish later, else here, as the data
/// can be read only while the drop lasts.
fn save_virtual_later(
    data: IDataObject,
    folder: PathBuf,
    done: impl FnOnce(Outcome) + Send + 'static,
) {
    // SAFETY: the calls only ask the source whether it can hand the data
    // over later, and tell it that it is to.
    let later = unsafe {
        data.cast::<IDataObjectAsyncCapability>()
            .ok()
            .filter(|capable| capable.GetAsyncMode().is_ok_and(|mode| mode.as_bool()))
            .filter(|capable| capable.StartOperation(None).is_ok())
    };
    let Some(capable) = later else {
        return done(save_virtual(&data, &folder));
    };
    let shared = match AgileReference::new(&data) {
        Ok(shared) => shared,
        Err(_) => {
            let outcome = save_virtual(&data, &folder);
            // SAFETY: the operation started above is ended once.
            let _ = unsafe { capable.EndOperation(S_OK, None, DROPEFFECT_COPY.0) };
            return done(outcome);
        }
    };
    ShellThread::run_alone("drop", move || {
        let outcome = match shared.resolve() {
            Ok(data) => {
                let outcome = save_virtual(&data, &folder);
                if let Ok(capable) = data.cast::<IDataObjectAsyncCapability>() {
                    // SAFETY: the operation started on the drop is ended
                    // once, from here.
                    let _ = unsafe { capable.EndOperation(S_OK, None, DROPEFFECT_COPY.0) };
                }
                outcome
            }
            Err(error) => Outcome {
                problems: vec![format!("the dropped files cannot be read: {error}")],
                ..Outcome::default()
            },
        };
        done(outcome);
    });
}

/// Saves the files of `data` that exist only in their program into
/// `folder`, and writes down what was made in the journal.
fn save_virtual(data: &IDataObject, folder: &Path) -> Outcome {
    let files = virtual_files(data);
    let plan = plan_virtual(&files, |name| folder.join(name).exists());
    let mut outcome = Outcome::default();
    let mut made_tops: HashSet<PathBuf> = HashSet::new();
    for (index, (file, relative)) in files.iter().zip(plan).enumerate() {
        let Some(relative) = relative else {
            outcome
                .problems
                .push(format!("`{}` cannot be the name of a file", file.name));
            continue;
        };
        let path = folder.join(&relative);
        let saved = match file.folder {
            true => std::fs::create_dir_all(&path)
                .with_context(|| format!("{} cannot be made", path.display())),
            false => save_contents(data, index, &path),
        };
        if let Err(error) = saved {
            outcome.problems.push(format!("{error:#}"));
            continue;
        }
        // Only what is made directly in the folder is written down: the
        // rest goes and comes back with it.
        let top = relative.components().next().map(|part| folder.join(part));
        if let Some(top) = top
            && made_tops.insert(top.clone())
        {
            let made = match top == path && !file.folder {
                true => Made::File { template: None },
                false => Made::Folder,
            };
            outcome.deeds.push(FileDeed::Created { path: top, made });
        }
    }
    UndoJournal::note(outcome.deeds.clone());
    outcome
}

/// Saves the contents of the file numbered `index` in `data` at `path`.
fn save_contents(data: &IDataObject, index: usize, path: &Path) -> Result<()> {
    let Some(contents) = format(CFSTR_FILECONTENTS) else {
        bail!("the contents of the dropped files cannot be asked for");
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("{} cannot be made", parent.display()))?;
    }
    let tymed = TYMED_HGLOBAL.0 | TYMED_ISTREAM.0 | TYMED_ISTORAGE.0;
    let request = request(contents, index as i32, tymed);
    // SAFETY: the medium is read according to its kind while it is held,
    // and released once after.
    unsafe {
        let mut medium = data
            .GetData(&raw const request)
            .with_context(|| format!("the contents of {} cannot be read", path.display()))?;
        let saved = (|| -> Result<()> {
            match medium.tymed as i32 {
                kind if kind == TYMED_HGLOBAL.0 => {
                    let bytes = read_global(medium.u.hGlobal).context("the contents are empty")?;
                    std::fs::write(path, bytes)?;
                }
                kind if kind == TYMED_ISTREAM.0 => {
                    let stream = (*medium.u.pstm)
                        .as_ref()
                        .context("the contents are empty")?;
                    let mut file = std::fs::File::create(path)?;
                    let mut buffer = vec![0u8; 64 * 1024];
                    loop {
                        let mut read = 0u32;
                        let result = stream.Read(
                            buffer.as_mut_ptr().cast(),
                            buffer.len() as u32,
                            Some(&raw mut read),
                        );
                        result.ok()?;
                        if read == 0 {
                            break;
                        }
                        file.write_all(&buffer[..read as usize])?;
                    }
                }
                kind if kind == TYMED_ISTORAGE.0 => {
                    let storage = (*medium.u.pstg)
                        .as_ref()
                        .context("the contents are empty")?;
                    let name = super::shell_item::wide(path.as_os_str());
                    let copy = StgCreateDocfile(
                        windows::core::PCWSTR(name.as_ptr()),
                        STGM_CREATE | STGM_READWRITE | STGM_SHARE_EXCLUSIVE,
                        None,
                    )?;
                    storage.CopyTo(None, None, &copy)?;
                    copy.Commit(STGC_DEFAULT.0 as u32)?;
                }
                _ => bail!("the contents come in a form Tessera does not read"),
            }
            Ok(())
        })();
        ReleaseStgMedium(&raw mut medium);
        saved.with_context(|| format!("{} cannot be saved", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn zone(x: f32, y: f32, width: f32, height: f32, folder: &str) -> DropZone {
        DropZone {
            bounds: Bounds::new(point(px(x), px(y)), size(px(width), px(height))),
            folder: PathBuf::from(folder),
        }
    }

    #[test]
    fn the_topmost_zone_takes_the_drop() {
        let zones = [
            zone(0., 0., 100., 100., r"C:\shown"),
            zone(0., 20., 100., 20., r"C:\shown\row"),
        ];
        assert_eq!(
            top_zone(&zones, point(px(10.), px(25.))),
            Some((1, PathBuf::from(r"C:\shown\row")))
        );
        assert_eq!(
            top_zone(&zones, point(px(10.), px(60.))),
            Some((0, PathBuf::from(r"C:\shown")))
        );
        assert_eq!(top_zone(&zones, point(px(150.), px(60.))), None);
    }

    #[test]
    fn an_id_array_gives_absolute_lists() {
        // Two items below a parent of one item.
        let parent = [3u8, 0, 7, 0, 0];
        let first = [3u8, 0, 1, 0, 0];
        let second = [4u8, 0, 2, 2, 0, 0];
        let header = 4 + 3 * 4;
        let offsets = [
            header,
            header + parent.len(),
            header + parent.len() + first.len(),
        ];
        let mut bytes = 2u32.to_le_bytes().to_vec();
        for offset in offsets {
            bytes.extend((offset as u32).to_le_bytes());
        }
        bytes.extend(parent);
        bytes.extend(first);
        bytes.extend(second);
        let items = items_of_id_array(&bytes);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_bytes(), [3, 0, 7, 3, 0, 1, 0, 0]);
        assert_eq!(items[1].as_bytes(), [3, 0, 7, 4, 0, 2, 2, 0, 0]);
        assert!(items_of_id_array(&[1, 0]).is_empty());
    }

    #[test]
    fn a_dragged_name_stays_inside_the_folder() {
        assert_eq!(safe_relative("photo.jpg"), Some(PathBuf::from("photo.jpg")));
        assert_eq!(
            safe_relative(r"Album\photo.jpg"),
            Some(Path::new("Album").join("photo.jpg"))
        );
        for bad in [
            r"..\evil.exe",
            r"C:\Windows\x",
            "a:b",
            "",
            "dot.",
            r"\\server\x\..\y",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad}");
        }
    }

    #[test]
    fn dragged_files_get_free_names_with_their_folders() {
        let taken = ["Album", "photo.jpg"];
        let file = |name: &str, folder| VirtualFile {
            name: name.to_owned(),
            folder,
        };
        let files = [
            file("Album", true),
            file(r"Album\a.jpg", false),
            file(r"album\b.jpg", false),
            file("photo.jpg", false),
            file("photo.jpg", false),
            file(r"..\x", false),
        ];
        let plan = plan_virtual(&files, |name| taken.contains(&name));
        assert_eq!(
            plan,
            [
                Some(PathBuf::from("Album (2)")),
                Some(Path::new("Album (2)").join("a.jpg")),
                Some(Path::new("Album (2)").join("b.jpg")),
                Some(PathBuf::from("photo (2).jpg")),
                Some(PathBuf::from("photo (3).jpg")),
                None,
            ]
        );
    }
}
