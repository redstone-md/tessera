// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! A builder for the trees `render` returns.

use crate::{Button, Image, ImageSource, Node, NodeKind, Slider, TextInput, Toggle};

/// The most bytes the picture of an image may have.
pub const IMAGE_LIMIT: usize = 2 << 20;

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

    /// A picture of a PNG or JPEG file of at most 2 MiB ([`IMAGE_LIMIT`]),
    /// `height` logical pixels high, or as high as a line of text for 0.
    pub fn image(&mut self, parent: Id, bytes: Vec<u8>, height: f32) -> Id {
        self.picture(parent, ImageSource::Bytes(bytes), height)
    }

    /// A picture fetched from an `https://` address on a host the
    /// manifest lists under `permissions.network`.
    pub fn image_url(&mut self, parent: Id, url: impl Into<String>, height: f32) -> Id {
        self.picture(parent, ImageSource::Url(url.into()), height)
    }

    /// The icon of an application: the path of its program file or the
    /// AppUserModelID of a packaged one.
    pub fn app_icon(&mut self, parent: Id, app: impl Into<String>, height: f32) -> Id {
        self.picture(parent, ImageSource::App(app.into()), height)
    }

    fn picture(&mut self, parent: Id, source: ImageSource, height: f32) -> Id {
        let height = if height.is_finite() {
            height.clamp(0., 512.)
        } else {
            0.
        };
        self.add(parent, NodeKind::Image(Image { source, height }))
    }

    /// A character of the icon font of the shell, Segoe Fluent Icons.
    pub fn icon(&mut self, parent: Id, glyph: impl Into<String>) -> Id {
        self.add(parent, NodeKind::Icon(glyph.into()))
    }

    /// A field of one line, `id` made of a-z, 0-9 and `-`. It shows
    /// `value` when it is first drawn and when `value` differs from the
    /// one given before.
    pub fn text_input(
        &mut self,
        parent: Id,
        id: impl Into<String>,
        placeholder: impl Into<String>,
        value: impl Into<String>,
    ) -> Id {
        let input = TextInput {
            id: id.into(),
            placeholder: placeholder.into(),
            value: value.into(),
        };
        self.add(parent, NodeKind::TextInput(input))
    }

    /// A slider from `min` to `max`, at `value`, which is kept between
    /// them.
    pub fn slider(
        &mut self,
        parent: Id,
        id: impl Into<String>,
        min: f32,
        max: f32,
        value: f32,
    ) -> Id {
        let value = if value.is_finite() { value } else { min };
        let slider = Slider {
            id: id.into(),
            min,
            max,
            value: value.clamp(min.min(max), max.max(min)),
        };
        self.add(parent, NodeKind::Slider(slider))
    }

    /// A switch, on or off.
    pub fn toggle(&mut self, parent: Id, id: impl Into<String>, on: bool) -> Id {
        let toggle = Toggle { id: id.into(), on };
        self.add(parent, NodeKind::Toggle(toggle))
    }

    /// A column at most `max_height` logical pixels high that scrolls what
    /// does not fit.
    pub fn scroll(&mut self, parent: Id, max_height: f32) -> Id {
        let height = if max_height.is_finite() {
            max_height.clamp(1., 4000.)
        } else {
            4000.
        };
        self.add(parent, NodeKind::Scroll(height))
    }

    /// An empty space that pushes what comes after it to the end of its
    /// row or column.
    pub fn spacer(&mut self, parent: Id) -> Id {
        self.add(parent, NodeKind::Spacer)
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
    fn inputs_keep_their_numbers_usable() {
        let mut ui = Ui::panel();
        let list = ui.scroll(Ui::ROOT, f32::INFINITY);
        ui.slider(list, "volume", 0., 100., 140.);
        ui.slider(list, "level", 0., 1., f32::NAN);
        ui.image_url(list, "https://example.com/a.png", -3.);
        let nodes = ui.into_nodes();
        assert!(matches!(nodes[1].kind, NodeKind::Scroll(height) if height == 4000.));
        assert!(matches!(&nodes[2].kind, NodeKind::Slider(slider) if slider.value == 100.));
        assert!(matches!(&nodes[3].kind, NodeKind::Slider(slider) if slider.value == 0.));
        assert!(matches!(&nodes[4].kind, NodeKind::Image(image) if image.height == 0.));
        assert_eq!(nodes[1].children, [2, 3, 4]);
    }

    #[test]
    fn words_and_actions_are_listed() {
        let mut ui = Ui::bar_caption("idle");
        ui.button(Ui::ROOT, "Go", "go");
        assert_eq!(ui.words(), ["idle", "Go"]);
        assert_eq!(ui.actions(), ["go"]);
    }
}
