// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HiddenDeadline {
    pub(super) epoch: u64,
    pub(super) generation: u64,
}

impl PowerMenuController {
    pub(super) fn surface_snapshot(&self) -> Option<(u64, Rc<TransientWindow<PowerMenuSurface>>)> {
        let state = self.state.borrow();
        if state.closed {
            return None;
        }
        let epoch = state.epoch;
        let surface = self.surface.borrow().clone()?;
        Some((epoch, surface))
    }

    pub(super) fn epoch_is(&self, epoch: u64) -> bool {
        let state = self.state.borrow();
        !state.closed && state.epoch == epoch && self.surface.borrow().is_some()
    }

    pub(super) fn scope_is(&self, generation: u64, epoch: u64) -> bool {
        self.generation_is(generation) && self.epoch_is(epoch)
    }

    pub(super) fn attach_callbacks(&self, surface: &PowerMenuSurface, epoch: u64) {
        let weak = self.weak.clone();
        // A queued wake carries no input authority. Even an old wake may drain
        // current domain tokens, but projection always obtains a current surface.
        surface.on_power_event_ready(move || {
            if let Some(controller) = weak.upgrade() {
                controller.process_events();
            }
        });
        let weak = self.weak.clone();
        surface.on_viewport_changed(move || {
            if let Some(controller) = weak
                .upgrade()
                .filter(|controller| controller.epoch_is(epoch))
            {
                controller.schedule_fit();
            }
        });
        let weak = self.weak.clone();
        surface.on_action_requested(move |action| {
            if let Some(controller) = weak
                .upgrade()
                .filter(|controller| controller.epoch_is(epoch))
            {
                controller.activate(action);
            }
        });
        let weak = self.weak.clone();
        surface.on_updates_choice_changed(move |choice| {
            if let Some(controller) = weak
                .upgrade()
                .filter(|controller| controller.epoch_is(epoch))
            {
                controller.choose_updates(epoch, choice);
            }
        });
        let weak = self.weak.clone();
        surface.on_hide_requested(move || {
            if let Some(controller) = weak
                .upgrade()
                .filter(|controller| controller.epoch_is(epoch))
            {
                controller.hide();
            }
        });
        let weak = self.weak.clone();
        surface.window().on_close_requested(move || {
            if let Some(controller) = weak
                .upgrade()
                .filter(|controller| controller.epoch_is(epoch))
            {
                controller.hide();
            }
            slint::CloseRequestResponse::KeepWindowShown
        });
    }

    // Only construction and an explicit show may create a generated surface.
    // Constructor/property callbacks may reenter; validate both intent and epoch
    // before publishing, never replace a newer inner candidate.
    pub(super) fn ensure_surface(&self, generation: u64) -> Result<bool, String> {
        if self.surface_snapshot().is_some() {
            return Ok(self.current(generation));
        }
        let epoch = {
            let mut state = self.state.borrow_mut();
            if !state.current(generation) {
                return Ok(false);
            }
            // Every candidate has a unique epoch, including an unpublished
            // outer constructor displaced by a reentrant inner show.
            let Some(epoch) = state.epoch.checked_add(1) else {
                drop(state);
                self.close();
                return Err("Power menu presentation identity is exhausted.".into());
            };
            state.epoch = epoch;
            epoch
        };
        let component =
            PowerMenuSurface::new().map_err(|_| "Could not create the Power menu surface.")?;
        let candidate = Rc::new(TransientWindow::new(
            self.host.clone(),
            component,
            SurfaceKind::Popup,
        ));
        self.attach_callbacks(&candidate, epoch);
        let published = {
            let mut state = self.state.borrow_mut();
            let mut surface = self.surface.borrow_mut();
            if !state.current(generation) || state.epoch != epoch || surface.is_some() {
                false
            } else {
                *surface = Some(candidate.clone());
                state.install_updates = true;
                state.updates_hint = None;
                state.updates_status.clear();
                state.initialized = false;
                state.initial_updates_generation = None;
                true
            }
        };
        if !published {
            return Ok(false);
        }
        let installed = self.mailbox.lock().install(epoch, candidate.as_weak());
        if let Err(error) = installed {
            self.close();
            return Err(error);
        }
        // Publish only. show() owns the single first drain/pump, including held
        // command terminals before busy projection; construction must not consume
        // inline display/update results and attach the native popup early.
        Ok(self.current(generation) && self.epoch_is(epoch))
    }

    // A cancelled native attachment is not a completed hide. Its local lease
    // and Presentation guard must unwind before this post-effect check can arm.
    pub(super) fn arm_completed_hide(&self) {
        if self.native_effect.get() || self.retirement_timer.running() {
            return;
        }
        let deadline = self.state.borrow().hidden_deadline;
        let Some(deadline) = deadline else { return };
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        if epoch != deadline.epoch
            || !self.retired(deadline.generation)
            || surface.is_visible()
            || surface.window().is_visible()
            || !self.scope_is(deadline.generation, epoch)
        {
            return;
        }
        let weak = self.weak.clone();
        self.retirement_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_secs(30),
            move || {
                if let Some(controller) = weak.upgrade() {
                    controller.retire_presentation(deadline);
                }
            },
        );
    }

    fn retire_presentation(&self, deadline: HiddenDeadline) {
        if self.native_effect.get() {
            return;
        }
        let Some((epoch, surface)) = self.surface_snapshot() else {
            return;
        };
        if epoch != deadline.epoch
            || !self.retired(deadline.generation)
            || surface.is_visible()
            || surface.window().is_visible()
        {
            return;
        }
        let retired = {
            let mut state = self.state.borrow_mut();
            if state.closed
                || state.desired
                || state.epoch != epoch
                || state.generation != deadline.generation
                || state.hidden_deadline != Some(deadline)
            {
                return;
            }
            let Some(next_epoch) = state.epoch.checked_add(1) else {
                drop(state);
                self.close();
                return;
            };
            state.epoch = next_epoch;
            state.hidden_deadline = None;
            state.authorized = None;
            self.surface.borrow_mut().take()
        };
        // Retire only the presentation wake before native/UI destructors.
        // The weak Root command wake and bounded command terminal survive.
        self.mailbox.lock().retire(epoch);
        self.fit_timer.stop();
        self.work_timer.stop();
        self.focus_watch.stop();
        if let Some(retired) = retired {
            retired.hide();
            drop(retired);
        }
        drop(surface);
    }
}
