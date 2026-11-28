// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The server of the Shell's change notifications, as Explorer's desktop
//! hosts it: its numbers, the layouts of what it is handed, and which
//! registered window gets which notification.
//!
//! `SHChangeNotifyRegister` (file dialogs, hosts of `ExplorerBrowser`,
//! Office) and `SHChangeNotify` (installers, Tessera's own
//! `SHCNE_ASSOCCHANGED`) talk to a server that the desktop of the shell
//! gives. How Windows 11's shell32 talks to it is not documented. The one
//! open description is ReactOS's shell32 (`changenotify.cpp`,
//! `CChangeNotifyServer` and `undocshell.h`): the client asks the desktop
//! window for the server with [`WM_DESKTOP_GET_CNOTIFY_SERVER`] and sends
//! the server tickets in shared memory (`SHAllocShared`), a registration
//! ([`RegEntry`]) with [`CN_REGISTER`] and a notification ([`Ticket`])
//! with [`CN_DELIVER`]; the server posts each window that registered for
//! it its message with the ticket, which the window reads with
//! `SHChangeNotification_Lock`.
//!
//! Whether Windows speaks this protocol is unknown. The window side is
//! therefore not built: the recorder of the shell host
//! (`[shell_services] log_messages`) shows on Windows whether
//! `WM_USER+25` reaches the desktop window, and only then is it wired,
//! behind `[shell_services] change_server`. What is here is pure and
//! tested: the constants, each with its source, the layouts, and the
//! delivery ([`Registry::recipients`]): the masks of `SHCNE_*` events,
//! the sources (`SHCNRF_*`), recursive registrations, and the match of an
//! item list under another (`ILIsParent`), with paths of the file system
//! as the second way of matching, as ReactOS does. Until it is proven,
//! programs keep polling, and Tessera's own views keep their watchers.
#![allow(
    dead_code,
    reason = "the windows of the server wait until the recorder shows the protocol on Windows"
)]

use windows::Win32::UI::WindowsAndMessaging::WM_USER;

/// Asks the desktop window for the server window (ReactOS undocshell.h:
/// `WM_DESKTOP_GET_CNOTIFY_SERVER`, `WM_USER + 25`); `wParam` non-zero
/// makes it where it is missing.
pub(super) const WM_DESKTOP_GET_CNOTIFY_SERVER: u32 = WM_USER + 25;
/// To the server: a registration, `wParam` the ticket of a [`RegEntry`],
/// `lParam` the process that allocated it (ReactOS: `CN_REGISTER`).
pub(super) const CN_REGISTER: u32 = WM_USER + 1;
/// To the server: `wParam` the number of a registration to end.
pub(super) const CN_UNREGISTER: u32 = WM_USER + 2;
/// To the server: a notification, `wParam` the ticket of a [`Ticket`],
/// `lParam` the process that allocated it.
pub(super) const CN_DELIVER: u32 = WM_USER + 3;
/// To the server: `wParam` non-zero holds the notifications of the
/// process `lParam` back, zero lets them go again.
pub(super) const CN_SUSPEND_RESUME: u32 = WM_USER + 6;
/// To the server: ends every registration of the process `lParam`.
pub(super) const CN_UNREGISTER_PROCESS: u32 = WM_USER + 7;

/// The first field of a [`RegEntry`] (ReactOS: `REGENTRY_MAGIC`).
pub(super) const REGENTRY_MAGIC: u32 = 0xB58B;
/// The first field of a [`Ticket`] (ReactOS: `DELITICKET_MAGIC`).
pub(super) const DELITICKET_MAGIC: u32 = 0xDEAD_FACE;
/// The first field of the bag `SHChangeNotification_Lock` makes
/// (ReactOS: `HANDBAG_MAGIC`).
pub(super) const HANDBAG_MAGIC: u32 = 0xFACE_B00C;

/// The bit of an event that comes from watching the file system rather
/// than from `SHChangeNotify` (shlobj.h: `SHCNE_INTERRUPT`).
pub(super) const SHCNE_INTERRUPT: u32 = 0x8000_0000;
/// Every event (shlobj.h: `SHCNE_ALLEVENTS`).
pub(super) const SHCNE_ALLEVENTS: u32 = 0x7FFF_FFFF;
/// Some events of shlobj.h, by name.
pub(super) const SHCNE_RENAMEITEM: u32 = 0x0000_0001;
pub(super) const SHCNE_CREATE: u32 = 0x0000_0002;
pub(super) const SHCNE_DELETE: u32 = 0x0000_0004;
pub(super) const SHCNE_MKDIR: u32 = 0x0000_0008;
pub(super) const SHCNE_RMDIR: u32 = 0x0000_0010;
pub(super) const SHCNE_UPDATEDIR: u32 = 0x0000_1000;
pub(super) const SHCNE_UPDATEITEM: u32 = 0x0000_2000;
pub(super) const SHCNE_ASSOCCHANGED: u32 = 0x0800_0000;

/// The sources a registration takes (shlobj.h `SHCNRF_*`): events from
/// watching the file system, events of `SHChangeNotify`, the first for
/// the whole tree below, and delivery by ticket.
pub(super) const SHCNRF_INTERRUPT_LEVEL: i32 = 0x0001;
pub(super) const SHCNRF_SHELL_LEVEL: i32 = 0x0002;
pub(super) const SHCNRF_RECURSIVE_INTERRUPT: i32 = 0x1000;
pub(super) const SHCNRF_NEW_DELIVERY: i32 = 0x8000;

/// How wide a pointer and a window handle are in the process that wrote
/// a [`RegEntry`]: a 32-bit program writes them in 4 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Width {
    Bits32,
    Bits64,
}

impl Width {
    fn handle(self) -> usize {
        match self {
            Self::Bits32 => 4,
            Self::Bits64 => 8,
        }
    }
}

/// The fields of ReactOS's `REGENTRY` by their offsets: `dwMagic`,
/// `cbSize`, `nRegID`, `hwnd`, `uMsg`, `fSources`, `fEvents`,
/// `fRecursive`, `hwndBroker`, `ibPidl`, with the alignment of a handle
/// of its width; the item list follows at `ibPidl`, or there is none
/// where it is zero.
#[derive(Clone, Copy, Debug)]
struct RegLayout {
    window: usize,
    message: usize,
    sources: usize,
    events: usize,
    recursive: usize,
    broker: usize,
    item_offset: usize,
    size: usize,
}

impl RegLayout {
    const fn of(width: Width) -> Self {
        match width {
            Width::Bits32 => Self {
                window: 12,
                message: 16,
                sources: 20,
                events: 24,
                recursive: 28,
                broker: 32,
                item_offset: 36,
                size: 40,
            },
            Width::Bits64 => Self {
                window: 16,
                message: 24,
                sources: 28,
                events: 32,
                recursive: 36,
                broker: 40,
                item_offset: 48,
                size: 56,
            },
        }
    }
}

/// A registration of `SHChangeNotifyRegister`, as the client hands it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RegEntry {
    /// Its number; zero asks the server for one.
    pub id: u32,
    /// The window told, and the message it is told with.
    pub window: isize,
    pub message: u32,
    /// `SHCNRF_*`.
    pub sources: i32,
    /// The `SHCNE_*` events it wants.
    pub events: u32,
    /// Whether the folder's whole tree counts, not only its children.
    pub recursive: bool,
    /// The window that relays to `window` in another process, if any.
    pub broker: isize,
    /// The item list of the folder watched; `None` for everything.
    pub item: Option<Vec<u8>>,
}

impl RegEntry {
    /// Reads one written by a process of `width`.
    pub(super) fn decode(bytes: &[u8], width: Width) -> Option<Self> {
        let layout = RegLayout::of(width);
        if bytes.len() < layout.size || read_u32(bytes, 0)? != REGENTRY_MAGIC {
            return None;
        }
        let size = read_u32(bytes, 4)? as usize;
        let bytes = bytes.get(..size.min(bytes.len()))?;
        let offset = read_u32(bytes, layout.item_offset)? as usize;
        let item = match offset {
            0 => None,
            offset => Some(id_list_at(bytes, offset)?.to_vec()),
        };
        Some(Self {
            id: read_u32(bytes, 8)?,
            window: read_handle(bytes, layout.window, width)?,
            message: read_u32(bytes, layout.message)?,
            sources: read_u32(bytes, layout.sources)? as i32,
            events: read_u32(bytes, layout.events)?,
            recursive: read_u32(bytes, layout.recursive)? != 0,
            broker: read_handle(bytes, layout.broker, width)?,
            item,
        })
    }

    /// Writes it as a process of `width` would.
    pub(super) fn encode(&self, width: Width) -> Vec<u8> {
        let layout = RegLayout::of(width);
        let mut bytes = vec![0u8; layout.size];
        let item = self.item.as_deref().unwrap_or_default();
        let size = layout.size + item.len();
        write_u32(&mut bytes, 0, REGENTRY_MAGIC);
        write_u32(&mut bytes, 4, size as u32);
        write_u32(&mut bytes, 8, self.id);
        write_handle(&mut bytes, layout.window, width, self.window);
        write_u32(&mut bytes, layout.message, self.message);
        write_u32(&mut bytes, layout.sources, self.sources as u32);
        write_u32(&mut bytes, layout.events, self.events);
        write_u32(&mut bytes, layout.recursive, u32::from(self.recursive));
        write_handle(&mut bytes, layout.broker, width, self.broker);
        let offset = if self.item.is_some() { layout.size } else { 0 };
        write_u32(&mut bytes, layout.item_offset, offset as u32);
        bytes.extend_from_slice(item);
        bytes
    }
}

/// A notification of `SHChangeNotify`, as ReactOS's `DELITICKET` holds
/// it: `dwMagic`, `wEventId`, `uFlags`, `dwTick`, `ibOffset1`,
/// `ibOffset2`, each four bytes, then the item lists at their offsets
/// (zero where there is none).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Ticket {
    /// `SHCNE_*`, with [`SHCNE_INTERRUPT`] for one from the file system.
    pub event: u32,
    /// `SHCNF_*` as the client gave them.
    pub flags: u32,
    /// When it was made, in milliseconds since Windows started.
    pub tick: u32,
    /// The item lists it is about: the item, and the new name of one
    /// renamed.
    pub items: [Option<Vec<u8>>; 2],
}

/// The size of the fields of a ticket before its item lists.
const TICKET_HEAD: usize = 24;

impl Ticket {
    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < TICKET_HEAD || read_u32(bytes, 0)? != DELITICKET_MAGIC {
            return None;
        }
        let item = |at: usize| -> Option<Option<Vec<u8>>> {
            match read_u32(bytes, at)? as usize {
                0 => Some(None),
                offset => Some(Some(id_list_at(bytes, offset)?.to_vec())),
            }
        };
        Some(Self {
            event: read_u32(bytes, 4)?,
            flags: read_u32(bytes, 8)?,
            tick: read_u32(bytes, 12)?,
            items: [item(16)?, item(20)?],
        })
    }

    pub(super) fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0u8; TICKET_HEAD];
        write_u32(&mut bytes, 0, DELITICKET_MAGIC);
        write_u32(&mut bytes, 4, self.event);
        write_u32(&mut bytes, 8, self.flags);
        write_u32(&mut bytes, 12, self.tick);
        for (field, item) in [16, 20].into_iter().zip(&self.items) {
            if let Some(item) = item {
                let offset = bytes.len() as u32;
                write_u32(&mut bytes, field, offset);
                bytes.extend_from_slice(item);
            }
        }
        bytes
    }

    /// Whether it comes from watching the file system.
    fn is_of_file_system(&self) -> bool {
        self.event & SHCNE_INTERRUPT != 0
    }
}

/// A registration the server holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Registration {
    pub entry: RegEntry,
    /// The process that registered.
    pub process: u32,
    /// The path of the folder watched, where it is one of the file
    /// system: the second way a notification is matched, as items of one
    /// folder can have different lists.
    pub path: Option<String>,
}

/// A window to tell of a notification, and the message to tell it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Delivery {
    pub id: u32,
    pub window: isize,
    pub message: u32,
}

/// The registrations, and the processes whose notifications are held
/// back.
#[derive(Debug, Default)]
pub(super) struct Registry {
    registrations: Vec<Registration>,
    next_id: u32,
    suspended: Vec<u32>,
}

impl Registry {
    /// Holds `entry` of `process`, and gives its number: the one it asked
    /// for where that is free, a new one otherwise.
    pub(super) fn register(
        &mut self,
        mut entry: RegEntry,
        process: u32,
        path: Option<String>,
    ) -> u32 {
        let taken = |id: u32, registrations: &[Registration]| {
            id == 0 || registrations.iter().any(|held| held.entry.id == id)
        };
        if taken(entry.id, &self.registrations) {
            loop {
                self.next_id = self.next_id.wrapping_add(1).max(1);
                if !taken(self.next_id, &self.registrations) {
                    break;
                }
            }
            entry.id = self.next_id;
        }
        let id = entry.id;
        self.registrations.push(Registration {
            entry,
            process,
            path,
        });
        id
    }

    /// Ends the registration `id`; says whether there was one.
    pub(super) fn unregister(&mut self, id: u32) -> bool {
        let before = self.registrations.len();
        self.registrations.retain(|held| held.entry.id != id);
        self.registrations.len() != before
    }

    /// Ends every registration of `process`, and says how many there were.
    pub(super) fn unregister_process(&mut self, process: u32) -> usize {
        let before = self.registrations.len();
        self.registrations.retain(|held| held.process != process);
        before - self.registrations.len()
    }

    /// Holds the notifications of `process` back, or lets them go again.
    pub(super) fn suspend(&mut self, process: u32, suspended: bool) {
        self.suspended.retain(|other| *other != process);
        if suspended {
            self.suspended.push(process);
        }
    }

    /// The windows `ticket` goes to; `paths` are the paths of its items
    /// where they are of the file system.
    pub(super) fn recipients(&self, ticket: &Ticket, paths: [Option<&str>; 2]) -> Vec<Delivery> {
        { self.registrations.iter() }
            .filter(|held| !self.suspended.contains(&held.process))
            .filter(|held| wants(held, ticket, paths))
            .map(|held| Delivery {
                id: held.entry.id,
                window: held.entry.window,
                message: held.entry.message,
            })
            .collect()
    }
}

/// Whether `held` is told of `ticket`.
fn wants(held: &Registration, ticket: &Ticket, paths: [Option<&str>; 2]) -> bool {
    let entry = &held.entry;
    if (ticket.event & !SHCNE_INTERRUPT) & entry.events == 0 {
        return false;
    }
    let source = match ticket.is_of_file_system() {
        true => SHCNRF_INTERRUPT_LEVEL,
        false => SHCNRF_SHELL_LEVEL,
    };
    if entry.sources & source == 0 {
        return false;
    }
    // A registration of everything, and an event about no item, such as
    // SHCNE_ASSOCCHANGED, need no match of places.
    let Some(watched) = entry.item.as_deref() else {
        return true;
    };
    if ticket.items.iter().all(Option::is_none) {
        return true;
    }
    let by_list = { ticket.items.iter().flatten() }
        .any(|item| is_equal(watched, item) || is_parent(watched, item, !entry.recursive));
    let by_path = held.path.as_deref().is_some_and(|folder| {
        { paths.into_iter().flatten() }.any(|path| path_within(folder, path, entry.recursive))
    });
    by_list || by_path
}

/// The items (`SHITEMID`s) of an item list, each with its size field, up
/// to its ending; `None` where it is broken.
pub(super) fn items_of(list: &[u8]) -> Option<Vec<&[u8]>> {
    let mut items = Vec::new();
    let mut at = 0usize;
    loop {
        let size = u16::from_le_bytes([*list.get(at)?, *list.get(at + 1)?]) as usize;
        if size == 0 {
            return Some(items);
        }
        if size < 2 {
            return None;
        }
        items.push(list.get(at..at + size)?);
        at += size;
    }
}

/// Whether two item lists are the same, item by item.
pub(super) fn is_equal(one: &[u8], other: &[u8]) -> bool {
    matches!((items_of(one), items_of(other)), (Some(one), Some(other)) if one == other)
}

/// Whether `parent` is a folder above `child`, as `ILIsParent` says: its
/// items begin the child's, which has more; with `immediate`, exactly
/// one more. The desktop's empty list is above every other.
pub(super) fn is_parent(parent: &[u8], child: &[u8], immediate: bool) -> bool {
    let (Some(parent), Some(child)) = (items_of(parent), items_of(child)) else {
        return false;
    };
    if child.len() <= parent.len() || child[..parent.len()] != parent[..] {
        return false;
    }
    !immediate || child.len() == parent.len() + 1
}

/// Whether the path `path` is `folder` or lies in it (anywhere below with
/// `recursive`, directly otherwise), without case, as Windows compares
/// paths.
pub(super) fn path_within(folder: &str, path: &str, recursive: bool) -> bool {
    let folder = folder.trim_end_matches('\\').to_lowercase();
    let path = path.trim_end_matches('\\').to_lowercase();
    if path == folder {
        return true;
    }
    let Some(rest) = path
        .strip_prefix(&folder)
        .and_then(|rest| rest.strip_prefix('\\'))
    else {
        return false;
    };
    !rest.is_empty() && (recursive || !rest.contains('\\'))
}

/// The item list at `offset` of `bytes`, with its ending.
fn id_list_at(bytes: &[u8], offset: usize) -> Option<&[u8]> {
    let list = bytes.get(offset..)?;
    let length = super::shell_item::id_list_length(list)?;
    list.get(..length)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_handle(bytes: &[u8], at: usize, width: Width) -> Option<isize> {
    match width {
        Width::Bits32 => Some(read_u32(bytes, at)? as i32 as isize),
        Width::Bits64 => Some(i64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?) as isize),
    }
}

fn write_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_handle(bytes: &mut [u8], at: usize, width: Width, value: isize) {
    let size = width.handle();
    bytes[at..at + size].copy_from_slice(&(value as i64).to_le_bytes()[..size]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An item list of items of the given bytes, with its ending.
    fn list(items: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for item in items {
            bytes.extend_from_slice(&((item.len() + 2) as u16).to_le_bytes());
            bytes.extend_from_slice(item);
        }
        bytes.extend_from_slice(&[0, 0]);
        bytes
    }

    fn entry(item: Option<Vec<u8>>, events: u32, recursive: bool) -> RegEntry {
        RegEntry {
            id: 0,
            window: 0x1234,
            message: 0x8001,
            sources: SHCNRF_SHELL_LEVEL | SHCNRF_INTERRUPT_LEVEL | SHCNRF_NEW_DELIVERY,
            events,
            recursive,
            broker: 0,
            item,
        }
    }

    fn ticket(event: u32, item: Option<Vec<u8>>) -> Ticket {
        Ticket {
            event,
            items: [item, None],
            ..Ticket::default()
        }
    }

    #[test]
    fn the_numbers_are_those_of_reactos() {
        assert_eq!(WM_DESKTOP_GET_CNOTIFY_SERVER, 0x419);
        assert_eq!(CN_REGISTER, 0x401);
        assert_eq!(CN_DELIVER, 0x403);
        assert_eq!(CN_SUSPEND_RESUME, 0x406);
        assert_eq!(CN_UNREGISTER_PROCESS, 0x407);
    }

    #[test]
    fn a_folder_is_above_its_children_and_the_desktop_above_all() {
        let drive = list(&[b"C"]);
        let users = list(&[b"C", b"Users"]);
        let me = list(&[b"C", b"Users", b"Me"]);
        let desktop = list(&[]);
        assert!(is_parent(&drive, &users, true));
        assert!(!is_parent(&drive, &me, true));
        assert!(is_parent(&drive, &me, false));
        assert!(is_parent(&desktop, &me, false));
        assert!(!is_parent(&users, &users, false));
        assert!(!is_parent(&me, &users, false));
        assert!(is_equal(&users, &list(&[b"C", b"Users"])));
        // A broken list is no parent and no child.
        assert!(!is_parent(&[5, 0, 1], &users, false));
    }

    #[test]
    fn an_item_list_is_split_into_its_items() {
        let both = list(&[b"ab", b"c"]);
        let items = items_of(&both).unwrap();
        assert_eq!(items, [&[4u8, 0, b'a', b'b'][..], &[3, 0, b'c'][..]]);
        assert_eq!(items_of(&[1, 0, 0, 0]), None);
        assert_eq!(items_of(&[9, 0, 1]), None);
    }

    #[test]
    fn a_registration_gets_the_events_of_its_mask_and_place() {
        let mut registry = Registry::default();
        let folder = list(&[b"C", b"Temp"]);
        let id = registry.register(entry(Some(folder.clone()), SHCNE_CREATE, false), 7, None);
        let child = list(&[b"C", b"Temp", b"new.txt"]);
        let deeper = list(&[b"C", b"Temp", b"x", b"new.txt"]);
        let made = ticket(SHCNE_CREATE, Some(child.clone()));
        assert_eq!(
            registry.recipients(&made, [None, None]),
            [Delivery {
                id,
                window: 0x1234,
                message: 0x8001
            }]
        );
        // Another event, or a place deeper than a registration that is
        // not recursive, is not told.
        assert!(
            registry
                .recipients(&ticket(SHCNE_DELETE, Some(child)), [None, None])
                .is_empty()
        );
        assert!(
            registry
                .recipients(&ticket(SHCNE_CREATE, Some(deeper.clone())), [None, None])
                .is_empty()
        );
        // The folder itself is.
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, Some(folder)), [None, None])
                .len(),
            1
        );
        // A recursive one is told of the whole tree.
        registry.register(entry(Some(list(&[b"C"])), SHCNE_ALLEVENTS, true), 8, None);
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, Some(deeper)), [None, None])
                .len(),
            1
        );
    }

    #[test]
    fn events_without_items_and_registrations_of_everything_go_by_mask() {
        let mut registry = Registry::default();
        registry.register(
            entry(Some(list(&[b"C"])), SHCNE_ASSOCCHANGED, true),
            1,
            None,
        );
        registry.register(entry(None, SHCNE_UPDATEDIR, false), 2, None);
        let associations = ticket(SHCNE_ASSOCCHANGED, None);
        assert_eq!(registry.recipients(&associations, [None, None]).len(), 1);
        let update = ticket(SHCNE_UPDATEDIR, Some(list(&[b"D", b"x"])));
        assert_eq!(registry.recipients(&update, [None, None]).len(), 1);
    }

    #[test]
    fn the_sources_of_a_registration_are_kept() {
        let mut registry = Registry::default();
        let mut shell_only = entry(None, SHCNE_ALLEVENTS, true);
        shell_only.sources = SHCNRF_SHELL_LEVEL;
        registry.register(shell_only, 1, None);
        let from_disk = ticket(SHCNE_CREATE | SHCNE_INTERRUPT, None);
        assert!(registry.recipients(&from_disk, [None, None]).is_empty());
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, None), [None, None])
                .len(),
            1
        );
    }

    #[test]
    fn paths_match_where_item_lists_differ() {
        let mut registry = Registry::default();
        let watched = Some(r"C:\Temp".to_owned());
        registry.register(
            entry(Some(list(&[b"one"])), SHCNE_CREATE, false),
            1,
            watched,
        );
        let other_list = ticket(SHCNE_CREATE, Some(list(&[b"two", b"x"])));
        assert_eq!(
            registry
                .recipients(&other_list, [Some(r"c:\temp\new.txt"), None])
                .len(),
            1
        );
        assert!(
            registry
                .recipients(&other_list, [Some(r"C:\Temp\a\new.txt"), None])
                .is_empty()
        );
        assert!(
            registry
                .recipients(&other_list, [Some(r"C:\Temporary\x"), None])
                .is_empty()
        );
        assert!(path_within(r"C:\Temp\", r"C:\TEMP", false));
        assert!(path_within(r"C:\Temp", r"C:\Temp\a\b", true));
    }

    #[test]
    fn numbers_are_given_once_and_registrations_end() {
        let mut registry = Registry::default();
        let first = registry.register(entry(None, SHCNE_ALLEVENTS, true), 1, None);
        let mut asked = entry(None, SHCNE_ALLEVENTS, true);
        asked.id = first;
        let second = registry.register(asked, 1, None);
        assert_ne!(first, second);
        registry.register(entry(None, SHCNE_ALLEVENTS, true), 2, None);
        assert!(registry.unregister(first));
        assert!(!registry.unregister(first));
        // A suspended process is told nothing until it is resumed.
        registry.suspend(2, true);
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, None), [None, None])
                .len(),
            1
        );
        registry.suspend(2, false);
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, None), [None, None])
                .len(),
            2
        );
        assert_eq!(registry.unregister_process(1), 1);
        assert_eq!(
            registry
                .recipients(&ticket(SHCNE_CREATE, None), [None, None])
                .len(),
            1
        );
    }

    #[test]
    fn tickets_and_registrations_are_read_as_written() {
        let renamed = Ticket {
            event: SHCNE_RENAMEITEM,
            flags: 0x1000,
            tick: 42,
            items: [Some(list(&[b"C", b"a"])), Some(list(&[b"C", b"b"]))],
        };
        assert_eq!(Ticket::decode(&renamed.encode()), Some(renamed));
        assert_eq!(Ticket::decode(&[0; TICKET_HEAD]), None);
        for width in [Width::Bits32, Width::Bits64] {
            let mut registered = entry(
                Some(list(&[b"C", b"Temp"])),
                SHCNE_MKDIR | SHCNE_RMDIR,
                true,
            );
            registered.id = 9;
            registered.window = -2;
            assert_eq!(
                RegEntry::decode(&registered.encode(width), width),
                Some(registered)
            );
            let everything = entry(None, SHCNE_UPDATEITEM, false);
            assert_eq!(
                RegEntry::decode(&everything.encode(width), width),
                Some(everything)
            );
        }
        assert_eq!(RegLayout::of(Width::Bits64).size, 56);
        assert_eq!(HANDBAG_MAGIC, 0xFACE_B00C);
    }
}
