// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One native global-position observation, independent of images and monitors.

use tessera_system::wallpaper::WallpaperError;
use tessera_system::wallpaper::position::{
    Observation, Position, Target, WriteDisposition, WriteOutcome,
};
use windows::Win32::UI::Shell::{
    DESKTOP_WALLPAPER_POSITION, DWPOS_CENTER, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN, DWPOS_STRETCH,
    DWPOS_TILE, IDesktopWallpaper,
};

use super::displays;

/// Only the common wallpaper STA owns this bounded ledger. Its public ticket
/// contains identity only; retaining it cannot retain a COM object or image.
#[derive(Default)]
pub(super) struct Ledger {
    current: Option<Observation>,
}

impl Ledger {
    pub(super) fn clear(&mut self) {
        self.current = None;
    }

    pub(super) fn issuer(&self) -> Option<Target> {
        self.current
            .as_ref()
            .map(|observation| observation.target.clone())
    }

    pub(super) fn read(&mut self) -> Result<Observation, WallpaperError> {
        // An accepted explicit read replaces only the previous position ticket,
        // including when the fresh SDK observation is unavailable.
        self.clear();
        let desktop = displays::desktop()?;
        let observation = observe(&desktop)?;
        self.current = Some(observation.clone());
        Ok(observation)
    }

    pub(super) fn set(
        &mut self,
        target: Target,
        desired: Position,
    ) -> Result<WriteOutcome, WallpaperError> {
        // Admission already consumed the inbox ticket. Recheck the exact native
        // observation before COM acquisition, and consume it even on rejection.
        let original = self
            .current
            .take()
            .filter(|observation| observation.target == target)
            .ok_or(WallpaperError::InvalidTarget)?;
        let desktop = displays::desktop()?;
        let actual = read(&desktop)?;
        if actual != original.position {
            // External change is not permission to retry, infer a replacement
            // target, or write against the newly discovered state.
            return Err(WallpaperError::InvalidTarget);
        }
        let outcome = if actual == desired {
            WriteOutcome {
                disposition: WriteDisposition::AlreadyCurrent,
                observation: Some(Observation {
                    target: Target::new(),
                    position: actual,
                }),
            }
        } else {
            // SAFETY: this is the existing owner STA's COM interface and one
            // closed SDK position value. SetPosition applies to the system's
            // monitors, including future ones; it has no monitor selector.
            // GetPosition/SetPosition are non-atomic: an external change can
            // still race this checked read and the setter. Never retry/rollback.
            let receipt = unsafe { desktop.SetPosition(to_native(desired)) };
            let disposition = if receipt.is_ok() {
                WriteDisposition::Accepted
            } else {
                WriteDisposition::Rejected
            };
            // Read independently even after a rejected setter: its receipt is
            // not current-state proof, much less proof of rendered pixels.
            WriteOutcome {
                disposition,
                observation: observe(&desktop).ok(),
            }
        };
        self.current = outcome.observation.clone();
        Ok(outcome)
    }
}

fn observe(desktop: &IDesktopWallpaper) -> Result<Observation, WallpaperError> {
    Ok(Observation {
        target: Target::new(),
        position: read(desktop)?,
    })
}

fn read(desktop: &IDesktopWallpaper) -> Result<Position, WallpaperError> {
    // SAFETY: the interface is acquired, used and dropped on the existing STA.
    let value = unsafe { desktop.GetPosition() }.map_err(|_| WallpaperError::Unavailable)?;
    match value {
        DWPOS_CENTER => Ok(Position::Center),
        DWPOS_TILE => Ok(Position::Tile),
        DWPOS_STRETCH => Ok(Position::Stretch),
        DWPOS_FIT => Ok(Position::Fit),
        DWPOS_FILL => Ok(Position::Fill),
        DWPOS_SPAN => Ok(Position::Span),
        _ => Err(WallpaperError::Unavailable),
    }
}

fn to_native(position: Position) -> DESKTOP_WALLPAPER_POSITION {
    match position {
        Position::Center => DWPOS_CENTER,
        Position::Tile => DWPOS_TILE,
        Position::Stretch => DWPOS_STRETCH,
        Position::Fit => DWPOS_FIT,
        Position::Fill => DWPOS_FILL,
        Position::Span => DWPOS_SPAN,
    }
}
