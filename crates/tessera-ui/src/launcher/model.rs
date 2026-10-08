// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Complete logical launcher metadata, independent of native row materialization.

use std::{collections::HashSet, rc::Rc};

use slint::{Image, Model, ModelNotify, ModelRc, ModelTracker, VecModel};

use crate::{
    PixelIcon,
    generated::{LaunchRow, LaunchTile},
    projection::AppProjection,
};

/// Immutable ordered metadata and identity authority; no Slint images are built.
#[derive(Default)]
pub(crate) struct LauncherInventory {
    applications: Vec<AppProjection>,
    keys: Vec<String>,
}

impl LauncherInventory {
    pub(crate) fn new(applications: Vec<AppProjection>) -> Self {
        let keys = applications.iter().map(|app| app.key.clone()).collect();
        Self { applications, keys }
    }

    pub(crate) fn keys(&self) -> &[String] {
        &self.keys
    }

    pub(crate) fn len(&self) -> usize {
        self.applications.len()
    }
}

type IconImageConverter = dyn Fn(Option<&PixelIcon>) -> Image;

/// Read-only presentation adapter: each request converts only its short row.
/// Images and visited rows are never retained here; the injected converter may
/// reuse the existing bounded icon cache. Replace the model for a new snapshot.
pub(crate) struct LauncherRows {
    inventory: Rc<LauncherInventory>,
    favorites: HashSet<String>,
    columns: usize,
    image_for: Box<IconImageConverter>,
    notify: ModelNotify,
}

impl LauncherRows {
    /// `columns` must be positive; zero is a programmer error.
    pub(crate) fn new(
        inventory: Rc<LauncherInventory>,
        favorites: Vec<String>,
        columns: usize,
        image_for: impl Fn(Option<&PixelIcon>) -> Image + 'static,
    ) -> Self {
        assert!(columns > 0, "launcher rows require positive columns");
        Self {
            inventory,
            favorites: favorites.into_iter().collect(),
            columns,
            image_for: Box::new(image_for),
            notify: ModelNotify::default(),
        }
    }
}

impl Model for LauncherRows {
    type Data = LaunchRow;

    fn row_count(&self) -> usize {
        self.inventory.len().div_ceil(self.columns)
    }

    fn row_data(&self, row: usize) -> Option<Self::Data> {
        if row >= self.row_count() {
            return None;
        }
        // The row bound guarantees this product lies within the inventory.
        let start = row * self.columns;
        let tiles = self.inventory.applications[start..]
            .iter()
            .take(self.columns)
            .map(|app| LaunchTile {
                key: app.key.as_str().into(),
                label: app.label.as_str().into(),
                favorite: self.favorites.contains(&app.key),
                icon: (self.image_for)(app.icon.as_ref()),
            })
            .collect::<Vec<_>>();
        Some(LaunchRow {
            tiles: ModelRc::new(VecModel::from(tiles)),
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &self.notify
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn applications(count: usize) -> Vec<AppProjection> {
        (0..count)
            .map(|index| AppProjection {
                key: format!("opaque:{index}"),
                label: format!("Label {index}"),
                pinned: true,
                icon: Some(
                    PixelIcon::new(1, 1, vec![(index & 255) as u8, (index >> 8) as u8, 0, 255])
                        .unwrap(),
                ),
            })
            .collect()
    }

    fn recording_rows(
        inventory: Rc<LauncherInventory>,
        favorites: Vec<String>,
        columns: usize,
        converted: Rc<RefCell<Vec<Option<usize>>>>,
    ) -> LauncherRows {
        LauncherRows::new(inventory, favorites, columns, move |icon| {
            converted.borrow_mut().push(icon.map(|icon| {
                let pixels = icon.rgba();
                usize::from(pixels[0]) | (usize::from(pixels[1]) << 8)
            }));
            Image::default()
        })
    }

    #[test]
    fn constructor_and_complete_identity_authority_do_not_convert_images() {
        let inventory = Rc::new(LauncherInventory::new(applications(1024)));
        let converted = Rc::new(RefCell::new(Vec::new()));
        let rows = recording_rows(inventory.clone(), vec![], 7, converted.clone());
        assert_eq!(inventory.len(), 1024);
        assert_eq!(
            inventory.keys(),
            (0..1024)
                .map(|index| format!("opaque:{index}"))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            inventory.keys().iter().position(|key| key == "opaque:1023"),
            Some(1023)
        );
        assert_eq!(rows.row_count(), 147);
        let _ = rows.model_tracker();
        assert!(rows.row_data(rows.row_count()).is_none());
        assert!(rows.row_data(usize::MAX).is_none());
        assert!(converted.borrow().is_empty());
    }

    #[test]
    fn requested_row_converts_only_its_chunk_and_keeps_exact_identity_and_favorites() {
        let mut apps = applications(1024);
        apps[450].icon = None;
        let inventory = Rc::new(LauncherInventory::new(apps));
        let converted = Rc::new(RefCell::new(Vec::new()));
        let rows = recording_rows(
            inventory,
            vec!["opaque:449".into(), "opaque:45".into(), "missing".into()],
            7,
            converted.clone(),
        );
        let row = rows.row_data(64).unwrap();
        assert_eq!(
            *converted.borrow(),
            [
                Some(448),
                Some(449),
                None,
                Some(451),
                Some(452),
                Some(453),
                Some(454)
            ]
        );
        assert_eq!(row.tiles.row_count(), 7);
        for column in 0..7 {
            let tile = row.tiles.row_data(column).unwrap();
            let index = 448 + column;
            assert_eq!(tile.key.as_str(), format!("opaque:{index}"));
            assert_eq!(tile.label.as_str(), format!("Label {index}"));
            assert_eq!(tile.favorite, index == 449);
        }
        assert_eq!(converted.borrow().len(), 7);
    }

    #[test]
    fn final_partial_row_has_no_padding_and_repeated_requests_are_not_memoized() {
        let inventory = Rc::new(LauncherInventory::new(applications(1024)));
        let converted = Rc::new(RefCell::new(Vec::new()));
        let rows = recording_rows(inventory, vec!["opaque:1023".into()], 7, converted.clone());
        for _ in 0..2 {
            let row = rows.row_data(146).unwrap();
            assert_eq!(row.tiles.row_count(), 2);
            assert_eq!(row.tiles.row_data(0).unwrap().key.as_str(), "opaque:1022");
            let tail = row.tiles.row_data(1).unwrap();
            assert_eq!(tail.key.as_str(), "opaque:1023");
            assert!(tail.favorite);
            assert!(row.tiles.row_data(2).is_none());
        }
        assert_eq!(
            *converted.borrow(),
            [Some(1022), Some(1023), Some(1022), Some(1023)]
        );
        assert!(rows.row_data(147).is_none());
        assert_eq!(converted.borrow().len(), 4);
    }

    #[test]
    fn empty_and_row_boundary_counts_do_not_request_images() {
        let empty = LauncherInventory::default();
        assert_eq!(empty.len(), 0);
        assert!(empty.keys().is_empty());
        for (count, expected_rows) in [
            (0, 0),
            (1, 1),
            (7, 1),
            (8, 2),
            (64, 10),
            (65, 10),
            (1024, 147),
        ] {
            let inventory = Rc::new(LauncherInventory::new(applications(count)));
            let rows = LauncherRows::new(inventory, vec![], 7, |_| {
                panic!("metadata must not request an image")
            });
            assert_eq!(rows.row_count(), expected_rows);
            assert!(rows.row_data(expected_rows).is_none());
        }
    }

    #[test]
    fn columns_larger_than_inventory_do_not_overflow_chunk_bounds() {
        let inventory = Rc::new(LauncherInventory::new(applications(2)));
        let converted = Rc::new(RefCell::new(Vec::new()));
        let rows = recording_rows(inventory, vec![], usize::MAX, converted.clone());
        assert_eq!(rows.row_count(), 1);
        assert_eq!(rows.row_data(0).unwrap().tiles.row_count(), 2);
        assert!(rows.row_data(1).is_none());
        assert!(rows.row_data(usize::MAX).is_none());
        assert_eq!(*converted.borrow(), [Some(0), Some(1)]);
    }

    #[test]
    #[should_panic(expected = "launcher rows require positive columns")]
    fn zero_columns_are_a_programmer_error() {
        LauncherRows::new(Rc::new(LauncherInventory::default()), vec![], 0, |_| {
            panic!("construction must not request an image")
        });
    }
}
