// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The program manager's DDE, answered by Tessera as the shell of the
//! session, so that old installers can make the groups and the shortcuts
//! of the Start menu.
//!
//! Installers of the 1990s, and setup programs built with old tools still
//! in use, talk to the service `PROGMAN`, topic `PROGMAN`, and send
//! commands such as `[CreateGroup(Games)][AddItem(C:\Games\x.exe,X)]`.
//! Explorer answers them, and turns groups into folders and items into
//! shortcuts in the Start menu. Without Explorer nobody answers, and such
//! an installer stops or reports an error.
//!
//! [`ProgmanDde`] registers the names `PROGMAN`, `Folders` and
//! `AppProperties` (those Explorer registers) with DDEML on a thread of
//! its own, and executes the commands:
//!
//! - `CreateGroup(name[,common])` makes the folder `name` in the user's
//!   `Start Menu\Programs`, or with `common` = 1 in the one of all users
//!   where the user may write it (and in the user's otherwise), and makes
//!   it the group the next items go to. `ShowGroup(name,cmd[,common])`
//!   makes it the group too, and shows its folder where `cmd` shows a
//!   window.
//! - `AddItem(cmdline[,name[,icon[,index[,x,y[,dir[,hotkey[,min[,separate]]]]]]]])`
//!   saves a shortcut (`IShellLinkW` with `IPersistFile`) in the group;
//!   `DeleteItem(name)` and `ReplaceItem(name)` remove one (the next
//!   `AddItem` takes the place of the item replaced).
//! - `DeleteGroup(name[,common])` removes the folder.
//! - `Reload` and `ExitProgman` are accepted and do nothing, as Explorer.
//! - `ViewFolder` and `ExploreFolder`, which the `Folders` service once
//!   took for opening folders, show the folder.
//!
//! A request for the item `Groups` gives the groups, one a line. The
//! names are kept inside the Programs folders: a name that leaves them
//! (`..`, a drive) is refused.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IPersistFile,
};
use windows::Win32::System::DataExchange::{
    APPCLASS_STANDARD, CBF_FAIL_ADVISES, CBF_FAIL_POKES, CBF_SKIP_ALLNOTIFICATIONS, CP_WINUNICODE,
    DDE_FACK, DDE_FNOTPROCESSED, DNS_REGISTER, DNS_UNREGISTER, DdeCmpStringHandles,
    DdeCreateDataHandle, DdeCreateStringHandleW, DdeFreeStringHandle, DdeGetData, DdeInitializeW,
    DdeNameService, DdeUninitialize, HCONV, HDDEDATA, HSZ, XTYP_CONNECT, XTYP_DISCONNECT,
    XTYP_EXECUTE, XTYP_REQUEST,
};
use windows::Win32::System::Ole::{CF_TEXT, CF_UNICODETEXT};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Programs, IShellLinkW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW,
    SHOW_WINDOW_CMD, SW_SHOWMINNOACTIVE, SW_SHOWNORMAL, TranslateMessage, WM_QUIT,
};
use windows::core::{HSTRING, Interface as _, w};

use super::shell_items::known_folder;

/// A command of the program manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Command {
    CreateGroup {
        name: String,
        common: bool,
    },
    ShowGroup {
        name: String,
        show: i32,
        common: bool,
    },
    AddItem(Item),
    DeleteItem(String),
    ReplaceItem(String),
    DeleteGroup {
        name: String,
        common: bool,
    },
    Reload,
    ExitProgman,
    /// `ViewFolder` or `ExploreFolder` of the `Folders` service.
    ShowFolder(String),
}

/// What `AddItem` names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Item {
    /// The program and its arguments.
    pub command: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub icon_index: i32,
    pub directory: Option<String>,
    /// As `IShellLinkW::SetHotkey` takes it: the key in the low byte, the
    /// modifiers in the high one.
    pub hotkey: u16,
    pub minimized: bool,
}

/// Why commands were refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The text is not commands in brackets.
    Malformed(String),
    /// A command this server does not know.
    Unknown(String),
    /// A command lacks an argument it needs.
    Missing(&'static str),
}

/// The commands of an execute, as `[Name(arg,arg)][Name]`.
pub(super) fn parse(text: &str) -> Result<Vec<Command>, Refusal> {
    let mut commands = Vec::new();
    let mut rest = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    while !rest.is_empty() {
        let Some(inside) = rest.strip_prefix('[') else {
            return Err(Refusal::Malformed(rest.to_owned()));
        };
        let (name, arguments, after) = split_command(inside)?;
        commands.push(command_of(&name, arguments)?);
        rest = after.trim_start();
    }
    Ok(commands)
}

/// The name, the arguments as written (quotes kept, spaces around them
/// not) and what follows of a command after its `[`.
fn split_command(text: &str) -> Result<(String, Vec<String>, &str), Refusal> {
    let malformed = || Refusal::Malformed(text.to_owned());
    let name_end = text.find(['(', ']']).ok_or_else(malformed)?;
    let name = text[..name_end].trim().to_owned();
    let mut arguments = Vec::new();
    let mut at = name_end;
    if text[at..].starts_with('(') {
        at += 1;
        let mut current = String::new();
        let mut quoted = false;
        let mut closed = false;
        for (offset, c) in text[at..].char_indices() {
            match c {
                '"' => {
                    quoted = !quoted;
                    current.push(c);
                }
                ',' | ')' if !quoted => {
                    arguments.push(current.trim().to_owned());
                    current.clear();
                    if c == ')' {
                        at += offset + 1;
                        closed = true;
                        break;
                    }
                }
                c => current.push(c),
            }
        }
        if !closed {
            return Err(malformed());
        }
    }
    let after = text[at..]
        .trim_start()
        .strip_prefix(']')
        .ok_or_else(malformed)?;
    // `Name()` has no arguments rather than one empty one.
    if arguments.len() == 1 && arguments[0].is_empty() {
        arguments.clear();
    }
    Ok((name, arguments, after))
}

/// The command `name` with `arguments` as written.
fn command_of(name: &str, arguments: Vec<String>) -> Result<Command, Refusal> {
    let raw = |index: usize| {
        arguments
            .get(index)
            .filter(|text| !text.is_empty())
            .cloned()
    };
    // Quotes only keep commas and parentheses inside an argument.
    let text = |index: usize| raw(index).map(|text| text.replace('"', ""));
    let number = |index: usize| text(index).and_then(|text| text.trim().parse::<i64>().ok());
    let flag = |index: usize| number(index).is_some_and(|number| number != 0);
    let named = |what| text(0).ok_or(Refusal::Missing(what));
    Ok(match name.to_ascii_lowercase().as_str() {
        "creategroup" => Command::CreateGroup {
            name: named("the name of the group")?,
            common: flag(1),
        },
        "showgroup" => Command::ShowGroup {
            name: named("the name of the group")?,
            show: number(1).unwrap_or(1) as i32,
            common: flag(2),
        },
        // The command line keeps its quotes, which end a program whose
        // path holds spaces.
        "additem" => Command::AddItem(Item {
            command: raw(0).ok_or(Refusal::Missing("the command line of the item"))?,
            name: text(1),
            icon: text(2),
            icon_index: number(3).unwrap_or(0) as i32,
            directory: text(6),
            hotkey: number(7).unwrap_or(0) as u16,
            minimized: flag(8),
        }),
        "deleteitem" => Command::DeleteItem(named("the name of the item")?),
        "replaceitem" => Command::ReplaceItem(named("the name of the item")?),
        "deletegroup" => Command::DeleteGroup {
            name: named("the name of the group")?,
            common: flag(1),
        },
        "reload" => Command::Reload,
        "exitprogman" => Command::ExitProgman,
        "viewfolder" | "explorefolder" | "openfolder" => Command::ShowFolder(named("the folder")?),
        _ => return Err(Refusal::Unknown(name.to_owned())),
    })
}

/// The folder `name` names under `root`, where it stays inside: its parts
/// are split at backslashes and slashes, and one that leaves (`..`), names
/// a drive or holds a character a name may not hold is refused.
pub(super) fn inside(root: &Path, name: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    let mut any = false;
    for part in name.split(['\\', '/']).map(str::trim) {
        if part.is_empty() || part == "." {
            continue;
        }
        let forbidden = |c: char| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || c < ' ';
        if part == ".." || part.contains(forbidden) {
            return None;
        }
        // A part of dots and spaces only names nothing Windows keeps.
        if part.trim_end_matches(['.', ' ']).is_empty() {
            return None;
        }
        path.push(part);
        any = true;
    }
    let stays = { path.components() }.all(|part| !matches!(part, Component::ParentDir));
    (any && stays).then_some(path)
}

/// The name a shortcut file of an item is saved under, without `.lnk`:
/// characters a name may not hold become `_`.
pub(super) fn file_name_of(name: &str) -> String {
    let cleaned: String = { name.chars() }
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c < ' ' => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_owned();
    match cleaned.is_empty() {
        true => "_".to_owned(),
        false => cleaned,
    }
}

/// The program of a command line and its arguments. A quoted program
/// ends at its quote; an unquoted one at the first space after which
/// `exists` finds a file, or at the first space when none is found, as
/// `CreateProcess` reads an unquoted path with spaces.
pub(super) fn split_command_line(line: &str, exists: impl Fn(&str) -> bool) -> (String, String) {
    let line = line.trim();
    if let Some(quoted) = line.strip_prefix('"') {
        return match quoted.split_once('"') {
            Some((program, rest)) => (program.to_owned(), rest.trim().to_owned()),
            None => (quoted.to_owned(), String::new()),
        };
    }
    let spaces: Vec<usize> = { line.char_indices() }
        .filter(|&(_, c)| c == ' ')
        .map(|(at, _)| at)
        .collect();
    for &at in &spaces {
        if exists(&line[..at]) {
            return (line[..at].to_owned(), line[at + 1..].trim().to_owned());
        }
    }
    if exists(line) || spaces.is_empty() {
        return (line.to_owned(), String::new());
    }
    let first = spaces[0];
    (
        line[..first].to_owned(),
        line[first + 1..].trim().to_owned(),
    )
}

/// The text of an execute: DDEML hands Unicode servers Unicode, and the
/// text of an ANSI client as it came; both end in NUL.
pub(super) fn text_of(bytes: &[u8]) -> String {
    let wide = bytes.len() >= 2 && bytes.len().is_multiple_of(2) && bytes[1] == 0;
    let text = match wide {
        true => {
            let units: Vec<u16> = { bytes.chunks_exact(2) }
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .take_while(|&unit| unit != 0)
                .collect();
            String::from_utf16_lossy(&units)
        }
        false => {
            let end = bytes
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        }
    };
    text.trim().to_owned()
}

/// The answer to the item `Groups`: the names, a line each.
pub(super) fn groups_text(groups: &[String]) -> String {
    let mut text = String::new();
    for group in groups {
        text.push_str(group);
        text.push_str("\r\n");
    }
    text
}

/// The program manager's DDE, served by Tessera.
pub struct ProgmanDde;

/// The thread of the server while it runs, by its number.
static THREAD: AtomicU32 = AtomicU32::new(0);
/// Where the folders to show go.
static FOLDERS: OnceLock<(Sender<PathBuf>, Receiver<PathBuf>)> = OnceLock::new();

impl ProgmanDde {
    /// Starts the server on a thread of its own, where it does not run.
    pub fn start() {
        if THREAD.load(Ordering::SeqCst) != 0 {
            return;
        }
        let (ready, thread) = std::sync::mpsc::sync_channel(1);
        let started = thread::Builder::new()
            .name("progman dde".to_owned())
            .spawn(move || serve(ready));
        match started {
            Ok(_) => {
                if let Ok(thread) = thread.recv_timeout(std::time::Duration::from_secs(2)) {
                    THREAD.store(thread, Ordering::SeqCst);
                }
            }
            Err(error) => report!("old installers find no program manager: {error}"),
        }
    }

    /// Stops the server, where it runs.
    pub fn stop() {
        let thread = THREAD.swap(0, Ordering::SeqCst);
        if thread != 0 {
            // SAFETY: a thread that ended refuses the message without harm.
            let _ = unsafe { PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
    }

    /// The folders the commands asked to show, as `ShowGroup` and
    /// `ViewFolder` do; the user interface opens them.
    pub fn folders() -> Receiver<PathBuf> {
        FOLDERS.get_or_init(async_channel::unbounded).1.clone()
    }
}

/// What the server knows on its thread.
struct Server {
    instance: u32,
    /// The names it registered and answers to.
    services: Vec<HSZ>,
    /// The topics it takes.
    topics: Vec<HSZ>,
    groups_item: HSZ,
    /// The group each conversation adds items to.
    current: HashMap<isize, PathBuf>,
}

thread_local! {
    static SERVER: RefCell<Option<Server>> = const { RefCell::new(None) };
}

/// The body of the thread.
fn serve(ready: std::sync::mpsc::SyncSender<u32>) {
    let mut message = MSG::default();
    // SAFETY: COM is initialized for this thread and left at its end; the
    // peek makes the queue of the thread before its number is told.
    let initialized = unsafe {
        let _ = PeekMessageW(&raw mut message, None, 0, 0, PM_NOREMOVE);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok()
    };
    let mut instance = 0u32;
    let commands =
        APPCLASS_STANDARD | CBF_FAIL_POKES | CBF_FAIL_ADVISES | CBF_SKIP_ALLNOTIFICATIONS;
    // SAFETY: `instance` outlives the call; the callback is a function of
    // this module that DDEML calls on this thread only.
    let status = unsafe { DdeInitializeW(&raw mut instance, Some(respond), commands, None) };
    if status != 0 {
        report!("old installers find no program manager: DDE did not start ({status:#x})");
        if initialized {
            // SAFETY: matched with the initialization above.
            unsafe { CoUninitialize() };
        }
        return;
    }
    // The number is told only once DDE runs, so that a server that did
    // not start is not taken for one that runs.
    // SAFETY: the call only reads the number of this thread.
    let _ = ready.send(unsafe { GetCurrentThreadId() });
    let handle = |name| {
        // SAFETY: the instance is initialized and the names are literals.
        unsafe { DdeCreateStringHandleW(instance, name, CP_WINUNICODE) }
    };
    let services = vec![
        handle(w!("PROGMAN")),
        handle(w!("Folders")),
        handle(w!("AppProperties")),
    ];
    let topics = vec![handle(w!("PROGMAN")), handle(w!("AppProperties"))];
    let groups_item = handle(w!("Groups"));
    SERVER.with(|server| {
        *server.borrow_mut() = Some(Server {
            instance,
            services: services.clone(),
            topics: topics.clone(),
            groups_item,
            current: HashMap::new(),
        });
    });
    // The server is known before its names are, so that the first
    // connection finds it.
    for &service in &services {
        // SAFETY: the instance and the name are alive.
        let registered = unsafe { DdeNameService(instance, Some(service), None, DNS_REGISTER) };
        if registered.0.is_null() {
            report!("a name of the program manager could not be registered for DDE");
        }
    }
    // SAFETY: `message` outlives the calls; DDEML's windows get their
    // messages, and the callback is called, from inside them.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
        for &service in &services {
            let _ = DdeNameService(instance, Some(service), None, DNS_UNREGISTER);
        }
        for name in services.into_iter().chain(topics).chain([groups_item]) {
            let _ = DdeFreeStringHandle(instance, name);
        }
        SERVER.with(|server| server.borrow_mut().take());
        let _ = DdeUninitialize(instance);
        if initialized {
            CoUninitialize();
        }
    }
}

/// Whether two string handles name the same text, without case.
fn same(one: HSZ, other: HSZ) -> bool {
    // SAFETY: the call only compares the handles' texts.
    unsafe { DdeCmpStringHandles(one, other) == 0 }
}

/// The callback of DDEML.
unsafe extern "system" fn respond(
    kind: u32,
    format: u32,
    conversation: HCONV,
    first: HSZ,
    second: HSZ,
    data: HDDEDATA,
    _data1: usize,
    _data2: usize,
) -> HDDEDATA {
    let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        SERVER.with(|server| {
            let mut server = server.borrow_mut();
            let server = server.as_mut()?;
            Some(match kind {
                // `first` is the topic, `second` the service.
                kind if kind == XTYP_CONNECT.0 => {
                    let known = { server.services.iter() }.any(|&service| same(service, second))
                        && { server.topics.iter() }.any(|&topic| same(topic, first));
                    HDDEDATA(usize::from(known) as *mut c_void)
                }
                kind if kind == XTYP_DISCONNECT.0 => {
                    server.current.remove(&(conversation.0 as isize));
                    HDDEDATA::default()
                }
                kind if kind == XTYP_EXECUTE.0 => execute(server, conversation, data),
                kind if kind == XTYP_REQUEST.0 => request(server, format, second),
                _ => HDDEDATA::default(),
            })
        })
    }));
    match answer {
        Ok(answer) => answer.unwrap_or_default(),
        Err(_) => {
            report!("a command of the program manager failed");
            HDDEDATA(DDE_FNOTPROCESSED as usize as *mut c_void)
        }
    }
}

/// Executes the commands in `data` for `conversation`.
fn execute(server: &mut Server, conversation: HCONV, data: HDDEDATA) -> HDDEDATA {
    let refused = HDDEDATA(DDE_FNOTPROCESSED as usize as *mut c_void);
    // SAFETY: the handle is the data of this transaction; the first call
    // gives its size, the second copies it into a buffer of that size.
    let bytes = unsafe {
        let size = DdeGetData(data, None, 0);
        let mut bytes = vec![0u8; size as usize];
        DdeGetData(data, Some(&mut bytes), 0);
        bytes
    };
    let text = text_of(&bytes);
    let commands = match parse(&text) {
        Ok(commands) => commands,
        Err(refusal) => {
            report!("the program manager did not take {text:?}: {refusal:?}");
            return refused;
        }
    };
    let key = conversation.0 as isize;
    for command in commands {
        let mut current = server.current.get(&key).cloned();
        let done = run(&command, &mut current);
        if let Some(group) = current {
            server.current.insert(key, group);
        }
        if let Err(error) = done {
            report!("the program manager did not do {command:?}: {error:#}");
            return refused;
        }
    }
    HDDEDATA(DDE_FACK as usize as *mut c_void)
}

/// Answers a request for the item `item`: only `Groups` is known.
fn request(server: &Server, format: u32, item: HSZ) -> HDDEDATA {
    if !same(item, server.groups_item) {
        return HDDEDATA::default();
    }
    let text = groups_text(&groups());
    let bytes: Vec<u8> = match format {
        format if format == u32::from(CF_UNICODETEXT.0) => { text.encode_utf16().chain([0]) }
            .flat_map(u16::to_le_bytes)
            .collect(),
        format if format == u32::from(CF_TEXT.0) => {
            let mut bytes = text.into_bytes();
            bytes.push(0);
            bytes
        }
        _ => return HDDEDATA::default(),
    };
    // SAFETY: the data is copied by the call; the item handle is the one
    // of the request.
    unsafe { DdeCreateDataHandle(server.instance, Some(&bytes), 0, Some(item), format, 0) }
}

/// The folders of the Start menu's programs: the user's and all users'.
fn programs(common: bool) -> anyhow::Result<PathBuf> {
    match common {
        true => known_folder(&FOLDERID_CommonPrograms),
        false => known_folder(&FOLDERID_Programs),
    }
}

/// The names of the groups of both Programs folders, each once.
fn groups() -> Vec<String> {
    let mut names = Vec::new();
    for common in [false, true] {
        let Ok(folder) = programs(common) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let folder = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if folder
                && !names
                    .iter()
                    .any(|other: &String| other.eq_ignore_ascii_case(&name))
            {
                names.push(name);
            }
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// The folder of the group `name`, made where `make`: in all users'
/// Programs with `common` where the user may write there, and in the
/// user's otherwise.
fn group_folder(name: &str, common: bool, make: bool) -> anyhow::Result<PathBuf> {
    use anyhow::Context as _;
    let refused = || anyhow::anyhow!("the group name {name:?} leaves the Start menu");
    if common {
        let folder = programs(true).and_then(|root| inside(&root, name).ok_or_else(refused));
        match folder {
            Ok(folder) if !make || folder.is_dir() => return Ok(folder),
            Ok(folder) => match std::fs::create_dir_all(&folder) {
                Ok(()) => return Ok(folder),
                // Without the right to write there, as Explorer does for a
                // user who is no administrator, the group is the user's.
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("{} was not made", folder.display()));
                }
            },
            Err(error) => return Err(error),
        }
    }
    let folder = inside(&programs(false)?, name).ok_or_else(refused)?;
    if make {
        std::fs::create_dir_all(&folder)
            .with_context(|| format!("{} was not made", folder.display()))?;
    }
    Ok(folder)
}

/// Does `command`; `current` is the group of the conversation.
fn run(command: &Command, current: &mut Option<PathBuf>) -> anyhow::Result<()> {
    use anyhow::Context as _;
    match command {
        Command::CreateGroup { name, common } => {
            *current = Some(group_folder(name, *common, true)?);
        }
        Command::ShowGroup { name, show, common } => {
            let folder = group_folder(name, *common, true)?;
            // 0 hides, 2, 6, 7 and 11 minimize: no window is shown then.
            if !matches!(show, 0 | 2 | 6 | 7 | 11) {
                show_folder(folder.clone());
            }
            *current = Some(folder);
        }
        Command::AddItem(item) => {
            let group = current
                .clone()
                .context("an item was added before a group was made")?;
            save_shortcut(&group, item)?;
        }
        Command::DeleteItem(name) | Command::ReplaceItem(name) => {
            let group = current
                .clone()
                .context("an item was removed before a group was chosen")?;
            let link = group.join(format!("{}.lnk", file_name_of(name)));
            match std::fs::remove_file(&link) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("{} stays", link.display()));
                }
            }
        }
        Command::DeleteGroup { name, common } => {
            let folder = group_folder(name, *common, false)?;
            if folder.is_dir() {
                std::fs::remove_dir_all(&folder)
                    .with_context(|| format!("{} stays", folder.display()))?;
            }
            if current.as_ref() == Some(&folder) {
                *current = None;
            }
        }
        Command::Reload | Command::ExitProgman => {}
        Command::ShowFolder(path) => {
            let path = PathBuf::from(path);
            if path.is_dir() {
                show_folder(path);
            }
        }
    }
    Ok(())
}

/// Hands `folder` to the user interface to be shown.
fn show_folder(folder: PathBuf) {
    let (sender, _) = FOLDERS.get_or_init(async_channel::unbounded);
    let _ = sender.try_send(folder);
}

/// Saves the shortcut of `item` in `group`.
fn save_shortcut(group: &Path, item: &Item) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let exists = |path: &str| Path::new(path).is_file();
    let (program, arguments) = split_command_line(&item.command, exists);
    let name = item.name.clone().unwrap_or_else(|| {
        Path::new(&program).file_stem().map_or_else(
            || program.clone(),
            |stem| stem.to_string_lossy().into_owned(),
        )
    });
    let file = group.join(format!("{}.lnk", file_name_of(&name)));
    let show: SHOW_WINDOW_CMD = match item.minimized {
        true => SW_SHOWMINNOACTIVE,
        false => SW_SHOWNORMAL,
    };
    // SAFETY: COM is initialized on this thread; the texts outlive the
    // calls, which copy them.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(program.as_str()))?;
        if !arguments.is_empty() {
            link.SetArguments(&HSTRING::from(arguments.as_str()))?;
        }
        if let Some(directory) = &item.directory {
            link.SetWorkingDirectory(&HSTRING::from(directory.as_str()))?;
        }
        if let Some(icon) = &item.icon {
            link.SetIconLocation(&HSTRING::from(icon.as_str()), item.icon_index)?;
        }
        if item.hotkey != 0 {
            link.SetHotkey(item.hotkey)?;
        }
        link.SetShowCmd(show)?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(file.as_os_str()), true)
            .with_context(|| format!("{} was not saved", file.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_read_in_order() {
        let commands =
            parse("[CreateGroup(Games,1)][ShowGroup(Games, 1)] [AddItem(\"C:\\Games\\x.exe\",X)]")
                .unwrap();
        assert_eq!(
            commands,
            [
                Command::CreateGroup {
                    name: "Games".to_owned(),
                    common: true
                },
                Command::ShowGroup {
                    name: "Games".to_owned(),
                    show: 1,
                    common: false
                },
                Command::AddItem(Item {
                    command: r#""C:\Games\x.exe""#.to_owned(),
                    name: Some("X".to_owned()),
                    ..Item::default()
                }),
            ]
        );
    }

    #[test]
    fn every_argument_of_an_item_is_read() {
        let commands = parse(
            r#"[AddItem("C:\App\a.exe" -x,"A, the app",C:\App\a.ico,2,10,20,C:\App,1601,1,0)]"#,
        )
        .unwrap();
        assert_eq!(
            commands,
            [Command::AddItem(Item {
                command: r#""C:\App\a.exe" -x"#.to_owned(),
                name: Some("A, the app".to_owned()),
                icon: Some(r"C:\App\a.ico".to_owned()),
                icon_index: 2,
                directory: Some(r"C:\App".to_owned()),
                hotkey: 1601,
                minimized: true,
            })]
        );
    }

    #[test]
    fn names_of_commands_take_no_case_and_may_have_no_arguments() {
        assert_eq!(
            parse("[reload][EXITPROGMAN()]").unwrap(),
            [Command::Reload, Command::ExitProgman]
        );
        assert_eq!(parse("  \0").unwrap(), []);
    }

    #[test]
    fn broken_and_unknown_commands_are_refused() {
        assert!(matches!(
            parse("CreateGroup(x)"),
            Err(Refusal::Malformed(_))
        ));
        assert!(matches!(
            parse("[CreateGroup(x]"),
            Err(Refusal::Malformed(_))
        ));
        assert!(matches!(
            parse("[CreateGroup(\"x)]"),
            Err(Refusal::Malformed(_))
        ));
        assert_eq!(
            parse("[Format(C:)]"),
            Err(Refusal::Unknown("Format".to_owned()))
        );
        assert_eq!(
            parse("[CreateGroup()]"),
            Err(Refusal::Missing("the name of the group"))
        );
    }

    #[test]
    fn a_group_stays_inside_the_start_menu() {
        let root = Path::new(r"C:\Start\Programs");
        assert_eq!(inside(root, "Games"), Some(root.join("Games")));
        assert_eq!(
            inside(root, r"Tools\Small"),
            Some(root.join("Tools").join("Small"))
        );
        assert_eq!(inside(root, r"..\Startup"), None);
        assert_eq!(inside(root, r"C:\Windows"), None);
        assert_eq!(inside(root, "a|b"), None);
        assert_eq!(inside(root, ".."), None);
        assert_eq!(inside(root, " . "), None);
        assert_eq!(inside(root, ""), None);
    }

    #[test]
    fn a_shortcut_name_holds_only_what_a_file_name_may() {
        assert_eq!(file_name_of("Read me: first?"), "Read me_ first_");
        assert_eq!(file_name_of(" ... "), "_");
        assert_eq!(file_name_of("Tool."), "Tool");
    }

    #[test]
    fn a_command_line_is_split_into_program_and_arguments() {
        let none = |_: &str| false;
        assert_eq!(
            split_command_line(r#""C:\Program Files\A\a.exe" /x"#, none),
            (r"C:\Program Files\A\a.exe".to_owned(), "/x".to_owned())
        );
        let found = |path: &str| path == r"C:\Program Files\A\a.exe";
        assert_eq!(
            split_command_line(r"C:\Program Files\A\a.exe /x /y", found),
            (r"C:\Program Files\A\a.exe".to_owned(), "/x /y".to_owned())
        );
        assert_eq!(
            split_command_line(r"C:\a.exe /x", none),
            (r"C:\a.exe".to_owned(), "/x".to_owned())
        );
        assert_eq!(
            split_command_line(r"notepad.exe", none),
            ("notepad.exe".to_owned(), String::new())
        );
    }

    #[test]
    fn the_text_of_an_execute_is_read_in_either_width() {
        let wide: Vec<u8> = { "[Reload]\0".encode_utf16() }
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(text_of(&wide), "[Reload]");
        assert_eq!(text_of(b"[Reload]\0"), "[Reload]");
    }

    #[test]
    fn the_groups_are_a_line_each() {
        let groups = ["Games".to_owned(), "Tools".to_owned()];
        assert_eq!(groups_text(&groups), "Games\r\nTools\r\n");
    }
}
