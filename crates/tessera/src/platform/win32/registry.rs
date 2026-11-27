// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Reading the text values of the registry.

#![allow(unsafe_code)]

use windows::Win32::System::Registry::{HKEY, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::PCWSTR;

/// The text value `name` of the key `key` under `root`, or its default
/// value where `name` is `None`, of at most `capacity` UTF-16 units: one
/// of the expanding kind comes with its `%VARIABLES%` expanded. `None`
/// where the key or the value is not there, is of another kind or is
/// longer.
pub(super) fn registry_text(
    root: HKEY,
    key: &str,
    name: Option<&str>,
    capacity: usize,
) -> Option<String> {
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let name: Option<Vec<u16>> = name.map(|name| name.encode_utf16().chain([0]).collect());
    let name = name
        .as_ref()
        .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
    let mut text = vec![0u16; capacity.max(1)];
    let mut size = (text.len() * size_of::<u16>()) as u32;
    // SAFETY: `size` is the size in bytes of `text`, which outlives the
    // call, and the key and the name are NUL-terminated or null. Asked
    // for `RRF_RT_REG_SZ` alone, Windows expands a value of the expanding
    // kind and gives it as a plain one; with `RRF_RT_REG_EXPAND_SZ`
    // added it would refuse the call without `RRF_NOEXPAND`.
    let status = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            name,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&raw mut size),
        )
    };
    status.ok().ok()?;
    let length = text.iter().position(|&unit| unit == 0)?;
    Some(String::from_utf16_lossy(&text[..length]))
}
