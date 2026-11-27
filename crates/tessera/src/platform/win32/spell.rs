// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The spell checker of Windows (`ISpellCheckerFactory`), in the
//! languages of the user that it has dictionaries for: the languages of
//! the list in the settings of Windows, in their order.

use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result};
use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_GET_SUGGESTIONS, CORRECTIVE_ACTION_REPLACE, ISpellChecker,
    ISpellCheckerFactory, SpellCheckerFactory,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx, CoUninitialize, IEnumString,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SAM_FLAGS};
use windows::core::{HSTRING, PWSTR};

use super::packaged::take;
use super::registry::Key;

/// Where Windows keeps the list of the languages of the user.
const PROFILE: &str = r"Control Panel\International\User Profile";

/// How many suggestions are read at most.
const SUGGESTIONS: usize = 8;

/// What the spell checker says of a word in one language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spelling {
    /// The language, as a tag such as `en-US`.
    pub language: String,
    /// Whether the word is spelled right in it.
    pub right: bool,
    /// The words it may be meant as, the likeliest first; a word to be
    /// replaced comes with the one replacement.
    pub suggestions: Vec<String>,
}

/// The spell checker of Windows.
pub struct SpellCheck;

impl SpellCheck {
    /// What the spell checker of each language of the user that has a
    /// dictionary says of `word`. Fails where there is no spell checker;
    /// it is made in the calling thread, which must not be the thread of
    /// GPUI.
    pub fn word(word: &str) -> Result<Vec<Spelling>> {
        // SAFETY: COM is initialized for the duration of the call on this
        // thread and released only when that succeeded, after everything
        // made with it is gone.
        unsafe {
            let initialized =
                CoInitializeEx(None, COINIT_MULTITHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
            let checked = check(word);
            if initialized {
                CoUninitialize();
            }
            checked
        }
    }
}

/// The tags of the languages of the user, as the list in the settings
/// of Windows orders them, read once.
fn user_languages() -> Vec<String> {
    static READ: Mutex<Option<Vec<String>>> = Mutex::new(None);
    let mut read = READ.lock().unwrap_or_else(PoisonError::into_inner);
    read.get_or_insert_with(|| {
        let key = Key::read(HKEY_CURRENT_USER, PROFILE, REG_SAM_FLAGS(0));
        let listed = key.and_then(|key| key.bytes("Languages"));
        listed.map(|bytes| texts_of(&bytes)).unwrap_or_default()
    })
    .clone()
}

/// The texts of a value of several strings, each ended by a NUL.
fn texts_of(bytes: &[u8]) -> Vec<String> {
    let units: Vec<u16> = { bytes.chunks_exact(2) }
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    { units.split(|&unit| unit == 0) }
        .filter(|text| !text.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// The texts `strings` gives, at most `most`.
///
/// # Safety
///
/// COM must be initialized on the calling thread.
unsafe fn read_strings(strings: &IEnumString, most: usize) -> Vec<String> {
    let mut read = Vec::new();
    while read.len() < most {
        let mut text = [PWSTR::null()];
        let mut fetched = 0;
        // SAFETY: one string is asked for and written into `text`, which
        // `take` frees.
        let next = unsafe { strings.Next(&mut text, Some(&raw mut fetched)) };
        if next.is_err() || fetched == 0 {
            break;
        }
        read.extend(take(text[0]));
    }
    read
}

/// Checks `word` in every language of the user that the spell checker
/// knows.
///
/// # Safety
///
/// COM must be initialized on the calling thread.
unsafe fn check(word: &str) -> Result<Vec<Spelling>> {
    // SAFETY: COM is initialized; every text handed over outlives its
    // call, and what is returned is freed when it is dropped.
    unsafe {
        let factory: ISpellCheckerFactory =
            CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER)
                .context("Windows has no spell checker")?;
        let mut languages = user_languages();
        if languages.is_empty() {
            // Without a list, the languages of the dictionaries there are.
            languages = read_strings(&factory.SupportedLanguages()?, 4);
        }
        let text = HSTRING::from(word);
        let mut said = Vec::new();
        for language in languages {
            let tag = HSTRING::from(&language);
            if !factory
                .IsSupported(&tag)
                .is_ok_and(|supported| supported.as_bool())
            {
                continue;
            }
            let checker: ISpellChecker = factory.CreateSpellChecker(&tag)?;
            let errors = checker.Check(&text)?;
            let mut error = None;
            // The first error tells of the word; there is one word.
            let found = errors.Next(&raw mut error).is_ok() && error.is_some();
            let suggestions = match error {
                Some(error) if found => match error.CorrectiveAction()? {
                    CORRECTIVE_ACTION_REPLACE => take(error.Replacement()?).into_iter().collect(),
                    CORRECTIVE_ACTION_GET_SUGGESTIONS => {
                        read_strings(&checker.Suggest(&text)?, SUGGESTIONS)
                    }
                    // A word to delete, as a repeated one, has nothing
                    // to suggest.
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            said.push(Spelling {
                language,
                right: !found,
                suggestions,
            });
        }
        Ok(said)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_of_several_strings_is_split_at_its_nuls() {
        let bytes: Vec<u8> = { "en-US\0ru\0\0".encode_utf16() }
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(texts_of(&bytes), ["en-US", "ru"]);
        assert!(texts_of(&[]).is_empty());
    }

    /// Asks the spell checker of this computer.
    #[test]
    #[ignore = "asks the spell checker of the computer it runs on"]
    fn the_spell_checker_of_this_computer_answers() {
        let said = SpellCheck::word("necesary").expect("there is a spell checker");
        println!("{said:#?}");
    }
}
