// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The production request owner is also the recording-adapter test surface.
//! Resources never escape these lexical query scopes into portable snapshots.
use super::Driver;
use std::marker::PhantomData;
use std::rc::Rc;
use tessera_system::bluetooth::{
    BluetoothConnectionState, BluetoothDeviceId, BluetoothError, BluetoothPairedDevice,
    BluetoothRadioObservation, BluetoothRadioState, BluetoothSnapshot, BluetoothTransport,
};

pub(super) const CONNECTED_PROPERTY: &str = "System.Devices.Aep.IsConnected";

// Native async operations are scoped within collection acquisition, not retained
// by this interface. Both native and recording adapters retire them on all exits.
pub(super) trait Calls {
    type Radios;
    type Radio;
    type Selector;
    type RequestedProperties;
    type Devices;
    type Device;
    type Pairing;
    type Properties;
    type Value;

    /// Success includes S_FALSE and must be balanced exactly once on this thread.
    fn initialize(&mut self) -> Result<(), BluetoothError>;
    fn uninitialize(&mut self);
    fn radios(&mut self) -> Result<Self::Radios, BluetoothError>;
    fn radio_count(&mut self, radios: &Self::Radios) -> Result<u32, BluetoothError>;
    fn radio(&mut self, radios: &Self::Radios, index: u32) -> Result<Self::Radio, BluetoothError>;
    fn is_bluetooth(&mut self, radio: &Self::Radio) -> Result<bool, BluetoothError>;
    fn radio_name(&mut self, radio: &Self::Radio) -> Result<String, BluetoothError>;
    fn radio_state(&mut self, radio: &Self::Radio) -> Result<i32, BluetoothError>;
    fn selector(
        &mut self,
        transport: BluetoothTransport,
        paired: bool,
    ) -> Result<Self::Selector, BluetoothError>;
    fn requested_properties(
        &mut self,
        names: &[&str],
    ) -> Result<Self::RequestedProperties, BluetoothError>;
    /// Enumerates AssociationEndpoint only; neither discovery nor device opening.
    fn devices(
        &mut self,
        selector: &Self::Selector,
        properties: &Self::RequestedProperties,
    ) -> Result<Self::Devices, BluetoothError>;
    fn device_count(&mut self, devices: &Self::Devices) -> Result<u32, BluetoothError>;
    fn device(
        &mut self,
        devices: &Self::Devices,
        index: u32,
    ) -> Result<Self::Device, BluetoothError>;
    fn pairing(&mut self, device: &Self::Device) -> Result<Self::Pairing, BluetoothError>;
    fn is_paired(&mut self, pairing: &Self::Pairing) -> Result<bool, BluetoothError>;
    fn device_id(&mut self, device: &Self::Device) -> Result<String, BluetoothError>;
    fn device_name(&mut self, device: &Self::Device) -> Result<String, BluetoothError>;
    fn properties(&mut self, device: &Self::Device) -> Result<Self::Properties, BluetoothError>;
    fn connected_property(
        &mut self,
        properties: &Self::Properties,
        name: &str,
    ) -> Result<Option<Self::Value>, BluetoothError>;
    /// A supplied non-Boolean value is InvalidData, never Disconnected/Unknown.
    fn boolean(&mut self, value: &Self::Value) -> Result<bool, BluetoothError>;
}

pub(super) struct Owner<C: Calls> {
    calls: C,
    // Enforce thread affinity even when a recording adapter is otherwise Send.
    _thread: PhantomData<Rc<()>>,
}

impl<C: Calls> Owner<C> {
    pub(super) fn new(mut calls: C) -> Result<Self, BluetoothError> {
        calls.initialize()?;
        Ok(Self {
            calls,
            _thread: PhantomData,
        })
    }

    fn read_radios(&mut self) -> Result<Vec<BluetoothRadioObservation>, BluetoothError> {
        let radios = self.calls.radios()?;
        let mut observations = Vec::new();
        for index in 0..self.calls.radio_count(&radios)? {
            let radio = self.calls.radio(&radios, index)?;
            if self.calls.is_bluetooth(&radio)? {
                let name = self.calls.radio_name(&radio)?;
                let state = match self.calls.radio_state(&radio)? {
                    1 => BluetoothRadioState::On,
                    2 => BluetoothRadioState::Off,
                    3 => BluetoothRadioState::Disabled,
                    _ => BluetoothRadioState::Unknown,
                };
                observations.push(BluetoothRadioObservation { name, state });
            }
        }
        Ok(observations)
    }

    fn read_devices(
        &mut self,
        transport: BluetoothTransport,
    ) -> Result<Vec<BluetoothPairedDevice>, BluetoothError> {
        // false would initiate wireless discovery. This request never does so.
        let selector = self.calls.selector(transport, true)?;
        let requested = self.calls.requested_properties(&[CONNECTED_PROPERTY])?;
        let devices = self.calls.devices(&selector, &requested)?;
        let mut observations = Vec::new();
        for index in 0..self.calls.device_count(&devices)? {
            let device = self.calls.device(&devices, index)?;
            let pairing = self.calls.pairing(&device)?;
            if !self.calls.is_paired(&pairing)? {
                // Only a confirmed unpaired race may exclude a row. Failures in
                // required pairing/identity data invalidate this inventory.
                continue;
            }
            let id = BluetoothDeviceId::new(self.calls.device_id(&device)?)?;
            let name = self.calls.device_name(&device)?;
            let properties = self.calls.properties(&device)?;
            let connection = match self
                .calls
                .connected_property(&properties, CONNECTED_PROPERTY)?
            {
                None => BluetoothConnectionState::Unknown,
                Some(value) => {
                    if self.calls.boolean(&value)? {
                        BluetoothConnectionState::Connected
                    } else {
                        BluetoothConnectionState::Disconnected
                    }
                }
            };
            observations.push(BluetoothPairedDevice {
                id,
                name,
                connection,
            });
        }
        Ok(observations)
    }
}

impl<C: Calls> Driver for Owner<C> {
    fn read(&mut self) -> BluetoothSnapshot {
        // No early propagation: each independent observation survives failures
        // in the other two queries. Successful empty radios do not prove absence.
        let radios = self.read_radios();
        let classic = self.read_devices(BluetoothTransport::Classic);
        let low_energy = self.read_devices(BluetoothTransport::LowEnergy);
        BluetoothSnapshot::new(radios, classic, low_energy)
    }
}

impl<C: Calls> Drop for Owner<C> {
    fn drop(&mut self) {
        // All SDK resources were scoped to query methods and already retired,
        // including during unwind. The adapter itself holds no SDK resources.
        self.calls.uninitialize();
    }
}
