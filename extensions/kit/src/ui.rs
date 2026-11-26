// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A builder for the trees `render` returns.

use crate::{Button, Node, NodeKind};

/// A node of a [`Ui`], to add children to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Id(u32);

/// The content of a widget, built from the root down.
///
/// Each node is appended after its parent and numbered by its place in
/// the list, so every child comes after the node it is in, as Tessera
/// wants. Children are kept in the order they were added.
#[derive(Debug)]
pub struct Ui {
    nodes: Vec<Node>,
}

impl Ui {
    /// The root, which every tree has.
    pub const ROOT: Id = Id(0);

    /// A tree whose root is a row, which makes the widget a pill as the
    /// widgets of a bar are.
    pub fn bar() -> Self {
        Self::rooted(NodeKind::Row)
    }

    /// A tree whose root is a column, for a popup: it scrolls what does
    /// not fit.
    pub fn panel() -> Self {
        Self::rooted(NodeKind::Column)
    }

    /// A bar with a single line of less prominent text, for when there is
    /// nothing else to show.
    pub fn bar_caption(text: impl Into<String>) -> Self {
        let mut ui = Self::bar();
        ui.caption(Self::ROOT, text);
        ui
    }

    fn rooted(kind: NodeKind) -> Self {
        Self {
            nodes: vec![Node {
                kind,
                children: Vec::new(),
            }],
        }
    }

    /// Appends a node of `kind` to the children of `parent`.
    pub fn add(&mut self, parent: Id, kind: NodeKind) -> Id {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            kind,
            children: Vec::new(),
        });
        self.nodes[parent.0 as usize].children.push(id);
        Id(id)
    }

    /// A row in `parent`: inside a column as wide as the column, with its
    /// children spread apart.
    pub fn row(&mut self, parent: Id) -> Id {
        self.add(parent, NodeKind::Row)
    }

    /// A column in `parent`: inside a column, a card of its own.
    pub fn column(&mut self, parent: Id) -> Id {
        self.add(parent, NodeKind::Column)
    }

    pub fn text(&mut self, parent: Id, text: impl Into<String>) -> Id {
        self.add(parent, NodeKind::Text(text.into()))
    }

    pub fn caption(&mut self, parent: Id, text: impl Into<String>) -> Id {
        self.add(parent, NodeKind::Caption(text.into()))
    }

    /// A button that hands `action` to `activate` when pressed.
    pub fn button(
        &mut self,
        parent: Id,
        label: impl Into<String>,
        action: impl Into<String>,
    ) -> Id {
        let button = Button {
            label: label.into(),
            action: action.into(),
        };
        self.add(parent, NodeKind::Button(button))
    }

    /// A bar filled to `share`, which is kept between 0 and 1.
    pub fn progress(&mut self, parent: Id, share: f32) -> Id {
        let share = if share.is_finite() {
            share.clamp(0., 1.)
        } else {
            0.
        };
        self.add(parent, NodeKind::Progress(share))
    }

    /// The nodes in the order Tessera wants them, the root first.
    pub fn into_nodes(self) -> Vec<Node> {
        self.nodes
    }

    /// The text of every text, caption and button label, in order; for
    /// tests.
    pub fn words(&self) -> Vec<&str> {
        fn word(node: &Node) -> Option<&str> {
            match &node.kind {
                NodeKind::Text(text) | NodeKind::Caption(text) => Some(text.as_str()),
                NodeKind::Button(button) => Some(button.label.as_str()),
                _ => None,
            }
        }
        self.nodes.iter().filter_map(word).collect()
    }

    /// The actions of the buttons, in order; for tests.
    pub fn actions(&self) -> Vec<&str> {
        fn action(node: &Node) -> Option<&str> {
            match &node.kind {
                NodeKind::Button(button) => Some(button.action.as_str()),
                _ => None,
            }
        }
        self.nodes.iter().filter_map(action).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn children_follow_their_parents_in_order() {
        let mut ui = Ui::panel();
        let line = ui.row(Ui::ROOT);
        ui.text(line, "a");
        ui.caption(Ui::ROOT, "b");
        ui.button(line, "c", "go");
        let nodes = ui.into_nodes();
        assert_eq!(nodes[0].children, [1, 3]);
        assert_eq!(nodes[1].children, [2, 4]);
        for (at, node) in nodes.iter().enumerate() {
            assert!(node.children.iter().all(|&child| child as usize > at));
        }
    }

    #[test]
    fn progress_is_kept_between_empty_and_full() {
        let mut ui = Ui::bar();
        ui.progress(Ui::ROOT, 2.);
        ui.progress(Ui::ROOT, f32::NAN);
        let nodes = ui.into_nodes();
        assert!(matches!(nodes[1].kind, NodeKind::Progress(share) if share == 1.));
        assert!(matches!(nodes[2].kind, NodeKind::Progress(share) if share == 0.));
    }

    #[test]
    fn words_and_actions_are_listed() {
        let mut ui = Ui::bar_caption("idle");
        ui.button(Ui::ROOT, "Go", "go");
        assert_eq!(ui.words(), ["idle", "Go"]);
        assert_eq!(ui.actions(), ["go"]);
    }
}
