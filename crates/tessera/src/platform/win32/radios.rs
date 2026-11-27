// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Every radio of the machine, through Windows.Devices.Radios: what each
//! is and whether it is on, switching one, and airplane mode as that API
//! allows it, by switching every radio off and later those back on that
//! were on before. The airplane mode flag of Windows itself, which only
//! its own quick settings set, is not touched.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::core::Result as WindowsResult;

/// What a radio is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RadioType {
    Wifi,
    Bluetooth,
    MobileBroadband,
    Fm,
    Other,
}

impl RadioType {
    fn of(kind: RadioKind) -> Self {
        match kind {
            RadioKind::WiFi => Self::Wifi,
            RadioKind::Bluetooth => Self::Bluetooth,
            RadioKind::MobileBroadband => Self::MobileBroadband,
            RadioKind::FM => Self::Fm,
            _ => Self::Other,
        }
    }
}

/// What tells a radio from the others: Windows gives radios no lasting
/// identifier, so it is their kind and their name.
pub type RadioId = (RadioType, String);

/// One radio, as it was when it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioInfo {
    pub kind: RadioType,
    /// What the driver calls it, such as "Wi-Fi" or "Bluetooth".
    pub name: String,
    pub on: bool,
    /// Whether it can be switched; a radio switched off by a key or a
    /// switch of the machine itself cannot.
    pub switchable: bool,
}

impl RadioInfo {
    pub fn id(&self) -> RadioId {
        (self.kind, self.name.clone())
    }
}

/// Whether `radios` are in airplane mode: there is one, and none is on.
pub fn airplane_mode(radios: &[RadioInfo]) -> bool {
    !radios.is_empty() && radios.iter().all(|radio| !radio.on)
}

/// The radios of the machine.
pub struct Radios;

impl Radios {
    /// Every radio, in the order Windows gives them.
    pub fn all() -> Result<Vec<RadioInfo>, String> {
        let radios = listed().map_err(|error| format!("the radios cannot be read: {error}"))?;
        Ok(radios
            .iter()
            .filter_map(|radio| {
                let state = radio.State().ok()?;
                Some(RadioInfo {
                    kind: RadioType::of(radio.Kind().ok()?),
                    name: radio
                        .Name()
                        .map(|name| name.to_string())
                        .unwrap_or_default(),
                    on: state == RadioState::On,
                    switchable: state != RadioState::Disabled,
                })
            })
            .collect())
    }

    /// Switches the radios of `kind` on or off; with `name`, only the one
    /// so called.
    pub fn switch(kind: RadioType, name: Option<&str>, on: bool) -> Result<(), String> {
        let chosen = |radio: &Radio| {
            radio.Kind().is_ok_and(|found| RadioType::of(found) == kind)
                && name.is_none_or(|name| radio.Name().is_ok_and(|found| found == name))
        };
        let switched = set_where(chosen, on)?;
        match switched {
            0 => Err("the machine has no such radio".to_owned()),
            _ => Ok(()),
        }
    }

    /// Switches every radio off, and gives those that were on, to be
    /// switched back on afterwards.
    pub fn airplane_on() -> Result<Vec<RadioId>, String> {
        let were_on: Vec<RadioId> = Self::all()?
            .into_iter()
            .filter(|radio| radio.on)
            .map(|radio| radio.id())
            .collect();
        set_where(|_| true, false)?;
        Ok(were_on)
    }

    /// Switches the radios of `were_on` back on, or every radio where it is
    /// not known which were on.
    pub fn airplane_off(were_on: Option<&[RadioId]>) -> Result<(), String> {
        let chosen = |radio: &Radio| {
            let Some(were_on) = were_on else {
                return true;
            };
            let (Ok(kind), Ok(name)) = (radio.Kind(), radio.Name()) else {
                return false;
            };
            let id = (RadioType::of(kind), name.to_string());
            were_on.contains(&id)
        };
        set_where(chosen, true).map(|_| ())
    }
}

fn listed() -> WindowsResult<Vec<Radio>> {
    Ok(Radio::GetRadiosAsync()?.get()?.into_iter().collect())
}

/// Switches each radio `chosen` takes on or off and says how many there
/// were. Each radio is switched even when another refused; the first
/// refusal is what is reported.
fn set_where(chosen: impl Fn(&Radio) -> bool, on: bool) -> Result<usize, String> {
    let radios = listed().map_err(|error| format!("the radios cannot be read: {error}"))?;
    let radios: Vec<Radio> = radios.into_iter().filter(|radio| chosen(radio)).collect();
    if radios.is_empty() {
        return Ok(0);
    }
    let access = Radio::RequestAccessAsync().and_then(|asked| asked.get());
    if !matches!(access, Ok(RadioAccessStatus::Allowed)) {
        return Err("Windows does not let Tessera switch the radios".to_owned());
    }
    let state = if on { RadioState::On } else { RadioState::Off };
    let mut refusal = None;
    for radio in &radios {
        // A radio switched off by the machine itself stays as it is.
        if radio
            .State()
            .is_ok_and(|state| state == RadioState::Disabled)
        {
            continue;
        }
        let set = radio.SetStateAsync(state).and_then(|asked| asked.get());
        let failed = match set {
            Ok(RadioAccessStatus::Allowed) => continue,
            Ok(_) => "Windows refused".to_owned(),
            Err(error) => error.to_string(),
        };
        let name = radio
            .Name()
            .map(|name| name.to_string())
            .unwrap_or_default();
        refusal.get_or_insert(format!("the radio {name} was not switched: {failed}"));
    }
    match refusal {
        Some(refusal) => Err(refusal),
        None => Ok(radios.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn radio(kind: RadioType, on: bool) -> RadioInfo {
        RadioInfo {
            kind,
            name: format!("{kind:?}"),
            on,
            switchable: true,
        }
    }

    #[test]
    fn airplane_mode_is_every_radio_off() {
        assert!(!airplane_mode(&[]));
        let wifi = radio(RadioType::Wifi, false);
        assert!(airplane_mode(std::slice::from_ref(&wifi)));
        assert!(airplane_mode(&[
            wifi.clone(),
            radio(RadioType::Bluetooth, false)
        ]));
        assert!(!airplane_mode(&[wifi, radio(RadioType::Bluetooth, true)]));
    }

    /// Reads the radios of the machine it runs on; switches nothing.
    #[test]
    #[ignore = "asks the radios of this machine"]
    fn the_machine_lists_its_radios() {
        for radio in Radios::all().unwrap() {
            println!("{radio:?}");
        }
    }
}
