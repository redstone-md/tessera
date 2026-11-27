// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The power plans of Windows: which there are, which one is in use, and
//! putting another one in use.

use std::io;

use windows::Win32::Foundation::{ERROR_SUCCESS, HLOCAL, LocalFree};
use windows::Win32::System::Power::{
    ACCESS_SCHEME, PowerEnumerate, PowerGetActiveScheme, PowerReadFriendlyName,
    PowerSetActiveScheme,
};
use windows::core::GUID;

/// The most plans that are asked for.
const MOST_PLANS: u32 = 32;

/// A power plan, as the Control Panel lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerPlan {
    id: GUID,
    /// What Windows calls it, in the language of Windows.
    pub name: String,
    /// Whether it is the one in use.
    pub active: bool,
}

impl PowerPlan {
    /// The plans of this computer in the order Windows gives them; none
    /// where Windows tells of none.
    pub fn all() -> Vec<Self> {
        let active = active();
        (0..MOST_PLANS)
            .map_while(plan_at)
            .map(|id| Self {
                name: name_of(&id).unwrap_or_default(),
                active: active == Some(id),
                id,
            })
            .collect()
    }

    /// Whether both are the same plan, whichever is in use.
    pub fn is(&self, other: &Self) -> bool {
        self.id == other.id
    }

    /// Marks `chosen` as the one in use among `plans`, and no other, as
    /// they are once Windows has put it in use.
    pub fn mark_in_use(plans: &mut [Self], chosen: &Self) {
        for plan in plans {
            plan.active = plan.is(chosen);
        }
    }

    /// Puts the plan in use. It takes a moment: Windows tells every
    /// program that asked to know.
    pub fn activate(&self) -> io::Result<()> {
        // SAFETY: the identifier outlives the call.
        let result = unsafe { PowerSetActiveScheme(None, Some(&raw const self.id)) };
        match result {
            ERROR_SUCCESS => Ok(()),
            error => Err(io::Error::from_raw_os_error(error.0 as i32)),
        }
    }
}

/// The identifier of the plan with the number `index`.
fn plan_at(index: u32) -> Option<GUID> {
    let mut id = GUID::zeroed();
    let mut size = size_of::<GUID>() as u32;
    // SAFETY: the buffer is as large as `size` says and outlives the call.
    let result = unsafe {
        PowerEnumerate(
            None,
            None,
            None,
            ACCESS_SCHEME,
            index,
            Some((&raw mut id).cast()),
            &raw mut size,
        )
    };
    (result == ERROR_SUCCESS).then_some(id)
}

/// The identifier of the plan in use.
fn active() -> Option<GUID> {
    let mut found: *mut GUID = std::ptr::null_mut();
    // SAFETY: Windows allocates the identifier, which is copied and then
    // freed the way the function asks for.
    unsafe {
        if PowerGetActiveScheme(None, &raw mut found) != ERROR_SUCCESS || found.is_null() {
            return None;
        }
        let id = *found;
        let _ = LocalFree(Some(HLOCAL(found.cast())));
        Some(id)
    }
}

/// What Windows calls the plan `id`.
fn name_of(id: &GUID) -> Option<String> {
    let mut size = 0u32;
    // SAFETY: the first call asks for the size alone; the second is given
    // a buffer of that many bytes.
    let name = unsafe {
        let _ = PowerReadFriendlyName(None, Some(id), None, None, None, &raw mut size);
        if size == 0 {
            return None;
        }
        let mut bytes = vec![0u8; size as usize];
        let result = PowerReadFriendlyName(
            None,
            Some(id),
            None,
            None,
            Some(bytes.as_mut_ptr()),
            &raw mut size,
        );
        if result != ERROR_SUCCESS {
            return None;
        }
        bytes
    };
    Some(name_in(&name))
}

/// The text in `bytes`, which hold UTF-16 that may end with a NUL.
fn name_in(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_ends_at_its_nul() {
        let bytes: Vec<u8> = "Balanced\0x"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(name_in(&bytes), "Balanced");
        assert_eq!(name_in(&[]), "");
    }

    #[test]
    fn one_plan_at_a_time_is_in_use() {
        let plan = |number: u128, active| PowerPlan {
            id: GUID::from_u128(number),
            name: format!("plan {number}"),
            active,
        };
        let mut plans = vec![plan(1, true), plan(2, false), plan(3, false)];
        PowerPlan::mark_in_use(&mut plans, &plan(3, false));
        let active: Vec<bool> = plans.iter().map(|plan| plan.active).collect();
        assert_eq!(active, [false, false, true]);
        assert!(plans[0].is(&plan(1, false)));
        assert!(!plans[0].is(&plans[1]));
    }

    /// Reads the plans of this computer; changes nothing.
    #[test]
    fn the_plans_of_this_computer_are_read() {
        let plans = PowerPlan::all();
        assert!(plans.iter().filter(|plan| plan.active).count() <= 1);
        assert!(plans.iter().all(|plan| !plan.name.is_empty()));
    }
}
