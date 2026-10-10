// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One optional reader and one timer, bound to the exact admitted toolbar frame.

use crate::DesktopHost;
use crate::generated::Toolbar;
use parking_lot::Mutex;
use slint::{PhysicalPosition, PhysicalSize, SharedString};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tessera_system::telemetry::{
    CpuCoverage, TelemetryEpoch, TelemetryError, TelemetryFact, TelemetryHost, TelemetrySnapshot,
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const FRESH_FOR: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct TelemetryFrame {
    pub position: PhysicalPosition,
    pub size: PhysicalSize,
    pub scale: f32,
}
impl TelemetryFrame {
    fn valid(self) -> bool {
        self.size.width > 0 && self.size.height > 0 && self.scale.is_finite() && self.scale > 0.0
    }
}

#[derive(Clone)]
pub(crate) struct TelemetryProjection {
    pub visible: bool,
    pub text: SharedString,
    pub accessible_label: SharedString,
}

#[derive(Default)]
struct State {
    enabled: bool,
    frame: Option<TelemetryFrame>,
    epoch: Option<TelemetryEpoch>,
    provider: Option<Arc<dyn TelemetryHost>>,
    in_flight: bool,
    snapshot: Option<TelemetrySnapshot>,
    error: Option<TelemetryError>,
}

#[derive(Default)]
struct Mailbox {
    expected: Option<TelemetryEpoch>,
    receipt: Option<Result<TelemetrySnapshot, TelemetryError>>,
    scheduled: bool,
}
impl Mailbox {
    fn close(&mut self) {
        self.expected = None;
        self.receipt = None;
        // Preserve an already queued wake: resumes coalesce behind that wake.
    }
}

struct FlagGuard<'a>(&'a Cell<bool>);
impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

pub(crate) struct TelemetryController {
    host: Arc<dyn DesktopHost>,
    toolbar: slint::Weak<Toolbar>,
    admission: Rc<dyn Fn() -> Option<TelemetryFrame>>,
    projection: Rc<dyn Fn(TelemetryProjection)>,
    state: RefCell<State>,
    mailbox: Arc<Mutex<Mailbox>>,
    timer: slint::Timer,
    reconciling: Cell<bool>,
    projecting: Cell<bool>,
}

impl TelemetryController {
    /// The parent supplies weak readonly Root/native-frame admission, never a
    /// strong controller capture, desktop observation, or popup-operation token.
    pub(crate) fn new(
        host: Arc<dyn DesktopHost>,
        toolbar: slint::Weak<Toolbar>,
        admission: Rc<dyn Fn() -> Option<TelemetryFrame>>,
        projection: Rc<dyn Fn(TelemetryProjection)>,
    ) -> Rc<Self> {
        let controller = Rc::new(Self {
            host,
            toolbar,
            admission,
            projection,
            state: RefCell::default(),
            mailbox: Arc::new(Mutex::default()),
            timer: slint::Timer::default(),
            reconciling: Cell::new(false),
            projecting: Cell::new(false),
        });
        if let Some(toolbar) = controller.toolbar.upgrade() {
            let weak = Rc::downgrade(&controller);
            toolbar.on_telemetry_event_ready(move || {
                if let Some(controller) = weak.upgrade() {
                    controller.drain();
                }
            });
        }
        controller
    }

    pub(crate) fn set_enabled(self: &Rc<Self>, enabled: bool) {
        self.state.borrow_mut().enabled = enabled;
        self.refresh_root();
    }

    fn source(&self) -> Option<TelemetryFrame> {
        if !self.state.borrow().enabled {
            return None;
        }
        (self.admission)().filter(|frame| frame.valid())
    }

    /// Call synchronously on Root show/geometry/fullscreen/liveness transitions.
    /// Repeated reconciliation of an unchanged source does not request a read.
    pub(crate) fn refresh_root(self: &Rc<Self>) {
        if self.reconciling.replace(true) {
            return;
        }
        let _guard = FlagGuard(&self.reconciling);
        let Some(frame) = self.source() else {
            self.stop_root();
            return;
        };
        if self.state.borrow().frame == Some(frame) {
            self.project();
            return;
        }
        self.stop_root();
        if self.source() != Some(frame) {
            return;
        }
        // Capability lookup is inert and occurs only after actual admission.
        let provider = self.host.telemetry_host();
        if self.source() != Some(frame) {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.frame = Some(frame);
            state.epoch = Some(TelemetryEpoch::new());
            state.provider = provider;
        }
        self.project();
        if self.source() != Some(frame) {
            self.stop_root();
            return;
        }
        if self.state.borrow().provider.is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        self.timer
            .start(slint::TimerMode::Repeated, SAMPLE_INTERVAL, move || {
                if let Some(controller) = weak.upgrade() {
                    controller.refresh_root();
                    controller.request_read();
                }
            });
        self.request_read();
    }

    /// Retires demand and receipts immediately, never cancels or joins an accepted read.
    pub(crate) fn stop_root(&self) {
        self.timer.stop();
        self.mailbox.lock().close();
        {
            let mut state = self.state.borrow_mut();
            state.frame = None;
            state.epoch = None;
            state.provider = None;
            state.in_flight = false;
            state.snapshot = None;
            state.error = None;
        }
        self.project();
    }

    fn request_read(self: &Rc<Self>) {
        let Some(frame) = self.source() else {
            self.stop_root();
            return;
        };
        let request = {
            let mut state = self.state.borrow_mut();
            if state.frame != Some(frame) || state.in_flight {
                return;
            }
            let Some(provider) = state.provider.clone() else {
                return;
            };
            let Some(epoch) = state.epoch.clone() else {
                return;
            };
            // A failed owner is terminal for this source session; no retry loop.
            if state
                .error
                .is_some_and(|error| error != TelemetryError::Busy)
            {
                return;
            }
            state.in_flight = true;
            state.error = None;
            (provider, epoch)
        };
        let (provider, epoch) = request;
        self.mailbox.lock().expected = Some(epoch.clone());
        self.project();
        if self.source() != Some(frame) || self.state.borrow().epoch.as_ref() != Some(&epoch) {
            self.stop_root();
            return;
        }
        let mailbox = self.mailbox.clone();
        let toolbar = self.toolbar.clone();
        let expected = epoch.clone();
        let result = provider.read(
            epoch.clone(),
            Box::new(move |result| {
                complete(&mailbox, &toolbar, &expected, result);
            }),
        );
        if let Err(error) = result {
            if self.state.borrow().epoch.as_ref() != Some(&epoch) {
                return;
            }
            self.mailbox.lock().close();
            {
                let mut state = self.state.borrow_mut();
                state.in_flight = false;
                state.error = Some(error);
                state.snapshot = None;
            }
            if error != TelemetryError::Busy {
                self.timer.stop();
            }
            self.project();
        }
    }

    fn drain(self: &Rc<Self>) {
        // This queued wake has run even if its old source has since retired.
        self.mailbox.lock().scheduled = false;
        let source = self.source();
        if source.is_none() || source != self.state.borrow().frame {
            self.refresh_root();
            return;
        }
        let delivery = {
            let mut mailbox = self.mailbox.lock();
            let epoch = mailbox.expected.clone();
            mailbox.receipt.take().map(|receipt| (epoch, receipt))
        };
        if let Some((epoch, receipt)) = delivery {
            {
                let mut state = self.state.borrow_mut();
                if epoch != state.epoch || !state.in_flight {
                    return;
                }
                state.in_flight = false;
                match receipt {
                    Ok(snapshot) => {
                        state.snapshot = Some(snapshot);
                        state.error = None;
                    }
                    Err(error) => {
                        state.snapshot = None;
                        state.error = Some(error);
                    }
                }
            }
            self.mailbox.lock().expected = None;
            if self
                .state
                .borrow()
                .error
                .is_some_and(|error| error != TelemetryError::Busy)
            {
                self.timer.stop();
            }
        }
        self.project();
    }

    fn project(&self) {
        if self.projecting.replace(true) {
            return;
        }
        let projection = {
            let state = self.state.borrow();
            present(&state)
        };
        let visible = projection.visible;
        {
            let _guard = FlagGuard(&self.projecting);
            (self.projection)(projection);
        }
        // Generated setters may synchronously retire the Root during projection.
        let source = self.source();
        if visible && (source.is_none() || source != self.state.borrow().frame) {
            self.stop_root();
        }
    }
}

impl Drop for TelemetryController {
    fn drop(&mut self) {
        self.timer.stop();
        self.mailbox.lock().close();
        // Completion retains only its bounded mailbox and the weak toolbar.
    }
}

fn complete(
    mailbox: &Arc<Mutex<Mailbox>>,
    toolbar: &slint::Weak<Toolbar>,
    epoch: &TelemetryEpoch,
    result: Result<TelemetrySnapshot, TelemetryError>,
) {
    let schedule = {
        let mut mailbox = mailbox.lock();
        if mailbox.expected.as_ref() != Some(epoch) {
            return;
        }
        mailbox.receipt = Some(result);
        if mailbox.scheduled {
            false
        } else {
            mailbox.scheduled = true;
            true
        }
    };
    if schedule
        && toolbar
            .upgrade_in_event_loop(|toolbar| toolbar.invoke_telemetry_event_ready())
            .is_err()
    {
        mailbox.lock().scheduled = false;
    }
}

fn present(state: &State) -> TelemetryProjection {
    let visible = state.enabled && state.frame.is_some() && state.provider.is_some();
    let mut cpu = "CPU —".to_owned();
    let mut ram = "RAM —".to_owned();
    let mut details = vec![
        "CPU counters and usable physical RAM; independently observed, not an atomic live snapshot. Reads scheduled every 1 second only while this toolbar is enabled and admitted; observations older than 2 seconds are not displayed.".to_owned(),
    ];
    if let Some(error) = state.error {
        details.push(format!("Read unavailable: {error}."));
        if error != TelemetryError::Busy {
            details.push("Sampling paused for this source session.".to_owned());
        }
    } else if let Some(snapshot) = state.snapshot {
        let age = Instant::now().checked_duration_since(snapshot.observed_at);
        if let Some(age) = age.filter(|age| *age <= FRESH_FOR) {
            details.push(format!("Observation age {:.1} seconds.", age.as_secs_f64()));
            let coverage = match snapshot.cpu_coverage {
                TelemetryFact::Known(CpuCoverage::System { logical_processors })
                    if logical_processors > 0 && logical_processors <= 64 =>
                {
                    details.push(format!(
                        "CPU covers all {logical_processors} logical processors."
                    ));
                    Some("CPU".to_owned())
                }
                TelemetryFact::Known(CpuCoverage::PrimaryGroup {
                    group,
                    logical_processors,
                    total_logical_processors,
                }) if logical_processors > 0
                    && logical_processors <= 64
                    && total_logical_processors > 64
                    && logical_processors <= total_logical_processors =>
                {
                    details.push(format!("CPU covers primary processor group {group}: {logical_processors} of {total_logical_processors} system logical processors, not whole-system CPU."));
                    Some(format!("CPU G{group}"))
                }
                _ => {
                    details.push("CPU coverage unavailable.".to_owned());
                    None
                }
            };
            match snapshot.cpu {
                TelemetryFact::Known(sample)
                    if sample.utilization_basis_points <= 10_000
                        && !sample.interval.is_zero()
                        && coverage.is_some() =>
                {
                    cpu = format!(
                        "{} {}%",
                        coverage.as_deref().unwrap_or("CPU"),
                        (u32::from(sample.utilization_basis_points) + 50) / 100
                    );
                    details.push(format!(
                        "CPU {:.2}% averaged over {:.2} seconds between counter samples.",
                        f64::from(sample.utilization_basis_points) / 100.0,
                        sample.interval.as_secs_f64()
                    ));
                }
                TelemetryFact::Unknown => details.push(
                    "CPU unknown: waiting for a second counter sample in this sampling session."
                        .to_owned(),
                ),
                TelemetryFact::Unavailable(error) => {
                    details.push(format!("CPU unavailable: {error}."))
                }
                _ => details.push("CPU sample invalid or coverage unavailable.".to_owned()),
            }
            match snapshot.memory {
                TelemetryFact::Known(memory)
                    if memory.total_bytes > 0 && memory.available_bytes <= memory.total_bytes =>
                {
                    let used = memory.total_bytes - memory.available_bytes;
                    let percent = u128::from(used) * 100 / u128::from(memory.total_bytes);
                    ram = format!("RAM {percent}%");
                    details.push(format!("Usable physical RAM: {used} bytes used, {} bytes available, {} bytes total; not installed-memory capacity.", memory.available_bytes, memory.total_bytes));
                }
                TelemetryFact::Unavailable(error) => {
                    details.push(format!("Physical RAM unavailable: {error}."))
                }
                _ => details.push("Physical RAM unknown or invalid.".to_owned()),
            }
        } else {
            details.push(
                "Observation stale or timestamp invalid; waiting for a current read.".to_owned(),
            );
        }
    } else {
        details.push("No current sample.".to_owned());
    }
    TelemetryProjection {
        visible,
        text: format!("{cpu} · {ram}").into(),
        accessible_label: details.join(" ").into(),
    }
}
