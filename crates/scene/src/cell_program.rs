//! Renderer-neutral whole-frame cell paint program.
//!
//! The compiler turns semantic scene content into terminal-sized cells once.
//! Frontends translate the resulting glyphs, colors, modifiers, selection, and
//! cursor marks into their own paint types; they do not reimplement pane or
//! content presentation rules.

use std::collections::BTreeMap;

use crate::{SceneCellStyle, SceneSize, Theme, WorkspaceScene};

mod overlays;
mod panes;
mod primitives;

/// What occupies one terminal-sized position in the cell program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellOccupancy {
    Glyph(char),
    /// The cell is occupied by the leading glyph immediately before it.
    ///
    /// Current terminal surfaces do not yet emit continuation metadata. The
    /// explicit variant gives Phase 5 a truthful seam without changing the
    /// existing [`crate::SceneCell`] contract.
    WideContinuation,
}

/// Why a cell is selected. Terminal selection uses the theme's copy-selection
/// contract; item selection is already expressed by semantic row styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellSelection {
    Terminal,
    Item,
}

/// One renderer-neutral cell after scene cursor and selection semantics apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgramCell {
    pub occupancy: CellOccupancy,
    pub style: SceneCellStyle,
    pub selection: Option<CellSelection>,
    pub cursor: bool,
}

impl ProgramCell {
    fn glyph(character: char, style: SceneCellStyle) -> Self {
        Self {
            occupancy: CellOccupancy::Glyph(character),
            style,
            selection: None,
            cursor: false,
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
}

/// Compile every workspace surface into one renderer-neutral cell program.
pub fn compile_cell_program(scene: &WorkspaceScene, theme: &Theme) -> CellProgram {
    let mut compiler = Compiler {
        program: CellProgram {
            size: scene.size,
            cells: BTreeMap::new(),
        },
    };

    compiler.paint_header(scene, theme);
    for pane in &scene.panes {
        compiler.paint_pane(pane, theme);
    }
    compiler.paint_status(scene, theme);
    if let Some(overlay) = &scene.overlay {
        compiler.paint_overlay(overlay, theme);
    }

    compiler.program
}

struct Compiler {
    program: CellProgram,
}
