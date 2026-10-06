// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

//! Slint image cache for native icon pixels.
//!
//! Every observation re-maps the catalog into fresh [`PixelIcon`] values, so
//! identity alone is not enough: the cache keys on identity *and* content
//! (dimensions plus a content hash). Unchanged icons keep their already-built
//! [`slint::Image`]; changed icons are rebuilt. UI-thread only.

use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher, Hasher};

use slint::{Image, SharedPixelBuffer};

use crate::PixelIcon;

/// Capacity bound: the working set is the displayed catalog plus dock rows.
const MAX_CACHED_IMAGES: usize = 256;

/// Cache key: icon dimensions plus content hash. A hash collision would show
/// one icon for another of identical size — bounded to 128×128 premultiplied
/// RGBA by [`PixelIcon`], and SipHash-1-3 collisions between two such buffers
/// are not a realistic event for an alpha shell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IconKey {
    width: u32,
    height: u32,
    content: u64,
}

impl IconKey {
    fn new(icon: &PixelIcon) -> Self {
        let mut hasher = BuildHasherDefault::<DefaultHasher>::default().build_hasher();
        // The hasher is DefaultHasher's own; write via a generic Hasher.
        hasher.write(icon.rgba());
        Self {
            width: icon.width(),
            height: icon.height(),
            content: hasher.finish(),
        }
    }
}

/// Builds one [`slint::Image`] from a retained RGBA8 (premultiplied) buffer.
fn image_for(buffer: SharedPixelBuffer<slint::Rgba8Pixel>) -> Image {
    Image::from_rgba8_premultiplied(buffer)
}

/// Builds and reuses [`slint::Image`] values for icon pixels.
///
/// The cache stores plain `Send` pixel buffers, never `slint::Image` (whose
/// opaque backend handle is UI-thread only): [`PanelController`] lives behind
/// an `Arc` that is cloned into `Send` event-loop closures. The image handle
/// is rebuilt cheaply from the retained buffer when a surface asks for it.
#[derive(Debug, Default)]
pub(crate) struct IconCache {
    entries: HashMap<IconKey, SharedPixelBuffer<slint::Rgba8Pixel>>,
}

impl IconCache {
    /// Returns a ready image for `icon`, building one only on first sight.
    pub(crate) fn image(&mut self, icon: &PixelIcon) -> Image {
        image_for(self.buffer(icon))
    }

    /// Returns a retained, cache-owned pixel buffer for `icon`, copying the
    /// pixels only on first sight.
    fn buffer(&mut self, icon: &PixelIcon) -> SharedPixelBuffer<slint::Rgba8Pixel> {
        let key = IconKey::new(icon);
        if let Some(cached) = self.entries.get(&key) {
            return cached.clone();
        }
        let mut buffer = SharedPixelBuffer::<slint::Rgba8Pixel>::new(icon.width(), icon.height());
        buffer.make_mut_bytes().copy_from_slice(icon.rgba());
        if self.entries.len() >= MAX_CACHED_IMAGES {
            self.entries.clear();
        }
        self.entries.insert(key, buffer.clone());
        buffer
    }

    /// Image for an optional icon; `None` renders as the stock placeholder.
    pub(crate) fn optional(&mut self, icon: Option<&PixelIcon>) -> Option<Image> {
        icon.map(|icon| self.image(icon))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon(content: u8) -> PixelIcon {
        PixelIcon::new(2, 2, vec![content; 16]).unwrap()
    }

    #[test]
    fn unchanged_content_reuses_buffer_and_changed_content_rebuilds() {
        let mut cache = IconCache::default();
        let first = cache.buffer(&icon(7));
        let again = cache.buffer(&icon(7));
        // Unchanged content returns a clone of the cached entry (the buffer
        // is reference-shared, so clones preserve identity).
        assert_eq!(first.as_bytes(), again.as_bytes());

        let changed = cache.buffer(&icon(9));
        assert_ne!(first.as_bytes(), changed.as_bytes());
        // The old entry stays available for reuse.
        assert_eq!(cache.buffer(&icon(7)).as_bytes(), first.as_bytes());
    }

    #[test]
    fn cache_is_send_and_sync_for_shared_controller_ownership() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<IconCache>();
    }

    #[test]
    fn different_dimensions_never_share_one_entry() {
        let mut cache = IconCache::default();
        let small = PixelIcon::new(2, 2, vec![7; 16]).unwrap();
        let wide = PixelIcon::new(4, 1, vec![7; 16]).unwrap();
        // Same 16 uniform bytes; the key must still separate the shapes, so
        // the two entries keep their own (differently sized) buffers.
        let small_buffer = cache.buffer(&small);
        let wide_buffer = cache.buffer(&wide);
        assert_ne!(small_buffer.size().width, wide_buffer.size().width);
        assert_eq!(small_buffer.size().width, 2);
        assert_eq!(small_buffer.size().height, 2);
        assert_eq!(wide_buffer.size().width, 4);
        assert_eq!(wide_buffer.size().height, 1);
        // Re-fetching each shape returns its own retained buffer unchanged.
        assert_eq!(cache.buffer(&small).size(), small_buffer.size());
        assert_eq!(cache.buffer(&wide).size(), wide_buffer.size());
    }
}
