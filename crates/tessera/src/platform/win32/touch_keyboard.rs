// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The touch keyboard of Windows, shown and hidden as the button of the
//! taskbar of Explorer does: through the interface `ITipInvocation` of the
//! touch keyboard and handwriting panel (TabTip.exe), which Windows does
//! not document; `Toggle` with the desktop window shows the keyboard where
//! it is hidden and hides it where it is shown. Where TabTip does not run
//! yet, it is started first.

#![allow(unsafe_code)]

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, interface};

use super::execute::Launch;
use super::program_path::expand_variables;
use super::shell_thread::ShellThread;

/// The class of TabTip that answers without starting it.
const UI_HOST_NO_LAUNCH: GUID = GUID::from_u128(0x4ce576fa_83dc_4f88_951c_9d0782b4e376);

/// Where TabTip is.
const TAB_TIP: &str = r"%CommonProgramFiles%\microsoft shared\ink\TabTip.exe";

/// How long TabTip may take to answer once it was started, and how often
/// it is asked meanwhile.
const START_TIME: Duration = Duration::from_secs(3);
const ASK_EVERY: Duration = Duration::from_millis(100);

#[interface("37c994e7-432b-4834-a2f7-dce1f13b834b")]
unsafe trait ITipInvocation: IUnknown {
    unsafe fn toggle(&self, window: HWND) -> HRESULT;
}

/// The touch keyboard of Windows.
pub struct TouchKeyboard;

impl TouchKeyboard {
    /// Shows the touch keyboard, or hides it where it is shown, on a thread
    /// of its own: starting TabTip takes a moment. A failure is reported.
    pub fn toggle() {
        ShellThread::run_alone("touch keyboard", || {
            if let Err(error) = toggle_here() {
                report!("the touch keyboard was not shown: {error:#}");
            }
        });
    }
}

/// Toggles the keyboard on this thread, which uses COM, starting TabTip
/// where it does not answer.
fn toggle_here() -> Result<()> {
    let invocation = match invocation() {
        Ok(invocation) => invocation,
        Err(_) => {
            let started = Launch::new(expand_variables(TAB_TIP))
                .quiet()
                .named("the touch keyboard")
                .started()
                .recv_blocking();
            match started {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => anyhow::bail!("TabTip did not start: {error}"),
                Err(_) => anyhow::bail!("TabTip did not start"),
            }
            wait_for_invocation()?
        }
    };
    // SAFETY: a COM call with the desktop window, which always exists.
    unsafe { invocation.toggle(GetDesktopWindow()) }
        .ok()
        .context("TabTip did not toggle the keyboard")
}

/// The interface of a TabTip that runs.
fn invocation() -> windows::core::Result<ITipInvocation> {
    // SAFETY: a COM call that creates an object of a registered class.
    unsafe {
        CoCreateInstance(
            &UI_HOST_NO_LAUNCH,
            None,
            CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER,
        )
    }
}

/// The interface of TabTip once it was started, asked for until it
/// answers or [`START_TIME`] passed.
fn wait_for_invocation() -> Result<ITipInvocation> {
    let started = Instant::now();
    loop {
        match invocation() {
            Ok(invocation) => return Ok(invocation),
            Err(error) if started.elapsed() >= START_TIME => {
                return Err(error).context("TabTip started but does not answer");
            }
            Err(_) => thread::sleep(ASK_EVERY),
        }
    }
}
