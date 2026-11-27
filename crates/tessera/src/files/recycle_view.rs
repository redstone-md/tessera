// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! The Recycle Bin as a place of the file manager, as Explorer shows it:
//! what lies in the bins of every drive, with where each item was deleted
//! from and when, and the bar of what can be done with them: bring the
//! chosen items back, bring every item back, delete the chosen ones for
//! good, empty the bin.
//!
//! Items are brought back through the Shell as one piece of work into the
//! undo journal, so that Ctrl+Z puts them into the bin again; deleting for
//! good and emptying ask first, as Explorer does, and cannot be taken
//! back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_channel::Receiver;
use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, prelude::FluentBuilder, px,
};

use super::model::{Entry, SortKey, SortKeys};
use crate::i18n::Language;
use crate::platform::{FileOperation, Outcome, RecycleBin, RecycledItem};
use crate::theme::Theme;
use crate::ui::icon;

/// Characters of the icon font.
const ICON_RESTORE: &str = "\u{e7a7}";
const ICON_RESTORE_ALL: &str = "\u{e777}";
const ICON_ERASE: &str = "\u{e74d}";
const ICON_EMPTY: &str = "\u{ef88}";

/// What the bin holds, by the path each entry of its listing has: where
/// the item lies in the bin, or a name of its own where it lies nowhere
/// the file system reaches.
#[derive(Debug, Default)]
pub struct Bin {
    items: HashMap<PathBuf, RecycledItem>,
}

impl Bin {
    /// The entries of `items` and what the bin knows of them. Blocks on
    /// the disk, to tell folders from files.
    pub fn read(items: Vec<RecycledItem>) -> (Vec<Entry>, Self) {
        let mut entries = Vec::with_capacity(items.len());
        let mut bin = Self::default();
        for (index, item) in items.into_iter().enumerate() {
            let path = { item.stored.clone() }
                .unwrap_or_else(|| PathBuf::from(format!("::recycled::{index}")));
            entries.push(entry_of(&item, path.clone()));
            bin.items.insert(path, item);
        }
        (entries, bin)
    }

    /// The items the entries at `paths` stand for, in their order.
    pub fn items(&self, paths: &[PathBuf]) -> Vec<RecycledItem> {
        { paths.iter() }
            .filter_map(|path| self.items.get(path).cloned())
            .collect()
    }

    /// Every item, as the bin listed them.
    pub fn all(&self) -> Vec<RecycledItem> {
        self.items.values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The folder the item of the entry at `path` was deleted from.
    pub fn original_folder(&self, path: &Path) -> Option<String> {
        let item = self.items.get(path)?;
        Some(item.original.parent()?.display().to_string())
    }

    /// What the entries are ordered by in the column of where they came
    /// from.
    pub fn keys(&self) -> SortKeys {
        { self.items.iter() }
            .map(|(path, item)| {
                let from = { item.original.parent() }
                    .map(|folder| folder.display().to_string())
                    .unwrap_or_default();
                let key = SortKey {
                    number: None,
                    text: from,
                };
                (path.clone(), key)
            })
            .collect()
    }
}

/// The entry of the item `item` of the bin, under `path`.
fn entry_of(item: &RecycledItem, path: PathBuf) -> Entry {
    let folder = item.stored.as_deref().is_some_and(Path::is_dir);
    let link = { Path::new(&item.name).extension() }.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("lnk") || extension.eq_ignore_ascii_case("url")
    });
    Entry {
        name: item.name.clone(),
        display: item.name.clone(),
        path,
        folder,
        size: item.size.unwrap_or_default(),
        // The date of the listing is when it was deleted.
        modified: item.deleted,
        link,
        ..Default::default()
    }
}

/// Brings `items` back to where they were deleted from, as one piece of
/// work in the undo journal; the Shell asks where a name there is taken.
pub fn restore(items: Vec<RecycledItem>) -> Receiver<Outcome> {
    let operation = { items.into_iter() }.fold(FileOperation::new(), |operation, item| {
        let to = item.original.clone();
        operation.restore(item.id().clone(), &to)
    });
    operation.start()
}

/// Deletes `items` for good, after the Shell asks.
pub fn erase(items: Vec<RecycledItem>) -> Receiver<Result<(), String>> {
    RecycleBin::erase(items, true)
}

/// Empties the bin, after the Shell asks.
pub fn empty() -> Receiver<Result<bool, String>> {
    RecycleBin::empty()
}

/// What the bar of the bin does when one of its buttons is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinDeed {
    Restore,
    RestoreAll,
    Erase,
    Empty,
}

impl BinDeed {
    const ALL: [Self; 4] = [Self::Restore, Self::RestoreAll, Self::Erase, Self::Empty];

    fn glyph(self) -> &'static str {
        match self {
            Self::Restore => ICON_RESTORE,
            Self::RestoreAll => ICON_RESTORE_ALL,
            Self::Erase => ICON_ERASE,
            Self::Empty => ICON_EMPTY,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Restore => "Restore the selected items",
            Self::RestoreAll => "Restore all items",
            Self::Erase => "Delete permanently",
            Self::Empty => "Empty the Recycle Bin",
        }
    }

    /// Whether it needs items chosen.
    fn needs_choice(self) -> bool {
        matches!(self, Self::Restore | Self::Erase)
    }
}

/// The bar over the list of the bin, with a button for each deed; those
/// that need a choice are dimmed without one, and all where the bin is
/// empty. `pressed` is called with the deed of the button pressed.
pub fn render_bar(
    theme: &Theme,
    language: Language,
    chosen: bool,
    empty: bool,
    pressed: impl Fn(BinDeed, &mut gpui::Window, &mut gpui::App) + Clone + 'static,
) -> AnyElement {
    let buttons = BinDeed::ALL.into_iter().map(|deed| {
        let enabled = !empty && (chosen || !deed.needs_choice());
        let color = match enabled {
            true => theme.colors.on_surface,
            false => theme.colors.on_surface_variant.opacity(0.45),
        };
        let pressed = pressed.clone();
        div()
            .id(("files-bin", deed as usize))
            .h(px(30.))
            .flex()
            .items_center()
            .gap(theme.spacing.small)
            .px(theme.spacing.small)
            .rounded(theme.shape.small)
            .text_color(color)
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.colors.surface_container_highest))
                    .on_click(move |_, window, cx| pressed(deed, window, cx))
            })
            .child(icon(theme, deed.glyph()))
            .child(language.say(deed.title()))
    });
    div()
        .flex_none()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(theme.spacing.tiny)
        .p(theme.spacing.small)
        .border_b_1()
        .border_color(theme.colors.outline.opacity(0.25))
        .text_size(theme.typography.label_size)
        .children(buttons)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_offers_what_needs_a_choice_only_with_one() {
        let needing: Vec<BinDeed> = { BinDeed::ALL.into_iter() }
            .filter(|deed| deed.needs_choice())
            .collect();
        assert_eq!(needing, [BinDeed::Restore, BinDeed::Erase]);
    }

    #[test]
    fn an_empty_bin_lists_nothing() {
        let (entries, bin) = Bin::read(Vec::new());
        assert!(entries.is_empty() && bin.is_empty());
        assert!(bin.keys().is_empty());
        assert_eq!(bin.original_folder(Path::new("C:\\$Recycle.Bin\\x")), None);
    }
}
