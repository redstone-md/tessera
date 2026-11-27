// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The zones of the editor of snap zones as a tree of splits, as the
//! custom tiling of KWin keeps them: a zone, or parts that lie side by
//! side or one above the other, each with its share of the room. A zone
//! is split in two, the line between two parts is moved, and the two
//! parts beside a line are merged into one zone. The tree is written as
//! the zones of a `[[snap.layout]]`, in shares of the work area, or as a
//! layout file of the tiling, whose containers are the same parts.

use super::Zone;

/// The least share of the work area a zone keeps, along each side.
pub const LEAST: f32 = 0.05;

/// How far apart two shares may be and still count as the same.
const SAME: f32 = 0.002;

/// How parts lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Side by side, with lines from the top to the bottom between them.
    Columns,
    /// One above the other.
    Rows,
}

/// A zone, or parts of the room each with its share of it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Split {
    #[default]
    Zone,
    Parts {
        axis: Axis,
        /// Each part with its share, the shares adding up to 1.
        parts: Vec<(f32, Split)>,
    },
}

/// A line between two parts, which is dragged to move it and clicked in
/// the editor to merge them.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// The way from the top of the tree to the parts it lies between.
    path: Vec<usize>,
    /// The part before the line; the one after it follows.
    after: usize,
    pub axis: Axis,
    /// Where it lies across the room, in shares of the work area, and
    /// where it begins and ends along it.
    pub at: f32,
    pub from: f32,
    pub to: f32,
}

impl Split {
    /// The zones of the tree in the order they are read, from the top
    /// left, each in shares of the work area.
    pub fn zones(&self) -> Vec<Zone> {
        let mut zones = Vec::new();
        self.walk(
            Zone::new(0., 0., 1., 1.),
            &mut Vec::new(),
            &mut |_, zone| zones.push(zone),
        );
        zones
    }

    /// The zones as `[[snap.layout]]` writes them.
    pub fn written(&self) -> Vec<[f32; 4]> {
        { self.zones().into_iter() }
            .map(|zone| [zone.x, zone.y, zone.width, zone.height])
            .collect()
    }

    /// Calls `seen` with the way to each zone and the zone, `room` being
    /// what this part of the tree covers.
    fn walk(&self, room: Zone, path: &mut Vec<usize>, seen: &mut impl FnMut(&[usize], Zone)) {
        match self {
            Self::Zone => seen(path, room),
            Self::Parts { axis, parts } => {
                let mut start = 0.;
                for (index, (share, part)) in parts.iter().enumerate() {
                    path.push(index);
                    part.walk(piece(room, *axis, start, *share), path, seen);
                    path.pop();
                    start += share;
                }
            }
        }
    }

    /// The lines between parts, each of every split of the tree.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        self.gather_lines(Zone::new(0., 0., 1., 1.), &mut Vec::new(), &mut lines);
        lines
    }

    fn gather_lines(&self, room: Zone, path: &mut Vec<usize>, lines: &mut Vec<Line>) {
        let Self::Parts { axis, parts } = self else {
            return;
        };
        let mut start = 0.;
        for (index, (share, part)) in parts.iter().enumerate() {
            path.push(index);
            part.gather_lines(piece(room, *axis, start, *share), path, lines);
            path.pop();
            start += share;
            if index + 1 < parts.len() {
                let (at, from, to) = match axis {
                    Axis::Columns => (room.x + room.width * start, room.y, room.y + room.height),
                    Axis::Rows => (room.y + room.height * start, room.x, room.x + room.width),
                };
                lines.push(Line {
                    path: path.clone(),
                    after: index,
                    axis: *axis,
                    at,
                    from,
                    to,
                });
            }
        }
    }

    /// The part at `path`, and the room it covers.
    fn at_path(&mut self, path: &[usize]) -> Option<(&mut Self, Zone)> {
        let mut node = self;
        let mut room = Zone::new(0., 0., 1., 1.);
        for &step in path {
            let Self::Parts { axis, parts } = node else {
                return None;
            };
            let start: f32 = parts.iter().take(step).map(|(share, _)| share).sum();
            let (share, part) = parts.get_mut(step)?;
            room = piece(room, *axis, start, *share);
            node = part;
        }
        Some((node, room))
    }

    /// The way to the zone at `index` in the order they are read.
    fn path_of(&self, index: usize) -> Option<Vec<usize>> {
        let mut found = None;
        let mut count = 0;
        self.walk(
            Zone::new(0., 0., 1., 1.),
            &mut Vec::new(),
            &mut |path, _| {
                if count == index {
                    found = Some(path.to_vec());
                }
                count += 1;
            },
        );
        found
    }

    /// Splits the zone at `index` in two along `axis`, where `at` lies
    /// across the work area (in shares of it); nothing where either half
    /// would be smaller than [`LEAST`]. Whether it was split is returned.
    pub fn split(&mut self, index: usize, axis: Axis, at: f32) -> bool {
        let Some(path) = self.path_of(index) else {
            return false;
        };
        let Some((_, room)) = self.at_path(&path) else {
            return false;
        };
        let (start, length) = along(room, axis);
        let local = (at - start) / length.max(f32::EPSILON);
        if (local * length) < LEAST || ((1. - local) * length) < LEAST {
            return false;
        }
        // A zone whose parts lie the same way gets one part more instead.
        if let Some((&last, parent_path)) = path.split_last()
            && let Some((Self::Parts { axis: outer, parts }, _)) = self.at_path(parent_path)
            && *outer == axis
        {
            let share = parts[last].0;
            parts[last].0 = share * local;
            parts.insert(last + 1, (share * (1. - local), Self::Zone));
            return true;
        }
        let Some((zone, _)) = self.at_path(&path) else {
            return false;
        };
        *zone = Self::Parts {
            axis,
            parts: vec![(local, Self::Zone), (1. - local, Self::Zone)],
        };
        true
    }

    /// Moves `line` to where `at` lies across the work area, as far as
    /// the parts on both sides keep [`LEAST`] of their side.
    pub fn drag(&mut self, line: &Line, at: f32) -> bool {
        let Some((Self::Parts { axis, parts }, room)) = self.at_path(&line.path) else {
            return false;
        };
        if line.after + 1 >= parts.len() || *axis != line.axis {
            return false;
        }
        let (start, length) = along(room, *axis);
        let length = length.max(f32::EPSILON);
        let before: f32 = parts.iter().take(line.after).map(|(share, _)| share).sum();
        let pair = parts[line.after].0 + parts[line.after + 1].0;
        let least = LEAST / length;
        if pair < least * 2. {
            return false;
        }
        let local = ((at - start) / length - before).clamp(least, pair - least);
        parts[line.after].0 = local;
        parts[line.after + 1].0 = pair - local;
        true
    }

    /// Makes the two parts beside `line` one zone. A split left with one
    /// part is that part.
    pub fn merge(&mut self, line: &Line) -> bool {
        let Some((node, _)) = self.at_path(&line.path) else {
            return false;
        };
        let Self::Parts { parts, .. } = node else {
            return false;
        };
        if line.after + 1 >= parts.len() {
            return false;
        }
        let (next, _) = parts.remove(line.after + 1);
        parts[line.after] = (parts[line.after].0 + next, Self::Zone);
        if parts.len() == 1 {
            let (_, only) = parts.remove(0);
            *node = only;
        }
        true
    }

    /// The tree whose zones are `zones`, where they fill the work area
    /// without gaps or overlaps and can be cut into it with straight
    /// lines from side to side; `None` otherwise.
    pub fn from_zones(zones: &[Zone]) -> Option<Self> {
        let rects: Vec<[f32; 4]> = { zones.iter() }
            .map(|zone| [zone.x, zone.y, zone.x + zone.width, zone.y + zone.height])
            .collect();
        let area: f32 = zones.iter().map(|zone| zone.width * zone.height).sum();
        if (area - 1.).abs() > SAME * 4. {
            return None;
        }
        cut(&rects, [0., 0., 1., 1.], 0)
    }

    /// The tree as a layout file of the tiling, named `name`: containers
    /// side by side (`horizontal`) or one above the other (`vertical`)
    /// with their shares as `grow`, a leaf for each zone, given windows in
    /// the order the zones are read, the last zone taking every window the
    /// others do not.
    pub fn tiling_file(&self, name: &str) -> String {
        let count = self.zones().len();
        let mut next = 0;
        let mut structure = self.tiling_part(count, &mut next);
        if !matches!(self, Self::Parts { .. }) {
            // A layout of one zone is one container for every window.
            structure = toml::Table::from_iter([("kind".to_owned(), "vertical".into())]);
        }
        let mut file = toml::Table::new();
        file.insert("name".to_owned(), name.into());
        file.insert("structure".to_owned(), toml::Value::Table(structure));
        let body = toml::to_string(&file).unwrap_or_default();
        format!(
            "# A tiling layout made with the editor of snap zones of Tessera.\n# Format: see tiling-layouts in docs/CONFIGURATION.md.\n{body}"
        )
    }

    fn tiling_part(&self, count: usize, next: &mut usize) -> toml::Table {
        let mut table = toml::Table::new();
        match self {
            Self::Zone => {
                *next += 1;
                let kind = match *next == count {
                    // The last zone holds the windows the others do not.
                    true => "vertical",
                    false => "leaf",
                };
                table.insert("kind".to_owned(), kind.into());
                table.insert("priority".to_owned(), (*next as i64).into());
            }
            Self::Parts { axis, parts } => {
                let kind = match axis {
                    Axis::Columns => "horizontal",
                    Axis::Rows => "vertical",
                };
                table.insert("kind".to_owned(), kind.into());
                let children: Vec<toml::Value> = { parts.iter() }
                    .map(|(share, part)| {
                        let mut child = part.tiling_part(count, next);
                        let grow = (f64::from(*share) * parts.len() as f64 * 1000.).round() / 1000.;
                        child.insert("grow".to_owned(), grow.max(0.001).into());
                        toml::Value::Table(child)
                    })
                    .collect();
                table.insert("children".to_owned(), toml::Value::Array(children));
            }
        }
        table
    }
}

/// The part of `room` that begins `start` of the way along `axis` and
/// takes `share` of it.
fn piece(room: Zone, axis: Axis, start: f32, share: f32) -> Zone {
    match axis {
        Axis::Columns => Zone::new(
            room.x + room.width * start,
            room.y,
            room.width * share,
            room.height,
        ),
        Axis::Rows => Zone::new(
            room.x,
            room.y + room.height * start,
            room.width,
            room.height * share,
        ),
    }
}

/// Where `room` begins along `axis` and how long it is there.
fn along(room: Zone, axis: Axis) -> (f32, f32) {
    match axis {
        Axis::Columns => (room.x, room.width),
        Axis::Rows => (room.y, room.height),
    }
}

/// The tree of the `rects` (left, top, right, bottom) within `room`, cut
/// with lines from side to side that no rect crosses; `depth` keeps it
/// from going on without end.
fn cut(rects: &[[f32; 4]], room: [f32; 4], depth: usize) -> Option<Split> {
    if depth > 16 || rects.is_empty() {
        return None;
    }
    if let [only] = rects {
        let fills = only
            .iter()
            .zip(room)
            .all(|(side, edge)| (side - edge).abs() <= SAME);
        return fills.then_some(Split::Zone);
    }
    for (axis, low, high) in [(Axis::Columns, 0, 2), (Axis::Rows, 1, 3)] {
        let mut cuts: Vec<f32> = { rects.iter() }
            .flat_map(|rect| [rect[low], rect[high]])
            .filter(|&at| at > room[low] + SAME && at < room[high] - SAME)
            .filter(|&at| {
                !rects
                    .iter()
                    .any(|rect| rect[low] < at - SAME && rect[high] > at + SAME)
            })
            .collect();
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|one, other| (*one - *other).abs() <= SAME);
        if cuts.is_empty() {
            continue;
        }
        let length = room[high] - room[low];
        let edges: Vec<f32> = { std::iter::once(room[low]) }
            .chain(cuts)
            .chain(std::iter::once(room[high]))
            .collect();
        let mut parts = Vec::new();
        for pair in edges.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            let inside: Vec<[f32; 4]> = { rects.iter() }
                .filter(|rect| {
                    let middle = (rect[low] + rect[high]) / 2.;
                    middle > start && middle < end
                })
                .copied()
                .collect();
            let mut piece = room;
            piece[low] = start;
            piece[high] = end;
            parts.push(((end - start) / length, cut(&inside, piece, depth + 1)?));
        }
        return Some(Split::Parts { axis, parts });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(one: &[[f32; 4]], other: &[[f32; 4]]) -> bool {
        one.len() == other.len()
            && one
                .iter()
                .zip(other)
                .all(|(a, b)| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.001))
    }

    #[test]
    fn a_zone_is_split_and_split_again_the_same_way_into_more_parts() {
        let mut tree = Split::Zone;
        assert!(tree.split(0, Axis::Columns, 0.5));
        assert!(close(
            &tree.written(),
            &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 1.]]
        ));
        // The right half split again side by side gives three columns.
        assert!(tree.split(1, Axis::Columns, 0.75));
        let Split::Parts { parts, .. } = &tree else {
            panic!("{tree:?}");
        };
        assert_eq!(parts.len(), 3);
        // The left one split the other way gives two rows in it.
        assert!(tree.split(0, Axis::Rows, 0.3));
        assert!(close(
            &tree.written(),
            &[
                [0., 0., 0.5, 0.3],
                [0., 0.3, 0.5, 0.7],
                [0.5, 0., 0.25, 1.],
                [0.75, 0., 0.25, 1.],
            ]
        ));
        // Too near an edge, nothing is split.
        assert!(!tree.split(3, Axis::Columns, 0.98));
        assert_eq!(tree.zones().len(), 4);
    }

    #[test]
    fn a_line_is_dragged_and_keeps_the_least_share() {
        let mut tree = Split::Zone;
        tree.split(0, Axis::Columns, 0.5);
        tree.split(1, Axis::Rows, 0.5);
        let lines = tree.lines();
        assert_eq!(lines.len(), 2);
        let middle = lines
            .iter()
            .find(|line| line.axis == Axis::Columns)
            .unwrap()
            .clone();
        assert!((middle.at - 0.5).abs() < 0.001);
        assert!(tree.drag(&middle, 0.3));
        assert!(close(
            &tree.written(),
            &[[0., 0., 0.3, 1.], [0.3, 0., 0.7, 0.5], [0.3, 0.5, 0.7, 0.5]]
        ));
        assert!(tree.drag(&middle, 0.));
        assert!((tree.zones()[0].width - LEAST).abs() < 0.001);
        let across = tree
            .lines()
            .into_iter()
            .find(|line| line.axis == Axis::Rows)
            .unwrap();
        assert!((across.from - LEAST).abs() < 0.001);
        assert!(tree.drag(&across, 0.8));
        assert!((tree.zones()[1].height - 0.8).abs() < 0.001);
    }

    #[test]
    fn the_parts_beside_a_line_are_merged() {
        let mut tree = Split::Zone;
        tree.split(0, Axis::Columns, 0.5);
        tree.split(1, Axis::Rows, 0.5);
        let rows = tree
            .lines()
            .into_iter()
            .find(|line| line.axis == Axis::Rows)
            .unwrap();
        assert!(tree.merge(&rows));
        assert!(close(
            &tree.written(),
            &[[0., 0., 0.5, 1.], [0.5, 0., 0.5, 1.]]
        ));
        let columns = tree.lines().remove(0);
        assert!(tree.merge(&columns));
        assert_eq!(tree, Split::Zone);
        assert!(tree.lines().is_empty());
    }

    #[test]
    fn zones_that_can_be_cut_make_a_tree_and_others_do_not() {
        let quarters = [
            Zone::new(0., 0., 0.5, 0.5),
            Zone::new(0.5, 0., 0.5, 0.5),
            Zone::new(0., 0.5, 0.5, 0.5),
            Zone::new(0.5, 0.5, 0.5, 0.5),
        ];
        let tree = Split::from_zones(&quarters).unwrap();
        assert_eq!(tree.zones().len(), 4);
        let mut written = tree.written();
        written.sort_by(|a, b| (a[1], a[0]).partial_cmp(&(b[1], b[0])).unwrap());
        assert!(close(
            &written,
            &quarters.map(|zone| [zone.x, zone.y, zone.width, zone.height])
        ));
        for zones in super::super::LAYOUTS {
            let tree = Split::from_zones(zones).unwrap();
            assert_eq!(tree.zones().len(), zones.len());
        }
        // A pinwheel cannot be cut with lines from side to side.
        let pinwheel = [
            Zone::new(0., 0., 0.6, 0.4),
            Zone::new(0.6, 0., 0.4, 0.6),
            Zone::new(0.4, 0.6, 0.6, 0.4),
            Zone::new(0., 0.4, 0.4, 0.6),
            Zone::new(0.4, 0.4, 0.2, 0.2),
        ];
        assert!(Split::from_zones(&pinwheel).is_none());
        // Nor can zones with a gap between them.
        assert!(
            Split::from_zones(&[Zone::new(0., 0., 0.4, 1.), Zone::new(0.5, 0., 0.5, 1.)]).is_none()
        );
    }

    #[test]
    fn the_tree_is_a_layout_file_the_tiling_reads() {
        let mut tree = Split::Zone;
        assert!(crate::tiling::check_layout_file(&tree.tiling_file("One")).is_ok());
        tree.split(0, Axis::Columns, 0.6);
        tree.split(1, Axis::Rows, 0.5);
        let text = tree.tiling_file("Mine");
        crate::tiling::check_layout_file(&text).unwrap_or_else(|error| panic!("{error}\n{text}"));
        assert!(text.contains("name = \"Mine\""), "{text}");
        assert!(text.contains("kind = \"horizontal\""), "{text}");
        assert!(text.contains("grow = 1.2"), "{text}");
        assert!(text.contains("kind = \"leaf\""), "{text}");
    }
}
