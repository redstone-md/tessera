// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The keys of a keyboard for the volume, the microphone and the media
//! player, which Windows hands to the shell.
//!
//! A press of such a key becomes a `WM_APPCOMMAND` to the window in front;
//! a window that does not take it passes it on, and in the end Windows
//! tells every window registered with `RegisterShellHookWindow` of it as
//! `HSHELL_APPCOMMAND`. Explorer is such a window and does what the key
//! says. In shell mode there is no Explorer to do it, so Tessera registers
//! the hidden window of the shell thread and answers the keys itself;
//! beside Explorer it must not, or every press would count twice.

use std::cell::RefCell;
use std::sync::OnceLock;

use async_channel::{Receiver, Sender};
use windows::Win32::Foundation::{LPARAM, LRESULT};
use windows::Win32::System::SystemServices::{
    APPCOMMAND_ID, APPCOMMAND_MEDIA_NEXTTRACK, APPCOMMAND_MEDIA_PAUSE, APPCOMMAND_MEDIA_PLAY,
    APPCOMMAND_MEDIA_PLAY_PAUSE, APPCOMMAND_MEDIA_PREVIOUSTRACK, APPCOMMAND_MEDIA_STOP,
    APPCOMMAND_MIC_ON_OFF_TOGGLE, APPCOMMAND_MICROPHONE_VOLUME_MUTE, APPCOMMAND_VOLUME_DOWN,
    APPCOMMAND_VOLUME_MUTE, APPCOMMAND_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FAPPCOMMAND_MASK, HSHELL_APPCOMMAND, RegisterShellHookWindow, RegisterWindowMessageW,
};
use windows::core::w;

use super::shell_thread::{Interception, ShellThread};

/// A key of the keyboard for sound or media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    /// Mutes the microphone, or lifts its mute.
    MicrophoneMute,
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    Previous,
}

impl AppCommand {
    /// The key the `lParam` of `HSHELL_APPCOMMAND` names: its high word
    /// holds the command, under the bits that tell which device sent it.
    fn of(l: LPARAM) -> Option<Self> {
        let high = ((l.0 as usize) >> 16) as u32 & 0xffff;
        let command = APPCOMMAND_ID(high & !FAPPCOMMAND_MASK);
        Some(match command {
            APPCOMMAND_VOLUME_UP => Self::VolumeUp,
            APPCOMMAND_VOLUME_DOWN => Self::VolumeDown,
            APPCOMMAND_VOLUME_MUTE => Self::VolumeMute,
            APPCOMMAND_MICROPHONE_VOLUME_MUTE | APPCOMMAND_MIC_ON_OFF_TOGGLE => {
                Self::MicrophoneMute
            }
            APPCOMMAND_MEDIA_PLAY_PAUSE => Self::PlayPause,
            APPCOMMAND_MEDIA_PLAY => Self::Play,
            APPCOMMAND_MEDIA_PAUSE => Self::Pause,
            APPCOMMAND_MEDIA_STOP => Self::Stop,
            APPCOMMAND_MEDIA_NEXTTRACK => Self::Next,
            APPCOMMAND_MEDIA_PREVIOUSTRACK => Self::Previous,
            _ => return None,
        })
    }
}

thread_local! {
    /// What keeps the keys coming while they are answered, on the shell
    /// thread only.
    static ANSWERING: RefCell<Option<Interception>> = const { RefCell::new(None) };
}

/// The keys of the keyboard for sound and media.
pub struct AppCommands;

impl AppCommands {
    /// Has the keys for sound and media come to Tessera from now on, for
    /// as long as it runs: the receiver yields each key pressed. Asked
    /// again, it gives the same keys, which go to one receiver or the
    /// other. Meant for shell mode only (see the module documentation).
    /// The keys a window in front takes for itself, as a media player may,
    /// do not come.
    pub fn answer() -> Receiver<AppCommand> {
        static PRESSED: OnceLock<Receiver<AppCommand>> = OnceLock::new();
        let pressed = PRESSED.get_or_init(|| {
            let (keys, pressed) = async_channel::unbounded();
            ShellThread::run(move || {
                if let Err(error) = Self::register(keys) {
                    report!("the keys for sound and media are not answered: {error}");
                }
            });
            pressed
        });
        pressed.clone()
    }

    /// Registers the hidden window of the shell thread for the messages
    /// of the shell, on that thread.
    fn register(keys: Sender<AppCommand>) -> Result<(), String> {
        let window = ShellThread::owner().ok_or("the shell thread has no window")?;
        // SAFETY: the name is a constant string.
        let shell_hook = unsafe { RegisterWindowMessageW(w!("SHELLHOOK")) };
        if shell_hook == 0 {
            return Err("the messages of the shell have no number".to_owned());
        }
        let interception = ShellThread::intercept(move |message, w, l| {
            if message != shell_hook || w.0 as u32 != HSHELL_APPCOMMAND {
                return None;
            }
            let command = AppCommand::of(l)?;
            keys.try_send(command).ok();
            // Taken: no other window is to do it as well.
            Some(LRESULT(1))
        });
        // SAFETY: the window is the live hidden window of this thread.
        if !unsafe { RegisterShellHookWindow(window) }.as_bool() {
            return Err("Windows does not tell the window of the shell's messages".to_owned());
        }
        ANSWERING.with(|answering| answering.replace(Some(interception)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `lParam` of a key from the keyboard: the command in the high
    /// word, whose device bits are 0 for the keyboard.
    fn from_keyboard(command: APPCOMMAND_ID) -> LPARAM {
        LPARAM((command.0 as isize) << 16)
    }

    #[test]
    fn a_key_is_known_by_its_command_whichever_device_sent_it() {
        assert_eq!(
            AppCommand::of(from_keyboard(APPCOMMAND_VOLUME_UP)),
            Some(AppCommand::VolumeUp)
        );
        // From the mouse (0x8000) and from another device (0x1000).
        let mouse = LPARAM(((APPCOMMAND_MEDIA_NEXTTRACK.0 as isize) | 0x8000) << 16);
        assert_eq!(AppCommand::of(mouse), Some(AppCommand::Next));
        let other = LPARAM(((APPCOMMAND_MIC_ON_OFF_TOGGLE.0 as isize) | 0x1000) << 16 | 0x5);
        assert_eq!(AppCommand::of(other), Some(AppCommand::MicrophoneMute));
        // The browser keys are not for the shell.
        assert_eq!(AppCommand::of(from_keyboard(APPCOMMAND_ID(1))), None);
    }
}
