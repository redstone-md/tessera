// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What other programs ask of the taskbar, answered on the thread of the
//! interface: a program that has the taskbar hide or stay with
//! `SHAppBarMessage(ABM_SETSTATE)`, and the numbered commands scripts and
//! `Shell.Application` post to it, such as the Run dialog or minimizing
//! every window.
//!
//! The tray thread hears both while Tessera serves the edge bars, and
//! hands them over on channels (platform/win32/appbar.rs and tray/), so it
//! never waits for this thread.
//!
//! A request to hide or to stay is kept here, for the session only: it is
//! applied to the bar the configuration puts first on the primary
//! display, the one programs are told is the taskbar, whenever the shell
//! makes its surfaces from the configuration, and nothing is written to
//! `config.toml`.

use gpui::{App, AsyncApp, Global};

use crate::actions::Actions;
use crate::config::{Hiding, SurfaceConfig};
use crate::modules::Settings;
use crate::platform::{Launch, Tray, TrayCommand};
use crate::shell::Shell;

/// What the dock and its previews read of what an application told the
/// taskbar about a window, with
/// [`TaskbarState::overlay_description`](crate::platform::TaskbarState::overlay_description).
#[allow(unused_imports, reason = "for the dock, which reads it after merge")]
pub use crate::platform::TaskbarState;

/// What programs last asked of the taskbar: to hide (`true`) or to stay
/// (`false`), for the rest of the session; `None` while none asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HideOverride {
    hides: Option<bool>,
}

impl Global for HideOverride {}

impl HideOverride {
    /// Starts listening to what programs ask of the taskbar, and doing it.
    pub fn start(cx: &mut App) {
        let hiding = Tray::hiding_requests();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(hides) = hiding.recv().await {
                if cx.update(|cx| Self::asked(hides, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let commands = Tray::commands();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok(command) = commands.recv().await {
                if cx.update(|cx| run(command, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// A program asked the taskbar to hide, or to stay: the surfaces are
    /// made anew from the configuration in use, which applies it, unless
    /// that was asked last already.
    fn asked(hides: bool, cx: &mut App) {
        let this = cx.default_global::<Self>();
        if this.hides == Some(hides) {
            return;
        }
        this.hides = Some(hides);
        // Arranges the surfaces again with the configuration in use, as
        // it does after new pictures of icons.
        Shell::icons_changed(cx);
    }

    /// Has the bar that is the taskbar among `layouts`, the surfaces of
    /// one display in the order of the configuration, hide or stay as
    /// programs last asked, where the display is the primary one.
    pub fn apply<R>(primary: bool, layouts: &mut [(usize, (SurfaceConfig, R))], cx: &App) {
        let hides = cx.try_global::<Self>().and_then(|this| this.hides);
        let (Some(hides), true) = (hides, primary) else {
            return;
        };
        let configured: Vec<Hiding> = { layouts.iter() }
            .map(|(_, (layout, _))| layout.hide)
            .collect();
        if let Some(index) = taskbar_of(&configured) {
            let layout = &mut layouts[index].1.0;
            layout.hide = overridden(layout.hide, hides);
        }
    }
}

/// Which of the surfaces of the primary display, hiding as `hides` says
/// in the order of the configuration, programs are told is the taskbar:
/// the first that keeps its edge, or else the first.
fn taskbar_of(hides: &[Hiding]) -> Option<usize> {
    let keeps = hides.iter().position(|hiding| !hiding.hides());
    keeps.or_else(|| (!hides.is_empty()).then_some(0))
}

/// How a surface the configuration has hide as `configured` hides once a
/// program asked it to hide, or to stay, as `hides` says: one that keeps
/// its edge hides by itself, one that hides keeps its edge, and one that
/// already does what is asked stays as it is.
fn overridden(configured: Hiding, hides: bool) -> Hiding {
    match (hides, configured.hides()) {
        (true, false) => Hiding::Pointer,
        (false, true) => Hiding::Never,
        _ => configured,
    }
}

/// Does what a numbered command posted to the taskbar asks for, as the
/// taskbar of Windows would.
fn run(command: TrayCommand, cx: &mut App) {
    match command {
        TrayCommand::Run => Actions::run("run", None, cx),
        TrayCommand::ShutDown => Actions::run("power-dialog", None, cx),
        TrayCommand::Cascade | TrayCommand::TileHorizontally | TrayCommand::TileVertically => {
            command.arrange();
        }
        TrayCommand::ShowDesktop => Actions::run("desktop", Some("toggle"), cx),
        TrayCommand::DateAndTime => Actions::run("popup", Some("calendar"), cx),
        TrayCommand::TaskbarProperties => Settings::open_at("bars", None, cx),
        TrayCommand::MinimizeAll => Actions::run("desktop", Some("minimize"), cx),
        TrayCommand::UndoMinimizeAll => Actions::run("desktop", Some("restore"), cx),
        TrayCommand::TaskManager => Launch::new("taskmgr.exe").start(),
        TrayCommand::NotificationIcons => Settings::open_at("tray", None, cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_taskbar_is_the_first_surface_that_keeps_its_edge() {
        use Hiding::*;
        assert_eq!(taskbar_of(&[Pointer, Never, Never]), Some(1));
        assert_eq!(taskbar_of(&[Overlap, Pointer]), Some(0));
        assert_eq!(taskbar_of(&[]), None);
    }

    #[test]
    fn a_request_changes_only_a_surface_that_does_otherwise() {
        assert_eq!(overridden(Hiding::Never, true), Hiding::Pointer);
        assert_eq!(overridden(Hiding::Overlap, true), Hiding::Overlap);
        assert_eq!(overridden(Hiding::Overlap, false), Hiding::Never);
        assert_eq!(overridden(Hiding::Never, false), Hiding::Never);
    }
}
