// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

use super::owner::{CONNECTED_PROPERTY, Calls, Owner};
use super::*;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use tessera_system::bluetooth::{
    BluetoothConnectionState, BluetoothRadioState, BluetoothTransport,
};

#[derive(Clone)]
struct RadioRow {
    bluetooth: bool,
    name: String,
    state: i32,
}

#[derive(Clone)]
enum Connection {
    Missing,
    Boolean(bool),
    WrongType,
}

#[derive(Clone)]
struct DeviceRow {
    id: String,
    name: String,
    paired: Result<bool, BluetoothError>,
    connection: Connection,
}

impl DeviceRow {
    fn paired(id: &str, connection: Connection) -> Self {
        Self {
            id: id.into(),
            name: "Same name".into(),
            paired: Ok(true),
            connection,
        }
    }
}

#[derive(Clone)]
struct Plan {
    // Native S_OK/S_FALSE both enter the same production Owner success path.
    init_status: &'static str,
    radios: Result<Vec<RadioRow>, BluetoothError>,
    classic: Result<Vec<DeviceRow>, BluetoothError>,
    low_energy: Result<Vec<DeviceRow>, BluetoothError>,
    fail_at: Option<&'static str>,
    panic_at: Option<&'static str>,
    panic_uninitialize: bool,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            init_status: "S_OK",
            radios: Ok(vec![RadioRow {
                bluetooth: true,
                name: "Radio".into(),
                state: 1,
            }]),
            classic: Ok(vec![DeviceRow::paired(
                "classic-id",
                Connection::Boolean(true),
            )]),
            low_energy: Ok(vec![DeviceRow::paired("le-id", Connection::Boolean(false))]),
            fail_at: None,
            panic_at: None,
            panic_uninitialize: false,
        }
    }
}

#[derive(Default)]
struct Trace {
    events: Vec<String>,
    live: usize,
}

type SharedTrace = Arc<Mutex<Trace>>;

struct Tracked<T> {
    data: T,
    name: &'static str,
    trace: SharedTrace,
}

impl<T> Drop for Tracked<T> {
    fn drop(&mut self) {
        let mut trace = self.trace.lock();
        trace.events.push(format!("release:{}", self.name));
        trace.live -= 1;
    }
}

struct RecordingCalls {
    plan: Plan,
    trace: SharedTrace,
}

fn denied() -> BluetoothError {
    let mut error = BluetoothError::new(BluetoothErrorKind::AccessDenied, "recorded denial");
    error.native_code = Some(0x8007_0005);
    error
}

impl RecordingCalls {
    fn step(&self, name: &'static str) -> Result<(), BluetoothError> {
        self.trace.lock().events.push(name.into());
        // Never panic under the recording mutex: resource Drop needs the same
        // trace while production unwinds this adapter call.
        if self.plan.panic_at == Some(name) {
            panic!("recorded SDK panic at {name}");
        }
        if self.plan.fail_at == Some(name) {
            return Err(denied());
        }
        Ok(())
    }

    fn resource<T>(&self, name: &'static str, data: T) -> Tracked<T> {
        let mut trace = self.trace.lock();
        trace.events.push(format!("acquire:{name}"));
        trace.live += 1;
        Tracked {
            data,
            name,
            trace: self.trace.clone(),
        }
    }
}

impl Calls for RecordingCalls {
    type Radios = Tracked<Vec<RadioRow>>;
    type Radio = Tracked<RadioRow>;
    type Selector = Tracked<BluetoothTransport>;
    type RequestedProperties = Tracked<()>;
    type Devices = Tracked<Vec<DeviceRow>>;
    type Device = Tracked<DeviceRow>;
    type Pairing = Tracked<Result<bool, BluetoothError>>;
    type Properties = Tracked<Connection>;
    type Value = Tracked<Connection>;

    fn initialize(&mut self) -> Result<(), BluetoothError> {
        self.step("initialize")?;
        self.trace.lock().events.push(self.plan.init_status.into());
        Ok(())
    }

    fn uninitialize(&mut self) {
        {
            let mut trace = self.trace.lock();
            let live = trace.live;
            trace.events.push(format!("uninitialize:live={live}"));
        }
        if self.plan.panic_uninitialize {
            panic!("recorded apartment retirement panic");
        }
    }

    fn radios(&mut self) -> Result<Self::Radios, BluetoothError> {
        self.step("radios-start")?;
        let _operation = self.resource("radio-operation", ());
        self.step("radios-join")?;
        Ok(self.resource("radios", self.plan.radios.clone()?))
    }

    fn radio_count(&mut self, radios: &Self::Radios) -> Result<u32, BluetoothError> {
        self.step("radio-count")?;
        Ok(radios.data.len() as u32)
    }

    fn radio(&mut self, radios: &Self::Radios, index: u32) -> Result<Self::Radio, BluetoothError> {
        self.step("radio")?;
        Ok(self.resource("radio", radios.data[index as usize].clone()))
    }

    fn is_bluetooth(&mut self, radio: &Self::Radio) -> Result<bool, BluetoothError> {
        self.step("radio-kind")?;
        Ok(radio.data.bluetooth)
    }

    fn radio_name(&mut self, radio: &Self::Radio) -> Result<String, BluetoothError> {
        self.step("radio-name")?;
        Ok(radio.data.name.clone())
    }

    fn radio_state(&mut self, radio: &Self::Radio) -> Result<i32, BluetoothError> {
        self.step("radio-state")?;
        Ok(radio.data.state)
    }

    fn selector(
        &mut self,
        transport: BluetoothTransport,
        paired: bool,
    ) -> Result<Self::Selector, BluetoothError> {
        self.trace
            .lock()
            .events
            .push(format!("selector:{transport:?}:paired={paired}"));
        assert!(paired, "false paired selector requests discovery");
        self.step("selector")?;
        Ok(self.resource("selector", transport))
    }

    fn requested_properties(
        &mut self,
        names: &[&str],
    ) -> Result<Self::RequestedProperties, BluetoothError> {
        assert_eq!(names, &[CONNECTED_PROPERTY]);
        self.step("requested-properties")?;
        Ok(self.resource("requested-properties", ()))
    }

    fn devices(
        &mut self,
        selector: &Self::Selector,
        _: &Self::RequestedProperties,
    ) -> Result<Self::Devices, BluetoothError> {
        self.step("devices-start:AssociationEndpoint")?;
        let _operation = self.resource("device-operation", ());
        self.step("devices-join")?;
        let rows = match selector.data {
            BluetoothTransport::Classic => self.plan.classic.clone()?,
            BluetoothTransport::LowEnergy => self.plan.low_energy.clone()?,
        };
        Ok(self.resource("devices", rows))
    }

    fn device_count(&mut self, devices: &Self::Devices) -> Result<u32, BluetoothError> {
        self.step("device-count")?;
        Ok(devices.data.len() as u32)
    }

    fn device(
        &mut self,
        devices: &Self::Devices,
        index: u32,
    ) -> Result<Self::Device, BluetoothError> {
        self.step("device")?;
        Ok(self.resource("device", devices.data[index as usize].clone()))
    }

    fn pairing(&mut self, device: &Self::Device) -> Result<Self::Pairing, BluetoothError> {
        self.step("pairing")?;
        Ok(self.resource("pairing", device.data.paired.clone()))
    }

    fn is_paired(&mut self, pairing: &Self::Pairing) -> Result<bool, BluetoothError> {
        self.step("is-paired")?;
        pairing.data.clone()
    }

    fn device_id(&mut self, device: &Self::Device) -> Result<String, BluetoothError> {
        self.step("device-id")?;
        Ok(device.data.id.clone())
    }

    fn device_name(&mut self, device: &Self::Device) -> Result<String, BluetoothError> {
        self.step("device-name")?;
        Ok(device.data.name.clone())
    }

    fn properties(&mut self, device: &Self::Device) -> Result<Self::Properties, BluetoothError> {
        self.step("properties")?;
        Ok(self.resource("properties", device.data.connection.clone()))
    }

    fn connected_property(
        &mut self,
        properties: &Self::Properties,
        name: &str,
    ) -> Result<Option<Self::Value>, BluetoothError> {
        assert_eq!(name, CONNECTED_PROPERTY);
        self.step("has-connected")?;
        if matches!(properties.data, Connection::Missing) {
            return Ok(None);
        }
        self.step("lookup-connected")?;
        Ok(Some(self.resource("value", properties.data.clone())))
    }

    fn boolean(&mut self, value: &Self::Value) -> Result<bool, BluetoothError> {
        self.step("boolean-cast")?;
        let _property = self.resource("property-value", ());
        self.step("boolean-type")?;
        if let Connection::Boolean(value) = value.data {
            self.step("boolean-value")?;
            Ok(value)
        } else {
            Err(BluetoothError::new(
                BluetoothErrorKind::InvalidData,
                "recorded wrong property type",
            ))
        }
    }
}

fn queued_host(plan: Plan, trace: SharedTrace) -> (NativeBluetoothHost, Arc<Mutex<Vec<Job>>>) {
    let jobs = Arc::new(Mutex::new(Vec::new()));
    let queue = jobs.clone();
    let host = NativeBluetoothHost {
        gate: FlightGate::default(),
        factory: Arc::new(move || {
            Ok(Box::new(Owner::new(RecordingCalls {
                plan: plan.clone(),
                trace: trace.clone(),
            })?))
        }),
        spawn: Arc::new(move |job| {
            queue.lock().push(job);
            Ok(())
        }),
    };
    (host, jobs)
}

fn run_next(jobs: &Mutex<Vec<Job>>) {
    let job = jobs.lock().remove(0);
    job();
}

fn observe(plan: Plan) -> (ReadResult, SharedTrace) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (host, jobs) = queued_host(plan, trace.clone());
    let result = Arc::new(Mutex::new(None));
    let recorded = result.clone();
    let retired = host.gate.clone();
    let callback_trace = trace.clone();
    host.read(Box::new(move |value| {
        assert!(
            retired.try_enter().is_some(),
            "flight must retire before callback"
        );
        callback_trace.lock().events.push("callback".into());
        assert!(recorded.lock().replace(value).is_none());
    }))
    .unwrap();
    run_next(&jobs);
    let result = result.lock().take().unwrap();
    (result, trace)
}

fn assert_retired(trace: &SharedTrace, initialized: bool) {
    let trace = trace.lock();
    assert_eq!(trace.live, 0, "resource trace: {:?}", trace.events);
    let uninitializations: Vec<_> = trace
        .events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.starts_with("uninitialize:"))
        .collect();
    assert_eq!(uninitializations.len(), usize::from(initialized));
    if let Some((position, event)) = uninitializations.first() {
        assert_eq!(event.as_str(), "uninitialize:live=0");
        assert!(
            !trace.events[position + 1..]
                .iter()
                .any(|event| event.starts_with("release:"))
        );
        assert!(
            *position
                < trace
                    .events
                    .iter()
                    .position(|event| event == "callback")
                    .unwrap()
        );
    }
    assert_eq!(
        trace
            .events
            .iter()
            .filter(|event| *event == "callback")
            .count(),
        1
    );
}

#[test]
fn paired_inventory_preserves_identity_blank_names_unknown_connection_and_all_radios() {
    let mut unpaired = DeviceRow::paired("", Connection::WrongType);
    unpaired.paired = Ok(false);
    let mut unnamed = DeviceRow::paired("other-id", Connection::Missing);
    unnamed.name.clear();
    let plan = Plan {
        radios: Ok(vec![
            RadioRow {
                bluetooth: false,
                name: "Wi-Fi".into(),
                state: 1,
            },
            RadioRow {
                bluetooth: true,
                name: "First".into(),
                state: 2,
            },
            RadioRow {
                bluetooth: true,
                name: "Second".into(),
                state: 77,
            },
            RadioRow {
                bluetooth: true,
                name: "Third".into(),
                state: 3,
            },
        ]),
        classic: Ok(vec![
            DeviceRow::paired("first-id", Connection::Boolean(true)),
            DeviceRow::paired("second-id", Connection::Boolean(false)),
            unnamed,
            unpaired,
        ]),
        ..Plan::default()
    };
    let (result, trace) = observe(plan);
    let snapshot = result.unwrap();
    let radios = snapshot.radios.unwrap();
    assert_eq!(radios.len(), 3);
    assert_eq!(
        radios.iter().map(|radio| radio.state).collect::<Vec<_>>(),
        vec![
            BluetoothRadioState::Off,
            BluetoothRadioState::Unknown,
            BluetoothRadioState::Disabled,
        ]
    );
    let rows = snapshot.classic.devices.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].id.as_str(), "first-id");
    assert_eq!(rows[1].id.as_str(), "second-id");
    assert_eq!(rows[0].name, rows[1].name);
    assert_eq!(rows[2].name, "");
    assert_eq!(
        rows.iter().map(|row| row.connection).collect::<Vec<_>>(),
        vec![
            BluetoothConnectionState::Connected,
            BluetoothConnectionState::Disconnected,
            BluetoothConnectionState::Unknown,
        ]
    );
    let events = trace.lock().events.clone();
    for transport in ["Classic", "LowEnergy"] {
        assert!(events.contains(&format!("selector:{transport}:paired=true")));
    }
    for forbidden in [
        "consent",
        "mutation",
        "discovery",
        "FromId",
        "GATT",
        "watcher",
    ] {
        assert!(!events.iter().any(|event| event.contains(forbidden)));
    }
    assert_retired(&trace, true);
}

#[test]
fn independent_failures_preserve_other_fields_and_empty_success() {
    for failed in 0..3 {
        let mut plan = Plan::default();
        match failed {
            0 => plan.radios = Err(denied()),
            1 => plan.classic = Err(denied()),
            _ => plan.low_energy = Err(denied()),
        }
        let (result, trace) = observe(plan);
        let snapshot = result.unwrap();
        assert_eq!(snapshot.radios.is_err(), failed == 0);
        assert_eq!(snapshot.classic.devices.is_err(), failed == 1);
        assert_eq!(snapshot.low_energy.devices.is_err(), failed == 2);
        assert_retired(&trace, true);
    }
    let (result, trace) = observe(Plan {
        radios: Ok(vec![]),
        classic: Ok(vec![]),
        low_energy: Err(denied()),
        ..Plan::default()
    });
    let snapshot = result.unwrap();
    assert_eq!(snapshot.radios, Ok(vec![]));
    assert_eq!(snapshot.classic.devices, Ok(vec![]));
    assert_eq!(snapshot.low_energy.devices, Err(denied()));
    assert_retired(&trace, true);
}

#[test]
fn malformed_required_data_and_wrong_connection_type_fail_the_inventory() {
    let mut failed_pairing = DeviceRow::paired("valid-id", Connection::Missing);
    failed_pairing.paired = Err(denied());
    for (row, kind) in [
        (
            DeviceRow::paired("", Connection::Missing),
            BluetoothErrorKind::InvalidData,
        ),
        (failed_pairing, BluetoothErrorKind::AccessDenied),
        (
            DeviceRow::paired("valid-id", Connection::WrongType),
            BluetoothErrorKind::InvalidData,
        ),
    ] {
        let (result, trace) = observe(Plan {
            classic: Ok(vec![row]),
            ..Plan::default()
        });
        let snapshot = result.unwrap();
        assert_eq!(snapshot.classic.devices.unwrap_err().kind, kind);
        assert!(snapshot.radios.is_ok());
        assert!(snapshot.low_energy.devices.is_ok());
        assert_retired(&trace, true);
    }
}

// Failure injection after every native resource-acquisition stage exercises the
// real Owner plus the real host admission/callback path, not a second state model.
const STAGES: &[&str] = &[
    "initialize",
    "radios-start",
    "radios-join",
    "radio-count",
    "radio",
    "radio-kind",
    "radio-name",
    "radio-state",
    "selector",
    "requested-properties",
    "devices-start:AssociationEndpoint",
    "devices-join",
    "device-count",
    "device",
    "pairing",
    "is-paired",
    "device-id",
    "device-name",
    "properties",
    "has-connected",
    "lookup-connected",
    "boolean-cast",
    "boolean-type",
    "boolean-value",
];

#[test]
fn resource_failure_at_every_stage_retires_before_apartment_flight_and_callback() {
    for &stage in STAGES {
        let (result, trace) = observe(Plan {
            fail_at: Some(stage),
            ..Plan::default()
        });
        if stage == "initialize" {
            assert_eq!(result.unwrap_err(), denied());
        } else {
            let snapshot = result.unwrap();
            assert!(
                snapshot.radios.is_err()
                    || snapshot.classic.devices.is_err()
                    || snapshot.low_energy.devices.is_err(),
                "{stage}"
            );
        }
        assert_retired(&trace, stage != "initialize");
    }
    for init_status in ["S_OK", "S_FALSE"] {
        let (result, trace) = observe(Plan {
            init_status,
            ..Plan::default()
        });
        assert!(result.is_ok());
        assert!(trace.lock().events.contains(&init_status.into()));
        assert_retired(&trace, true);
    }
}

#[test]
fn panic_at_every_stage_retires_native_resources_and_completes_once() {
    for &stage in STAGES {
        let (result, trace) = observe(Plan {
            panic_at: Some(stage),
            ..Plan::default()
        });
        assert_eq!(
            result.unwrap_err().kind,
            BluetoothErrorKind::Unavailable,
            "{stage}"
        );
        assert_retired(&trace, stage != "initialize");
    }
}

#[test]
fn busy_rejection_has_zero_callbacks_and_host_drop_does_not_join() {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (host, jobs) = queued_host(Plan::default(), trace.clone());
    let callbacks = Arc::new(AtomicUsize::new(0));
    let accepted = callbacks.clone();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        accepted.fetch_add(1, Ordering::SeqCst);
    }))
    .unwrap();
    let rejected = callbacks.clone();
    let concurrent = host.clone();
    let rejection = std::thread::spawn(move || {
        concurrent.read(Box::new(move |_| {
            rejected.fetch_add(100, Ordering::SeqCst);
        }))
    })
    .join()
    .unwrap();
    assert_eq!(rejection.unwrap_err().kind, BluetoothErrorKind::Busy);
    assert_eq!(jobs.lock().len(), 1);
    assert!(
        trace.lock().events.is_empty(),
        "factory must be worker-local"
    );
    drop(host);
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    run_next(&jobs);
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(trace.lock().live, 0);
}

#[test]
fn rejected_spawn_and_spawn_panic_retire_flight_without_callbacks() {
    for panic in [false, true] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let submissions = attempts.clone();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let (mut host, jobs) = queued_host(Plan::default(), trace);
        let normal_spawn = host.spawn.clone();
        host.spawn = Arc::new(move |job| {
            if submissions.fetch_add(1, Ordering::SeqCst) == 0 {
                if panic {
                    panic!("recorded spawn panic");
                }
                return Err(std::io::Error::other("recorded spawn failure"));
            }
            normal_spawn(job)
        });
        let callbacks = Arc::new(AtomicUsize::new(0));
        let rejected = callbacks.clone();
        assert_eq!(
            host.read(Box::new(move |_| {
                rejected.fetch_add(100, Ordering::SeqCst);
            }))
            .unwrap_err()
            .kind,
            BluetoothErrorKind::Unavailable
        );
        assert_eq!(callbacks.load(Ordering::SeqCst), 0);
        assert!(jobs.lock().is_empty());
        let accepted = callbacks.clone();
        host.read(Box::new(move |_| {
            accepted.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        run_next(&jobs);
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn completion_reentry_and_consumer_panic_cannot_strand_admission() {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let (host, jobs) = queued_host(Plan::default(), trace.clone());
    let reenter = host.clone();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let first = callbacks.clone();
    let second = callbacks.clone();
    host.read(Box::new(move |result| {
        assert!(result.is_ok());
        assert_eq!(trace.lock().events.last().unwrap(), "uninitialize:live=0");
        first.fetch_add(1, Ordering::SeqCst);
        reenter
            .read(Box::new(move |result| {
                assert!(result.is_ok());
                second.fetch_add(1, Ordering::SeqCst);
            }))
            .unwrap();
        panic!("recorded consumer panic after reentry");
    }))
    .unwrap();
    run_next(&jobs);
    assert_eq!(jobs.lock().len(), 1);
    run_next(&jobs);
    assert_eq!(callbacks.load(Ordering::SeqCst), 2);
    host.read(Box::new(|_| panic!("recorded consumer panic")))
        .unwrap();
    run_next(&jobs);
    assert!(host.gate.try_enter().is_some());
}

#[test]
fn separate_hosts_have_independent_flights_and_inline_spawn_can_reenter() {
    let (host, jobs) = queued_host(Plan::default(), Arc::new(Mutex::new(Trace::default())));
    let (other, other_jobs) = queued_host(Plan::default(), Arc::new(Mutex::new(Trace::default())));
    host.read(Box::new(|_| {})).unwrap();
    other.read(Box::new(|_| {})).unwrap();
    run_next(&jobs);
    run_next(&other_jobs);
    let mut inline = host;
    inline.spawn = Arc::new(|job| {
        job();
        Ok(())
    });
    let reenter = inline.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let first = calls.clone();
    let second = calls.clone();
    inline
        .read(Box::new(move |_| {
            first.fetch_add(1, Ordering::SeqCst);
            reenter
                .read(Box::new(move |_| {
                    second.fetch_add(1, Ordering::SeqCst);
                }))
                .unwrap();
        }))
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn teardown_panic_still_releases_flight_and_completes_once() {
    let (result, trace) = observe(Plan {
        panic_uninitialize: true,
        ..Plan::default()
    });
    assert_eq!(result.unwrap_err().kind, BluetoothErrorKind::Unavailable);
    assert_retired(&trace, true);
}
