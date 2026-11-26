// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How busy the machine is and how much charge it has left.

use std::mem::size_of;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{GetCurrentProcess, GetSystemTimes};

/// The `BatteryFlag` bit that says the machine has no battery.
const NO_BATTERY: u8 = 128;

/// The value Windows gives for a flag or a charge it does not know.
const UNKNOWN: u8 = 255;

/// Measures how busy the processors are between two readings.
#[derive(Debug, Default)]
pub struct ProcessorLoad {
    /// Busy and total processor time at the last reading.
    previous: Option<(u64, u64)>,
}

impl ProcessorLoad {
    /// The share of processor time spent working since the last reading,
    /// from 0 to 1. The first reading has nothing to compare with.
    pub fn read(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) = Default::default();
        // SAFETY: the three structures outlive the call that fills them.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        // Kernel time includes idle time.
        let total = ticks(kernel) + ticks(user);
        let now = (total.saturating_sub(ticks(idle)), total);
        let previous = self.previous.replace(now)?;
        share(previous, now)
    }
}

/// Measures how many bytes go through the network adapters each second
/// between two readings, over all adapters that are up, but loopback.
#[derive(Debug, Default)]
pub struct NetworkTraffic {
    /// Bytes received and sent in all, and when, at the last reading.
    previous: Option<(u64, u64, std::time::Instant)>,
}

impl NetworkTraffic {
    /// Bytes received and sent per second since the last reading. The
    /// first reading has nothing to compare with.
    pub fn read(&mut self) -> Option<(f64, f64)> {
        let (received, sent) = Self::totals()?;
        let now = std::time::Instant::now();
        let (before_in, before_out, then) = self.previous.replace((received, sent, now))?;
        let seconds = now.duration_since(then).as_secs_f64();
        (seconds > 0.).then(|| {
            (
                received.saturating_sub(before_in) as f64 / seconds,
                sent.saturating_sub(before_out) as f64 / seconds,
            )
        })
    }

    fn totals() -> Option<(u64, u64)> {
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, MIB_IF_TABLE2,
        };
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: Windows allocates the table, which is read while it
        // lives and freed once; its rows are as many as it says.
        unsafe {
            GetIfTable2(&raw mut table).ok().ok()?;
            let count = (*table).NumEntries as usize;
            let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), count);
            let totals = rows
                .iter()
                .filter(|row| {
                    row.OperStatus == IfOperStatusUp && row.Type != IF_TYPE_SOFTWARE_LOOPBACK
                })
                // Filters stacked on an adapter count its bytes again.
                .filter(|row| row.InterfaceAndOperStatusFlags._bitfield & 1 != 0)
                .fold((0u64, 0u64), |(received, sent), row| {
                    (received + row.InOctets, sent + row.OutOctets)
                });
            FreeMibTable(table.cast());
            Some(totals)
        }
    }
}

/// The bytes of memory that are this process's alone, as Windows counts
/// them. `None` where Windows does not tell.
pub fn memory_held() -> Option<u64> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: `counters` is of the size it says and outlives the call; the
    // handle of the process itself needs no closing.
    let told = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    told.as_bool().then_some(counters.PrivateUsage as u64)
}

/// The busy share of the time between two `(busy, total)` readings.
fn share(previous: (u64, u64), now: (u64, u64)) -> Option<f32> {
    let busy = now.0.checked_sub(previous.0)?;
    let total = now.1.checked_sub(previous.1)?;
    (total > 0).then(|| (busy as f32 / total as f32).clamp(0., 1.))
}

fn ticks(time: FILETIME) -> u64 {
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}

/// Physical memory, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryLoad {
    pub used: u64,
    pub total: u64,
}

impl MemoryLoad {
    /// How much physical memory is in use now.
    pub fn read() -> Option<Self> {
        let mut status = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a correctly sized structure that outlives
        // the call that fills it.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(Self {
            used: status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            total: status.ullTotalPhys,
        })
    }
}

/// The battery of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    /// The charge left, from 0 to 100.
    pub percent: u8,
    /// The machine runs on mains power.
    pub plugged_in: bool,
}

impl Battery {
    /// The state of the battery now; `None` if the machine has none or
    /// Windows does not know its charge.
    pub fn read() -> Option<Self> {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` outlives the call that fills it.
        unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
        Self::of(
            status.BatteryFlag,
            status.BatteryLifePercent,
            status.ACLineStatus,
        )
    }

    fn of(flag: u8, percent: u8, line: u8) -> Option<Self> {
        if flag == UNKNOWN || flag & NO_BATTERY != 0 || percent > 100 {
            return None;
        }
        Some(Self {
            percent,
            plugged_in: line == 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_is_the_busy_share_of_the_time_between_readings() {
        assert_eq!(share((100, 400), (150, 600)), Some(0.25));
        assert_eq!(share((100, 400), (100, 400)), None);
        assert_eq!(share((100, 400), (90, 500)), None);
    }

    #[test]
    fn a_machine_without_a_battery_has_no_charge() {
        assert_eq!(Battery::of(NO_BATTERY, 255, 1), None);
        assert_eq!(Battery::of(UNKNOWN, 50, 1), None);
        assert_eq!(Battery::of(0, 255, 0), None);
        let battery = Battery::of(8, 62, 1).unwrap();
        assert_eq!((battery.percent, battery.plugged_in), (62, true));
        assert!(!Battery::of(0, 62, 0).unwrap().plugged_in);
    }

    #[test]
    fn this_machine_reports_its_memory() {
        let memory = MemoryLoad::read().unwrap();
        assert!(memory.total > 0 && memory.used <= memory.total);
    }
}
