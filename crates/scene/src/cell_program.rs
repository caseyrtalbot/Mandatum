//! Renderer-neutral whole-frame cell paint program.
//!
//! The compiler turns semantic scene content into terminal-sized cells once.
//! Frontends translate the resulting glyphs, colors, modifiers, selection, and
//! cursor marks into their own paint types; they do not reimplement pane or
//! content presentation rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{SceneCellStyle, SceneRect, SceneSize, Theme, WorkspaceScene};

mod overlays;
mod panes;
mod primitives;
mod text_input;

pub use primitives::{display_width, scalar_range_to_columns};

/// What occupies one terminal-sized position in the cell program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CellOccupancy {
    /// Exactly one extended grapheme cluster in its leading grid cell.
    Grapheme(String),
    /// The cell is occupied by the leading glyph immediately before it.
    WideContinuation,
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
    PaneContent,
    Overlay,
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
        Self::grapheme(character.to_string(), style)
    }

    fn grapheme(grapheme: String, style: SceneCellStyle) -> Self {
        Self {
            occupancy: CellOccupancy::Grapheme(grapheme),
            style,
            selection: None,
            cursor: false,
            raster_layer: None,
        }
    }
}

/// Sparse whole-frame cell program containing only final topmost cells.
///
/// Later instructions at the same coordinate replace earlier ones while the
/// compiler runs. Storage is therefore bounded by the frame area even when
/// many opaque panes or overlays fully overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellProgram {
    size: SceneSize,
    /// `(row, column)` keeps iteration deterministic in row-major order.
    cells: BTreeMap<(u16, u16), ProgramCell>,
    /// Paint ownership follows the final topmost cell at each coordinate.
    paint_scopes: BTreeMap<(u16, u16), TextPaintScope>,
}

impl CellProgram {
    pub fn size(&self) -> SceneSize {
        self.size
    }

    /// The topmost compiled cell at whole-frame coordinates.
    pub fn cell_at(&self, x: u16, y: u16) -> Option<&ProgramCell> {
        self.cells.get(&(y, x))
    }

    /// Final topmost cells in deterministic row-major order.
    pub fn cells(&self) -> impl Iterator<Item = (u16, u16, &ProgramCell)> {
        self.cells.iter().map(|(&(y, x), cell)| (x, y, cell))
    }

    /// Text paint ownership of the final topmost cell at whole-frame
    /// coordinates.
    pub fn paint_scope_at(&self, x: u16, y: u16) -> Option<TextPaintScope> {
        self.cells
            .contains_key(&(y, x))
            .then(|| self.paint_scopes.get(&(y, x)).copied())
            .flatten()
    }

    /// Final topmost cells and their text-paint ownership in row-major order.
    pub fn scoped_cells(&self) -> impl Iterator<Item = (u16, u16, &ProgramCell, TextPaintScope)> {
        self.cells.iter().filter_map(|(&(y, x), cell)| {
            self.paint_scopes
                .get(&(y, x))
                .copied()
                .map(|scope| (x, y, cell, scope))
        })
    }
}

/// Compile every workspace surface into one renderer-neutral cell program.
pub fn compile_cell_program(scene: &WorkspaceScene, theme: &Theme) -> CellProgram {
    let mut compiler = Compiler {
        program: CellProgram {
            size: scene.size,
            cells: BTreeMap::new(),
            paint_scopes: BTreeMap::new(),
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
