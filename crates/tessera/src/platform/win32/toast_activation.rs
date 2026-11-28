// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Doing what a click on a notification, or on one of its buttons, asks
//! for, as Windows does when its own notification center is clicked.
//!
//! - An address (`activationType="protocol"`) is opened as any link.
//! - A desktop application that registered a COM activator for its
//!   notifications names it on its Start menu item, in the property
//!   `System.AppUserModel.ToastActivatorCLSID`. It is created out of
//!   process (`CLSCTX_LOCAL_SERVER`) as an `INotificationActivationCallback`
//!   and given the arguments and the values of the input fields, which is
//!   what Windows gives it, in the foreground and in the background alike.
//! - A packaged application is started with `IApplicationActivationManager`
//!   and the arguments, as an ordinary start: it sees no
//!   `ToastNotificationActivatedEventArgs`, so an application that reads
//!   its buttons only from those does nothing with them, and its
//!   background tasks are not started.
//! - Anything else is started from the Start menu, without arguments.
//!
//! Each of these may wait for the application, so the work runs on a
//! thread of its own with COM, never on the thread that draws.

#![allow(unsafe_code)]

use async_channel::Receiver;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ToastActivatorCLSID;
use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, NOTIFICATION_USER_INPUT_DATA,
};
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, IApplicationActivationManager, IShellItem2,
    SHCreateItemFromParsingName,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows::core::{GUID, HSTRING, PCWSTR};

use super::ShellThread;
use super::execute::Launch;
use super::packaged::app_path;

/// How a click reaches the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastRoute {
    /// The application comes to the front with the arguments.
    Foreground,
    /// The application gets the arguments without coming to the front.
    Background,
    /// The arguments are an address to open.
    Protocol,
}

/// What a click on a notification asks of its application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToastActivation {
    /// The identifier the application posts under.
    pub app_id: String,
    /// The `launch` of the toast, or the arguments of the button.
    pub arguments: String,
    pub route: ToastRoute,
    /// The values of the input fields, by their identifiers.
    pub inputs: Vec<(String, String)>,
}

/// How an activation reached the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activated {
    /// Through its COM activator, as Windows does it.
    AsToast,
    /// As a start of the packaged application with the arguments.
    AsLaunch,
    /// The application was only started, without the arguments.
    Opened,
    /// The address was opened.
    Address,
}

impl ToastActivation {
    /// Does it away from the caller's thread; the receiver yields how it
    /// went once. Await it; never block the thread of GPUI on it.
    pub fn start(self) -> Receiver<Result<Activated, String>> {
        if self.route == ToastRoute::Protocol {
            let (answer, answered) = async_channel::bounded(1);
            let result = match self.arguments.trim() {
                "" => Err("the notification names no address to open".to_owned()),
                address => {
                    Launch::new(address).quiet().start();
                    Ok(Activated::Address)
                }
            };
            answer.try_send(result).ok();
            return answered;
        }
        ShellThread::call_alone("notification activation", move || self.activate_here())
    }

    /// Does it on this thread, which has COM.
    fn activate_here(&self) -> Result<Activated, String> {
        if self.app_id.is_empty() {
            return Err("the notification names no application".to_owned());
        }
        // The application started or called may bring its window to the
        // front, as the click of the user asked.
        // SAFETY: the call only grants a permission; failing is harmless.
        unsafe { AllowSetForegroundWindow(ASFW_ANY) }.ok();
        if let Some(activator) = activator_of(&self.app_id) {
            return self.call_activator(activator).map(|()| Activated::AsToast);
        }
        if is_packaged(&self.app_id) {
            return self.start_packaged().map(|()| Activated::AsLaunch);
        }
        Launch::app(&self.app_id).quiet().start();
        Ok(Activated::Opened)
    }

    /// Hands the arguments and the inputs to the COM activator `clsid`.
    fn call_activator(&self, clsid: GUID) -> Result<(), String> {
        let app_id = HSTRING::from(&self.app_id);
        let arguments = HSTRING::from(&self.arguments);
        let texts: Vec<(HSTRING, HSTRING)> = { self.inputs.iter() }
            .map(|(key, value)| (HSTRING::from(key), HSTRING::from(value)))
            .collect();
        let data: Vec<NOTIFICATION_USER_INPUT_DATA> = { texts.iter() }
            .map(|(key, value)| NOTIFICATION_USER_INPUT_DATA {
                Key: PCWSTR(key.as_ptr()),
                Value: PCWSTR(value.as_ptr()),
            })
            .collect();
        // SAFETY: COM is initialized on this thread; every string the data
        // points to is held by `texts` until after the call.
        unsafe {
            let callback: INotificationActivationCallback =
                CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)
                    .map_err(|error| format!("the application did not answer: {error}"))?;
            callback
                .Activate(&app_id, &arguments, &data)
                .map_err(|error| format!("the application refused the notification: {error}"))
        }
    }

    /// Starts the packaged application with the arguments.
    fn start_packaged(&self) -> Result<(), String> {
        // SAFETY: COM is initialized on this thread, and the manager is
        // released when it goes out of scope.
        unsafe {
            let manager: IApplicationActivationManager =
                CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                    .map_err(|error| format!("applications cannot be started: {error}"))?;
            manager
                .ActivateApplication(
                    &HSTRING::from(&self.app_id),
                    &HSTRING::from(&self.arguments),
                    AO_NONE,
                )
                .map(drop)
                .map_err(|error| format!("the application did not start: {error}"))
        }
    }
}

/// The COM activator the application `app_id` names on its Start menu
/// item, if it has one.
fn activator_of(app_id: &str) -> Option<GUID> {
    let path = HSTRING::from(app_path(app_id).as_os_str());
    // SAFETY: COM is initialized on this thread; the item is released when
    // it goes out of scope.
    let clsid = unsafe {
        let item: IShellItem2 = SHCreateItemFromParsingName(&path, None).ok()?;
        item.GetCLSID(&PKEY_AppUserModel_ToastActivatorCLSID).ok()?
    };
    (clsid != GUID::zeroed()).then_some(clsid)
}

/// Whether `app_id` is that of a packaged application: a package family
/// and an application in it.
fn is_packaged(app_id: &str) -> bool {
    app_id
        .split_once('!')
        .is_some_and(|(family, app)| family.contains('_') && !app.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_applications_are_told_from_programs() {
        assert!(is_packaged("Microsoft.WindowsAlarms_8wekyb3d8bbwe!App"));
        assert!(!is_packaged("Microsoft.Office.OUTLOOK.EXE.15"));
        assert!(!is_packaged("Chrome!"));
    }
}
