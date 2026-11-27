// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The state of the machine and of the shell, and what can be done to it.
//!
//! Each function asks Tessera through an import of the interface and
//! fails, saying why, unless the manifest lists its permission under
//! `[permissions] system`: `media`, `media-control`, `meters`, `network`,
//! `windows`, `window-control`, `workspaces`, `theme`, `popups` or
//! `notify`, or the action under `[permissions] actions` for [`run`].
//! Those that do something happen once the call into the extension has
//! returned, and fail while [`Extension::answer`](crate::Extension::answer)
//! runs.

use crate::bindings;
use crate::{Media, MediaCommand, Meters, Network, Palette, WindowAction, WindowInfo, Workspaces};

/// What the media player Windows shows controls for plays, if anything.
pub fn media() -> Result<Option<Media>, String> {
    bindings::media_now()
}

/// Asks the media player to play, pause or skip.
pub fn media_control(command: MediaCommand) -> Result<(), String> {
    bindings::media_control(command)
}

/// The processor, memory, battery and network meters, read at most a
/// second ago.
pub fn meters() -> Result<Meters, String> {
    bindings::system_meters()
}

/// The connection Windows prefers, if there is one.
pub fn network() -> Result<Option<Network>, String> {
    bindings::network_state()
}

/// The windows a task switcher shows, the one in front first.
pub fn windows() -> Result<Vec<WindowInfo>, String> {
    bindings::windows()
}

/// Brings the window `id` in front.
pub fn focus(id: u64) -> Result<(), String> {
    bindings::window_command(id, WindowAction::Focus)
}

/// Minimizes the window `id`.
pub fn minimize(id: u64) -> Result<(), String> {
    bindings::window_command(id, WindowAction::Minimize)
}

/// Asks the window `id` to close.
pub fn close(id: u64) -> Result<(), String> {
    bindings::window_command(id, WindowAction::Close)
}

/// Has the window `id` live on the workspace at `index`, counted from 0.
pub fn move_to_workspace(id: u64, index: u32) -> Result<(), String> {
    bindings::window_command(id, WindowAction::MoveToWorkspace(index))
}

/// The workspaces of the display the pointer is on.
pub fn workspaces() -> Result<Workspaces, String> {
    bindings::workspace_state()
}

/// The colors of the active theme.
pub fn palette() -> Result<Palette, String> {
    bindings::theme_colors()
}

/// Opens the popup `name` of the manifest, unless it is open.
pub fn open_popup(name: &str) -> Result<(), String> {
    bindings::open_popup(name)
}

/// Closes the popup `name` of the manifest, if it is open.
pub fn close_popup(name: &str) -> Result<(), String> {
    bindings::close_popup(name)
}

/// Does the action `name` of Tessera with `arg`, as a `[[key]]` does.
pub fn run(name: &str, arg: Option<&str>) -> Result<(), String> {
    bindings::run_action(name, arg)
}

/// Posts a notice with the notifications of Tessera.
pub fn notify(title: &str, body: &str) -> Result<(), String> {
    bindings::notify(title, body)
}

/// The red, green and blue of a color written `#rrggbb`, as the colors of
/// [`palette`] are.
pub fn rgb(color: &str) -> Option<[u8; 3]> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let part = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([part(0)?, part(2)?, part(4)?])
}

/// The share of the memory in use, from 0 to 1.
pub fn memory_share(meters: &Meters) -> f32 {
    if meters.memory_total == 0 {
        return 0.;
    }
    (meters.memory_used as f64 / meters.memory_total as f64).clamp(0., 1.) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_read_from_their_hex_digits() {
        assert_eq!(rgb("#adD28e"), Some([0xad, 0xd2, 0x8e]));
        for wrong in ["add28e", "#add28", "#add28e00", "#zzzzzz", "#ädd28"] {
            assert_eq!(rgb(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn the_share_of_memory_is_kept_between_none_and_all() {
        let meters = |used, total| Meters {
            cpu: None,
            memory_used: used,
            memory_total: total,
            battery: None,
            received: None,
            sent: None,
        };
        assert_eq!(memory_share(&meters(4, 16)), 0.25);
        assert_eq!(memory_share(&meters(4, 0)), 0.);
        assert_eq!(memory_share(&meters(20, 16)), 1.);
    }
}
