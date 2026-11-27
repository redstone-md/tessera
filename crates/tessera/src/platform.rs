// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

#[allow(unused_imports, reason = "for the starts that wait for a program")]
pub use win32::Process;
#[allow(
    unused_imports,
    reason = "for the packages that hand work to the Shell"
)]
pub use win32::ShellThread;
pub use win32::ShowDesktop;
pub use win32::{
    Account, AppSound, AppWindow, AppWindowId, AudioDevice, AudioOutput, BackdropPicture, Battery,
    Bluetooth, Carrier, ChoiceKey, Commands, Connection, DesktopBackdrop, DesktopVideo, DisplayKey,
    Drive, DriveKind, EdgeReservation, FileClipboard, FileWork, Hdr, Hotkey, Hotkeys, IconPixels,
    InputLanguage, JumpTask, Link, Media, MediaCommand, MemoryLoad, Modifiers, NativeSurface,
    Network, NetworkTraffic, Notification, Notifications, NowPlaying, Pace, PairedDevice, Passage,
    Place, Position, PowerAction, PowerPlan, ProcessorLoad, RecentDocument, RecycleBin,
    ScreenBrightness, ScreenEdge, ScreenRect, SearchIndex, Shortcut, SmallBirth, TakenKeys,
    Taskbar, Thumbnail, Tray, TrayButton, TrayIcon, Typing, Veil, VideoState, VirtualDesktops, Vpn,
    VpnState, Wifi, WifiNetwork, WifiReach, WifiSecurity, WindowEvents, WindowsKey, Wireless,
    accent_color, app_id_in, app_id_of_program, app_path, desktop_look, desktop_pixels, disk_space,
    display_frames, is_hidden, light_mode, memory_held, open_terminal, password_fits, pointer_at,
    pointer_held, pointer_on_own_window, read_desktop_if_requested, reveal_in_explorer,
    set_light_mode, show_properties,
};
pub use win32::{
    ExplorerShell, NextShell, SHELL_FLAG, ShellRegistration, attach_console, is_safe_mode,
    own_program_name, recovery_keys_held, started_as_shell,
};
pub use win32::{HostOptions, ShellEvent, ShellHost};
#[allow(
    unused_imports,
    reason = "Show is for the launchers that ask for a window state"
)]
pub use win32::{Launch, Show};
#[allow(
    unused_imports,
    reason = "for the features that ask Explorer while it runs"
)]
pub use win32::{explorer_running, shell_mode};
