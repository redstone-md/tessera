// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The mobile hotspot of Windows: sharing the connection the machine
//! reaches the internet through over Wi-Fi, through the tethering manager
//! of Windows.Networking.NetworkOperators. Starting and stopping it, the
//! name and password of its network, its band and how many devices are
//! on it. Every call waits for Windows, so none is made on the thread of
//! the interface. Why the machine has no hotspot is said in a fixed text,
//! which the interface can translate.

use windows::Networking::Connectivity::NetworkInformation;
use windows::Networking::NetworkOperators::{
    NetworkOperatorTetheringManager, TetheringCapability, TetheringOperationStatus,
    TetheringOperationalState, TetheringWiFiBand,
};

/// Whether the hotspot shares the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotspotState {
    On,
    Off,
    /// Starting or stopping.
    Changing,
}

/// The band the network of the hotspot is sent on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotspotBand {
    /// Whichever Windows chooses.
    Any,
    Ghz2,
    Ghz5,
    Ghz6,
}

impl HotspotBand {
    /// Every band, in the order they are offered.
    pub const ALL: [Self; 4] = [Self::Any, Self::Ghz2, Self::Ghz5, Self::Ghz6];

    fn of(band: TetheringWiFiBand) -> Option<Self> {
        match band {
            TetheringWiFiBand::Auto => Some(Self::Any),
            TetheringWiFiBand::TwoPointFourGigahertz => Some(Self::Ghz2),
            TetheringWiFiBand::FiveGigahertz => Some(Self::Ghz5),
            TetheringWiFiBand::SixGigahertz => Some(Self::Ghz6),
            _ => None,
        }
    }

    fn native(self) -> TetheringWiFiBand {
        match self {
            Self::Any => TetheringWiFiBand::Auto,
            Self::Ghz2 => TetheringWiFiBand::TwoPointFourGigahertz,
            Self::Ghz5 => TetheringWiFiBand::FiveGigahertz,
            Self::Ghz6 => TetheringWiFiBand::SixGigahertz,
        }
    }
}

/// The hotspot as it was when it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotspotInfo {
    pub state: HotspotState,
    /// The name of its network.
    pub ssid: String,
    /// The password of its network.
    pub passphrase: String,
    /// The band it is sent on, where Windows says.
    pub band: Option<HotspotBand>,
    /// The bands the adapter can send it on; empty where Windows does not
    /// say, which older versions of it do not.
    pub bands: Vec<HotspotBand>,
    /// How many devices are joined, and how many may be.
    pub clients: u32,
    pub most_clients: u32,
}

/// The mobile hotspot of the machine.
pub struct Hotspot;

impl Hotspot {
    /// What the hotspot is now, or why the machine cannot have one.
    pub fn read() -> Result<HotspotInfo, &'static str> {
        let manager = manager()?;
        let state = match manager.TetheringOperationalState() {
            Ok(TetheringOperationalState::On) => HotspotState::On,
            Ok(TetheringOperationalState::InTransition) => HotspotState::Changing,
            _ => HotspotState::Off,
        };
        let failed = |_| SILENT;
        let configuration = manager
            .GetCurrentAccessPointConfiguration()
            .map_err(failed)?;
        let bands = HotspotBand::ALL
            .into_iter()
            .filter(|band| {
                configuration
                    .IsBandSupported(band.native())
                    .unwrap_or(false)
            })
            .collect();
        Ok(HotspotInfo {
            state,
            ssid: configuration.Ssid().map_err(failed)?.to_string(),
            passphrase: configuration.Passphrase().map_err(failed)?.to_string(),
            band: configuration.Band().ok().and_then(HotspotBand::of),
            bands,
            clients: manager.ClientCount().unwrap_or(0),
            most_clients: manager.MaxClientCount().unwrap_or(0),
        })
    }

    /// Starts sharing the connection, or stops.
    pub fn switch(on: bool) -> Result<(), String> {
        let manager =
            manager().map_err(|reason| format!("the hotspot was not switched: {reason}"))?;
        let asked = match on {
            true => manager.StartTetheringAsync(),
            false => manager.StopTetheringAsync(),
        };
        let result = asked
            .and_then(|asked| asked.get())
            .map_err(|error| format!("the hotspot was not switched: {error}"))?;
        let status = result.Status().unwrap_or(TetheringOperationStatus::Unknown);
        if status == TetheringOperationStatus::Success
            || (on && status == TetheringOperationStatus::AlreadyOn)
        {
            return Ok(());
        }
        let more = result
            .AdditionalErrorMessage()
            .map(|more| more.to_string())
            .unwrap_or_default();
        let reason = refusal(status);
        Err(match more.is_empty() {
            true => format!("the hotspot was not switched: {reason}"),
            false => format!("the hotspot was not switched: {reason} ({more})"),
        })
    }

    /// Sends the network on `band` from now on. A hotspot that is on is
    /// started again for it.
    pub fn set_band(band: HotspotBand) -> Result<(), String> {
        let manager = manager().map_err(|reason| format!("the band was not changed: {reason}"))?;
        let failed = |error: windows::core::Error| format!("the band was not changed: {error}");
        let configuration = manager
            .GetCurrentAccessPointConfiguration()
            .map_err(failed)?;
        configuration.SetBand(band.native()).map_err(failed)?;
        let asked = manager.ConfigureAccessPointAsync(&configuration);
        asked.and_then(|asked| asked.get()).map_err(failed)?;
        let on = manager
            .TetheringOperationalState()
            .is_ok_and(|state| state == TetheringOperationalState::On);
        if on {
            Self::switch(false)?;
            Self::switch(true)?;
        }
        Ok(())
    }
}

/// What is said where Windows gives no answer about the hotspot.
const SILENT: &str = "Windows does not say what the hotspot is";

/// The tethering manager of the connection the machine reaches the
/// internet through.
fn manager() -> Result<NetworkOperatorTetheringManager, &'static str> {
    let profile = NetworkInformation::GetInternetConnectionProfile()
        .map_err(|_| "There is no internet connection to share")?;
    let capability =
        NetworkOperatorTetheringManager::GetTetheringCapabilityFromConnectionProfile(&profile)
            .map_err(|_| SILENT)?;
    if let Some(reason) = incapability(capability) {
        return Err(reason);
    }
    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile).map_err(|_| SILENT)
}

/// Why the machine cannot share its connection, as a sentence; `None`
/// where it can.
fn incapability(capability: TetheringCapability) -> Option<&'static str> {
    Some(match capability {
        TetheringCapability::Enabled => return None,
        TetheringCapability::DisabledByGroupPolicy => "A group policy forbids the hotspot",
        TetheringCapability::DisabledByHardwareLimitation => {
            "The Wi-Fi adapter cannot be a hotspot"
        }
        TetheringCapability::DisabledByOperator => "The mobile operator forbids the hotspot",
        TetheringCapability::DisabledBySku => "This edition of Windows has no hotspot",
        TetheringCapability::DisabledByRequiredAppNotInstalled => {
            "The application of the mobile operator is missing"
        }
        TetheringCapability::DisabledBySystemCapability => "Windows does not allow the hotspot",
        _ => "The hotspot is not available",
    })
}

/// Why Windows did not switch the hotspot.
fn refusal(status: TetheringOperationStatus) -> &'static str {
    match status {
        TetheringOperationStatus::MobileBroadbandDeviceOff => "the mobile radio is off",
        TetheringOperationStatus::WiFiDeviceOff => "Wi-Fi is off",
        TetheringOperationStatus::BluetoothDeviceOff => "Bluetooth is off",
        TetheringOperationStatus::EntitlementCheckTimeout
        | TetheringOperationStatus::EntitlementCheckFailure => {
            "the mobile operator did not allow it"
        }
        TetheringOperationStatus::OperationInProgress => "it is already being switched",
        TetheringOperationStatus::NetworkLimitedConnectivity => {
            "the connection reaches no internet"
        }
        TetheringOperationStatus::RadioRestriction => "a radio restriction forbids it",
        TetheringOperationStatus::BandInterference => "the band is disturbed",
        _ => "Windows gave no reason",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_band_goes_to_windows_and_back() {
        for band in HotspotBand::ALL {
            assert_eq!(HotspotBand::of(band.native()), Some(band));
        }
    }

    #[test]
    fn a_machine_that_can_share_has_no_reason_not_to() {
        assert_eq!(incapability(TetheringCapability::Enabled), None);
        assert!(incapability(TetheringCapability::DisabledBySku).is_some());
    }

    /// Reads the hotspot of the machine it runs on; switches nothing.
    #[test]
    #[ignore = "asks the hotspot of this machine"]
    fn the_machine_says_what_its_hotspot_is() {
        println!("{:?}", Hotspot::read().map(|info| (info.state, info.band)));
    }
}
