// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::collections::HashMap;

use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::IPropertyValue;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW,
    CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROPTYPE};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result as WindowsResult};
use windows_collections::IIterable;

/// The properties asked for along with each device.
const IS_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const ADDRESS: &str = "System.Devices.Aep.DeviceAddress";

/// Where Windows keeps the charge a Bluetooth device reported, in percent.
const BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};

/// The parts of the device tree that hold what Bluetooth devices offer:
/// the classic ones and the low energy ones.
const ENUMERATORS: [&str; 2] = ["BTHENUM", "BTHLE"];

/// A Bluetooth device paired with the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PairedDevice {
    pub name: String,
    pub connected: bool,
    /// Its charge in percent, when it reported one to Windows.
    pub battery: Option<u8>,
}

/// The Bluetooth of the machine.
pub struct Bluetooth;

impl Bluetooth {
    /// Whether the machine has Bluetooth at all, switched on or not.
    pub fn present() -> bool {
        BluetoothAdapter::GetDefaultAsync()
            .and_then(|adapter| adapter.get())
            .is_ok()
    }

    /// The paired devices, the connected ones first, each group by name.
    /// Empty without Bluetooth.
    pub fn devices() -> Result<Vec<PairedDevice>, String> {
        let read = || -> WindowsResult<Vec<(Option<u64>, PairedDevice)>> {
            let mut devices = paired(&BluetoothDevice::GetDeviceSelectorFromPairingState(true)?)?;
            let low_energy = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
            devices.extend(paired(&low_energy)?);
            Ok(devices)
        };
        let devices = read()
            .map_err(|error| format!("the Bluetooth devices could not be listed: {error}"))?;
        Ok(arranged(devices, &batteries()))
    }
}

/// The paired devices `selector` picks, each with its address if it has one.
fn paired(selector: &HSTRING) -> WindowsResult<Vec<(Option<u64>, PairedDevice)>> {
    let wanted = vec![HSTRING::from(IS_CONNECTED), HSTRING::from(ADDRESS)];
    let wanted = IIterable::<HSTRING>::from(wanted);
    let found = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(selector, &wanted)?
        .get()?;
    let mut devices = Vec::new();
    for device in found {
        let properties = device.Properties()?;
        // A property Windows does not know of the device is absent, which
        // reads as an error.
        let property = |name: &str| {
            let value = properties.Lookup(&HSTRING::from(name)).ok()?;
            value.cast::<IPropertyValue>().ok()
        };
        let connected = property(IS_CONNECTED).and_then(|value| value.GetBoolean().ok());
        let address = property(ADDRESS).and_then(|value| value.GetString().ok());
        devices.push((
            address.and_then(|address| address_of(&address.to_string())),
            PairedDevice {
                name: device.Name()?.to_string(),
                connected: connected.unwrap_or(false),
                battery: None,
            },
        ));
    }
    Ok(devices)
}

/// `devices` in the order they are shown in, with their charge from
/// `batteries`, which goes by address. A device that speaks both kinds of
/// Bluetooth is listed by both and kept once.
fn arranged(
    devices: Vec<(Option<u64>, PairedDevice)>,
    batteries: &HashMap<u64, u8>,
) -> Vec<PairedDevice> {
    let mut kept: Vec<(Option<u64>, PairedDevice)> = Vec::new();
    for (address, mut device) in devices {
        device.battery = address.and_then(|address| batteries.get(&address).copied());
        let twin = kept.iter_mut().find(|(other, kept)| match address {
            Some(address) => *other == Some(address),
            None => other.is_none() && kept.name == device.name,
        });
        match twin {
            Some((_, twin)) => twin.connected |= device.connected,
            None => kept.push((address, device)),
        }
    }
    let mut devices: Vec<_> = kept.into_iter().map(|(_, device)| device).collect();
    devices.sort_by(|one, other| {
        let order = (!one.connected, one.name.to_lowercase());
        order.cmp(&(!other.connected, other.name.to_lowercase()))
    });
    devices
}

/// The address written as `aa:bb:cc:dd:ee:ff`.
fn address_of(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|&c| c != ':').collect();
    (digits.len() == 12).then(|| u64::from_str_radix(&digits, 16).ok())?
}

/// The address of the Bluetooth device that the device tree entry `id`
/// belongs to. A classic one ends in `…&AABBCCDDEEFF_C00000000`, a low
/// energy one has a part `DEV_AABBCCDDEEFF`.
fn address_in(id: &str) -> Option<u64> {
    let parts = id.split(['\\', '&']);
    let mut digits = parts.filter_map(|part| {
        let part = part.strip_prefix("DEV_").unwrap_or(part);
        let part = part.split('_').next()?;
        let hex = part.len() == 12 && part.chars().all(|c| c.is_ascii_hexdigit());
        hex.then_some(part)
    });
    u64::from_str_radix(digits.next_back()?, 16).ok()
}

/// The charge of every present Bluetooth device that reported one, by its
/// address. What cannot be read is left out.
fn batteries() -> HashMap<u64, u8> {
    let mut batteries = HashMap::new();
    for enumerator in ENUMERATORS {
        for id in entries(enumerator) {
            if let (Some(address), Some(charge)) = (address_in(&id), battery_of(&id)) {
                batteries.insert(address, charge);
            }
        }
    }
    batteries
}

/// The present entries of the device tree under `enumerator`.
fn entries(enumerator: &str) -> Vec<String> {
    let filter = HSTRING::from(enumerator);
    let flags = CM_GETIDLIST_FILTER_ENUMERATOR | CM_GETIDLIST_FILTER_PRESENT;
    let mut length = 0;
    // SAFETY: the filter and the length outlive the call.
    let sized = unsafe { CM_Get_Device_ID_List_SizeW(&mut length, &filter, flags) };
    if sized != CR_SUCCESS || length == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: the buffer is as long as Windows asked for.
    let listed = unsafe { CM_Get_Device_ID_ListW(&filter, &mut buffer, flags) };
    if listed != CR_SUCCESS {
        return Vec::new();
    }
    // The names follow one another, each ended by a zero.
    let names = buffer.split(|&unit| unit == 0);
    let names = names.filter(|name| !name.is_empty());
    names.map(String::from_utf16_lossy).collect()
}

/// The charge the device tree entry `id` carries, in percent.
fn battery_of(id: &str) -> Option<u8> {
    let name = HSTRING::from(id);
    let mut entry = 0;
    // SAFETY: the name and `entry` outlive the call.
    let located =
        unsafe { CM_Locate_DevNodeW(&mut entry, PCWSTR(name.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) };
    if located != CR_SUCCESS {
        return None;
    }
    let mut kind = DEVPROPTYPE::default();
    let mut charge = 0u8;
    let mut size = 1;
    // SAFETY: the buffer is one byte, as `size` says, and all pointers are
    // to locals that outlive the call.
    let read = unsafe {
        CM_Get_DevNode_PropertyW(entry, &BATTERY, &mut kind, Some(&mut charge), &mut size, 0)
    };
    (read == CR_SUCCESS && kind == DEVPROP_TYPE_BYTE && charge <= 100).then_some(charge)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, connected: bool) -> PairedDevice {
        PairedDevice {
            name: name.to_owned(),
            connected,
            battery: None,
        }
    }

    #[test]
    fn an_address_is_read_with_colons() {
        assert_eq!(address_of("aa:bb:cc:dd:ee:ff"), Some(0xaabb_ccdd_eeff));
        assert_eq!(address_of("aa:bb"), None);
        assert_eq!(address_of(""), None);
    }

    #[test]
    fn a_device_tree_entry_names_the_address_of_its_device() {
        let classic = "BTHENUM\\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001004C_PID&2014\\\
                       7&2F1A&0&AABBCCDDEEFF_C00000000";
        assert_eq!(address_in(classic), Some(0xaabb_ccdd_eeff));
        let low_energy = "BTHLE\\DEV_AABBCCDDEEFF\\8&1B2C&0&AABBCCDDEEFF";
        assert_eq!(address_in(low_energy), Some(0xaabb_ccdd_eeff));
        assert_eq!(
            address_in("BTHLE\\DEV_AABBCCDDEEFF\\8&1B2C&0"),
            Some(0xaabb_ccdd_eeff)
        );
        assert_eq!(address_in("BTHENUM\\Dev\\1&2&3"), None);
    }

    #[test]
    fn connected_devices_come_first_and_twins_are_one() {
        let batteries = HashMap::from([(2, 80)]);
        let listed = arranged(
            vec![
                (Some(1), device("Speaker", false)),
                (Some(2), device("Mouse", false)),
                (Some(2), device("Mouse", true)),
                (None, device("Alpha", false)),
            ],
            &batteries,
        );
        let mut mouse = device("Mouse", true);
        mouse.battery = Some(80);
        assert_eq!(
            listed,
            vec![mouse, device("Alpha", false), device("Speaker", false)]
        );
    }

    /// Run by hand: prints what is paired with this machine, without names.
    #[test]
    #[ignore = "reads the Bluetooth devices of this machine"]
    fn the_bluetooth_devices_of_this_machine() {
        match Bluetooth::devices() {
            Ok(devices) => {
                println!("{} devices", devices.len());
                for device in devices {
                    println!(
                        "name_chars={} connected={} battery={:?}",
                        device.name.chars().count(),
                        device.connected,
                        device.battery
                    );
                }
            }
            Err(error) => println!("error: {error}"),
        }
    }
}
