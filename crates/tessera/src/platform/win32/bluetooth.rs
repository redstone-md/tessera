// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Bluetooth devices paired with the machine, those in reach that can
//! be paired, pairing them and removing them.
//!
//! Every call waits for Windows to answer, so none belongs on the thread
//! that draws.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use windows::Devices::Bluetooth::{BluetoothAdapter, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationCustomPairing, DeviceInformationUpdate, DevicePairingKinds,
    DevicePairingRequestedEventArgs, DevicePairingResultStatus, DeviceUnpairingResultStatus,
    DeviceWatcher,
};
use windows::Foundation::{IPropertyValue, TypedEventHandler};
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
const IS_PAIRED: &str = "System.Devices.Aep.IsPaired";
const CAN_PAIR: &str = "System.Devices.Aep.CanPair";

/// How long a pairing waits for the user to answer the device.
const ANSWER_WAIT: Duration = Duration::from_secs(120);

/// How many prompts of a pairing wait for the user interface.
const PROMPTS_WAITING: usize = 4;

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
        let devices = Self::paired_devices()?;
        Ok(devices.into_iter().map(|(_, device)| device).collect())
    }

    /// The paired devices as [`Self::devices`] lists them, each with what
    /// Windows names it by, to remove it with.
    pub fn paired_devices() -> Result<Vec<(String, PairedDevice)>, String> {
        let read = || -> WindowsResult<Vec<Listed>> {
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
        for (_, device) in &mut devices {
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

/// A Bluetooth device in reach that is not paired and can be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundDevice {
    /// What Windows names the device by, to pair it with.
    pub id: String,
    pub name: String,
    /// Whether it speaks Bluetooth Low Energy rather than the classic kind.
    pub low_energy: bool,
}

/// What the user is asked while a device pairs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingPrompt {
    /// The PIN to type on the device; the pairing goes on by itself.
    Show(String),
    /// Whether the device shows this PIN too; answered with yes or no.
    Confirm(String),
    /// The PIN the device shows or came with, to type here.
    Enter,
}

/// Why a device was not paired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairingFailure {
    /// The device, or the PIN given, was refused.
    Refused,
    /// The device did not answer in time.
    TimedOut,
    /// The device is not ready, as when it is not in its pairing mode.
    NotReady,
    /// The user cancelled.
    Cancelled,
    /// Windows gave another reason, which this names.
    Other(String),
}

impl PairingFailure {
    fn of(status: DevicePairingResultStatus) -> Self {
        match status {
            DevicePairingResultStatus::AuthenticationFailure
            | DevicePairingResultStatus::AuthenticationNotAllowed
            | DevicePairingResultStatus::ConnectionRejected
            | DevicePairingResultStatus::RejectedByHandler
            | DevicePairingResultStatus::AccessDenied => Self::Refused,
            DevicePairingResultStatus::AuthenticationTimeout => Self::TimedOut,
            DevicePairingResultStatus::NotReadyToPair
            | DevicePairingResultStatus::HardwareFailure
            | DevicePairingResultStatus::TooManyConnections => Self::NotReady,
            DevicePairingResultStatus::PairingCanceled => Self::Cancelled,
            status => Self::Other(format!("Windows answered with status {}", status.0)),
        }
    }
}

/// A device being paired: what the user is asked, how it ended, and the
/// way to answer.
pub struct Pairing {
    /// What the user is to be asked, as the device asks for it.
    pub prompts: Receiver<PairingPrompt>,
    /// How the pairing ended, once it has.
    pub outcome: Receiver<Result<(), PairingFailure>>,
    answers: mpsc::Sender<Option<String>>,
}

impl Pairing {
    /// Answers the last prompt: the PIN typed for
    /// [`PairingPrompt::Enter`], any text for yes to
    /// [`PairingPrompt::Confirm`], and `None` for no, which cancels.
    pub fn answer(&self, answer: Option<String>) {
        let _ = self.answers.send(answer);
    }
}

/// The search for Bluetooth devices to pair, which runs until it is
/// dropped. Windows keeps looking the whole time, so it is meant to run
/// only while the list it fills is shown.
pub struct BluetoothScan {
    watchers: Vec<DeviceWatcher>,
    found: Arc<Mutex<Found>>,
    changes: Receiver<()>,
}

/// What a search found, by the identifier of each device: what Windows
/// said of it, and what of that is shown, if it can be paired.
#[derive(Default)]
struct Found {
    devices: HashMap<String, (DeviceInformation, Option<FoundDevice>)>,
}

fn locked(found: &Mutex<Found>) -> std::sync::MutexGuard<'_, Found> {
    found
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BluetoothScan {
    /// Starts looking for classic and low energy devices that are not
    /// paired.
    pub fn start() -> Result<Self, String> {
        let found = Arc::new(Mutex::new(Found::default()));
        let (signal, changes) = async_channel::bounded(1);
        let mut watchers = Vec::new();
        let selectors = [
            (
                BluetoothDevice::GetDeviceSelectorFromPairingState(false),
                false,
            ),
            (
                BluetoothLEDevice::GetDeviceSelectorFromPairingState(false),
                true,
            ),
        ];
        for (selector, low_energy) in selectors {
            let watcher = selector
                .and_then(|selector| watch(&selector, low_energy, &found, &signal))
                .map_err(|error| format!("Windows does not look for Bluetooth devices: {error}"))?;
            watchers.push(watcher);
        }
        Ok(Self {
            watchers,
            found,
            changes,
        })
    }

    /// The devices found so far that can be paired, by name.
    pub fn found(&self) -> Vec<FoundDevice> {
        let found = locked(&self.found);
        let mut devices: Vec<FoundDevice> = { found.devices.values() }
            .filter_map(|(_, device)| device.clone())
            .collect();
        devices.sort_by(|one, other| {
            let order = (one.name.to_lowercase(), &one.id);
            order.cmp(&(other.name.to_lowercase(), &other.id))
        });
        devices
    }

    /// A signal for each change of what was found; a reader that falls
    /// behind gets one for several.
    pub fn changes(&self) -> Receiver<()> {
        self.changes.clone()
    }
}

impl Drop for BluetoothScan {
    fn drop(&mut self) {
        for watcher in &self.watchers {
            let _ = watcher.Stop();
        }
    }
}

/// Starts a watcher of the devices `selector` picks, which records them in
/// `found` and signals each change on `signal`.
fn watch(
    selector: &HSTRING,
    low_energy: bool,
    found: &Arc<Mutex<Found>>,
    signal: &Sender<()>,
) -> WindowsResult<DeviceWatcher> {
    let wanted = [IS_PAIRED, CAN_PAIR].map(HSTRING::from).to_vec();
    let watcher = DeviceInformation::CreateWatcherAqsFilterAndAdditionalProperties(
        selector,
        &IIterable::<HSTRING>::from(wanted),
    )?;
    let (added, added_signal) = (found.clone(), signal.clone());
    watcher.Added(&TypedEventHandler::new(
        move |_, info: windows::core::Ref<'_, DeviceInformation>| {
            if let Some(info) = info.as_ref() {
                let device = snapshot(info, low_energy);
                let id = info.Id()?.to_string();
                locked(&added).devices.insert(id, (info.clone(), device));
                let _ = added_signal.force_send(());
            }
            Ok(())
        },
    ))?;
    let (updated, updated_signal) = (found.clone(), signal.clone());
    watcher.Updated(&TypedEventHandler::new(
        move |_, update: windows::core::Ref<'_, DeviceInformationUpdate>| {
            if let Some(update) = update.as_ref() {
                let id = update.Id()?.to_string();
                let mut found = locked(&updated);
                if let Some((info, device)) = found.devices.get_mut(&id) {
                    info.Update(update)?;
                    *device = snapshot(info, low_energy);
                    let _ = updated_signal.force_send(());
                }
            }
            Ok(())
        },
    ))?;
    let (removed, removed_signal) = (found.clone(), signal.clone());
    watcher.Removed(&TypedEventHandler::new(
        move |_, update: windows::core::Ref<'_, DeviceInformationUpdate>| {
            if let Some(update) = update.as_ref() {
                let id = update.Id()?.to_string();
                if locked(&removed).devices.remove(&id).is_some() {
                    let _ = removed_signal.force_send(());
                }
            }
            Ok(())
        },
    ))?;
    watcher.Start()?;
    Ok(watcher)
}

/// What is shown of the device `info`: nothing for one without a name, one
/// that is paired by now, or one Windows says cannot be paired.
fn snapshot(info: &DeviceInformation, low_energy: bool) -> Option<FoundDevice> {
    let properties = info.Properties().ok()?;
    let flag = |name: &str| {
        let value = properties.Lookup(&HSTRING::from(name)).ok()?;
        value.cast::<IPropertyValue>().ok()?.GetBoolean().ok()
    };
    let name = info.Name().ok()?.to_string();
    let shown =
        !name.trim().is_empty() && flag(IS_PAIRED) != Some(true) && flag(CAN_PAIR) != Some(false);
    shown.then(|| FoundDevice {
        id: info.Id().map(|id| id.to_string()).unwrap_or_default(),
        name,
        low_energy,
    })
}

/// Pairs the device `id` with the machine and answers what it asks: a
/// device that only wants a yes gets it, one that shows a PIN or wants one
/// asks the user through `prompts` and waits for `answers`.
fn pair(
    id: &str,
    prompts: Sender<PairingPrompt>,
    answers: mpsc::Receiver<Option<String>>,
) -> Result<(), PairingFailure> {
    let failed = |error: windows::core::Error| PairingFailure::Other(error.message());
    let info = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))
        .and_then(|found| found.get())
        .map_err(failed)?;
    let custom = info
        .Pairing()
        .and_then(|pairing| pairing.Custom())
        .map_err(failed)?;
    let answers = Mutex::new(answers);
    let handler =
        TypedEventHandler::<DeviceInformationCustomPairing, DevicePairingRequestedEventArgs>::new(
            move |_, args: windows::core::Ref<'_, DevicePairingRequestedEventArgs>| {
                let Some(args) = args.as_ref() else {
                    return Ok(());
                };
                let kind = args.PairingKind()?;
                if kind == DevicePairingKinds::ConfirmOnly {
                    return args.Accept();
                }
                if kind == DevicePairingKinds::DisplayPin {
                    let _ = prompts.force_send(PairingPrompt::Show(args.Pin()?.to_string()));
                    return args.Accept();
                }
                let prompt = match kind {
                    DevicePairingKinds::ConfirmPinMatch => {
                        PairingPrompt::Confirm(args.Pin()?.to_string())
                    }
                    DevicePairingKinds::ProvidePin => PairingPrompt::Enter,
                    _ => return Ok(()),
                };
                // The answer comes from the user; Windows waits for it while
                // the deferral is held.
                let deferral = args.GetDeferral()?;
                let _ = prompts.force_send(prompt);
                let answers = answers
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let accepted = match answers.recv_timeout(ANSWER_WAIT) {
                    Ok(Some(pin)) if kind == DevicePairingKinds::ProvidePin => {
                        args.AcceptWithPin(&HSTRING::from(pin))
                    }
                    Ok(Some(_)) => args.Accept(),
                    Ok(None) | Err(_) => Ok(()),
                };
                deferral.Complete()?;
                accepted
            },
        );
    let token = custom.PairingRequested(&handler).map_err(failed)?;
    let kinds = DevicePairingKinds::ConfirmOnly
        | DevicePairingKinds::DisplayPin
        | DevicePairingKinds::ProvidePin
        | DevicePairingKinds::ConfirmPinMatch;
    let result = custom
        .PairAsync(kinds)
        .and_then(|pairing| pairing.get())
        .and_then(|result| result.Status());
    let _ = custom.RemovePairingRequested(token);
    match result.map_err(failed)? {
        DevicePairingResultStatus::Paired | DevicePairingResultStatus::AlreadyPaired => Ok(()),
        status => Err(PairingFailure::of(status)),
    }
}

impl Bluetooth {
    /// Starts pairing the device `id`, which a [`BluetoothScan`] found, on
    /// a thread of its own: Windows waits for the device, and for the
    /// user where the device asks for a PIN.
    pub fn pair(id: String) -> Pairing {
        let (prompts, prompted) = async_channel::bounded(PROMPTS_WAITING);
        let (ended, outcome) = async_channel::bounded(1);
        let (answers, answered) = mpsc::channel();
        let started = std::thread::Builder::new()
            .name("bluetooth-pairing".to_owned())
            .spawn({
                let ended = ended.clone();
                move || {
                    let _ = ended.force_send(pair(&id, prompts, answered));
                }
            });
        if let Err(error) = started {
            let failure = PairingFailure::Other(format!("the pairing did not start: {error}"));
            let _ = ended.force_send(Err(failure));
        }
        Pairing {
            prompts: prompted,
            outcome,
            answers,
        }
    }

    /// Removes the pairing of the device `id`, as Windows' settings do
    /// with "Remove device".
    pub fn unpair(id: &str) -> Result<(), String> {
        let status = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))
            .and_then(|found| found.get())
            .and_then(|info| info.Pairing())
            .and_then(|pairing| pairing.UnpairAsync())
            .and_then(|unpairing| unpairing.get())
            .and_then(|result| result.Status())
            .map_err(|error| format!("the device was not removed: {}", error.message()))?;
        match status {
            DeviceUnpairingResultStatus::Unpaired
            | DeviceUnpairingResultStatus::AlreadyUnpaired => Ok(()),
            DeviceUnpairingResultStatus::AccessDenied => {
                Err("the device was not removed: Windows denied it".to_owned())
            }
            status => Err(format!(
                "the device was not removed: Windows answered with status {}",
                status.0
            )),
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

/// A paired device as it is listed: its address if it has one, what
/// Windows names it by, and what is shown of it.
type Listed = (Option<u64>, String, PairedDevice);

/// The paired devices `selector` picks.
fn paired(selector: &HSTRING) -> WindowsResult<Vec<Listed>> {
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
            device.Id()?.to_string(),
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
/// Bluetooth is listed by both and kept once, under the name of the first.
fn arranged(devices: Vec<Listed>, batteries: &HashMap<u64, u8>) -> Vec<(String, PairedDevice)> {
    let mut kept: Vec<Listed> = Vec::new();
    for (address, id, mut device) in devices {
        device.battery = address.and_then(|address| batteries.get(&address).copied());
        let twin = kept.iter_mut().find(|(other, _, kept)| match address {
            Some(address) => *other == Some(address),
            None => other.is_none() && kept.name == device.name,
        });
        match twin {
            Some((_, _, twin)) => {
                twin.connected |= device.connected;
                twin.audio = twin.audio.or(device.audio);
            }
            None => kept.push((address, id, device)),
        }
    }
    let mut devices: Vec<_> = kept
        .into_iter()
        .map(|(_, id, device)| (id, device))
        .collect();
    devices.sort_by(|(_, one), (_, other)| {
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
                (Some(1), "s".to_owned(), device("Speaker", false)),
                (Some(2), "classic".to_owned(), device("Mouse", false)),
                (Some(2), "low".to_owned(), device("Mouse", true)),
                (None, "a".to_owned(), device("Alpha", false)),
            ],
            &batteries,
        );
        let mut mouse = device("Mouse", true);
        mouse.battery = Some(80);
        assert_eq!(
            listed,
            vec![
                ("classic".to_owned(), mouse),
                ("a".to_owned(), device("Alpha", false)),
                ("s".to_owned(), device("Speaker", false))
            ]
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
