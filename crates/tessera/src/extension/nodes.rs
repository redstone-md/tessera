// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The content an extension describes, checked before it is drawn: a
//! node that breaks a rule of the interface is left out and said why, so
//! that the thread that draws gets only what it can draw.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use super::access::Permissions;
use super::runtime::{ImageSource, Node, NodeKind};

/// The most nodes one widget may have; those after are left out.
pub const NODE_LIMIT: usize = 4000;

/// The most bytes the picture of an image may have.
pub const IMAGE_LIMIT: usize = 2 << 20;

/// The highest an image may be drawn, in logical pixels.
pub const IMAGE_HEIGHT: f32 = 512.;

/// The highest a scrolling column may be, in logical pixels.
pub const SCROLL_HEIGHT: f32 = 4000.;

/// The longest name of an input, a slider or a toggle.
const ID_LIMIT: usize = 64;

/// The longest text an icon node may have: a character or two.
const ICON_LIMIT: usize = 8;

/// The longest name of an application whose icon is shown.
const APP_LIMIT: usize = 1024;

/// What a widget shows, as the extension described it last.
#[derive(Clone, Debug, Default)]
pub struct Content {
    pub nodes: Vec<Node>,
    /// Whether each node is drawn; one that breaks a rule is not.
    pub shown: Vec<bool>,
    /// What tells the pictures of the image nodes with bytes apart, by
    /// node index.
    pub digests: HashMap<usize, u64>,
}

impl Content {
    /// `nodes` checked against the rules of the interface and the
    /// permissions of the extension, and why the nodes left out are.
    pub fn checked(mut nodes: Vec<Node>, permissions: &Permissions) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        if nodes.len() > NODE_LIMIT {
            problems.push(format!(
                "{} nodes are more than {NODE_LIMIT}; the rest are left out",
                nodes.len()
            ));
            nodes.truncate(NODE_LIMIT);
        }
        problems.extend(own_children(&mut nodes));
        let mut shown = vec![true; nodes.len()];
        let mut digests = HashMap::new();
        let mut ids = HashSet::new();
        for (index, node) in nodes.iter_mut().enumerate() {
            match check(&mut node.kind, permissions, &mut ids) {
                Ok(()) => {}
                Err(problem) => {
                    shown[index] = false;
                    problems.push(format!("node {index}: {problem}"));
                }
            }
            if let NodeKind::Image(image) = &node.kind
                && let ImageSource::Bytes(bytes) = &image.source
                && shown[index]
            {
                digests.insert(index, digest(bytes));
            }
        }
        let content = Self {
            nodes,
            shown,
            digests,
        };
        (content, problems)
    }

    /// Whether the node at `index` is there to be drawn.
    pub fn shows(&self, index: usize) -> bool {
        self.shown.get(index).copied().unwrap_or(false)
    }
}

/// Leaves every node at most one parent: a child is kept only where it
/// comes later in the list than its parent and no earlier parent claimed
/// it, so drawing the tree visits each node once and never loops. Says
/// which parents lost children.
fn own_children(nodes: &mut [Node]) -> Vec<String> {
    let count = nodes.len();
    let mut claimed = vec![false; count];
    let mut problems = Vec::new();
    for (index, node) in nodes.iter_mut().enumerate() {
        let before = node.children.len();
        node.children.retain(|&child| {
            let child = child as usize;
            let kept = child > index && child < count && !claimed[child];
            if kept {
                claimed[child] = true;
            }
            kept
        });
        let dropped = before - node.children.len();
        if dropped > 0 {
            problems.push(format!(
                "node {index}: {dropped} of its children are not later nodes of its own; they are left out here"
            ));
        }
    }
    problems
}

/// Fails, saying why, where `kind` breaks a rule; numbers out of their
/// range are moved into it. `ids` are the names of the inputs seen so far.
fn check(
    kind: &mut NodeKind,
    permissions: &Permissions,
    ids: &mut HashSet<String>,
) -> Result<(), String> {
    match kind {
        NodeKind::Row
        | NodeKind::Column
        | NodeKind::Text(_)
        | NodeKind::Caption(_)
        | NodeKind::Button(_)
        | NodeKind::Progress(_)
        | NodeKind::Spacer => Ok(()),
        NodeKind::Image(image) => {
            image.height = if image.height.is_finite() {
                image.height.clamp(0., IMAGE_HEIGHT)
            } else {
                0.
            };
            check_source(&image.source, permissions)
        }
        NodeKind::Icon(glyph) => match glyph.chars().count() {
            0 => Err("an icon has no character".to_owned()),
            _ if glyph.len() > ICON_LIMIT => Err("an icon is one character".to_owned()),
            _ => Ok(()),
        },
        NodeKind::TextInput(input) => unique(&input.id, ids),
        NodeKind::Toggle(toggle) => unique(&toggle.id, ids),
        NodeKind::Slider(slider) => {
            unique(&slider.id, ids)?;
            let (min, max) = (slider.min, slider.max);
            if !(min.is_finite() && max.is_finite() && min < max) {
                return Err(format!(
                    "the slider `{}` goes from {min} to {max}",
                    slider.id
                ));
            }
            slider.value = match slider.value.is_finite() {
                true => slider.value.clamp(min, max),
                false => min,
            };
            Ok(())
        }
        NodeKind::Scroll(height) => {
            *height = match height.is_finite() {
                true => height.clamp(1., SCROLL_HEIGHT),
                false => SCROLL_HEIGHT,
            };
            Ok(())
        }
    }
}

/// Fails unless `id` is a name of an input that no other has.
fn unique(id: &str, ids: &mut HashSet<String>) -> Result<(), String> {
    let named = !id.is_empty()
        && id.len() <= ID_LIMIT
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !named {
        return Err(format!(
            "`{id}` is not a name of up to {ID_LIMIT} of a-z, 0-9 and `-`"
        ));
    }
    match ids.insert(id.to_owned()) {
        true => Ok(()),
        false => Err(format!("another input is named `{id}` as well")),
    }
}

/// Fails unless the picture `source` names can be shown.
fn check_source(source: &ImageSource, permissions: &Permissions) -> Result<(), String> {
    match source {
        ImageSource::Bytes(bytes) => {
            if bytes.len() > IMAGE_LIMIT {
                return Err(format!(
                    "a picture of {} bytes is larger than {IMAGE_LIMIT}",
                    bytes.len()
                ));
            }
            match picture_kind(bytes) {
                Some(_) => Ok(()),
                None => Err("a picture is neither a PNG nor a JPEG file".to_owned()),
            }
        }
        ImageSource::Url(url) => {
            permissions
                .check_url(url)
                .and_then(|()| match url.starts_with("https://") {
                    true => Ok(()),
                    false => Err("a picture is fetched over https:// only".to_owned()),
                })
        }
        ImageSource::App(app) => match is_app(app) {
            true => Ok(()),
            false => Err(format!(
                "`{app}` is neither the path of a program file nor the AppUserModelID of a packaged application"
            )),
        },
    }
}

/// The kinds of picture files an image can be made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureKind {
    Png,
    Jpeg,
}

/// What kind of picture file `bytes` are, by how they start.
pub fn picture_kind(bytes: &[u8]) -> Option<PictureKind> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(PictureKind::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(PictureKind::Jpeg)
    } else {
        None
    }
}

/// Whether `app` names an application whose icon can be shown: a path on
/// a local drive (`C:\\` or `C:/`) that ends in `.exe`, or an AppUserModelID
/// of a packaged application, a package family and an application apart
/// by `!`. A network path is refused, since reading its icon would reach
/// another computer for an extension that may not.
pub fn is_app(app: &str) -> bool {
    if app.is_empty() || app.len() > APP_LIMIT {
        return false;
    }
    if let Some((family, name)) = app.split_once('!') {
        let plain = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        };
        return plain(family) && plain(name) && family.contains('_');
    }
    is_program_file(app)
}

/// Whether `app` is the path of a program file on a local drive.
fn is_program_file(app: &str) -> bool {
    let bytes = app.as_bytes();
    let on_drive = bytes.len() > 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    on_drive && app.to_ascii_lowercase().ends_with(".exe")
}

/// Whether `app` is the AppUserModelID of a packaged application.
pub fn is_packaged(app: &str) -> bool {
    app.contains('!') && is_app(app)
}

/// What tells one picture from another.
pub fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::{Image, Slider, TextInput, Toggle};

    fn node(kind: NodeKind) -> Node {
        Node {
            kind,
            children: Vec::new(),
        }
    }

    fn image(source: ImageSource, height: f32) -> Node {
        node(NodeKind::Image(Image { source, height }))
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nrest";

    #[test]
    fn pictures_are_png_or_jpeg_and_small_enough() {
        let permissions = Permissions::default();
        let mut big = PNG.to_vec();
        big.resize(IMAGE_LIMIT + 1, 0);
        let mut most = PNG.to_vec();
        most.resize(IMAGE_LIMIT, 0);
        let nodes = vec![
            node(NodeKind::Column),
            image(ImageSource::Bytes(PNG.to_vec()), 2000.),
            image(ImageSource::Bytes(vec![0xff, 0xd8, 0xff, 0xe0]), f32::NAN),
            image(ImageSource::Bytes(b"GIF89a".to_vec()), 10.),
            image(ImageSource::Bytes(big), 10.),
            image(ImageSource::Bytes(most), 10.),
        ];
        let (content, problems) = Content::checked(nodes, &permissions);
        assert_eq!(content.shown, [true, true, true, false, false, true]);
        assert_eq!(problems.len(), 2);
        assert!(
            matches!(&content.nodes[1].kind, NodeKind::Image(image) if image.height == IMAGE_HEIGHT)
        );
        assert!(matches!(&content.nodes[2].kind, NodeKind::Image(image) if image.height == 0.));
        assert_eq!(content.digests.len(), 3);
        assert_eq!(content.digests[&1], digest(PNG));
        assert!(!content.digests.contains_key(&3));
    }

    #[test]
    fn pictures_are_fetched_only_from_permitted_hosts() {
        let permissions: Permissions =
            toml::from_str("network = [\"img.example.com\"]\nlocal = [\"nas.local:80\"]").unwrap();
        let nodes = vec![
            image(
                ImageSource::Url("https://img.example.com/a.png".to_owned()),
                0.,
            ),
            image(
                ImageSource::Url("https://other.example.com/a.png".to_owned()),
                0.,
            ),
            image(ImageSource::Url("http://nas.local:80/a.png".to_owned()), 0.),
        ];
        let (content, _) = Content::checked(nodes, &permissions);
        assert_eq!(content.shown, [true, false, false]);
    }

    #[test]
    fn applications_are_named_by_program_file_or_package() {
        assert!(is_app(r"C:\Windows\notepad.exe"));
        assert!(is_app("c:/tools/App.EXE"));
        assert!(is_app("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        for wrong in [
            "",
            "notepad.exe",
            r"C:\Windows\notes.txt",
            "Calculator!App",
            "Microsoft_x!",
            "a_b!c d",
            "//attacker/share/a:/x.exe",
            r"\\?\UNC\host\share\x:\a.exe",
            r"\\host\share\x.exe",
            r"C:\",
            r"1:\x.exe",
        ] {
            assert!(!is_app(wrong), "{wrong}");
        }
    }

    #[test]
    fn inputs_are_named_once_and_sliders_have_a_range() {
        let input = |id: &str| {
            node(NodeKind::TextInput(TextInput {
                id: id.to_owned(),
                placeholder: String::new(),
                value: String::new(),
            }))
        };
        let slider = |id: &str, min: f32, max: f32, value: f32| {
            node(NodeKind::Slider(Slider {
                id: id.to_owned(),
                min,
                max,
                value,
            }))
        };
        let nodes = vec![
            node(NodeKind::Column),
            input("search"),
            input("search"),
            input("Search"),
            input(""),
            node(NodeKind::Toggle(Toggle {
                id: "wifi".to_owned(),
                on: true,
            })),
            slider("volume", 0., 10., 12.),
            slider("flat", 1., 1., 1.),
            slider("lost", 0., f32::INFINITY, 1.),
            slider("level", -1., 1., f32::NAN),
            node(NodeKind::Icon(String::new())),
            node(NodeKind::Icon("\u{e768}".to_owned())),
            node(NodeKind::Icon("not an icon".to_owned())),
            node(NodeKind::Scroll(-5.)),
            node(NodeKind::Scroll(f32::NAN)),
        ];
        let (content, problems) = Content::checked(nodes, &Permissions::default());
        assert_eq!(
            content.shown,
            [
                true, true, false, false, false, true, true, false, false, true, false, true,
                false, true, true
            ]
        );
        assert_eq!(problems.len(), 7, "{problems:?}");
        assert!(matches!(&content.nodes[6].kind, NodeKind::Slider(slider) if slider.value == 10.));
        assert!(matches!(&content.nodes[9].kind, NodeKind::Slider(slider) if slider.value == -1.));
        assert!(matches!(content.nodes[13].kind, NodeKind::Scroll(height) if height == 1.));
        assert!(
            matches!(content.nodes[14].kind, NodeKind::Scroll(height) if height == SCROLL_HEIGHT)
        );
        assert!(content.shows(1) && !content.shows(2) && !content.shows(99));
    }

    #[test]
    fn every_node_has_one_parent_that_comes_before_it() {
        let parent = |children: &[u32]| Node {
            kind: NodeKind::Column,
            children: children.to_vec(),
        };
        // 0 holds 1 and 2; 1 asks for 2 again, itself and one beyond the
        // end; 2 asks for 0 above it and 3.
        let nodes = vec![
            parent(&[1, 2]),
            parent(&[2, 1, 9]),
            parent(&[0, 3]),
            node(NodeKind::Spacer),
        ];
        let (content, problems) = Content::checked(nodes, &Permissions::default());
        let children: Vec<&[u32]> = content.nodes.iter().map(|n| &n.children[..]).collect();
        assert_eq!(children, [&[1, 2][..], &[], &[3], &[]]);
        assert_eq!(problems.len(), 2, "{problems:?}");
        // A chain where each node names the next two is drawn once each.
        let nodes = (0..NODE_LIMIT as u32)
            .map(|index| parent(&[index + 1, index + 2]))
            .collect();
        let (content, _) = Content::checked(nodes, &Permissions::default());
        let edges: usize = content.nodes.iter().map(|n| n.children.len()).sum();
        assert!(edges < NODE_LIMIT);
    }

    #[test]
    fn too_many_nodes_are_cut_off() {
        let nodes = vec![node(NodeKind::Spacer); NODE_LIMIT + 3];
        let (content, problems) = Content::checked(nodes, &Permissions::default());
        assert_eq!(content.nodes.len(), NODE_LIMIT);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn pictures_are_told_by_their_start() {
        assert_eq!(picture_kind(PNG), Some(PictureKind::Png));
        assert_eq!(
            picture_kind(&[0xff, 0xd8, 0xff, 0xdb]),
            Some(PictureKind::Jpeg)
        );
        assert_eq!(picture_kind(b"RIFF"), None);
        assert_eq!(picture_kind(b""), None);
    }
}
