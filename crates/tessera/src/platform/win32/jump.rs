// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the taskbar's jump lists know: the documents an application opened
//! lately, which Windows records for every application by its identifier,
//! and the tasks an application put in its list.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr;
use std::thread;

use windows::Win32::Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_Title};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoUninitialize, IPersistStream,
};
use windows::Win32::UI::Shell::Common::IObjectArray;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    ADLT_RECENT, ApplicationDocumentLists, IApplicationDocumentLists, IShellItem, IShellLinkW,
    SHCreateMemStream, SHLoadIndirectString, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellExecuteW,
    ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{BSTR, HSTRING, Interface, PCWSTR, w};

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

/// Where Windows keeps the lists applications made themselves, below the
/// roaming profile, one file for each application.
const CUSTOM_LISTS: &str = r"Microsoft\Windows\Recent\CustomDestinations";

/// How such a file starts: its version, the number of its parts, and a
/// number that is not looked at.
const LIST_HEADER: usize = 12;

/// The kind of the part of a list that holds the tasks.
const TASKS: u32 = 2;

/// What ends each part of a list.
const PART_END: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];

/// How a shortcut in a list starts: the identifier of the class of
/// shortcuts, then the shortcut as it would be in a file of its own, which
/// starts with the size of its header and that identifier again.
const LINK_START: [u8; 36] = {
    const CLASS: [u8; 16] = [
        0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
    ];
    let mut start = [0; 36];
    let mut index = 0;
    while index < 16 {
        start[index] = CLASS[index];
        start[20 + index] = CLASS[index];
        index += 1;
    }
    start[16] = 0x4C;
    start
};

/// The longest text read from a shortcut in a list.
const LINK_TEXT: usize = 1024;

/// A task an application put in its jump list, such as opening a new
/// window of a certain kind: a program and what it is started with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpTask {
    pub name: String,
    pub program: PathBuf,
    pub arguments: String,
}

impl JumpTask {
    /// The tasks the application `app_id` put in its jump list; empty
    /// when it made no list or the list cannot be read.
    pub fn of_app(app_id: &str) -> Vec<Self> {
        let Some(profile) = env::var_os("APPDATA") else {
            return Vec::new();
        };
        let name = format!("{:016x}.customDestinations-ms", list_name(app_id));
        let Ok(list) = fs::read(Path::new(&profile).join(CUSTOM_LISTS).join(name)) else {
            return Vec::new();
        };
        task_links(&list)
            .into_iter()
            .filter_map(Self::read)
            .collect()
    }

    /// The task of a shortcut as a list holds it; `None` for one that
    /// names no program, such as a separator.
    fn read(link: &[u8]) -> Option<Self> {
        let mut program = [0u16; LINK_TEXT];
        let mut arguments = [0u16; LINK_TEXT];
        let mut description = [0u16; LINK_TEXT];
        // SAFETY: COM is initialized on the calling thread; the stream
        // copies the bytes, and each buffer is passed with its length.
        let title = unsafe {
            let stream = SHCreateMemStream(Some(link))?;
            let shortcut: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            shortcut.cast::<IPersistStream>().ok()?.Load(&stream).ok()?;
            // Any of these may be missing; the buffer then stays empty.
            let _ = shortcut.GetPath(&mut program, ptr::null_mut(), 0);
            let _ = shortcut.GetArguments(&mut arguments);
            let _ = shortcut.GetDescription(&mut description);
            let store = shortcut.cast::<IPropertyStore>().ok();
            store
                .and_then(|store| store.GetValue(&PKEY_Title).ok())
                .and_then(|title| BSTR::try_from(&title).ok())
                .map(|title| title.to_string())
        };
        let program = PathBuf::from(text_in(&program)?);
        let name = title
            .filter(|title| !title.is_empty())
            .map(|title| spelled_out(&title))
            .or_else(|| text_in(&description))
            .or_else(|| Some(program.file_stem()?.to_string_lossy().into_owned()))?;
        Some(Self {
            name,
            program,
            arguments: text_in(&arguments).unwrap_or_default(),
        })
    }

    /// Does the task, as the jump list would. A failure is reported.
    pub fn start(&self) {
        let program = HSTRING::from(self.program.as_os_str());
        execute(program, HSTRING::from(&self.arguments), self.name.clone());
    }
}

/// The text in a NUL-terminated buffer, unless it is empty.
fn text_in(buffer: &[u16]) -> Option<String> {
    let length = buffer.iter().position(|&unit| unit == 0)?;
    (length > 0).then(|| String::from_utf16_lossy(&buffer[..length]))
}

/// A name that may point into the resources of a program, which starts
/// with `@`, as the text it points to; the name itself if that fails.
fn spelled_out(name: &str) -> String {
    if !name.starts_with('@') {
        return name.to_owned();
    }
    let source = HSTRING::from(name);
    let mut text = [0u16; LINK_TEXT];
    // SAFETY: the source is NUL-terminated and the buffer carries its
    // length.
    let loaded = unsafe { SHLoadIndirectString(PCWSTR(source.as_ptr()), &mut text, None) };
    loaded
        .ok()
        .and_then(|()| text_in(&text))
        .unwrap_or_else(|| name.to_owned())
}

/// The number Windows names the file of the list of an application by: a
/// checksum of its identifier in capitals.
fn list_name(app_id: &str) -> u64 {
    const POLYNOMIAL: u64 = 0x92C6_4265_D321_39A4;
    let bytes = app_id
        .to_uppercase()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<u8>>();
    bytes.into_iter().fold(u64::MAX, |sum, byte| {
        (0..8).fold(sum ^ u64::from(byte), |sum, _| {
            if sum & 1 == 1 {
                (sum >> 1) ^ POLYNOMIAL
            } else {
                sum >> 1
            }
        })
    })
}

/// The shortcuts in the part of `list` that holds the tasks, each from
/// its start to where the next one or the end of the part begins.
fn task_links(list: &[u8]) -> Vec<&[u8]> {
    let mut links = Vec::new();
    let mut rest = list.get(LIST_HEADER..).unwrap_or_default();
    while let Some((kind, part)) = rest.split_first_chunk::<4>() {
        let Some(end) = position(part, &PART_END) else {
            break;
        };
        if u32::from_le_bytes(*kind) == TASKS {
            let mut part = &part[..end];
            while let Some(start) = position(part, &LINK_START) {
                let link = &part[start + 16..];
                let length = position(&link[1..], &LINK_START).map_or(link.len(), |next| next + 1);
                links.push(&link[..length]);
                part = &link[length..];
            }
        }
        rest = &part[end + PART_END.len()..];
    }
    links
}

/// Where `pattern` first is in `bytes`.
fn position(bytes: &[u8], pattern: &[u8]) -> Option<usize> {
    bytes
        .windows(pattern.len())
        .position(|window| window == pattern)
}

/// Starts `program` with `arguments` the way the shell does. A failure is
/// reported under `name`.
///
/// The work is done on a thread of its own, as the shell handles window
/// messages while it starts a program.
fn execute(program: HSTRING, arguments: HSTRING, name: String) {
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
                PCWSTR(arguments.as_ptr()),
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

/// The identifier Explorer goes by.
const EXPLORER: &str = "Microsoft.Windows.Explorer";

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
/// directory. Explorer is the exception: its windows state no identifier,
/// yet Windows keeps its lists under a name.
pub fn app_id_of_program(executable: &Path) -> String {
    let explorer = env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("explorer.exe"));
    let whole = executable.as_os_str();
    if explorer.is_some_and(|explorer| explorer.as_os_str().eq_ignore_ascii_case(whole)) {
        return EXPLORER.to_owned();
    }
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
    pub fn open_with(&self, program: &Path) {
        let program = HSTRING::from(program.as_os_str());
        let document = HSTRING::from(format!("\"{}\"", self.path.display()));
        execute(program, document, self.name.clone());
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

    #[test]
    fn the_file_of_a_list_is_named_by_a_checksum_of_the_identifier() {
        assert_eq!(list_name("Chrome"), 0x5d69_6d52_1de2_38c3);
        assert_eq!(list_name("chrome"), list_name("CHROME"));
    }

    #[test]
    fn only_the_shortcuts_among_the_tasks_are_taken() {
        let link = |mark: u8| [&LINK_START[..], &[mark; 5]].concat();
        let mut list = vec![0; LIST_HEADER];
        // A part of another kind, with a shortcut of its own.
        list.extend(1u32.to_le_bytes());
        list.extend(link(1));
        list.extend(PART_END);
        list.extend(TASKS.to_le_bytes());
        list.extend(2u32.to_le_bytes());
        list.extend(link(2));
        list.extend(link(3));
        list.extend(PART_END);
        let links = task_links(&list);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0], [&LINK_START[16..], &[2; 5]].concat());
        assert_eq!(links[1], [&LINK_START[16..], &[3; 5]].concat());
        assert!(task_links(&list[..list.len() - 2]).len() <= 2);
        assert!(task_links(&[]).is_empty());
    }

    /// Prints how many tasks the applications of the open windows put in
    /// their jump lists. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "reads the record of the machine it runs on"]
    fn tasks_of_open_windows() {
        // SAFETY: COM is initialized once for the thread of the test.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        for window in super::super::AppWindow::enumerate() {
            let stated = window.id.stated_app_id();
            let made_up = window.executable.as_deref().map(app_id_of_program);
            let packaged = window.app_id.clone();
            for (kind, id) in [
                ("packaged", packaged),
                ("stated", stated),
                ("made up", made_up),
            ] {
                if let Some(id) = id {
                    let tasks = JumpTask::of_app(&id);
                    let named = tasks.iter().filter(|task| !task.name.starts_with('@'));
                    let with_arguments = tasks.iter().filter(|task| !task.arguments.is_empty());
                    println!(
                        "{kind}: {} tasks, {} named in words, {} with arguments",
                        tasks.len(),
                        named.count(),
                        with_arguments.count()
                    );
                }
            }
        }
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
