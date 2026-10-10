// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Exact native dialog array: protected files or one transiently checked folder.

use std::sync::Arc;

use tessera_system::wallpaper::{WallpaperError, collection as contract};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::{
    DESKTOP_SLIDESHOW_OPTIONS, DSO_SHUFFLEIMAGES, DSS_DISABLED_BY_REMOTE_SESSION, DSS_ENABLED,
    DSS_SLIDESHOW, IDesktopWallpaper, IShellItem, IShellItemArray,
};

use super::{Mailbox, displays, retirement_collection_target, source};

const MIN_ITEMS: u32 = 2;
const MAX_ITEMS: u32 = 32;

pub(super) struct Collection {
    pub(super) target: contract::TargetWeak,
    array: IShellItemArray,
    source: Source,
}

enum Source {
    Images {
        images: Vec<source::Image>,
        parent: IShellItem,
    },
    Folder(source::PolicyEntry),
}

impl Collection {
    pub(super) fn choose(
        owner: HWND,
        pick: source::Pick,
        mailbox: &Arc<Mailbox>,
    ) -> Result<Option<(Self, contract::Selection)>, WallpaperError> {
        let Some(dialog) = source::choose_dialog(owner, pick)? else {
            return Ok(None);
        };
        // Retain the exact SDK result, never an independently synthesized array.
        let array = unsafe { dialog.GetResults() }.map_err(|_| WallpaperError::Unavailable)?;
        let count = unsafe { array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?;
        let (source, presentation) = match pick {
            source::Pick::Images => {
                if !(MIN_ITEMS..=MAX_ITEMS).contains(&count) {
                    return Err(WallpaperError::Unavailable);
                }
                let mut images: Vec<source::Image> = Vec::with_capacity(count as usize);
                let mut parent = None;
                for index in 0..count {
                    let item = unsafe { array.GetItemAt(index) }
                        .map_err(|_| WallpaperError::Unavailable)?;
                    let image = source::Image::from_item(item)?;
                    if images.iter().any(|original| original.same_file(&image)) {
                        return Err(WallpaperError::Unavailable);
                    }
                    if let Some(parent) = &parent {
                        if !image.is_in_container(parent)? {
                            return Err(WallpaperError::Unavailable);
                        }
                    } else {
                        parent = Some(image.parent()?);
                    }
                    images.push(image);
                }
                let items = images
                    .iter()
                    .map(|image| {
                        Ok(contract::Item {
                            caption: image.caption.clone(),
                            preview: image.preview()?,
                        })
                    })
                    .collect::<Result<Vec<_>, WallpaperError>>()?;
                (
                    Source::Images {
                        images,
                        parent: parent.ok_or(WallpaperError::Unavailable)?,
                    },
                    contract::Source::Images(items),
                )
            }
            source::Pick::Folder => {
                if count != 1 {
                    return Err(WallpaperError::Unavailable);
                }
                let item =
                    unsafe { array.GetItemAt(0) }.map_err(|_| WallpaperError::Unavailable)?;
                let folder = source::PolicyEntry::capture(item)?;
                if !folder.is_directory() {
                    return Err(WallpaperError::Unavailable);
                }
                let caption = folder.caption()?;
                (Source::Folder(folder), contract::Source::Folder { caption })
            }
            source::Pick::Image => return Err(WallpaperError::Unavailable),
        };
        let target = retirement_collection_target(mailbox);
        let collection = Self {
            target: target.downgrade(),
            array,
            source,
        };
        // Native handlers may reenter: validate the original files or folder and
        // exact original SDK array again immediately before issuing authority.
        let _fresh_files = collection.validate()?;
        Ok(Some((
            collection,
            contract::Selection {
                target,
                source: presentation,
            },
        )))
    }

    fn validate(&self) -> Result<Vec<source::FileLease>, WallpaperError> {
        let count = unsafe { self.array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?;
        match &self.source {
            Source::Images { images, parent } => {
                if count as usize != images.len() || !(MIN_ITEMS..=MAX_ITEMS).contains(&count) {
                    return Err(WallpaperError::Unavailable);
                }
                let mut files = Vec::with_capacity(images.len());
                for (index, image) in images.iter().enumerate() {
                    files.push(image.validate()?);
                    let item = unsafe { self.array.GetItemAt(index as u32) }
                        .map_err(|_| WallpaperError::Unavailable)?;
                    if !image.matches_item(&item)?
                        || !image.is_in_container(parent)?
                        || !source::same_container(&item, parent)?
                    {
                        return Err(WallpaperError::Unavailable);
                    }
                }
                Ok(files)
            }
            Source::Folder(folder) => {
                if count != 1 || !folder.is_current() {
                    return Err(WallpaperError::Unavailable);
                }
                let item =
                    unsafe { self.array.GetItemAt(0) }.map_err(|_| WallpaperError::Unavailable)?;
                let fresh = source::PolicyEntry::capture(item)?;
                if !fresh.is_directory()
                    || !folder.same_identity(&fresh)
                    || unsafe { self.array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?
                        != 1
                {
                    return Err(WallpaperError::Unavailable);
                }
                // No persistent directory lease and no child inventory.
                Ok(Vec::new())
            }
        }
    }

    pub(super) fn apply(
        self,
        options: contract::Options,
    ) -> Result<contract::ApplyOutcome, WallpaperError> {
        let desktop = displays::desktop()?;
        let _fresh_files = self.validate()?;
        // SetSlideshow also enables the desktop background. Its scope is global,
        // including future monitors; no captured monitor list or Enable effect.
        let collection = receipt(unsafe { desktop.SetSlideshow(&self.array) });
        let mut options_receipt = contract::NativeStep::NotSubmitted;
        if collection == contract::NativeStep::Accepted
            && let Ok(_fresh_files) = self.validate()
        {
            options_receipt = receipt(set_options(&desktop, options));
        }
        // Every readback is independent, even if a setter rejected its request.
        // A later loss of source authority cannot erase an earlier SDK receipt.
        let collection_matches = unsafe { desktop.GetSlideshow() }
            .ok()
            .and_then(|array| self.matches(&array).ok());
        let options_readback = read_options(&desktop);
        let state = read_state(&desktop);
        Ok(contract::ApplyOutcome {
            collection,
            options: options_receipt,
            collection_matches,
            options_readback,
            state,
        })
    }

    fn matches(&self, array: &IShellItemArray) -> Result<bool, WallpaperError> {
        let count = unsafe { array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?;
        // Count mismatches never require an unbounded/folder-content traversal.
        match &self.source {
            Source::Images { images, .. } => {
                if count as usize != images.len() {
                    return Ok(false);
                }
                let mut matched = vec![false; images.len()];
                for index in 0..count {
                    let item = unsafe { array.GetItemAt(index) }
                        .map_err(|_| WallpaperError::Unavailable)?;
                    let Some(member) = source::Image::find_file(images, &item)? else {
                        return Ok(false);
                    };
                    if matched[member] {
                        return Ok(false);
                    }
                    matched[member] = true;
                }
                Ok(true)
            }
            Source::Folder(folder) => {
                // Recheck original directory and array independently of the
                // setter receipt. This confirms an object, never its contents.
                self.validate()?;
                if count != 1 {
                    return Ok(false);
                }
                let item =
                    unsafe { array.GetItemAt(0) }.map_err(|_| WallpaperError::Unavailable)?;
                let fresh = source::PolicyEntry::capture(item)?;
                Ok(fresh.is_directory()
                    && folder.same_identity(&fresh)
                    && unsafe { array.GetCount() }.map_err(|_| WallpaperError::Unavailable)? == 1)
            }
        }
    }
}

fn receipt(result: windows::core::Result<()>) -> contract::NativeStep {
    if result.is_ok() {
        contract::NativeStep::Accepted
    } else {
        contract::NativeStep::Rejected
    }
}

/// One options-only SDK step; callers own source validation and native receipts.
pub(super) fn set_options(
    desktop: &IDesktopWallpaper,
    options: contract::Options,
) -> windows::core::Result<()> {
    let flags = if options.shuffle {
        DSO_SHUFFLEIMAGES
    } else {
        DESKTOP_SLIDESHOW_OPTIONS(0)
    };
    unsafe { desktop.SetSlideshowOptions(flags, options.interval.milliseconds()) }
}

pub(super) fn read_options(desktop: &IDesktopWallpaper) -> Option<contract::OptionsReadback> {
    let mut flags = DESKTOP_SLIDESHOW_OPTIONS(0);
    let mut interval_ms = 0;
    unsafe { desktop.GetSlideshowOptions(&mut flags, &mut interval_ms) }.ok()?;
    if flags.0 & !DSO_SHUFFLEIMAGES.0 != 0 {
        return None;
    }
    Some(contract::OptionsReadback {
        interval_ms,
        shuffle: flags.0 & DSO_SHUFFLEIMAGES.0 != 0,
    })
}

pub(super) fn read_state(desktop: &IDesktopWallpaper) -> Option<contract::StateFacts> {
    let state = unsafe { desktop.GetStatus() }.ok()?;
    let known = DSS_ENABLED.0 | DSS_SLIDESHOW.0 | DSS_DISABLED_BY_REMOTE_SESSION.0;
    if state.0 & !known != 0 {
        return None;
    }
    Some(contract::StateFacts {
        enabled: state.0 & DSS_ENABLED.0 != 0,
        slideshow: state.0 & DSS_SLIDESHOW.0 != 0,
        disabled_by_remote_session: state.0 & DSS_DISABLED_BY_REMOTE_SESSION.0 != 0,
    })
}
