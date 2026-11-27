// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! How busy the machine is and how much charge it has left.
//!
//! The graphics processors are read through the performance counters
//! Task Manager reads, `GPU Engine` and `GPU Adapter Memory`, and the size
//! of their own memory through DXGI.

use std::mem::size_of;

use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{
    K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{GetCurrentProcess, GetSystemTimes};
use windows::core::HSTRING;

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

/// The counter of how busy each engine of each graphics processor is, by
/// process; and the one of how much of its own memory each uses.
const ENGINE_COUNTER: &str = "\\GPU Engine(*)\\Utilization Percentage";
const MEMORY_COUNTER: &str = "\\GPU Adapter Memory(*)\\Dedicated Usage";

/// What the graphics processors do at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuUsage {
    /// How busy the busiest engine of any graphics processor is, from 0 to
    /// 1, as Task Manager shows a processor.
    pub busy: f32,
    /// The bytes of its own memory in use on the processor that uses the
    /// most.
    pub used: u64,
    /// How much memory of its own that processor has, where DXGI tells.
    pub total: Option<u64>,
}

/// Measures how busy the graphics processors are between two readings.
/// It may be moved to another thread between readings.
pub struct GpuLoad {
    query: PDH_HQUERY,
    engines: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
    /// The memory of its own each processor has, by its identifier.
    totals: Vec<(Luid, u64)>,
}

// SAFETY: the handles of a query may be used from any thread; `&mut self`
// keeps them to one at a time.
unsafe impl Send for GpuLoad {}

impl GpuLoad {
    /// Starts measuring; `None` where Windows has no such counters, as
    /// before Windows 10 1709 or without a driver that gives them.
    pub fn open() -> Option<Self> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: the handle outlives the call that fills it.
        if unsafe { PdhOpenQueryW(None, 0, &raw mut query) } != ERROR_SUCCESS.0 {
            return None;
        }
        let mut load = Self {
            query,
            engines: PDH_HCOUNTER::default(),
            memory: PDH_HCOUNTER::default(),
            totals: dedicated_memory(),
        };
        let engine_path = HSTRING::from(ENGINE_COUNTER);
        let memory_path = HSTRING::from(MEMORY_COUNTER);
        // SAFETY: the query is open, and the paths and handles outlive the
        // calls; a query that fails is closed when `load` is dropped.
        unsafe {
            if PdhAddEnglishCounterW(load.query, &engine_path, 0, &raw mut load.engines)
                != ERROR_SUCCESS.0
            {
                return None;
            }
            // Without the memory counter the load is still measured.
            let _ = PdhAddEnglishCounterW(load.query, &memory_path, 0, &raw mut load.memory);
            // The first collection is what the next is compared with.
            PdhCollectQueryData(load.query);
        }
        Some(load)
    }

    /// The usage since the last reading; `None` where the counters gave
    /// nothing, as when no process used a graphics processor.
    pub fn read(&mut self) -> Option<GpuUsage> {
        // SAFETY: the query is open for as long as `self` lives.
        if unsafe { PdhCollectQueryData(self.query) } != ERROR_SUCCESS.0 {
            return None;
        }
        let engines = counter_values(self.engines)?;
        let busy = busiest_engine(engines.iter().map(|(name, value)| (name.as_str(), *value)));
        let memory = match self.memory.is_invalid() {
            true => Vec::new(),
            false => counter_values(self.memory).unwrap_or_default(),
        };
        let busiest = { memory.iter() }
            .filter_map(|(name, used)| Some((adapter_of(name)?, *used)))
            .max_by(|(_, one), (_, other)| one.total_cmp(other));
        let (used, total) = match busiest {
            Some((adapter, used)) => (used.max(0.) as u64, self.total_of(adapter)),
            None => (0, None),
        };
        Some(GpuUsage { busy, used, total })
    }

    fn total_of(&self, adapter: Luid) -> Option<u64> {
        { self.totals.iter() }
            .find(|(known, _)| *known == adapter)
            .map(|&(_, total)| total)
    }
}

impl Drop for GpuLoad {
    fn drop(&mut self) {
        // SAFETY: the query was opened by `open` and is closed once, which
        // also removes its counters.
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// The identifier of a graphics processor, as its high and low parts.
type Luid = (u32, u32);

/// The value of each instance of `counter`, by its name, as of the last
/// collection; instances that have no value yet are left out.
fn counter_values(counter: PDH_HCOUNTER) -> Option<Vec<(String, f64)>> {
    let (mut size, mut count) = (0u32, 0u32);
    // SAFETY: asked without a buffer, the call only gives the size the
    // values need.
    let asked = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &raw mut size, &raw mut count, None)
    };
    if asked != PDH_MORE_DATA || size == 0 {
        return None;
    }
    // Eight-byte units keep the values, which hold doubles, aligned.
    let mut buffer = vec![0u64; (size as usize).div_ceil(size_of::<u64>())];
    let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    // SAFETY: the buffer has the size the first call asked for and
    // outlives the call; the items it fills come first in it, followed by
    // the names they point to, and are read while it lives.
    unsafe {
        let filled = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &raw mut size,
            &raw mut count,
            Some(items),
        );
        if filled != ERROR_SUCCESS.0 {
            return None;
        }
        let items = std::slice::from_raw_parts(items, count as usize);
        let values = { items.iter() }
            .filter(|item| {
                matches!(
                    item.FmtValue.CStatus,
                    PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                )
            })
            .map(|item| {
                let name = item.szName.to_string().unwrap_or_default();
                (name, item.FmtValue.Anonymous.doubleValue)
            })
            .collect();
        Some(values)
    }
}

/// The memory of its own each graphics processor has that is no software
/// renderer, by its identifier.
fn dedicated_memory() -> Vec<(Luid, u64)> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
    };
    // SAFETY: the factory and the adapters it gives are released when
    // dropped; the descriptions are copied out.
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
            return Vec::new();
        };
        (0..)
            .map_while(|index| factory.EnumAdapters1(index).ok())
            .filter_map(|adapter| adapter.GetDesc1().ok())
            .filter(|desc| desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
            .map(|desc| {
                let luid = (desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart);
                (luid, desc.DedicatedVideoMemory as u64)
            })
            .collect()
    }
}

/// What follows `word` among the words of an instance name, which
/// `pid_4_luid_0x0_0xC6F4_phys_0_eng_0_engtype_3D` shows.
fn after<'a>(words: &[&'a str], word: &str) -> Option<&'a str> {
    let at = words.iter().position(|known| *known == word)?;
    words.get(at + 1).copied()
}

fn hex(text: &str) -> Option<u32> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    u32::from_str_radix(digits, 16).ok()
}

/// The graphics processor an instance of either counter is of.
fn adapter_of(name: &str) -> Option<Luid> {
    let words: Vec<&str> = name.split('_').collect();
    let at = words.iter().position(|word| *word == "luid")?;
    Some((hex(words.get(at + 1)?)?, hex(words.get(at + 2)?)?))
}

/// The engine an instance of the engine counter is of: its processor, the
/// physical part of it and the number of the engine there. One engine is
/// of one type, so this groups by processor and type of engine as well.
fn engine_of(name: &str) -> Option<(Luid, u32, u32)> {
    let words: Vec<&str> = name.split('_').collect();
    let physical = after(&words, "phys")?.parse().ok()?;
    let engine = after(&words, "eng")?.parse().ok()?;
    Some((adapter_of(name)?, physical, engine))
}

/// How busy the busiest engine is, from 0 to 1, given how much each
/// process keeps each engine busy, in percent.
fn busiest_engine<'a>(instances: impl Iterator<Item = (&'a str, f64)>) -> f32 {
    let mut engines: Vec<((Luid, u32, u32), f64)> = Vec::new();
    for (name, percent) in instances {
        let Some(engine) = engine_of(name) else {
            continue;
        };
        match engines.iter_mut().find(|(known, _)| *known == engine) {
            Some((_, sum)) => *sum += percent,
            None => engines.push((engine, percent)),
        }
    }
    let most = engines.iter().map(|(_, sum)| *sum).fold(0., f64::max);
    (most / 100.).clamp(0., 1.) as f32
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
    fn the_busiest_engine_is_the_sum_of_what_each_process_asks_of_it() {
        let instances = [
            (
                "pid_10_luid_0x00000000_0x0000C6F4_phys_0_eng_0_engtype_3D",
                30.,
            ),
            (
                "pid_11_luid_0x00000000_0x0000C6F4_phys_0_eng_0_engtype_3D",
                25.,
            ),
            (
                "pid_11_luid_0x00000000_0x0000C6F4_phys_0_eng_3_engtype_VideoDecode",
                70.,
            ),
            (
                "pid_12_luid_0x00000000_0x0000D1B5_phys_0_eng_0_engtype_3D",
                40.,
            ),
            ("not an engine", 99.),
        ];
        assert_eq!(busiest_engine(instances.into_iter()), 0.7);
        let busy = [
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 80.),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 60.),
        ];
        assert_eq!(busiest_engine(busy.into_iter()), 1.);
        assert_eq!(busiest_engine(std::iter::empty()), 0.);
    }

    #[test]
    fn instances_are_read_as_the_processor_and_engine_they_are_of() {
        let engine = "pid_4_luid_0x00000001_0x0000C6F4_phys_0_eng_12_engtype_Copy";
        assert_eq!(engine_of(engine), Some(((1, 0xC6F4), 0, 12)));
        let memory = "luid_0x00000000_0x0000d1b5_phys_0";
        assert_eq!(adapter_of(memory), Some((0, 0xD1B5)));
        assert_eq!(adapter_of("luid_0x00000000"), None);
        assert_eq!(engine_of(memory), None);
    }

    #[test]
    fn this_machine_reports_its_memory() {
        let memory = MemoryLoad::read().unwrap();
        assert!(memory.total > 0 && memory.used <= memory.total);
    }
}
