// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The font families installed in Windows, as DirectWrite lists them, for
//! the settings to offer.

#![allow(unsafe_code)]

use std::sync::OnceLock;

use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection,
    IDWriteLocalizedStrings,
};
use windows::core::{BOOL, PCWSTR, w};

/// The longest name of a locale, with its end, as Windows counts it.
const LOCALE_NAME_LENGTH: usize = 85;

/// The names of the font families installed, each once, sorted the way
/// the settings show them; read the first time it is asked for and kept.
/// The first call reads every family of the system's collection and can
/// take a moment, so it is not to be made on the thread that draws.
pub fn font_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| match read_families() {
        Ok(families) => families,
        Err(error) => {
            report!("the installed fonts cannot be listed: {error}");
            Vec::new()
        }
    })
}

fn read_families() -> windows::core::Result<Vec<String>> {
    // SAFETY: a shared factory takes no arguments but its type.
    let factory: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
    let mut collection: Option<IDWriteFontCollection> = None;
    // SAFETY: `collection` outlives the call, which fills it.
    unsafe { factory.GetSystemFontCollection(&raw mut collection, false)? };
    let Some(collection) = collection else {
        return Ok(Vec::new());
    };
    let locale = user_locale();
    // SAFETY: the collection is a valid COM object for as long as it is
    // held.
    let count = unsafe { collection.GetFontFamilyCount() };
    let mut families = Vec::new();
    for index in 0..count {
        // SAFETY: `index` is below the count the collection gave.
        let Ok(family) = (unsafe { collection.GetFontFamily(index) }) else {
            continue;
        };
        // SAFETY: the family is a valid COM object for as long as it is
        // held.
        let Ok(names) = (unsafe { family.GetFamilyNames() }) else {
            continue;
        };
        if let Some(name) = family_name(&names, &locale) {
            families.push(name);
        }
    }
    families.sort_by_key(|name| name.to_lowercase());
    families.dedup();
    Ok(families)
}

/// The user's locale, such as `ru-RU`, with its end; empty where Windows
/// does not tell it.
fn user_locale() -> Vec<u16> {
    let mut locale = [0_u16; LOCALE_NAME_LENGTH];
    // SAFETY: the buffer is as long as the call is told.
    let length = unsafe { GetUserDefaultLocaleName(&mut locale) };
    let length = usize::try_from(length).unwrap_or(0);
    // The length counts the end.
    locale[..length].to_vec()
}

/// The name of a family in `locale`, or else in American English, or else
/// its first name.
fn family_name(names: &IDWriteLocalizedStrings, locale: &[u16]) -> Option<String> {
    let find = |locale: PCWSTR| {
        let (mut index, mut exists) = (0_u32, BOOL(0));
        // SAFETY: the out-parameters outlive the call, and `locale` ends
        // with a zero.
        let found = unsafe { names.FindLocaleName(locale, &raw mut index, &raw mut exists) };
        (found.is_ok() && exists.as_bool()).then_some(index)
    };
    let own = (!locale.is_empty())
        .then(|| find(PCWSTR(locale.as_ptr())))
        .flatten();
    let index = own.or_else(|| find(w!("en-us"))).unwrap_or(0);
    // SAFETY: the strings are a valid COM object while they are held.
    if index >= unsafe { names.GetCount() } {
        return None;
    }
    // SAFETY: `index` is one of the strings.
    let length = unsafe { names.GetStringLength(index) }.ok()?;
    let mut name = vec![0_u16; length as usize + 1];
    // SAFETY: the buffer holds the string and its end.
    unsafe { names.GetString(index, &mut name) }.ok()?;
    name.truncate(length as usize);
    let name = String::from_utf16_lossy(&name);
    (!name.trim().is_empty()).then_some(name)
}
