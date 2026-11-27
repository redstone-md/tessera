// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the taskbar's jump lists know: the documents an application opened
//! lately and how often, which Windows records for every application by
//! its identifier, the documents the user pinned there, and the
//! categories and tasks an application put in its list.
//!
//! Windows reads none of this out for another application but the recent
//! documents ([`RecentDocument`]). The rest is read from the two files
//! Explorer keeps for each application below the roaming profile, named by
//! a checksum of the identifier of the application:
//!
//! - `AutomaticDestinations\<checksum>.automaticDestinations-ms`, what
//!   Windows recorded by itself: a compound file (the format of the old
//!   Office documents) with a stream `DestList` that lists the documents
//!   with whether and where they are pinned, how often and when last they
//!   were opened, and a stream for each of them, named by its number in
//!   hexadecimal, that holds a shortcut to it;
//! - `CustomDestinations\<checksum>.customDestinations-ms`, the list the
//!   application made with `ICustomDestinationList`: its categories, each
//!   with a name and shortcuts, the known categories it asked for
//!   (Frequent or Recent) and its tasks.
//!
//! Neither layout is documented. They were worked out by others from the
//! files of many machines (the descriptions of the `libyal` project are the
//! most complete); what is read here is what those agree on, and a file that
//! does not fit is read as far as it fits.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use anyhow::{Context as _, bail};
use windows::Win32::Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_Title};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistStream};
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::Shell::Common::IObjectArray;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{
    ADLT_RECENT, ApplicationDestinations, ApplicationDocumentLists, IApplicationDestinations,
    IApplicationDocumentLists, ILFree, IShellItem, IShellLinkW, SHCreateItemFromIDList,
    SHCreateItemFromParsingName, SHCreateMemStream, SHLoadIndirectString,
    SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY, ShellItem, ShellLink,
};
use windows::core::{BSTR, HSTRING, IUnknown, Interface, PCWSTR};

use super::AppWindowId;
use super::IconPixels;
use super::execute::Launch;
use super::packaged::take;
use super::program_path::expand_variables;
use super::registry::registry_dword;

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

/// Where Windows keeps what it recorded of each application by itself.
const AUTOMATIC_LISTS: &str = r"Microsoft\Windows\Recent\AutomaticDestinations";

/// How such a file starts: its version, the number of its parts, and a
/// number that is not looked at.
const LIST_HEADER: usize = 12;

/// The kinds of the parts of a list: a category of the application's own,
/// a known category, and the tasks.
const CATEGORY: u32 = 0;
const KNOWN: u32 = 1;
const TASKS: u32 = 2;

/// The numbers a known category goes by (`KNOWNDESTCATEGORY`).
const KNOWN_FREQUENT: u32 = 1;
const KNOWN_RECENT: u32 = 2;

/// What ends each part of a list.
const PART_END: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];

/// The identifier of the class of shortcuts (`CLSID_ShellLink`), as it
/// stands before each shortcut a list holds.
const LINK_CLASS: [u8; 16] = [
    0x01, 0x14, 0x02, 0, 0, 0, 0, 0, 0xC0, 0, 0, 0, 0, 0, 0, 0x46,
];

/// The identifier of the class of the items of the Shell
/// (`CLSID_ShellItem`), as it stands before each item a list holds: an
/// application that lists its documents as items instead of shortcuts.
const ITEM_CLASS: [u8; 16] = [
    0xE1, 0xFB, 0xC9, 0x9A, 0xA2, 0xE0, 0xD6, 0x4A, 0xB4, 0xEE, 0xE2, 0x12, 0x01, 0x3E, 0xA9, 0x17,
];

/// How a shortcut in a list starts: the identifier of its class, then the
/// shortcut as it would be in a file of its own, which starts with the
/// size of its header and that identifier again.
const LINK_START: [u8; 36] = {
    let mut start = [0; 36];
    let mut index = 0;
    while index < 16 {
        start[index] = LINK_CLASS[index];
        start[20 + index] = LINK_CLASS[index];
        index += 1;
    }
    start[16] = 0x4C;
    start
};

/// The size of the header of a shortcut, which its first four bytes say.
const LINK_HEADER: usize = 0x4C;

/// Where the flags of a shortcut are, which say what follows the header.
const LINK_FLAGS: usize = 0x14;

/// The flags of a shortcut that say it holds an item list, a block about
/// where the target is, which of the texts it holds, and that these are in
/// UTF-16.
const HAS_ITEM_LIST: u32 = 0x1;
const HAS_LINK_INFO: u32 = 0x2;
const LINK_TEXTS: [u32; 5] = [0x4, 0x8, 0x10, 0x20, 0x40];
const IS_UNICODE: u32 = 0x80;

/// The stream of a recorded list that lists its documents.
const DEST_LIST: &str = "DestList";

/// How long the header of that stream is.
const DEST_HEADER: usize = 32;

/// The longest text read from a shortcut in a list.
const LINK_TEXT: usize = 1024;

/// The key and value of the setting "Show recently opened items in Jump
/// Lists"; it is on unless it is 0.
const ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
const TRACK_DOCUMENTS: &str = "Start_TrackDocs";

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
        let Some(list) = read_list(CUSTOM_LISTS, "customDestinations-ms", app_id) else {
            return Vec::new();
        };
        let tasks = custom_parts(&list).into_iter().find_map(|part| match part {
            Part::Tasks(links) => Some(links),
            _ => None,
        });
        { tasks.unwrap_or_default().into_iter() }
            .filter_map(Saved::link)
            .filter_map(LinkFacts::load)
            .filter_map(|facts| facts.task())
            .collect()
    }

    /// Does the task, as the jump list would. A failure is reported.
    pub fn start(&self) {
        execute(&self.program, &self.arguments, &self.name);
    }
}

/// A known category of a jump list, which Windows fills from its record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KnownCategory {
    /// The documents opened most often.
    Frequent,
    /// The documents opened last.
    Recent,
}

/// What a click on an item of a jump list opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpTarget {
    /// A file, a folder or another item of the Shell, by its path or the
    /// name the Shell parses, for the application to open.
    Item(PathBuf),
    /// A program, started with arguments.
    Command { program: PathBuf, arguments: String },
}

/// One entry of a jump list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpItem {
    /// The name the jump list shows.
    pub name: String,
    pub target: JumpTarget,
    /// The file and the number of the icon the entry asks for, if it
    /// names one.
    pub icon: Option<(PathBuf, i32)>,
    /// Whether Windows recorded the entry by itself (Recent and Frequent),
    /// so that it can be taken off the list.
    pub recorded: bool,
}

impl JumpItem {
    /// Opens the entry, as a click on it in the jump list would: a
    /// document with `program`, the program of the application, or else
    /// the way the Shell opens it; a command as it is. A failure is
    /// reported.
    pub fn open(&self, program: Option<&Path>) {
        match (&self.target, program) {
            (JumpTarget::Command { program, arguments }, _) => {
                execute(program, arguments, &self.name);
            }
            (JumpTarget::Item(path), Some(program)) => {
                execute(program, &format!("\"{}\"", path.display()), &self.name);
            }
            (JumpTarget::Item(path), None) => Launch::new(path).named(&self.name).start(),
        }
    }

    /// Whether [`Self::remove`] can take the entry off its list: a
    /// document Windows recorded by itself. A recorded shortcut that
    /// starts a program with arguments cannot be named to Windows by a
    /// path, so it stays.
    pub fn removable(&self) -> bool {
        self.recorded && matches!(self.target, JumpTarget::Item(_))
    }

    /// Takes the entry off the jump list of `app_id`, from both Recent and
    /// Frequent, with `IApplicationDestinations`, which is documented for
    /// what Windows recorded. Pinned entries and those of the
    /// application's own categories are not taken off by it. To be called
    /// where COM is initialized.
    pub fn remove(&self, app_id: &str) -> anyhow::Result<()> {
        let (JumpTarget::Item(path), true) = (&self.target, self.removable()) else {
            bail!("only a recorded document can be taken off a jump list");
        };
        // SAFETY: COM is initialized on the calling thread; the strings
        // outlive the calls, and every interface is released when it goes
        // out of scope.
        unsafe {
            let lists: IApplicationDestinations =
                CoCreateInstance(&ApplicationDestinations, None, CLSCTX_INPROC_SERVER)
                    .context("Windows gave no jump lists")?;
            lists
                .SetAppID(&HSTRING::from(app_id))
                .context("the application was not found")?;
            let item: IShellItem =
                SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)
                    .context("the document is not there")?;
            lists
                .RemoveDestination(&item.cast::<IUnknown>()?)
                .context("Windows did not take the document off the list")
        }
    }

    /// The pixels of the icon to show for the entry, `size` pixels wide
    /// and high: the one it asks for, or else that of its file or program.
    /// To be called where COM is initialized.
    pub fn icon_pixels(&self, size: u32) -> Option<IconPixels> {
        let asked = { self.icon.as_ref() }.and_then(|(file, index)| {
            IconPixels::of_file(file, *index, size).or_else(|| IconPixels::of_item(file, size))
        });
        asked.or_else(|| match &self.target {
            JumpTarget::Item(path) => IconPixels::of_item(path, size),
            JumpTarget::Command { program, .. } => IconPixels::of_file(program, 0, size),
        })
    }
}

/// The jump list of an application as Explorer would show it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JumpList {
    /// What the user pinned, in the order of the pins.
    pub pinned: Vec<JumpItem>,
    /// What Windows recorded, opened most often first.
    pub frequent: Vec<JumpItem>,
    /// The same, opened last first.
    pub recent: Vec<JumpItem>,
    /// Which of the two the list shows, in its order: those the
    /// application asked for, or Recent where it made no list of its own;
    /// none while the user switched the record off.
    pub known: Vec<KnownCategory>,
    /// The categories the application made, each with its name.
    pub categories: Vec<(String, Vec<JumpItem>)>,
    /// The tasks the application put in its list.
    pub tasks: Vec<JumpItem>,
}

/// When the files of a list last changed, and whether the user lets
/// Windows record documents: what has the list read again when it changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    automatic: Option<SystemTime>,
    custom: Option<SystemTime>,
    tracked: bool,
}

impl Stamp {
    fn of_app(app_id: &str) -> Self {
        let changed = |folder: &str, kind: &str| {
            let path = list_path(folder, kind, app_id)?;
            fs::metadata(path).and_then(|file| file.modified()).ok()
        };
        Self {
            automatic: changed(AUTOMATIC_LISTS, "automaticDestinations-ms"),
            custom: changed(CUSTOM_LISTS, "customDestinations-ms"),
            tracked: registry_dword(HKEY_CURRENT_USER, ADVANCED, TRACK_DOCUMENTS) != Some(0),
        }
    }
}

/// The lists read before, by the identifier of the application, with the
/// stamp of their files then.
type ReadLists = HashMap<String, (Stamp, Arc<JumpList>)>;
static READ: Mutex<Option<ReadLists>> = Mutex::new(None);

impl JumpList {
    /// The jump list of the application `app_id`, read from the files of
    /// Explorer; empty where the application has none. It reads files and
    /// asks COM to load shortcuts: not for the thread of the interface,
    /// and to be called where COM is initialized.
    pub fn of_app(app_id: &str) -> Self {
        Self::read(app_id, Stamp::of_app(app_id).tracked)
    }

    /// The same, read again only when one of its files changed since it
    /// was read last, or the user switched the record on or off.
    pub fn cached(app_id: &str) -> Arc<Self> {
        let stamp = Stamp::of_app(app_id);
        {
            let cache = READ.lock().unwrap_or_else(PoisonError::into_inner);
            let known = cache.as_ref().and_then(|cache| cache.get(app_id));
            if let Some((_, list)) = known.filter(|(was, _)| *was == stamp) {
                return list.clone();
            }
        }
        let list = Arc::new(Self::of_app(app_id));
        { READ.lock().unwrap_or_else(PoisonError::into_inner) }
            .get_or_insert_default()
            .insert(app_id.to_owned(), (stamp, list.clone()));
        list
    }

    fn read(app_id: &str, tracked: bool) -> Self {
        let mut list = Self::default();
        let recorded = { list_path(AUTOMATIC_LISTS, "automaticDestinations-ms", app_id) }
            .and_then(|path| fs::File::open(path).ok())
            .and_then(recorded_links)
            .unwrap_or_default();
        let (pinned, frequent, recent) = arranged(&recorded);
        let item = |recorded: &(Recorded, Vec<u8>)| {
            let facts = LinkFacts::load_destination(Saved::Link(&recorded.1));
            let item = facts.and_then(|facts| facts.destination(true));
            item.or_else(|| Recorded::item(recorded))
        };
        list.pinned = pinned.into_iter().filter_map(item).collect();
        if tracked {
            list.frequent = frequent.into_iter().filter_map(item).collect();
            list.recent = recent.into_iter().filter_map(item).collect();
        }
        let custom = read_list(CUSTOM_LISTS, "customDestinations-ms", app_id);
        let parts = custom.as_deref().map(custom_parts);
        for part in parts.iter().flatten() {
            match part {
                Part::Category(name, links) => {
                    let items = { links.iter() }
                        .filter_map(|saved| LinkFacts::load_destination(*saved)?.destination(false))
                        .collect::<Vec<_>>();
                    if !items.is_empty() {
                        list.categories.push((spelled_out(name), items));
                    }
                }
                Part::Tasks(links) => {
                    list.tasks = { links.iter() }
                        .filter_map(|saved| LinkFacts::load(saved.link()?)?.task_item())
                        .collect();
                }
                Part::Known(_) => {}
            }
        }
        if tracked {
            list.known = known_shown(parts.as_deref());
        }
        list
    }

    /// The entries of the known category `known`.
    pub fn of_known(&self, known: KnownCategory) -> &[JumpItem] {
        match known {
            KnownCategory::Frequent => &self.frequent,
            KnownCategory::Recent => &self.recent,
        }
    }
}

/// The path of the list of `app_id` of the kind `kind` in `folder` below
/// the roaming profile.
fn list_path(folder: &str, kind: &str, app_id: &str) -> Option<PathBuf> {
    let profile = env::var_os("APPDATA")?;
    let name = format!("{:016x}.{kind}", list_name(app_id));
    Some(Path::new(&profile).join(folder).join(name))
}

/// The bytes of that list, unless it cannot be read.
fn read_list(folder: &str, kind: &str, app_id: &str) -> Option<Vec<u8>> {
    fs::read(list_path(folder, kind, app_id)?).ok()
}

/// What a shortcut in a list says, as COM loads it.
#[derive(Default)]
struct LinkFacts {
    /// The file the shortcut points at, where it is one.
    path: Option<String>,
    arguments: Option<String>,
    description: Option<String>,
    /// The title the application gave it, in its property store.
    title: Option<String>,
    icon: Option<(PathBuf, i32)>,
    /// The name the Shell shows for the item the shortcut points at.
    shown: Option<String>,
    /// The name the Shell parses for that item, also where it is no file.
    parsed: Option<String>,
}

impl LinkFacts {
    /// Loads the shortcut `link` holds, without asking the Shell about the
    /// item it points at: enough for a task, which names its program.
    /// `None` where it is none.
    fn load(link: &[u8]) -> Option<Self> {
        Self::shortcut(link).map(|(facts, _)| facts)
    }

    /// Loads `saved` as a destination, also with what the Shell calls the
    /// item it is or points at, which may wait on the network or on a
    /// drive for an item there. `None` where it is no shortcut or item.
    fn load_destination(saved: Saved) -> Option<Self> {
        match saved {
            Saved::Link(link) => {
                let (mut facts, shortcut) = Self::shortcut(link)?;
                // SAFETY: COM is initialized on the calling thread, and the
                // item list is freed once the item is made of it.
                let item = unsafe {
                    let list = shortcut.GetIDList().ok().filter(|list| !list.is_null());
                    list.and_then(|list| {
                        let item = SHCreateItemFromIDList::<IShellItem>(list).ok();
                        ILFree(Some(list));
                        item
                    })
                };
                if let Some(item) = item {
                    facts.name_item(&item);
                }
                Some(facts)
            }
            Saved::Item(data) => {
                // SAFETY: COM is initialized on the calling thread; the
                // stream copies the bytes, and the item is loaded by the
                // class that saved it.
                let item = unsafe {
                    let stream = SHCreateMemStream(Some(data))?;
                    let saved: IPersistStream =
                        CoCreateInstance(&ShellItem, None, CLSCTX_INPROC_SERVER).ok()?;
                    saved.Load(&stream).ok()?;
                    saved.cast::<IShellItem>().ok()?
                };
                // SAFETY: the item is a loaded item of the Shell.
                let path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) };
                let mut facts = Self {
                    path: path.ok().and_then(take),
                    ..Self::default()
                };
                facts.name_item(&item);
                Some(facts)
            }
        }
    }

    /// Notes what the Shell shows and parses as the name of `item`.
    fn name_item(&mut self, item: &IShellItem) {
        // SAFETY: the item is a loaded item of the Shell; each name the
        // Shell gives is taken over and freed by `take`.
        unsafe {
            self.shown = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok().and_then(take);
            self.parsed = { item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING).ok() }.and_then(take);
        }
    }

    /// The shortcut `link` holds, loaded, with what it says.
    fn shortcut(link: &[u8]) -> Option<(Self, IShellLinkW)> {
        let mut path = [0u16; LINK_TEXT];
        let mut arguments = [0u16; LINK_TEXT];
        let mut description = [0u16; LINK_TEXT];
        let mut icon = [0u16; LINK_TEXT];
        let mut index = 0;
        // SAFETY: COM is initialized on the calling thread; the stream
        // copies the bytes, and each buffer is passed with its length.
        unsafe {
            let stream = SHCreateMemStream(Some(link))?;
            let shortcut: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            shortcut.cast::<IPersistStream>().ok()?.Load(&stream).ok()?;
            // Any of these may be missing; the buffer then stays empty.
            let _ = shortcut.GetPath(&mut path, ptr::null_mut(), 0);
            let _ = shortcut.GetArguments(&mut arguments);
            let _ = shortcut.GetDescription(&mut description);
            let _ = shortcut.GetIconLocation(&mut icon, &raw mut index);
            let store = shortcut.cast::<IPropertyStore>().ok();
            let title = store
                .and_then(|store| store.GetValue(&PKEY_Title).ok())
                .and_then(|title| BSTR::try_from(&title).ok())
                .map(|title| title.to_string())
                .filter(|title| !title.is_empty());
            let facts = Self {
                path: text_in(&path),
                arguments: text_in(&arguments),
                description: text_in(&description),
                title,
                icon: text_in(&icon).map(|file| (PathBuf::from(expand_variables(&file)), index)),
                shown: None,
                parsed: None,
            };
            Some((facts, shortcut))
        }
    }

    /// The name to show: the title, or else the description, or else
    /// what the Shell calls the item, or else the name of the file.
    fn name(&self) -> Option<String> {
        let title = self.title.as_deref().map(spelled_out);
        let stem = || {
            let path = Path::new(self.path.as_deref().or(self.parsed.as_deref())?);
            Some(path.file_stem()?.to_string_lossy().into_owned())
        };
        title
            .or_else(|| self.description.as_deref().map(spelled_out))
            .or_else(|| self.shown.clone())
            .or_else(stem)
    }

    /// The task of the shortcut; `None` for one that names no program,
    /// such as a separator.
    fn task(self) -> Option<JumpTask> {
        let program = PathBuf::from(self.path.as_deref()?);
        let name = self.name()?;
        Some(JumpTask {
            name,
            program,
            arguments: self.arguments.unwrap_or_default(),
        })
    }

    /// The same as an entry of a jump list, with its icon.
    fn task_item(self) -> Option<JumpItem> {
        let icon = self.icon.clone();
        let task = self.task()?;
        Some(JumpItem {
            name: task.name,
            target: JumpTarget::Command {
                program: task.program,
                arguments: task.arguments,
            },
            icon,
            recorded: false,
        })
    }

    /// The shortcut as a destination: a document or folder for the
    /// application to open, or, where it starts a program, that command.
    /// `recorded` says whether Windows recorded it by itself.
    fn destination(self, recorded: bool) -> Option<JumpItem> {
        let name = self.name()?;
        let is_program = |path: &str| {
            let extension = Path::new(path).extension();
            extension.is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        };
        let target = match (&self.path, &self.arguments) {
            (Some(path), Some(_)) => JumpTarget::Command {
                program: path.into(),
                arguments: self.arguments.clone().unwrap_or_default(),
            },
            (Some(path), None) if is_program(path) && !recorded => JumpTarget::Command {
                program: path.into(),
                arguments: String::new(),
            },
            (Some(path), None) => JumpTarget::Item(path.into()),
            (None, _) => JumpTarget::Item(self.parsed.as_deref()?.into()),
        };
        Some(JumpItem {
            name,
            target,
            icon: self.icon,
            recorded,
        })
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

/// The number of two bytes at `at` of `bytes`, little end first.
fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// The number of four bytes at `at`.
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// The number of eight bytes at `at`.
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(*bytes.get(at..)?.first_chunk()?))
}

/// `units` characters of UTF-16 at `at`.
fn utf16_at(bytes: &[u8], at: usize, units: usize) -> Option<String> {
    let text = bytes.get(at..at.checked_add(units * 2)?)?;
    let units: Vec<u16> = { text.chunks_exact(2) }
        .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// How many bytes the shortcut at the start of `link` takes: its header,
/// its item list, its block about the target, its texts and its blocks of
/// further data up to the one that ends them, as the format of shortcut
/// files (MS-SHLLINK, which is documented) lays them out. `None` where the
/// bytes are no whole shortcut.
fn link_length(link: &[u8]) -> Option<usize> {
    if u32_at(link, 0)? as usize != LINK_HEADER {
        return None;
    }
    let flags = u32_at(link, LINK_FLAGS)?;
    let mut at = LINK_HEADER;
    if flags & HAS_ITEM_LIST != 0 {
        at += 2 + usize::from(u16_at(link, at)?);
    }
    if flags & HAS_LINK_INFO != 0 {
        at += u32_at(link, at)? as usize;
    }
    let unit = if flags & IS_UNICODE != 0 { 2 } else { 1 };
    for text in LINK_TEXTS {
        if flags & text != 0 {
            at += 2 + usize::from(u16_at(link, at)?) * unit;
        }
    }
    // Each block of further data starts with its size; a size below four
    // ends them.
    loop {
        let size = u32_at(link, at)? as usize;
        if size < 4 {
            at += 4;
            break;
        }
        at += size;
    }
    (at <= link.len()).then_some(at)
}

/// A part of a list an application made.
#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    /// A category of its own: its name and its entries.
    Category(String, Vec<Saved<'a>>),
    Known(KnownCategory),
    /// The tasks, which are shortcuts.
    Tasks(Vec<Saved<'a>>),
}

/// An entry of a part of a list, as the object it was saved from saved
/// itself, by the class of that object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Saved<'a> {
    /// A shortcut (`IShellLink`).
    Link(&'a [u8]),
    /// An item of the Shell (`IShellItem`).
    Item(&'a [u8]),
}

impl<'a> Saved<'a> {
    /// The bytes of the shortcut, where it is one.
    fn link(self) -> Option<&'a [u8]> {
        match self {
            Self::Link(link) => Some(link),
            Self::Item(_) => None,
        }
    }
}

/// The parts of `list`, a list an application made, in their order. A
/// part that does not fit what is known of the layout ends the reading.
fn custom_parts(list: &[u8]) -> Vec<Part<'_>> {
    let mut parts = Vec::new();
    let count = u32_at(list, 4).unwrap_or(0);
    let mut rest = list.get(LIST_HEADER..).unwrap_or_default();
    for _ in 0..count {
        let Some((kind, body)) = rest.split_first_chunk::<4>() else {
            break;
        };
        let read = match u32::from_le_bytes(*kind) {
            CATEGORY => (|| {
                let units = usize::from(u16_at(body, 0)?);
                let name = utf16_at(body, 2, units)?;
                let (links, after) = shortcuts(body.get(2 + units * 2..)?);
                Some((Some(Part::Category(name, links)), after))
            })(),
            KNOWN => u32_at(body, 0).map(|known| {
                let known = match known {
                    KNOWN_FREQUENT => Some(Part::Known(KnownCategory::Frequent)),
                    KNOWN_RECENT => Some(Part::Known(KnownCategory::Recent)),
                    _ => None,
                };
                (known, &body[4..])
            }),
            TASKS => {
                let (links, after) = shortcuts(body);
                Some((Some(Part::Tasks(links)), after))
            }
            _ => None,
        };
        let Some((part, after)) = read else {
            break;
        };
        // The part ends where it says; where the shortcuts could not be
        // measured, at the next end of a part.
        let Some(end) = position(after, &PART_END) else {
            break;
        };
        parts.extend(part);
        rest = &after[end + PART_END.len()..];
    }
    parts
}

/// The entries at the start of `body`, which begins with their number,
/// and what follows them. Each entry is the identifier of a class and what
/// the object of that class saved. A shortcut is measured by its layout;
/// an item, whose layout is not described, and a shortcut that cannot be
/// measured are taken up to where the next entry or the end of the part
/// begins. An entry of another class is passed over in the same way.
fn shortcuts(body: &[u8]) -> (Vec<Saved<'_>>, &[u8]) {
    let mut entries = Vec::new();
    let count = u32_at(body, 0).unwrap_or(0);
    let mut rest = body.get(4..).unwrap_or_default();
    for _ in 0..count {
        if rest.starts_with(&PART_END) {
            break;
        }
        let Some((class, data)) = rest.split_first_chunk::<16>() else {
            break;
        };
        let length = match *class {
            LINK_CLASS => link_length(data),
            _ => None,
        };
        let length = length.unwrap_or_else(|| unmeasured(data));
        let saved = &data[..length];
        entries.extend(match *class {
            LINK_CLASS => Some(Saved::Link(saved)),
            ITEM_CLASS => Some(Saved::Item(saved)),
            _ => None,
        });
        rest = &data[length..];
    }
    (entries, rest)
}

/// How long the entry that starts `data` is taken to be where its length
/// is not known: up to where the next shortcut or item or the end of the
/// part begins, whichever comes first, or else all of `data`.
fn unmeasured(data: &[u8]) -> usize {
    let after_first = |pattern: &[u8]| {
        let next = position(data.get(1..)?, pattern)?;
        Some(next + 1)
    };
    [
        after_first(&LINK_START),
        after_first(&ITEM_CLASS),
        position(data, &PART_END),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(data.len())
}

/// Which known categories a list shows, given the parts of the list the
/// application made, if it made one: those it asked for, each once, or
/// Recent where it made none.
fn known_shown(parts: Option<&[Part]>) -> Vec<KnownCategory> {
    let Some(parts) = parts else {
        return vec![KnownCategory::Recent];
    };
    let mut known = Vec::new();
    for part in parts {
        if let Part::Known(category) = part
            && !known.contains(category)
        {
            known.push(*category);
        }
    }
    known
}

/// Where `pattern` first is in `bytes`.
fn position(bytes: &[u8], pattern: &[u8]) -> Option<usize> {
    bytes
        .windows(pattern.len())
        .position(|window| window == pattern)
}

/// A document as the stream `DestList` of a recorded list has it.
#[derive(Clone, Debug, PartialEq)]
struct Recorded {
    /// The number of the stream that holds the shortcut to it.
    stream: u32,
    /// Its place among the pinned documents, if it is pinned.
    pinned: Option<u32>,
    /// How often it was opened.
    uses: u32,
    /// When it was opened last, in the hundreds of nanoseconds Windows
    /// counts from 1601.
    used: u64,
    /// Its path or the address of the item.
    path: String,
}

impl Recorded {
    /// The document as an entry of a jump list, by its path alone, for
    /// when its shortcut cannot be loaded.
    fn item((entry, _): &(Recorded, Vec<u8>)) -> Option<JumpItem> {
        let path = PathBuf::from(&entry.path);
        let name = path.file_name()?.to_string_lossy().into_owned();
        Some(JumpItem {
            name,
            target: JumpTarget::Item(path),
            icon: None,
            recorded: true,
        })
    }
}

/// The documents `list`, a stream `DestList`, names. Its header gives the
/// version of the layout and the number of entries. Each entry has a
/// checksum, four identifiers of the document for the link tracking
/// service, the name of the computer, the number of its stream, then at 96
/// how often it was opened (as a floating point number in the first
/// version), at 100 when last, at 108 its place among the pins (-1 for
/// none), and in later versions at 116 how often it was opened as a whole
/// number. Its path follows, after the number of its characters, and in
/// later versions four bytes more.
fn recorded_entries(list: &[u8]) -> Vec<Recorded> {
    let (Some(version), Some(count)) = (u32_at(list, 0), u32_at(list, 4)) else {
        return Vec::new();
    };
    let (fixed, trailer) = if version <= 1 { (114, 0) } else { (130, 4) };
    let mut entries = Vec::new();
    let mut at = DEST_HEADER;
    for _ in 0..count {
        let read = (|| {
            let entry = list.get(at..)?;
            let units = usize::from(u16_at(entry, fixed - 2)?);
            let path = utf16_at(entry, fixed, units)?;
            let uses = if version <= 1 {
                let uses = f32::from_bits(u32_at(entry, 96)?);
                uses.round().clamp(0., u32::MAX as f32) as u32
            } else {
                u32_at(entry, 116)?
            };
            let pin = u32_at(entry, 108)?;
            let recorded = Recorded {
                stream: u32_at(entry, 88)?,
                pinned: (pin != u32::MAX).then_some(pin),
                uses,
                used: u64_at(entry, 100)?,
                path,
            };
            Some((recorded, fixed + units * 2 + trailer))
        })();
        let Some((recorded, length)) = read else {
            break;
        };
        entries.push(recorded);
        at += length;
    }
    entries
}

/// The documents of the recorded list in `file`, a compound file, each
/// with the bytes of its shortcut; `None` where it is no such list.
fn recorded_links<F: Read + Seek>(file: F) -> Option<Vec<(Recorded, Vec<u8>)>> {
    let mut compound = cfb::CompoundFile::open(file).ok()?;
    let mut list = Vec::new();
    compound
        .open_stream(format!("/{DEST_LIST}"))
        .ok()?
        .read_to_end(&mut list)
        .ok()?;
    let entries = recorded_entries(&list);
    let links = entries.into_iter().filter_map(|entry| {
        let mut link = Vec::new();
        let mut stream = compound.open_stream(format!("/{:x}", entry.stream)).ok()?;
        stream.read_to_end(&mut link).ok()?;
        Some((entry, link))
    });
    Some(links.collect())
}

/// The pinned documents of `entries` in the order of their pins, and the
/// others twice: opened most often first, and opened last first.
fn arranged<T: AsRef<Recorded>>(entries: &[T]) -> (Vec<&T>, Vec<&T>, Vec<&T>) {
    let (mut pinned, others): (Vec<&T>, Vec<&T>) =
        { entries.iter() }.partition(|entry| entry.as_ref().pinned.is_some());
    pinned.sort_by_key(|entry| entry.as_ref().pinned);
    let mut frequent = others.clone();
    frequent.sort_by(|first, second| {
        let (first, second) = (first.as_ref(), second.as_ref());
        (second.uses, second.used).cmp(&(first.uses, first.used))
    });
    let mut recent = others;
    recent.sort_by_key(|entry| std::cmp::Reverse(entry.as_ref().used));
    (pinned, frequent, recent)
}

impl AsRef<Recorded> for Recorded {
    fn as_ref(&self) -> &Recorded {
        self
    }
}

impl AsRef<Recorded> for (Recorded, Vec<u8>) {
    fn as_ref(&self) -> &Recorded {
        &self.0
    }
}

/// Starts `program` with `arguments` the way the shell does, with
/// [`Launch`]. A failure is reported under `name`.
fn execute(program: &Path, arguments: &str, name: &str) {
    Launch::new(program)
        .verb("open")
        .arguments(arguments)
        .named(name)
        .start();
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
        let document = format!("\"{}\"", self.path.display());
        execute(program, &document, &self.name);
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

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

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    /// A shortcut with the flags `flags`: an item list of `items` bytes,
    /// a block about the target of `info` bytes, the texts in `texts` and
    /// one block of further data of `extra` bytes.
    fn link(flags: u32, items: u16, info: u32, texts: &[&str], extra: u32) -> Vec<u8> {
        let mut link = vec![0; LINK_HEADER];
        link[..4].copy_from_slice(&(LINK_HEADER as u32).to_le_bytes());
        link[4..20].copy_from_slice(&LINK_START[..16]);
        link[LINK_FLAGS..LINK_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        if flags & HAS_ITEM_LIST != 0 {
            link.extend(items.to_le_bytes());
            link.extend(vec![7; usize::from(items)]);
        }
        if flags & HAS_LINK_INFO != 0 {
            link.extend(info.to_le_bytes());
            link.extend(vec![8; info as usize - 4]);
        }
        for text in texts {
            link.extend((text.encode_utf16().count() as u16).to_le_bytes());
            link.extend(utf16(text));
        }
        if extra > 0 {
            link.extend(extra.to_le_bytes());
            link.extend(vec![9; extra as usize - 4]);
        }
        link.extend(0u32.to_le_bytes());
        link
    }

    #[test]
    fn a_shortcut_is_measured_by_its_layout() {
        let bare = link(0, 0, 0, &[], 0);
        assert_eq!(link_length(&bare), Some(LINK_HEADER + 4));
        let flags = HAS_ITEM_LIST | HAS_LINK_INFO | 0x20 | 0x40 | IS_UNICODE;
        let full = link(flags, 20, 28, &["--new-window", r"C:\app.exe"], 16);
        assert_eq!(link_length(&full), Some(full.len()));
        let mut longer = full.clone();
        longer.extend([1, 2, 3]);
        assert_eq!(link_length(&longer), Some(full.len()));
        assert_eq!(link_length(&full[..full.len() - 1]), None);
        assert_eq!(link_length(&[0; 8]), None);
    }

    /// A list an application made, of `parts`.
    fn custom_list(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut list = 2u32.to_le_bytes().to_vec();
        list.extend((parts.len() as u32).to_le_bytes());
        list.extend(0u32.to_le_bytes());
        for part in parts {
            list.extend(part);
            list.extend(PART_END);
        }
        list
    }

    fn with_links(mut part: Vec<u8>, links: &[&[u8]]) -> Vec<u8> {
        part.extend((links.len() as u32).to_le_bytes());
        for link in links {
            part.extend(&LINK_START[..16]);
            part.extend(*link);
        }
        part
    }

    #[test]
    fn the_parts_of_a_list_are_read_in_their_order() {
        let first = link(0x20 | IS_UNICODE, 0, 0, &["one"], 0);
        let second = link(HAS_ITEM_LIST, 6, 0, &[], 12);
        let task = link(0, 0, 0, &[], 0);
        let mut category = CATEGORY.to_le_bytes().to_vec();
        category.extend(5u16.to_le_bytes());
        category.extend(utf16("Media"));
        let category = with_links(category, &[&first, &second]);
        let mut known = KNOWN.to_le_bytes().to_vec();
        known.extend(KNOWN_FREQUENT.to_le_bytes());
        let tasks = with_links(TASKS.to_le_bytes().to_vec(), &[&task]);
        let list = custom_list(&[category, known, tasks]);
        let parts = custom_parts(&list);
        assert_eq!(
            parts,
            [
                Part::Category(
                    "Media".to_owned(),
                    vec![Saved::Link(&first), Saved::Link(&second)]
                ),
                Part::Known(KnownCategory::Frequent),
                Part::Tasks(vec![Saved::Link(&task)]),
            ]
        );
        assert_eq!(known_shown(Some(&parts)), [KnownCategory::Frequent]);
        assert_eq!(known_shown(None), [KnownCategory::Recent]);
        assert_eq!(known_shown(Some(&parts[2..])), []);
    }

    #[test]
    fn shortcuts_that_cannot_be_measured_are_taken_up_to_the_next() {
        let odd = |mark: u8| [&LINK_START[16..], &[mark; 5]].concat();
        let mut tasks = TASKS.to_le_bytes().to_vec();
        tasks.extend(2u32.to_le_bytes());
        tasks.extend(&LINK_START[..16]);
        tasks.extend(odd(2));
        tasks.extend(&LINK_START[..16]);
        tasks.extend(odd(3));
        let mut other = 1u32.to_le_bytes().to_vec();
        other.extend(7u32.to_le_bytes());
        let list = custom_list(&[other, tasks]);
        let parts = custom_parts(&list);
        assert_eq!(
            parts,
            [Part::Tasks(vec![
                Saved::Link(&odd(2)),
                Saved::Link(&odd(3))
            ])]
        );
        assert!(custom_parts(&list[..list.len() - 2]).is_empty());
        assert!(custom_parts(&[]).is_empty());
    }

    #[test]
    fn a_list_cut_short_after_the_class_of_a_shortcut_is_read_as_far_as_it_goes() {
        let mut tasks = TASKS.to_le_bytes().to_vec();
        tasks.extend(1u32.to_le_bytes());
        tasks.extend(&LINK_START[..16]);
        let mut list = custom_list(&[tasks]);
        list.truncate(list.len() - PART_END.len());
        assert_eq!(
            shortcuts(&list[LIST_HEADER + 4..]),
            (vec![Saved::Link(&[])], &[][..])
        );
        assert!(custom_parts(&list).is_empty());
    }

    #[test]
    fn items_and_entries_of_other_classes_are_passed_over_to_the_next() {
        let first = link(0x20 | IS_UNICODE, 0, 0, &["one"], 0);
        let item = [5u8; 12];
        let other = [6u8; 9];
        let last = link(0, 0, 0, &[], 0);
        let mut category = CATEGORY.to_le_bytes().to_vec();
        category.extend(4u16.to_le_bytes());
        category.extend(utf16("Docs"));
        category.extend(4u32.to_le_bytes());
        category.extend(&LINK_START[..16]);
        category.extend(&first);
        category.extend([0x11; 16]);
        category.extend(other);
        category.extend(ITEM_CLASS);
        category.extend(item);
        category.extend(&LINK_START[..16]);
        category.extend(&last);
        let mut known = KNOWN.to_le_bytes().to_vec();
        known.extend(KNOWN_RECENT.to_le_bytes());
        let list = custom_list(&[category, known]);
        assert_eq!(
            custom_parts(&list),
            [
                Part::Category(
                    "Docs".to_owned(),
                    vec![Saved::Link(&first), Saved::Item(&item), Saved::Link(&last)]
                ),
                Part::Known(KnownCategory::Recent),
            ]
        );
        // An item that ends its part is taken up to the end of the part.
        let mut tasks = TASKS.to_le_bytes().to_vec();
        tasks.extend(1u32.to_le_bytes());
        tasks.extend(ITEM_CLASS);
        tasks.extend(item);
        assert_eq!(
            custom_parts(&custom_list(&[tasks])),
            [Part::Tasks(vec![Saved::Item(&item)])]
        );
    }

    #[test]
    fn only_a_recorded_document_can_be_taken_off() {
        let document = JumpItem {
            name: "notes".to_owned(),
            target: JumpTarget::Item(PathBuf::from(
                r"C:
otes.txt",
            )),
            icon: None,
            recorded: true,
        };
        let command = JumpItem {
            target: JumpTarget::Command {
                program: PathBuf::from(r"C:pp.exe"),
                arguments: "--open notes".to_owned(),
            },
            ..document.clone()
        };
        let own = JumpItem {
            recorded: false,
            ..document.clone()
        };
        assert!(document.removable());
        assert!(!command.removable());
        assert!(!own.removable());
    }

    /// An entry of a `DestList` of `version`.
    fn dest_entry(
        version: u32,
        stream: u32,
        pin: i32,
        uses: u32,
        used: u64,
        path: &str,
    ) -> Vec<u8> {
        let mut entry = vec![0; 130];
        entry[88..92].copy_from_slice(&stream.to_le_bytes());
        entry[100..108].copy_from_slice(&used.to_le_bytes());
        entry[108..112].copy_from_slice(&pin.to_le_bytes());
        let units = path.encode_utf16().count() as u16;
        if version <= 1 {
            entry[96..100].copy_from_slice(&(uses as f32).to_le_bytes());
            entry.truncate(114);
            entry[112..114].copy_from_slice(&units.to_le_bytes());
            entry.extend(utf16(path));
        } else {
            entry[116..120].copy_from_slice(&uses.to_le_bytes());
            entry[128..130].copy_from_slice(&units.to_le_bytes());
            entry.extend(utf16(path));
            entry.extend([0; 4]);
        }
        entry
    }

    fn dest_list(version: u32, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut list = vec![0; DEST_HEADER];
        list[..4].copy_from_slice(&version.to_le_bytes());
        list[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for entry in entries {
            list.extend(entry);
        }
        list
    }

    #[test]
    fn the_record_of_documents_is_read_in_both_layouts() {
        for version in [1, 4] {
            let list = dest_list(
                version,
                &[
                    dest_entry(version, 1, -1, 3, 500, r"C:\a.txt"),
                    dest_entry(version, 2, 0, 1, 100, r"C:\pinned.txt"),
                ],
            );
            let entries = recorded_entries(&list);
            assert_eq!(entries.len(), 2, "version {version}");
            assert_eq!(entries[0].stream, 1);
            assert_eq!(entries[0].pinned, None);
            assert_eq!(entries[0].uses, 3);
            assert_eq!(entries[0].used, 500);
            assert_eq!(entries[0].path, r"C:\a.txt");
            assert_eq!(entries[1].pinned, Some(0));
            assert_eq!(entries[1].path, r"C:\pinned.txt");
            assert_eq!(recorded_entries(&list[..list.len() - 3]).len(), 1);
        }
        assert!(recorded_entries(&[1, 2]).is_empty());
    }

    #[test]
    fn pins_come_in_their_order_and_the_rest_by_use() {
        let entry = |stream, pinned, uses, used| Recorded {
            stream,
            pinned,
            uses,
            used,
            path: String::new(),
        };
        let entries = [
            entry(1, None, 1, 900),
            entry(2, Some(1), 9, 100),
            entry(3, None, 5, 200),
            entry(4, Some(0), 1, 50),
            entry(5, None, 5, 300),
        ];
        let (pinned, frequent, recent) = arranged(&entries);
        let streams =
            |list: Vec<&Recorded>| list.iter().map(|entry| entry.stream).collect::<Vec<_>>();
        assert_eq!(streams(pinned), [4, 2]);
        assert_eq!(streams(frequent), [5, 3, 1]);
        assert_eq!(streams(recent), [1, 5, 3]);
    }

    #[test]
    fn a_recorded_list_is_read_from_its_compound_file() {
        let mut compound = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        let list = dest_list(
            4,
            &[
                dest_entry(4, 0x1a, -1, 2, 10, r"C:\one.txt"),
                dest_entry(4, 0x2, -1, 1, 20, r"C:\gone.txt"),
            ],
        );
        compound
            .create_stream("/DestList")
            .unwrap()
            .write_all(&list)
            .unwrap();
        let shortcut = link(0, 0, 0, &[], 0);
        compound
            .create_stream("/1a")
            .unwrap()
            .write_all(&shortcut)
            .unwrap();
        compound.flush().unwrap();
        let mut file = compound.into_inner();
        file.set_position(0);
        let links = recorded_links(file).unwrap();
        // The second document has no stream, and is left out.
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].0.path, r"C:\one.txt");
        assert_eq!(links[0].1, shortcut);
        assert!(recorded_links(Cursor::new(vec![0; 600])).is_none());
    }

    /// Prints how many tasks the applications of the open windows put in
    /// their jump lists. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "reads the record of the machine it runs on"]
    fn tasks_of_open_windows() {
        // SAFETY: COM is initialized once for the thread of the test.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
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

    /// Prints what the jump lists of the applications of the open windows
    /// hold, as counts. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "reads the record of the machine it runs on"]
    fn jump_lists_of_open_windows() {
        // SAFETY: COM is initialized once for the thread of the test.
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
        }
        for window in super::super::AppWindow::enumerate() {
            let id = { window.app_id.clone() }
                .or_else(|| window.id.stated_app_id())
                .or_else(|| window.executable.as_deref().map(app_id_of_program));
            if let Some(id) = id {
                let list = JumpList::of_app(&id);
                println!(
                    "{} pinned, {} frequent, {} recent, shown {:?}, {} categories, {} tasks",
                    list.pinned.len(),
                    list.frequent.len(),
                    list.recent.len(),
                    list.known,
                    list.categories.len(),
                    list.tasks.len()
                );
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
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            );
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
