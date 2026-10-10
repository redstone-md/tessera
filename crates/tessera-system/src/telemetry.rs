// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Optional, independent CPU-counter and usable physical-memory observations.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryError {
    Unsupported,
    Busy,
    Stopped,
    Unavailable,
    InvalidData,
    Native { code: u32 },
}

impl fmt::Display for TelemetryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("System telemetry is not supported"),
            Self::Busy => formatter.write_str("A telemetry read is already in progress"),
            Self::Stopped => formatter.write_str("The telemetry reader has stopped"),
            Self::Unavailable => formatter.write_str("System telemetry is unavailable"),
            Self::InvalidData => formatter.write_str("Windows returned invalid telemetry counters"),
            Self::Native { code } => {
                write!(
                    formatter,
                    "Telemetry read failed (native code 0x{code:08x})"
                )
            }
        }
    }
}
impl std::error::Error for TelemetryError {}

/// A fresh opaque identity resets the reader's CPU baseline on its next read.
/// Previously accepted reads remain independent and complete normally.
#[derive(Clone, Debug)]
pub struct TelemetryEpoch(Arc<EpochMarker>);
#[derive(Debug)]
struct EpochMarker;

impl TelemetryEpoch {
    pub fn new() -> Self {
        Self(Arc::new(EpochMarker))
    }
}
impl Default for TelemetryEpoch {
    fn default() -> Self {
        Self::new()
    }
}
impl PartialEq for TelemetryEpoch {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for TelemetryEpoch {}

/// Unknown is not zero. A first CPU sample has no previous counters to compare.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryFact<T> {
    Known(T),
    Unknown,
    Unavailable(TelemetryError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuCoverage {
    System {
        logical_processors: u32,
    },
    /// GetSystemTimes covers only the calling thread's primary group above 64 CPUs.
    PrimaryGroup {
        group: u16,
        logical_processors: u32,
        total_logical_processors: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuSample {
    /// Valid range is 0..=10_000; 10_000 means 100%. Never clamp malformed data.
    pub utilization_basis_points: u16,
    /// Actual monotonic time between these two successful counter observations.
    pub interval: Duration,
}

/// Physical memory usable by Windows, not installed DIMM capacity or process RAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhysicalMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Sequential best-effort reads, not an atomic snapshot or freshness guarantee.
/// CPU, topology, memory bytes, and the SDK memory-load estimate fail independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetrySnapshot {
    pub cpu: TelemetryFact<CpuSample>,
    pub cpu_coverage: TelemetryFact<CpuCoverage>,
    pub memory: TelemetryFact<PhysicalMemory>,
    pub memory_load_percent: TelemetryFact<u8>,
    /// Monotonic time before the sequential observations; delayed UI delivery
    /// must not turn an old receipt into a newly sampled value.
    pub observed_at: Instant,
}

pub type TelemetryCompletion =
    Box<dyn FnOnce(Result<TelemetrySnapshot, TelemetryError>) + Send + 'static>;

/// One shared lazy reader with one accepted read at a time, no watch or idle poll.
/// Success acknowledges admission only and accepts exactly one callback (possibly
/// inline); immediate failure accepts none. Accepted reads retain the native owner
/// through completion, including host retirement. Never join work on the UI thread.
pub trait TelemetryHost: Send + Sync + 'static {
    fn read(
        &self,
        epoch: TelemetryEpoch,
        completion: TelemetryCompletion,
    ) -> Result<(), TelemetryError>;
}
