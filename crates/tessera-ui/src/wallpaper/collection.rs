// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Collection facts extend the existing file-selection actor; no second lifecycle or scheduler.

use std::rc::Rc;

use tessera_system::wallpaper::collection::{
    ApplyOutcome, Interval, NativeStep, Options, Selection, Source, Target,
};

use crate::generated::{Panel, WallpaperCollectionItem};

use super::{
    Flight, Operation, Reply, Session, State, WallpaperController, error_notice, image, selection,
};

/// Only the optional target authorizes Start. The gallery and proposals are readonly facts
/// while an accepted request owns that target independently of this Root.
pub(super) struct SelectedCollection {
    pub(super) target: Option<Target>,
    pub(super) options: Options,
    pub(super) index: i32,
    items: slint::ModelRc<WallpaperCollectionItem>,
    source: SourceFacts,
}

enum SourceFacts {
    Images { count: usize },
    Folder { caption: String },
}

impl SelectedCollection {
    fn new(selection: Selection, operation: Operation) -> Option<Self> {
        let (source, items) = match (operation, selection.source) {
            (Operation::ChooseCollection, Source::Images(images)) => {
                if !(2..=32).contains(&images.len()) {
                    return None;
                }
                // Bound external caption traversal before allocating UI image handles.
                let captions: Option<Vec<_>> = images
                    .iter()
                    .map(|item| selection::bounded_caption(&item.caption, 96))
                    .collect();
                let captions = captions?;
                let count = images.len();
                let items = images
                    .iter()
                    .zip(captions)
                    .map(|(item, caption)| WallpaperCollectionItem {
                        caption: caption.into(),
                        preview: item
                            .preview
                            .as_ref()
                            .map(image::slint_image)
                            .unwrap_or_default(),
                        available: item.preview.is_some(),
                    })
                    .collect::<Vec<_>>();
                (
                    SourceFacts::Images { count },
                    slint::ModelRc::new(slint::VecModel::from(items)),
                )
            }
            (Operation::ChooseFolder, Source::Folder { caption }) => {
                if caption.chars().take(257).count() > 256 {
                    return None;
                }
                let caption = selection::bounded_caption(&caption, 256)?;
                // A folder is not an image/gallery member. Never enumerate it here.
                (SourceFacts::Folder { caption }, slint::ModelRc::default())
            }
            _ => return None,
        };
        Some(Self {
            target: Some(selection.target),
            // These are new proposed commands, never asserted as current SDK policy.
            options: Options {
                interval: Interval::ThirtyMinutes,
                shuffle: false,
            },
            index: 2,
            items,
            source,
        })
    }

    fn folder_caption(&self) -> &str {
        match &self.source {
            SourceFacts::Images { .. } => "",
            SourceFacts::Folder { caption } => caption,
        }
    }

    fn source_notice(&self) -> String {
        match &self.source {
            SourceFacts::Images { count } => format!(
                "{count} actual native-selected files. Thumbnails are Shell previews, not desktop pixels."
            ),
            SourceFacts::Folder { caption } => format!(
                "Native-selected folder: {caption}. Windows manages/enumerates folder contents; image count unavailable. No folder thumbnails or inventory were requested."
            ),
        }
    }
}

pub(super) struct Projection {
    pub(super) items: slint::ModelRc<WallpaperCollectionItem>,
    pub(super) folder_caption: slint::SharedString,
    pub(super) command_available: bool,
    pub(super) apply_enabled: bool,
    pub(super) index: i32,
    pub(super) shuffle: bool,
    pub(super) status: slint::SharedString,
}

impl Projection {
    pub(super) fn capture(state: &State, session: Session) -> Self {
        let collection = state.collection.as_ref();
        let status = match state.flight.as_ref() {
            Some(flight) if flight.session != session => {
                "An older native request is pending; its result cannot populate this Root.".into()
            }
            Some(flight) if flight.operation == Operation::ChooseCollection => {
                "Waiting for the native multi-select picker. Choose 2–32 actual images in the same folder; choosing does not start a slideshow.".into()
            }
            Some(flight) if flight.operation == Operation::ChooseFolder => {
                "Waiting for the native folder picker. Choosing one actual filesystem folder does not start a slideshow or enumerate its images.".into()
            }
            Some(flight) if flight.operation == Operation::ApplyCollection => {
                "Requesting the global Windows collection first, then the proposed timing/shuffle. These writes are not atomic; awaiting independent SDK readbacks.".into()
            }
            _ if !state.collection_notice.is_empty() => state.collection_notice.clone(),
            _ if collection.is_some() => {
                "The selected native collection has been consumed. Choose a fresh collection to start again.".into()
            }
            _ => "No file collection draft. Actual current slideshow policy is shown separately.".into(),
        };
        Self {
            items: collection
                .map(|group| group.items.clone())
                .unwrap_or_default(),
            folder_caption: collection
                .map(|group| group.folder_caption().into())
                .unwrap_or_default(),
            command_available: collection.is_some(),
            apply_enabled: state.provider.is_some()
                && state.flight.is_none()
                && collection.is_some_and(|group| group.target.is_some()),
            index: collection.map_or(-1, |group| group.index),
            shuffle: collection.is_some_and(|group| group.options.shuffle),
            status: status.into(),
        }
    }
}

impl WallpaperController {
    /// Draft edits have no host calls. Invalid indices retire authority instead of
    /// silently substituting an interval, and every edit retires held Start gestures.
    pub(super) fn collection_options_changed(self: &Rc<Self>, changed_index: Option<i32>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let Some((panel, frame)) = self.source() else {
            return;
        };
        let Some(session) = self.state.borrow().session else {
            return;
        };
        if frame != session.frame || !self.current(session) || !Self::focused(&panel) {
            return;
        }
        let index = panel.get_wallpaper_collection_interval_index();
        let shuffle = panel.get_wallpaper_collection_shuffle();
        let (retired, exhausted) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.provider.is_none() {
                return;
            }
            let Some(group) = state.collection.as_mut() else {
                return;
            };
            if group.target.is_none() {
                return;
            }
            let interval = usize::try_from(index)
                .ok()
                .and_then(|index| Interval::ALL.get(index))
                .copied();
            let agrees = match changed_index {
                Some(changed) => changed == index && shuffle == group.options.shuffle,
                None => index == group.index,
            };
            let retired = if let Some(interval) = interval.filter(|_| agrees) {
                group.index = index;
                group.options = Options { interval, shuffle };
                let source = group.source_notice();
                state.collection_notice = format!(
                    "{source} Proposed interval: {} ms; proposed shuffle: {shuffle}. These are not the separate current-policy observation. Only Start requests a Windows effect.",
                    interval.milliseconds(),
                );
                None
            } else {
                let retired = state.collection.take();
                state.collection_notice = "The proposed options are invalid. No SDK effect was requested; choose a fresh collection.".into();
                retired
            };
            (retired, state.next().is_none())
        };
        drop(retired);
        if exhausted || !self.project(session, None) {
            self.stop_root();
        }
    }

    pub(super) fn collection_options_current(&self, panel: &Panel) -> bool {
        let index = panel.get_wallpaper_collection_interval_index();
        let shuffle = panel.get_wallpaper_collection_shuffle();
        let folder_caption = panel.get_wallpaper_collection_folder_caption();
        let state = self.state.borrow();
        let Some(group) = state.collection.as_ref() else {
            return false;
        };
        if group.index != index
            || group.options.shuffle != shuffle
            || group.folder_caption() != folder_caption.as_str()
            || usize::try_from(index)
                .ok()
                .and_then(|index| Interval::ALL.get(index))
                .copied()
                != Some(group.options.interval)
        {
            return false;
        }
        if let Some(flight) = state.flight.as_ref() {
            return flight.operation == Operation::ApplyCollection
                && state.session == Some(flight.session)
                && flight.collection_index == Some(index)
                && flight.collection_options == Some(group.options)
                && group.target.is_none()
                && flight
                    .collection_target
                    .as_ref()
                    .is_some_and(|target| target.is_alive());
        }
        group.target.is_some()
    }

    pub(super) fn receive_collection(self: &Rc<Self>, flight: Flight, reply: Reply) {
        let (selected, notice) = match (flight.operation, reply) {
            (Operation::ChooseCollection, Reply::CollectionChosen(Ok(Some(selection))))
            | (Operation::ChooseFolder, Reply::FolderChosen(Ok(Some(selection)))) => {
                match SelectedCollection::new(selection, flight.operation) {
                    Some(group) => {
                        let notice = format!(
                            "{} The initial 30 minute / shuffle off values are proposals only; they do not describe the separate current-policy observation.",
                            group.source_notice(),
                        );
                        (Some(group), notice)
                    }
                    None => (None, "The provider returned an invalid or mismatched native source. No collection draft is selected; choose fresh images or a folder.".into()),
                }
            }
            (Operation::ChooseCollection, Reply::CollectionChosen(Ok(None))) => {
                (None, "Collection picker cancelled. No group is selected.".into())
            }
            (Operation::ChooseFolder, Reply::FolderChosen(Ok(None))) => {
                (None, "Folder picker cancelled. No folder collection draft is selected.".into())
            }
            (Operation::ApplyCollection, Reply::CollectionApplied(Ok(outcome))) => {
                (None, outcome_notice(outcome))
            }
            (Operation::ChooseCollection, Reply::CollectionChosen(Err(error)))
            | (Operation::ChooseFolder, Reply::FolderChosen(Err(error)))
            | (Operation::ApplyCollection, Reply::CollectionApplied(Err(error))) => {
                (None, error_notice(flight.operation, error).into())
            }
            _ => (None, "The provider returned an invalid collection response. Actual Windows policy is unknown; choose a fresh collection.".into()),
        };
        if !self.current(flight.session) {
            drop(selected);
            self.refresh_root();
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            let retired = if matches!(
                flight.operation,
                Operation::ChooseCollection | Operation::ChooseFolder
            ) {
                std::mem::replace(&mut state.collection, selected)
            } else {
                // Applying consumed all authority before submission. Keep only the
                // real gallery/proposals as readonly evidence, never as a retry.
                None
            };
            state.collection_notice = notice;
            retired
        };
        drop(retired);
        if !self.project(flight.session, None) {
            self.stop_root();
        }
    }
}

fn outcome_notice(outcome: ApplyOutcome) -> String {
    if outcome.collection == NativeStep::NotSubmitted
        || (outcome.collection != NativeStep::Accepted
            && outcome.options != NativeStep::NotSubmitted)
    {
        return "Invalid native collection receipts. Actual Windows policy is unknown; no rollback or retry is inferred. Choose a fresh collection.".into();
    }
    let receipt = |step| match step {
        NativeStep::NotSubmitted => "not submitted",
        NativeStep::Accepted => "accepted",
        NativeStep::Rejected => "rejected",
    };
    let identity = match outcome.collection_matches {
        Some(true) => "same native source identities; folder contents are not enumerated",
        Some(false) => "different native collection",
        None => "unavailable",
    };
    let options = outcome.options_readback.map_or_else(
        || "unavailable".into(),
        |options| {
            format!(
                "interval {} ms; shuffle {}",
                options.interval_ms, options.shuffle
            )
        },
    );
    let state = outcome.state.map_or_else(
        || "unavailable".into(),
        |state| {
            format!(
                "enabled {}; slideshow {}; disabled by remote session {}",
                state.enabled, state.slideshow, state.disabled_by_remote_session
            )
        },
    );
    format!(
        "SDK collection write: {}. SDK options write: {}.\nNative collection readback: {}.\nNative options readback: {}.\nNative state flags: {}.\nIndependent readbacks do not prove rendered pixels or advancement. Writes can partially succeed; no rollback or retry. Choose a fresh collection to start again.",
        receipt(outcome.collection),
        receipt(outcome.options),
        identity,
        options,
        state,
    )
}
