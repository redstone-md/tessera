// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The whole namespace of the Shell as the file manager browses it: the
//! places that are no folders of the file system (Control Panel, the
//! libraries, Network, phones and cameras, the insides of ZIP files, the
//! Gallery, Windows' own Home), read item by item as Explorer reads them,
//! with `IShellItem::BindToHandler(BHID_EnumItems)`.
//!
//! A place is a [`ShellPath`]: the chain of its steps from the desktop,
//! each with its identifier list kept as bytes, its name as the user reads
//! it (`SIGDN_NORMALDISPLAY`) and its parsing name
//! (`SIGDN_DESKTOPABSOLUTEPARSING`), by which it is written to sessions
//! and found again. What a place holds is a list of [`ShellChild`]ren,
//! plain data that crosses threads; whoever needs a COM object of one
//! makes it again from its bytes on the thread that needs it.
//!
//! Reading a place can wait for seconds (Network asks Function Discovery,
//! a bare `\\server` asks the server, a phone may be asleep), so it is
//! done neither on the thread of GPUI nor on the [`ShellThread`], whose
//! menus and icons the desktop and the file manager share. Each reading
//! takes a thread of a small pool of its own, each thread in a
//! single-threaded apartment, at most [`MOST_THREADS`] reading at once,
//! made as they are needed. The items come in batches of
//! [`BATCH`] as the Shell hands them over. A reading stops between two
//! batches once its [`ShellListing`] is dropped, as when the pane leaves
//! the place, and gives its thread back; one that hangs inside the Shell
//! when it is dropped is left to end on its own, its items are thrown
//! away, and it no longer counts against the threads that may read.
#![allow(unsafe_code)]

use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result};
use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_DateModified, PKEY_ItemTypeText, PKEY_Photo_DateTaken, PKEY_Size,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoUninitialize, STGM_READWRITE,
};
use windows::Win32::System::SystemServices::{
    SFGAO_CANDELETE, SFGAO_CANRENAME, SFGAO_FILESYSTEM, SFGAO_FLAGS, SFGAO_FOLDER,
    SFGAO_HASSUBFOLDER, SFGAO_HIDDEN, SFGAO_LINK, SFGAO_STREAM,
};
use windows::Win32::UI::Shell::{
    BHID_EnumItems, DSFT_DETECT, FOLDERID_Libraries, IEnumShellItems, IShellItem, IShellItem2,
    IShellLibrary, LMD_ALLOWUNINDEXABLENETWORKLOCATIONS, LOF_PINNEDTONAVPANE, LSF_MAKEUNIQUENAME,
    SEE_MASK_FLAG_LOG_USAGE, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
    SHGetNameFromIDList, SHShowManageLibraryUI, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellExecuteExW, ShellLibrary,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, Interface, PCWSTR};

use super::ShellThread;
use super::shell_item::{IdList, ShellId, name_of_item, path_of_item, take_text};
use super::shell_items::dialog_owner;

/// How many places are read at once at most, each on a thread of its own.
const MOST_THREADS: usize = 4;

/// How many items the Shell is asked for at a time.
const BATCH: usize = 64;

/// How long a thread of the pool waits for work before it ends.
const IDLE_FOR: Duration = Duration::from_secs(60);

/// The attributes of an item the file manager asks for.
const ASKED: SFGAO_FLAGS = SFGAO_FLAGS(
    SFGAO_FOLDER.0
        | SFGAO_FILESYSTEM.0
        | SFGAO_STREAM.0
        | SFGAO_LINK.0
        | SFGAO_HIDDEN.0
        | SFGAO_CANRENAME.0
        | SFGAO_CANDELETE.0
        | SFGAO_HASSUBFOLDER.0,
);

/// One step of the way from the desktop to a place of the Shell.
#[derive(Clone, Debug)]
pub struct ShellStep {
    /// Its identifier list from the desktop; `None` until the name was
    /// resolved.
    id: Option<ShellId>,
    /// What the user reads, in the language of Windows.
    pub name: String,
    /// The name the Shell parses it by.
    pub parsing: String,
    /// Its folder of the file system, where it is one.
    pub path: Option<PathBuf>,
}

impl ShellStep {
    /// The step of what `parsing` names, not resolved yet: it is shown by
    /// `name` until it is.
    pub fn named(parsing: &str, name: &str) -> Self {
        Self {
            id: None,
            name: name.to_owned(),
            parsing: parsing.to_owned(),
            path: None,
        }
    }
}

/// A place of the Shell: the steps from the desktop to it, itself the
/// last. Two are the same place where their parsing names are, in any
/// case.
#[derive(Clone, Debug)]
pub struct ShellPath {
    steps: Vec<ShellStep>,
}

impl PartialEq for ShellPath {
    fn eq(&self, other: &Self) -> bool {
        self.parsing().eq_ignore_ascii_case(other.parsing())
    }
}

impl Eq for ShellPath {}

impl std::hash::Hash for ShellPath {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.parsing().to_ascii_lowercase().hash(state);
    }
}

impl ShellPath {
    /// The place that `parsing` names, to be resolved when it is read; it
    /// is called `name` until then.
    pub fn named(parsing: &str, name: &str) -> Self {
        Self {
            steps: vec![ShellStep::named(parsing, name)],
        }
    }

    fn last(&self) -> &ShellStep {
        // A path is never made without a step.
        &self.steps[self.steps.len() - 1]
    }

    /// The name the user reads.
    pub fn name(&self) -> &str {
        &self.last().name
    }

    /// The name the Shell parses it by, which sessions keep.
    pub fn parsing(&self) -> &str {
        &self.last().parsing
    }

    /// The steps from the desktop down to it, itself the last.
    pub fn steps(&self) -> &[ShellStep] {
        &self.steps
    }

    /// The place it lies in; none for a place right on the desktop.
    pub fn parent(&self) -> Option<Self> {
        (self.steps.len() > 1).then(|| Self {
            steps: self.steps[..self.steps.len() - 1].to_vec(),
        })
    }

    /// The place of the step `index` of its steps.
    pub fn up_to(&self, index: usize) -> Option<Self> {
        (index < self.steps.len()).then(|| Self {
            steps: self.steps[..=index].to_vec(),
        })
    }

    /// The place of `child`, which it holds.
    pub fn child(&self, child: &ShellChild) -> Self {
        let mut steps = self.steps.clone();
        steps.push(ShellStep {
            id: Some(child.id.clone()),
            name: child.name.clone(),
            parsing: child.parsing.clone(),
            path: { child.attributes.is_file_folder() }
                .then(|| child.path.clone())
                .flatten(),
        });
        Self { steps }
    }
}

/// What the Shell says an item is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShellAttributes {
    /// It holds items (a folder, a ZIP file, a phone).
    pub folder: bool,
    /// It is part of the file system, with a path.
    pub filesystem: bool,
    /// It is a file, which may be a folder too, as a ZIP file is.
    pub stream: bool,
    pub link: bool,
    pub hidden: bool,
    pub can_rename: bool,
    pub can_delete: bool,
    pub has_subfolders: bool,
}

impl ShellAttributes {
    /// The attributes in the bits `SFGAO_*` of `bits`.
    pub fn of(bits: u32) -> Self {
        let has = |flag: SFGAO_FLAGS| bits & flag.0 != 0;
        Self {
            folder: has(SFGAO_FOLDER),
            filesystem: has(SFGAO_FILESYSTEM),
            stream: has(SFGAO_STREAM),
            link: has(SFGAO_LINK),
            hidden: has(SFGAO_HIDDEN),
            can_rename: has(SFGAO_CANRENAME),
            can_delete: has(SFGAO_CANDELETE),
            has_subfolders: has(SFGAO_HASSUBFOLDER),
        }
    }

    /// Whether it is a folder of the file system, which the file manager
    /// shows as a folder with a path, not as a place of the Shell.
    pub fn is_file_folder(self) -> bool {
        self.folder && self.filesystem && !self.stream
    }

    /// Whether the file manager goes into it: a folder of any kind, an
    /// archive only with `archives`.
    pub fn is_browsed(self, archives: bool) -> bool {
        self.folder && (!self.stream || archives)
    }
}

/// An item a place of the Shell holds, as plain data.
#[derive(Clone, Debug, PartialEq)]
pub struct ShellChild {
    /// Its identifier list from the desktop.
    pub id: ShellId,
    /// What the user reads (localized, `Documents` as `Документы`).
    pub name: String,
    /// The name the Shell parses it by.
    pub parsing: String,
    /// Its path in the file system, where it has one.
    pub path: Option<PathBuf>,
    pub attributes: ShellAttributes,
    /// In bytes, where it is a file that tells.
    pub size: Option<u64>,
    pub modified: Option<SystemTime>,
    /// What kind of item it is, as the column Type says it.
    pub kind: Option<String>,
    /// When the picture was taken, where that was asked and it is one.
    pub taken: Option<SystemTime>,
}

/// What reading a place of the Shell tells, in this order: the place
/// resolved, or that it is a folder of the file system; then batches of
/// its items; then that it is done, or why it failed.
#[derive(Clone, Debug)]
pub enum ShellRead {
    /// The place, with all its steps known.
    Resolved(ShellPath),
    /// The place is a folder of the file system at this path, which is
    /// shown as such; nothing else follows.
    Folder(PathBuf),
    /// Items it holds.
    Items(Vec<ShellChild>),
    /// It was read to its end.
    Done,
    /// It cannot be read, with the Shell's reason.
    Failed(ShellFailure),
}

/// Why a place of the Shell could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellFailure {
    /// Windows knows no place of that name.
    Unknown(String),
    /// The place is there, but lists no items: an applet of Control Panel
    /// with a view of its own, a file, a network that cannot be reached.
    NotBrowsable(String),
    /// Reading stopped part of the way.
    Broken(String),
}

impl ShellFailure {
    /// What the Shell said.
    pub fn text(&self) -> &str {
        match self {
            Self::Unknown(text) | Self::NotBrowsable(text) | Self::Broken(text) => text,
        }
    }
}

/// What is read of each item, beside its names and attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// When pictures were taken, which the Gallery groups by; asking
    /// opens every file, so it is asked only there.
    pub dates_taken: bool,
}

/// A reading of a place under way; dropping it stops it.
pub struct ShellListing {
    events: Receiver<ShellRead>,
    stop: Arc<AtomicBool>,
}

impl ShellListing {
    /// The next of what the reading tells; `None` once it ended.
    pub async fn next(&self) -> Option<ShellRead> {
        self.events.recv().await.ok()
    }
}

impl Drop for ShellListing {
    fn drop(&mut self) {
        Pool::get().stop(&self.stop);
    }
}

/// The places of the Shell, read away from the user interface.
pub struct ShellFolder;

impl ShellFolder {
    /// Starts reading `place` on a thread of the pool and returns at once.
    pub fn open(place: &ShellPath, reading: Reading) -> ShellListing {
        let (sender, events) = async_channel::unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let place = place.clone();
        Pool::get().submit(Job {
            stop: stop.clone(),
            work: Box::new(move |stop| read(&place, reading, stop, &sender)),
        });
        ShellListing { events, stop }
    }

    /// Opens the item `id` with its default verb, as a double click in
    /// Explorer does, by its identifier list: an applet of Control Panel,
    /// a file inside an archive, a page that opens Settings. It may wait,
    /// so it runs on a thread of its own; a failure is reported as of
    /// `name`.
    pub fn invoke(id: ShellId, name: String) {
        let owner = dialog_owner().0 as isize;
        ShellThread::run_alone("shell item", move || {
            if let Err(error) = invoke_here(&id, owner) {
                report!("`{name}` did not open: {error:#}");
            }
        });
    }
}

/// Opens `id` with its default verb on this thread.
fn invoke_here(id: &ShellId, owner: isize) -> Result<()> {
    let list = IdList::of_id(id)?;
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_LOG_USAGE,
        hwnd: windows::Win32::Foundation::HWND(owner as *mut c_void),
        lpIDList: list.as_ptr().cast_mut().cast(),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the structure has its size, and the identifier list it
    // points to is owned here and outlives the call.
    unsafe { ShellExecuteExW(&raw mut info) }.context("the Shell did not open it")
}

/// Reads `place` on this thread of the pool, telling `events` until it
/// is done, it fails or `stop` is set.
fn read(place: &ShellPath, reading: Reading, stop: &AtomicBool, events: &Sender<ShellRead>) {
    let told = |event: ShellRead| events.try_send(event).is_ok() && !stop.load(Ordering::SeqCst);
    let found = match place.last().id.as_ref() {
        Some(id) => IdList::of_id(id),
        None => IdList::of_path(Path::new(&place.last().parsing)),
    };
    let list = match found {
        Ok(list) => list,
        Err(error) => {
            told(ShellRead::Failed(ShellFailure::Unknown(format!(
                "{error:#}"
            ))));
            return;
        }
    };
    let item = match list.item() {
        Ok(item) => item,
        Err(error) => {
            told(ShellRead::Failed(ShellFailure::Unknown(format!(
                "{error:#}"
            ))));
            return;
        }
    };
    let attributes = attributes_of(&item);
    if attributes.is_file_folder()
        && let Some(path) = path_of_item(&item)
    {
        told(ShellRead::Folder(path));
        return;
    }
    if !told(ShellRead::Resolved(resolved(&list))) {
        return;
    }
    // SAFETY: the item is alive for the call, which makes an enumerator
    // owned here.
    let items: IEnumShellItems = match unsafe { item.BindToHandler(None, &BHID_EnumItems) } {
        Ok(items) => items,
        Err(error) => {
            let failure = ShellFailure::NotBrowsable(error.message());
            told(ShellRead::Failed(failure));
            return;
        }
    };
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let mut batch: Vec<Option<IShellItem>> = vec![None; BATCH];
        let mut fetched = 0u32;
        // SAFETY: the slice holds room for the items asked for, which the
        // enumerator fills and counts in `fetched`; they are owned here.
        let next = unsafe { items.Next(&mut batch, Some(&raw mut fetched)) };
        if let Err(error) = next {
            told(ShellRead::Failed(ShellFailure::Broken(error.message())));
            return;
        }
        let children: Vec<ShellChild> = { batch.iter().take(fetched as usize).flatten() }
            .filter_map(|item| child_of(item, reading))
            .collect();
        if !children.is_empty() && !told(ShellRead::Items(children)) {
            return;
        }
        if (fetched as usize) < BATCH {
            break;
        }
    }
    told(ShellRead::Done);
}

/// The attributes of `item`; none where it does not tell.
fn attributes_of(item: &IShellItem) -> ShellAttributes {
    // SAFETY: the item is alive for the call, which only answers.
    let bits = unsafe { item.GetAttributes(ASKED) }.map_or(0, |bits| bits.0);
    ShellAttributes::of(bits)
}

/// The place of `list` with each of its steps from the desktop.
fn resolved(list: &IdList) -> ShellPath {
    let mut steps = Vec::new();
    let mut at = Some(list.to_id());
    while let Some(id) = at.take() {
        // The desktop, an empty list, is where every place starts.
        if id.as_bytes().len() <= 2 {
            break;
        }
        let Ok(step) = IdList::of_id(&id) else {
            break;
        };
        let name = |kind: SIGDN| name_of_list(&step, kind);
        let parsing = name(SIGDN_DESKTOPABSOLUTEPARSING).unwrap_or_default();
        let folder = { step.item().ok() }.is_some_and(|item| attributes_of(&item).is_file_folder());
        steps.push(ShellStep {
            name: name(SIGDN_NORMALDISPLAY).unwrap_or_else(|| parsing.clone()),
            path: { folder.then(|| name(SIGDN_FILESYSPATH)) }
                .flatten()
                .map(PathBuf::from),
            parsing,
            id: Some(id),
        });
        at = step.parent().map(|parent| parent.to_id());
    }
    steps.reverse();
    ShellPath { steps }
}

/// The name of `list` of the kind `kind`, where it has one.
fn name_of_list(list: &IdList, kind: SIGDN) -> Option<String> {
    // SAFETY: the list is valid for the call; the name it gives is read
    // and freed once.
    let text = unsafe { SHGetNameFromIDList(list.as_ptr(), kind) }.ok()?;
    Some(take_text(text).to_string_lossy().into_owned())
}

/// The child of a place `item` is, as plain data.
fn child_of(item: &IShellItem, reading: Reading) -> Option<ShellChild> {
    let id = IdList::of_object(item).ok()?.to_id();
    let name = name_of_item(item, SIGDN_NORMALDISPLAY)?
        .to_string_lossy()
        .into_owned();
    let parsing = { name_of_item(item, SIGDN_DESKTOPABSOLUTEPARSING) }
        .map_or_else(|| name.clone(), |name| name.to_string_lossy().into_owned());
    let attributes = attributes_of(item);
    let path = attributes.filesystem.then(|| path_of_item(item)).flatten();
    let rich: Option<IShellItem2> = item.cast().ok();
    // SAFETY: the item is alive for the calls, which only answer; a text
    // it gives is read and freed once.
    let (size, modified, kind, taken) = unsafe {
        match &rich {
            Some(rich) => (
                { (!attributes.folder || attributes.stream).then(|| rich.GetUInt64(&PKEY_Size)) }
                    .and_then(Result::ok),
                rich.GetFileTime(&PKEY_DateModified).ok().and_then(moment),
                { rich.GetString(&PKEY_ItemTypeText).ok() }
                    .map(|text| take_text(text).to_string_lossy().into_owned())
                    .filter(|kind| !kind.is_empty()),
                {
                    reading
                        .dates_taken
                        .then(|| rich.GetFileTime(&PKEY_Photo_DateTaken))
                }
                .and_then(Result::ok)
                .and_then(moment),
            ),
            None => (None, None, None, None),
        }
    };
    Some(ShellChild {
        id,
        name,
        parsing,
        path,
        attributes,
        size,
        modified,
        kind,
        taken,
    })
}

/// The moment a `FILETIME` names; none for zero.
fn moment(time: FILETIME) -> Option<SystemTime> {
    let ticks = (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    moment_of_ticks(ticks)
}

/// The moment `ticks` of 100 ns since 1601 name; none for zero or for a
/// moment before 1970.
fn moment_of_ticks(ticks: u64) -> Option<SystemTime> {
    /// The ticks from 1601 to 1970.
    const UNIX_START: u64 = 116_444_736_000_000_000;
    let since = ticks.checked_sub(UNIX_START).filter(|_| ticks != 0)?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(since.saturating_mul(100)))
}

/// A reading handed to the pool, with the flag that stops it.
struct Job {
    stop: Arc<AtomicBool>,
    work: Box<dyn FnOnce(&AtomicBool) + Send>,
}

/// The threads that read places, and the readings waiting for one.
struct Pool {
    state: Mutex<PoolState>,
    came: Condvar,
}

#[derive(Default)]
struct PoolState {
    waiting: VecDeque<Job>,
    /// The threads there are, and how many of them wait for work.
    threads: usize,
    idle: usize,
    /// The stop flags of the readings that run, one per busy thread.
    running: Vec<Arc<AtomicBool>>,
}

impl PoolState {
    /// The threads that count against [`MOST_THREADS`]: all but those
    /// whose reading was stopped while it hangs in the Shell.
    fn counted(&self) -> usize {
        let abandoned = { self.running.iter() }
            .filter(|stop| stop.load(Ordering::SeqCst))
            .count();
        self.threads.saturating_sub(abandoned)
    }
}

impl Pool {
    fn get() -> &'static Self {
        static POOL: OnceLock<Pool> = OnceLock::new();
        POOL.get_or_init(|| Self {
            state: Mutex::new(PoolState::default()),
            came: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `job`, with a new thread for it where none waits and fewer
    /// than the most read.
    fn submit(&'static self, job: Job) {
        let mut state = self.lock();
        state.waiting.push_back(job);
        if state.idle < state.waiting.len() && state.counted() < MOST_THREADS {
            state.threads += 1;
            let started = thread::Builder::new()
                .name("shell folder".to_owned())
                .spawn(move || self.work());
            if let Err(error) = started {
                state.threads -= 1;
                report!("no thread reads places of the Shell: {error}");
            }
        }
        drop(state);
        self.came.notify_one();
    }

    /// Stops the reading of `stop`: one that waits is taken out of the
    /// queue, one that runs stops at its next batch.
    fn stop(&'static self, stop: &Arc<AtomicBool>) {
        stop.store(true, Ordering::SeqCst);
        let mut state = self.lock();
        state.waiting.retain(|job| !Arc::ptr_eq(&job.stop, stop));
        // A reading stopped frees its place among those counted; one
        // waiting may now get a thread.
        if !state.waiting.is_empty() && state.idle == 0 && state.counted() < MOST_THREADS {
            state.threads += 1;
            if thread::Builder::new()
                .name("shell folder".to_owned())
                .spawn(move || self.work())
                .is_err()
            {
                state.threads -= 1;
            }
        }
    }

    /// The body of a thread of the pool: readings, one after the other,
    /// until none came for a while.
    fn work(&'static self) {
        // SAFETY: COM is initialized once for this thread, which uninit-
        // ializes it as it ends, after every object of it is released.
        let initialized = unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok()
        };
        loop {
            let mut state = self.lock();
            state.idle += 1;
            while state.waiting.is_empty() {
                let (woken, waited) = { self.came.wait_timeout(state, IDLE_FOR) }
                    .unwrap_or_else(PoisonError::into_inner);
                state = woken;
                if waited.timed_out() && state.waiting.is_empty() {
                    state.idle -= 1;
                    state.threads -= 1;
                    drop(state);
                    return Self::leave(initialized);
                }
            }
            state.idle -= 1;
            let Some(job) = state.waiting.pop_front() else {
                continue;
            };
            state.running.push(job.stop.clone());
            drop(state);
            let stop = job.stop.clone();
            if !stop.load(Ordering::SeqCst)
                && catch_unwind(AssertUnwindSafe(|| (job.work)(&stop))).is_err()
            {
                report!("a place of the Shell was not read to its end");
            }
            let mut state = self.lock();
            state.running.retain(|running| !Arc::ptr_eq(running, &stop));
            // Threads made while others hung are more than the pool keeps,
            // now that this one is free: it ends.
            if state.counted() > MOST_THREADS {
                state.threads -= 1;
                drop(state);
                return Self::leave(initialized);
            }
        }
    }

    fn leave(initialized: bool) {
        if initialized {
            // SAFETY: COM was initialized on this thread, and nothing of
            // it is used any more.
            unsafe { CoUninitialize() };
        }
    }
}

/// The libraries of Windows (Documents, Music, Pictures, Videos and the
/// user's own), changed as Explorer's ribbon changes them.
pub struct Libraries;

impl Libraries {
    /// Opens the dialog of Windows that adds and removes the folders of the
    /// library `parsing`.
    pub fn manage(parsing: String) {
        Self::change(parsing, "the library was not changed", |_, item| {
            let owner = dialog_owner();
            // SAFETY: the item is alive for the call, which shows a modal
            // dialog on this thread.
            unsafe {
                SHShowManageLibraryUI(
                    item,
                    owner,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    LMD_ALLOWUNINDEXABLENETWORKLOCATIONS,
                )
            }?;
            Ok(())
        });
    }

    /// Makes the folder `folder` the one the library `parsing` saves new
    /// files into.
    pub fn set_save_folder(parsing: String, folder: PathBuf) {
        Self::change(
            parsing,
            "the save location was not set",
            move |library, _| {
                let folder = super::shell_item::item_of_path(&folder)?;
                // SAFETY: the library and the folder are alive for the calls.
                unsafe {
                    library.SetDefaultSaveFolder(DSFT_DETECT, &folder)?;
                    library.Commit()?;
                }
                Ok(())
            },
        );
    }

    /// Shows the library `parsing` at the side of Explorer and the file
    /// manager with `shown`, or no longer.
    pub fn set_in_navigation(parsing: String, shown: bool) {
        Self::change(parsing, "the library was not changed", move |library, _| {
            let options = match shown {
                true => LOF_PINNEDTONAVPANE,
                false => Default::default(),
            };
            // SAFETY: the library is alive for the calls.
            unsafe {
                library.SetOptions(LOF_PINNEDTONAVPANE, options)?;
                library.Commit()?;
            }
            Ok(())
        });
    }

    /// Makes a new library called `name`, a free name like it where it is
    /// taken, in the folder of the libraries.
    pub fn create(name: String) -> Receiver<Result<(), String>> {
        ShellThread::call(move || {
            // SAFETY: the library is created and saved on this thread,
            // which has COM; the name outlives the call.
            unsafe {
                let library: IShellLibrary =
                    CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER)
                        .map_err(|error| error.message())?;
                {
                    library.SaveInKnownFolder(
                        &FOLDERID_Libraries,
                        &HSTRING::from(name),
                        LSF_MAKEUNIQUENAME,
                    )
                }
                .map(|_| ())
                .map_err(|error| error.message())
            }
        })
    }

    /// Loads the library `parsing` on the shell thread, which pumps the
    /// messages its dialogs need, and has `change` change it; a failure is
    /// reported as `failed`.
    fn change(
        parsing: String,
        failed: &'static str,
        change: impl FnOnce(&IShellLibrary, &IShellItem) -> Result<()> + Send + 'static,
    ) {
        ShellThread::run(move || {
            let loaded = (|| -> Result<()> {
                let item = IdList::of_path(Path::new(&parsing))?.item()?;
                // SAFETY: the library is created and loaded on this thread,
                // which has COM; the item is alive for the call.
                let library: IShellLibrary = unsafe {
                    let library: IShellLibrary =
                        CoCreateInstance(&ShellLibrary, None, CLSCTX_INPROC_SERVER)?;
                    library.LoadLibraryFromItem(&item, STGM_READWRITE.0)?;
                    library
                };
                change(&library, &item)
            })();
            if let Err(error) = loaded {
                report!("{failed}: {error:#}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(name: &str, parsing: &str) -> ShellChild {
        ShellChild {
            id: ShellId::from_bytes(vec![4, 0, 1, 2, 0, 0]).unwrap(),
            name: name.to_owned(),
            parsing: parsing.to_owned(),
            path: None,
            attributes: ShellAttributes::default(),
            size: None,
            modified: None,
            kind: None,
            taken: None,
        }
    }

    #[test]
    fn a_place_is_known_by_its_parsing_name_in_any_case() {
        let panel = ShellPath::named("::{26EE0668-A00A-44D7-9371-BEB064C98683}", "Control Panel");
        let again = ShellPath::named("::{26ee0668-a00a-44d7-9371-beb064c98683}", "Panneau");
        assert_eq!(panel, again);
        assert!(panel.last().id.is_none());
        assert_eq!(panel.name(), "Control Panel");
        assert_eq!(panel.parent(), None);
    }

    #[test]
    fn a_child_adds_a_step_and_its_parent_takes_it_away() {
        let computer = ShellPath::named("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}", "This PC");
        let phone = computer.child(&child("Phone", "::{20D04FE0}\\\\?\\usb#phone"));
        assert_eq!(phone.steps().len(), 2);
        assert!(phone.last().id.is_some());
        assert_eq!(phone.name(), "Phone");
        assert_eq!(phone.parent(), Some(computer.clone()));
        assert_eq!(phone.up_to(0), Some(computer));
        assert_eq!(phone.up_to(2), None);
    }

    #[test]
    fn the_attributes_say_what_the_file_manager_does_with_an_item() {
        let zip = ShellAttributes::of(SFGAO_FOLDER.0 | SFGAO_FILESYSTEM.0 | SFGAO_STREAM.0);
        assert!(!zip.is_file_folder());
        assert!(zip.is_browsed(true));
        assert!(!zip.is_browsed(false));
        let folder = ShellAttributes::of(SFGAO_FOLDER.0 | SFGAO_FILESYSTEM.0);
        assert!(folder.is_file_folder());
        let applet = ShellAttributes::of(SFGAO_CANRENAME.0);
        assert!(!applet.is_browsed(true));
        assert!(applet.can_rename && !applet.can_delete);
    }

    #[test]
    fn a_file_time_is_a_moment_after_1970() {
        assert_eq!(moment_of_ticks(0), None);
        assert_eq!(moment_of_ticks(1), None);
        assert_eq!(
            moment_of_ticks(116_444_736_000_000_000 + 10_000_000),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        );
    }
}
