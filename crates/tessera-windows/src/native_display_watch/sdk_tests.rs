// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! SDK projection/ABI compilation ONLY; never invoke a native function.

use windows::Devices::Display::Core::{
    DisplayManager, DisplayManagerChangedEventArgs, DisplayManagerDisabledEventArgs,
    DisplayManagerEnabledEventArgs, DisplayManagerOptions,
    DisplayManagerPathsFailedOrInvalidatedEventArgs,
};
use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::UISettings;
use windows::Win32::System::WinRT::{
    RO_INIT_MULTITHREADED, RO_INIT_TYPE, RoInitialize, RoUninitialize,
};
use windows::core::{IInspectable, Result};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{INFINITE, ResetEvent, SetEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, QS_ALLINPUT, WM_DISPLAYCHANGE,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};

#[test]
fn display_watch_sdk_winrt_method_signatures_compile_without_calls() {
    let _: unsafe fn(RO_INIT_TYPE) -> Result<()> = RoInitialize;
    let _: unsafe fn() = RoUninitialize;
    let _: fn(DisplayManagerOptions) -> Result<DisplayManager> = DisplayManager::Create;
    let _: fn() -> Result<UISettings> = UISettings::new;
    let _: fn(&DisplayManager) -> Result<()> = DisplayManager::Start;
    let _: fn(&DisplayManager) -> Result<()> = DisplayManager::Stop;
    let _: fn(&DisplayManager) -> Result<()> = DisplayManager::Close;
    let _: fn(&DisplayManager, i64) -> Result<()> = DisplayManager::RemoveEnabled;
    let _: fn(&DisplayManager, i64) -> Result<()> = DisplayManager::RemoveDisabled;
    let _: fn(&DisplayManager, i64) -> Result<()> = DisplayManager::RemoveChanged;
    let _: fn(&DisplayManager, i64) -> Result<()> = DisplayManager::RemovePathsFailedOrInvalidated;
    let _: fn(&UISettings, i64) -> Result<()> = UISettings::RemoveTextScaleFactorChanged;
    let _: fn(&DisplayManagerEnabledEventArgs, bool) -> Result<()> =
        DisplayManagerEnabledEventArgs::SetHandled;
    let _: fn(&DisplayManagerDisabledEventArgs, bool) -> Result<()> =
        DisplayManagerDisabledEventArgs::SetHandled;
    let _: fn(&DisplayManagerChangedEventArgs, bool) -> Result<()> =
        DisplayManagerChangedEventArgs::SetHandled;
    let _: fn(&DisplayManagerPathsFailedOrInvalidatedEventArgs, bool) -> Result<()> =
        DisplayManagerPathsFailedOrInvalidatedEventArgs::SetHandled;
    // Refer to generic registrations without constructing WinRT delegates or
    // event args. Production compilation instantiates these exact projections.
    let _ = DisplayManager::Enabled::<
        &TypedEventHandler<DisplayManager, DisplayManagerEnabledEventArgs>,
    >;
    let _ = DisplayManager::Disabled::<
        &TypedEventHandler<DisplayManager, DisplayManagerDisabledEventArgs>,
    >;
    let _ = DisplayManager::Changed::<
        &TypedEventHandler<DisplayManager, DisplayManagerChangedEventArgs>,
    >;
    let _ = DisplayManager::PathsFailedOrInvalidated::<
        &TypedEventHandler<DisplayManager, DisplayManagerPathsFailedOrInvalidatedEventArgs>,
    >;
    let _ = UISettings::TextScaleFactorChanged::<&TypedEventHandler<UISettings, IInspectable>>;
}

#[test]
fn display_watch_sdk_kernel_wait_signatures_and_hidden_top_level_flags_without_calls() {
    let _: unsafe extern "system" fn(HANDLE) -> i32 = SetEvent;
    let _: unsafe extern "system" fn(HANDLE) -> i32 = ResetEvent;
    let _: unsafe extern "system" fn(HANDLE) -> i32 = CloseHandle;
    let _: unsafe extern "system" fn(u32, *const HANDLE, u32, u32, u32) -> u32 =
        MsgWaitForMultipleObjectsEx;
    assert_eq!(RO_INIT_MULTITHREADED.0, 1);
    assert_eq!(DisplayManagerOptions::None.0, 0);
    assert_eq!(INFINITE, u32::MAX);
    assert_eq!(WM_DISPLAYCHANGE, 0x007e);
    assert_ne!(MWMO_INPUTAVAILABLE, 0);
    assert_ne!(QS_ALLINPUT, 0);
    assert_eq!(WS_POPUP & WS_VISIBLE, 0);
    assert_ne!(WS_EX_NOACTIVATE, 0);
    assert_ne!(WS_EX_TOOLWINDOW, 0);
}
