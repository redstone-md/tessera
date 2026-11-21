// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! What the machine is connected through: its established connections, the
//! VPN connections Windows keeps, and the radios it can switch.

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};
use windows::Networking::Connectivity::{
    ConnectionProfile, NetworkConnectivityLevel, NetworkInformation,
};
use windows::Networking::HostNameType;
use windows::Networking::Vpn::{
    IVpnProfile, VpnManagementAgent, VpnManagementConnectionStatus, VpnManagementErrorStatus,
    VpnNativeProfile, VpnPlugInProfile,
};
use windows::core::{GUID, HSTRING, Interface, Result as WindowsResult};

/// The interface types of the IANA that are told apart.
const IANA_ETHERNET: u32 = 6;
const IANA_WIFI: u32 = 71;

/// What carries a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carrier {
    Cable,
    Wifi,
    /// A mobile network, a tunnel, or anything else.
    Other,
}

/// A connection the machine has established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    /// What Windows calls the network.
    pub name: String,
    pub carrier: Carrier,
    /// Whether the internet is reached, rather than only the local network.
    pub internet: bool,
    /// The IPv4 address of the machine on it, when it has one.
    pub address: Option<String>,
    /// How many bits a second it sends at most, when the adapter says.
    pub speed: Option<u64>,
}

impl Connection {
    /// The established connections, those that reach the internet first.
    pub fn established() -> Result<Vec<Self>, String> {
        let read = || -> WindowsResult<Vec<Self>> {
            let addresses = addresses();
            let mut connections = Vec::new();
            for profile in NetworkInformation::GetConnectionProfiles()? {
                // A profile Windows cannot describe is one that just went.
                if let Ok(Some(connection)) = Self::of(&profile, &addresses) {
                    connections.push(connection);
                }
            }
            connections.sort_by_key(|connection| !connection.internet);
            Ok(connections)
        };
        read().map_err(|error| format!("the connections could not be listed: {error}"))
    }

    fn of(
        profile: &ConnectionProfile,
        addresses: &[(GUID, String)],
    ) -> WindowsResult<Option<Self>> {
        let level = profile.GetNetworkConnectivityLevel()?;
        if level == NetworkConnectivityLevel::None {
            return Ok(None);
        }
        let adapter = profile.NetworkAdapter()?;
        let id = adapter.NetworkAdapterId()?;
        let carrier = match adapter.IanaInterfaceType()? {
            IANA_ETHERNET => Carrier::Cable,
            IANA_WIFI => Carrier::Wifi,
            _ => Carrier::Other,
        };
        let address = addresses.iter().find(|(adapter, _)| *adapter == id);
        Ok(Some(Self {
            name: profile.ProfileName()?.to_string(),
            carrier,
            internet: level == NetworkConnectivityLevel::InternetAccess,
            address: address.map(|(_, address)| address.clone()),
            speed: adapter
                .OutboundMaxBitsPerSecond()
                .ok()
                .filter(|&speed| speed > 0),
        }))
    }
}

/// The IPv4 addresses of the machine, each with the adapter it is on.
fn addresses() -> Vec<(GUID, String)> {
    let Ok(names) = NetworkInformation::GetHostNames() else {
        return Vec::new();
    };
    names
        .into_iter()
        .filter_map(|name| {
            if name.Type().ok()? != HostNameType::Ipv4 {
                return None;
            }
            let adapter = name.IPInformation().ok()?.NetworkAdapter().ok()?;
            Some((
                adapter.NetworkAdapterId().ok()?,
                name.CanonicalName().ok()?.to_string(),
            ))
        })
        .collect()
}

/// Where a VPN connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VpnState {
    Off,
    Connecting,
    On,
    Disconnecting,
}

/// A VPN connection Windows keeps the settings of. One that a program of
/// its own makes, without telling Windows, is not among them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vpn {
    pub name: String,
    pub state: VpnState,
}

impl Vpn {
    /// The VPN connections Windows keeps, by name.
    pub fn kept() -> Result<Vec<Self>, String> {
        let read = || -> WindowsResult<Vec<Self>> {
            let mut kept = Vec::new();
            for profile in VpnManagementAgent::new()?.GetProfilesAsync()?.get()? {
                kept.push(Self {
                    name: profile.ProfileName()?.to_string(),
                    state: state_of(&profile),
                });
            }
            kept.sort_by_key(|vpn| vpn.name.to_lowercase());
            Ok(kept)
        };
        read().map_err(|error| format!("the VPN connections could not be listed: {error}"))
    }

    /// Connects the one called `name` when `on`, and disconnects it
    /// otherwise. Windows uses the credentials it keeps for it.
    pub fn switch(name: &str, on: bool) -> Result<(), String> {
        let switch = || -> WindowsResult<VpnManagementErrorStatus> {
            let agent = VpnManagementAgent::new()?;
            let wanted = HSTRING::from(name);
            let profile = agent
                .GetProfilesAsync()?
                .get()?
                .into_iter()
                .find(|profile| profile.ProfileName().is_ok_and(|name| name == wanted));
            let Some(profile) = profile else {
                return Ok(VpnManagementErrorStatus::CannotFindProfile);
            };
            if on {
                agent.ConnectProfileAsync(&profile)?.get()
            } else {
                agent.DisconnectProfileAsync(&profile)?.get()
            }
        };
        let what = if on { "connected" } else { "disconnected" };
        match switch() {
            Ok(
                VpnManagementErrorStatus::Ok
                | VpnManagementErrorStatus::AlreadyConnected
                | VpnManagementErrorStatus::AlreadyDisconnecting,
            ) => Ok(()),
            Ok(status) => Err(format!(
                "the VPN `{name}` could not be {what}: Windows answered {}",
                status.0
            )),
            Err(error) => Err(format!("the VPN `{name}` could not be {what}: {error}")),
        }
    }
}

/// Where `profile` is; off when it does not say.
fn state_of(profile: &IVpnProfile) -> VpnState {
    let native = profile.cast::<VpnNativeProfile>();
    let status = match native {
        Ok(native) => native.ConnectionStatus(),
        Err(_) => profile
            .cast::<VpnPlugInProfile>()
            .and_then(|plugin| plugin.ConnectionStatus()),
    };
    match status {
        Ok(VpnManagementConnectionStatus::Connected) => VpnState::On,
        Ok(VpnManagementConnectionStatus::Connecting) => VpnState::Connecting,
        Ok(VpnManagementConnectionStatus::Disconnecting) => VpnState::Disconnecting,
        _ => VpnState::Off,
    }
}

/// A radio of the machine that can be switched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wireless {
    Wifi,
    Bluetooth,
}

impl Wireless {
    fn kind(self) -> RadioKind {
        match self {
            Self::Wifi => RadioKind::WiFi,
            Self::Bluetooth => RadioKind::Bluetooth,
        }
    }

    fn radio(self) -> WindowsResult<Option<Radio>> {
        let radios = Radio::GetRadiosAsync()?.get()?;
        Ok(radios
            .into_iter()
            .find(|radio| radio.Kind().is_ok_and(|kind| kind == self.kind())))
    }

    /// Whether the radio is on; `None` on a machine without it.
    pub fn is_on(self) -> Option<bool> {
        let radio = self.radio().ok()??;
        Some(radio.State().ok()? == RadioState::On)
    }

    /// Switches the radio on or off.
    pub fn switch(self, on: bool) -> Result<(), String> {
        let state = if on { RadioState::On } else { RadioState::Off };
        let switch = || -> WindowsResult<Option<RadioAccessStatus>> {
            let Some(radio) = self.radio()? else {
                return Ok(None);
            };
            Radio::RequestAccessAsync()?.get()?;
            radio.SetStateAsync(state)?.get().map(Some)
        };
        match switch() {
            Ok(Some(RadioAccessStatus::Allowed)) => Ok(()),
            Ok(Some(_)) => Err("Windows does not let the radio be switched".to_owned()),
            Ok(None) => Err("the machine has no such radio".to_owned()),
            Err(error) => Err(format!("the radio could not be switched: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads what this machine is connected through; changes nothing.
    #[test]
    #[ignore = "asks the network of the machine"]
    fn the_machine_says_what_it_is_connected_through() {
        let connections = Connection::established().unwrap();
        println!("{} connections", connections.len());
        for connection in &connections {
            println!(
                "{:?} internet={} address={} speed={:?}",
                connection.carrier,
                connection.internet,
                connection.address.is_some(),
                connection.speed
            );
        }
        println!("{} VPN connections kept", Vpn::kept().unwrap().len());
        println!("wifi on: {:?}", Wireless::Wifi.is_on());
        println!("bluetooth on: {:?}", Wireless::Bluetooth.is_on());
    }
}
