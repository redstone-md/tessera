// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Sharing files through the share window of Windows, as "Share" in
//! Explorer opens it: nearby sharing, mail, the applications that take
//! files.
//!
//! The files are first made `StorageFile` and `StorageFolder` items of
//! Windows ([`SharePackage::prepare`]), which reads the disk and is done
//! away from the user interface. The share window is then opened for a
//! window of Tessera ([`SharePackage::show`], on the thread of that
//! window): Windows asks the `DataTransferManager` of the window for what
//! is shared, and gets the items.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};
use gpui::Window;
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile, StorageFolder};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{AgileReference, HSTRING, Interface, Ref, factory};
use windows_collections::IIterable;

use super::NativeSurface;

thread_local! {
    /// The handler of each window that shares, by the window, so that the
    /// one before is removed when the window shares again; on the thread
    /// of the windows.
    static HANDLERS: RefCell<Vec<(isize, DataTransferManager, i64)>> =
        const { RefCell::new(Vec::new()) };
}

/// Files ready to be shared, which can be handed to the thread of the
/// window that shares them.
#[derive(Clone)]
pub struct SharePackage {
    items: Vec<AgileReference<IStorageItem>>,
    /// What the share window calls them: the name of the one file, or how
    /// many there are.
    title: HSTRING,
}

impl SharePackage {
    /// The items of Windows for `paths`, files and folders. It reads the
    /// disk and waits for Windows, so it is not done on the thread of the
    /// user interface. A path Windows cannot make an item of fails the
    /// whole package, naming it.
    pub fn prepare(paths: &[PathBuf], many: &str) -> Result<Self> {
        if paths.is_empty() {
            bail!("nothing to share");
        }
        // SAFETY: the thread joins the multithreaded apartment for the
        // calls of Windows below, or is in one already.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let mut items = Vec::with_capacity(paths.len());
        for path in paths {
            let name = HSTRING::from(path.as_os_str());
            let item: AgileReference<IStorageItem> = if path.is_dir() {
                StorageFolder::GetFolderFromPathAsync(&name)
                    .and_then(|asked| asked.get())
                    .and_then(|folder| folder.cast::<IStorageItem>())
            } else {
                StorageFile::GetFileFromPathAsync(&name)
                    .and_then(|asked| asked.get())
                    .and_then(|file| file.cast::<IStorageItem>())
            }
            .and_then(|item| AgileReference::new(&item))
            .with_context(|| format!("{} cannot be shared", path.display()))?;
            items.push(item);
        }
        let title = match paths {
            [one] => one.file_name().map_or_else(
                || one.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            _ => format!("{} {many}", paths.len()),
        };
        Ok(Self {
            items,
            title: HSTRING::from(title),
        })
    }

    /// Opens the share window of Windows for `window`, with these items.
    /// To be called on the thread of the window; it returns at once.
    pub fn show(&self, window: &Window) -> Result<()> {
        let hwnd = NativeSurface::of(window)?.hwnd;
        self.show_for(hwnd)
    }

    fn show_for(&self, hwnd: HWND) -> Result<()> {
        let interop = factory::<DataTransferManager, IDataTransferManagerInterop>()
            .context("the share window of Windows is not there")?;
        // SAFETY: the window belongs to this process and is alive while it
        // is asked for.
        let manager: DataTransferManager =
            unsafe { interop.GetForWindow(hwnd) }.context("the window cannot share")?;
        let (items, title) = (self.items.clone(), self.title.clone());
        let handler = TypedEventHandler::new(
            move |_: Ref<'_, DataTransferManager>, asked: Ref<'_, DataRequestedEventArgs>| {
                let data = asked.ok()?.Request()?.Data()?;
                data.Properties()?.SetTitle(&title)?;
                let items = { items.iter() }
                    .map(|item| item.resolve().map(Some))
                    .collect::<windows::core::Result<Vec<_>>>()?;
                data.SetStorageItemsReadOnly(&IIterable::<IStorageItem>::from(items))
            },
        );
        let token = manager.DataRequested(&handler)?;
        let number = hwnd.0 as isize;
        HANDLERS.with(|handlers| {
            let mut handlers = handlers.borrow_mut();
            if let Some(at) = handlers.iter().position(|(window, ..)| *window == number) {
                let (_, before, token) = handlers.remove(at);
                let _ = before.RemoveDataRequested(token);
            }
            handlers.push((number, manager, token));
        });
        // SAFETY: as above.
        unsafe { interop.ShowShareUIForWindow(hwnd) }
            .context("the share window of Windows did not open")
    }
}
