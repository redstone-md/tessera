// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Raw enumeration currently lacks source PhysicalTarget admission and stable-id
//! fallback ordering. Reads are non-atomic and have no dedicated live display/text
//! change subscription. The explicit FirstFallback must not imply those guarantees.
use crate::single_flight::FlightGate;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use tessera_core::Rect;
use tessera_system::display_context::{
    DisplayContextCompletion, DisplayContextError, DisplayContextHost, DisplayLayout,
    DisplaySelection,
};

mod owner;

#[cfg(test)]
mod tests;
#[cfg(windows)]
mod win32;

type ReadResult = Result<Option<DisplayLayout>, DisplayContextError>;
type Job = Box<dyn FnOnce() + Send + 'static>;

/// Driver creation occurs on the request owner, never on the submitting thread.
/// Native driver and every resource it owns remain non-Send and worker-local.
trait Driver {
    fn monitors(&mut self) -> Result<Vec<Monitor>, DisplayContextError>;
    fn create_probe(
        &mut self,
        selected: Monitor,
        x: i32,
        y: i32,
    ) -> Result<(), DisplayContextError>;
    fn probe_matches(&mut self, selected: Monitor) -> Result<bool, DisplayContextError>;
    fn dpi(&mut self) -> Result<u32, DisplayContextError>;
    fn text_scale(&mut self) -> Result<f64, DisplayContextError>;
    fn close_probe(&mut self) -> Result<(), DisplayContextError>;
    fn finish(&mut self) -> Result<(), DisplayContextError>;
}

#[derive(Clone, Copy)]
struct Monitor {
    // Request-local identity only. It is never part of DisplayLayout or a callback.
    identity: usize,
    bounds: Rect,
    primary: bool,
}

type DriverFactory = dyn Fn() -> Result<Box<dyn Driver>, DisplayContextError> + Send + Sync;
type Spawn = dyn Fn(Job) -> std::io::Result<()> + Send + Sync;

#[derive(Clone)]
pub(crate) struct NativeDisplayContextHost {
    gate: FlightGate,
    factory: Arc<DriverFactory>,
    spawn: Arc<Spawn>,
}

#[cfg(windows)]
impl Default for NativeDisplayContextHost {
    fn default() -> Self {
        Self {
            gate: FlightGate::default(),
            factory: Arc::new(|| Ok(Box::new(owner::Owner::new(win32::NativeCalls::default())?))),
            spawn: Arc::new(|job| {
                std::thread::Builder::new()
                    .name("tessera-display-context".into())
                    .spawn(job)
                    .map(|_| ())
            }),
        }
    }
}

impl DisplayContextHost for NativeDisplayContextHost {
    fn read(&self, completion: DisplayContextCompletion) -> Result<(), DisplayContextError> {
        let flight = self.gate.try_enter().ok_or(DisplayContextError::Busy)?;
        let factory = self.factory.clone();
        // A failed spawn drops this job, releasing the flight and consumer with
        // zero callbacks. No queue, GUI join, retained driver, or idle polling.
        catch_unwind(AssertUnwindSafe(|| {
            (self.spawn)(Box::new(move || {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    let mut driver = factory()?;
                    let result = read_layout(driver.as_mut());
                    let cleanup = driver.finish();
                    match result {
                        Ok(layout) => cleanup.map(|()| layout),
                        Err(error) => Err(error),
                    }
                    // Driver drop (probe fallback, apartment, DPI) is inside catch
                    // and precedes gate retirement even if native reading panics.
                }))
                .unwrap_or(Err(DisplayContextError::Unavailable));
                drop(flight);
                let _ = catch_unwind(AssertUnwindSafe(|| completion(result)));
            }))
        }))
        .unwrap_or_else(|_| Err(std::io::Error::other("display worker spawn panicked")))
        .map_err(|_| DisplayContextError::Unavailable)
    }
}

fn read_layout(driver: &mut dyn Driver) -> ReadResult {
    let monitors = driver.monitors()?;
    let Some((desktop, selected, selection)) = geometry(&monitors)? else {
        return Ok(None);
    };
    let (x, y) = probe_center(selected.bounds)?;
    driver.create_probe(selected, x, y)?;
    if !driver.probe_matches(selected)? {
        return Err(DisplayContextError::InvalidData);
    }
    let dpi = driver.dpi()?;
    if dpi == 0 {
        return Err(DisplayContextError::InvalidData);
    }
    let text = driver.text_scale()?;
    if !text.is_finite() || text <= 0.0 {
        return Err(DisplayContextError::InvalidData);
    }
    let layout = DisplayLayout::new(
        desktop,
        selected.bounds,
        (f64::from(dpi) / 96.0) * text,
        selection,
    )?;
    // Explicit checked destruction is required before claiming successful data.
    // Failure/unwind still has the driver's best-effort cleanup fallback.
    driver.close_probe()?;
    Ok(Some(layout))
}

fn geometry(
    monitors: &[Monitor],
) -> Result<Option<(Rect, Monitor, DisplaySelection)>, DisplayContextError> {
    let Some(first) = monitors.first().copied() else {
        return Ok(None);
    };
    let mut selected = first;
    let mut primary_count = 0;
    let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
    for (index, monitor) in monitors.iter().enumerate() {
        if monitor.identity == 0
            || monitors[..index]
                .iter()
                .any(|prior| prior.identity == monitor.identity)
        {
            return Err(DisplayContextError::InvalidData);
        }
        if monitor.primary {
            primary_count += 1;
            selected = *monitor;
        }
        left = left.min(monitor.bounds.x());
        top = top.min(monitor.bounds.y());
        right = right.max(monitor.bounds.right());
        bottom = bottom.max(monitor.bounds.bottom());
    }
    if primary_count > 1 {
        return Err(DisplayContextError::InvalidData);
    }
    let width = u32::try_from(i64::from(right) - i64::from(left))
        .map_err(|_| DisplayContextError::InvalidData)?;
    let height = u32::try_from(i64::from(bottom) - i64::from(top))
        .map_err(|_| DisplayContextError::InvalidData)?;
    let desktop =
        Rect::new(left, top, width, height).map_err(|_| DisplayContextError::InvalidData)?;
    let selection = if primary_count == 1 {
        DisplaySelection::Primary
    } else {
        DisplaySelection::FirstFallback
    };
    Ok(Some((desktop, selected, selection)))
}

fn probe_center(bounds: Rect) -> Result<(i32, i32), DisplayContextError> {
    let x = i64::from(bounds.x()) + i64::from(bounds.width()) / 2;
    let y = i64::from(bounds.y()) + i64::from(bounds.height()) / 2;
    let x = i32::try_from(x).map_err(|_| DisplayContextError::InvalidData)?;
    let y = i32::try_from(y).map_err(|_| DisplayContextError::InvalidData)?;
    if x < bounds.x() || x >= bounds.right() || y < bounds.y() || y >= bounds.bottom() {
        return Err(DisplayContextError::InvalidData);
    }
    Ok((x, y))
}
