//! Frontend-neutral geometry in terminal cells.

use serde::{Deserialize, Serialize};

/// A rectangle in cell coordinates. `x`/`y` are the top-left corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl SceneRect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// First column past the right edge.
    pub fn right(&self) -> u16 {
        self.x.saturating_add(self.width)
    }

    /// First row past the bottom edge.
    pub fn bottom(&self) -> u16 {
        self.y.saturating_add(self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Whether a cell coordinate lands inside this rect.
    pub fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x && column < self.right() && row >= self.y && row < self.bottom()
    }
}

/// A frontend surface size in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneSize {
    pub width: u16,
    pub height: u16,
}

impl SceneSize {
    pub fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}
