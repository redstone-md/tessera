// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Operating system integration behind safe types.

mod win32;

pub use win32::AppCommand;
pub use win32::BalloonKind;
pub use win32::BinUsage;
pub use win32::BrightnessLevel;
pub use win32::DesktopClose;
pub use win32::DisplayMode;
pub use win32::FileSearch;
pub use win32::InputName;
pub use win32::Occupation;
pub use win32::OwnInput;
pub use win32::PowerChoices;
#[allow(unused_imports, reason = "for the starts that wait for a program")]
pub use win32::Process;
pub use win32::QuickFolder;
#[allow(unused_imports, reason = "for the view of the bin in the file manager")]
pub use win32::RecycledItem;
pub use win32::SessionKey;
#[allow(
    unused_imports,
    reason = "for the packages that hand work to the Shell"
)]
pub use win32::ShellThread;
pub use win32::ShowDesktop;
pub use win32::SystemZone;
pub use win32::TileBorder;
pub use win32::TrayPromotion;
pub use win32::UriScheme;
pub use win32::SpellCheck;
pub use win32::decode_with_wic;
#[allow(
    unused_imports,
    reason = "for the dock and the rules, which ask of one window"
)]
pub use win32::desktop_of;
pub use win32::display_at;
pub use win32::font_families;
pub use win32::on_resizing_border;
pub use win32::paint_desktop;
pub use win32::rename_new;
pub use win32::save_screenshot;
pub use win32::spatial_sound;
pub use win32::wheel_lines;
pub use win32::{
    Account, AppSound, AppWindow, AppWindowId, AudioDevice, AudioOutput, BackdropPicture, Battery,
    Bluetooth, Carrier, ChoiceKey, Commands, Connection, DesktopBackdrop, DesktopVideo, DisplayKey,
    Drive, DriveKind, EdgeReservation, FileClipboard, FileWork, Hdr, Hotkey, Hotkeys, IconPixels,
    InputLanguage, Link, Media, MediaCommand, MemoryLoad, Modifiers, NativeSurface, Network,
    NetworkTraffic, Notification, Notifications, NowPlaying, Pace, PairedDevice, Passage, Place,
    Position, PowerAction, PowerPlan, ProcessorLoad, RecycleBin, ScreenBrightness, ScreenEdge,
    ScreenRect, SearchIndex, Shortcut, SmallBirth, TakenKeys, Taskbar, Thumbnail, Tray, TrayButton,
    TrayIcon, Typing, Veil, VideoState, VirtualDesktops, Vpn, VpnState, Wifi, WifiNetwork,
    WifiReach, WifiSecurity, WindowEvents, WindowsKey, Wireless, accent_color, app_id_in,
    app_id_of_program, app_path, desktop_look, desktop_pixels, disk_space, display_frames,
    is_hidden, light_mode, memory_held, password_fits, pointer_at, pointer_held,
    pointer_on_own_window, read_desktop_if_requested, reveal_in_explorer, set_light_mode,
    show_properties,
};
pub use win32::{ActiveTracking, GrabKey, MouseHook, PointerButton, PointerEvent};
pub use win32::{Attributes, ExplorerSettings, NameRules, localized_candidates, shell_names};
pub use win32::{Balloon, BalloonEvent};
pub use win32::{BatteryChanges, BatteryLevels, CriticalAction};
pub use win32::{BluetoothScan, FoundDevice, Pairing, PairingFailure, PairingPrompt};
pub use win32::{COLDEST, GammaNight, WARMEST, WindowsNight, WindowsNightLight};
pub use win32::{Capability, CapabilityUse, PrivacyWatch};
pub use win32::{Clip, ClipboardWatch, Offer, paste_into};
pub use win32::{DesktopSurvey, WindowMark};
#[allow(
    unused_imports,
    reason = "named by the views that keep the windows of a survey or a still picture"
)]
pub use win32::{DesktopWindow, Still};
pub use win32::{
    DeviceChanges, EjectRefusal, Ejection, RemovableDevice, RemovableDrive, Removables,
};
pub use win32::{
    Display, MaximizeHover, MoveEvent, MoveWatch, WindowArranging, front_fills, shift_held,
};
pub use win32::{DisplayName, display_device, display_names};
pub use win32::{DragOut, DropEvent, DropTarget, DropZone, DropZones};
pub use win32::{EdgeBand, EdgeSpan, HidingClaim};
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
#[allow(unused_imports, reason = "the parts the dock and its previews read")]
pub use win32::{GivenIcon, Progress, ProgressState, TaskbarList, TaskbarState, ThumbButton};
pub use win32::{HostOptions, ShellEvent, ShellHost};
pub use win32::{Hotspot, HotspotBand, HotspotInfo, HotspotState};
pub use win32::{ImeMode, ImeState, TouchKeyboard};
pub use win32::{Intercept, MenuOutcome, ShellMenu, ShellVerb};
pub use win32::{JumpItem, JumpList, JumpTarget, KnownCategory};
pub use win32::{KeyLocks, window_in_front};
#[allow(
    unused_imports,
    reason = "Show is for the launchers that ask for a window state"
)]
pub use win32::{Launch, Show};
pub use win32::{NewTemplate, OpenWithHandler, SendToTarget, choose_app};
pub use win32::{PowerMode, PowerModeState};
pub use win32::{QuickAccess, ShellPlace, onedrive, recent_files, wsl_distributions};
pub use win32::{RadioId, RadioInfo, RadioType, Radios, airplane_mode};
pub use win32::{Removal, app_settings, put_on_desktop};
pub use win32::{Repeat, SessionInfo};
pub use win32::{RunHistory, RunningProcess};
pub use win32::{SessionStartup, StartupEntry, StartupSource, SwitchRefusal};
pub use win32::{SystemWallpaper, WallpaperPosition, user_locale};
pub use win32::{expand_variables, find_program, recent_folder};
#[allow(
    unused_imports,
    reason = "for the features that ask Explorer while it runs"
)]
pub use win32::{explorer_running, shell_mode};
pub use win32::{terminal, windows_terminal};
