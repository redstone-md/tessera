// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windows' notification database: the toast XML of each notification of
//! the notification center, which `UserNotificationListener` does not give
//! out, read from `wpndatabase.db` in
//! `%LOCALAPPDATA%\Microsoft\Windows\Notifications`.
//!
//! The database is SQLite in write-ahead mode, kept open by Windows. It is
//! read through Windows' own `winsqlite3.dll` (part of Windows 10 and 11),
//! loaded with `LoadLibraryW` and called through the functions
//! `GetProcAddress` finds; no SQLite is brought along. It is opened read
//! only (`mode=ro`), on a thread of its own, and never written.
//!
//! The layout is not documented by Microsoft. What is read: the table
//! `Notification` (`Id`, `HandlerId`, `Payload`, `ArrivalTime`) joined to
//! `NotificationHandler` (`RecordId`, `PrimaryId`, the identifier the
//! application posts under), by the `Id` that `UserNotification.Id` gives;
//! and, where a table `NotificationData` with columns of a notification,
//! a key and a value is there, the values the application bound to its
//! progress bars. Where the layout is otherwise, reading fails with a
//! reason, and the texts of the listener are shown as before.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError, mpsc};
use std::thread;

use async_channel::Receiver;
use windows::Management::Deployment::PackageManager;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{HSTRING, PCSTR, w};

/// The file of the database, below the local application data.
const DATABASE: &str = r"Microsoft\Windows\Notifications\wpndatabase.db";

/// `SQLITE_OPEN_READONLY | SQLITE_OPEN_URI`.
const OPEN_FLAGS: c_int = 0x0000_0001 | 0x0000_0040;

/// What `sqlite3_step` says while it has a row, and once it has no more.
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OK: c_int = 0;

/// How long a read waits for Windows, which writes the database, in
/// milliseconds.
const BUSY_WAIT: c_int = 2_000;

/// The largest toast XML that is read, in bytes.
const PAYLOAD_LIMIT: usize = 256 * 1024;

/// The toast of a notification, as the database holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastPayload {
    /// The identifier `UserNotification.Id` gives.
    pub id: u32,
    /// The identifier the application posts under.
    pub app_id: String,
    /// The toast XML.
    pub xml: String,
    /// The values the application bound to `{names}` of the toast.
    pub data: HashMap<String, String>,
}

type Open =
    unsafe extern "system" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Prepare = unsafe extern "system" fn(
    *mut c_void,
    *const c_char,
    c_int,
    *mut *mut c_void,
    *mut *const c_char,
) -> c_int;
type BindInt64 = unsafe extern "system" fn(*mut c_void, c_int, i64) -> c_int;
type Step = unsafe extern "system" fn(*mut c_void) -> c_int;
type ColumnText = unsafe extern "system" fn(*mut c_void, c_int) -> *const u8;
type ColumnBlob = unsafe extern "system" fn(*mut c_void, c_int) -> *const c_void;
type ColumnBytes = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;
type ColumnInt64 = unsafe extern "system" fn(*mut c_void, c_int) -> i64;
type Finalize = unsafe extern "system" fn(*mut c_void) -> c_int;
type Close = unsafe extern "system" fn(*mut c_void) -> c_int;
type ErrorMessage = unsafe extern "system" fn(*mut c_void) -> *const c_char;
type BusyTimeout = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;

/// The functions of `winsqlite3.dll` that are used. The library stays
/// loaded for as long as Tessera runs.
struct Sqlite {
    open: Open,
    prepare: Prepare,
    bind_int64: BindInt64,
    step: Step,
    reset: Step,
    column_text: ColumnText,
    column_blob: ColumnBlob,
    column_bytes: ColumnBytes,
    column_int64: ColumnInt64,
    finalize: Finalize,
    close: Close,
    errmsg: ErrorMessage,
    busy_timeout: BusyTimeout,
}

impl Sqlite {
    /// Loads the library once; the reason it could not be is kept.
    fn get() -> Result<&'static Self, String> {
        static LOADED: OnceLock<Result<Sqlite, String>> = OnceLock::new();
        LOADED
            .get_or_init(Self::load)
            .as_ref()
            .map_err(Clone::clone)
    }

    fn load() -> Result<Self, String> {
        // SAFETY: the name is a NUL-terminated constant; the library is a
        // part of Windows and is never freed.
        let library = unsafe { LoadLibraryW(w!("winsqlite3.dll")) }
            .map_err(|error| format!("Windows' SQLite (winsqlite3.dll) did not load: {error}"))?;
        macro_rules! function {
            ($kind:ty, $name:literal) => {{
                // SAFETY: the library is loaded and the name is
                // NUL-terminated.
                let found =
                    unsafe { GetProcAddress(library, PCSTR(concat!($name, "\0").as_ptr())) };
                let Some(found) = found else {
                    return Err(format!("winsqlite3.dll has no {}", $name));
                };
                // SAFETY: the function of that name has the signature of
                // the field it is put in: SQLite's documented C function,
                // with the calling convention winsqlite3.dll exports it in.
                unsafe { std::mem::transmute::<unsafe extern "system" fn() -> isize, $kind>(found) }
            }};
        }
        Ok(Self {
            open: function!(Open, "sqlite3_open_v2"),
            prepare: function!(Prepare, "sqlite3_prepare_v2"),
            bind_int64: function!(BindInt64, "sqlite3_bind_int64"),
            step: function!(Step, "sqlite3_step"),
            reset: function!(Step, "sqlite3_reset"),
            column_text: function!(ColumnText, "sqlite3_column_text"),
            column_blob: function!(ColumnBlob, "sqlite3_column_blob"),
            column_bytes: function!(ColumnBytes, "sqlite3_column_bytes"),
            column_int64: function!(ColumnInt64, "sqlite3_column_int64"),
            finalize: function!(Finalize, "sqlite3_finalize"),
            close: function!(Close, "sqlite3_close"),
            errmsg: function!(ErrorMessage, "sqlite3_errmsg"),
            busy_timeout: function!(BusyTimeout, "sqlite3_busy_timeout"),
        })
    }
}

/// An open connection, closed when it is dropped.
struct Connection {
    sqlite: &'static Sqlite,
    handle: *mut c_void,
}

impl Connection {
    /// Opens the database at `path` to read it only.
    fn open(path: &Path) -> Result<Self, String> {
        let sqlite = Sqlite::get()?;
        let uri = CString::new(database_uri(path))
            .map_err(|_| "the path of the notification database cannot be named".to_owned())?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: the name is NUL-terminated, `handle` outlives the call,
        // and the default file system is asked for with a null name.
        let status =
            unsafe { (sqlite.open)(uri.as_ptr(), &raw mut handle, OPEN_FLAGS, std::ptr::null()) };
        let connection = Self { sqlite, handle };
        if status != SQLITE_OK {
            return Err(format!(
                "Windows' notification database did not open: {}",
                connection.error()
            ));
        }
        // SAFETY: the connection is open.
        unsafe { (sqlite.busy_timeout)(connection.handle, BUSY_WAIT) };
        Ok(connection)
    }

    /// What SQLite says of the last failure.
    fn error(&self) -> String {
        if self.handle.is_null() {
            return "out of memory".to_owned();
        }
        // SAFETY: the connection exists; the message is NUL-terminated and
        // copied before the next call.
        unsafe {
            let message = (self.sqlite.errmsg)(self.handle);
            match message.is_null() {
                true => String::new(),
                false => CStr::from_ptr(message).to_string_lossy().into_owned(),
            }
        }
    }

    /// Prepares `sql`.
    fn prepare(&self, sql: &str) -> Result<Statement<'_>, String> {
        let text = CString::new(sql).map_err(|_| "a query has a NUL".to_owned())?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: the connection is open, the text is NUL-terminated
        // (length -1), and `handle` outlives the call.
        let status = unsafe {
            (self.sqlite.prepare)(
                self.handle,
                text.as_ptr(),
                -1,
                &raw mut handle,
                std::ptr::null_mut(),
            )
        };
        if status != SQLITE_OK || handle.is_null() {
            return Err(self.error());
        }
        Ok(Statement {
            connection: self,
            handle,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: every statement borrowed the connection and is finalized
        // before; closing a null handle is a harmless no-op.
        unsafe { (self.sqlite.close)(self.handle) };
    }
}

/// A prepared statement, finalized when it is dropped.
struct Statement<'a> {
    connection: &'a Connection,
    handle: *mut c_void,
}

impl Statement<'_> {
    /// Binds `value` to the first parameter, after a reset.
    fn bind(&self, value: i64) -> Result<(), String> {
        let sqlite = self.connection.sqlite;
        // SAFETY: the statement exists and has a first parameter.
        let status = unsafe {
            (sqlite.reset)(self.handle);
            (sqlite.bind_int64)(self.handle, 1, value)
        };
        match status {
            SQLITE_OK => Ok(()),
            _ => Err(self.connection.error()),
        }
    }

    /// Steps to the next row: `true` while there is one.
    fn next(&self) -> Result<bool, String> {
        // SAFETY: the statement exists.
        match unsafe { (self.connection.sqlite.step)(self.handle) } {
            SQLITE_ROW => Ok(true),
            SQLITE_DONE => Ok(false),
            _ => Err(self.connection.error()),
        }
    }

    /// The bytes of column `at` of the row, as text or a blob holds them.
    fn bytes(&self, at: c_int) -> Vec<u8> {
        let sqlite = self.connection.sqlite;
        // SAFETY: the statement is on a row; the pointer is valid for the
        // length SQLite gives until the next step, and is copied now.
        unsafe {
            let data = (sqlite.column_blob)(self.handle, at);
            let length = (sqlite.column_bytes)(self.handle, at).max(0) as usize;
            if data.is_null() || length == 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(data.cast::<u8>(), length).to_vec()
        }
    }

    /// Column `at` of the row as text.
    fn text(&self, at: c_int) -> String {
        let sqlite = self.connection.sqlite;
        // SAFETY: as for `bytes`; the text is asked for first, so that its
        // length is that of the text.
        unsafe {
            let data = (sqlite.column_text)(self.handle, at);
            let length = (sqlite.column_bytes)(self.handle, at).max(0) as usize;
            if data.is_null() {
                return String::new();
            }
            String::from_utf8_lossy(std::slice::from_raw_parts(data, length)).into_owned()
        }
    }

    fn int(&self, at: c_int) -> i64 {
        // SAFETY: the statement is on a row.
        unsafe { (self.connection.sqlite.column_int64)(self.handle, at) }
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        // SAFETY: the statement was prepared and is finalized once.
        unsafe { (self.connection.sqlite.finalize)(self.handle) };
    }
}

/// The address SQLite opens `path` by, read only: `file:` with the path
/// written with forward slashes and the letters an address may not hold
/// escaped.
fn database_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file:");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for letter in path.chars() {
        match letter {
            '%' | '?' | '#' | ' ' | '&' | '=' => uri.push_str(&format!("%{:02X}", letter as u32)),
            _ => uri.push(letter),
        }
    }
    uri.push_str("?mode=ro");
    uri
}

/// The database file of the signed-in user.
fn database_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|local| !local.is_empty())?;
    Some(PathBuf::from(local).join(DATABASE))
}

/// The column names of the table `table`, in small letters.
fn columns(connection: &Connection, table: &str) -> Vec<String> {
    let query = format!("SELECT name FROM pragma_table_info('{table}')");
    let Ok(statement) = connection.prepare(&query) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    while let Ok(true) = statement.next() {
        names.push(statement.text(0).to_lowercase());
    }
    names
}

/// The query of the values a notification bound, where the table of them
/// has a layout Tessera knows: a column of the notification, of the key
/// and of the value.
fn data_query(columns: &[String]) -> Option<String> {
    fn find(columns: &[String], names: &[&'static str]) -> Option<&'static str> {
        { names.iter() }
            .find(|name| columns.iter().any(|column| column == *name))
            .copied()
    }
    let notification = find(columns, &["notificationid", "notification", "id"])?;
    let key = find(columns, &["key", "name"])?;
    let value = find(columns, &["value", "data"])?;
    Some(format!(
        "SELECT \"{key}\", \"{value}\" FROM NotificationData WHERE \"{notification}\" = ?1"
    ))
}

/// Reads the toasts of the notifications `ids` from the database. A
/// notification it does not hold is left out.
fn read_now(ids: &[u32]) -> Result<Vec<ToastPayload>, String> {
    let path = database_path().ok_or("there is no folder of local application data")?;
    let connection = Connection::open(&path)?;
    let toasts = connection
        .prepare(
            "SELECT n.Id, h.PrimaryId, n.Payload FROM Notification AS n \
             JOIN NotificationHandler AS h ON h.RecordId = n.HandlerId WHERE n.Id = ?1",
        )
        .map_err(|error| {
            format!("Windows' notification database has another layout than Tessera reads: {error}")
        })?;
    let data = data_query(&columns(&connection, "NotificationData"))
        .and_then(|query| connection.prepare(&query).ok());
    let mut found = Vec::new();
    for &id in ids {
        toasts.bind(i64::from(id))?;
        if !toasts.next()? {
            continue;
        }
        let read_id = toasts.int(0);
        let app_id = toasts.text(1);
        let mut payload = toasts.bytes(2);
        if payload.len() > PAYLOAD_LIMIT || read_id != i64::from(id) {
            continue;
        }
        // A payload written as UTF-16 starts with its byte order mark.
        let xml = match payload.as_slice() {
            [0xFF, 0xFE, rest @ ..] => {
                let units: Vec<u16> = rest
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            _ => {
                while payload.last() == Some(&0) {
                    payload.pop();
                }
                String::from_utf8_lossy(&payload).into_owned()
            }
        };
        let mut bound = HashMap::new();
        if let Some(data) = &data
            && data.bind(i64::from(id)).is_ok()
        {
            while let Ok(true) = data.next() {
                bound.insert(data.text(0), data.text(1));
            }
        }
        found.push(ToastPayload {
            id,
            app_id,
            xml,
            data: bound,
        });
    }
    Ok(found)
}

/// A request to the thread of the database: the notifications to read,
/// and where the answer goes.
type Request = (
    Vec<u32>,
    async_channel::Sender<Result<Vec<ToastPayload>, String>>,
);

/// The thread that reads the database, started with the first request.
static READER: LazyLock<Mutex<Option<mpsc::Sender<Request>>>> = LazyLock::new(Mutex::default);

/// The titles of the notifications whose toast was read, for the check of
/// the self test.
static SEEN_TITLES: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Mutex::default);

/// How many titles are kept for the self test.
const TITLES_KEPT: usize = 64;

/// Windows' notification database.
pub struct NotificationDatabase;

impl NotificationDatabase {
    /// Reads the toasts of the notifications `ids` on the thread of the
    /// database, never on the caller's; the receiver yields the answer
    /// once. Await it; never block the thread of GPUI on it.
    pub fn read(ids: Vec<u32>) -> Receiver<Result<Vec<ToastPayload>, String>> {
        let (answer, answered) = async_channel::bounded(1);
        let mut reader = READER.lock().unwrap_or_else(PoisonError::into_inner);
        let request = (ids, answer);
        let request = match reader.as_ref() {
            Some(sender) => match sender.send(request) {
                Ok(()) => return answered,
                Err(mpsc::SendError(request)) => request,
            },
            None => request,
        };
        let (sender, requests) = mpsc::channel::<Request>();
        let started = thread::Builder::new()
            .name("notification database".to_owned())
            .spawn(move || {
                for (ids, answer) in requests {
                    answer.try_send(read_now(&ids)).ok();
                }
            });
        match started {
            Ok(_) => {
                sender.send(request).ok();
                *reader = Some(sender);
            }
            Err(error) => {
                request
                    .1
                    .try_send(Err(format!(
                        "no thread reads the notification database: {error}"
                    )))
                    .ok();
            }
        }
        answered
    }

    /// Notes that the toast of a notification titled `title` was read.
    pub fn saw(title: &str) {
        let mut titles = SEEN_TITLES.lock().unwrap_or_else(PoisonError::into_inner);
        if !titles.iter().any(|seen| seen == title) {
            titles.push(title.to_owned());
            if titles.len() > TITLES_KEPT {
                titles.remove(0);
            }
        }
    }

    /// Whether the toast of a notification titled `title` was read from
    /// the database while Tessera runs.
    pub fn seen(title: &str) -> bool {
        let titles = SEEN_TITLES.lock().unwrap_or_else(PoisonError::into_inner);
        titles.iter().any(|seen| seen == title)
    }
}

/// The install folders of packages, by family name, as found once.
static PACKAGE_FOLDERS: LazyLock<Mutex<HashMap<String, Option<PathBuf>>>> =
    LazyLock::new(Mutex::default);

/// The family of the packaged application `app_id`: what stands before
/// its `!`.
fn family_of(app_id: &str) -> Option<&str> {
    let (family, app) = app_id.split_once('!')?;
    (family.contains('_') && !app.is_empty()).then_some(family)
}

/// The folder the package of the application `app_id` is installed in,
/// which `ms-appx:///` names, found with the package manager of Windows
/// and kept; `None` for an application that is no package. It waits for
/// Windows, so it is for a thread away from the user interface.
pub fn package_folder(app_id: &str) -> Option<PathBuf> {
    let family = family_of(app_id)?.to_owned();
    if let Some(known) = PACKAGE_FOLDERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&family)
    {
        return known.clone();
    }
    let find = || -> windows::core::Result<Option<PathBuf>> {
        let manager = PackageManager::new()?;
        let packages = manager.FindPackagesByUserSecurityIdPackageFamilyName(
            &HSTRING::new(),
            &HSTRING::from(&family),
        )?;
        let first = packages.into_iter().next();
        let Some(package) = first else {
            return Ok(None);
        };
        Ok(Some(PathBuf::from(
            package.InstalledLocation()?.Path()?.to_string(),
        )))
    };
    let folder = find().ok().flatten();
    PACKAGE_FOLDERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(family, folder.clone());
    folder
}

/// The `LocalState` folder of the package of the application `app_id`,
/// which `ms-appdata:///local/` names.
pub fn package_data_folder(app_id: &str) -> Option<PathBuf> {
    let family = family_of(app_id)?;
    let local = std::env::var_os("LOCALAPPDATA").filter(|local| !local.is_empty())?;
    Some(
        PathBuf::from(local)
            .join("Packages")
            .join(family)
            .join("LocalState"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_is_opened_by_an_address_that_reads_only() {
        let path = Path::new(
            r"C:\Users\Ada Lovelace\AppData\Local\Microsoft\Windows\Notifications\wpndatabase.db",
        );
        assert_eq!(
            database_uri(path),
            "file:///C:/Users/Ada%20Lovelace/AppData/Local/Microsoft/Windows/Notifications/wpndatabase.db?mode=ro"
        );
        assert_eq!(
            database_uri(Path::new(r"C:\a#b\c%d")),
            "file:///C:/a%23b/c%25d?mode=ro"
        );
    }

    #[test]
    fn the_bound_values_are_read_where_their_table_has_a_known_layout() {
        let names = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            data_query(&names(&["notificationid", "key", "value"])).as_deref(),
            Some("SELECT \"key\", \"value\" FROM NotificationData WHERE \"notificationid\" = ?1")
        );
        assert!(data_query(&names(&["recordid", "blob"])).is_none());
        assert!(data_query(&[]).is_none());
    }

    #[test]
    fn a_package_family_is_what_stands_before_the_application() {
        assert_eq!(
            family_of("Microsoft.WindowsAlarms_8wekyb3d8bbwe!App"),
            Some("Microsoft.WindowsAlarms_8wekyb3d8bbwe")
        );
        assert_eq!(family_of("Microsoft.Office.OUTLOOK.EXE.15"), None);
        assert_eq!(family_of("chrome"), None);
    }
}
