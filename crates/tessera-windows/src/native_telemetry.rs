// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Read-only SDK calls confined to the same telemetry worker for every sample.

use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Instant;
use tessera_system::telemetry::{
    CpuCoverage, CpuSample, PhysicalMemory, TelemetryEpoch, TelemetryError, TelemetryFact,
    TelemetrySnapshot,
};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::SystemInformation::{
    GROUP_AFFINITY, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetCurrentThread, GetSystemTimes,
    GetThreadGroupAffinity,
};

fn error(value: windows::core::Error) -> TelemetryError {
    TelemetryError::Native {
        code: value.code().0 as u32,
    }
}
fn ticks(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

#[derive(Clone, Copy)]
struct Counters {
    idle: u64,
    kernel: u64,
    user: u64,
    observed: Instant,
    coverage: CpuCoverage,
}

#[derive(Default)]
pub(crate) struct NativeReader {
    epoch: Option<TelemetryEpoch>,
    baseline: Option<Counters>,
    _thread: PhantomData<Rc<()>>,
}

impl NativeReader {
    pub(crate) fn read(&mut self, epoch: TelemetryEpoch) -> TelemetrySnapshot {
        if self.epoch.as_ref() != Some(&epoch) {
            self.baseline = None;
            self.epoch = Some(epoch);
        }
        let observed_at = Instant::now();
        let (cpu, cpu_coverage) = self.cpu();
        let (memory, memory_load_percent) = memory();
        TelemetrySnapshot {
            cpu,
            cpu_coverage,
            memory,
            memory_load_percent,
            observed_at,
        }
    }

    fn cpu(&mut self) -> (TelemetryFact<CpuSample>, TelemetryFact<CpuCoverage>) {
        let current = match counters() {
            Ok(current) => current,
            Err(error) => {
                self.baseline = None;
                return (
                    TelemetryFact::Unavailable(error),
                    coverage().map_or_else(TelemetryFact::Unavailable, TelemetryFact::Known),
                );
            }
        };
        let previous = self.baseline.replace(current);
        let sample = match previous {
            Some(previous) if previous.coverage == current.coverage => {
                match delta(previous, current) {
                    Ok(sample) => TelemetryFact::Known(sample),
                    Err(error) => {
                        self.baseline = None;
                        TelemetryFact::Unavailable(error)
                    }
                }
            }
            _ => TelemetryFact::Unknown,
        };
        (sample, TelemetryFact::Known(current.coverage))
    }
}

fn coverage() -> Result<CpuCoverage, TelemetryError> {
    // SAFETY: no pointer arguments or state mutation; zero is an invalid count.
    let total = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    if total == 0 {
        return Err(TelemetryError::InvalidData);
    }
    if total <= 64 {
        return Ok(CpuCoverage::System {
            logical_processors: total,
        });
    }
    let mut affinity = GROUP_AFFINITY::default();
    // SAFETY: pseudo-handle belongs to this stable worker; output is initialized
    // and writable. Windows 11+ reports the thread's primary group here, even
    // when its default affinity spans groups. This is GetSystemTimes' coverage.
    if !unsafe { GetThreadGroupAffinity(GetCurrentThread(), &mut affinity) }.as_bool() {
        return Err(error(windows::core::Error::from_thread()));
    }
    // SAFETY: SDK-provided group number; the API reports zero on invalid groups.
    let count = unsafe { GetActiveProcessorCount(affinity.Group) };
    if affinity.Group == ALL_PROCESSOR_GROUPS || count == 0 || count > 64 || count > total {
        return Err(TelemetryError::InvalidData);
    }
    Ok(CpuCoverage::PrimaryGroup {
        group: affinity.Group,
        logical_processors: count,
        total_logical_processors: total,
    })
}

fn counters() -> Result<Counters, TelemetryError> {
    let before = coverage()?;
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all optional outputs point to initialized, separate stack values.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }
        .map_err(error)?;
    let observed = Instant::now();
    let after = coverage()?;
    let idle = ticks(idle);
    let kernel = ticks(kernel);
    let user = ticks(user);
    if before != after || idle > kernel || kernel.checked_add(user).is_none() {
        return Err(TelemetryError::InvalidData);
    }
    Ok(Counters {
        idle,
        kernel,
        user,
        observed,
        coverage: after,
    })
}

fn delta(previous: Counters, current: Counters) -> Result<CpuSample, TelemetryError> {
    let invalid = TelemetryError::InvalidData;
    let kernel = current.kernel.checked_sub(previous.kernel).ok_or(invalid)?;
    let user = current.user.checked_sub(previous.user).ok_or(invalid)?;
    let idle = current.idle.checked_sub(previous.idle).ok_or(invalid)?;
    let total = kernel.checked_add(user).ok_or(invalid)?;
    let active = total.checked_sub(idle).ok_or(invalid)?;
    let interval = current
        .observed
        .checked_duration_since(previous.observed)
        .filter(|interval| !interval.is_zero())
        .ok_or(invalid)?;
    // Kernel counters include idle. Reject inconsistent deltas, never clamp.
    if total == 0 || idle > kernel {
        return Err(invalid);
    }
    let basis_points = u128::from(active) * 10_000 / u128::from(total);
    let utilization_basis_points = u16::try_from(basis_points)
        .ok()
        .filter(|value| *value <= 10_000)
        .ok_or(invalid)?;
    Ok(CpuSample {
        utilization_basis_points,
        interval,
    })
}

fn memory() -> (TelemetryFact<PhysicalMemory>, TelemetryFact<u8>) {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: initialized struct has the required byte length and writable storage.
    if let Err(value) = unsafe { GlobalMemoryStatusEx(&mut status) } {
        let error = error(value);
        return (
            TelemetryFact::Unavailable(error),
            TelemetryFact::Unavailable(error),
        );
    }
    let bytes = if status.ullTotalPhys == 0 || status.ullAvailPhys > status.ullTotalPhys {
        TelemetryFact::Unavailable(TelemetryError::InvalidData)
    } else {
        TelemetryFact::Known(PhysicalMemory {
            total_bytes: status.ullTotalPhys,
            available_bytes: status.ullAvailPhys,
        })
    };
    // The SDK's rounded load estimate is separate: bad load never discards bytes.
    let load = match u8::try_from(status.dwMemoryLoad) {
        Ok(value) if value <= 100 => TelemetryFact::Known(value),
        _ => TelemetryFact::Unavailable(TelemetryError::InvalidData),
    };
    (bytes, load)
}
