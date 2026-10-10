// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Explicit Material preview intent, orthogonal to immediate native wallpaper application.

use std::rc::Rc;
use std::sync::Arc;

use tessera_system::wallpaper::{WallpaperHost, WallpaperImageTarget};

use crate::SourceSeed;

use super::{ResetFlag, Session, WallpaperController};

struct ColorIntent {
    session: Session,
    ticket: u64,
    provider: Arc<dyn WallpaperHost>,
    target: WallpaperImageTarget,
    seed: SourceSeed,
    rgb: i32,
}

impl WallpaperController {
    pub(super) fn preview_colors(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let session = self.state.borrow().session;
        let Some(session) = session else { return };
        let Some((panel, frame)) = self.source() else {
            return;
        };
        if !self.current(session)
            || frame != session.frame
            || !Self::focused(&panel)
            || !panel.get_wallpaper_colors_input_active()
            || !panel.get_wallpaper_colors_control_visible()
        {
            return;
        }
        let displayed_rgb = panel.get_wallpaper_color_rgb();
        let intent = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.session != Some(session) || state.exhausted {
                return;
            }
            let Some(selection) = state.selection.as_ref() else {
                return;
            };
            let Some(seed) = selection.seed() else { return };
            let Ok(rgb) = i32::try_from(seed.rgb()) else {
                return;
            };
            if displayed_rgb != rgb {
                return;
            }
            let target = selection.image.target.clone();
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(ticket) = state.next() else {
                drop(state);
                drop(target);
                self.stop_root();
                return;
            };
            ColorIntent {
                session,
                ticket,
                provider,
                target,
                seed,
                rgb,
            }
        };
        if !self.color_current(&intent) {
            self.stop_root();
            return;
        }
        self.projecting.set(true);
        let guard = ResetFlag(&self.projecting);
        let changed = (|| {
            macro_rules! publish {
                ($setter:ident, $value:expr) => {
                    if !self.color_current(&intent) {
                        return false;
                    }
                    panel.$setter($value);
                    if !self.color_current(&intent) {
                        return false;
                    }
                };
            }
            publish!(set_wallpaper_colors_enabled, false);
            // A fresh action invalidates other held gestures without consuming
            // the selected file or submitting any native wallpaper work.
            publish!(set_wallpaper_input_key, intent.ticket.to_string().into());
            // Existing source_rgb/appearance-changed owns captured Material/HCT
            // preview propagation and ordinary preference Save/Cancel semantics.
            publish!(set_source_rgb, intent.rgb);
            let mut state = self.state.borrow_mut();
            let Some(selection) = state.selection.as_mut() else {
                return false;
            };
            selection.mark_colors_previewed();
            true
        })();
        drop(guard);
        if !changed || !self.project(session, None) {
            self.stop_root();
        }
    }

    fn color_current(&self, intent: &ColorIntent) -> bool {
        if !self.current(intent.session) || !self.provider_current(Some(&intent.provider)) {
            return false;
        }
        let Some((panel, frame)) = self.source() else {
            return false;
        };
        if frame != intent.session.frame
            || !Self::focused(&panel)
            || !panel.get_wallpaper_colors_input_active()
            || !panel.get_wallpaper_colors_control_visible()
            || panel.get_wallpaper_color_rgb() != intent.rgb
        {
            return false;
        }
        let state = self.state.borrow();
        state.flight.is_none()
            && state.sequence == intent.ticket
            && state.selection.as_ref().is_some_and(|selection| {
                selection.image.target == intent.target && selection.seed() == Some(intent.seed)
            })
    }
}
