// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Static wallpaper is immediate OS state, independent of preference drafts.

use slint::ComponentHandle;

use super::{PanelController, Rc};
use crate::generated::Panel;
use crate::transient_window::TransientCache;
use crate::wallpaper::WallpaperController;

pub(super) type WallpaperRoots = TransientCache<WallpaperController>;

impl PanelController {
    pub(super) fn wire_wallpaper(&self, panel: &Panel) {
        if self.wallpaper.borrow().is_some() {
            return;
        }
        let admitted = self.settings_source_admission(panel, Panel::get_wallpaper_page_visible);
        let actor = WallpaperController::new(self.core.host().clone(), panel.as_weak(), admitted);
        let published = {
            let mut slot = self.wallpaper.borrow_mut();
            if slot.is_none() {
                *slot = Some(Rc::clone(&actor));
                true
            } else {
                false
            }
        };
        if !published {
            actor.stop_root();
            return;
        }
        let controller = self.clone();
        panel.on_wallpaper_view_changed(move || controller.sync_wallpaper_root());
        self.sync_wallpaper_root();
    }

    pub(super) fn sync_wallpaper_root(&self) {
        let actor = self.wallpaper.borrow().clone();
        if let Some(actor) = actor {
            actor.refresh_root();
        }
    }

    pub(super) fn stop_wallpaper_root(&self) {
        let actor = self.wallpaper.borrow().clone();
        if let Some(actor) = actor {
            actor.stop_root();
        }
    }
}
