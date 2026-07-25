//! Pure cell-aware row-run planning and shaped-layout admission.
//!
//! This component consumes only the final renderer-neutral [`CellProgram`].
//! GPU buffers, font provisioning, palette materialization, and diagnostic
//! retention remain owned by their respective renderer seams.

use std::collections::BTreeSet;
use std::ops::Range;

use mandatum_scene::{
    CellOccupancy, CellProgram, CellSelection, LogicalRect, ProgramCell, SceneCellStyle, SceneRect,
    TextPaintScope,
};

use crate::NativeTextMetricIdentity;

/// Palette-resolved attributes that may affect shaped glyph output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedGlyphStyle {
    pub foreground: [u8; 4],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// One rich-text style range in a row run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowRunStyleRange {
    pub bytes: Range<usize>,
    pub style: ResolvedGlyphStyle,
}

/// Checked mapping from one complete input grapheme to its declared cells.
///
/// Cell ranges are relative to the run origin. Relative `u16` ranges allow a
/// full-width run without requiring an unrepresentable `u16::MAX + 1`
/// absolute endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteCellSpan {
    pub bytes: Range<usize>,
    pub cells: Range<u16>,
}

/// A maximal same-row, same-style shaping unit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowRun {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub text: String,
    pub style_ranges: Vec<RowRunStyleRange>,
    pub byte_cells: Vec<ByteCellSpan>,
    pub glyph_style: ResolvedGlyphStyle,
    pub source_style: SceneCellStyle,
    pub selection: Option<CellSelection>,
    pub cursor: bool,
    pub paint_scope: TextPaintScope,
    /// Native interface metric identity projected onto this complete shaping
    /// unit. Child-terminal rows retain `None` and use renderer settings.
    pub native_metrics: Option<NativeTextMetricIdentity>,
    /// Semantic line-box bounds and clip for app-owned interface text.
    /// Child-terminal rows retain `None` and remain cell-exact.
    pub native_geometry: Option<NativeTextGeometry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeTextGeometry {
    pub logical_rect: LogicalRect,
    pub clip: LogicalRect,
}

impl RowRun {
    /// Exact cell-coordinate text bound after intersecting the declared span
    /// with the scene compiler's paint-scope clip.
    pub fn clipped_cell_bounds(&self) -> Option<SceneRect> {
        intersect_rects(
            SceneRect::new(self.x, self.y, self.width, 1),
            self.paint_scope.clip,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRunBuildIssue {
    OrphanWideContinuation { x: u16, y: u16 },
    CellOutsidePaintClip { x: u16, y: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowRunBuildError {
    ByteLengthOverflow,
    RunWidthOverflow,
    InvalidSplitBoundary { boundary: usize, spans: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowRunPlan {
    pub runs: Vec<RowRun>,
    pub issues: Vec<RowRunBuildIssue>,
}

/// Convert final topmost cells into bounded shaping runs.
///
/// `resolve` is deliberately injected: terminal-palette materialization owns
/// color resolution, while this component owns only run grouping and cell
/// geometry.
pub fn build_row_runs(
    program: &CellProgram,
    mut resolve: impl FnMut(&ProgramCell) -> ResolvedGlyphStyle,
) -> Result<RowRunPlan, RowRunBuildError> {
    let cells = program
        .scoped_cells()
        .map(|(x, y, cell, scope)| RunInputCell {
            x,
            y,
            cell: cell.clone(),
            scope,
        })
        .collect::<Vec<_>>();
    build_from_cells(&cells, &mut resolve)
}

#[derive(Clone, Debug)]
struct RunInputCell {
    x: u16,
    y: u16,
    cell: ProgramCell,
    scope: TextPaintScope,
}

fn build_from_cells(
    cells: &[RunInputCell],
    resolve: &mut impl FnMut(&ProgramCell) -> ResolvedGlyphStyle,
) -> Result<RowRunPlan, RowRunBuildError> {
    let mut plan = RowRunPlan::default();
    let mut pending: Option<RowRun> = None;
    let mut index = 0usize;

    while let Some(input) = cells.get(index) {
        if !input.scope.clip.contains(input.x, input.y) {
            flush(&mut pending, &mut plan.runs);
            plan.issues.push(RowRunBuildIssue::CellOutsidePaintClip {
                x: input.x,
                y: input.y,
            });
            index += 1;
            continue;
        }

        let CellOccupancy::Grapheme(grapheme) = &input.cell.occupancy else {
            flush(&mut pending, &mut plan.runs);
            plan.issues.push(RowRunBuildIssue::OrphanWideContinuation {
                x: input.x,
                y: input.y,
            });
            index += 1;
            continue;
        };

        if !is_printable(input, grapheme) {
            flush(&mut pending, &mut plan.runs);
            index += 1;
            continue;
        }

        let next = cells.get(index + 1);
        let wide = next.is_some_and(|next| valid_wide_continuation(input, next));
        let decorated_space =
            grapheme == " " && (input.cell.style.underline || input.cell.style.strikethrough);
        let glyph_style = resolve(&input.cell);
        let candidate = new_run(input, grapheme, if wide { 2 } else { 1 }, glyph_style)?;

        if wide || decorated_space {
            flush(&mut pending, &mut plan.runs);
            plan.runs.push(candidate);
        } else if pending
            .as_ref()
            .is_some_and(|run| can_append(run, input, glyph_style))
        {
            append_run(pending.as_mut().expect("pending row run"), grapheme)?;
        } else {
            flush(&mut pending, &mut plan.runs);
            pending = Some(candidate);
        }

        index += if wide { 2 } else { 1 };
    }
    flush(&mut pending, &mut plan.runs);
    Ok(plan)
}

fn is_printable(input: &RunInputCell, grapheme: &str) -> bool {
    if input.cell.raster_layer.is_some()
        || input.cell.style.hidden
        || grapheme.is_empty()
        || grapheme == "\r"
        || grapheme == "\n"
    {
        return false;
    }
    grapheme != " " || input.cell.style.underline || input.cell.style.strikethrough
}

fn valid_wide_continuation(lead: &RunInputCell, next: &RunInputCell) -> bool {
    next.y == lead.y
        && lead.x.checked_add(1) == Some(next.x)
        && matches!(next.cell.occupancy, CellOccupancy::WideContinuation)
        && next.scope == lead.scope
        && next.scope.clip.contains(next.x, next.y)
        && next.cell.style == lead.cell.style
        && next.cell.selection == lead.cell.selection
        && next.cell.cursor == lead.cell.cursor
        && next.cell.raster_layer.is_none()
}

fn new_run(
    input: &RunInputCell,
    grapheme: &str,
    cell_width: u16,
    glyph_style: ResolvedGlyphStyle,
) -> Result<RowRun, RowRunBuildError> {
    let byte_end = 0usize
        .checked_add(grapheme.len())
        .ok_or(RowRunBuildError::ByteLengthOverflow)?;
    Ok(RowRun {
        x: input.x,
        y: input.y,
        width: cell_width,
        text: grapheme.to_owned(),
        style_ranges: vec![RowRunStyleRange {
            bytes: 0..byte_end,
            style: glyph_style,
        }],
        byte_cells: vec![ByteCellSpan {
            bytes: 0..byte_end,
            cells: 0..cell_width,
        }],
        glyph_style,
        source_style: input.cell.style,
        selection: input.cell.selection,
        cursor: input.cell.cursor,
        paint_scope: input.scope,
        native_metrics: None,
        native_geometry: None,
    })
}

fn can_append(run: &RowRun, input: &RunInputCell, glyph_style: ResolvedGlyphStyle) -> bool {
    run.y == input.y
        && run.x.checked_add(run.width) == Some(input.x)
        && run.paint_scope == input.scope
        && run.glyph_style == glyph_style
        && run.source_style == input.cell.style
        && run.selection == input.cell.selection
        && run.cursor == input.cell.cursor
}

fn append_run(run: &mut RowRun, grapheme: &str) -> Result<(), RowRunBuildError> {
    let byte_start = run.text.len();
    let byte_end = byte_start
        .checked_add(grapheme.len())
        .ok_or(RowRunBuildError::ByteLengthOverflow)?;
    let cell_start = run.width;
    let cell_end = cell_start
        .checked_add(1)
        .ok_or(RowRunBuildError::RunWidthOverflow)?;

    run.text.push_str(grapheme);
    run.width = cell_end;
    run.byte_cells.push(ByteCellSpan {
        bytes: byte_start..byte_end,
        cells: cell_start..cell_end,
    });
    if let Some(range) = run.style_ranges.last_mut() {
        range.bytes.end = byte_end;
    }
    Ok(())
}

fn flush(pending: &mut Option<RowRun>, runs: &mut Vec<RowRun>) {
    if let Some(run) = pending.take() {
        runs.push(run);
    }
}

fn intersect_rects(left: SceneRect, right: SceneRect) -> Option<SceneRect> {
    let x = left.x.max(right.x);
    let y = left.y.max(right.y);
    let right_edge = left.right().min(right.right());
    let bottom = left.bottom().min(right.bottom());
    (right_edge > x && bottom > y).then(|| SceneRect::new(x, y, right_edge - x, bottom - y))
}

/// Renderer-independent facts copied from a laid-out glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutGlyphFacts {
    pub bytes: Range<usize>,
    pub x: f32,
    pub advance: f32,
    pub rtl: bool,
}

/// Renderer-independent facts copied from one laid-out line.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutRunFacts {
    pub rtl: bool,
    pub line_width: f32,
    pub glyphs: Vec<LayoutGlyphFacts>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRunFallbackReason {
    BidiOrVisualReordering,
    NonMonotonicClusters,
    PartialCellCluster,
    AdvanceMismatch,
    MissingLayout,
    NonFiniteLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRunFallbackAction {
    SplitAtSpan(usize),
    AnchorAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowRunAdmission {
    Accepted,
    Fallback {
        reason: RowRunFallbackReason,
        action: RowRunFallbackAction,
    },
}

/// Validate shaped clusters against declared cell ownership.
///
/// `logical_to_physical_scale` converts the layout coordinates into physical
/// pixels. Every interval and total-width check uses a 0.5 physical-pixel
/// tolerance.
pub fn admit_layout(
    run: &RowRun,
    layout: &LayoutRunFacts,
    cell_width: f32,
    logical_to_physical_scale: f32,
) -> RowRunAdmission {
    if layout.rtl || layout.glyphs.iter().any(|glyph| glyph.rtl) {
        return anchored(RowRunFallbackReason::BidiOrVisualReordering);
    }
    if layout.glyphs.is_empty() {
        return anchored(RowRunFallbackReason::MissingLayout);
    }
    if !cell_width.is_finite()
        || cell_width <= 0.0
        || !logical_to_physical_scale.is_finite()
        || logical_to_physical_scale <= 0.0
        || !layout.line_width.is_finite()
    {
        return anchored(RowRunFallbackReason::NonFiniteLayout);
    }

    let groups = match cluster_groups(run, layout) {
        Ok(groups) => groups,
        Err(RowRunFallbackReason::PartialCellCluster) => {
            return fallback_at(run, RowRunFallbackReason::PartialCellCluster, None);
        }
        Err(reason) => return anchored(reason),
    };
    let mut previous_bytes_end = 0usize;
    let mut previous_cell_end = 0u16;
    let mut previous_visual_right = 0.0f32;
    for group in &groups {
        if group.bytes.start < previous_bytes_end
            || group.left + physical_tolerance(logical_to_physical_scale) < previous_visual_right
        {
            return anchored(RowRunFallbackReason::NonMonotonicClusters);
        }
        let Some((first, last)) = complete_span_interval(run, &group.bytes) else {
            return fallback_at(
                run,
                RowRunFallbackReason::PartialCellCluster,
                span_at_byte(run, group.bytes.start),
            );
        };
        if group.bytes.start != previous_bytes_end
            || run.byte_cells[first].cells.start != previous_cell_end
        {
            return fallback_at(run, RowRunFallbackReason::PartialCellCluster, Some(first));
        }
        let expected_left = f32::from(run.byte_cells[first].cells.start) * cell_width;
        let expected_right = f32::from(run.byte_cells[last].cells.end) * cell_width;
        if !within_half_physical_pixel(group.left - expected_left, logical_to_physical_scale)
            || !within_half_physical_pixel(group.right - expected_right, logical_to_physical_scale)
        {
            return fallback_at(run, RowRunFallbackReason::AdvanceMismatch, Some(first));
        }
        previous_bytes_end = group.bytes.end;
        previous_cell_end = run.byte_cells[last].cells.end;
        previous_visual_right = group.right;
    }
    if previous_bytes_end != run.text.len() || previous_cell_end != run.width {
        return fallback_at(
            run,
            RowRunFallbackReason::PartialCellCluster,
            span_at_byte(run, previous_bytes_end),
        );
    }

    let expected_width = f32::from(run.width) * cell_width;
    if !within_half_physical_pixel(
        layout.line_width - expected_width,
        logical_to_physical_scale,
    ) {
        return fallback_at(
            run,
            RowRunFallbackReason::AdvanceMismatch,
            run.byte_cells.len().checked_sub(1),
        );
    }
    RowRunAdmission::Accepted
}

#[derive(Clone, Debug)]
struct ClusterGroup {
    bytes: Range<usize>,
    left: f32,
    right: f32,
}

fn cluster_groups(
    run: &RowRun,
    layout: &LayoutRunFacts,
) -> Result<Vec<ClusterGroup>, RowRunFallbackReason> {
    let mut groups: Vec<ClusterGroup> = Vec::new();
    let mut closed = BTreeSet::new();
    for glyph in &layout.glyphs {
        if glyph.bytes.start >= glyph.bytes.end
            || glyph.bytes.end > run.text.len()
            || !run.text.is_char_boundary(glyph.bytes.start)
            || !run.text.is_char_boundary(glyph.bytes.end)
        {
            return Err(RowRunFallbackReason::PartialCellCluster);
        }
        if !glyph.x.is_finite() || !glyph.advance.is_finite() || glyph.advance < 0.0 {
            return Err(RowRunFallbackReason::NonFiniteLayout);
        }
        let right = glyph.x + glyph.advance;
        if !right.is_finite() {
            return Err(RowRunFallbackReason::NonFiniteLayout);
        }
        if let Some(group) = groups.last_mut()
            && group.bytes == glyph.bytes
        {
            group.left = group.left.min(glyph.x);
            group.right = group.right.max(right);
            continue;
        }
        if !closed.insert((glyph.bytes.start, glyph.bytes.end)) {
            return Err(RowRunFallbackReason::NonMonotonicClusters);
        }
        groups.push(ClusterGroup {
            bytes: glyph.bytes.clone(),
            left: glyph.x,
            right,
        });
    }
    Ok(groups)
}

fn complete_span_interval(run: &RowRun, bytes: &Range<usize>) -> Option<(usize, usize)> {
    let first = run
        .byte_cells
        .iter()
        .position(|span| span.bytes.start == bytes.start)?;
    let mut last = first;
    while run.byte_cells.get(last)?.bytes.end < bytes.end {
        let current = run.byte_cells.get(last)?;
        let next = run.byte_cells.get(last + 1)?;
        if current.bytes.end != next.bytes.start {
            return None;
        }
        last += 1;
    }
    (run.byte_cells.get(last)?.bytes.end == bytes.end).then_some((first, last))
}

fn span_at_byte(run: &RowRun, byte: usize) -> Option<usize> {
    run.byte_cells
        .iter()
        .position(|span| byte >= span.bytes.start && byte < span.bytes.end)
}

fn within_half_physical_pixel(delta: f32, scale: f32) -> bool {
    delta.abs() * scale <= 0.5
}

fn physical_tolerance(scale: f32) -> f32 {
    0.5 / scale
}

fn anchored(reason: RowRunFallbackReason) -> RowRunAdmission {
    RowRunAdmission::Fallback {
        reason,
        action: RowRunFallbackAction::AnchorAll,
    }
}

fn fallback_at(
    run: &RowRun,
    reason: RowRunFallbackReason,
    offending_span: Option<usize>,
) -> RowRunAdmission {
    let spans = run.byte_cells.len();
    let action = if spans <= 1 {
        RowRunFallbackAction::AnchorAll
    } else {
        let offending = offending_span.unwrap_or(0);
        let boundary = offending.clamp(1, spans - 1);
        RowRunFallbackAction::SplitAtSpan(boundary)
    };
    RowRunAdmission::Fallback { reason, action }
}

/// Split a rejected run at a complete grapheme boundary for another shaping
/// attempt.
pub fn split_at_span(run: &RowRun, boundary: usize) -> Result<(RowRun, RowRun), RowRunBuildError> {
    if boundary == 0 || boundary >= run.byte_cells.len() {
        return Err(RowRunBuildError::InvalidSplitBoundary {
            boundary,
            spans: run.byte_cells.len(),
        });
    }
    Ok((
        slice_run(run, 0..boundary)?,
        slice_run(run, boundary..run.byte_cells.len())?,
    ))
}

/// Ultimate fail-safe: one anchored run per declared grapheme span.
pub fn anchored_fallback_runs(run: &RowRun) -> Result<Vec<RowRun>, RowRunBuildError> {
    (0..run.byte_cells.len())
        .map(|index| slice_run(run, index..index + 1))
        .collect()
}

fn slice_run(run: &RowRun, spans: Range<usize>) -> Result<RowRun, RowRunBuildError> {
    let selected =
        run.byte_cells
            .get(spans.clone())
            .ok_or(RowRunBuildError::InvalidSplitBoundary {
                boundary: spans.start,
                spans: run.byte_cells.len(),
            })?;
    let first = selected
        .first()
        .ok_or(RowRunBuildError::InvalidSplitBoundary {
            boundary: spans.start,
            spans: run.byte_cells.len(),
        })?;
    let last = selected.last().expect("selected run spans are non-empty");
    let byte_start = first.bytes.start;
    let byte_end = last.bytes.end;
    let cell_start = first.cells.start;
    let cell_end = last.cells.end;
    let x = run
        .x
        .checked_add(cell_start)
        .ok_or(RowRunBuildError::RunWidthOverflow)?;
    let width = cell_end
        .checked_sub(cell_start)
        .ok_or(RowRunBuildError::RunWidthOverflow)?;
    let text = run
        .text
        .get(byte_start..byte_end)
        .ok_or(RowRunBuildError::ByteLengthOverflow)?
        .to_owned();
    let byte_cells = selected
        .iter()
        .map(|span| ByteCellSpan {
            bytes: span.bytes.start - byte_start..span.bytes.end - byte_start,
            cells: span.cells.start - cell_start..span.cells.end - cell_start,
        })
        .collect::<Vec<_>>();
    Ok(RowRun {
        x,
        y: run.y,
        width,
        style_ranges: vec![RowRunStyleRange {
            bytes: 0..text.len(),
            style: run.glyph_style,
        }],
        text,
        byte_cells,
        glyph_style: run.glyph_style,
        source_style: run.source_style,
        selection: run.selection,
        cursor: run.cursor,
        paint_scope: run.paint_scope,
        native_metrics: run.native_metrics,
        native_geometry: run.native_geometry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::{
        HeaderScene, PaneContent, PaneId, PaneScene, PaneSceneKind, SceneCell, SceneColor,
        SceneSize, StatusScene, SurfacePosition, TerminalSurface, TextPaintScopeKind, Theme,
        WorkspaceScene, compile_cell_program,
    };

    fn glyph_style(cell: &ProgramCell) -> ResolvedGlyphStyle {
        let foreground = match cell.style.foreground {
            SceneColor::Rgb(red, green, blue) => [red, green, blue, 255],
            _ => [220, 220, 224, 255],
        };
        ResolvedGlyphStyle {
            foreground,
            bold: cell.style.bold,
            italic: cell.style.italic,
            underline: cell.style.underline,
            strikethrough: cell.style.strikethrough,
        }
    }

    fn scene_for_surface(surface: TerminalSurface) -> WorkspaceScene {
        let pane_id = PaneId::new("row-run");
        let width = surface.rows.iter().map(Vec::len).max().unwrap_or(1) as u16 + 2;
        let pane_height = surface.rows.len() as u16 + 2;
        WorkspaceScene {
            size: SceneSize::new(width, pane_height + 2),
            header: HeaderScene {
                area: SceneRect::new(0, 0, width, 1),
                workspace_name: "test".to_owned(),
                project_name: "project".to_owned(),
                session_name: "test".to_owned(),
                pane_count: 1,
                focused_pane: pane_id.clone(),
                zoomed: false,
                connector_label: "none".to_owned(),
                text: String::new(),
                attention: Vec::new(),
            },
            panes: vec![PaneScene {
                id: pane_id.clone(),
                title: String::new(),
                kind: PaneSceneKind::Terminal,
                area: SceneRect::new(0, 1, width, pane_height),
                focused: false,
                floating: false,
                stacked: false,
                zoomed: false,
                content: PaneContent::Terminal(surface),
            }],
            overlay: None,
            status: StatusScene {
                area: SceneRect::new(0, pane_height + 1, width, 1),
                text: String::new(),
            },
            focused_pane: pane_id,
            hit_targets: Vec::new(),
            copy_mode: false,
            text_input: None,
            presentation: mandatum_scene::ScenePresentation::default(),
        }
    }

    fn program_for_rows(rows: Vec<Vec<SceneCell>>) -> CellProgram {
        compile_cell_program(
            &scene_for_surface(TerminalSurface {
                rows,
                ..TerminalSurface::default()
            }),
            &Theme::default(),
        )
    }

    fn content_run<'a>(plan: &'a RowRunPlan, text: &str) -> &'a RowRun {
        plan.runs
            .iter()
            .find(|run| run.paint_scope.kind == TextPaintScopeKind::PaneContent && run.text == text)
            .expect("expected pane-content row run")
    }

    #[test]
    fn row_run_joins_adjacent_same_style_cells_with_complete_maps() {
        let row = "fi->"
            .chars()
            .map(|character| SceneCell::grapheme(character.to_string(), SceneCellStyle::default()))
            .collect();
        let plan = build_row_runs(&program_for_rows(vec![row]), glyph_style).unwrap();
        let run = content_run(&plan, "fi->");

        assert_eq!(run.width, 4);
        assert_eq!(
            run.byte_cells,
            vec![
                ByteCellSpan {
                    bytes: 0..1,
                    cells: 0..1
                },
                ByteCellSpan {
                    bytes: 1..2,
                    cells: 1..2
                },
                ByteCellSpan {
                    bytes: 2..3,
                    cells: 2..3
                },
                ByteCellSpan {
                    bytes: 3..4,
                    cells: 3..4
                },
            ]
        );
        assert_eq!(run.style_ranges[0].bytes, 0..4);
    }

    #[test]
    fn row_run_breaks_at_space_style_cursor_selection_raster_and_scope() {
        let bold = SceneCellStyle {
            bold: true,
            ..SceneCellStyle::default()
        };
        let row = vec![
            SceneCell::grapheme("A", SceneCellStyle::default()),
            SceneCell::grapheme("B", SceneCellStyle::default()),
            SceneCell::grapheme(" ", SceneCellStyle::default()),
            SceneCell::grapheme("C", bold),
            SceneCell::grapheme("D", SceneCellStyle::default()),
        ];
        let program = program_for_rows(vec![row]);
        let mut cells = program
            .scoped_cells()
            .filter(|(_, y, _, scope)| *y == 2 && scope.kind == TextPaintScopeKind::PaneContent)
            .map(|(x, y, cell, scope)| RunInputCell {
                x,
                y,
                cell: cell.clone(),
                scope,
            })
            .collect::<Vec<_>>();
        cells[4].cell.cursor = true;
        cells[3].cell.selection = Some(CellSelection::Terminal);
        let mut raster = cells[1].clone();
        raster.x = 20;
        raster.cell.raster_layer = Some(1);
        cells.push(raster);

        let plan = build_from_cells(&cells, &mut glyph_style).unwrap();
        let texts = plan
            .runs
            .iter()
            .map(|run| run.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["AB", "C", "D"]);
        assert!(plan.runs.iter().all(|run| run.x != 20));
    }

    #[test]
    fn wide_grapheme_and_decorated_space_are_atomic_standalone_runs() {
        let decorated = SceneCellStyle {
            underline: true,
            ..SceneCellStyle::default()
        };
        let row = vec![
            SceneCell::grapheme("A", SceneCellStyle::default()),
            SceneCell::grapheme("界", SceneCellStyle::default()),
            SceneCell::wide_continuation(SceneCellStyle::default()),
            SceneCell::grapheme(" ", decorated),
            SceneCell::grapheme("B", SceneCellStyle::default()),
        ];
        let plan = build_row_runs(&program_for_rows(vec![row]), glyph_style).unwrap();
        let wide = content_run(&plan, "界");
        let space = content_run(&plan, " ");

        assert_eq!(wide.width, 2);
        assert_eq!(wide.byte_cells[0].cells, 0..2);
        assert_eq!(space.width, 1);
        assert_eq!(space.style_ranges.len(), 1);
        assert!(space.glyph_style.underline);
    }

    #[test]
    fn checked_u16_run_bounds_are_not_clamped_to_two_or_u8_max() {
        let row = (0..300)
            .map(|_| SceneCell::grapheme("x", SceneCellStyle::default()))
            .collect();
        let plan = build_row_runs(&program_for_rows(vec![row]), glyph_style).unwrap();
        let run = content_run(&plan, &"x".repeat(300));

        assert_eq!(run.width, 300);
        assert_eq!(run.byte_cells.last().unwrap().cells, 299..300);
        assert_eq!(run.clipped_cell_bounds().unwrap().width, 300);
    }

    #[test]
    fn complete_ligature_cluster_is_admitted_against_multiple_cells() {
        let program = program_for_rows(vec![vec![
            SceneCell::grapheme("f", SceneCellStyle::default()),
            SceneCell::grapheme("i", SceneCellStyle::default()),
        ]]);
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let run = content_run(&plan, "fi");
        let layout = LayoutRunFacts {
            rtl: false,
            line_width: 20.0,
            glyphs: vec![LayoutGlyphFacts {
                bytes: 0..2,
                x: 0.0,
                advance: 20.0,
                rtl: false,
            }],
        };

        assert_eq!(
            admit_layout(run, &layout, 10.0, 1.0),
            RowRunAdmission::Accepted
        );
    }

    #[test]
    fn mapping_failure_requests_a_complete_grapheme_split() {
        let program = program_for_rows(vec![vec![
            SceneCell::grapheme("a", SceneCellStyle::default()),
            SceneCell::grapheme("b", SceneCellStyle::default()),
            SceneCell::grapheme("c", SceneCellStyle::default()),
        ]]);
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let run = content_run(&plan, "abc");
        let layout = LayoutRunFacts {
            rtl: false,
            line_width: 30.0,
            glyphs: vec![
                LayoutGlyphFacts {
                    bytes: 0..1,
                    x: 0.0,
                    advance: 10.0,
                    rtl: false,
                },
                LayoutGlyphFacts {
                    bytes: 1..2,
                    x: 10.0,
                    advance: 15.0,
                    rtl: false,
                },
                LayoutGlyphFacts {
                    bytes: 2..3,
                    x: 25.0,
                    advance: 5.0,
                    rtl: false,
                },
            ],
        };

        assert_eq!(
            admit_layout(run, &layout, 10.0, 1.0),
            RowRunAdmission::Fallback {
                reason: RowRunFallbackReason::AdvanceMismatch,
                action: RowRunFallbackAction::SplitAtSpan(1),
            }
        );
        let (left, right) = split_at_span(run, 1).unwrap();
        assert_eq!((left.text.as_str(), left.width), ("a", 1));
        assert_eq!((right.text.as_str(), right.width), ("bc", 2));
    }

    #[test]
    fn admission_rejects_a_missing_middle_cluster_even_with_full_line_width() {
        let program = program_for_rows(vec![vec![
            SceneCell::grapheme("a", SceneCellStyle::default()),
            SceneCell::grapheme("b", SceneCellStyle::default()),
            SceneCell::grapheme("c", SceneCellStyle::default()),
        ]]);
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let run = content_run(&plan, "abc");
        let layout = LayoutRunFacts {
            rtl: false,
            line_width: 30.0,
            glyphs: vec![
                LayoutGlyphFacts {
                    bytes: 0..1,
                    x: 0.0,
                    advance: 10.0,
                    rtl: false,
                },
                LayoutGlyphFacts {
                    bytes: 2..3,
                    x: 20.0,
                    advance: 10.0,
                    rtl: false,
                },
            ],
        };

        assert!(matches!(
            admit_layout(run, &layout, 10.0, 1.0),
            RowRunAdmission::Fallback {
                reason: RowRunFallbackReason::PartialCellCluster,
                ..
            }
        ));
    }

    #[test]
    fn rtl_or_visual_reordering_takes_anchored_fallback() {
        let program = program_for_rows(vec![vec![
            SceneCell::grapheme("א", SceneCellStyle::default()),
            SceneCell::grapheme("ב", SceneCellStyle::default()),
        ]]);
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let run = content_run(&plan, "אב");
        let layout = LayoutRunFacts {
            rtl: true,
            line_width: 20.0,
            glyphs: vec![LayoutGlyphFacts {
                bytes: 2..4,
                x: 0.0,
                advance: 10.0,
                rtl: true,
            }],
        };

        assert_eq!(
            admit_layout(run, &layout, 10.0, 1.0),
            RowRunAdmission::Fallback {
                reason: RowRunFallbackReason::BidiOrVisualReordering,
                action: RowRunFallbackAction::AnchorAll,
            }
        );
        let anchored = anchored_fallback_runs(run).unwrap();
        assert_eq!(anchored.len(), 2);
        assert_eq!((anchored[0].x, anchored[0].width), (run.x, 1));
        assert_eq!((anchored[1].x, anchored[1].width), (run.x + 1, 1));
        assert!(
            anchored
                .iter()
                .all(|part| part.clipped_cell_bounds().is_some())
        );
    }

    #[test]
    fn half_physical_pixel_tolerance_tracks_scale() {
        let program = program_for_rows(vec![vec![
            SceneCell::grapheme("a", SceneCellStyle::default()),
            SceneCell::grapheme("b", SceneCellStyle::default()),
        ]]);
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let run = content_run(&plan, "ab");
        let mut layout = LayoutRunFacts {
            rtl: false,
            line_width: 20.2,
            glyphs: vec![LayoutGlyphFacts {
                bytes: 0..2,
                x: 0.0,
                advance: 20.2,
                rtl: false,
            }],
        };
        assert_eq!(
            admit_layout(run, &layout, 10.0, 2.0),
            RowRunAdmission::Accepted
        );

        layout.line_width = 20.3;
        layout.glyphs[0].advance = 20.3;
        assert!(matches!(
            admit_layout(run, &layout, 10.0, 2.0),
            RowRunAdmission::Fallback {
                reason: RowRunFallbackReason::AdvanceMismatch,
                ..
            }
        ));
    }

    #[test]
    fn orphan_continuation_is_observable_and_never_becomes_text() {
        let program = program_for_rows(vec![vec![SceneCell::grapheme(
            "x",
            SceneCellStyle::default(),
        )]]);
        let (x, y, cell, scope) = program
            .scoped_cells()
            .find(|(_, _, _, scope)| scope.kind == TextPaintScopeKind::PaneContent)
            .expect("pane-content test cell");
        let mut cell = cell.clone();
        cell.occupancy = CellOccupancy::WideContinuation;
        let plan =
            build_from_cells(&[RunInputCell { x, y, cell, scope }], &mut glyph_style).unwrap();

        assert!(plan.runs.iter().all(|run| !run.text.is_empty()));
        assert!(
            plan.issues
                .iter()
                .any(|issue| matches!(issue, RowRunBuildIssue::OrphanWideContinuation { .. }))
        );
    }

    #[test]
    fn cursor_and_selection_transitions_preserve_cell_ownership() {
        let surface = TerminalSurface {
            rows: vec![vec![
                SceneCell::grapheme("a", SceneCellStyle::default()),
                SceneCell::grapheme("b", SceneCellStyle::default()),
            ]],
            first_row: 0,
            cursor: Some(SurfacePosition::new(0, 1)),
            scroll_offset: 0,
            scrollback_len: 0,
            selection: Some((SurfacePosition::new(0, 0), SurfacePosition::new(0, 0))),
            copy_cursor: None,
        };
        let program = compile_cell_program(&scene_for_surface(surface), &Theme::default());
        let plan = build_row_runs(&program, glyph_style).unwrap();
        let content_runs = plan
            .runs
            .iter()
            .filter(|run| run.paint_scope.kind == TextPaintScopeKind::PaneContent)
            .collect::<Vec<_>>();

        assert_eq!(content_runs.len(), 2);
        assert_eq!(content_runs[0].selection, Some(CellSelection::Terminal));
        assert!(content_runs[1].cursor);
    }
}
