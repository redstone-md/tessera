// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::{
    AppWindow, AppWindowId, AudioOutput, Battery, DisplayKey, EdgeReservation, IconPixels, Link,
    Media, MediaCommand, MemoryLoad, NativeSurface, Network, Notification, Notifications,
    NowPlaying, PowerAction, ProcessorLoad, ScreenEdge, ScreenRect, Shortcut, Taskbar, Thumbnail,
    Tray, TrayButton, TrayIcon, WindowEvents, accent_color, light_mode,
};
