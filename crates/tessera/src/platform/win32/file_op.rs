// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Work on files the way Explorer does it, with `IFileOperation`: copying,
//! moving, renaming, deleting into the Recycle Bin or for good, and making
//! new folders, files and shortcuts. The Shell shows its own progress and
//! its own questions about conflicts, asks for an administrator where one
//! is needed, and tells what it did item by item.
//!
//! What it did comes back as [`FileDeed`]s, and goes into the process-wide
//! [`UndoJournal`], so that Ctrl+Z in the file manager, or on the desktop,
//! takes the last work back and Ctrl+Y does it again. An item put into the
//! Recycle Bin is remembered by the identifier of the very item there (the
//! `$R` file the Shell made), so taking it back restores that one item, not
//! the newest of the same name.
//!
//! ```ignore
//! let done = FileOperation::new()
//!     .copy(paths, &folder)
//!     .owned_by(window)
//!     .start();
//! cx.spawn(async move |this, cx| {
//!     if let Ok(outcome) = done.recv().await { outcome.report(); }
//!     this.update(cx, |this, cx| this.load(cx)).ok();
//! }).detach();
//! // Taking the last work back, from anywhere:
//! if let Some(done) = UndoJournal::undo() { ... }
//! ```
//!
//! Each operation runs on a thread of its own (see
//! [`ShellThread::run_alone`]), as its dialogs keep it for as long as the
//! work lasts; the thread of GPUI never waits for it.
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString, c_void};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use async_channel::Receiver;
use gpui::Window;
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile,
};
use windows::Win32::UI::Shell::{
    COPYENGINE_E_USER_CANCELLED, FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR,
    FOF_RENAMEONCOLLISION, FOFX_RECYCLEONDELETE, FileOperation as FileOperationClass,
    IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    IShellLinkW, SHGNLI_PIDL, SHGetNewLinkInfoW, ShellLink,
};
use windows::core::{BOOL, HRESULT, Interface, PCWSTR, Ref, implement};

use super::shell_item::{IdList, ShellId, item_of_path, path_of_item, wide};
use super::{NativeSurface, ShellThread, shell_menu};

/// How many pieces of work the journal keeps to take back.
const JOURNAL_DEPTH: usize = 100;

/// What a shortcut that Tessera makes points to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// A file or folder.
    Path(PathBuf),
    /// An item of the Shell without a path, such as This PC or a printer.
    Item(ShellId),
}

/// What a new item is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Made {
    Folder,
    /// A file, empty or made from the template file of that name, which
    /// the Shell looks for in the folders of templates (the user's,
    /// everyone's, and `%WINDIR%\ShellNew`).
    File {
        template: Option<String>,
    },
    /// A shortcut (`.lnk`).
    Link {
        target: LinkTarget,
    },
}

/// One thing that work on files did, as the Shell told it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDeed {
    Copied {
        from: PathBuf,
        to: PathBuf,
    },
    Moved {
        from: PathBuf,
        to: PathBuf,
    },
    Renamed {
        from: PathBuf,
        to: PathBuf,
    },
    /// Put into the Recycle Bin, where it is the item `item`.
    Recycled {
        from: PathBuf,
        item: ShellId,
    },
    /// Deleted for good, which cannot be taken back.
    Erased {
        from: PathBuf,
    },
    Created {
        path: PathBuf,
        made: Made,
    },
    /// Brought back from the Recycle Bin to where it was.
    Restored {
        to: PathBuf,
    },
}

impl FileDeed {
    /// What takes this deed back, if anything can.
    fn reverse(&self) -> Option<Step> {
        Some(match self {
            Self::Copied { to: path, .. }
            | Self::Created { path, .. }
            | Self::Restored { to: path } => Step::Delete {
                path: path.clone(),
                recycle: true,
            },
            Self::Moved { from, to } => Step::moving(to, from),
            Self::Renamed { from, to } => Step::Rename {
                path: to.clone(),
                name: from.file_name()?.to_owned(),
            },
            Self::Recycled { from, item } => Step::Restore {
                item: item.clone(),
                to: from.clone(),
            },
            Self::Erased { .. } => return None,
        })
    }

    /// What does this deed again, after it was taken back, if anything
    /// can.
    fn again(&self) -> Option<Step> {
        Some(match self {
            Self::Copied { from, to } => {
                let Step::Move { from, into, name } = Step::moving(from, to) else {
                    return None;
                };
                Step::Copy { from, into, name }
            }
            Self::Moved { from, to } => Step::moving(from, to),
            Self::Renamed { from, to } => Step::Rename {
                path: from.clone(),
                name: to.file_name()?.to_owned(),
            },
            Self::Recycled { from, .. } => Step::Delete {
                path: from.clone(),
                recycle: true,
            },
            Self::Created { path, made } => Step::New {
                into: path.parent()?.to_owned(),
                name: path.file_name()?.to_owned(),
                made: made.clone(),
            },
            Self::Restored { .. } | Self::Erased { .. } => return None,
        })
    }

    /// Whether the deed can be taken back.
    fn reversible(&self) -> bool {
        !matches!(self, Self::Erased { .. })
    }
}

/// One step of a [`FileOperation`].
#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    /// Copies `from` into the folder `into`, under `name` or its own.
    Copy {
        from: PathBuf,
        into: PathBuf,
        name: Option<OsString>,
    },
    Move {
        from: PathBuf,
        into: PathBuf,
        name: Option<OsString>,
    },
    Rename {
        path: PathBuf,
        name: OsString,
    },
    Delete {
        path: PathBuf,
        recycle: bool,
    },
    New {
        into: PathBuf,
        name: OsString,
        made: Made,
    },
    /// Brings `item` back from the Recycle Bin, to `to`, where it was.
    Restore {
        item: ShellId,
        to: PathBuf,
    },
}

impl Step {
    /// The move that puts what is at `from` at `to`: into the folder of
    /// `to`, under the name of `to` where it differs.
    fn moving(from: &Path, to: &Path) -> Self {
        let into = to.parent().unwrap_or(to).to_owned();
        let name = { to.file_name() }
            .filter(|name| Some(*name) != from.file_name())
            .map(OsStr::to_owned);
        Self::Move {
            from: from.to_owned(),
            into,
            name,
        }
    }

    /// The path the Shell tells of in its notice of this step: the item
    /// it works on, or for a new item the folder it is made in.
    fn asked(&self) -> Option<&Path> {
        match self {
            Self::Copy { from, .. } | Self::Move { from, .. } => Some(from),
            Self::Rename { path, .. } | Self::Delete { path, .. } => Some(path),
            Self::New { into, .. } => Some(into),
            Self::Restore { .. } => None,
        }
    }

    /// Which kind of pass does the step.
    fn pass(&self) -> Pass {
        match self {
            Self::Delete { recycle, .. } => Pass::Shell(Some(*recycle)),
            Self::New {
                made: Made::Link { .. },
                ..
            } => Pass::Links,
            Self::Restore { .. } => Pass::Restore,
            _ => Pass::Shell(None),
        }
    }
}

/// A run of steps done together: by one `IFileOperation`, which deletes
/// either into the Recycle Bin or for good (`None` where it deletes
/// nothing), by making shortcuts, or by restoring from the Recycle Bin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pass {
    Shell(Option<bool>),
    Links,
    Restore,
}

impl Pass {
    /// The pass that does the steps of both, if one can.
    fn joined(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Shell(None), Self::Shell(other)) | (Self::Shell(other), Self::Shell(None)) => {
                Some(Self::Shell(other))
            }
            (one, other) if one == other => Some(one),
            _ => None,
        }
    }
}

/// The steps cut into passes, in their order.
fn passes(steps: &[Step]) -> Vec<(Pass, Range<usize>)> {
    let mut passes: Vec<(Pass, Range<usize>)> = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let pass = step.pass();
        if let Some((last, range)) = passes.last_mut()
            && let Some(joined) = last.joined(pass)
        {
            *last = joined;
            range.end = index + 1;
            continue;
        }
        passes.push((pass, index..index + 1));
    }
    passes
}

/// The flags of an `IFileOperation` that does `steps`, deleting into the
/// Recycle Bin when `recycle`.
fn flags_of(steps: &[Step], recycle: bool) -> FILEOPERATION_FLAGS {
    let mut flags = FOF_NOCONFIRMMKDIR.0;
    if recycle {
        flags |= FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0;
    }
    // A copy into the folder it comes from gets a name of its own
    // ("- Copy"), as do new items whose name is taken meanwhile; any other
    // conflict is asked about.
    let renames = steps.iter().any(|step| match step {
        Step::Copy {
            from,
            into,
            name: None,
        } => from.parent().is_some_and(|parent| same_path(parent, into)),
        Step::New { .. } => true,
        _ => false,
    });
    if renames {
        flags |= FOF_RENAMEONCOLLISION.0;
    }
    FILEOPERATION_FLAGS(flags)
}

/// Whether two paths name the same place, as Windows compares them:
/// without regard to case or a separator at the end.
pub(super) fn same_path(one: &Path, other: &Path) -> bool {
    let key = |path: &Path| {
        let text = path.to_string_lossy();
        text.trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_lowercase()
    };
    key(one) == key(other)
}

/// `name`, or where `taken` says it is taken, the first free one of
/// `name (2)`, `name (3)`… with the number before the extension of a file.
pub fn unique_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    (2u32..)
        .map(|number| format!("{stem} ({number}){extension}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| name.to_owned())
}

/// What a [`FileOperation`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// What was done, in the order the Shell did it. Items inside a
    /// folder that was copied, moved or deleted are not listed apart.
    pub deeds: Vec<FileDeed>,
    /// Whether the user stopped the work, or skipped an item.
    pub aborted: bool,
    /// What went wrong, for the user to read.
    pub problems: Vec<String>,
}

impl Outcome {
    /// Whether the user stopped the work before anything was done, and
    /// nothing went wrong.
    fn stopped_at_once(&self) -> bool {
        self.aborted && self.deeds.is_empty() && self.problems.is_empty()
    }

    /// Reports what went wrong.
    pub fn report(&self) {
        for problem in &self.problems {
            report!("{problem}");
        }
    }
}

/// Work on files, done by the Shell: built step by step, then started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileOperation {
    steps: Vec<Step>,
    /// The window that owns the dialogs, as a number.
    owner: Option<isize>,
    /// Whether what is done is left out of the journal.
    unjournaled: bool,
}

impl FileOperation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Copies each of `paths` into the folder `into`. A copy into the
    /// folder it comes from gets a name of its own, as in Explorer.
    pub fn copy(mut self, paths: impl IntoIterator<Item = PathBuf>, into: &Path) -> Self {
        for from in paths {
            self.steps.push(Step::Copy {
                from,
                into: into.to_owned(),
                name: None,
            });
        }
        self
    }

    /// Copies `path` into the folder `into` under the name `name`.
    #[allow(dead_code, reason = "for the file manager and the desktop")]
    pub fn copy_as(mut self, path: &Path, into: &Path, name: impl AsRef<OsStr>) -> Self {
        self.steps.push(Step::Copy {
            from: path.to_owned(),
            into: into.to_owned(),
            name: Some(name.as_ref().to_owned()),
        });
        self
    }

    /// Moves each of `paths` into the folder `into`.
    pub fn move_(mut self, paths: impl IntoIterator<Item = PathBuf>, into: &Path) -> Self {
        for from in paths {
            self.steps.push(Step::Move {
                from,
                into: into.to_owned(),
                name: None,
            });
        }
        self
    }

    /// Gives the file or folder at `path` the name `name`.
    pub fn rename(mut self, path: &Path, name: impl AsRef<OsStr>) -> Self {
        self.steps.push(Step::Rename {
            path: path.to_owned(),
            name: name.as_ref().to_owned(),
        });
        self
    }

    /// Puts each of `paths` into the Recycle Bin. What does not fit there
    /// is deleted for good, after the Shell asks.
    pub fn recycle(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        for path in paths {
            self.steps.push(Step::Delete {
                path,
                recycle: true,
            });
        }
        self
    }

    /// Deletes each of `paths` for good, after the Shell asks.
    pub fn erase(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        for path in paths {
            self.steps.push(Step::Delete {
                path,
                recycle: false,
            });
        }
        self
    }

    /// Makes a folder called `name` in `into`; where the name is taken
    /// meanwhile, the Shell finds a free one, which the outcome tells.
    #[allow(dead_code, reason = "for the file manager and the desktop")]
    pub fn new_folder(mut self, into: &Path, name: impl AsRef<OsStr>) -> Self {
        self.steps.push(Step::New {
            into: into.to_owned(),
            name: name.as_ref().to_owned(),
            made: Made::Folder,
        });
        self
    }

    /// Makes a file called `name` in `into`: empty, or a copy of the
    /// template file called `template` in a folder of templates.
    #[allow(dead_code, reason = "for the file manager and the desktop")]
    pub fn new_file(
        mut self,
        into: &Path,
        name: impl AsRef<OsStr>,
        template: Option<&str>,
    ) -> Self {
        self.steps.push(Step::New {
            into: into.to_owned(),
            name: name.as_ref().to_owned(),
            made: Made::File {
                template: template.map(str::to_owned),
            },
        });
        self
    }

    /// Makes a shortcut to `target` in the folder `into`, named as
    /// Explorer names one ("Name - Shortcut", in the language of
    /// Windows). A shortcut to a shortcut is a copy of it.
    pub fn link(mut self, target: LinkTarget, into: &Path) -> Self {
        self.steps.push(Step::New {
            into: into.to_owned(),
            // The name is found when the link is made.
            name: OsString::new(),
            made: Made::Link { target },
        });
        self
    }

    /// Brings `item` back from the Recycle Bin to `to`, where it was.
    #[allow(dead_code, reason = "for the view of the Recycle Bin")]
    pub fn restore(mut self, item: ShellId, to: &Path) -> Self {
        self.steps.push(Step::Restore {
            item,
            to: to.to_owned(),
        });
        self
    }

    /// The window of Tessera that owns the dialogs of the work; without
    /// one, the window in front where it is Tessera's.
    #[allow(dead_code, reason = "for callers that know their window")]
    pub fn owned_by(mut self, window: &Window) -> Self {
        if let Ok(surface) = NativeSurface::of(window) {
            self.owner = Some(surface.hwnd.0 as isize);
        }
        self
    }

    /// Leaves what is done out of the [`UndoJournal`].
    fn unjournaled(mut self) -> Self {
        self.unjournaled = true;
        self
    }

    #[allow(dead_code, reason = "for the file manager and the desktop")]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Starts the work on a thread of its own and returns at once; the
    /// receiver yields what was done, and is closed without an answer if
    /// the work failed in a way that ended it. Await it from a task of
    /// GPUI. Nothing is reported; [`Outcome::report`] does that.
    #[allow(dead_code, reason = "for the file manager and the desktop")]
    pub fn start(self) -> Receiver<Outcome> {
        let (answer, answered) = async_channel::bounded(1);
        self.then(move |outcome| {
            answer.try_send(outcome).ok();
        });
        answered
    }

    /// Starts the work as [`Self::start`] does, and has `done` called with
    /// what was done, on the thread of the work.
    pub fn then(mut self, done: impl FnOnce(Outcome) + Send + 'static) {
        if self.owner.is_none() {
            self.owner = own_window_in_front();
        }
        if self.steps.is_empty() {
            return done(Outcome::default());
        }
        // Ctrl+Z waits until this work is in the journal, so that it takes
        // this work back and not the one before.
        let running = (!self.unjournaled).then(Running::start);
        ShellThread::run_alone("file operation", move || {
            let outcome = self.perform();
            if !self.unjournaled {
                UndoJournal::note(outcome.deeds.clone());
            }
            drop(running);
            done(outcome);
        });
    }

    /// Does the steps on this thread, which has COM initialized
    /// apartment-threaded.
    fn perform(&self) -> Outcome {
        let owner = { self.owner.map(|window| HWND(window as *mut c_void)) }.unwrap_or_default();
        let mut outcome = Outcome::default();
        for (pass, range) in passes(&self.steps) {
            let steps = &self.steps[range];
            match pass {
                Pass::Shell(recycle) => {
                    if let Err(error) =
                        shell_pass(steps, recycle.unwrap_or(true), owner, &mut outcome)
                    {
                        outcome.problems.push(format!("{error:#}"));
                    }
                }
                Pass::Links => {
                    for step in steps {
                        if let Err(error) = make_link(step, &mut outcome) {
                            outcome.problems.push(format!("{error:#}"));
                        }
                    }
                }
                Pass::Restore => {
                    for step in steps {
                        if let Err(error) = restore(step, owner, &mut outcome) {
                            outcome.problems.push(format!("{error:#}"));
                        }
                    }
                }
            }
            if outcome.aborted {
                break;
            }
        }
        outcome
    }
}

/// Does `steps` with one `IFileOperation`.
fn shell_pass(steps: &[Step], recycle: bool, owner: HWND, outcome: &mut Outcome) -> Result<()> {
    // SAFETY: the class is created on this thread, which has COM
    // initialized, and is used and released here.
    let operation: IFileOperation =
        unsafe { CoCreateInstance(&FileOperationClass, None, CLSCTX_ALL) }
            .context("Windows cannot work on files here")?;
    let asked: Vec<PathBuf> = steps
        .iter()
        .filter_map(Step::asked)
        .map(Path::to_owned)
        .collect();
    let deeds = Arc::new(Mutex::new(Vec::new()));
    let sink: IFileOperationProgressSink = Sink {
        asked,
        deeds: deeds.clone(),
    }
    .into();
    // SAFETY: the operation and the sink are alive for the calls.
    let cookie = unsafe {
        if !owner.is_invalid() {
            operation.SetOwnerWindow(owner)?;
        }
        operation.SetOperationFlags(flags_of(steps, recycle))?;
        operation.Advise(&sink)?
    };
    let mut queued = 0usize;
    for step in steps {
        match queue(&operation, step) {
            Ok(()) => queued += 1,
            Err(error) => outcome.problems.push(format!("{error:#}")),
        }
    }
    // SAFETY: the operation is alive; it runs its dialogs on this thread
    // and calls the sink here, before it returns.
    let (performed, aborted) = unsafe {
        let performed = match queued {
            0 => Ok(()),
            _ => operation.PerformOperations(),
        };
        let aborted = operation.GetAnyOperationsAborted().is_ok_and(BOOL::as_bool);
        let _ = operation.Unadvise(cookie);
        (performed, aborted)
    };
    outcome.deeds.extend(std::mem::take(&mut *lock(&deeds)));
    outcome.aborted |= aborted;
    if let Err(error) = performed {
        if cancelled(error.code()) {
            outcome.aborted = true;
        } else {
            outcome.problems.push(format!(
                "Windows did not finish the work on the files: {error}"
            ));
        }
    }
    Ok(())
}

/// Whether `code` says that the user stopped the work.
fn cancelled(code: HRESULT) -> bool {
    code == COPYENGINE_E_USER_CANCELLED || code == HRESULT::from_win32(ERROR_CANCELLED.0)
}

/// Hands `step` to `operation`.
fn queue(operation: &IFileOperation, step: &Step) -> Result<()> {
    let name_of = |name: &Option<OsString>| name.as_deref().map(wide);
    let pointer = |name: &Option<Vec<u16>>| {
        name.as_ref()
            .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()))
    };
    let none = None::<&IFileOperationProgressSink>;
    // SAFETY: the items are alive for the calls, and every name is
    // NUL-terminated and outlives `PerformOperations`, which the step only
    // waits for: the operation copies what it is given.
    unsafe {
        match step {
            Step::Copy { from, into, name } => {
                let name = name_of(name);
                operation.CopyItem(&item_of(from)?, &item_of(into)?, pointer(&name), none)?;
            }
            Step::Move { from, into, name } => {
                let name = name_of(name);
                operation.MoveItem(&item_of(from)?, &item_of(into)?, pointer(&name), none)?;
            }
            Step::Rename { path, name } => {
                let name = wide(name);
                operation.RenameItem(&item_of(path)?, PCWSTR(name.as_ptr()), none)?;
            }
            Step::Delete { path, .. } => operation.DeleteItem(&item_of(path)?, none)?,
            Step::New { into, name, made } => {
                let name = wide(name);
                let (attributes, template) = match made {
                    Made::Folder => (FILE_ATTRIBUTE_DIRECTORY.0, None),
                    Made::File { template } => (
                        FILE_ATTRIBUTE_NORMAL.0,
                        template
                            .as_deref()
                            .map(|template| wide(OsStr::new(template))),
                    ),
                    Made::Link { .. } => bail!("a shortcut is not made by a file operation"),
                };
                operation.NewItem(
                    &item_of(into)?,
                    attributes,
                    PCWSTR(name.as_ptr()),
                    pointer(&template),
                    none,
                )?;
            }
            Step::Restore { .. } => bail!("an item is not restored by a file operation"),
        }
    }
    Ok(())
}

/// The item of the Shell at `path`, or why there is none.
fn item_of(path: &Path) -> Result<IShellItem> {
    item_of_path(path).with_context(|| format!("{} cannot be found", path.display()))
}

/// Makes the shortcut of a step that makes one.
fn make_link(step: &Step, outcome: &mut Outcome) -> Result<()> {
    let Step::New {
        into,
        made: Made::Link { target },
        ..
    } = step
    else {
        return Ok(());
    };
    let folder = wide(into.as_os_str());
    let mut name = [0u16; 260];
    let mut must_copy = BOOL(0);
    // SAFETY: the link object is created and used on this thread; every
    // text is NUL-terminated and outlives the calls, the identifier list
    // outlives them too, and `name` and `must_copy` are written by
    // `SHGetNewLinkInfoW` within their sizes.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let proposed = match target {
            LinkTarget::Path(path) => {
                let target = wide(path.as_os_str());
                link.SetPath(PCWSTR(target.as_ptr()))?;
                if let Some(parent) = path.parent().filter(|_| path.is_file()) {
                    let parent = wide(parent.as_os_str());
                    link.SetWorkingDirectory(PCWSTR(parent.as_ptr()))?;
                }
                SHGetNewLinkInfoW(
                    PCWSTR(target.as_ptr()),
                    PCWSTR(folder.as_ptr()),
                    &mut name,
                    &raw mut must_copy,
                    0,
                )
            }
            LinkTarget::Item(id) => {
                let list = IdList::of_id(id)?;
                link.SetIDList(list.as_ptr())?;
                SHGetNewLinkInfoW(
                    PCWSTR(list.as_ptr().cast()),
                    PCWSTR(folder.as_ptr()),
                    &mut name,
                    &raw mut must_copy,
                    SHGNLI_PIDL as u32,
                )
            }
        };
        if !proposed.as_bool() {
            bail!("Windows gives no name for a shortcut in {}", into.display());
        }
        let length = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        let path = into.join(PathBuf::from(
            <OsString as std::os::windows::ffi::OsStringExt>::from_wide(&name[..length]),
        ));
        if let (true, LinkTarget::Path(source)) = (must_copy.as_bool(), target) {
            // The target is a shortcut itself, which is copied instead.
            let file_name = source.file_name().context("the shortcut has no name")?;
            let copy = into.join(unique_name(&file_name.to_string_lossy(), |name| {
                into.join(name).exists()
            }));
            std::fs::copy(source, &copy)
                .with_context(|| format!("{} cannot be copied", source.display()))?;
            outcome.deeds.push(FileDeed::Copied {
                from: source.clone(),
                to: copy,
            });
            return Ok(());
        }
        let persist: IPersistFile = link.cast()?;
        let file = wide(path.as_os_str());
        persist
            .Save(PCWSTR(file.as_ptr()), true)
            .with_context(|| format!("the shortcut {} cannot be saved", path.display()))?;
        outcome.deeds.push(FileDeed::Created {
            path,
            made: Made::Link {
                target: target.clone(),
            },
        });
    }
    Ok(())
}

/// Brings back the item of a step that restores one: with the verb
/// "undelete" of the Recycle Bin, as Explorer restores it, or where the
/// item is known by its file in the bin (`$R…`) rather than as an item of
/// the bin, by moving that file back and deleting its record (`$I…`).
fn restore(step: &Step, owner: HWND, outcome: &mut Outcome) -> Result<()> {
    let Step::Restore { item, to } = step else {
        return Ok(());
    };
    let gone = || format!("{} is no longer in the Recycle Bin", to.display());
    let undeleted =
        shell_menu::invoke_verb_here(vec![IdList::of_id(item)?], "undelete", owner, true);
    if undeleted.is_err() {
        let file = { IdList::of_id(item)?.item().ok() }
            .and_then(|item| path_of_item(&item))
            .filter(|file| file.exists())
            .with_context(gone)?;
        let record = super::bin::info_path(&file).with_context(gone)?;
        let back = Step::moving(&file, to);
        let mut moved = Outcome::default();
        shell_pass(std::slice::from_ref(&back), true, owner, &mut moved)?;
        outcome.problems.append(&mut moved.problems);
        if !moved.deeds.is_empty() {
            // The record only tells the bin what the file was.
            let _ = std::fs::remove_file(record);
        }
    }
    // The item came back only where it left the bin: where it is still
    // there, the user kept what is in its place now, or skipped it.
    let in_bin = { IdList::of_id(item).and_then(|list| list.item()).ok() }
        .and_then(|item| path_of_item(&item))
        .is_some_and(|file| file.exists());
    match !in_bin && to.symlink_metadata().is_ok() {
        true => outcome.deeds.push(FileDeed::Restored { to: to.clone() }),
        false => outcome.aborted = true,
    }
    Ok(())
}

/// The window in front, as a number, where it is Tessera's.
fn own_window_in_front() -> Option<isize> {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut process = 0u32;
    // SAFETY: the calls only read which window is in front and whose it
    // is; `process` outlives the call that fills it.
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() {
            return None;
        }
        GetWindowThreadProcessId(window, Some(&raw mut process));
        (process == GetCurrentProcessId()).then_some(window.0 as isize)
    }
}

/// The listener of an `IFileOperation`, which writes down what the Shell
/// did with the items it was asked about. It is agile, as `#[implement]`
/// makes it, so the Shell may call it from any thread: what it writes is
/// behind a lock.
#[implement(IFileOperationProgressSink)]
struct Sink {
    /// The paths the steps name; what the Shell does to the items inside
    /// a folder is not written down apart.
    asked: Vec<PathBuf>,
    deeds: Arc<Mutex<Vec<FileDeed>>>,
}

/// What `mutex` holds, also where a thread panicked while it held it.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Sink {
    /// The path of `item`, where it is one the steps name.
    fn asked_path(&self, item: Ref<'_, IShellItem>) -> Option<PathBuf> {
        let path = path_of_item(item.as_ref()?)?;
        { self.asked.iter() }
            .any(|asked| same_path(asked, &path))
            .then_some(path)
    }

    /// Writes down what was done to `item`, as `deed` makes of its path
    /// and of the path of what was made, where both are known and the
    /// Shell did it.
    fn write(
        &self,
        item: Ref<'_, IShellItem>,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
        deed: impl FnOnce(PathBuf, PathBuf) -> FileDeed,
    ) {
        if result.is_err() {
            return;
        }
        let made = made.as_ref().and_then(path_of_item);
        if let (Some(from), Some(to)) = (self.asked_path(item), made) {
            lock(&self.deeds).push(deed(from, to));
        }
    }
}

#[allow(non_snake_case, reason = "the names of the methods of COM")]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn FinishOperations(&self, _result: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }

    fn PreRenameItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn PostRenameItem(
        &self,
        _flags: u32,
        item: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.write(item, result, made, |from, to| FileDeed::Renamed {
            from,
            to,
        });
        Ok(())
    }

    fn PreMoveItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn PostMoveItem(
        &self,
        _flags: u32,
        item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.write(item, result, made, |from, to| FileDeed::Moved { from, to });
        Ok(())
    }

    fn PreCopyItem(
        &self,
        _flags: u32,
        _item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn PostCopyItem(
        &self,
        _flags: u32,
        item: Ref<'_, IShellItem>,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        self.write(item, result, made, |from, to| FileDeed::Copied { from, to });
        Ok(())
    }

    fn PreDeleteItem(&self, _flags: u32, _item: Ref<'_, IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }

    fn PostDeleteItem(
        &self,
        _flags: u32,
        item: Ref<'_, IShellItem>,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        if result.is_err() {
            return Ok(());
        }
        let Some(from) = self.asked_path(item) else {
            return Ok(());
        };
        // The item the Shell made in the Recycle Bin, where it went there.
        let in_bin = { made.as_ref() }.and_then(|made| IdList::of_object(made).ok());
        let deed = match in_bin {
            Some(list) => FileDeed::Recycled {
                from,
                item: list.to_id(),
            },
            None => FileDeed::Erased { from },
        };
        lock(&self.deeds).push(deed);
        Ok(())
    }

    fn PreNewItem(
        &self,
        _flags: u32,
        _folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn PostNewItem(
        &self,
        _flags: u32,
        folder: Ref<'_, IShellItem>,
        _name: &PCWSTR,
        template: &PCWSTR,
        attributes: u32,
        result: HRESULT,
        made: Ref<'_, IShellItem>,
    ) -> windows::core::Result<()> {
        if result.is_err() || self.asked_path(folder).is_none() {
            return Ok(());
        }
        let Some(path) = made.as_ref().and_then(path_of_item) else {
            return Ok(());
        };
        let made = match attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0 {
            true => Made::Folder,
            false => Made::File {
                // SAFETY: the template, where there is one, is a
                // NUL-terminated text that lives for the call.
                template: (!template.is_null())
                    .then(|| unsafe { template.to_string() }.ok())
                    .flatten()
                    .filter(|template| !template.is_empty()),
            },
        };
        lock(&self.deeds).push(FileDeed::Created { path, made });
        Ok(())
    }

    fn UpdateProgress(&self, _total: u32, _done: u32) -> windows::core::Result<()> {
        Ok(())
    }

    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// What was done to files, to take back and to do again: each entry is
/// the deeds of one piece of work. There is one for the whole process
/// (see [`UndoJournal::undo`]), which the file manager and the desktop
/// share, as Explorer's windows share theirs.
#[derive(Debug, Default)]
pub struct UndoJournal {
    /// What can be taken back, the newest last.
    done: Vec<Vec<FileDeed>>,
    /// What was taken back and can be done again, the newest last.
    undone: Vec<Vec<FileDeed>>,
    /// How many times new work was written down.
    generation: u64,
    /// How many pieces of work are running that the journal waits for:
    /// work that will be written down, and undoing or redoing.
    running: usize,
}

/// The journal of the process.
static JOURNAL: Mutex<UndoJournal> = Mutex::new(UndoJournal {
    done: Vec::new(),
    undone: Vec::new(),
    generation: 0,
    running: 0,
});

/// Work on files that the [`UndoJournal`] waits for while this exists:
/// meanwhile Ctrl+Z and Ctrl+Y do nothing, so that they take back the
/// newest work and not the one before it, one at a time.
#[derive(Debug)]
pub(super) struct Running(());

impl Running {
    /// Counts work that has started.
    pub(super) fn start() -> Self {
        UndoJournal::shared(|journal| journal.running += 1);
        Self(())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        UndoJournal::shared(|journal| journal.running = journal.running.saturating_sub(1));
    }
}

impl UndoJournal {
    /// Writes down new work: what of `deeds` can be taken back. Anything
    /// taken back before can no longer be done again.
    fn record(&mut self, deeds: Vec<FileDeed>) {
        let deeds: Vec<FileDeed> = deeds.into_iter().filter(FileDeed::reversible).collect();
        if deeds.is_empty() {
            return;
        }
        self.generation += 1;
        self.undone.clear();
        self.push_done(deeds);
    }

    /// Takes the newest entry to undo, or with `again` to redo, with the
    /// generation it is taken in, and counts the work on it as running.
    /// `None` where there is none, or other work is running.
    fn take(&mut self, again: bool) -> Option<(Vec<FileDeed>, u64)> {
        if self.running > 0 {
            return None;
        }
        let deeds = match again {
            true => self.undone.pop(),
            false => self.done.pop(),
        }?;
        self.running += 1;
        Some((deeds, self.generation))
    }

    /// Puts back the entry `deeds` that an undo took in `generation`:
    /// with what can be done again, or where the user stopped the undo
    /// before anything was done, with what can be taken back. Where new
    /// work was written down meanwhile, the entry is dropped, as it no
    /// longer follows the newest work.
    fn after_undo(&mut self, deeds: Vec<FileDeed>, generation: u64, stopped: bool) {
        if self.generation != generation {
            return;
        }
        match stopped {
            true => self.push_done(deeds),
            false => self.undone.push(deeds),
        }
    }

    /// Puts back what a redo of the entry `deeds`, taken in `generation`,
    /// did (`again`): with what can be taken back, or where the user
    /// stopped it before anything was done, the entry with what can be
    /// done again. Where new work was written down meanwhile, both are
    /// dropped, as [`Self::after_undo`] drops its entry.
    fn after_redo(
        &mut self,
        deeds: Vec<FileDeed>,
        again: Vec<FileDeed>,
        generation: u64,
        stopped: bool,
    ) {
        if self.generation != generation {
            return;
        }
        if stopped {
            self.undone.push(deeds);
        } else if !again.is_empty() {
            self.push_done(again);
        }
    }

    fn push_done(&mut self, deeds: Vec<FileDeed>) {
        self.done.push(deeds);
        if self.done.len() > JOURNAL_DEPTH {
            self.done.remove(0);
        }
    }

    /// The operation that takes `deeds` back: each deed reversed, the
    /// last first.
    fn reversal(deeds: &[FileDeed]) -> FileOperation {
        FileOperation {
            steps: deeds.iter().rev().filter_map(FileDeed::reverse).collect(),
            ..FileOperation::default()
        }
        .unjournaled()
    }

    /// The operation that does `deeds` again, in their order.
    fn repetition(deeds: &[FileDeed]) -> FileOperation {
        FileOperation {
            steps: deeds.iter().filter_map(FileDeed::again).collect(),
            ..FileOperation::default()
        }
        .unjournaled()
    }

    /// Writes `deeds` down as work done in the process outside a
    /// [`FileOperation`], such as a folder made with `std::fs`, so that it
    /// can be taken back too.
    pub fn note(deeds: Vec<FileDeed>) {
        Self::shared(|journal| journal.record(deeds));
    }

    /// Whether there is work to take back, and to do again.
    #[allow(dead_code, reason = "for the menus that offer to undo")]
    pub fn can_undo() -> bool {
        Self::shared(|journal| !journal.done.is_empty())
    }

    #[allow(dead_code, reason = "for the menus that offer to redo")]
    pub fn can_redo() -> bool {
        Self::shared(|journal| !journal.undone.is_empty())
    }

    /// Takes back the last work that can be, with the Shell's dialogs, and
    /// returns at once; the receiver yields what was done for it. `None`
    /// where there is nothing to take back, or while other work on files
    /// that goes into the journal, or another undo or redo, still runs.
    /// Where the user stops it before anything is done, the work stays in
    /// the journal to take back later; where it fails, it is dropped, so
    /// that the work before it can be taken back next.
    pub fn undo() -> Option<Receiver<Outcome>> {
        let (deeds, generation) = Self::shared(|journal| journal.take(false))?;
        // `take` counted it.
        let running = Running(());
        let (answer, answered) = async_channel::bounded(1);
        Self::reversal(&deeds).then(move |outcome| {
            let stopped = outcome.stopped_at_once();
            Self::shared(|journal| journal.after_undo(deeds, generation, stopped));
            drop(running);
            answer.try_send(outcome).ok();
        });
        Some(answered)
    }

    /// Does again the last work that was taken back, as [`Self::undo`]
    /// takes it back. `None` where there is nothing to do again, or while
    /// other work runs.
    pub fn redo() -> Option<Receiver<Outcome>> {
        let (deeds, generation) = Self::shared(|journal| journal.take(true))?;
        // `take` counted it.
        let running = Running(());
        let (answer, answered) = async_channel::bounded(1);
        Self::repetition(&deeds).then(move |outcome| {
            let again: Vec<FileDeed> = { outcome.deeds.iter() }
                .filter(|deed| deed.reversible())
                .cloned()
                .collect();
            let stopped = outcome.stopped_at_once();
            Self::shared(|journal| journal.after_redo(deeds, again, generation, stopped));
            drop(running);
            answer.try_send(outcome).ok();
        });
        Some(answered)
    }

    /// Does `work` with the journal of the process.
    fn shared<R>(work: impl FnOnce(&mut Self) -> R) -> R {
        work(&mut lock(&JOURNAL))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> PathBuf {
        PathBuf::from(text)
    }

    fn bin_item() -> ShellId {
        ShellId::from_bytes(vec![4, 0, 1, 2, 0, 0]).unwrap()
    }

    #[test]
    fn each_deed_is_taken_back_by_its_opposite() {
        let journal = UndoJournal::reversal(&[
            FileDeed::Copied {
                from: path(r"C:\a\x.txt"),
                to: path(r"C:\b\x.txt"),
            },
            FileDeed::Moved {
                from: path(r"C:\a\y.txt"),
                to: path(r"D:\c\y.txt"),
            },
            FileDeed::Renamed {
                from: path(r"C:\a\old"),
                to: path(r"C:\a\new"),
            },
            FileDeed::Recycled {
                from: path(r"C:\a\z.txt"),
                item: bin_item(),
            },
            FileDeed::Erased {
                from: path(r"C:\a\gone.txt"),
            },
            FileDeed::Created {
                path: path(r"C:\a\New folder"),
                made: Made::Folder,
            },
        ]);
        assert_eq!(
            journal.steps,
            [
                Step::Delete {
                    path: path(r"C:\a\New folder"),
                    recycle: true
                },
                Step::Restore {
                    item: bin_item(),
                    to: path(r"C:\a\z.txt")
                },
                Step::Rename {
                    path: path(r"C:\a\new"),
                    name: "old".into()
                },
                Step::Move {
                    from: path(r"D:\c\y.txt"),
                    into: path(r"C:\a"),
                    name: None
                },
                Step::Delete {
                    path: path(r"C:\b\x.txt"),
                    recycle: true
                },
            ]
        );
        assert!(journal.unjournaled);
    }

    #[test]
    fn a_copy_under_a_new_name_is_done_again_under_it() {
        let again = UndoJournal::repetition(&[FileDeed::Copied {
            from: path(r"C:\a\x.txt"),
            to: path(r"C:\a\x - Copy.txt"),
        }]);
        assert_eq!(
            again.steps,
            [Step::Copy {
                from: path(r"C:\a\x.txt"),
                into: path(r"C:\a"),
                name: Some("x - Copy.txt".into())
            }]
        );
    }

    #[test]
    fn work_done_again_repeats_each_deed() {
        let again = UndoJournal::repetition(&[
            FileDeed::Recycled {
                from: path(r"C:\a\z.txt"),
                item: bin_item(),
            },
            FileDeed::Created {
                path: path(r"C:\a\notes.txt"),
                made: Made::File { template: None },
            },
            FileDeed::Restored {
                to: path(r"C:\a\back.txt"),
            },
        ]);
        assert_eq!(
            again.steps,
            [
                Step::Delete {
                    path: path(r"C:\a\z.txt"),
                    recycle: true
                },
                Step::New {
                    into: path(r"C:\a"),
                    name: "notes.txt".into(),
                    made: Made::File { template: None }
                },
            ]
        );
    }

    #[test]
    fn the_journal_keeps_what_can_be_taken_back() {
        let mut journal = UndoJournal::default();
        journal.record(vec![FileDeed::Erased {
            from: path(r"C:\a"),
        }]);
        assert!(journal.done.is_empty());
        journal.undone.push(Vec::new());
        journal.record(vec![
            FileDeed::Erased {
                from: path(r"C:\a"),
            },
            FileDeed::Created {
                path: path(r"C:\b"),
                made: Made::Folder,
            },
        ]);
        assert_eq!(journal.done.len(), 1);
        assert_eq!(journal.done[0].len(), 1);
        assert!(
            journal.undone.is_empty(),
            "new work ends what could be redone"
        );
        for _ in 0..JOURNAL_DEPTH + 5 {
            journal.record(vec![FileDeed::Created {
                path: path(r"C:\c"),
                made: Made::Folder,
            }]);
        }
        assert_eq!(journal.done.len(), JOURNAL_DEPTH);
    }

    fn folder(text: &str) -> Vec<FileDeed> {
        vec![FileDeed::Created {
            path: path(text),
            made: Made::Folder,
        }]
    }

    #[test]
    fn nothing_is_taken_back_while_work_runs() {
        let mut journal = UndoJournal::default();
        journal.record(folder(r"C:\a"));
        journal.running = 1;
        assert_eq!(journal.take(false), None);
        journal.running = 0;
        let (deeds, generation) = journal.take(false).unwrap();
        assert_eq!(deeds, folder(r"C:\a"));
        assert_eq!(journal.running, 1, "the undo itself runs");
        assert_eq!(journal.take(false), None);
        journal.after_undo(deeds, generation, false);
        assert_eq!(journal.undone, [folder(r"C:\a")]);
    }

    #[test]
    fn an_undo_that_ends_after_new_work_is_dropped() {
        let mut journal = UndoJournal::default();
        journal.record(folder(r"C:\a"));
        let (deeds, generation) = journal.take(false).unwrap();
        journal.record(folder(r"C:\b"));
        journal.after_undo(deeds, generation, false);
        assert!(journal.undone.is_empty(), "nothing older is redone");
        assert_eq!(journal.done, [folder(r"C:\b")]);
        journal.running = 0;
        let (deeds, generation) = journal.take(false).unwrap();
        journal.record(folder(r"C:\c"));
        journal.after_undo(deeds, generation, true);
        assert_eq!(journal.done, [folder(r"C:\c")]);
    }

    #[test]
    fn a_redo_goes_back_where_it_was_taken_from() {
        let mut journal = UndoJournal::default();
        journal.undone.push(folder(r"C:\a"));
        let (deeds, generation) = journal.take(true).unwrap();
        journal.after_redo(deeds.clone(), Vec::new(), generation, true);
        assert_eq!(journal.undone, [folder(r"C:\a")]);
        journal.running = 0;
        let (deeds, generation) = journal.take(true).unwrap();
        journal.after_redo(deeds, folder(r"C:\a (2)"), generation, false);
        assert_eq!(journal.done, [folder(r"C:\a (2)")]);
        assert!(journal.undone.is_empty());
    }

    #[test]
    fn steps_are_cut_into_passes_that_can_do_them() {
        let steps = [
            Step::Copy {
                from: path(r"C:\a"),
                into: path(r"C:\b"),
                name: None,
            },
            Step::Delete {
                path: path(r"C:\c"),
                recycle: true,
            },
            Step::Rename {
                path: path(r"C:\d"),
                name: "e".into(),
            },
            Step::Delete {
                path: path(r"C:\f"),
                recycle: false,
            },
            Step::Restore {
                item: bin_item(),
                to: path(r"C:\g"),
            },
            Step::Restore {
                item: bin_item(),
                to: path(r"C:\h"),
            },
        ];
        assert_eq!(
            passes(&steps),
            [
                (Pass::Shell(Some(true)), 0..3),
                (Pass::Shell(Some(false)), 3..4),
                (Pass::Restore, 4..6),
            ]
        );
        assert!(passes(&[]).is_empty());
    }

    #[test]
    fn a_copy_into_its_own_folder_is_renamed() {
        let into_own = [Step::Copy {
            from: path(r"C:\a\x.txt"),
            into: path(r"c:\A\"),
            name: None,
        }];
        assert_ne!(flags_of(&into_own, true).0 & FOF_RENAMEONCOLLISION.0, 0);
        let elsewhere = [Step::Copy {
            from: path(r"C:\a\x.txt"),
            into: path(r"C:\b"),
            name: None,
        }];
        let flags = flags_of(&elsewhere, false).0;
        assert_eq!(flags & FOF_RENAMEONCOLLISION.0, 0);
        assert_eq!(flags & FOF_ALLOWUNDO.0, 0, "erasing is for good");
        assert_ne!(flags_of(&elsewhere, true).0 & FOFX_RECYCLEONDELETE.0, 0);
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let taken = ["a.txt", "a (2).txt", "folder", ".hidden"];
        let is_taken = |name: &str| taken.contains(&name);
        assert_eq!(unique_name("b.txt", is_taken), "b.txt");
        assert_eq!(unique_name("a.txt", is_taken), "a (3).txt");
        assert_eq!(unique_name("folder", is_taken), "folder (2)");
        assert_eq!(unique_name(".hidden", is_taken), ".hidden (2)");
    }

    #[test]
    fn paths_are_compared_as_windows_does() {
        assert!(same_path(
            Path::new(r"C:\Users\A"),
            Path::new(r"c:\users\a\")
        ));
        assert!(same_path(Path::new("C:/x/y"), Path::new(r"C:\x\y")));
        assert!(!same_path(Path::new(r"C:\x"), Path::new(r"C:\xy")));
    }
}
