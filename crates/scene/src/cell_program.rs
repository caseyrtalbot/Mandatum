//! Renderer-neutral whole-frame cell paint program.
//!
//! The compiler turns semantic scene content into terminal-sized cells once.
//! Frontends translate the resulting glyphs, colors, modifiers, selection, and
//! cursor marks into their own paint types; they do not reimplement pane or
//! content presentation rules.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::{SceneCellStyle, SceneRect, SceneSize, Theme, WorkspaceScene};

mod overlays;
mod panes;
mod primitives;
mod text_input;

pub use primitives::{display_width, scalar_range_to_columns};

/// What occupies one terminal-sized position in the cell program.
///
/// Single-scalar graphemes — the overwhelming majority of terminal cells —
/// are stored inline; only multi-scalar clusters heap-allocate. Construct
/// through [`CellOccupancy::grapheme`] so single scalars always take the
/// inline form; [`PartialEq`] additionally compares `Char` and `Cluster` by
/// grapheme text so direct variant construction cannot break frame equality.
#[derive(Clone, Debug, Eq)]
pub enum CellOccupancy {
    /// Exactly one extended grapheme cluster made of one Unicode scalar,
    /// stored inline, in its leading grid cell.
    Char(char),
    /// One multi-scalar extended grapheme cluster in its leading grid cell.
    Cluster(String),
    /// The cell is occupied by the leading glyph immediately before it.
    WideContinuation,
}

impl CellOccupancy {
    /// One extended grapheme cluster, stored inline when it is a single
    /// Unicode scalar.
    pub fn grapheme(text: impl AsRef<str> + Into<String>) -> Self {
        let mut scalars = text.as_ref().chars();
        match (scalars.next(), scalars.next()) {
            (Some(character), None) => Self::Char(character),
            _ => Self::Cluster(text.into()),
        }
    }

    /// The grapheme text, viewed through `scratch` for the inline form.
    /// `None` for wide continuations, which contribute no glyph.
    pub fn grapheme_str<'a>(&'a self, scratch: &'a mut [u8; 4]) -> Option<&'a str> {
        match self {
            Self::Char(character) => Some(character.encode_utf8(scratch)),
            Self::Cluster(cluster) => Some(cluster.as_str()),
            Self::WideContinuation => None,
        }
    }
}

impl PartialEq for CellOccupancy {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Char(left), Self::Char(right)) => left == right,
            (Self::Cluster(left), Self::Cluster(right)) => left == right,
            (Self::Char(character), Self::Cluster(cluster))
            | (Self::Cluster(cluster), Self::Char(character)) => {
                let mut scratch = [0u8; 4];
                character.encode_utf8(&mut scratch) == cluster.as_str()
            }
            (Self::WideContinuation, Self::WideContinuation) => true,
            (Self::WideContinuation, _) | (_, Self::WideContinuation) => false,
        }
    }
}

/// Frozen serialized shape: the pre-split `Grapheme(String)`/`WideContinuation`
/// wire form, so persisted or cross-process scenes survive the inline-char
/// storage change.
#[derive(Serialize, Deserialize)]
#[serde(rename = "CellOccupancy")]
enum CellOccupancyWire<'a> {
    Grapheme(Cow<'a, str>),
    WideContinuation,
}

impl Serialize for CellOccupancy {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut scratch = [0u8; 4];
        let wire = match self {
            Self::Char(character) => {
                CellOccupancyWire::Grapheme(Cow::Borrowed(&*character.encode_utf8(&mut scratch)))
            }
            Self::Cluster(cluster) => CellOccupancyWire::Grapheme(Cow::Borrowed(cluster)),
            Self::WideContinuation => CellOccupancyWire::WideContinuation,
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CellOccupancy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match CellOccupancyWire::deserialize(deserializer)? {
            CellOccupancyWire::Grapheme(text) => Self::grapheme(text),
            CellOccupancyWire::WideContinuation => Self::WideContinuation,
        })
    }
}

/// Why a cell is selected. Terminal selection uses the theme's copy-selection
/// contract; item selection is already expressed by semantic row styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSelection {
    Terminal,
    Item,
}

/// Stable identity for one text-paint region in a compiled frame.
///
/// Identity, rather than geometry alone, prevents adjacent panes or semantic
/// surfaces with coincident clips from becoming one shaping run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPaintScopeId(u32);

impl TextPaintScopeId {
    pub fn get(self) -> u32 {
        self.0
    }
}

/// The semantic class of a renderer-neutral text-paint region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextPaintScopeKind {
    Header,
    Status,
    PaneChrome,
    /// Terminal-parity pane borders and compact focus marks. Native renderers
    /// consume typed pane materials instead of shaping these decoration glyphs.
    PaneDecoration,
    PaneContent,
    Overlay,
    /// Terminal-parity overlay borders. Native renderers consume the typed
    /// rounded overlay shell instead of shaping these box glyphs.
    OverlayDecoration,
    TextInput,
}

/// Identity and exact cell-coordinate clip for shaped text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPaintScope {
    pub id: TextPaintScopeId,
    pub kind: TextPaintScopeKind,
    pub clip: SceneRect,
}

/// One renderer-neutral cell after scene cursor and selection semantics apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramCell {
    pub occupancy: CellOccupancy,
    pub style: SceneCellStyle,
    pub selection: Option<CellSelection>,
    pub cursor: bool,
    /// Ready artifact pixels assigned to this final-topmost cell, identified by
    /// the artifact pane's draw index. Cell-only adapters ignore this marker.
    pub raster_layer: Option<u16>,
}

impl ProgramCell {
    fn glyph(character: char, style: SceneCellStyle) -> Self {
        Self {
            occupancy: CellOccupancy::Char(character),
            style,
            selection: None,
            cursor: false,
            raster_layer: None,
        }
    }
}

/// Whole-frame cell program containing only final topmost cells.
///
/// Later instructions at the same coordinate replace earlier ones while the
/// compiler runs. Storage is a flat row-major grid (`y * width + x`), so it
/// stays bounded by the frame area even when many opaque panes or overlays
/// fully overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellProgram {
    size: SceneSize,
    /// Row-major `y * width + x` slots; `None` marks an unpainted position.
    cells: Vec<Option<ProgramCell>>,
    /// Paint ownership follows the final topmost cell at each coordinate.
    paint_scopes: Vec<Option<TextPaintScope>>,
}

impl CellProgram {
    pub fn size(&self) -> SceneSize {
        self.size
    }

    /// Bounds-checked flat index of whole-frame coordinates.
    fn slot(&self, x: u16, y: u16) -> Option<usize> {
        (x < self.size.width && y < self.size.height)
            .then(|| usize::from(y) * usize::from(self.size.width) + usize::from(x))
    }

    fn coordinates(&self, slot: usize) -> (u16, u16) {
        let width = usize::from(self.size.width).max(1);
        ((slot % width) as u16, (slot / width) as u16)
    }

    /// The topmost compiled cell at whole-frame coordinates.
    pub fn cell_at(&self, x: u16, y: u16) -> Option<&ProgramCell> {
        self.slot(x, y).and_then(|slot| self.cells[slot].as_ref())
    }

    fn cell_mut(&mut self, x: u16, y: u16) -> Option<&mut ProgramCell> {
        self.slot(x, y).and_then(|slot| self.cells[slot].as_mut())
    }

    /// Store the final topmost cell and its paint ownership; positions outside
    /// the frame are ignored, matching the compiler's clipping guards.
    fn put_cell(&mut self, x: u16, y: u16, cell: ProgramCell, scope: TextPaintScope) {
        if let Some(slot) = self.slot(x, y) {
            self.cells[slot] = Some(cell);
            self.paint_scopes[slot] = Some(scope);
        }
    }

    fn clear_cell(&mut self, x: u16, y: u16) {
        if let Some(slot) = self.slot(x, y) {
            self.cells[slot] = None;
            self.paint_scopes[slot] = None;
        }
    }

    fn set_scope(&mut self, x: u16, y: u16, scope: TextPaintScope) {
        if let Some(slot) = self.slot(x, y) {
            self.paint_scopes[slot] = Some(scope);
        }
    }

    /// Final topmost cells in deterministic row-major order.
    pub fn cells(&self) -> impl Iterator<Item = (u16, u16, &ProgramCell)> {
        self.cells.iter().enumerate().filter_map(|(slot, cell)| {
            let cell = cell.as_ref()?;
            let (x, y) = self.coordinates(slot);
            Some((x, y, cell))
        })
    }

    /// Text paint ownership of the final topmost cell at whole-frame
    /// coordinates.
    pub fn paint_scope_at(&self, x: u16, y: u16) -> Option<TextPaintScope> {
        self.slot(x, y)
            .filter(|&slot| self.cells[slot].is_some())
            .and_then(|slot| self.paint_scopes[slot])
    }

    /// Final topmost cells and their text-paint ownership in row-major order.
    pub fn scoped_cells(&self) -> impl Iterator<Item = (u16, u16, &ProgramCell, TextPaintScope)> {
        self.cells
            .iter()
            .zip(self.paint_scopes.iter())
            .enumerate()
            .filter_map(|(slot, (cell, scope))| {
                let cell = cell.as_ref()?;
                let scope = (*scope)?;
                let (x, y) = self.coordinates(slot);
                Some((x, y, cell, scope))
            })
    }
}

/// Compile every workspace surface into one renderer-neutral cell program.
pub fn compile_cell_program(scene: &WorkspaceScene, theme: &Theme) -> CellProgram {
    let area = usize::from(scene.size.width) * usize::from(scene.size.height);
    let mut compiler = Compiler {
        program: CellProgram {
            size: scene.size,
            cells: vec![None; area],
            paint_scopes: vec![None; area],
        },
        active_scope: TextPaintScope {
            id: TextPaintScopeId(0),
            kind: TextPaintScopeKind::Header,
            clip: SceneRect::default(),
        },
        next_scope_id: 0,
    };

    compiler.begin_text_scope(TextPaintScopeKind::Header, scene.header.area);
    compiler.paint_header(scene, theme);
    for (draw_index, pane) in scene.panes.iter().enumerate() {
        compiler.paint_pane(pane, theme, u16::try_from(draw_index).ok());
    }
    compiler.begin_text_scope(TextPaintScopeKind::Status, scene.status.area);
    compiler.paint_status(scene, theme);
    if let Some(overlay) = &scene.overlay {
        compiler.paint_overlay(overlay, theme);
    }
    if let Some(text_input) = &scene.text_input {
        compiler.paint_text_input(text_input, theme);
    }

    compiler.program
}

struct Compiler {
    program: CellProgram,
    active_scope: TextPaintScope,
    next_scope_id: u32,
}

impl Compiler {
    fn begin_text_scope(&mut self, kind: TextPaintScopeKind, clip: SceneRect) -> TextPaintScope {
        let scope = TextPaintScope {
            id: TextPaintScopeId(self.next_scope_id),
            kind,
            clip: self.clipped_rect(clip),
        };
        self.next_scope_id = self.next_scope_id.saturating_add(1);
        self.active_scope = scope;
        scope
    }

    fn set_text_scope(&mut self, scope: TextPaintScope) {
        self.active_scope = scope;
    }
}
