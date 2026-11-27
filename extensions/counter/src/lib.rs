// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Example extension: counts presses of its own buttons, in a bar and in a
//! popup it brings along, and shows how far the count is on its way to the
//! next ten.

use std::sync::atomic::{AtomicU32, Ordering};

wit_bindgen::generate!({
    world: "extension",
    path: "../../wit",
});

static CLICKS: AtomicU32 = AtomicU32::new(0);

struct Counter;

/// A node without children.
fn leaf(kind: NodeKind) -> Node {
    Node {
        kind,
        children: Vec::new(),
    }
}

fn button(label: &str, action: &str) -> Node {
    leaf(NodeKind::Button(Button {
        label: label.to_owned(),
        action: action.to_owned(),
    }))
}

impl Guest for Counter {
    fn render(widget: String) -> Vec<Node> {
        let clicks = CLICKS.load(Ordering::Relaxed);
        let count = leaf(NodeKind::Text(format!("Clicks: {clicks}")));
        let progress = leaf(NodeKind::Progress((clicks % 10) as f32 / 10.));
        match widget.as_str() {
            "total" => vec![
                Node {
                    kind: NodeKind::Row,
                    children: vec![1],
                },
                count,
            ],
            "controls" => vec![
                Node {
                    kind: NodeKind::Column,
                    children: vec![1, 2, 3],
                },
                count,
                progress,
                Node {
                    kind: NodeKind::Row,
                    children: vec![4, 5, 6],
                },
                button("+1", "increment"),
                button("+10", "add-ten"),
                button("Reset", "reset"),
            ],
            _ => vec![
                Node {
                    kind: NodeKind::Row,
                    children: vec![1, 2, 3],
                },
                count,
                progress,
                button("+1", "increment"),
            ],
        }
    }

    fn activate(_widget: String, action: String) {
        let clicks = match action.as_str() {
            "increment" => CLICKS.fetch_add(1, Ordering::Relaxed) + 1,
            "add-ten" => CLICKS.fetch_add(10, Ordering::Relaxed) + 10,
            "reset" => {
                CLICKS.store(0, Ordering::Relaxed);
                0
            }
            _ => return,
        };
        log(&format!("the count is {clicks}"));
    }

    fn fetched(_request: u32, _outcome: Result<Response, String>) {
        // This extension starts no requests.
    }

    fn settings_changed() {
        // This extension reads no settings.
    }

    fn answer(_query: String) -> Vec<Answer> {
        // This extension answers no launcher queries.
        Vec::new()
    }

    fn run_answer(_id: String) {
        // This extension gives no answers to run.
    }

    fn shortcut(_id: String) {
        // This extension declares no shortcuts.
    }
}

export!(Counter);
