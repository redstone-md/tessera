// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The index of Windows Search, asked for files by a part of their names.

use std::mem::offset_of;
use std::path::PathBuf;
use std::ptr;

use anyhow::{Context as _, Result};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Search::{
    DBACCESSOR_ROWDATA, DBBINDING, DBMEMOWNER_CLIENTOWNED, DBPARAMIO_NOTPARAM, DBPART_LENGTH,
    DBPART_STATUS, DBPART_VALUE, DBSTATUS_S_OK, DBTYPE_WSTR, HACCESSOR, IAccessor, ICommandText,
    IDBCreateCommand, IDBCreateSession, IDBInitialize, IDataInitialize, IRowset, MSDAINITIALIZE,
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
        let Some(question) = question(text, limit) else {
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
/// word of `text`, a word that begins with it; `None` where `text` has
/// no word. Asking for letters anywhere in a name instead makes the
/// index read every name, which took it up to twenty seconds.
fn question(text: &str, limit: usize) -> Option<String> {
    let words: Vec<String> = text
        .split_whitespace()
        // A quote of either kind would end the text it stands in, and a
        // star already follows every word.
        .map(|word| word.replace(['"', '*'], "").replace('\'', "''"))
        .filter(|word| !word.is_empty())
        .map(|word| format!("\"{word}*\""))
        .collect();
    if words.is_empty() {
        return None;
    }
    let words = words.join(" AND ");
    Some(format!(
        "SELECT TOP {limit} System.ItemUrl FROM SystemIndex \
         WHERE SCOPE='{FILE_SCHEME}' AND CONTAINS(System.FileName, '{words}') \
         ORDER BY System.DateModified DESC"
    ))
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
        let asked = question(" it's  \"my*\" ", 30).expect("there are words");
        assert!(asked.starts_with("SELECT TOP 30 System.ItemUrl FROM SystemIndex"));
        assert!(asked.contains("CONTAINS(System.FileName, '\"it''s*\" AND \"my*\"')"));
        assert_eq!(question(" \" * ", 30), None);
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
