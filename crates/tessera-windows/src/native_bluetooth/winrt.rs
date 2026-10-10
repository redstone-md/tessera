// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Raw SDK calls only; request ordering and lifetime belong to owner::Owner.
use super::owner::Calls;
use std::marker::PhantomData;
use std::rc::Rc;
use tessera_system::bluetooth::{BluetoothError, BluetoothErrorKind, BluetoothTransport};
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationCollection, DeviceInformationKind, DeviceInformationPairing,
};
use windows::Devices::Radios::{Radio, RadioKind};
use windows::Foundation::{IPropertyValue, PropertyType};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{HSTRING, IInspectable, Interface};
use windows_collections::{IIterable, IMapView, IVectorView};

pub(super) enum NativeRadios {
    Passive(IVectorView<Radio>),
    Controlled(Vec<Radio>),
}

#[derive(Default)]
pub(super) struct NativeCalls {
    _thread: PhantomData<Rc<()>>,
    pub(super) controls: super::radio_control::RadioInventory,
}

impl NativeCalls {
    pub(super) fn with_authorities(authorities: super::radio_control::Authorities) -> Self {
        Self {
            controls: super::radio_control::RadioInventory::with_authorities(authorities),
            ..Self::default()
        }
    }
}

impl Calls for NativeCalls {
    type Radios = NativeRadios;
    type Radio = Radio;
    type Selector = HSTRING;
    type RequestedProperties = IIterable<HSTRING>;
    type Devices = DeviceInformationCollection;
    type Device = DeviceInformation;
    type Pairing = DeviceInformationPairing;
    type Properties = IMapView<HSTRING, IInspectable>;
    type Value = IInspectable;
    fn begin_controls(&mut self) {
        self.controls.begin();
    }

    fn observe_control(
        &mut self,
        radio: &Radio,
        observation: &tessera_system::bluetooth::BluetoothRadioObservation,
    ) {
        self.controls.observe(radio, observation);
    }

    fn finish_controls(
        &mut self,
        result: &Result<Vec<tessera_system::bluetooth::BluetoothRadioObservation>, BluetoothError>,
    ) -> Result<Vec<tessera_system::bluetooth::BluetoothControlledRadio>, BluetoothError> {
        self.controls.finish(result)
    }

    fn retire_controls(&mut self) {
        self.controls.retire();
    }

    fn initialize(&mut self) -> Result<(), BluetoothError> {
        // Result treats both S_OK and S_FALSE as success, each balanced once.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| native_error(error, "Bluetooth apartment initialization failed"))
    }

    fn uninitialize(&mut self) {
        unsafe { RoUninitialize() };
    }

    fn radios(&mut self) -> Result<Self::Radios, BluetoothError> {
        if self.controls.active() {
            return self.controls.enumerate().map(NativeRadios::Controlled);
        }
        let operation = Radio::GetRadiosAsync()
            .map_err(|error| native_error(error, "Bluetooth radio enumeration could not start"))?;
        // Both operation and returned collection remain on this request thread.
        // The operation drops here even when join fails or unwinds.
        operation
            .join()
            .map(NativeRadios::Passive)
            .map_err(|error| native_error(error, "Bluetooth radio enumeration failed"))
    }

    fn radio_count(&mut self, radios: &Self::Radios) -> Result<u32, BluetoothError> {
        match radios {
            NativeRadios::Passive(radios) => radios
                .Size()
                .map_err(|error| native_error(error, "Radio collection size is unavailable")),
            NativeRadios::Controlled(radios) => u32::try_from(radios.len()).map_err(|_| {
                BluetoothError::new(
                    BluetoothErrorKind::InvalidData,
                    "Bluetooth radio inventory exceeds the native collection limit",
                )
            }),
        }
    }

    fn radio(&mut self, radios: &Self::Radios, index: u32) -> Result<Radio, BluetoothError> {
        match radios {
            NativeRadios::Passive(radios) => radios
                .GetAt(index)
                .map_err(|error| native_error(error, "Radio observation is unavailable")),
            NativeRadios::Controlled(radios) => usize::try_from(index)
                .ok()
                .and_then(|index| radios.get(index))
                .cloned()
                .ok_or_else(|| {
                    BluetoothError::new(
                        BluetoothErrorKind::InvalidData,
                        "Bluetooth radio inventory position is unavailable",
                    )
                }),
        }
    }

    fn is_bluetooth(&mut self, radio: &Radio) -> Result<bool, BluetoothError> {
        radio
            .Kind()
            .map(|kind| kind == RadioKind::Bluetooth)
            .map_err(|error| native_error(error, "Radio kind is unavailable"))
    }

    fn radio_name(&mut self, radio: &Radio) -> Result<String, BluetoothError> {
        let name = radio
            .Name()
            .map_err(|error| native_error(error, "Radio name is unavailable"))?;
        text(&name)
    }

    fn radio_state(&mut self, radio: &Radio) -> Result<i32, BluetoothError> {
        radio
            .State()
            .map(|state| state.0)
            .map_err(|error| native_error(error, "Radio state is unavailable"))
    }

    fn selector(
        &mut self,
        transport: BluetoothTransport,
        paired: bool,
    ) -> Result<HSTRING, BluetoothError> {
        let result = match transport {
            BluetoothTransport::Classic => {
                BluetoothDevice::GetDeviceSelectorFromPairingState(paired)
            }
            BluetoothTransport::LowEnergy => {
                BluetoothLEDevice::GetDeviceSelectorFromPairingState(paired)
            }
        };
        result.map_err(|error| native_error(error, "Paired Bluetooth selector is unavailable"))
    }

    fn requested_properties(
        &mut self,
        names: &[&str],
    ) -> Result<Self::RequestedProperties, BluetoothError> {
        // Established stock iterable: no custom COM implementation or event owner.
        Ok(IIterable::<HSTRING>::from(
            names
                .iter()
                .map(|name| HSTRING::from(*name))
                .collect::<Vec<_>>(),
        ))
    }

    fn devices(
        &mut self,
        selector: &HSTRING,
        properties: &Self::RequestedProperties,
    ) -> Result<Self::Devices, BluetoothError> {
        let operation = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            selector,
            properties,
            DeviceInformationKind::AssociationEndpoint,
        )
        .map_err(|error| native_error(error, "Paired Bluetooth inventory could not start"))?;
        operation
            .join()
            .map_err(|error| native_error(error, "Paired Bluetooth inventory failed"))
    }

    fn device_count(&mut self, devices: &Self::Devices) -> Result<u32, BluetoothError> {
        devices
            .Size()
            .map_err(|error| native_error(error, "Paired inventory size is unavailable"))
    }

    fn device(
        &mut self,
        devices: &Self::Devices,
        index: u32,
    ) -> Result<Self::Device, BluetoothError> {
        devices
            .GetAt(index)
            .map_err(|error| native_error(error, "Paired device observation is unavailable"))
    }

    fn pairing(&mut self, device: &Self::Device) -> Result<Self::Pairing, BluetoothError> {
        device
            .Pairing()
            .map_err(|error| native_error(error, "Required device pairing data is unavailable"))
    }

    fn is_paired(&mut self, pairing: &Self::Pairing) -> Result<bool, BluetoothError> {
        pairing
            .IsPaired()
            .map_err(|error| native_error(error, "Required device pairing state is unavailable"))
    }

    fn device_id(&mut self, device: &Self::Device) -> Result<String, BluetoothError> {
        let id = device
            .Id()
            .map_err(|error| native_error(error, "Required device identity is unavailable"))?;
        text(&id)
    }

    fn device_name(&mut self, device: &Self::Device) -> Result<String, BluetoothError> {
        let name = device
            .Name()
            .map_err(|error| native_error(error, "Device name is unavailable"))?;
        text(&name)
    }

    fn properties(&mut self, device: &Self::Device) -> Result<Self::Properties, BluetoothError> {
        device
            .Properties()
            .map_err(|error| native_error(error, "Device connection properties are unavailable"))
    }

    fn connected_property(
        &mut self,
        properties: &Self::Properties,
        name: &str,
    ) -> Result<Option<IInspectable>, BluetoothError> {
        let key = HSTRING::from(name);
        if !properties
            .HasKey(&key)
            .map_err(|error| native_error(error, "Connection property presence is unavailable"))?
        {
            return Ok(None);
        }
        properties
            .Lookup(&key)
            .map(Some)
            .map_err(|error| native_error(error, "Connection property is unavailable"))
    }

    fn boolean(&mut self, value: &IInspectable) -> Result<bool, BluetoothError> {
        let property = value.cast::<IPropertyValue>().map_err(|error| {
            let mut error = native_error(error, "Connection property is not a property value");
            error.kind = BluetoothErrorKind::InvalidData;
            error
        })?;
        let kind = property
            .Type()
            .map_err(|error| native_error(error, "Connection property type is unavailable"))?;
        if kind != PropertyType::Boolean {
            return Err(BluetoothError::new(
                BluetoothErrorKind::InvalidData,
                "Connection property is not Boolean",
            ));
        }
        property
            .GetBoolean()
            .map_err(|error| native_error(error, "Connection property Boolean is unavailable"))
    }
}

fn text(value: &HSTRING) -> Result<String, BluetoothError> {
    // Do not silently change opaque IDs by replacing malformed UTF-16.
    String::from_utf16(value).map_err(|_| {
        BluetoothError::new(
            BluetoothErrorKind::InvalidData,
            "Bluetooth text contains invalid UTF-16",
        )
    })
}

pub(super) fn native_error(error: windows::core::Error, message: &'static str) -> BluetoothError {
    let code = error.code().0 as u32;
    let kind = match code {
        0x8007_0005 => BluetoothErrorKind::AccessDenied,
        0x8000_4001 | 0x8004_0154 | 0x8007_0032 => BluetoothErrorKind::Unsupported,
        0x8007_00aa => BluetoothErrorKind::Busy,
        0x8000_0013 | 0x8000_4004 => BluetoothErrorKind::Stopped,
        0x8007_0057 | 0x8000_4003 | 0x8000_000b | 0x8002_0005 | 0x8002_8ca0 => {
            BluetoothErrorKind::InvalidData
        }
        _ => BluetoothErrorKind::Unavailable,
    };
    // Static context avoids routine native error text leaking device identities.
    let mut result = BluetoothError::new(kind, message);
    result.native_code = Some(code);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::{Error, HRESULT};

    #[test]
    fn hresult_bits_and_error_categories_are_preserved() {
        for (code, expected) in [
            (0x8007_0005u32, BluetoothErrorKind::AccessDenied),
            (0x8004_0154, BluetoothErrorKind::Unsupported),
            (0x8007_00aa, BluetoothErrorKind::Busy),
            (0x8000_0013, BluetoothErrorKind::Stopped),
            (0x8007_0057, BluetoothErrorKind::InvalidData),
            (0x8000_ffff, BluetoothErrorKind::Unavailable),
        ] {
            let error = native_error(
                Error::from_hresult(HRESULT(code as i32)),
                "recorded failure",
            );
            assert_eq!(error.kind, expected);
            assert_eq!(error.native_code, Some(code));
        }
    }
}
