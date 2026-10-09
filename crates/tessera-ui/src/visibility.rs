// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Windowless visibility actor. Native callbacks only fill a bounded mailbox;
//! an explicitly attached weak Slint route drains it on the GUI thread. The
//! parent's revision-validated effect owns leases, placement and native windows.

mod mailbox;
mod model;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use slint::ComponentHandle;
use tessera_core::Rect;
use tessera_system::visibility::{
    AutoHideMode, Edge, PhysicalPoint, PointerEnvironment, PointerHost, PointerWatchError,
    PointerWatchGuard, VisibilityInputs, decide_visibility, edge_at,
};
pub(crate) use tessera_system::visibility::{BarConfig, BarFacts};

use model::{BarState, DeadlineToken};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BarVisibility {
    pub dock: bool,
    pub toolbar: bool,
}

impl Default for BarVisibility {
    fn default() -> Self {
        Self {
            dock: true,
            toolbar: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VisibilityGeometry {
    pub revision: u64,
    /// Full physical monitor, never a work area or logical coordinate rectangle.
    pub monitor: Rect,
    pub dock_edge: Edge,
    pub toolbar_edge: Edge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct VisibilityConfig {
    pub dock: BarConfig,
    pub toolbar: BarConfig,
}

impl Default for VisibilityConfig {
    fn default() -> Self {
        Self {
            dock: BarConfig::default(),
            toolbar: BarConfig {
                mode: AutoHideMode::Never,
                ..BarConfig::default()
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct VisibilityObservations {
    pub dock: BarFacts,
    pub toolbar: BarFacts,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PointerReadiness {
    #[default]
    Inactive,
    Starting,
    Ready,
    Failed(PointerWatchError),
    Closed,
}

#[derive(Default)]
struct State {
    epoch: u64,
    geometry: Option<VisibilityGeometry>,
    geometry_history: Option<VisibilityGeometry>,
    config: VisibilityConfig,
    facts: VisibilityObservations,
    pointer: Option<PhysicalPoint>,
    environment: PointerEnvironment,
    readiness: PointerReadiness,
    bars: [BarState; 2],
    last_effect: Option<(u64, BarVisibility)>,
}

impl State {
    fn visibility(&self) -> BarVisibility {
        BarVisibility {
            dock: self.bars[0].visible(),
            toolbar: self.bars[1].visible(),
        }
    }
}

pub(crate) struct VisibilityController {
    host: Arc<dyn PointerHost>,
    effect: Box<dyn Fn(BarVisibility, u64)>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<mailbox::Mailbox>>,
    guard: RefCell<Option<Box<dyn PointerWatchGuard>>>,
    timers: [slint::Timer; 2],
    installed: [Cell<Option<DeadlineToken>>; 2],
    origin: Instant,
    closed: Cell<bool>,
}

impl VisibilityController {
    /// Inert construction: no capability acquisition, windows, timers or native
    /// registration. The effect executes only on GUI-thread update/drain/timer.
    pub(crate) fn new(
        host: Arc<dyn PointerHost>,
        effect: impl Fn(BarVisibility, u64) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            host,
            effect: Box::new(effect),
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            guard: RefCell::default(),
            timers: std::array::from_fn(|_| slint::Timer::default()),
            installed: std::array::from_fn(|_| Cell::new(None)),
            origin: Instant::now(),
            closed: Cell::new(false),
        })
    }

    /// Parent supplies an existing GUI event callback that calls process_events.
    /// This module retains only the weak component. Dispatch failure has no
    /// native-thread direct-effect fallback. Attach before start, including tests
    /// that wish to exercise callback admission through the real mailbox seam.
    pub(crate) fn attach_wake<C: ComponentHandle + 'static>(
        &self,
        component: &C,
        dispatch: impl Fn(C) + Send + Sync + 'static,
    ) {
        if self.closed.get() {
            return;
        }
        let weak = component.as_weak();
        let dispatch = Arc::new(dispatch);
        self.mailbox.lock().wake = Some(Arc::new(move || {
            let dispatch = dispatch.clone();
            weak.upgrade_in_event_loop(move |component| dispatch(component))
                .map_err(|_| ())
        }));
    }

    /// Accepted watch lives across both autohide and fullscreen. This does not
    /// gate supervisor/root readiness. Explicit failure recovery may call start
    /// again; there is no automatic polling or reinstall loop.
    pub(crate) fn start(self: &Rc<Self>) -> Result<(), PointerWatchError> {
        if self.closed.get() {
            return Err(PointerWatchError::Stopped);
        }
        if self.mailbox.lock().wake.is_none() {
            return Err(PointerWatchError::Unavailable);
        }
        if matches!(
            self.readiness(),
            PointerReadiness::Starting | PointerReadiness::Ready
        ) {
            return Err(PointerWatchError::Busy);
        }
        let epoch = {
            let mut state = self.state.borrow_mut();
            let epoch = state
                .epoch
                .checked_add(1)
                .ok_or(PointerWatchError::Stopped)?;
            state.epoch = epoch;
            state.readiness = PointerReadiness::Starting;
            state.pointer = None;
            state.environment = PointerEnvironment::default();
            epoch
        };
        self.mailbox.lock().activate(epoch);
        self.evaluate();
        if self.closed.get() || self.state.borrow().epoch != epoch {
            return Err(PointerWatchError::Stopped);
        }
        let event_slot = self.mailbox.clone();
        let ready_slot = self.mailbox.clone();
        // No RefCell borrow or lock spans a possibly synchronous host callback.
        let result = self.host.watch(
            Arc::new(move |event| mailbox::event(&event_slot, epoch, event)),
            Box::new(move |result| mailbox::ready(&ready_slot, epoch, result)),
        );
        match result {
            Ok(guard) => {
                if self.closed.get() || self.state.borrow().epoch != epoch {
                    drop(guard);
                } else {
                    *self.guard.borrow_mut() = Some(guard);
                }
                Ok(())
            }
            Err(error) => {
                mailbox::ready(&self.mailbox, epoch, Err(error));
                Err(error)
            }
        }
    }

    /// Facts come from the existing desktop subscription before lossy UI rows.
    /// Same-revision geometry is immutable; stale/reused geometry is denied.
    /// None retires geometry authority without touching a window. Touch/focus
    /// must remain None until real protection facts are available.
    pub(crate) fn update(
        self: &Rc<Self>,
        config: VisibilityConfig,
        geometry: Option<VisibilityGeometry>,
        facts: VisibilityObservations,
    ) {
        if self.closed.get() {
            return;
        }
        let changed = {
            let mut state = self.state.borrow_mut();
            if let (Some(current), Some(next)) = (state.geometry_history, geometry)
                && (next.revision < current.revision
                    || (next.revision == current.revision
                        && (next != current || state.geometry.is_none())))
            {
                return;
            }
            let changed = state.geometry != geometry;
            if changed {
                state.pointer = None;
                for bar in &mut state.bars {
                    bar.invalidate();
                }
            }
            state.geometry = geometry;
            if let Some(geometry) = geometry {
                state.geometry_history = Some(geometry);
            }
            state.config = config;
            state.facts = facts;
            changed
        };
        if changed {
            let mut mailbox = self.mailbox.lock();
            mailbox.geometry_revision = geometry.map(|geometry| geometry.revision);
            mailbox.position = None;
        }
        self.evaluate();
    }

    /// GUI callback only. Failure reveals/cancels overlap hiding, except already
    /// confirmed fullscreen, without observing windows/catalog/settings.
    pub(crate) fn process_events(self: &Rc<Self>) {
        if self.closed.get() {
            return;
        }
        let (epoch, ready, position, environment, failure) = {
            let mut mailbox = self.mailbox.lock();
            if !mailbox.live {
                return;
            }
            mailbox.queued = false;
            (
                mailbox.epoch,
                mailbox.ready.take(),
                mailbox.position.take(),
                mailbox.environment.take(),
                mailbox.failure.take(),
            )
        };
        let failed = {
            let mut state = self.state.borrow_mut();
            if state.epoch != epoch {
                return;
            }
            if let Some(ready) = ready {
                state.readiness = match ready {
                    Ok(()) => PointerReadiness::Ready,
                    Err(error) => PointerReadiness::Failed(error),
                };
            }
            if let Some(error) = failure {
                state.readiness = PointerReadiness::Failed(error);
            }
            let failed = matches!(state.readiness, PointerReadiness::Failed(_));
            if failed {
                state.pointer = None;
                state.environment = PointerEnvironment::default();
            } else {
                if let Some((revision, position)) = position
                    && revision == state.geometry.map(|geometry| geometry.revision)
                {
                    state.pointer = Some(position);
                }
                if let Some(environment) = environment {
                    state.environment = environment;
                }
            }
            failed
        };
        if failed {
            self.mailbox.lock().retire();
            let guard = self.guard.borrow_mut().take();
            drop(guard);
        }
        self.evaluate();
    }

    #[cfg(test)]
    pub(crate) fn current(&self) -> BarVisibility {
        self.state.borrow().visibility()
    }

    pub(crate) fn readiness(&self) -> PointerReadiness {
        self.state.borrow().readiness
    }

    fn evaluate(self: &Rc<Self>) {
        if self.closed.get() {
            return;
        }
        let now = self.origin.elapsed();
        {
            let mut state = self.state.borrow_mut();
            if let Some(geometry) = state.geometry {
                let inputs = [
                    (state.config.dock, state.facts.dock, geometry.dock_edge),
                    (
                        state.config.toolbar,
                        state.facts.toolbar,
                        geometry.toolbar_edge,
                    ),
                ];
                let edge = state.pointer.map(|point| edge_at(geometry.monitor, point));
                let pointer_ready = state.readiness == PointerReadiness::Ready;
                let environment = state.environment;
                for (index, (config, facts, selected)) in inputs.into_iter().enumerate() {
                    let facts = model::protected_facts(facts, environment);
                    let decision = decide_visibility(VisibilityInputs {
                        config,
                        facts,
                        pointer_ready,
                        selected_edge: edge.map(|edge| edge == Some(selected)),
                    });
                    state.bars[index].apply(decision, geometry.revision, now);
                }
            } else {
                for bar in &mut state.bars {
                    bar.apply(
                        tessera_system::visibility::VisibilityDecision::ShowNow,
                        0,
                        now,
                    );
                }
                state.last_effect = None;
            }
        }
        self.sync_timers(now);
        self.emit();
    }

    fn sync_timers(self: &Rc<Self>, now: Duration) {
        let pending = {
            let state = self.state.borrow();
            [state.bars[0].pending, state.bars[1].pending]
        };
        for (index, pending) in pending.into_iter().enumerate() {
            let Some(pending) = pending else {
                self.timers[index].stop();
                self.installed[index].set(None);
                continue;
            };
            if self.installed[index].get() == Some(pending.token) {
                continue;
            }
            self.installed[index].set(Some(pending.token));
            let weak = Rc::downgrade(self);
            self.timers[index].start(
                slint::TimerMode::SingleShot,
                pending.at.saturating_sub(now),
                move || {
                    if let Some(controller) = weak.upgrade() {
                        controller.timer_expired(index, pending.token);
                    }
                },
            );
        }
    }

    fn timer_expired(self: &Rc<Self>, index: usize, token: DeadlineToken) {
        if self.closed.get() || self.installed[index].get() != Some(token) {
            return;
        }
        self.installed[index].set(None);
        let now = self.origin.elapsed();
        {
            let mut state = self.state.borrow_mut();
            let Some(geometry) = state.geometry else {
                return;
            };
            state.bars[index].expire(token, geometry.revision, now);
        }
        self.sync_timers(now);
        self.emit();
    }

    fn emit(&self) {
        let effect = {
            let mut state = self.state.borrow_mut();
            let Some(geometry) = state.geometry else {
                return;
            };
            let effect = (geometry.revision, state.visibility());
            if state.last_effect == Some(effect) {
                return;
            }
            state.last_effect = Some(effect);
            effect
        };
        // The parent revalidates revision before lease-before-hide / reveal-
        // without-focus effects. Reentrant close/update is safe outside borrows.
        (self.effect)(effect.1, effect.0);
    }

    /// Closes mailbox admission before stopping finite timers and retiring the
    /// nonjoining native guard. No native callback retains the actor or windows.
    pub(crate) fn close(&self) {
        if self.closed.replace(true) {
            return;
        }
        self.mailbox.lock().retire();
        {
            let mut state = self.state.borrow_mut();
            state.readiness = PointerReadiness::Closed;
            state.pointer = None;
            for bar in &mut state.bars {
                bar.invalidate();
            }
        }
        for (timer, installed) in self.timers.iter().zip(&self.installed) {
            timer.stop();
            installed.set(None);
        }
        let guard = self.guard.borrow_mut().take();
        drop(guard);
    }
}

impl Drop for VisibilityController {
    fn drop(&mut self) {
        self.close();
    }
}
