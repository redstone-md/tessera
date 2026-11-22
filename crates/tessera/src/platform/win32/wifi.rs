// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Wi-Fi networks in reach, as Windows last saw them; joining one
//! Windows has the settings of, or one whose password is given here; and
//! having Windows forget the settings of one.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::ffi::c_void;
use std::ptr;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_NDIS_DOT11_POWER_STATE_INVALID, HANDLE,
};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_AUTH_ALGO_80211_OPEN, DOT11_AUTH_ALGO_RSNA_PSK, DOT11_AUTH_ALGO_WPA3_SAE,
    DOT11_AUTH_ALGORITHM, WLAN_AVAILABLE_NETWORK, WLAN_AVAILABLE_NETWORK_CONNECTED,
    WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS,
    WLAN_INTERFACE_INFO_LIST, WLAN_PROFILE_USER, WlanCloseHandle, WlanConnect, WlanDeleteProfile,
    WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList, WlanOpenHandle, WlanScan,
    WlanSetProfile, dot11_BSS_type_any, wlan_connection_mode_profile,
};
use windows::core::{GUID, HSTRING, PCWSTR};

/// The version of the interface asked for: the one since Windows Vista.
const CLIENT_VERSION: u32 = 2;

/// The fewest and the most characters a password of a network has.
const PASSWORD_LENGTHS: std::ops::RangeInclusive<usize> = 8..=63;

/// How a network lets a machine in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiSecurity {
    /// Anyone joins.
    Open,
    /// With a password all share, the way of WPA2.
    Password,
    /// With a password all share, the way of WPA3.
    Password3,
    /// Another way, such as an account for each person; such a network is
    /// not joined from here unless Windows has its settings.
    Other,
}

impl WifiSecurity {
    /// What the default way a network offers, with whether it is secured
    /// at all, comes to.
    fn of(way: DOT11_AUTH_ALGORITHM, secured: bool) -> Self {
        match way {
            DOT11_AUTH_ALGO_80211_OPEN if !secured => Self::Open,
            DOT11_AUTH_ALGO_RSNA_PSK => Self::Password,
            DOT11_AUTH_ALGO_WPA3_SAE => Self::Password3,
            _ => Self::Other,
        }
    }
}

/// Whether `password` can be the password of a network: 8 to 63 letters,
/// digits and signs of ASCII.
pub fn password_fits(password: &str) -> bool {
    PASSWORD_LENGTHS.contains(&password.len())
        && password.bytes().all(|byte| (0x20..0x7f).contains(&byte))
}

/// A Wi-Fi network in reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiNetwork {
    pub name: String,
    /// The name as the bytes the network sends.
    pub ssid: Vec<u8>,
    pub security: WifiSecurity,
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
        let adapters = client.adapters()?;
        client.connect(&adapters, profile)
    }

    /// Has Windows save settings for `network` with `password`, for this
    /// user alone, and asks it to join. The settings are called as the
    /// network is. Whether it joined is seen in [`Self::reach`] later; a
    /// wrong password shows only in that it never does.
    pub fn join(network: &WifiNetwork, password: &str) -> Result<(), String> {
        let settings = HSTRING::from(profile_of(network, password)?);
        let client = Client::open()?;
        let adapters = client.adapters()?;
        let mut saved = Vec::new();
        let mut failure = format!("there is no Wi-Fi adapter to join {} with", network.name);
        for adapter in adapters {
            let mut reason = 0;
            // SAFETY: the handle is open; the texts and `reason` outlive
            // the call.
            let status = unsafe {
                WlanSetProfile(
                    client.0,
                    &adapter,
                    WLAN_PROFILE_USER,
                    &settings,
                    PCWSTR::null(),
                    true,
                    None,
                    &mut reason,
                )
            };
            // The settings hold the password, so they are not reported.
            match check(status, "Windows did not save the settings of the network") {
                Ok(()) => saved.push(adapter),
                Err(error) => failure = format!("{error} (reason {reason})"),
            }
        }
        if saved.is_empty() {
            return Err(failure);
        }
        client.connect(&saved, &network.name)
    }

    /// Has Windows forget the settings called `profile`.
    pub fn forget(profile: &str) -> Result<(), String> {
        let client = Client::open()?;
        let name = HSTRING::from(profile);
        let mut failure = format!("there is no Wi-Fi adapter that knows {profile}");
        let mut forgotten = false;
        for adapter in client.adapters()? {
            // SAFETY: the handle is open; the name outlives the call.
            let status = unsafe { WlanDeleteProfile(client.0, &adapter, &name, None) };
            match check(status, "Windows did not forget the network") {
                Ok(()) => forgotten = true,
                Err(error) => failure = error,
            }
        }
        if forgotten { Ok(()) } else { Err(failure) }
    }
}

/// The settings Windows saves to join `network` with `password`, which an
/// open network has none of.
fn profile_of(network: &WifiNetwork, password: &str) -> Result<String, String> {
    let way = match network.security {
        WifiSecurity::Open => None,
        WifiSecurity::Password => Some("WPA2PSK"),
        WifiSecurity::Password3 => Some("WPA3SAE"),
        WifiSecurity::Other => {
            return Err(format!(
                "{} is not joined with a password alone",
                network.name
            ));
        }
    };
    if way.is_some() && !password_fits(password) {
        return Err("a password has 8 to 63 characters".to_owned());
    }
    let name = escaped(&network.name);
    let hex: String = network.ssid.iter().map(|b| format!("{b:02X}")).collect();
    let security = match way {
        None => "<authEncryption><authentication>open</authentication>\
                 <encryption>none</encryption><useOneX>false</useOneX></authEncryption>"
            .to_owned(),
        Some(way) => format!(
            "<authEncryption><authentication>{way}</authentication>\
             <encryption>AES</encryption><useOneX>false</useOneX></authEncryption>\
             <sharedKey><keyType>passPhrase</keyType><protected>false</protected>\
             <keyMaterial>{}</keyMaterial></sharedKey>",
            escaped(password)
        ),
    };
    Ok(format!(
        "<?xml version=\"1.0\"?>\
         <WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\
         <name>{name}</name>\
         <SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>\
         <connectionType>ESS</connectionType>\
         <connectionMode>auto</connectionMode>\
         <MSM><security>{security}</security></MSM>\
         </WLANProfile>"
    ))
}

/// `text` as it stands inside XML.
fn escaped(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for letter in text.chars() {
        match letter {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            letter => escaped.push(letter),
        }
    }
    escaped
}

impl Client {
    /// Asks the first of `adapters` that will to join the network whose
    /// saved settings are called `profile`.
    fn connect(&self, adapters: &[GUID], profile: &str) -> Result<(), String> {
        let client = self;
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
        for adapter in adapters {
            // SAFETY: the handle is open; the parameters, and the name they
            // point to, outlive the call.
            let status = unsafe { WlanConnect(client.0, adapter, &parameters, None) };
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
    let ssid = entry.dot11Ssid.ucSSID[..length].to_vec();
    let name = String::from_utf8_lossy(&ssid).into_owned();
    if name.trim_matches('\0').trim().is_empty() {
        return None;
    }
    let profile = entry.strProfileName;
    let end = profile.iter().position(|&unit| unit == 0);
    let profile = String::from_utf16_lossy(&profile[..end.unwrap_or(profile.len())]);
    let known = entry.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0 && !profile.is_empty();
    let secured = entry.bSecurityEnabled.as_bool();
    Some(WifiNetwork {
        name,
        ssid,
        security: WifiSecurity::of(entry.dot11DefaultAuthAlgorithm, secured),
        bars: bars_of(entry.wlanSignalQuality),
        secured,
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
            ssid: name.as_bytes().to_vec(),
            security: WifiSecurity::Password,
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

    #[test]
    fn the_settings_of_a_network_hold_its_name_and_password_escaped() {
        let mut cafe = network("Tom & <Jerry>", 3, false, false);
        let settings = profile_of(&cafe, "p<ss\"word&1").unwrap();
        assert!(settings.contains("<name>Tom &amp; &lt;Jerry&gt;</name>"));
        assert!(settings.contains("<hex>546F6D2026203C4A657272793E</hex>"));
        assert!(settings.contains("<authentication>WPA2PSK</authentication>"));
        assert!(settings.contains("<keyMaterial>p&lt;ss&quot;word&amp;1</keyMaterial>"));
        cafe.security = WifiSecurity::Password3;
        let settings = profile_of(&cafe, "password").unwrap();
        assert!(settings.contains("<authentication>WPA3SAE</authentication>"));
        // An open network has no key, whatever is given.
        cafe.security = WifiSecurity::Open;
        let settings = profile_of(&cafe, "").unwrap();
        assert!(settings.contains("<authentication>open</authentication>"));
        assert!(!settings.contains("sharedKey"));
        cafe.security = WifiSecurity::Other;
        assert!(profile_of(&cafe, "password").is_err());
    }

    #[test]
    fn a_password_has_8_to_63_characters_of_ascii() {
        assert!(!password_fits("1234567"));
        assert!(password_fits("12345678"));
        assert!(password_fits(&"x".repeat(63)));
        assert!(!password_fits(&"x".repeat(64)));
        assert!(!password_fits("пароль12"));
        let home = network("Home", 3, false, false);
        assert!(profile_of(&home, "short").is_err());
    }

    #[test]
    fn the_way_a_network_lets_in_is_told_from_what_it_offers() {
        let of = WifiSecurity::of;
        assert_eq!(of(DOT11_AUTH_ALGO_80211_OPEN, false), WifiSecurity::Open);
        // What is open in name but secured is the old WEP.
        assert_eq!(of(DOT11_AUTH_ALGO_80211_OPEN, true), WifiSecurity::Other);
        assert_eq!(of(DOT11_AUTH_ALGO_RSNA_PSK, true), WifiSecurity::Password);
        assert_eq!(of(DOT11_AUTH_ALGO_WPA3_SAE, true), WifiSecurity::Password3);
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
