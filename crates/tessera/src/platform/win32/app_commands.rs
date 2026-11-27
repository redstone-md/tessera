// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The keys of a keyboard for the volume, the microphone and the media
//! player, which Windows hands to the shell.
//!
//! A press of such a key becomes a `WM_APPCOMMAND` to the window in front;
//! a window that does not take it passes it on, and in the end Windows
//! tells every window registered with `RegisterShellHookWindow` of it as
//! `HSHELL_APPCOMMAND`. Explorer is such a window and does what the key
//! says. In shell mode there is no Explorer to do it, and Tessera is to.
//!
//! The window registered for the messages of the shell is the one of the
//! shell host, which owns them; a second window registered here would have
//! every press counted twice. This only tells which key a command of
//! `WM_APPCOMMAND` is, for whatever answers it.

use windows::Win32::System::SystemServices::{
    APPCOMMAND_ID, APPCOMMAND_MEDIA_NEXTTRACK, APPCOMMAND_MEDIA_PAUSE, APPCOMMAND_MEDIA_PLAY,
    APPCOMMAND_MEDIA_PLAY_PAUSE, APPCOMMAND_MEDIA_PREVIOUSTRACK, APPCOMMAND_MEDIA_STOP,
    APPCOMMAND_MIC_ON_OFF_TOGGLE, APPCOMMAND_MICROPHONE_VOLUME_MUTE, APPCOMMAND_VOLUME_DOWN,
    APPCOMMAND_VOLUME_MUTE, APPCOMMAND_VOLUME_UP,
};
use windows::Win32::UI::WindowsAndMessaging::FAPPCOMMAND_MASK;

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
    /// The key a command of `WM_APPCOMMAND` names, with or without the
    /// bits that tell which device sent it; `None` for one that is not
    /// for sound or media, as the keys of a browser are.
    #[allow(dead_code, reason = "the shell host hands its keys to it")]
    pub fn of(command: u16) -> Option<Self> {
        let command = APPCOMMAND_ID(u32::from(command) & !FAPPCOMMAND_MASK);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The command as the shell tells it, without the device bits.
    fn command(id: APPCOMMAND_ID) -> u16 {
        id.0 as u16
    }

    #[test]
    fn a_key_is_known_by_its_command_whichever_device_sent_it() {
        assert_eq!(
            AppCommand::of(command(APPCOMMAND_VOLUME_UP)),
            Some(AppCommand::VolumeUp)
        );
        // From the mouse (0x8000) and from another device (0x1000).
        let mouse = command(APPCOMMAND_MEDIA_NEXTTRACK) | 0x8000;
        assert_eq!(AppCommand::of(mouse), Some(AppCommand::Next));
        let other = command(APPCOMMAND_MIC_ON_OFF_TOGGLE) | 0x1000;
        assert_eq!(AppCommand::of(other), Some(AppCommand::MicrophoneMute));
        // The browser keys are not for the shell.
        assert_eq!(AppCommand::of(1), None);
    }
}
