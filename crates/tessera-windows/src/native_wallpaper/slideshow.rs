// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! One bounded current-policy ledger on the existing STA. No folder enumeration,
//! persistent file leases, null monitor dispatch or rendered-pixel inference.

use std::sync::Arc;

use tessera_system::wallpaper::{
    WallpaperError, WallpaperMonitorTarget, collection as collection_contract,
    slideshow as contract,
};
use windows::Win32::UI::Shell::{DSD_BACKWARD, DSD_FORWARD, IDesktopWallpaper, IShellItemArray};

use super::{Mailbox, collection, displays, retirement_callback, source};

const MAX_ENTRIES: u32 = 32;

pub(super) struct Issuer {
    target: contract::TargetWeak,
    monitors: Vec<WallpaperMonitorTarget>,
}

impl Issuer {
    pub(super) fn accepts_policy(&self, target: &contract::Target) -> bool {
        self.target.matches(target)
    }

    pub(super) fn accepts(
        &self,
        target: &contract::Target,
        monitor: &WallpaperMonitorTarget,
    ) -> bool {
        self.accepts_policy(target) && self.monitors.contains(monitor)
    }
}

#[derive(Default)]
pub(super) struct Ledger {
    snapshot: Option<Snapshot>,
}

impl Ledger {
    pub(super) fn clear(&mut self) {
        // All native COM resources drop here on their existing owner STA.
        self.snapshot = None;
    }

    pub(super) fn retire(&mut self) -> bool {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| !snapshot.target.is_alive())
        {
            self.clear();
            true
        } else {
            false
        }
    }

    pub(super) fn issuer(&self) -> Option<Issuer> {
        let snapshot = self.snapshot.as_ref()?;
        Some(Issuer {
            target: snapshot.target.clone(),
            monitors: snapshot
                .bindings
                .iter()
                .map(|binding| binding.target.clone())
                .collect(),
        })
    }

    pub(super) fn read(
        &mut self,
        mailbox: &Arc<Mailbox>,
    ) -> Result<contract::Observation, WallpaperError> {
        self.clear();
        let desktop = displays::desktop()?;
        self.capture(&desktop, mailbox)
    }

    fn capture(
        &mut self,
        desktop: &IDesktopWallpaper,
        mailbox: &Arc<Mailbox>,
    ) -> Result<contract::Observation, WallpaperError> {
        // Known facts remain readable even if this array shape, a file identity,
        // options or monitor cohort cannot safely authorize an action.
        let state = collection::read_state(desktop).ok_or(WallpaperError::Unavailable)?;
        let options = collection::read_options(desktop);
        let topology = displays::Topology::capture(desktop).ok();
        let array = unsafe { desktop.GetSlideshow() }.ok();
        let native_entry_count = array
            .as_ref()
            .and_then(|array| unsafe { array.GetCount() }.ok());
        let policy = array.and_then(|array| Policy::capture(array, native_entry_count?).ok());
        let (bindings, monitors) = topology
            .as_ref()
            .and_then(|topology| topology.issue_members().ok())
            .unwrap_or_default();
        let mut observation = contract::Observation {
            target: None,
            monitors,
            state,
            options,
            native_entry_count,
        };
        if state.enabled
            && state.slideshow
            && !state.disabled_by_remote_session
            && let (Some(policy), Some(topology), Some(options)) = (policy, topology, options)
            && !bindings.is_empty()
            && policy.is_current()
            && topology.matches(desktop)
            && collection::read_state(desktop) == Some(state)
            && collection::read_options(desktop) == Some(options)
            && policy.matches_desktop(desktop)
        {
            let target = contract::Target::with_retirement(retirement_callback(mailbox));
            self.snapshot = Some(Snapshot {
                target: target.downgrade(),
                policy,
                topology,
                bindings,
                state,
                options,
            });
            observation.target = Some(target);
        }
        Ok(observation)
    }

    fn take_snapshot(&mut self, target: &contract::Target) -> Result<Snapshot, WallpaperError> {
        let snapshot = self.snapshot.take().ok_or(WallpaperError::InvalidTarget)?;
        if !snapshot.target.matches(target) {
            return Err(WallpaperError::InvalidTarget);
        }
        Ok(snapshot)
    }

    pub(super) fn set_options(
        &mut self,
        target: contract::Target,
        options: collection_contract::Options,
        mailbox: &Arc<Mailbox>,
    ) -> Result<contract::OptionsOutcome, WallpaperError> {
        // Exact original policy authority is consumed before the COM factory.
        // This global operation neither needs nor carries a monitor member.
        let snapshot = self.take_snapshot(&target)?;
        let desktop = displays::desktop()?;
        if !snapshot.is_current(&desktop) {
            return Err(WallpaperError::InvalidTarget);
        }
        let disposition = if snapshot.options.interval_ms == options.interval.milliseconds()
            && snapshot.options.shuffle == options.shuffle
        {
            contract::OptionsDisposition::AlreadyCurrent
        } else if collection::set_options(&desktop, options).is_ok() {
            contract::OptionsDisposition::Accepted
        } else {
            contract::OptionsDisposition::Rejected
        };
        // Also read after rejection/no-op. Readback loss cannot erase a receipt;
        // fresh authority uses the actual new policy/cohort, never old UI choice.
        let observation = self.capture(&desktop, mailbox).ok();
        let outcome = contract::OptionsOutcome {
            disposition,
            observation,
        };
        drop(desktop);
        drop(snapshot);
        drop(target);
        Ok(outcome)
    }

    pub(super) fn advance(
        &mut self,
        target: contract::Target,
        monitor: WallpaperMonitorTarget,
        direction: contract::Direction,
        mailbox: &Arc<Mailbox>,
    ) -> Result<contract::AdvanceOutcome, WallpaperError> {
        let snapshot = self.take_snapshot(&target)?;
        // Exact original member resolution precedes desktop factory/SDK work.
        let selected = snapshot
            .bindings
            .iter()
            .find(|binding| binding.target == monitor)
            .ok_or(WallpaperError::InvalidTarget)?;
        let desktop = displays::desktop()?;
        let before = unsafe { desktop.GetWallpaper(selected.monitor.id()) }
            .ok()
            .and_then(|path| source::PolicyEntry::wallpaper(path).ok());
        // Optional wallpaper read must not separate revalidation from mutation.
        // Recheck original array/items, whole cohort, policy, options and status.
        if !snapshot.is_current(&desktop) {
            return Err(WallpaperError::InvalidTarget);
        }
        let native_direction = match direction {
            contract::Direction::Forward => DSD_FORWARD,
            contract::Direction::Backward => DSD_BACKWARD,
        };
        // SAFETY: nonempty actual original GetMonitorDevicePathAt ID. NULL would
        // mean the monitor scheduled next, so it is never used as a fallback.
        let receipt =
            if unsafe { desktop.AdvanceSlideshow(selected.monitor.id(), native_direction) }.is_ok()
            {
                collection_contract::NativeStep::Accepted
            } else {
                collection_contract::NativeStep::Rejected
            };
        // Independent readbacks also run after rejection. After-setter failures
        // never turn the known receipt into an error implying rollback.
        let after = unsafe { desktop.GetWallpaper(selected.monitor.id()) }
            .ok()
            .and_then(|path| source::PolicyEntry::wallpaper(path).ok());
        let changed = before.zip(after).map(|(before, after)| before != after);
        let observation = self.capture(&desktop, mailbox).ok();
        let selected_monitor = self.snapshot.as_ref().and_then(|fresh| {
            fresh
                .bindings
                .iter()
                .find(|binding| binding.monitor.same_device(&selected.monitor))
                .map(|binding| binding.target.clone())
        });
        let outcome = contract::AdvanceOutcome {
            receipt,
            changed,
            observation,
            selected_monitor,
        };
        // The accepted exact strong ticket spans original COM resources and all
        // native writes/readbacks, independently of any UI Root/provider.
        drop(desktop);
        drop(snapshot);
        drop(target);
        Ok(outcome)
    }
}

struct Snapshot {
    target: contract::TargetWeak,
    policy: Policy,
    topology: displays::Topology,
    bindings: Vec<displays::Binding>,
    state: collection_contract::StateFacts,
    options: collection_contract::OptionsReadback,
}

impl Snapshot {
    fn is_current(&self, desktop: &IDesktopWallpaper) -> bool {
        self.policy.is_current()
            && self.policy.matches_desktop(desktop)
            && collection::read_options(desktop) == Some(self.options)
            && collection::read_state(desktop) == Some(self.state)
            && self.topology.matches(desktop)
    }
}

struct Policy {
    array: IShellItemArray,
    entries: Vec<source::PolicyEntry>,
    parent: Option<source::PolicyEntry>,
}

impl Policy {
    fn capture(array: IShellItemArray, count: u32) -> Result<Self, WallpaperError> {
        if !(1..=MAX_ENTRIES).contains(&count) {
            return Err(WallpaperError::Unavailable);
        }
        let mut entries: Vec<source::PolicyEntry> = Vec::with_capacity(count as usize);
        let mut parent = None;
        for index in 0..count {
            let item =
                unsafe { array.GetItemAt(index) }.map_err(|_| WallpaperError::Unavailable)?;
            let entry = source::PolicyEntry::capture(item)?;
            if entry.is_directory() {
                // Exactly one native folder member; never enumerate its images.
                if count != 1 {
                    return Err(WallpaperError::Unavailable);
                }
            } else {
                if entries.iter().any(|original| original.same_object(&entry)) {
                    return Err(WallpaperError::Unavailable);
                }
                if let Some(parent) = &parent {
                    if !entry.is_in_container(parent)? {
                        return Err(WallpaperError::Unavailable);
                    }
                } else {
                    parent = Some(entry.parent()?);
                }
            }
            entries.push(entry);
        }
        if unsafe { array.GetCount() }.map_err(|_| WallpaperError::Unavailable)? != count {
            return Err(WallpaperError::Unavailable);
        }
        Ok(Self {
            array,
            entries,
            parent,
        })
    }

    fn same_policy(&self, fresh: &Self) -> bool {
        self.entries.len() == fresh.entries.len()
            && self
                .entries
                .iter()
                .zip(&fresh.entries)
                .all(|(original, fresh)| original.same_identity(fresh))
            && match (&self.parent, &fresh.parent) {
                (None, None) => true,
                (Some(original), Some(fresh)) => original.same_identity(fresh),
                _ => false,
            }
    }

    fn is_current(&self) -> bool {
        if !self.entries.iter().all(source::PolicyEntry::is_current)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| !parent.is_current())
        {
            return false;
        }
        // Retained original SDK array/order must still resolve the exact IDs.
        Self::capture(self.array.clone(), self.entries.len() as u32)
            .is_ok_and(|fresh| self.same_policy(&fresh))
    }

    fn matches_desktop(&self, desktop: &IDesktopWallpaper) -> bool {
        let Ok(array) = (unsafe { desktop.GetSlideshow() }) else {
            return false;
        };
        let Ok(count) = (unsafe { array.GetCount() }) else {
            return false;
        };
        // A count mismatch needs no traversal, including arbitrary large u32s.
        count as usize == self.entries.len()
            && Self::capture(array, count).is_ok_and(|fresh| self.same_policy(&fresh))
    }
}
