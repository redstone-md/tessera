// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Wi-Fi networks in reach, as Windows last saw them, and joining one
//! Windows has the settings of.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_NDIS_DOT11_POWER_STATE_INVALID, HANDLE,
};
use windows::Win32::NetworkManagement::WiFi::{
    WLAN_AVAILABLE_NETWORK, WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_HAS_PROFILE,
    WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST,
    WlanCloseHandle, WlanConnect, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList,
    WlanOpenHandle, WlanScan, dot11_BSS_type_any, wlan_connection_mode_profile,
};
use windows::core::{GUID, HSTRING, PCWSTR};

/// The version of the interface asked for: the one since Windows Vista.
const CLIENT_VERSION: u32 = 2;

/// A Wi-Fi network in reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub name: String,
    /// The signal, from 0 to 5 bars.
    pub bars: u8,
    /// Whether joining it takes a password or the like.
    pub secured: bool,
    pub connected: bool,
    /// The settings Windows saved for it, by their name; with them it can
    /// be joined without asking for anything.
    pub profile: Option<String>,
}

/// What the Wi-Fi adapters of the machine reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiReach {
    /// The machine has no Wi-Fi adapter.
    NoAdapter,
    /// Wi-Fi is switched off.
    Off,
    /// The networks seen when the adapters last looked, the connected one
    /// first, then the stronger before the weaker. Networks that hide
    /// their name are left out.
    Networks(Vec<WifiNetwork>),
}

/// The Wi-Fi adapters of the machine.
pub struct Wifi;

impl Wifi {
    /// What the adapters reach now.
    pub fn reach() -> Result<WifiReach, String> {
        let client = Client::open()?;
        let adapters = client.adapters()?;
        if adapters.is_empty() {
            return Ok(WifiReach::NoAdapter);
        }
        let mut seen = Vec::new();
        let mut on = false;
        for adapter in adapters {
            if let Some(networks) = client.networks(&adapter)? {
                on = true;
                seen.extend(networks);
            }
        }
        Ok(if on {
            WifiReach::Networks(arranged(seen))
        } else {
            WifiReach::Off
        })
    }

    /// Asks every adapter to look for networks anew. What they find shows
    /// in [`Self::reach`] a few seconds later. An adapter that is switched
    /// off is passed over.
    pub fn scan() -> Result<(), String> {
        let client = Client::open()?;
        for adapter in client.adapters()? {
            // SAFETY: the handle is open and the identifier outlives the call.
            let status = unsafe { WlanScan(client.0, &adapter, None, None, None) };
            if status != ERROR_NDIS_DOT11_POWER_STATE_INVALID.0 {
                check(status, "Windows did not look for Wi-Fi networks")?;
            }
        }
        Ok(())
    }

    /// Asks Windows to join the network whose saved settings are called
    /// `profile`. Whether it joined is seen in [`Self::reach`] later.
    pub fn connect(profile: &str) -> Result<(), String> {
        let client = Client::open()?;
        let name = HSTRING::from(profile);
        let parameters = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(name.as_ptr()),
            pDot11Ssid: ptr::null_mut(),
            pDesiredBssidList: ptr::null_mut(),
            dot11BssType: dot11_BSS_type_any,
            dwFlags: 0,
        };
        let mut failure = format!("there is no Wi-Fi adapter to join {profile} with");
        for adapter in client.adapters()? {
            // SAFETY: the handle is open; the parameters, and the name they
            // point to, outlive the call.
            let status = unsafe { WlanConnect(client.0, &adapter, &parameters, None) };
            match check(status, "Windows did not join the network") {
                Ok(()) => return Ok(()),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}

/// `seen` with one entry for each name, in the order they are shown in.
/// Windows lists a network once for each of its saved settings and once
/// without; the entry that is connected, or else has settings, is kept.
fn arranged(mut seen: Vec<WifiNetwork>) -> Vec<WifiNetwork> {
    let rank = |network: &WifiNetwork| {
        (
            !network.connected,
            network.profile.is_none(),
            u8::MAX - network.bars,
        )
    };
    seen.sort_by(|one, other| {
        let by_name = one.name.cmp(&other.name);
        by_name.then_with(|| rank(one).cmp(&rank(other)))
    });
    seen.dedup_by(|later, kept| later.name == kept.name);
    seen.sort_by(|one, other| {
        let order = (!one.connected, u8::MAX - one.bars);
        order
            .cmp(&(!other.connected, u8::MAX - other.bars))
            .then_with(|| one.name.cmp(&other.name))
    });
    seen
}

/// The bars that stand for a signal quality of 0 to 100.
fn bars_of(quality: u32) -> u8 {
    (quality.min(100).div_ceil(20)) as u8
}

/// An open connection to the service of Windows that runs Wi-Fi.
struct Client(HANDLE);

impl Client {
    fn open() -> Result<Self, String> {
        let mut handle = HANDLE::default();
        let mut version = 0;
        // SAFETY: both pointers are to locals that outlive the call.
        let status = unsafe { WlanOpenHandle(CLIENT_VERSION, None, &mut version, &mut handle) };
        check(status, "the Wi-Fi service of Windows could not be reached")?;
        Ok(Self(handle))
    }

    /// The identifiers of the adapters.
    fn adapters(&self) -> Result<Vec<GUID>, String> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = ptr::null_mut();
        // SAFETY: the handle is open and `list` outlives the call.
        let status = unsafe { WlanEnumInterfaces(self.0, None, &mut list) };
        check(status, "the Wi-Fi adapters could not be listed")?;
        if list.is_null() {
            return Ok(Vec::new());
        }
        // SAFETY: Windows filled the list with as many entries as it says,
        // laid out one after another, and it stays until it is freed below.
        let adapters = unsafe {
            let count = (*list).dwNumberOfItems as usize;
            let first = ptr::addr_of!((*list).InterfaceInfo).cast();
            let adapters: &[windows::Win32::NetworkManagement::WiFi::WLAN_INTERFACE_INFO] =
                std::slice::from_raw_parts(first, count);
            let adapters = adapters.iter().map(|adapter| adapter.InterfaceGuid);
            let adapters = adapters.collect();
            WlanFreeMemory(list as *const c_void);
            adapters
        };
        Ok(adapters)
    }

    /// The networks `adapter` saw; `None` while it is switched off.
    fn networks(&self, adapter: &GUID) -> Result<Option<Vec<WifiNetwork>>, String> {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = ptr::null_mut();
        // SAFETY: the handle is open; the identifier and `list` outlive the
        // call.
        let status = unsafe { WlanGetAvailableNetworkList(self.0, adapter, 0, None, &mut list) };
        if status == ERROR_ACCESS_DENIED.0 {
            return Err(
                "Windows shows Wi-Fi networks only to applications that may use the location; \
                 that is switched in its privacy settings"
                    .to_owned(),
            );
        }
        if status == ERROR_NDIS_DOT11_POWER_STATE_INVALID.0 {
            return Ok(None);
        }
        check(status, "the Wi-Fi networks could not be listed")?;
        if list.is_null() {
            return Ok(Some(Vec::new()));
        }
        // SAFETY: as for the adapters above.
        let networks = unsafe {
            let count = (*list).dwNumberOfItems as usize;
            let first = ptr::addr_of!((*list).Network).cast();
            let entries: &[WLAN_AVAILABLE_NETWORK] = std::slice::from_raw_parts(first, count);
            let networks = entries.iter().filter_map(network_of).collect();
            WlanFreeMemory(list as *const c_void);
            networks
        };
        Ok(Some(networks))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by `open` and is closed only here.
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

/// What `entry` says; `None` for a network without a name.
fn network_of(entry: &WLAN_AVAILABLE_NETWORK) -> Option<WifiNetwork> {
    let length = (entry.dot11Ssid.uSSIDLength as usize).min(entry.dot11Ssid.ucSSID.len());
    let name = String::from_utf8_lossy(&entry.dot11Ssid.ucSSID[..length]).into_owned();
    if name.trim_matches('\0').trim().is_empty() {
        return None;
    }
    let profile = entry.strProfileName;
    let end = profile.iter().position(|&unit| unit == 0);
    let profile = String::from_utf16_lossy(&profile[..end.unwrap_or(profile.len())]);
    let known = entry.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0 && !profile.is_empty();
    Some(WifiNetwork {
        name,
        bars: bars_of(entry.wlanSignalQuality),
        secured: entry.bSecurityEnabled.as_bool(),
        connected: entry.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
        profile: known.then_some(profile),
    })
}

/// `Err` naming `what` failed unless `status` reports success.
fn check(status: u32, what: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        let reason = windows::core::Error::from_hresult(windows::core::HRESULT::from_win32(status));
        Err(format!("{what}: {}", reason.message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(name: &str, bars: u8, connected: bool, known: bool) -> WifiNetwork {
        WifiNetwork {
            name: name.to_owned(),
            bars,
            secured: true,
            connected,
            profile: known.then(|| name.to_owned()),
        }
    }

    #[test]
    fn a_signal_quality_becomes_bars() {
        assert_eq!(bars_of(0), 0);
        assert_eq!(bars_of(1), 1);
        assert_eq!(bars_of(20), 1);
        assert_eq!(bars_of(61), 4);
        assert_eq!(bars_of(100), 5);
        assert_eq!(bars_of(250), 5);
    }

    #[test]
    fn a_network_is_listed_once_with_what_is_known_of_it() {
        let listed = arranged(vec![
            network("Home", 4, false, false),
            network("Home", 4, true, true),
            network("Cafe", 5, false, false),
            network("Cafe", 2, false, true),
        ]);
        assert_eq!(
            listed,
            vec![
                network("Home", 4, true, true),
                network("Cafe", 2, false, true)
            ]
        );
    }

    #[test]
    fn the_connected_network_comes_first_and_then_the_stronger() {
        let listed = arranged(vec![
            network("Weak", 1, false, false),
            network("Strong", 5, false, false),
            network("Mine", 2, true, true),
        ]);
        let names: Vec<_> = listed.iter().map(|network| network.name.as_str()).collect();
        assert_eq!(names, ["Mine", "Strong", "Weak"]);
    }

    /// Run by hand: prints what the adapters of this machine see.
    #[test]
    #[ignore = "reads the Wi-Fi networks of this machine"]
    fn the_wifi_networks_of_this_machine() {
        match Wifi::reach() {
            Ok(WifiReach::NoAdapter) => println!("no adapter"),
            Ok(WifiReach::Off) => println!("switched off"),
            Ok(WifiReach::Networks(networks)) => {
                println!("{} networks", networks.len());
                for network in networks {
                    println!(
                        "bars={} secured={} connected={} known={}",
                        network.bars,
                        network.secured,
                        network.connected,
                        network.profile.is_some()
                    );
                }
            }
            Err(error) => println!("error: {error}"),
        }
    }
}
