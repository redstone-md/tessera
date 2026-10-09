// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Pure SDK/type contracts only: these tests never initialize COM or switch input.

use super::decoded;
use windows::{
    Win32::UI::TextServices::{
        GUID_TFCAT_TIP_KEYBOARD, TF_IPP_FLAG_ACTIVE, TF_IPP_FLAG_ENABLED, TF_IPPMF_FORPROCESS,
        TF_IPPMF_FORSESSION, TF_PROFILETYPE_INPUTPROCESSOR, TF_PROFILETYPE_KEYBOARDLAYOUT,
    },
    core::HRESULT,
};

#[test]
fn private_recording_constants_match_installed_sdk() {
    use super::super::*;
    assert_eq!(STA, windows::Win32::System::Com::COINIT_APARTMENTTHREADED.0);
    assert_eq!(KEYBOARD_LAYOUT, TF_PROFILETYPE_KEYBOARDLAYOUT);
    assert_eq!(INPUT_PROCESSOR, TF_PROFILETYPE_INPUTPROCESSOR);
    assert_eq!(KEYBOARD_CATEGORY, GUID_TFCAT_TIP_KEYBOARD.to_u128());
    assert_eq!(ACTIVE, TF_IPP_FLAG_ACTIVE);
    assert_eq!(ENABLED, TF_IPP_FLAG_ENABLED);
    assert_eq!(DESKTOP_SCOPE, TF_IPPMF_FORPROCESS | TF_IPPMF_FORSESSION);
}

#[test]
fn s_false_is_nonnegative_but_must_not_be_used_as_activate_success() {
    assert!(HRESULT(1).ok().is_ok());
    // The operation's recording tests assert this exact raw code is failure.
    assert_ne!(HRESULT(1).0, HRESULT(0).0);
}

#[test]
fn utf16_counts_terminators_and_unicode_are_decoded_without_fallback() {
    let label: Vec<u16> = "日本語 — Français".encode_utf16().chain(Some(0)).collect();
    assert_eq!(
        decoded(&label, label.len() as i32).as_deref(),
        Some("日本語 — Français")
    );
    assert_eq!(decoded(&[0], 1).as_deref(), Some(""));
    assert_eq!(decoded(&label, 0), None);
    assert_eq!(decoded(&label, -1), None);
    assert_eq!(decoded(&label, label.len() as i32 + 1), None);
    assert_eq!(decoded(&[65, 66], 2), None);
    assert_eq!(decoded(&[65, 0, 66, 0], 4), None);
    assert_eq!(decoded(&[0xd800, 0], 2), None);
}
