// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Tessera contributors.

/// Opaque 24-bit RGB source intent; generated presentation paint is never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourceSeed(u32);

impl SourceSeed {
    /// Product's default source, not a computed primary role.
    pub const GREEN: Self = Self(0x7ca45c);

    /// Rejects alpha bits or overflow instead of masking them into RGB.
    pub const fn from_rgb(rgb: u32) -> Option<Self> {
        if rgb <= 0xffffff {
            Some(Self(rgb))
        } else {
            None
        }
    }

    /// Returns the original opaque RGB intent without adding an alpha byte.
    pub const fn rgb(self) -> u32 {
        self.0
    }
}

impl Default for SourceSeed {
    fn default() -> Self {
        Self::GREEN
    }
}

#[cfg(test)]
mod tests {
    use super::SourceSeed;

    #[test]
    fn accepts_all_rgb_bounds_without_alpha_or_truncation() {
        for rgb in [0, 0x123456, 0xffffff] {
            assert_eq!(SourceSeed::from_rgb(rgb).unwrap().rgb(), rgb);
        }
        assert!(SourceSeed::from_rgb(0x1000000).is_none());
        assert!(SourceSeed::from_rgb(u32::MAX).is_none());
        assert_eq!(SourceSeed::default().rgb(), 0x7ca45c);
    }
}
