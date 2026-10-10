// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Exact dialog array and protected member files, owned exclusively by the STA.

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
    images: Vec<source::Image>,
    parent: IShellItem,
}

impl Collection {
    pub(super) fn choose(
        owner: HWND,
        mailbox: &Arc<Mailbox>,
    ) -> Result<Option<(Self, contract::Selection)>, WallpaperError> {
        let Some(dialog) = source::choose_dialog(owner, true)? else {
            return Ok(None);
        };
        // Retain the exact SDK result, never an independently synthesized array.
        let array = unsafe { dialog.GetResults() }.map_err(|_| WallpaperError::Unavailable)?;
        let count = unsafe { array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?;
        if !(MIN_ITEMS..=MAX_ITEMS).contains(&count) {
            return Err(WallpaperError::Unavailable);
        }
        let mut images: Vec<source::Image> = Vec::with_capacity(count as usize);
        let mut parent = None;
        for index in 0..count {
            let item =
                unsafe { array.GetItemAt(index) }.map_err(|_| WallpaperError::Unavailable)?;
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
        let target = retirement_collection_target(mailbox);
        let collection = Self {
            target: target.downgrade(),
            array,
            images,
            parent: parent.ok_or(WallpaperError::Unavailable)?,
        };
        // Thumbnail handlers may reenter native code: validate the whole original
        // collection again immediately before issuing its portable authority.
        let _fresh_files = collection.validate()?;
        Ok(Some((collection, contract::Selection { target, items })))
    }

    fn validate(&self) -> Result<Vec<source::FileLease>, WallpaperError> {
        let count = unsafe { self.array.GetCount() }.map_err(|_| WallpaperError::Unavailable)?;
        if count as usize != self.images.len() || !(MIN_ITEMS..=MAX_ITEMS).contains(&count) {
            return Err(WallpaperError::Unavailable);
        }
        let mut files = Vec::with_capacity(self.images.len());
        for (index, image) in self.images.iter().enumerate() {
            files.push(image.validate()?);
            let item = unsafe { self.array.GetItemAt(index as u32) }
                .map_err(|_| WallpaperError::Unavailable)?;
            if !image.matches_item(&item)?
                || !image.is_in_container(&self.parent)?
                || !source::same_container(&item, &self.parent)?
            {
                return Err(WallpaperError::Unavailable);
            }
        }
        Ok(files)
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
            let flags = if options.shuffle {
                DSO_SHUFFLEIMAGES
            } else {
                DESKTOP_SLIDESHOW_OPTIONS(0)
            };
            options_receipt = receipt(unsafe {
                desktop.SetSlideshowOptions(flags, options.interval.milliseconds())
            });
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
        // A count mismatch is an actual different native collection, and does
        // not require enumerating an unbounded or folder-based native result.
        if count as usize != self.images.len() {
            return Ok(false);
        }
        let mut matched = vec![false; self.images.len()];
        for index in 0..count {
            let item =
                unsafe { array.GetItemAt(index) }.map_err(|_| WallpaperError::Unavailable)?;
            let Some(member) = source::Image::find_file(&self.images, &item)? else {
                return Ok(false);
            };
            if matched[member] {
                return Ok(false);
            }
            matched[member] = true;
        }
        Ok(true)
    }
}

fn receipt(result: windows::core::Result<()>) -> contract::NativeStep {
    if result.is_ok() {
        contract::NativeStep::Accepted
    } else {
        contract::NativeStep::Rejected
    }
}

fn read_options(desktop: &IDesktopWallpaper) -> Option<contract::OptionsReadback> {
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

fn read_state(desktop: &IDesktopWallpaper) -> Option<contract::StateFacts> {
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
