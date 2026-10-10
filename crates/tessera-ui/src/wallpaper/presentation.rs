// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Snapshot presentation facts without carrying a borrow across UI projection.

use std::sync::Arc;

use tessera_system::wallpaper::WallpaperHost;

use super::selection::SelectionPresentation;
use super::{Operation, ResetFlag, SelectedImage, Session, State, WallpaperController, collection};

struct Projection {
    provider: Option<Arc<dyn WallpaperHost>>,
    busy: bool,
    has_selection: bool,
    captions: Vec<slint::SharedString>,
    index: i32,
    key: slint::SharedString,
    status: slint::SharedString,
    selected: SelectionPresentation,
    collection: collection::Projection,
    revision: u64,
    ticket: Option<u64>,
}

impl Projection {
    fn capture(state: &State, session: Session) -> Self {
        let status = match state.flight.as_ref() {
            Some(flight) if flight.session != session => {
                "A previous wallpaper request is pending. Its result will not be shown in this view.".into()
            }
            Some(flight) if flight.operation == Operation::Choose => {
                "Waiting for the Windows image picker… Choosing does not apply the image.".into()
            }
            Some(flight) if matches!(flight.operation, Operation::ChooseCollection | Operation::ApplyCollection) => {
                "A native collection request is pending. See global slideshow receipts below.".into()
            }
            Some(flight) => format!(
                "Requesting wallpaper on {} selected captured display(s)… Awaiting native path/file readback; rendered pixels are not checked.",
                flight.requested
            ),
            None if state.notice.is_empty() => {
                "Current Windows wallpaper has not been read. Choose an image to begin.".into()
            }
            None => state.notice.clone(),
        };
        Self {
            provider: state.provider.clone(),
            busy: state.flight.is_some(),
            has_selection: state.selection.is_some(),
            captions: state.monitor_captions.clone(),
            index: if state.monitor_captions.is_empty() {
                -1
            } else {
                state.monitor_index
            },
            key: state.sequence.to_string().into(),
            status: status.into(),
            selected: state
                .selection
                .as_ref()
                .map(SelectedImage::presentation)
                .unwrap_or_default(),
            collection: collection::Projection::capture(state, session),
            revision: state.sequence,
            ticket: state.flight.as_ref().map(|flight| flight.ticket),
        }
    }
}

impl WallpaperController {
    pub(super) fn project(&self, session: Session, intent: Option<Operation>) -> bool {
        if self.projecting.replace(true) {
            return false;
        }
        let _guard = ResetFlag(&self.projecting);
        let projection = {
            let state = self.state.borrow();
            Projection::capture(&state, session)
        };
        let Some(panel) = self.panel.upgrade() else {
            return false;
        };
        let current = || {
            self.current(session)
                && self.provider_current(projection.provider.as_ref())
                && {
                    let state = self.state.borrow();
                    state.sequence == projection.revision
                        && state.flight.as_ref().map(|flight| flight.ticket) == projection.ticket
                }
                && intent.is_none_or(|operation| self.intent_current(session, operation))
        };
        // Every setter may reenter. Check the full scope after even same-state writes.
        macro_rules! publish {
            ($setter:ident, $value:expr) => {
                if !current() {
                    return false;
                }
                panel.$setter($value);
                if !current() {
                    return false;
                }
            };
        }
        publish!(set_wallpaper_controls_enabled, false);
        publish!(set_wallpaper_apply_enabled, false);
        publish!(set_wallpaper_colors_enabled, false);
        publish!(set_wallpaper_collection_controls_enabled, false);
        publish!(set_wallpaper_collection_apply_enabled, false);
        // Consuming the image hides all selection-only presentation immediately.
        // Pending readonly monitor captions/index preserve final scope agreement only.
        publish!(
            set_wallpaper_monitor_selection_available,
            projection.has_selection
        );
        publish!(set_wallpaper_preview_available, false);
        publish!(set_wallpaper_preview, projection.selected.preview);
        publish!(
            set_wallpaper_preview_status,
            projection.selected.preview_status
        );
        publish!(set_wallpaper_color_rgb, projection.selected.seed_rgb);
        publish!(
            set_wallpaper_colors_status,
            projection.selected.colors_status
        );
        publish!(
            set_wallpaper_preview_available,
            projection.selected.preview_available
        );
        publish!(
            set_wallpaper_monitors,
            slint::ModelRc::new(slint::VecModel::from(projection.captions))
        );
        publish!(set_wallpaper_monitor_index, projection.index);
        publish!(set_wallpaper_available, projection.provider.is_some());
        publish!(set_wallpaper_busy, projection.busy);
        publish!(set_wallpaper_input_key, projection.key);
        publish!(set_wallpaper_status, projection.status);
        publish!(set_wallpaper_collection_items, projection.collection.items);
        publish!(
            set_wallpaper_collection_interval_index,
            projection.collection.index
        );
        publish!(
            set_wallpaper_collection_shuffle,
            projection.collection.shuffle
        );
        publish!(
            set_wallpaper_collection_command_available,
            projection.collection.command_available
        );
        publish!(
            set_wallpaper_collection_status,
            projection.collection.status
        );
        publish!(
            set_wallpaper_collection_apply_enabled,
            projection.collection.apply_enabled
        );
        publish!(
            set_wallpaper_collection_controls_enabled,
            projection.provider.is_some() && !projection.busy
        );
        publish!(
            set_wallpaper_apply_enabled,
            projection.provider.is_some() && !projection.busy && projection.has_selection
        );
        publish!(
            set_wallpaper_colors_enabled,
            projection.provider.is_some()
                && !projection.busy
                && projection.selected.colors_available
        );
        publish!(
            set_wallpaper_controls_enabled,
            projection.provider.is_some() && !projection.busy
        );
        true
    }
}
