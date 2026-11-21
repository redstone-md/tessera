// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Example extension: counts presses of its own button, and shows how far
//! the count is on its way to the next ten.

use std::sync::atomic::{AtomicU32, Ordering};

wit_bindgen::generate!({
    world: "extension",
    path: "../../wit",
});

static CLICKS: AtomicU32 = AtomicU32::new(0);

struct Counter;

impl Guest for Counter {
    fn render(_widget: String) -> Vec<Node> {
        let clicks = CLICKS.load(Ordering::Relaxed);
        vec![
            Node {
                kind: NodeKind::Row,
                children: vec![1, 2, 3],
            },
            Node {
                kind: NodeKind::Text(format!("Clicks: {clicks}")),
                children: Vec::new(),
            },
            Node {
                kind: NodeKind::Progress((clicks % 10) as f32 / 10.),
                children: Vec::new(),
            },
            Node {
                kind: NodeKind::Button(Button {
                    label: "+1".to_owned(),
                    action: "increment".to_owned(),
                }),
                children: Vec::new(),
            },
        ]
    }

    fn activate(_widget: String, action: String) {
        if action == "increment" {
            let clicks = CLICKS.fetch_add(1, Ordering::Relaxed) + 1;
            log(&format!("button pressed {clicks} times"));
        }
    }

    fn fetched(_request: u32, _outcome: Result<Response, String>) {
        // This extension starts no requests.
    }
}

export!(Counter);
