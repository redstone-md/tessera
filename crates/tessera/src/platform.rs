// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::{
    AppWindow, AppWindowId, AudioOutput, Battery, DisplayKey, EdgeReservation, IconPixels, Media,
    MediaCommand, MemoryLoad, NativeSurface, Notification, Notifications, NowPlaying, PowerAction,
    ProcessorLoad, ScreenEdge, ScreenRect, Shortcut, Taskbar, Tray, TrayButton, TrayIcon,
    WindowEvents, accent_color,
};
