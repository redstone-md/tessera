// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::{
    AppWindow, AppWindowId, AudioOutput, BackdropPicture, Battery, Bluetooth, DesktopBackdrop,
    DisplayKey, EdgeReservation, IconPixels, InputLanguage, JumpTask, Link, Media, MediaCommand,
    MemoryLoad, NativeSurface, Network, Notification, Notifications, NowPlaying, PairedDevice,
    Position, PowerAction, ProcessorLoad, RecentDocument, ScreenEdge, ScreenRect, Shortcut,
    Taskbar, Thumbnail, Tray, TrayButton, TrayIcon, Typing, Veil, VirtualDesktops, Wifi,
    WifiNetwork, WifiReach, WindowEvents, WindowsKey, accent_color, app_id_in, app_id_of_program,
    app_path, display_frames, light_mode, pointer_held,
};
