// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::ptr::null_mut;

use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::IPropertyValue;
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_GETIDLIST_FILTER_ENUMERATOR, CM_GETIDLIST_FILTER_PRESENT, CM_Get_DevNode_PropertyW,
    CM_Get_Device_ID_List_SizeW, CM_Get_Device_ID_ListW, CM_LOCATE_DEVNODE_NORMAL,
    CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROPTYPE};
use windows::Win32::Foundation::{DEVPROPKEY, PROPERTYKEY};
use windows::Win32::Media::Audio::{
    DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_UNPLUGGED, IDeviceTopology, IMMDevice, eAll,
};
use windows::Win32::Media::KernelStreaming::{
    IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_ONESHOT_DISCONNECT,
    KSPROPERTY_ONESHOT_RECONNECT, KSPROPERTY_TYPE_GET, KSPROPSETID_BtAudio,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToGUID;
use windows::Win32::System::Com::{CLSCTX_ALL, STGM_READ};
use windows::core::{GUID, HSTRING, Interface, PCWSTR, Result as WindowsResult};
use windows_collections::IIterable;

use super::audio::enumerator;
use super::packaged::take;

/// The properties asked for along with each device.
const IS_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
const CONTAINER: &str = "System.Devices.Aep.ContainerId";

/// The container of the device an audio endpoint belongs to: what Windows
/// holds the parts of one physical device together by.
const ENDPOINT_CONTAINER: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c),
    pid: 2,
};

/// How the name of the device behind an audio endpoint goes on when that
/// device is a Bluetooth one.
const BLUETOOTH_PATH: &str = "\\\\?\\bth";

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
    /// What Windows holds the device together by, for a device Windows
    /// plays or records sound through: only such a device can be told to
    /// connect and to disconnect.
    pub audio: Option<u128>,
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
        let mut devices = arranged(devices, &batteries());
        let sounding: HashSet<u128> = links()
            .into_iter()
            .map(|(container, _)| container)
            .collect();
        for device in &mut devices {
            device.audio = device
                .audio
                .filter(|container| sounding.contains(container));
        }
        Ok(devices)
    }

    /// Asks the sound device held together by `container` to connect, or
    /// to disconnect. The answer says only that the device driver took the
    /// request: whether the device then connects shows in `devices`.
    pub fn link(container: u128, connect: bool) -> Result<(), String> {
        let wish = match connect {
            true => KSPROPERTY_ONESHOT_RECONNECT,
            false => KSPROPERTY_ONESHOT_DISCONNECT,
        };
        let property = KSIDENTIFIER {
            Anonymous: KSIDENTIFIER_0 {
                Anonymous: KSIDENTIFIER_0_0 {
                    Set: KSPROPSETID_BtAudio,
                    Id: wish.0 as u32,
                    Flags: KSPROPERTY_TYPE_GET,
                },
            },
        };
        let mut taken = false;
        let mut failure = None;
        for (_, device) in links().into_iter().filter(|(of, _)| *of == container) {
            let mut returned = 0;
            // SAFETY: COM calls on a device that was just listed; the
            // property outlives the call and says its own size, and the
            // request carries no data.
            let asked = unsafe {
                device
                    .Activate::<IKsControl>(CLSCTX_ALL, None)
                    .and_then(|control| {
                        control.KsProperty(
                            &raw const property,
                            size_of::<KSIDENTIFIER>() as u32,
                            null_mut(),
                            0,
                            &raw mut returned,
                        )
                    })
            };
            match asked {
                Ok(()) => taken = true,
                Err(error) => failure = Some(error),
            }
        }
        match (taken, failure) {
            (true, _) => Ok(()),
            (false, Some(error)) => Err(format!("the device did not take the request: {error}")),
            (false, None) => Err("the device has no sound to connect by".to_owned()),
        }
    }
}

/// The Bluetooth devices behind the audio endpoints, those in use and
/// those that wait for their device, each with the container of its
/// device. One device may be there several times: for playing, for
/// recording and for calls.
fn links() -> Vec<(u128, IMMDevice)> {
    let Ok(devices) = enumerator() else {
        return Vec::new();
    };
    let mut links = Vec::new();
    // SAFETY: plain COM calls; each value read is owned here, and `take`
    // frees the string that is allocated for the caller.
    unsafe {
        let Ok(found) = devices.EnumAudioEndpoints(
            eAll,
            DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_UNPLUGGED.0),
        ) else {
            return links;
        };
        for index in 0..found.GetCount().unwrap_or(0) {
            let Ok(endpoint) = found.Item(index) else {
                continue;
            };
            let container = endpoint
                .OpenPropertyStore(STGM_READ)
                .and_then(|store| store.GetValue(&ENDPOINT_CONTAINER))
                .and_then(|value| PropVariantToGUID(&raw const value));
            let behind = endpoint
                .Activate::<IDeviceTopology>(CLSCTX_ALL, None)
                .and_then(|topology| topology.GetConnector(0))
                .and_then(|connector| connector.GetDeviceIdConnectedTo())
                .ok()
                .and_then(take);
            let (Ok(container), Some(behind)) = (container, behind) else {
                continue;
            };
            if !is_bluetooth(&behind) {
                continue;
            }
            if let Ok(device) = devices.GetDevice(&HSTRING::from(behind)) {
                links.push((container.to_u128(), device));
            }
        }
    }
    links
}

/// Whether `id`, which names the device behind an audio endpoint, names a
/// Bluetooth one.
fn is_bluetooth(id: &str) -> bool {
    id.to_lowercase().contains(BLUETOOTH_PATH)
}

/// The paired devices `selector` picks, each with its address if it has one.
fn paired(selector: &HSTRING) -> WindowsResult<Vec<(Option<u64>, PairedDevice)>> {
    let wanted = [IS_CONNECTED, ADDRESS, CONTAINER]
        .map(HSTRING::from)
        .to_vec();
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
        let container = property(CONTAINER).and_then(|value| value.GetGuid().ok());
        devices.push((
            address.and_then(|address| address_of(&address.to_string())),
            PairedDevice {
                name: device.Name()?.to_string(),
                connected: connected.unwrap_or(false),
                battery: None,
                audio: container.map(|container| container.to_u128()),
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
            Some((_, twin)) => {
                twin.connected |= device.connected;
                twin.audio = twin.audio.or(device.audio);
            }
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
            audio: None,
        }
    }

    #[test]
    fn a_bluetooth_device_behind_an_endpoint_is_told_by_its_name() {
        let sink = "{2}.\\\\?\\bthenum#{0000110b-0000-1000-8000-00805f9b34fb}_vid&0001004c#7&2f1a&0&\
                    aabbccddeeff_c00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\\snk";
        assert!(is_bluetooth(sink));
        assert!(is_bluetooth(
            "{2}.\\\\?\\BTHHFENUM#BthHFPAudio#8&1b2c&0&97#{guid}\\wave"
        ));
        assert!(!is_bluetooth(
            "{2}.\\\\?\\usb#vid_046d&pid_0a8f&mi_00#7&1b2c&0&0000#{guid}\\wave"
        ));
        assert!(!is_bluetooth(
            "{2}.\\\\?\\hdaudio#func_01&ven_10ec#5&1b2c&0&0001#{guid}\\speaker"
        ));
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
                        "name_chars={} connected={} battery={:?} audio={}",
                        device.name.chars().count(),
                        device.connected,
                        device.battery,
                        device.audio.is_some()
                    );
                }
            }
            Err(error) => println!("error: {error}"),
        }
        println!("{} sound links", links().len());
    }
}
