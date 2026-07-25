//! Frontend-neutral geometry in terminal cells and logical pixels.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

const LOGICAL_UNITS_PER_PIXEL: f64 = 64.0;

/// Why neutral viewport metrics or fixed-point logical geometry were rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeometryError {
    NonFiniteLogicalValue,
    NegativeLogicalSize,
    LogicalValueOutOfRange,
    InvalidBackingScale,
    EmptyViewport,
    InvalidCellMetrics,
    IncoherentPhysicalSize,
}

impl fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NonFiniteLogicalValue => "logical geometry must be finite",
            Self::NegativeLogicalSize => "logical sizes must be non-negative",
            Self::LogicalValueOutOfRange => "logical geometry is out of fixed-point range",
            Self::InvalidBackingScale => "backing scale must be finite and greater than zero",
            Self::EmptyViewport => "viewport dimensions must be greater than zero",
            Self::InvalidCellMetrics => "measured cell dimensions must be greater than zero",
            Self::IncoherentPhysicalSize => {
                "logical size, physical size, and backing scale disagree by more than one pixel"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for GeometryError {}

/// A point in signed 1/64-logical-pixel fixed-point coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogicalPoint {
    x_units: i64,
    y_units: i64,
}

impl LogicalPoint {
    pub const fn from_units(x_units: i64, y_units: i64) -> Self {
        Self { x_units, y_units }
    }

    pub fn from_pixels(x: f64, y: f64) -> Result<Self, GeometryError> {
        Ok(Self {
            x_units: signed_logical_units(x)?,
            y_units: signed_logical_units(y)?,
        })
    }

    pub const fn x_units(self) -> i64 {
        self.x_units
    }

    pub const fn y_units(self) -> i64 {
        self.y_units
    }

    pub fn x_pixels(self) -> f64 {
        self.x_units as f64 / LOGICAL_UNITS_PER_PIXEL
    }

    pub fn y_pixels(self) -> f64 {
        self.y_units as f64 / LOGICAL_UNITS_PER_PIXEL
    }
}

/// A non-negative size in 1/64-logical-pixel fixed-point coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogicalSize {
    width_units: u64,
    height_units: u64,
}

impl LogicalSize {
    pub const fn from_units(width_units: u64, height_units: u64) -> Self {
        Self {
            width_units,
            height_units,
        }
    }

    pub fn from_pixels(width: f64, height: f64) -> Result<Self, GeometryError> {
        Ok(Self {
            width_units: unsigned_logical_units(width)?,
            height_units: unsigned_logical_units(height)?,
        })
    }

    pub const fn width_units(self) -> u64 {
        self.width_units
    }

    pub const fn height_units(self) -> u64 {
        self.height_units
    }

    pub fn width_pixels(self) -> f64 {
        self.width_units as f64 / LOGICAL_UNITS_PER_PIXEL
    }

    pub fn height_pixels(self) -> f64 {
        self.height_units as f64 / LOGICAL_UNITS_PER_PIXEL
    }

    pub const fn is_empty(self) -> bool {
        self.width_units == 0 || self.height_units == 0
    }
}

/// A half-open rectangle in deterministic logical-pixel coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LogicalRect {
    pub origin: LogicalPoint,
    pub size: LogicalSize,
}

impl LogicalRect {
    pub const fn new(origin: LogicalPoint, size: LogicalSize) -> Self {
        Self { origin, size }
    }

    pub const fn from_units(
        x_units: i64,
        y_units: i64,
        width_units: u64,
        height_units: u64,
    ) -> Self {
        Self::new(
            LogicalPoint::from_units(x_units, y_units),
            LogicalSize::from_units(width_units, height_units),
        )
    }

    pub fn right_units(self) -> i64 {
        self.origin
            .x_units
            .saturating_add_unsigned(self.size.width_units)
    }

    pub fn bottom_units(self) -> i64 {
        self.origin
            .y_units
            .saturating_add_unsigned(self.size.height_units)
    }

    pub const fn is_empty(self) -> bool {
        self.size.is_empty()
    }

    pub fn contains(self, point: LogicalPoint) -> bool {
        point.x_units >= self.origin.x_units
            && point.x_units < self.right_units()
            && point.y_units >= self.origin.y_units
            && point.y_units < self.bottom_units()
    }
}

/// Client surface dimensions in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// Positive finite backing scale with canonical floating-point bit identity.
///
/// The raw float exists only at construction/deserialization boundaries;
/// retained scene identity is the validated IEEE bit pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BackingScale(u64);

impl BackingScale {
    pub fn new(value: f64) -> Result<Self, GeometryError> {
        if !value.is_finite() || value <= 0.0 {
            return Err(GeometryError::InvalidBackingScale);
        }
        Ok(Self(value.to_bits()))
    }

    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl Serialize for BackingScale {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_f64(self.get())
    }
}

impl<'de> Deserialize<'de> for BackingScale {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(f64::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// One coherent shell-provided viewport snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportMetrics {
    pub logical_size: LogicalSize,
    pub physical_size: PhysicalSize,
    pub backing_scale: BackingScale,
    pub measured_cell_metrics: LogicalSize,
}

impl ViewportMetrics {
    pub fn new(
        logical_size: LogicalSize,
        physical_size: PhysicalSize,
        backing_scale: BackingScale,
        measured_cell_metrics: LogicalSize,
    ) -> Result<Self, GeometryError> {
        if logical_size.is_empty() || physical_size.width == 0 || physical_size.height == 0 {
            return Err(GeometryError::EmptyViewport);
        }
        if measured_cell_metrics.is_empty() {
            return Err(GeometryError::InvalidCellMetrics);
        }

        let scale = backing_scale.get();
        let expected_width = logical_size.width_pixels() * scale;
        let expected_height = logical_size.height_pixels() * scale;
        if (expected_width - f64::from(physical_size.width)).abs() > 1.0
            || (expected_height - f64::from(physical_size.height)).abs() > 1.0
        {
            return Err(GeometryError::IncoherentPhysicalSize);
        }

        Ok(Self {
            logical_size,
            physical_size,
            backing_scale,
            measured_cell_metrics,
        })
    }

    /// Compatibility metrics for cell-only frontends and fixtures.
    pub fn from_scene_size(size: SceneSize) -> Self {
        let logical_size =
            LogicalSize::from_units(u64::from(size.width) * 64, u64::from(size.height) * 64);
        Self {
            logical_size,
            physical_size: PhysicalSize::new(u32::from(size.width), u32::from(size.height)),
            backing_scale: BackingScale::new(1.0).expect("one is a valid backing scale"),
            measured_cell_metrics: LogicalSize::from_units(64, 64),
        }
    }

    /// The complete cell grid that fits inside the logical client area.
    pub fn scene_size(self) -> SceneSize {
        let columns = self.logical_size.width_units / self.measured_cell_metrics.width_units;
        let rows = self.logical_size.height_units / self.measured_cell_metrics.height_units;
        SceneSize::new(
            columns.min(u64::from(u16::MAX)) as u16,
            rows.min(u64::from(u16::MAX)) as u16,
        )
    }

    pub fn logical_rect_for_cells(self, rect: SceneRect) -> LogicalRect {
        let cell_width = self.measured_cell_metrics.width_units;
        let cell_height = self.measured_cell_metrics.height_units;
        LogicalRect::from_units(
            i64::from(rect.x).saturating_mul(cell_width as i64),
            i64::from(rect.y).saturating_mul(cell_height as i64),
            u64::from(rect.width).saturating_mul(cell_width),
            u64::from(rect.height).saturating_mul(cell_height),
        )
    }

    /// Map an admitted logical point to a zero-based cell inside `cell_rect`.
    pub fn logical_point_to_cell(
        self,
        cell_rect: SceneRect,
        point: LogicalPoint,
    ) -> Option<(u16, u16)> {
        let logical_rect = self.logical_rect_for_cells(cell_rect);
        if !logical_rect.contains(point) {
            return None;
        }
        let column = (point.x_units - logical_rect.origin.x_units) as u64
            / self.measured_cell_metrics.width_units;
        let row = (point.y_units - logical_rect.origin.y_units) as u64
            / self.measured_cell_metrics.height_units;
        Some((column as u16, row as u16))
    }
}

fn signed_logical_units(value: f64) -> Result<i64, GeometryError> {
    if !value.is_finite() {
        return Err(GeometryError::NonFiniteLogicalValue);
    }
    let scaled = value * LOGICAL_UNITS_PER_PIXEL;
    if scaled < i64::MIN as f64 || scaled > i64::MAX as f64 {
        return Err(GeometryError::LogicalValueOutOfRange);
    }
    Ok(scaled.round() as i64)
}

fn unsigned_logical_units(value: f64) -> Result<u64, GeometryError> {
    if !value.is_finite() {
        return Err(GeometryError::NonFiniteLogicalValue);
    }
    if value < 0.0 {
        return Err(GeometryError::NegativeLogicalSize);
    }
    let scaled = value * LOGICAL_UNITS_PER_PIXEL;
    if scaled > u64::MAX as f64 {
        return Err(GeometryError::LogicalValueOutOfRange);
    }
    Ok(scaled.round() as u64)
}

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
