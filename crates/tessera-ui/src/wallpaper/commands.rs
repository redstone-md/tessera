// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One submission/mailbox lifecycle for file drafts and actual slideshow policy.

use super::*;

impl WallpaperController {
    pub(super) fn request(self: &Rc<Self>, operation: Operation) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let session = self.state.borrow().session;
        let Some(session) = session else { return };
        if !self.intent_current(session, operation) {
            return;
        }
        let (provider, flight, request, retired, retired_current, retired_policy) = {
            let mut state = self.state.borrow_mut();
            if state.flight.is_some() || state.session != Some(session) {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            // Capture only weak policy identity and authority-free member tokens.
            // Read/advance never consume the independent image or collection drafts.
            let slideshow = match operation {
                Operation::ReadSlideshow => {
                    state.slideshow.as_ref().map(|current| current.issued(None))
                }
                Operation::AdvanceSlideshow(direction) => state
                    .slideshow
                    .as_ref()
                    .map(|current| current.issued(Some(direction))),
                _ => None,
            };
            let mut retired_current = None;
            let (request, retired) = match operation {
                Operation::Choose => (Request::Choose, state.clear_files()),
                Operation::ChooseCollection => (Request::ChooseCollection, state.clear_files()),
                Operation::Apply => {
                    let Some(selection) = state.selection.as_ref() else {
                        return;
                    };
                    if selection.scope_at(state.monitor_index).as_ref() != Some(&selection.scope) {
                        return;
                    }
                    let Some(requested) = selection.requested() else {
                        return;
                    };
                    let request = Request::Apply {
                        target: selection.image.target.clone(),
                        scope: selection.scope.clone(),
                        index: state.monitor_index,
                        requested,
                    };
                    // Keep only readonly display projection until the flight
                    // completes; no image authority remains in actor state.
                    (request, (state.selection.take(), None))
                }
                Operation::ApplyCollection => {
                    let Some(collection) = state.collection.as_mut() else {
                        return;
                    };
                    let Some(target) = collection.target.take() else {
                        return;
                    };
                    let request = Request::ApplyCollection {
                        target,
                        options: collection.options,
                        index: collection.index,
                    };
                    // Gallery and proposals remain readonly during submission so
                    // the final live geometry/input check still has its subject.
                    (request, (None, None))
                }
                Operation::ReadSlideshow => {
                    retired_current = state.slideshow.take();
                    (Request::ReadSlideshow, (None, None))
                }
                Operation::AdvanceSlideshow(direction) => {
                    let Some(current) = state.slideshow.as_mut() else {
                        return;
                    };
                    let Some(monitor) = current.member(current.index) else {
                        return;
                    };
                    let Some(target) = current.observation.target.take() else {
                        return;
                    };
                    (
                        Request::AdvanceSlideshow {
                            target,
                            monitor,
                            direction,
                            index: current.index,
                        },
                        (None, None),
                    )
                }
            };
            let Some(ticket) = state.next() else {
                drop(state);
                drop(retired);
                drop(request);
                drop(retired_current);
                self.stop_root();
                return;
            };
            let (scope, monitor_index, requested) = match &request {
                Request::Choose
                | Request::ChooseCollection
                | Request::ApplyCollection { .. }
                | Request::ReadSlideshow
                | Request::AdvanceSlideshow { .. } => (None, None, 0),
                Request::Apply {
                    scope,
                    index,
                    requested,
                    ..
                } => (Some(scope.clone()), Some(*index), *requested),
            };
            let (collection_target, collection_options, collection_index) = match &request {
                Request::ApplyCollection {
                    target,
                    options,
                    index,
                } => (Some(target.downgrade()), Some(*options), Some(*index)),
                _ => (None, None, None),
            };
            let flight = Flight {
                ticket,
                session,
                operation,
                scope,
                monitor_index,
                requested,
                collection_target,
                collection_options,
                collection_index,
                slideshow,
            };
            state.flight = Some(flight.clone());
            if !matches!(
                operation,
                Operation::ReadSlideshow | Operation::AdvanceSlideshow(_)
            ) {
                state.notice.clear();
                state.collection_notice.clear();
            } else {
                state.slideshow_notice.clear();
            }
            let retired_policy = if matches!(
                operation,
                Operation::Apply | Operation::ApplyCollection
            ) {
                let retired = state
                    .slideshow
                    .as_mut()
                    .and_then(|current| current.observation.target.take());
                state.slideshow_notice = "A file-policy apply intent retired current slideshow control authority. Read current again explicitly; displayed facts are only the previous observation.".into();
                retired
            } else {
                None
            };
            (
                provider,
                flight,
                request,
                retired,
                retired_current,
                retired_policy,
            )
        };
        drop(retired);
        drop(retired_current);
        drop(retired_policy);
        if !self.project(session, Some(operation))
            || !self.intent_current(session, operation)
            || !self.provider_current(Some(&provider))
            || !self.request_current(&request, flight.ticket)
            || self
                .state
                .borrow()
                .flight
                .as_ref()
                .is_none_or(|current| current.ticket != flight.ticket)
        {
            let retired = {
                let mut state = self.state.borrow_mut();
                state.flight = None;
                let retired = if matches!(
                    operation,
                    Operation::ReadSlideshow | Operation::AdvanceSlideshow(_)
                ) {
                    state.slideshow_notice = "The slideshow request lost live admission before submission. No SDK work was requested; Read current again explicitly.".into();
                    (None, None, state.slideshow.take())
                } else {
                    let (image, collection) = state.clear_files();
                    state.notice = "The file request lost its live admission before submission. No SDK work was requested; choose fresh files.".into();
                    (image, collection, None)
                };
                let _ = state.next();
                retired
            };
            drop(retired);
            drop(request);
            // A clipped file gesture is not retirement of the orthogonal
            // global-position observation. Only actual Root loss stops it.
            self.refresh_root();
            return;
        }
        let mailbox = self.mailbox.clone();
        let panel = self.panel.clone();
        // Accepted work retains its provider independently of this actor/Root.
        let owner = provider.clone();
        let admitted = {
            self.submitting.set(true);
            let _guard = ResetFlag(&self.submitting);
            match request {
                Request::Choose => {
                    let completion: WallpaperChooseCompletion = Box::new(move |result| {
                        let _owner = owner;
                        let result = result.map(|image| image.map(PreparedSelection::new));
                        deliver(&mailbox, &panel, flight.ticket, Reply::Chosen(result));
                    });
                    provider.choose(completion)
                }
                Request::Apply { target, scope, .. } => {
                    let completion: WallpaperApplyCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(&mailbox, &panel, flight.ticket, Reply::Applied(result));
                    });
                    provider.apply(target, scope, completion)
                }
                Request::ChooseCollection => {
                    let completion: native_collection::ChooseCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(
                            &mailbox,
                            &panel,
                            flight.ticket,
                            Reply::CollectionChosen(result),
                        );
                    });
                    provider.choose_collection(completion)
                }
                Request::ApplyCollection {
                    target, options, ..
                } => {
                    let completion: native_collection::ApplyCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(
                            &mailbox,
                            &panel,
                            flight.ticket,
                            Reply::CollectionApplied(result),
                        );
                    });
                    provider.apply_collection(target, options, completion)
                }
                Request::ReadSlideshow => {
                    let completion: native_slideshow::ReadCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(
                            &mailbox,
                            &panel,
                            flight.ticket,
                            Reply::SlideshowRead(result),
                        );
                    });
                    provider.read_slideshow(completion)
                }
                Request::AdvanceSlideshow {
                    target,
                    monitor,
                    direction,
                    ..
                } => {
                    let completion: native_slideshow::AdvanceCompletion = Box::new(move |result| {
                        let _owner = owner;
                        deliver(
                            &mailbox,
                            &panel,
                            flight.ticket,
                            Reply::SlideshowAdvanced(result),
                        );
                    });
                    provider.advance_slideshow(target, monitor, direction, completion)
                }
            }
        };
        if let Err(error) = admitted {
            // Immediate errors accepted no work and owe no completion.
            let reply = match operation {
                Operation::Choose => Reply::Chosen(Err(error)),
                Operation::Apply => Reply::Applied(Err(error)),
                Operation::ChooseCollection => Reply::CollectionChosen(Err(error)),
                Operation::ApplyCollection => Reply::CollectionApplied(Err(error)),
                Operation::ReadSlideshow => Reply::SlideshowRead(Err(error)),
                Operation::AdvanceSlideshow(_) => Reply::SlideshowAdvanced(Err(error)),
            };
            let retired = self.mailbox.lock().replace(Receipt {
                ticket: flight.ticket,
                reply,
            });
            drop(retired);
        }
        // Synchronous/reentrant providers cannot recursively start another flight.
        self.receive();
    }

    pub(super) fn request_current(&self, request: &Request, ticket: u64) -> bool {
        let state = self.state.borrow();
        let Some(flight) = state.flight.as_ref() else {
            return false;
        };
        if flight.ticket != ticket {
            return false;
        }
        match request {
            Request::Choose => flight.operation == Operation::Choose && flight.scope.is_none(),
            Request::Apply {
                scope,
                index,
                requested,
                ..
            } => {
                flight.operation == Operation::Apply
                    && flight.scope.as_ref() == Some(scope)
                    && flight.monitor_index == Some(*index)
                    && state.monitor_index == *index
                    && flight.requested == *requested
            }
            Request::ChooseCollection => {
                flight.operation == Operation::ChooseCollection
                    && flight.collection_target.is_none()
            }
            Request::ApplyCollection {
                target,
                options,
                index,
            } => {
                flight.operation == Operation::ApplyCollection
                    && flight
                        .collection_target
                        .as_ref()
                        .is_some_and(|weak| weak.is_alive() && weak.matches(target))
                    && flight.collection_options == Some(*options)
                    && flight.collection_index == Some(*index)
                    && state.collection.as_ref().is_some_and(|collection| {
                        collection.target.is_none()
                            && collection.options == *options
                            && collection.index == *index
                    })
            }
            Request::ReadSlideshow => {
                flight.operation == Operation::ReadSlideshow && state.slideshow.is_none()
            }
            Request::AdvanceSlideshow {
                target,
                monitor,
                direction,
                index,
            } => {
                flight.operation == Operation::AdvanceSlideshow(*direction)
                    && flight.slideshow.as_ref().is_some_and(|issued| {
                        issued
                            .target
                            .as_ref()
                            .is_some_and(|weak| weak.is_alive() && weak.matches(target))
                            && issued.monitor.as_ref() == Some(monitor)
                            && issued.direction == Some(*direction)
                            && issued.index == *index
                    })
                    && state.slideshow.as_ref().is_some_and(|current| {
                        current.observation.target.is_none()
                            && current.index == *index
                            && current.member(*index).as_ref() == Some(monitor)
                    })
            }
        }
    }

    pub(super) fn receive(self: &Rc<Self>) {
        if self.projecting.get() || self.acquiring.get() || self.submitting.get() {
            return;
        }
        let receipt = self.mailbox.lock().take();
        let Some(receipt) = receipt else { return };
        let flight = {
            let mut state = self.state.borrow_mut();
            let Some(current) = state.flight.as_ref() else {
                return;
            };
            if current.ticket != receipt.ticket {
                return;
            }
            let Some(flight) = state.flight.take() else {
                return;
            };
            flight
        };
        if !self.current(flight.session) {
            // Discard every old result/target before independently projecting
            // current availability. Releasing a flight does not revive its scope.
            drop(receipt);
            self.refresh_root();
            return;
        }
        if matches!(
            flight.operation,
            Operation::ChooseCollection | Operation::ApplyCollection
        ) {
            self.receive_collection(flight, receipt.reply);
            return;
        }
        if matches!(
            flight.operation,
            Operation::ReadSlideshow | Operation::AdvanceSlideshow(_)
        ) {
            self.receive_slideshow(flight, receipt.reply);
            return;
        }
        // Validation and rejected target destruction happen outside RefCell borrows.
        let (selection, notice) = match (flight.operation, receipt.reply) {
            (Operation::Choose, Reply::Chosen(Ok(Some(image)))) => {
                match SelectedImage::new(image).and_then(|selection| {
                    let notice = selection.notice(&selection.scope)?;
                    Some((selection, notice))
                }) {
                    Some((selection, notice)) => (Some(selection), notice),
                    None => (
                        None,
                        "The native image selection contains invalid display metadata. No image is selected; choose again.".into(),
                    ),
                }
            }
            (Operation::Choose, Reply::Chosen(Ok(None))) => {
                (None, "Image picker cancelled. No image is selected.".into())
            }
            (Operation::Apply, Reply::Applied(Ok(outcome))) => {
                (None, outcome_notice(outcome, flight.requested))
            }
            (Operation::Choose, Reply::Chosen(Err(error)))
            | (Operation::Apply, Reply::Applied(Err(error))) => {
                (None, error_notice(flight.operation, error).into())
            }
            _ => (
                None,
                "The provider returned an invalid response. Actual Windows wallpaper is unknown; choose a fresh image.".into(),
            ),
        };
        if !self.current(flight.session) {
            drop(selection);
            self.refresh_root();
            return;
        }
        let captions = selection
            .as_ref()
            .map(SelectedImage::captions)
            .unwrap_or_default();
        let index = if selection.is_some() { 0 } else { -1 };
        {
            let mut state = self.state.borrow_mut();
            state.selection = selection;
            state.monitor_captions = captions;
            state.monitor_index = index;
            state.notice = notice;
        }
        if !self.project(flight.session, None) {
            self.stop_root();
        }
    }
}
