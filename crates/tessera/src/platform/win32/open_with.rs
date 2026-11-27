// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! "Open with": the programs and applications Windows knows can open a
//! kind of file, as Explorer lists them, opening files with one of them,
//! and the dialog of Windows that lets the user choose another.
//!
//! The handlers come from `SHAssocEnumHandlers`: their names, their icons
//! and whether Windows recommends them for the kind. A handler opens files
//! through `IAssocHandler::Invoke`, which starts its program or activates
//! its packaged application as Explorer does. Everything runs on the
//! [`ShellThread`]; the dialog runs on a thread of its own, as it waits
//! for the user.
//!
//! ```ignore
//! let handlers = OpenWithHandler::of(&path);
//! cx.spawn(async move |cx| {
//!     let handlers = handlers.recv().await.unwrap_or_default();
//!     // ... show them; once one is chosen:
//!     handlers[0].open(vec![path]);
//! }).detach();
//! ```
#![allow(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use async_channel::Receiver;
use windows::Win32::Foundation::{ERROR_CANCELLED, S_OK};
use windows::Win32::UI::Shell::{
    ASSOC_FILTER_NONE, IAssocHandler, IEnumAssocHandlers, OAIF_ALLOW_REGISTRATION, OAIF_EXEC,
    OAIF_REGISTER_EXT, OPENASINFO, SHAssocEnumHandlers, SHOpenWithDialog,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, PWSTR};

use super::ShellThread;
use super::shell_items::{
    IconPlace, data_object, dialog_owner, take_path, take_text, windows_path,
};

/// A program or application that can open a kind of file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWithHandler {
    /// What Windows knows the handler by: the path of its program, or the
    /// identifier of its packaged application.
    pub id: String,
    /// What it is called, as the menu shows it.
    pub name: String,
    pub icon: Option<IconPlace>,
    /// Whether Windows recommends it for the kind: it registered for it,
    /// or the user opened such files with it.
    pub recommended: bool,
}

#[allow(
    dead_code,
    reason = "for the menu Open with of the file manager and the desktop"
)]
impl OpenWithHandler {
    /// The handlers of the kind of the file at `path`, those Windows
    /// recommends first; none for a folder or a file without an
    /// extension. The receiver yields once, and an empty list where
    /// Windows gives none; failures are reported.
    pub fn of(path: &Path) -> Receiver<Vec<Self>> {
        let path = path.to_owned();
        ShellThread::call(move || {
            let Some(kind) = kind_of(&path) else {
                return Vec::new();
            };
            if path.is_dir() {
                return Vec::new();
            }
            match handlers_of(&kind) {
                Ok(handlers) => {
                    let mut found: Vec<Self> = { handlers.iter() }.filter_map(Self::read).collect();
                    recommended_first(&mut found);
                    found
                }
                Err(error) => {
                    report!("the programs that open {kind} files are not known: {error:#}");
                    Vec::new()
                }
            }
        })
    }

    /// Opens the files at `paths`, all of the kind the handler was found
    /// for, with it. It returns at once; a failure is reported.
    pub fn open(&self, paths: Vec<PathBuf>) {
        let id = self.id.clone();
        let name = self.name.clone();
        ShellThread::run(move || {
            if let Err(error) = open_with(&id, &paths) {
                report!("{name} did not open the files: {error:#}");
            }
        });
    }

    /// The handler as it is shown; `None` for one without a name.
    fn read(handler: &IAssocHandler) -> Option<Self> {
        // SAFETY: the texts the calls give are the caller's to free, which
        // `take_text` and `take_path` do.
        unsafe {
            let id = take_text(handler.GetName().ok()?)?;
            let name = take_text(handler.GetUIName().ok()?).filter(|name| !name.is_empty())?;
            let mut file = PWSTR::null();
            let mut index = 0;
            let icon = match handler.GetIconLocation(&raw mut file, &raw mut index) {
                Ok(()) => take_path(file).and_then(|file| icon_place(&file, index)),
                Err(_) => None,
            };
            let recommended = handler.IsRecommended() == S_OK;
            Some(Self {
                id,
                name,
                icon,
                recommended,
            })
        }
    }
}

/// Has Windows ask the user which program is to open the file at `path`,
/// in its own dialog, and open it with that one; the user may make it the
/// program for all files of the kind there. It returns at once; the
/// dialog runs on a thread of its own. A failure is reported; the user
/// closing the dialog is none.
#[allow(
    dead_code,
    reason = "for the menu Open with of the file manager and the desktop"
)]
pub fn choose_app(path: &Path) {
    let path = path.to_owned();
    ShellThread::run_alone("open with", move || {
        let file = HSTRING::from(windows_path(&path));
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };
        // SAFETY: the structure and the path it points to outlive the
        // call, which shows the dialog and returns once it is closed.
        let chosen = unsafe { SHOpenWithDialog(Some(dialog_owner()), &raw const info) };
        if let Err(error) = chosen
            && error.code() != HRESULT::from_win32(ERROR_CANCELLED.0)
        {
            report!(
                "the dialog to choose a program for {} did not open: {error}",
                path.display()
            );
        }
    });
}

/// What the handlers of a file are asked by: its extension with the dot,
/// in small letters; `None` for a file without one.
fn kind_of(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    (!extension.is_empty()).then(|| format!(".{}", extension.to_lowercase()))
}

/// The handlers recommended first, the others after them, each in the
/// order Windows gave them.
fn recommended_first(handlers: &mut [OpenWithHandler]) {
    handlers.sort_by_key(|handler| !handler.recommended);
}

/// Where the icon of a handler is, as `GetIconLocation` gives it: a
/// program and an index, or a resource string of a package.
fn icon_place(file: &Path, index: i32) -> Option<IconPlace> {
    let text = file.to_string_lossy();
    if text.starts_with('@') {
        return IconPlace::from_location(&text);
    }
    (!text.is_empty()).then(|| IconPlace::Resource {
        file: file.to_owned(),
        index,
    })
}

/// Every handler of `kind`, as Windows lists them.
fn handlers_of(kind: &str) -> Result<Vec<IAssocHandler>> {
    let kind = HSTRING::from(kind);
    // SAFETY: the kind is NUL-terminated and outlives the call; COM is
    // initialized on this thread.
    let listed: IEnumAssocHandlers = unsafe { SHAssocEnumHandlers(&kind, ASSOC_FILTER_NONE) }
        .context("Windows did not list them")?;
    let mut handlers = Vec::new();
    loop {
        let mut slot = [None];
        let mut fetched = 0u32;
        // SAFETY: the slot has room for the one handler asked for, and
        // `fetched` outlives the call.
        let next = unsafe { listed.Next(&mut slot, Some(&raw mut fetched)) };
        match (next, slot) {
            (Ok(()), [Some(handler)]) if fetched == 1 => handlers.push(handler),
            _ => break,
        }
    }
    Ok(handlers)
}

/// Opens `paths` with the handler of their kind that Windows knows by
/// `id`, on this thread.
fn open_with(id: &str, paths: &[PathBuf]) -> Result<()> {
    let Some(first) = paths.first() else {
        bail!("no file was given");
    };
    let kind = kind_of(first).ok_or_else(|| anyhow!("{} has no kind", first.display()))?;
    let found = handlers_of(&kind)?.into_iter().find(|handler| {
        // SAFETY: the name is the caller's to free, which `take_text` does.
        let name = unsafe { handler.GetName().ok().and_then(|name| take_text(name)) };
        name.as_deref() == Some(id)
    });
    let handler = found.context("the program is no longer one that opens such files")?;
    let data = data_object(paths)?;
    // SAFETY: the data object outlives the call; COM is initialized on
    // this thread.
    unsafe { handler.Invoke(&data) }.context("Windows did not start it")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handler(name: &str, recommended: bool) -> OpenWithHandler {
        OpenWithHandler {
            id: name.to_owned(),
            name: name.to_owned(),
            icon: None,
            recommended,
        }
    }

    #[test]
    fn a_kind_is_the_extension_in_small_letters() {
        assert_eq!(
            kind_of(Path::new(r"C:\a\Notes.TXT")).as_deref(),
            Some(".txt")
        );
        assert_eq!(
            kind_of(Path::new(r"C:\a\archive.tar.gz")).as_deref(),
            Some(".gz")
        );
        assert_eq!(kind_of(Path::new(r"C:\a\README")), None);
        assert_eq!(kind_of(Path::new(r"C:\a\.hidden")), None);
    }

    #[test]
    fn the_recommended_handlers_come_first_in_their_order() {
        let mut handlers = vec![
            handler("Paint", false),
            handler("Photos", true),
            handler("Word", false),
            handler("Notepad", true),
        ];
        recommended_first(&mut handlers);
        let names: Vec<&str> = handlers
            .iter()
            .map(|handler| handler.name.as_str())
            .collect();
        assert_eq!(names, ["Photos", "Notepad", "Paint", "Word"]);
    }

    #[test]
    fn a_handler_icon_is_a_program_and_an_index() {
        let place = icon_place(Path::new(r"C:\Windows\notepad.exe"), 2);
        assert_eq!(
            place,
            Some(IconPlace::Resource {
                file: PathBuf::from(r"C:\Windows\notepad.exe"),
                index: 2
            })
        );
        assert_eq!(icon_place(Path::new(""), 0), None);
    }
}
