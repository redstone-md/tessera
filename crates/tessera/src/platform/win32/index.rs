// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The index of Windows Search, asked for files by a part of their names,
//! anywhere or below a folder, and by what they hold and their
//! properties: [`FileSearch`] reads a search such as `content: budget
//! kind:document modified:>2026-01-01 size:>10MB in:~/Work` and makes the
//! question of the index of it, every value written in quotes the query
//! language of Windows Search cannot be broken out of.

use std::mem::offset_of;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use chrono::{Days, Local, Months, NaiveDate, NaiveDateTime, TimeZone};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED,
    CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Search::{
    CSearchManager, DBACCESSOR_ROWDATA, DBBINDING, DBMEMOWNER_CLIENTOWNED, DBPARAMIO_NOTPARAM,
    DBPART_LENGTH, DBPART_STATUS, DBPART_VALUE, DBSTATUS_S_OK, DBTYPE_WSTR, HACCESSOR, IAccessor,
    ICommandText, IDBCreateCommand, IDBCreateSession, IDBInitialize, IDataInitialize, IRowset,
    ISearchManager, MSDAINITIALIZE,
};
use windows::core::{GUID, HSTRING, IUnknown, Interface, w};

/// The dialect a provider reads a command in unless told another.
const DBGUID_DEFAULT: GUID = GUID::from_u128(0xc8b521fb_5cf3_11ce_ade5_00aa0044773d);

/// How many letters of an address are read; a longer one is left out.
const ADDRESS_LETTERS: usize = 1024;

/// How many rows are taken from the answer at once.
const ROWS_AT_ONCE: usize = 16;

/// What begins the address of a file in the index.
const FILE_SCHEME: &str = "file:";

/// The catalog of the index that holds the files of the user.
const CATALOG: &str = "SystemIndex";

/// One row of the answer as the provider writes it.
#[repr(C)]
struct Row {
    status: u32,
    /// The length of the address in bytes, without its end.
    length: usize,
    address: [u16; ADDRESS_LETTERS],
}

/// The index Windows Search keeps of the files of the computer.
pub struct SearchIndex;

impl SearchIndex {
    /// The files and folders in the index whose names have, for every
    /// word of `text`, a word that begins with it, at most `limit` of
    /// them, those changed last first. Fails where the index cannot be
    /// asked, as when its service does not run.
    pub fn files_named(text: &str, limit: usize) -> Result<Vec<PathBuf>> {
        Self::answer(question(text, limit, None), limit)
    }

    /// The files and folders `search` asks for, at most `limit` of them:
    /// the best matches first where it looks into what files hold, else
    /// those changed last. Fails where the index cannot be asked.
    pub fn files_matching(search: &FileSearch, limit: usize) -> Result<Vec<PathBuf>> {
        Self::answer(
            search.question(limit, Local::now().naive_local(), to_utc),
            limit,
        )
    }

    /// The same for the files and folders below the folder `root`, at any
    /// depth. Only what the index holds is found: see [`Self::covers`].
    pub fn files_under(root: &Path, text: &str, limit: usize) -> Result<Vec<PathBuf>> {
        Self::answer(question(text, limit, Some(root)), limit)
    }

    /// Whether the index holds the files below `root`, as the places it
    /// is told to index say; `false` also where it cannot be asked.
    pub fn covers(root: &Path) -> bool {
        let address = HSTRING::from(scope_address(root));
        // SAFETY: COM is initialized for the duration of the call on this
        // thread and released only when that succeeded, after everything
        // made with it is gone; the address outlives the call.
        unsafe {
            let initialized =
                CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let covered = (|| -> Result<bool> {
                // The search manager is served by the service of Windows
                // Search, out of this process.
                let manager: ISearchManager =
                    CoCreateInstance(&CSearchManager, None, CLSCTX_LOCAL_SERVER)
                        .context("the search manager of Windows Search cannot be made")?;
                let scopes = manager
                    .GetCatalog(&HSTRING::from(CATALOG))?
                    .GetCrawlScopeManager()?;
                Ok(scopes.IncludedInCrawlScope(&address)?.as_bool())
            })();
            if initialized {
                CoUninitialize();
            }
            covered.unwrap_or_else(|error| {
                // Told once, as every search asks.
                static TOLD: AtomicBool = AtomicBool::new(false);
                if !TOLD.swap(true, Ordering::Relaxed) {
                    report!("Windows Search cannot be asked what it indexes: {error:#}");
                }
                false
            })
        }
    }

    /// Asks the index `question`, where there is one, for at most `limit`
    /// files.
    fn answer(question: Option<String>, limit: usize) -> Result<Vec<PathBuf>> {
        let Some(question) = question else {
            return Ok(Vec::new());
        };
        let question = HSTRING::from(question);
        // SAFETY: COM is initialized for the duration of the call on this
        // thread and released only when that succeeded, after everything
        // made with it is gone.
        unsafe {
            let initialized =
                CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let found = ask(&question, limit);
            if initialized {
                CoUninitialize();
            }
            found
        }
    }
}

/// The address the places the index is told to index name `root` by.
fn scope_address(root: &Path) -> String {
    let path = root.to_string_lossy();
    let path = path.trim_end_matches('\\');
    format!("file:///{path}\\")
}

/// What a query of the index names the folder `root` by in its `SCOPE`,
/// with forward slashes and the quotes of the query doubled.
fn scope_of(root: &Path) -> String {
    let path = root.to_string_lossy().replace('\\', "/");
    // The root of a drive keeps its slash, as `file:C:/`.
    let path = match path.len() > 3 {
        true => path.trim_end_matches('/'),
        false => &path,
    };
    format!("{FILE_SCHEME}{path}").replace('\'', "''")
}

/// Asks the index `question` and reads at most `limit` addresses of the
/// answer.
///
/// # Safety
///
/// COM must be initialized on the calling thread.
unsafe fn ask(question: &HSTRING, limit: usize) -> Result<Vec<PathBuf>> {
    // SAFETY: COM is initialized; every pointer handed over outlives its
    // call; the one column asked for is bound into `Row`, whose size and
    // offsets the binding states, and rows are released once read.
    unsafe {
        let source: IDataInitialize = CoCreateInstance(&MSDAINITIALIZE, None, CLSCTX_INPROC_SERVER)
            .context("OLE DB is missing")?;
        let mut index = None;
        source
            .GetDataSource(
                None,
                CLSCTX_INPROC_SERVER.0,
                w!("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';"),
                &IDBInitialize::IID,
                &raw mut index,
            )
            .context("Windows Search is missing")?;
        let index: IDBInitialize = index.context("Windows Search gave nothing")?.cast()?;
        index.Initialize().context("Windows Search does not run")?;
        let session: IDBCreateCommand = index
            .cast::<IDBCreateSession>()?
            .CreateSession(None, &IDBCreateCommand::IID)?
            .cast()?;
        let command: ICommandText = session.CreateCommand(None, &ICommandText::IID)?.cast()?;
        command.SetCommandText(&DBGUID_DEFAULT, question)?;
        let mut answer: Option<IUnknown> = None;
        command
            .Execute(None, &IRowset::IID, None, None, Some(&raw mut answer))
            .context("the index did not answer")?;
        let rows: IRowset = answer.context("the index answered nothing")?.cast()?;

        let binding = DBBINDING {
            iOrdinal: 1,
            obValue: offset_of!(Row, address),
            obLength: offset_of!(Row, length),
            obStatus: offset_of!(Row, status),
            dwPart: (DBPART_VALUE.0 | DBPART_LENGTH.0 | DBPART_STATUS.0) as u32,
            dwMemOwner: DBMEMOWNER_CLIENTOWNED.0 as u32,
            eParamIO: DBPARAMIO_NOTPARAM.0 as u32,
            cbMaxLen: size_of::<[u16; ADDRESS_LETTERS]>(),
            wType: DBTYPE_WSTR.0 as u16,
            ..Default::default()
        };
        let accessor: IAccessor = rows.cast()?;
        let mut reader = HACCESSOR::default();
        accessor.CreateAccessor(
            DBACCESSOR_ROWDATA.0 as u32,
            1,
            &raw const binding,
            size_of::<Row>(),
            &raw mut reader,
            None,
        )?;

        let mut found = Vec::new();
        let mut row = Row {
            status: 0,
            length: 0,
            address: [0; ADDRESS_LETTERS],
        };
        while found.len() < limit {
            let mut handles = [0usize; ROWS_AT_ONCE];
            // The first of these says where the handles go; as many rows
            // are asked for as there are of them.
            let mut asked = [ptr::null_mut::<usize>(); ROWS_AT_ONCE];
            asked[0] = handles.as_mut_ptr();
            let mut got = 0;
            let taken = rows.GetNextRows(0, 0, &raw mut got, &mut asked);
            for &handle in &handles[..got.min(ROWS_AT_ONCE)] {
                let read = rows.GetData(handle, reader, (&raw mut row).cast());
                if read.is_ok() && row.status == DBSTATUS_S_OK.0 as u32 {
                    let letters = (row.length / size_of::<u16>()).min(ADDRESS_LETTERS);
                    let address = String::from_utf16_lossy(&row.address[..letters]);
                    found.extend(file_at(&address));
                }
            }
            let _ = rows.ReleaseRows(
                got,
                handles.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
            if taken.is_err() || got == 0 {
                break;
            }
        }
        let _ = accessor.ReleaseAccessor(reader, None);
        found.truncate(limit);
        Ok(found)
    }
}

/// What the index is asked for the files whose names have, for every
/// word of `text`, a word that begins with it, below `root` where one is
/// given; `None` where `text` has no word. Asking for letters anywhere in
/// a name instead makes the index read every name, which took it up to
/// twenty seconds.
fn question(text: &str, limit: usize, root: Option<&Path>) -> Option<String> {
    let search = FileSearch {
        words: text.split_whitespace().map(str::to_owned).collect(),
        folder: root.map(Path::to_owned),
        ..FileSearch::default()
    };
    // No date is asked for, so the time does not matter.
    search.question(limit, NaiveDateTime::default(), |local| local)
}

/// The local time `local` in UTC, as the index keeps its dates; one that
/// does not exist, as in the hour the clocks skip, is taken as UTC.
fn to_utc(local: NaiveDateTime) -> NaiveDateTime {
    match Local.from_local_datetime(&local).earliest() {
        Some(time) => time.naive_utc(),
        None => local,
    }
}

/// How a property is compared with a value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    Above,
    AtLeast,
    Below,
    AtMost,
    Equal,
}

impl Comparison {
    /// The comparison at the start of `value`, and the rest; equality
    /// where none is written.
    fn read(value: &str) -> (Self, &str) {
        for (written, comparison) in [
            (">=", Self::AtLeast),
            ("<=", Self::AtMost),
            (">", Self::Above),
            ("<", Self::Below),
            ("=", Self::Equal),
        ] {
            if let Some(rest) = value.strip_prefix(written) {
                return (comparison, rest);
            }
        }
        (Self::Equal, value)
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Above => ">",
            Self::AtLeast => ">=",
            Self::Below => "<",
            Self::AtMost => "<=",
            Self::Equal => "=",
        }
    }
}

/// The kinds of files Windows Search tells apart (`System.Kind`), with
/// other words for some of them.
const KINDS: [(&str, &[&str]); 14] = [
    ("document", &["doc", "docs", "text"]),
    (
        "picture",
        &["image", "images", "photo", "photos", "pictures"],
    ),
    ("video", &["videos", "movie", "movies"]),
    ("music", &["audio", "song", "songs"]),
    ("folder", &["folders", "dir"]),
    ("program", &["app", "apps", "programs"]),
    ("email", &["mail"]),
    ("link", &["links", "shortcut"]),
    ("contact", &["contacts"]),
    ("calendar", &[]),
    ("note", &["notes"]),
    ("playlist", &[]),
    ("game", &["games"]),
    ("webhistory", &[]),
];

/// The kind `name` names, as the index writes it.
fn kind_named(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    { KINDS.iter() }
        .find(|(kind, others)| *kind == name || others.contains(&name.as_str()))
        .map(|(kind, _)| *kind)
}

/// The number of bytes `text` writes, as `10MB`, `1.5 GB` or `4096`, in
/// units of 1024.
fn bytes_written(text: &str) -> Option<u64> {
    let text = text.trim().to_uppercase();
    let digits = text.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','));
    let (number, unit) = text.split_at(digits.unwrap_or(text.len()));
    let number: f64 = number.replace(',', ".").parse().ok()?;
    let power = match unit.trim() {
        "" | "B" => 0,
        "K" | "KB" | "KIB" => 1,
        "M" | "MB" | "MIB" => 2,
        "G" | "GB" | "GIB" => 3,
        "T" | "TB" | "TIB" => 4,
        _ => return None,
    };
    let bytes = number * 1024f64.powi(power);
    (bytes.is_finite() && bytes >= 0. && bytes < u64::MAX as f64).then_some(bytes as u64)
}

/// The day `text` names, as `2026-01-31`, or the first day of a span
/// back from `today`: `today`, `yesterday`, `week`, `month`, `year`.
fn day_written(text: &str, today: NaiveDate) -> Option<(NaiveDate, bool)> {
    let since = |day: Option<NaiveDate>| day.map(|day| (day, true));
    match text.to_lowercase().as_str() {
        "today" => since(Some(today)),
        "yesterday" => since(today.checked_sub_days(Days::new(1))),
        "week" => since(today.checked_sub_days(Days::new(7))),
        "month" => since(today.checked_sub_months(Months::new(1))),
        "year" => since(today.checked_sub_months(Months::new(12))),
        _ => NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .map(|day| (day, false)),
    }
}

/// What is looked for: words of the names or of what files hold, and
/// what their properties must be.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileSearch {
    /// The words or quoted phrases to find, without their quotes.
    pub words: Vec<String>,
    /// Whether the words are looked for in what files hold, and their
    /// properties, rather than in their names alone.
    pub content: bool,
    /// Whether the words are looked for loosely, as any of them in any
    /// form, the way a search with nothing found by name falls back.
    pub loose: bool,
    pub kind: Option<&'static str>,
    /// The extensions, any of which a file may have, without the dot.
    pub extensions: Vec<String>,
    /// How the date of the last change is compared, as typed.
    pub modified: Option<(Comparison, String)>,
    pub size: Option<(Comparison, u64)>,
    /// The folder to look below.
    pub folder: Option<PathBuf>,
}

impl FileSearch {
    /// The search `text` writes: words, `"quoted phrases"`, and the
    /// filters `kind:`, `ext:` (several with commas), `modified:` (with
    /// `>`, `>=`, `<`, `<=` or `=` and a day as `2026-01-31`, or `today`,
    /// `yesterday`, `week`, `month`, `year`), `size:` (with the same
    /// comparisons and `KB`, `MB`, `GB`), `in:` (a folder, `~` for that of
    /// the user) and `content:`, which looks into what files hold. A
    /// filter whose value cannot be read is taken as words.
    pub fn parse(text: &str) -> Self {
        let mut search = Self::default();
        for token in tokens(text) {
            let Some((key, value)) = token.split_once(':') else {
                search.add_words(&token);
                continue;
            };
            let read = match key.to_lowercase().as_str() {
                "content" => {
                    search.content = true;
                    search.add_words(value);
                    true
                }
                "kind" => kind_named(value)
                    .map(|kind| search.kind = Some(kind))
                    .is_some(),
                "ext" => {
                    let extensions: Vec<String> = { value.split(',') }
                        .map(|ext| ext.trim().trim_start_matches('.').to_lowercase())
                        .filter(|ext| !ext.is_empty() && ext.chars().all(is_name_letter))
                        .collect();
                    let read = !extensions.is_empty();
                    search.extensions.extend(extensions);
                    read
                }
                "modified" | "date" => {
                    let (comparison, day) = Comparison::read(value);
                    let fits = day_written(day, NaiveDate::default()).is_some();
                    fits.then(|| search.modified = Some((comparison, day.to_owned())))
                        .is_some()
                }
                "size" => {
                    let (comparison, size) = Comparison::read(value);
                    { bytes_written(size).map(|bytes| search.size = Some((comparison, bytes))) }
                        .is_some()
                }
                "in" => {
                    let folder = folder_named(value);
                    let read = folder.is_some();
                    search.folder = folder.or(search.folder.take());
                    read
                }
                _ => false,
            };
            if !read {
                search.add_words(&token);
            }
        }
        search
    }

    /// Whether the search asks more of the index than words of names.
    pub fn has_filters(&self) -> bool {
        self.content
            || self.kind.is_some()
            || !self.extensions.is_empty()
            || self.modified.is_some()
            || self.size.is_some()
            || self.folder.is_some()
    }

    /// The same search, looking loosely into what files hold.
    pub fn loosely(&self) -> Self {
        Self {
            content: true,
            loose: true,
            ..self.clone()
        }
    }

    fn add_words(&mut self, text: &str) {
        let text = text.trim_matches('"');
        if !text.trim().is_empty() {
            self.words.push(text.to_owned());
        }
    }

    /// What the index is asked, `now` being the local time and `to_utc`
    /// turning a local time into UTC; `None` where the search asks for
    /// nothing at all.
    fn question(
        &self,
        limit: usize,
        now: NaiveDateTime,
        to_utc: impl Fn(NaiveDateTime) -> NaiveDateTime,
    ) -> Option<String> {
        let mut conditions = Vec::new();
        // A quote of either kind would end the text it stands in, and a
        // star already follows every word.
        let terms: Vec<String> = { self.words.iter() }
            .map(|word| word.replace(['"', '*'], "").replace('\'', "''"))
            .filter(|word| !word.trim().is_empty())
            .collect();
        let column = match self.content {
            true => "*",
            false => "System.FileName",
        };
        if self.loose && !terms.is_empty() {
            conditions.push(format!("FREETEXT(*, '{}')", terms.join(" ")));
        } else if !terms.is_empty() {
            // A phrase is looked for whole, a word as the start of one.
            let terms: Vec<String> = { terms.iter() }
                .map(|term| match term.contains(char::is_whitespace) {
                    true => format!("\"{}\"", term.trim()),
                    false => format!("\"{term}*\""),
                })
                .collect();
            conditions.push(format!("CONTAINS({column}, '{}')", terms.join(" AND ")));
        }
        if let Some(kind) = self.kind {
            conditions.push(format!("System.Kind = '{kind}'"));
        }
        if !self.extensions.is_empty() {
            let any: Vec<String> = { self.extensions.iter() }
                .map(|ext| format!("System.FileExtension = '.{ext}'"))
                .collect();
            conditions.push(format!("({})", any.join(" OR ")));
        }
        if let Some((comparison, written)) = &self.modified
            && let Some((day, since)) = day_written(written, now.date())
        {
            let start = to_utc(day.and_time(chrono::NaiveTime::MIN));
            let end = to_utc(day.and_time(chrono::NaiveTime::MIN) + Days::new(1));
            let at = |time: NaiveDateTime| time.format("%Y-%m-%d %H:%M:%S").to_string();
            let field = "System.DateModified";
            conditions.push(match (comparison, since) {
                // A span back from today is since its first day.
                (_, true) => format!("{field} >= '{}'", at(start)),
                (Comparison::Equal, false) => {
                    format!("{field} >= '{}' AND {field} < '{}'", at(start), at(end))
                }
                (Comparison::Above, false) => format!("{field} >= '{}'", at(end)),
                (Comparison::AtLeast, false) => format!("{field} >= '{}'", at(start)),
                (Comparison::Below, false) => format!("{field} < '{}'", at(start)),
                (Comparison::AtMost, false) => format!("{field} < '{}'", at(end)),
            });
        }
        if let Some((comparison, bytes)) = self.size {
            conditions.push(format!("System.Size {} {bytes}", comparison.sql()));
        }
        if conditions.is_empty() {
            return None;
        }
        let scope = self
            .folder
            .as_deref()
            .map_or_else(|| FILE_SCHEME.to_owned(), scope_of);
        let order = match self.content && !terms.is_empty() {
            true => "System.Search.Rank DESC",
            false => "System.DateModified DESC",
        };
        Some(format!(
            "SELECT TOP {limit} System.ItemUrl FROM SystemIndex \
             WHERE SCOPE='{scope}' AND {} ORDER BY {order}",
            conditions.join(" AND ")
        ))
    }
}

/// Whether `c` may stand in an extension.
fn is_name_letter(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// The folder `name` names, a `~` at its start being the folder of the
/// user; `None` for nothing.
fn folder_named(name: &str) -> Option<PathBuf> {
    let name = name.trim().trim_matches('"');
    if name.is_empty() {
        return None;
    }
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from);
    Some(match (name.strip_prefix('~'), home) {
        (Some(rest), Some(home)) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(name),
    })
}

/// The words of `text`, a part in double quotes being one word with its
/// quotes, as `in:"C:\My Files"` or `"annual report"`.
fn tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                token.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            c => token.push(c),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    // The quotes of a value after a colon go with it.
    { tokens.into_iter() }
        .map(|token| match token.split_once(":\"") {
            Some((key, value)) => format!("{key}:{}", value.trim_end_matches('"')),
            None => token,
        })
        .collect()
}

/// The file the index has at `address`, unless that is no file.
fn file_at(address: &str) -> Option<PathBuf> {
    let path = address.strip_prefix(FILE_SCHEME)?;
    // A file of another computer is addressed with two slashes.
    let path = path.replace('/', "\\");
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_question_asks_for_a_word_beginning_with_each_word_written() {
        let asked = question(" it's  \"my*\" ", 30, None).expect("there are words");
        assert!(asked.starts_with("SELECT TOP 30 System.ItemUrl FROM SystemIndex"));
        assert!(asked.contains("SCOPE='file:'"));
        assert!(asked.contains("CONTAINS(System.FileName, '\"it''s*\" AND \"my*\"')"));
        assert_eq!(question(" \" * ", 30, None), None);
    }

    #[test]
    fn a_question_below_a_folder_names_it_in_its_scope() {
        let root = Path::new(r"C:\Users\O'Neil\Documents\");
        let asked = question("report", 10, Some(root)).expect("there is a word");
        assert!(asked.contains("SCOPE='file:C:/Users/O''Neil/Documents'"));
        assert_eq!(scope_address(Path::new(r"D:\Work")), r"file:///D:\Work\");
    }

    #[test]
    fn an_address_of_the_index_is_a_path() {
        let file = file_at("file:C:/Users/someone/Documents/notes.txt");
        let path = PathBuf::from(r"C:\Users\someone\Documents\notes.txt");
        assert_eq!(file, Some(path));
        let shared = file_at("file://server/share/a.txt");
        assert_eq!(shared, Some(PathBuf::from(r"\\server\share\a.txt")));
        assert_eq!(file_at("mapi://somewhere/letter"), None);
        assert_eq!(file_at("file:"), None);
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn asked(text: &str) -> Option<String> {
        FileSearch::parse(text).question(20, day(2026, 3, 15), |local| local)
    }

    #[test]
    fn a_search_is_read_into_words_and_filters() {
        let search = FileSearch::parse(
            r#"budget "annual report" kind:docs ext:pdf,.DOCX size:>=1.5MB modified:<2026-01-31 in:"D:\My Files""#,
        );
        assert_eq!(search.words, ["budget", "annual report"]);
        assert_eq!(search.kind, Some("document"));
        assert_eq!(search.extensions, ["pdf", "docx"]);
        assert_eq!(search.size, Some((Comparison::AtLeast, 1_572_864)));
        assert_eq!(
            search.modified,
            Some((Comparison::Below, "2026-01-31".to_owned()))
        );
        assert_eq!(search.folder, Some(PathBuf::from(r"D:\My Files")));
        assert!(!search.content && search.has_filters());
        // A filter that cannot be read is words, as a path is.
        let odd = FileSearch::parse(r"kind:spaceship size:big C:\Windows");
        assert_eq!(odd.words, ["kind:spaceship", "size:big", r"C:\Windows"]);
        assert!(!odd.has_filters());
        assert!(FileSearch::parse("content: invoice").content);
    }

    #[test]
    fn the_question_of_a_search_quotes_what_is_typed() {
        let names = asked("it's report").unwrap();
        assert!(names.contains("CONTAINS(System.FileName, '\"it''s*\" AND \"report*\"')"));
        assert!(names.ends_with("ORDER BY System.DateModified DESC"));
        let content = asked(r#"content:budget "q1 plan""#).unwrap();
        assert!(content.contains("CONTAINS(*, '\"budget*\" AND \"q1 plan\"')"));
        assert!(content.ends_with("ORDER BY System.Search.Rank DESC"));
        let loose = FileSearch::parse("budget plan").loosely();
        let loose = loose.question(5, day(2026, 3, 15), |local| local).unwrap();
        assert!(loose.contains("FREETEXT(*, 'budget plan')"));
        let filtered = asked("kind:image ext:png size:>10MB in:D:/Shots").unwrap();
        assert!(filtered.contains("SCOPE='file:D:/Shots'"));
        assert!(filtered.contains("System.Kind = 'picture'"));
        assert!(filtered.contains("(System.FileExtension = '.png')"));
        assert!(filtered.contains("System.Size > 10485760"));
        // An extension that would end the quotes is not taken.
        let hostile = FileSearch::parse("ext:pdf'--");
        assert!(hostile.extensions.is_empty());
        assert!(asked("").is_none());
    }

    #[test]
    fn days_are_compared_from_their_start_and_end() {
        let at = |text: &str| asked(text).unwrap();
        assert!(at("modified:2026-02-01").contains(
            "System.DateModified >= '2026-02-01 00:00:00' AND System.DateModified < '2026-02-02 00:00:00'"
        ));
        assert!(
            at("modified:>2026-02-01").contains("System.DateModified >= '2026-02-02 00:00:00'")
        );
        assert!(
            at("modified:<=2026-02-01").contains("System.DateModified < '2026-02-02 00:00:00'")
        );
        assert!(at("modified:week").contains("System.DateModified >= '2026-03-08 00:00:00'"));
        assert!(at("date:today").contains("System.DateModified >= '2026-03-15 00:00:00'"));
        assert_eq!(FileSearch::parse("modified:soon").words, ["modified:soon"]);
    }

    #[test]
    fn sizes_are_read_in_units_of_1024() {
        assert_eq!(bytes_written("4096"), Some(4096));
        assert_eq!(bytes_written("10KB"), Some(10_240));
        assert_eq!(bytes_written("1,5 gb"), Some(1_610_612_736));
        assert_eq!(bytes_written("big"), None);
        assert_eq!(bytes_written("3 parsecs"), None);
    }

    /// Asks the index of this computer; says only how many it found.
    #[test]
    #[ignore = "asks the Windows Search of the computer it runs on"]
    fn the_index_of_this_computer_answers() {
        let found = SearchIndex::files_named("re", 5).expect("the index answers");
        println!("found {}", found.len());
        assert!(found.len() <= 5);
        assert!(found.iter().all(|path| path.is_absolute()));
    }
}
