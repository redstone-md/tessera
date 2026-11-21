// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::{
    AppSound, AppWindow, AppWindowId, AudioDevice, AudioOutput, BackdropPicture, Battery,
    Bluetooth, Carrier, Connection, DesktopBackdrop, DisplayKey, EdgeReservation, Hotkey, Hotkeys,
    IconPixels, InputLanguage, JumpTask, Link, Media, MediaCommand, MemoryLoad, Modifiers,
    NativeSurface, Network, Notification, Notifications, NowPlaying, Pace, PairedDevice, Passage,
    Position, PowerAction, ProcessorLoad, RecentDocument, ScreenBrightness, ScreenEdge, ScreenRect,
    Shortcut, Taskbar, Thumbnail, Tray, TrayButton, TrayIcon, Typing, Veil, VirtualDesktops, Vpn,
    VpnState, Wifi, WifiNetwork, WifiReach, WindowEvents, WindowsKey, Wireless, accent_color,
    app_id_in, app_id_of_program, app_path, desktop_pixels, display_frames, light_mode, pointer_at,
    pointer_held,
};
