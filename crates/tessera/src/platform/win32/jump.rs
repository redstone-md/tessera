// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the taskbar's jump lists know: the documents an application opened
//! lately, which Windows records for every application by its identifier.

use std::env;
use std::path::{Path, PathBuf};
use std::thread;

use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::IObjectArray;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    ADLT_RECENT, ApplicationDocumentLists, IApplicationDocumentLists, IShellItem,
    SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{BSTR, HSTRING, PCWSTR, w};

use super::AppWindowId;
use super::packaged::take;

/// The directories Windows names by an identifier instead of their path
/// when it makes up the identifier of an application from its program
/// file, each with the environment variable that tells where it is and
/// what follows below that. The longer paths come first.
const KNOWN_FOLDERS: [(&str, &str, &str); 5] = [
    (
        "SystemRoot",
        "System32",
        "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}",
    ),
    (
        "SystemRoot",
        "SysWOW64",
        "{D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27}",
    ),
    ("SystemRoot", "", "{F38BF404-1D43-42F2-9305-67DE0B28FC23}"),
    ("ProgramW6432", "", "{6D809377-6AF0-444B-8957-A3773F02200E}"),
    (
        "ProgramFiles(x86)",
        "",
        "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}",
    ),
];

/// A document an application opened lately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentDocument {
    /// The name the shell shows for it.
    pub name: String,
    pub path: PathBuf,
}

impl AppWindowId {
    /// The identifier the application gave this window or its process to
    /// be told from others by; `None` for the many that give none.
    pub fn stated_app_id(self) -> Option<String> {
        // SAFETY: COM is initialized on the calling thread, and the call
        // tolerates a window that has since been destroyed.
        let id = unsafe {
            let store: IPropertyStore = SHGetPropertyStoreForWindow(self.hwnd()).ok()?;
            let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
            BSTR::try_from(&value).ok()?.to_string()
        };
        (!id.is_empty()).then_some(id)
    }
}

/// The identifier Windows makes up for the application whose program file
/// is `executable` and which states none: the path of the file, with a
/// well-known directory at its start replaced by the identifier of that
/// directory.
pub fn app_id_of_program(executable: &Path) -> String {
    let folders = KNOWN_FOLDERS.iter().filter_map(|&(variable, below, id)| {
        let folder = PathBuf::from(env::var_os(variable)?).join(below);
        Some((folder, id))
    });
    relative_to_known(executable, folders)
}

fn relative_to_known<'a>(
    executable: &Path,
    folders: impl IntoIterator<Item = (PathBuf, &'a str)>,
) -> String {
    let whole = executable.to_string_lossy();
    for (folder, id) in folders {
        let folder = folder.to_string_lossy();
        let folder = folder.trim_end_matches('\\');
        let Some((start, rest)) = whole.split_at_checked(folder.len()) else {
            continue;
        };
        if start.eq_ignore_ascii_case(folder) && rest.starts_with('\\') {
            return format!("{id}{rest}");
        }
    }
    whole.into_owned()
}

impl RecentDocument {
    /// Up to `most` documents the application `app_id` opened lately, the
    /// latest first, as Windows recorded them; empty when it recorded
    /// none, does not tell, or the user switched the record off.
    pub fn of_app(app_id: &str, most: u32) -> Vec<Self> {
        Self::read(app_id, most).unwrap_or_default()
    }

    fn read(app_id: &str, most: u32) -> windows::core::Result<Vec<Self>> {
        let mut documents = Vec::new();
        // SAFETY: COM is initialized on the calling thread; every interface
        // is released when it goes out of scope.
        unsafe {
            let lists: IApplicationDocumentLists =
                CoCreateInstance(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER)?;
            lists.SetAppID(&HSTRING::from(app_id))?;
            let items: IObjectArray = lists.GetList(ADLT_RECENT, most)?;
            for index in 0..items.GetCount()? {
                // What is not a file, such as a link with a command line,
                // is left out.
                let Ok(item) = items.GetAt::<IShellItem>(index) else {
                    continue;
                };
                let path = item.GetDisplayName(SIGDN_FILESYSPATH).ok().and_then(take);
                let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok().and_then(take);
                if let (Some(path), Some(name)) = (path, name) {
                    documents.push(Self {
                        name,
                        path: path.into(),
                    });
                }
            }
        }
        Ok(documents)
    }

    /// Opens the document with `program`, as the jump list of that
    /// program would. A failure is reported.
    ///
    /// The work is done on a thread of its own, as the shell handles
    /// window messages while it starts a program.
    pub fn open_with(&self, program: &Path) {
        let program = HSTRING::from(program.as_os_str());
        let document = HSTRING::from(format!("\"{}\"", self.path.display()));
        let name = self.name.clone();
        thread::spawn(move || {
            // SAFETY: COM is initialized for the duration of the call on
            // this thread and released only when that succeeded; the
            // strings are NUL-terminated and outlive the call.
            let instance = unsafe {
                let initialized =
                    CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
                let instance = ShellExecuteW(
                    None,
                    w!("open"),
                    PCWSTR(program.as_ptr()),
                    PCWSTR(document.as_ptr()),
                    None,
                    SW_SHOWNORMAL,
                );
                if initialized {
                    CoUninitialize();
                }
                instance
            };
            // The call reports success as a value above 32.
            if instance.0 as usize <= 32 {
                report!("{name} did not open: error {}", instance.0 as usize);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> Vec<(PathBuf, &'static str)> {
        vec![
            (PathBuf::from(r"C:\Windows\System32"), "{system}"),
            (PathBuf::from(r"C:\Windows"), "{windows}"),
            (PathBuf::from(r"C:\Program Files\"), "{programs}"),
        ]
    }

    #[test]
    fn a_well_known_directory_is_named_by_its_identifier() {
        let id = |path: &str| relative_to_known(Path::new(path), folders());
        assert_eq!(
            id(r"C:\Windows\System32\notepad.exe"),
            r"{system}\notepad.exe"
        );
        assert_eq!(id(r"c:\windows\explorer.exe"), r"{windows}\explorer.exe");
        assert_eq!(
            id(r"C:\Program Files\App\app.exe"),
            r"{programs}\App\app.exe"
        );
    }

    #[test]
    fn a_program_elsewhere_is_named_by_its_path() {
        let id = |path: &str| relative_to_known(Path::new(path), folders());
        assert_eq!(id(r"D:\tools\app.exe"), r"D:\tools\app.exe");
        assert_eq!(id(r"C:\WindowsOld\app.exe"), r"C:\WindowsOld\app.exe");
    }

    /// Prints how many recent documents Windows has on record for the
    /// windows that are open. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "reads the record of the machine it runs on"]
    fn recent_documents_of_open_windows() {
        // SAFETY: COM is initialized once for the thread of the test.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        for window in super::super::AppWindow::enumerate() {
            let stated = window.id.stated_app_id();
            let made_up = window.executable.as_deref().map(app_id_of_program);
            for (kind, id) in [("stated", stated), ("made up", made_up)] {
                if let Some(id) = id {
                    let found = RecentDocument::of_app(&id, 10).len();
                    println!("{kind}: {found} for {} characters of id", id.len());
                }
            }
        }
    }
}
