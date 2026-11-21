// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Example extension: the charge of a Bluetooth device in a bar, and every
//! paired device in a popup it brings along. It shows what Tessera tells
//! it and changes nothing.

wit_bindgen::generate!({
    world: "extension",
    path: "../../wit",
});

struct DeviceBattery;

/// A node without children.
fn leaf(kind: NodeKind) -> Node {
    Node {
        kind,
        children: Vec::new(),
    }
}

fn text(text: impl Into<String>) -> Node {
    leaf(NodeKind::Text(text.into()))
}

/// `kind` holding `children`, as the list of nodes `render` returns.
fn holding(kind: NodeKind, children: Vec<Node>) -> Vec<Node> {
    let indices = (1..=children.len() as u32).collect();
    let mut nodes = vec![Node {
        kind,
        children: indices,
    }];
    nodes.extend(children);
    nodes
}

/// The message with a capital letter, as a line of its own.
fn sentence(message: &str) -> String {
    let mut letters = message.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// The device the bar shows: the one `wanted` names, or else a connected
/// one that reports a charge, or else any whose charge is known.
fn chosen<'a>(
    devices: &'a [BluetoothDevice],
    wanted: Option<&str>,
) -> Result<&'a BluetoothDevice, String> {
    if let Some(wanted) = wanted {
        let lowercase = wanted.to_lowercase();
        let named = |device: &&BluetoothDevice| device.name.to_lowercase().contains(&lowercase);
        return devices
            .iter()
            .find(named)
            .ok_or_else(|| format!("No paired device is called `{wanted}`"));
    }
    let charged = || devices.iter().filter(|device| device.battery.is_some());
    charged()
        .find(|device| device.connected)
        .or_else(|| charged().next())
        .ok_or_else(|| "No Bluetooth device reports its charge".to_owned())
}

/// The name of one device, its charge as a bar and as a number; the
/// number says so when the device is not connected and the charge is old.
fn charge() -> Vec<Node> {
    let wanted = setting("device").filter(|name| !name.trim().is_empty());
    let devices = match bluetooth_devices() {
        Ok(devices) => devices,
        Err(error) => return holding(NodeKind::Row, vec![text(sentence(&error))]),
    };
    let device = match chosen(&devices, wanted.as_deref().map(str::trim)) {
        Ok(device) => device,
        Err(message) => return holding(NodeKind::Row, vec![text(message)]),
    };
    let mut shown = vec![text(device.name.clone())];
    match device.battery {
        Some(percent) => {
            shown.push(leaf(NodeKind::Progress(f32::from(percent) / 100.)));
            // Of a device that is not connected, Windows knows an old charge.
            let was = if device.connected { "" } else { "was " };
            shown.push(text(format!("{was}{percent}%")));
        }
        None => shown.push(text("charge unknown")),
    }
    holding(NodeKind::Row, shown)
}

/// What is known of `device`, in a few words.
fn note(device: &BluetoothDevice) -> String {
    match (device.connected, device.battery) {
        (true, Some(percent)) => format!("Connected, {percent}%"),
        (true, None) => "Connected".to_owned(),
        (false, Some(percent)) => format!("Not connected, was {percent}%"),
        (false, None) => "Not connected".to_owned(),
    }
}

/// Every paired device with what is known of it.
fn all() -> Vec<Node> {
    let devices = match bluetooth_devices() {
        Ok(devices) => devices,
        Err(error) => return holding(NodeKind::Column, vec![text(sentence(&error))]),
    };
    if devices.is_empty() {
        return holding(
            NodeKind::Column,
            vec![text("No Bluetooth device is paired")],
        );
    }
    let lines = devices.iter().flat_map(|device| {
        [
            text(device.name.clone()),
            leaf(NodeKind::Caption(note(device))),
        ]
    });
    holding(NodeKind::Column, lines.collect())
}

impl Guest for DeviceBattery {
    fn render(widget: String) -> Vec<Node> {
        match widget.as_str() {
            "all" => all(),
            _ => charge(),
        }
    }

    fn activate(_widget: String, _action: String) {
        // This extension has no buttons.
    }

    fn fetched(_request: u32, _outcome: Result<Response, String>) {
        // This extension starts no requests.
    }
}

export!(DeviceBattery);
