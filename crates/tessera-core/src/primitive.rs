use crate::error::LayoutError;

/// An axis-aligned rectangle in physical pixels.
///
/// Invariants: nonzero width and height; `x + width` and `y + height` fit in
/// `i32` (so right/bottom edges are representable even for negative origins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl Rect {
    /// Creates a rectangle, validating extent and edge bounds.
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Result<Self, LayoutError> {
        if width == 0 || height == 0 {
            return Err(LayoutError::InvalidRect);
        }
        // `x + width` as i64 keeps the check total; negative x is fine because
        // a u32 extent shifted into i64 can only push the edge up to i32::MAX.
        let right = i64::from(x) + i64::from(width);
        let bottom = i64::from(y) + i64::from(height);
        if right > i64::from(i32::MAX) || bottom > i64::from(i32::MAX) {
            return Err(LayoutError::InvalidRect);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn x(&self) -> i32 {
        self.x
    }

    pub fn y(&self) -> i32 {
        self.y
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Right edge (`x + width`); always representable by construction.
    pub fn right(&self) -> i32 {
        self.x.saturating_add_unsigned(self.width)
    }

    /// Bottom edge (`y + height`); always representable by construction.
    pub fn bottom(&self) -> i32 {
        self.y.saturating_add_unsigned(self.height)
    }
}
