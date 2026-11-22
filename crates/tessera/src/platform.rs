// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::{
    Account, AppSound, AppWindow, AppWindowId, AudioDevice, AudioOutput, BackdropPicture, Battery,
    Bluetooth, Carrier, Connection, DesktopBackdrop, DesktopVideo, DisplayKey, EdgeReservation,
    Hotkey, Hotkeys, IconPixels, InputLanguage, JumpTask, Link, Media, MediaCommand, MemoryLoad,
    Modifiers, NativeSurface, Network, Notification, Notifications, NowPlaying, Pace, PairedDevice,
    Passage, Place, Position, PowerAction, ProcessorLoad, RecentDocument, ScreenBrightness,
    ScreenEdge, ScreenRect, Shortcut, SmallBirth, Taskbar, Thumbnail, Tray, TrayButton, TrayIcon,
    Typing, Veil, VideoState, VirtualDesktops, Vpn, VpnState, Wifi, WifiNetwork, WifiReach,
    WindowEvents, WindowsKey, Wireless, accent_color, app_id_in, app_id_of_program, app_path,
    desktop_look, desktop_pixels, display_frames, light_mode, memory_held, pointer_at,
    pointer_held, pointer_on_own_window, read_desktop_if_requested,
};
