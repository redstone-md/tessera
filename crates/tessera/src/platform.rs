// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::BinUsage;
#[allow(unused_imports, reason = "for the starts that wait for a program")]
pub use win32::Process;
pub use win32::QuickFolder;
#[allow(unused_imports, reason = "for the view of the bin in the file manager")]
pub use win32::RecycledItem;
#[allow(
    unused_imports,
    reason = "for the packages that hand work to the Shell"
)]
pub use win32::ShellThread;
pub use win32::ShowDesktop;
pub use win32::rename_new;
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
pub use win32::{Attributes, ExplorerSettings, NameRules, localized_candidates, shell_names};
#[allow(
    unused_imports,
    reason = "for the notifications that show the balloons of the tray"
)]
pub use win32::{Balloon, BalloonEvent, BalloonKind};
#[allow(
    unused_imports,
    reason = "for the file manager and the desktop, which drag files out and take drops"
)]
pub use win32::{
    DragButton, DragOut, DropEffect, DropEffects, DropEvent, DropKeys, DropTarget, DropZone,
    DropZones, drop_effect, drop_into,
};
pub use win32::{
    ExplorerShell, NextShell, SHELL_FLAG, ShellRegistration, attach_console, is_safe_mode,
    own_program_name, recovery_keys_held, started_as_shell,
};
#[allow(
    unused_imports,
    reason = "for the file manager and the desktop, which take work on files back"
)]
pub use win32::{
    FileDeed, FileOperation, LinkTarget, Made, Outcome, ShellId, UndoJournal, unique_name,
};
pub use win32::{HostOptions, ShellEvent, ShellHost};
#[allow(
    unused_imports,
    reason = "for the menus New, Open with and Send to of the file manager and the desktop"
)]
pub use win32::{IconPlace, NewTemplate, OpenWithHandler, SendToKind, SendToTarget, choose_app};
#[allow(
    unused_imports,
    reason = "for the file manager and the desktop, which show the menus of the Shell"
)]
pub use win32::{Intercept, MenuOutcome, ShellMenu, ShellVerb};
#[allow(
    unused_imports,
    reason = "Show is for the launchers that ask for a window state"
)]
pub use win32::{Launch, Show};
pub use win32::{QuickAccess, ShellPlace, onedrive, recent_files, wsl_distributions};
pub use win32::{RunHistory, RunningProcess};
pub use win32::{SessionStartup, StartupEntry, StartupSource, SwitchRefusal};
pub use win32::{expand_variables, find_program, recent_folder};
#[allow(
    unused_imports,
    reason = "for the features that ask Explorer while it runs"
)]
pub use win32::{explorer_running, shell_mode};
