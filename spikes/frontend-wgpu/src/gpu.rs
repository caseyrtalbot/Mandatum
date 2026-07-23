// GPU frontend: wgpu surface + an instanced solid-quad pipeline for cell
// backgrounds/selection/cursor/status, layered under GPU-rasterized glyphs
// rendered by glyphon. All rendering is per-frame from a grid snapshot.

use std::sync::Arc;
use std::time::Instant;

use glyphon::{
    Attrs, Buffer, Cache, Color as GColor, Family, FontSystem, Metrics, Resolution, Shaping,
    SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Wrap,
};
// The renderer consumes ONLY the scene contract. It never imports
// mandatum-terminal-vt: the real app host converts its grids before the
// snapshot reaches this crate, so no parser type crosses into paint.
use mandatum_scene::{
    ContextMenuEntry, ContextMenuOverlay, HelpEntry, HelpOverlay, OverlayScene, PaletteOverlay,
    PaneContent, PaneScene, PromptOverlay, SESSION_MAP_FOCUS_GLYPH, SceneColor, SceneRect,
    SearchEntry, SearchOverlay, SessionMapOverlay, SessionMapRow, TerminalSurface, Theme,
    TimelineEntry, TimelineOverlay, WelcomeOverlay, WorkspaceScene, layout,
};
use winit::window::Window;

const DEFAULT_FG: [u8; 3] = [220, 220, 224];
const DEFAULT_BG: [u8; 3] = [18, 18, 22];
const SELECTION_BG: [u8; 4] = [70, 100, 180, 150];
const CURSOR_BG: [u8; 4] = [210, 210, 220, 150];
const STATUS_BG: [u8; 3] = [30, 32, 40];
const STATUS_FG: [u8; 3] = [170, 176, 190];
const BASE_FONT_PT: f32 = 15.0;

#[derive(Debug, PartialEq, Eq)]
pub enum UnsupportedScene {
    Overlay(&'static str),
    PaneCount(usize),
    PaneContent(&'static str),
    Layout(&'static str),
}

impl std::fmt::Display for UnsupportedScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Overlay(kind) => write!(f, "{kind} overlays are not implemented"),
            Self::PaneCount(count) => {
                write!(f, "unsupported pane count: {count}")
            }
            Self::PaneContent(kind) => write!(f, "{kind} pane content is not implemented"),
            Self::Layout(kind) => write!(f, "{kind} layout is not implemented"),
        }
    }
}

#[derive(Debug)]
pub struct PreparedPane<'a> {
    pane: &'a PaneScene,
    terminal: Option<&'a TerminalSurface>,
    pane_text: String,
    pane_text_rows: usize,
    body_wrap: Wrap,
}

impl PreparedPane<'_> {
    pub fn scene(&self) -> &PaneScene {
        self.pane
    }

    pub fn pane_text(&self) -> &str {
        &self.pane_text
    }

    pub fn pane_surface(&self) -> Option<&TerminalSurface> {
        self.terminal
    }
}

/// Validate the deliberately narrow spike boundary without touching a GPU.
/// Production-only scene shapes fail explicitly instead of being silently
/// ignored while the renderer paints whichever terminal pane appears first.
#[derive(Debug)]
pub struct PreparedScene<'a> {
    scene: &'a WorkspaceScene,
    theme: &'a Theme,
    panes: Vec<PreparedPane<'a>>,
    palette: Option<&'a PaletteOverlay>,
    context_menu: Option<&'a ContextMenuOverlay>,
    timeline: Option<&'a TimelineOverlay>,
    search: Option<&'a SearchOverlay>,
    session_map: Option<&'a SessionMapOverlay>,
    prompt: Option<&'a PromptOverlay>,
    help: Option<&'a HelpOverlay>,
    welcome: Option<&'a WelcomeOverlay>,
}

impl PreparedScene<'_> {
    pub fn header_text(&self) -> &str {
        &self.scene.header.text
    }

    pub fn pane_title(&self) -> &str {
        &self.panes[0].pane.title
    }

    pub fn pane_text(&self) -> &str {
        &self.panes[0].pane_text
    }

    pub fn pane_surface(&self) -> Option<&TerminalSurface> {
        self.panes[0].terminal
    }

    pub fn panes(&self) -> &[PreparedPane<'_>] {
        &self.panes
    }

    pub fn theme_name(&self) -> &str {
        &self.theme.name
    }

    pub fn has_palette(&self) -> bool {
        self.palette.is_some()
    }

    pub fn context_menu(&self) -> Option<&ContextMenuOverlay> {
        self.context_menu
    }

    pub fn timeline(&self) -> Option<&TimelineOverlay> {
        self.timeline
    }

    pub fn search(&self) -> Option<&SearchOverlay> {
        self.search
    }

    pub fn session_map(&self) -> Option<&SessionMapOverlay> {
        self.session_map
    }

    pub fn prompt(&self) -> Option<&PromptOverlay> {
        self.prompt
    }

    pub fn help(&self) -> Option<&HelpOverlay> {
        self.help
    }

    pub fn welcome(&self) -> Option<&WelcomeOverlay> {
        self.welcome
    }

    pub fn pane_text_visible_bounds(
        &self,
        pane_index: usize,
        cell_width: f32,
        cell_height: f32,
    ) -> Option<Vec<TextBounds>> {
        let pane = self.panes.get(pane_index)?;
        let bounds = scene_rect_to_text_bounds(
            layout::pane_inner_rect(pane.pane.area),
            cell_width,
            cell_height,
        );
        let (floating_occlusion, opaque_overlay_occlusion) =
            self.text_occlusions(cell_width, cell_height);

        Some(text_bounds_below_scene_occlusions(
            bounds,
            pane_index,
            floating_occlusion,
            opaque_overlay_occlusion,
        ))
    }

    fn text_occlusions(
        &self,
        cell_width: f32,
        cell_height: f32,
    ) -> (Option<(usize, TextBounds)>, Option<TextBounds>) {
        let floating_occlusion = self
            .panes
            .iter()
            .enumerate()
            .find(|(_, pane)| pane.pane.floating)
            .map(|(index, pane)| {
                (
                    index,
                    scene_rect_to_text_bounds(pane.pane.area, cell_width, cell_height),
                )
            });
        let opaque_overlay_occlusion = self
            .opaque_overlay_area()
            .map(|area| scene_rect_to_text_bounds(area, cell_width, cell_height));

        (floating_occlusion, opaque_overlay_occlusion)
    }

    fn chrome_text_visible_bounds(
        &self,
        area: SceneRect,
        cell_width: f32,
        cell_height: f32,
    ) -> Vec<TextBounds> {
        let bounds = scene_rect_to_text_bounds(area, cell_width, cell_height);
        match self.opaque_overlay_area() {
            Some(area) => text_bounds_around_occlusion(
                bounds,
                scene_rect_to_text_bounds(area, cell_width, cell_height),
            ),
            None => vec![bounds],
        }
    }

    fn opaque_overlay_area(&self) -> Option<SceneRect> {
        self.palette
            .map(|overlay| overlay.area)
            .or_else(|| self.context_menu.map(|overlay| overlay.area))
            .or_else(|| self.timeline.map(|overlay| overlay.area))
            .or_else(|| self.search.map(|overlay| overlay.area))
            .or_else(|| self.session_map.map(|overlay| overlay.area))
            .or_else(|| self.prompt.map(|overlay| overlay.area))
            .or_else(|| self.help.map(|overlay| overlay.area))
            .or_else(|| self.welcome.map(|overlay| overlay.area))
    }
}

/// Prepare the deliberately narrow paint plan without a window or
/// GPU. The displayed renderer consumes this same value, so the excluded
/// integration test exercises the real host-to-renderer boundary headlessly.
pub fn prepare_scene<'a>(
    scene: &'a WorkspaceScene,
    theme: &'a Theme,
) -> Result<PreparedScene<'a>, UnsupportedScene> {
    match scene.panes.len() {
        1 if scene.panes[0].stacked => {
            return Err(UnsupportedScene::Layout("stacked panes"));
        }
        1 => {}
        2 if is_two_horizontal_empty_panes(scene)
            || is_two_horizontal_empty_panes_with_palette(scene)
            || is_two_vertical_empty_panes(scene)
            || is_two_floating_empty_panes(scene) => {}
        2 => {
            return Err(UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes",
            ));
        }
        count => return Err(UnsupportedScene::PaneCount(count)),
    }

    let panes = scene.panes.iter().map(prepare_pane).collect::<Vec<_>>();
    let (palette, context_menu, timeline, search, session_map, prompt, help, welcome) = match &scene
        .overlay
    {
        Some(OverlayScene::Palette(palette)) => {
            (Some(palette), None, None, None, None, None, None, None)
        }
        Some(OverlayScene::ContextMenu(menu)) => {
            (None, Some(menu), None, None, None, None, None, None)
        }
        Some(OverlayScene::Timeline(timeline)) => {
            (None, None, Some(timeline), None, None, None, None, None)
        }
        Some(OverlayScene::Search(search)) => {
            (None, None, None, Some(search), None, None, None, None)
        }
        Some(OverlayScene::SessionMap(map)) => {
            (None, None, None, None, Some(map), None, None, None)
        }
        Some(OverlayScene::Prompt(prompt)) => {
            (None, None, None, None, None, Some(prompt), None, None)
        }
        Some(OverlayScene::Help(help)) => (None, None, None, None, None, None, Some(help), None),
        Some(OverlayScene::Welcome(welcome)) => {
            (None, None, None, None, None, None, None, Some(welcome))
        }
        None => (None, None, None, None, None, None, None, None),
    };
    Ok(PreparedScene {
        scene,
        theme,
        panes,
        palette,
        context_menu,
        timeline,
        search,
        session_map,
        prompt,
        help,
        welcome,
    })
}

fn is_two_horizontal_empty_panes(scene: &WorkspaceScene) -> bool {
    if scene.overlay.is_some() {
        return false;
    }
    is_two_horizontal_empty_pane_geometry(scene)
}

fn is_two_horizontal_empty_panes_with_palette(scene: &WorkspaceScene) -> bool {
    matches!(
        scene.overlay.as_ref(),
        Some(OverlayScene::Palette(palette))
            if palette.area == layout::palette_overlay_rect(scene.size)
    ) && is_two_horizontal_empty_pane_geometry(scene)
}

fn is_two_horizontal_empty_pane_geometry(scene: &WorkspaceScene) -> bool {
    let [first, second] = scene.panes.as_slice() else {
        return false;
    };
    let workspace = layout::workspace_scene_area(scene.size);
    first.area.x == workspace.x
        && first.area.y == workspace.y
        && first.area.height == workspace.height
        && first.area.right() == second.area.x
        && second.area.y == workspace.y
        && second.area.right() == workspace.right()
        && second.area.height == workspace.height
        && pane_has_usable_interior(first.area)
        && pane_has_usable_interior(second.area)
        && !first.floating
        && !first.stacked
        && !first.zoomed
        && !second.floating
        && !second.stacked
        && !second.zoomed
        && matches!(first.content, PaneContent::Empty(_))
        && matches!(second.content, PaneContent::Empty(_))
}

fn is_two_vertical_empty_panes(scene: &WorkspaceScene) -> bool {
    if scene.overlay.is_some() {
        return false;
    }
    let [first, second] = scene.panes.as_slice() else {
        return false;
    };
    let workspace = layout::workspace_scene_area(scene.size);
    first.area.x == workspace.x
        && first.area.y == workspace.y
        && first.area.width == workspace.width
        && first.area.bottom() == second.area.y
        && second.area.x == workspace.x
        && second.area.right() == workspace.right()
        && second.area.bottom() == workspace.bottom()
        && second.area.width == workspace.width
        && pane_has_usable_interior(first.area)
        && pane_has_usable_interior(second.area)
        && !first.floating
        && !first.stacked
        && !first.zoomed
        && !second.floating
        && !second.stacked
        && !second.zoomed
        && matches!(first.content, PaneContent::Empty(_))
        && matches!(second.content, PaneContent::Empty(_))
}

fn is_two_floating_empty_panes(scene: &WorkspaceScene) -> bool {
    if scene.overlay.is_some() {
        return false;
    }
    let [first, second] = scene.panes.as_slice() else {
        return false;
    };
    let workspace = layout::workspace_scene_area(scene.size);
    let default_floating = layout::default_floating_pane_rect(scene.size);

    first.area == workspace
        && pane_has_usable_interior(first.area)
        && !first.floating
        && !first.stacked
        && !first.zoomed
        && second.area == default_floating
        && pane_has_usable_interior(second.area)
        && second.floating
        && !second.stacked
        && !second.zoomed
        && matches!(first.content, PaneContent::Empty(_))
        && matches!(second.content, PaneContent::Empty(_))
}

fn pane_has_usable_interior(area: SceneRect) -> bool {
    area.width >= 3 && area.height >= 3
}

fn prepare_pane(pane: &PaneScene) -> PreparedPane<'_> {
    let inner_width = layout::pane_inner_rect(pane.area).width;
    let (terminal, pane_text, pane_text_rows, body_wrap) = match &pane.content {
        PaneContent::Terminal(surface) => (Some(surface), String::new(), 0, Wrap::None),
        PaneContent::Task(task) => {
            // Task output owns the rows below metadata, so each scene detail
            // entry must remain exactly one GPU row. Match the terminal
            // adapter's tail-preserving fit instead of allowing shaping to
            // wrap or embedded newlines to consume unbudgeted rows.
            let lines = pane
                .detail_lines()
                .into_iter()
                .map(|line| fit_cell_line(&normalize_cell_line(&line), inner_width))
                .collect::<Vec<_>>();
            let rows = lines.len();
            (task.output.as_ref(), lines.join("\n"), rows, Wrap::None)
        }
        PaneContent::Agent(_) => {
            let lines = pane.detail_lines();
            let rows = lines.len();
            (None, lines.join("\n"), rows, Wrap::WordOrGlyph)
        }
        PaneContent::Empty(_) => {
            let lines = pane.detail_lines();
            let rows = lines.len();
            (None, lines.join("\n"), rows, Wrap::WordOrGlyph)
        }
    };
    PreparedPane {
        pane,
        terminal,
        pane_text,
        pane_text_rows,
        body_wrap,
    }
}

fn normalize_cell_line(line: &str) -> String {
    line.chars()
        .map(|character| match character {
            '\r' | '\n' => ' ',
            other => other,
        })
        .collect()
}

/// Fit one logical task row to the scene's cell width while retaining both
/// the label and the load-bearing tail (exit code, flag, or filename).
fn fit_cell_line(line: &str, width: u16) -> String {
    let width = usize::from(width);
    let characters = line.chars().collect::<Vec<_>>();
    if characters.len() <= width {
        return line.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_owned();
    }
    let tail_len = (width - 1) / 2;
    let head_len = width - 1 - tail_len;
    let mut fitted = characters[..head_len].iter().collect::<String>();
    fitted.push('…');
    fitted.extend(&characters[characters.len() - tail_len..]);
    fitted
}

fn context_menu_line(item: &ContextMenuEntry, width: u16) -> String {
    let width = usize::from(width);
    let label = format!(" {}", item.label);
    let label_width = label.chars().count();
    let hint_width = item.chord_hint.chars().count() + 1;
    let padding = width.saturating_sub(label_width + hint_width).max(1);
    let line = format!(
        "{label}{}{hint}",
        " ".repeat(padding),
        hint = item.chord_hint
    );
    fit_cell_line(&line, width as u16)
}

fn timeline_line(item: &TimelineEntry, width: u16) -> String {
    fit_cell_line(
        &format!(" {} {:>10}  {}", item.glyph, item.when, item.text),
        width,
    )
}

fn timeline_outer_line(content: &str, inner_width: u16) -> String {
    format!(" {}", fit_cell_line(content, inner_width))
}

fn timeline_lines(timeline: &TimelineOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(timeline.area);
    let window = layout::palette_item_window(inner, timeline.items.len(), timeline.selected);
    let mut lines = vec![String::new(); usize::from(timeline.area.height)];
    if !lines.is_empty() {
        lines[0] = " Timeline ".to_owned();
    }
    if lines.len() > 1 {
        let input = if timeline.query.is_empty() {
            "> type to filter · pane:<id> kind:<family> since:<5m>".to_owned()
        } else {
            format!("> {}_", timeline.query)
        };
        lines[1] = timeline_outer_line(&input, inner.width);
    }
    if timeline.items.is_empty() && lines.len() > 2 {
        lines[2] = timeline_outer_line(" no matching events", inner.width);
    }
    for (row, index) in window.enumerate() {
        if let Some(slot) = lines.get_mut(row + 2) {
            *slot = timeline_outer_line(
                &timeline_line(&timeline.items[index], inner.width),
                inner.width,
            );
        }
    }
    if lines.len() > 1 {
        let footer_row = lines.len() - 2;
        lines[footer_row] = timeline_outer_line(&format!(" {}", timeline.footer), inner.width);
    }
    lines
}

fn search_line(item: &SearchEntry, source: &str, width: u16) -> String {
    fit_cell_line(&format!("{source}  {}", item.text), width)
}

fn search_outer_line(content: &str, inner_width: u16) -> String {
    format!(" {}", fit_cell_line(content, inner_width))
}

fn search_lines(search: &SearchOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(search.area);
    let window = layout::palette_item_window(inner, search.items.len(), search.selected);
    let mut lines = vec![String::new(); usize::from(search.area.height)];
    if !lines.is_empty() {
        lines[0] = " Search Session Output ".to_owned();
    }
    if lines.len() > 1 {
        let input = if search.query.is_empty() {
            "> type to search output · pane:<title> kind:<terminal|task|agent|timeline>".to_owned()
        } else {
            format!("> {}", search.query)
        };
        lines[1] = search_outer_line(&input, inner.width);
    }
    if search.items.is_empty() && lines.len() > 2 {
        let calm = if search.query.trim().is_empty() {
            " searching this session's pane output and timeline (snapshot)"
        } else {
            " no matches"
        };
        lines[2] = search_outer_line(calm, inner.width);
    }
    let mut previous_source: Option<&str> = None;
    for (row, index) in window.enumerate() {
        let item = &search.items[index];
        let source = if previous_source == Some(item.source.as_str()) {
            " ".repeat(item.source.chars().count())
        } else {
            item.source.clone()
        };
        previous_source = Some(item.source.as_str());
        if let Some(slot) = lines.get_mut(row + 2) {
            *slot = search_outer_line(&search_line(item, &source, inner.width), inner.width);
        }
    }
    if lines.len() > 1 {
        let footer_row = lines.len() - 2;
        lines[footer_row] = search_outer_line(&format!(" {}", search.footer), inner.width);
    }
    lines
}

fn search_cursor_cell(search: &SearchOverlay) -> Option<(u16, u16)> {
    let inner = layout::pane_inner_rect(search.area);
    if search.query.is_empty() || inner.width == 0 || inner.height == 0 {
        return None;
    }
    let query_end = 2usize.saturating_add(search.query.chars().count());
    let column = query_end.min(usize::from(inner.width.saturating_sub(1))) as u16;
    Some((inner.x.saturating_add(column), inner.y))
}

fn help_line(item: &HelpEntry, width: u16) -> String {
    let line = if item.heading {
        format!(" {}", item.label)
    } else if item.keys.is_empty() {
        format!("   {}", item.label)
    } else {
        format!("   {}  {}", item.label, item.keys)
    };
    fit_cell_line(&line, width)
}

fn help_outer_line(content: &str, inner_width: u16) -> String {
    format!(" {}", fit_cell_line(content, inner_width))
}

fn help_lines(help: &HelpOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(help.area);
    let window = layout::palette_item_window(inner, help.items.len(), help.selected);
    let mut lines = vec![String::new(); usize::from(help.area.height)];
    if !lines.is_empty() {
        lines[0] = " Help ".to_owned();
    }
    if lines.len() > 1 {
        let input = if help.query.is_empty() {
            "> type to filter the keymap".to_owned()
        } else {
            format!("> {}", help.query)
        };
        lines[1] = help_outer_line(&input, inner.width);
    }
    if help.items.is_empty() && lines.len() > 2 {
        lines[2] = help_outer_line(" no matching entries", inner.width);
    }
    for (row, index) in window.enumerate() {
        if let Some(slot) = lines.get_mut(row + 2) {
            *slot = help_outer_line(&help_line(&help.items[index], inner.width), inner.width);
        }
    }
    if lines.len() > 1 {
        let footer_row = lines.len() - 2;
        lines[footer_row] = help_outer_line(&format!(" {}", help.footer), inner.width);
    }
    lines
}

fn help_cursor_cell(help: &HelpOverlay) -> Option<(u16, u16)> {
    let inner = layout::pane_inner_rect(help.area);
    if help.query.is_empty() || inner.width == 0 || inner.height == 0 {
        return None;
    }
    let query_end = 2usize.saturating_add(help.query.chars().count());
    let column = query_end.min(usize::from(inner.width.saturating_sub(1))) as u16;
    Some((inner.x.saturating_add(column), inner.y))
}

fn welcome_lines(welcome: &WelcomeOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(welcome.area);
    let mut lines = vec![String::new(); usize::from(welcome.area.height)];
    if !lines.is_empty() {
        lines[0] = " Mandatum ".to_owned();
    }
    if lines.len() > 1 {
        lines[1] = format!(" {}", fit_cell_line(&welcome.introduction, inner.width));
    }

    let key_width = welcome
        .entries
        .iter()
        .map(|entry| entry.keys.chars().count())
        .max()
        .unwrap_or(0);
    for (row, entry) in welcome.entries.iter().enumerate() {
        if let Some(slot) = lines.get_mut(row + 3) {
            let padding = key_width.saturating_sub(entry.keys.chars().count());
            let content = format!(
                "  {}{}  {}",
                entry.keys,
                " ".repeat(padding),
                entry.description
            );
            *slot = format!(" {}", fit_cell_line(&content, inner.width));
        }
    }
    if let Some(row) = welcome.entries.len().checked_add(4)
        && let Some(slot) = lines.get_mut(row)
    {
        *slot = format!(" {}", fit_cell_line(&welcome.dismissal, inner.width));
    }
    lines
}

fn text_bounds_around_occlusion(bounds: TextBounds, occlusion: TextBounds) -> Vec<TextBounds> {
    let left = bounds.left.max(occlusion.left);
    let top = bounds.top.max(occlusion.top);
    let right = bounds.right.min(occlusion.right);
    let bottom = bounds.bottom.min(occlusion.bottom);
    if left >= right || top >= bottom {
        return vec![bounds];
    }

    let mut visible = Vec::with_capacity(4);
    if bounds.top < top {
        visible.push(TextBounds {
            bottom: top,
            ..bounds
        });
    }
    if bottom < bounds.bottom {
        visible.push(TextBounds {
            top: bottom,
            ..bounds
        });
    }
    if bounds.left < left {
        visible.push(TextBounds {
            top,
            right: left,
            bottom,
            ..bounds
        });
    }
    if right < bounds.right {
        visible.push(TextBounds {
            left: right,
            top,
            bottom,
            ..bounds
        });
    }
    visible
}

fn scene_rect_to_text_bounds(area: SceneRect, cell_width: f32, cell_height: f32) -> TextBounds {
    TextBounds {
        left: (area.x as f32 * cell_width).floor() as i32,
        top: (area.y as f32 * cell_height).floor() as i32,
        right: (area.right() as f32 * cell_width).ceil() as i32,
        bottom: (area.bottom() as f32 * cell_height).ceil() as i32,
    }
}

#[cfg(test)]
fn text_bounds_intersect(first: TextBounds, second: TextBounds) -> bool {
    first.left < second.right
        && first.right > second.left
        && first.top < second.bottom
        && first.bottom > second.top
}

fn text_bounds_below_later_float(
    bounds: TextBounds,
    pane_index: usize,
    floating_occlusion: Option<(usize, TextBounds)>,
) -> Vec<TextBounds> {
    match floating_occlusion {
        Some((floating_index, occlusion)) if pane_index < floating_index => {
            text_bounds_around_occlusion(bounds, occlusion)
        }
        _ => vec![bounds],
    }
}

fn text_bounds_below_scene_occlusions(
    bounds: TextBounds,
    pane_index: usize,
    floating_occlusion: Option<(usize, TextBounds)>,
    opaque_overlay_occlusion: Option<TextBounds>,
) -> Vec<TextBounds> {
    let mut visible = text_bounds_below_later_float(bounds, pane_index, floating_occlusion);
    if let Some(occlusion) = opaque_overlay_occlusion {
        visible = visible
            .into_iter()
            .flat_map(|bounds| text_bounds_around_occlusion(bounds, occlusion))
            .collect();
    }
    visible
}

fn session_map_line(row: &SessionMapRow, width: u16) -> String {
    let marker = if row.focused {
        SESSION_MAP_FOCUS_GLYPH
    } else {
        " "
    };
    let indent = "  ".repeat(usize::from(row.depth));
    let mut line = format!("{marker}{indent}{} {}", row.glyph, row.label);
    if !row.state.is_empty() {
        line.push_str(&format!("  {}", row.state));
    }
    if !row.badges.is_empty() {
        line.push_str(&format!("  [{}]", row.badges));
    }
    fit_cell_line(&line, width)
}

fn session_map_outer_line(content: &str, inner_width: u16) -> String {
    format!(" {}", fit_cell_line(content, inner_width))
}

fn session_map_lines(map: &SessionMapOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(map.area);
    let window = layout::session_map_item_window(inner, map.rows.len(), Some(map.selected));
    let mut lines = vec![String::new(); usize::from(map.area.height)];
    if !lines.is_empty() {
        lines[0] = " Sessions ".to_owned();
    }
    for (row, index) in window.enumerate() {
        if let Some(slot) = lines.get_mut(row + 1) {
            *slot = session_map_outer_line(
                &session_map_line(&map.rows[index], inner.width),
                inner.width,
            );
        }
    }
    if lines.len() > 1 {
        let footer_row = lines.len() - 2;
        lines[footer_row] = session_map_outer_line(&format!(" {}", map.footer), inner.width);
    }
    lines
}

fn prompt_lines(prompt: &PromptOverlay) -> Vec<String> {
    let inner = layout::pane_inner_rect(prompt.area);
    let mut lines = vec![String::new(); usize::from(prompt.area.height)];
    if !lines.is_empty() {
        lines[0] = fit_cell_line(&prompt.title, prompt.area.width);
    }
    if lines.len() > 1 {
        lines[1] = format!(
            " {}",
            fit_cell_line(&format!("> {}", prompt.input), inner.width)
        );
    }
    if lines.len() > 2 {
        let footer_row = lines.len() - 2;
        lines[footer_row] = format!(" {}", fit_cell_line(&prompt.footer, inner.width));
    }
    lines
}

fn prompt_cursor_cell(prompt: &PromptOverlay) -> Option<(u16, u16)> {
    let inner = layout::pane_inner_rect(prompt.area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let input_end = 2usize.saturating_add(prompt.input.chars().count());
    let column = input_end.min(usize::from(inner.width.saturating_sub(1))) as u16;
    Some((inner.x.saturating_add(column), inner.y))
}

pub struct GpuText {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // Solid-quad pipeline.
    quad_pipeline: wgpu::RenderPipeline,
    unit_buf: wgpu::Buffer,
    inst_buf: wgpu::Buffer,
    inst_capacity_floats: usize,
    res_buf: wgpu::Buffer,
    res_bind_group: wgpu::BindGroup,

    // Text stack.
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    header_buffer: Buffer,
    title_buffers: Vec<Buffer>,
    text_buffers: Vec<Buffer>,
    status_buffer: Buffer,
    overlay_buffer: Buffer,

    scale: f32,
    font_size: f32,
    cell_w: f32,
    cell_h: f32,
}

impl GpuText {
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("mandatum-spike-device"),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("request_device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // --- Quad pipeline ---------------------------------------------------
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad-shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
        });
        let res_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("resolution-uniform"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("res-bind-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let res_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("res-bind-group"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: res_buf.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("quad-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        const UNIT_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
        const INST_ATTRS: [wgpu::VertexAttribute; 2] =
            wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4];
        let quad_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 8,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &UNIT_ATTRS,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &INST_ATTRS,
                    }),
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let unit: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let unit_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("unit-quad"),
            size: 32,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&unit_buf, 0, bytes_of(&unit));

        let inst_capacity_floats = 8 * 4096;
        let inst_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad-instances"),
            size: (inst_capacity_floats * 4) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Text stack ------------------------------------------------------
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let font_size = (BASE_FONT_PT * scale).round();
        let line_height = (font_size * 1.3).round();
        let metrics = Metrics::new(font_size, line_height);
        let header_buffer = Buffer::new(&mut font_system, metrics);
        let title_buffers = (0..2)
            .map(|_| Buffer::new(&mut font_system, metrics))
            .collect();
        let text_buffers = (0..2)
            .map(|_| Buffer::new(&mut font_system, metrics))
            .collect();
        let status_buffer = Buffer::new(&mut font_system, metrics);
        let overlay_buffer = Buffer::new(&mut font_system, metrics);
        let cell_w = measure_cell_width(&mut font_system, metrics);
        let cell_h = line_height;

        Ok(Self {
            surface,
            device,
            queue,
            config,
            quad_pipeline,
            unit_buf,
            inst_buf,
            inst_capacity_floats,
            res_buf,
            res_bind_group,
            font_system,
            swash_cache,
            cache,
            viewport,
            atlas,
            text_renderer,
            header_buffer,
            title_buffers,
            text_buffers,
            status_buffer,
            overlay_buffer,
            scale,
            font_size,
            cell_w,
            cell_h,
        })
    }

    pub fn cell_w(&self) -> f32 {
        self.cell_w
    }

    pub fn cell_h(&self) -> f32 {
        self.cell_h
    }

    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < f32::EPSILON {
            return;
        }
        self.scale = scale;
        self.font_size = (BASE_FONT_PT * scale).round();
        let line_height = (self.font_size * 1.3).round();
        let metrics = Metrics::new(self.font_size, line_height);
        self.header_buffer.set_metrics(metrics);
        for buffer in &mut self.title_buffers {
            buffer.set_metrics(metrics);
        }
        for buffer in &mut self.text_buffers {
            buffer.set_metrics(metrics);
        }
        self.status_buffer.set_metrics(metrics);
        self.overlay_buffer.set_metrics(metrics);
        self.cell_w = measure_cell_width(&mut self.font_system, metrics);
        self.cell_h = line_height;
    }

    /// Render one frame from a `WorkspaceScene`. Consumes only scene types: the
    /// visible cells, styles, cursor/selection marks, and status come from the
    /// scene, never from a grid or parser. Returns the instant right after
    /// `present()` for input-to-present measurement. `Ok(None)` means the
    /// swapchain frame could not be acquired; unsupported scene shapes return
    /// a visible adapter error instead of being skipped or panicking.
    pub fn render(
        &mut self,
        scene: &WorkspaceScene,
        theme: &Theme,
    ) -> Result<Option<Instant>, UnsupportedScene> {
        let prepared = prepare_scene(scene, theme)?;

        // Assemble foreground text buffers and background quads per pane,
        // painting straight from the scene-owned draw order and geometry.
        let mut quads: Vec<f32> = Vec::with_capacity(1024);

        let header_background = resolve(theme.header_background, DEFAULT_BG);
        push_quad(
            &mut quads,
            scene.header.area.x as f32 * self.cell_w,
            scene.header.area.y as f32 * self.cell_h,
            scene.header.area.width as f32 * self.cell_w,
            scene.header.area.height as f32 * self.cell_h,
            [
                header_background[0],
                header_background[1],
                header_background[2],
                255,
            ],
        );

        let border = resolve(theme.pane_border, DEFAULT_FG);
        let border_rgba = [border[0], border[1], border[2], 255];
        struct PanePaint {
            origin_x: f32,
            origin_y: f32,
            title_x: f32,
            title_y: f32,
            title_width: f32,
            title_fg: [u8; 3],
        }
        let mut pane_paints = Vec::with_capacity(prepared.panes.len());
        for (index, pane_plan) in prepared.panes.iter().enumerate() {
            let pane = pane_plan.pane;
            let inner = layout::pane_inner_rect(pane.area);
            let origin_x = inner.x as f32 * self.cell_w;
            let origin_y = inner.y as f32 * self.cell_h;
            let pane_x = pane.area.x as f32 * self.cell_w;
            let pane_y = pane.area.y as f32 * self.cell_h;
            let pane_width = pane.area.width as f32 * self.cell_w;
            let pane_height = pane.area.height as f32 * self.cell_h;

            if pane.floating && pane.area.width > 0 && pane.area.height > 0 {
                push_quad(
                    &mut quads,
                    pane_x,
                    pane_y,
                    pane_width,
                    pane_height,
                    [DEFAULT_BG[0], DEFAULT_BG[1], DEFAULT_BG[2], 255],
                );
            }
            if pane.area.width > 0 && pane.area.height > 0 {
                push_quad(&mut quads, pane_x, pane_y, pane_width, 1.0, border_rgba);
                push_quad(
                    &mut quads,
                    pane_x,
                    pane_y + pane_height - 1.0,
                    pane_width,
                    1.0,
                    border_rgba,
                );
                push_quad(&mut quads, pane_x, pane_y, 1.0, pane_height, border_rgba);
                push_quad(
                    &mut quads,
                    pane_x + pane_width - 1.0,
                    pane_y,
                    1.0,
                    pane_height,
                    border_rgba,
                );
            }

            let mut screen_text = String::new();
            let mut runs: Vec<(std::ops::Range<usize>, GColor)> = Vec::new();
            if !pane_plan.pane_text.is_empty() {
                let start = screen_text.len();
                screen_text.push_str(&pane_plan.pane_text);
                screen_text.push('\n');
                runs.push((
                    start..screen_text.len(),
                    GColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
                ));
            }

            if let Some(surface) = pane_plan.terminal {
                let row_offset = pane_plan.pane_text_rows;
                for (y, row) in surface.rows.iter().enumerate() {
                    if row_offset + y >= usize::from(inner.height) {
                        break;
                    }
                    let abs = surface.first_row + y;
                    let py = origin_y + (row_offset + y) as f32 * self.cell_h;
                    let mut run_start = screen_text.len();
                    let mut run_color: Option<GColor> = None;
                    for (x, cell) in row.iter().take(usize::from(inner.width)).enumerate() {
                        let style = cell.style;
                        let (mut fg, mut bg) = (
                            resolve(style.foreground, DEFAULT_FG),
                            resolve(style.background, DEFAULT_BG),
                        );
                        if style.inverse {
                            std::mem::swap(&mut fg, &mut bg);
                        }

                        let column = x as u16;
                        let px = origin_x + x as f32 * self.cell_w;
                        if bg != DEFAULT_BG {
                            push_quad(
                                &mut quads,
                                px,
                                py,
                                self.cell_w,
                                self.cell_h,
                                [bg[0], bg[1], bg[2], 255],
                            );
                        }
                        if surface.selection_contains(abs, column) {
                            push_quad(&mut quads, px, py, self.cell_w, self.cell_h, SELECTION_BG);
                        }
                        if surface.cursor_at(abs, column) {
                            push_quad(&mut quads, px, py, self.cell_w, self.cell_h, CURSOR_BG);
                        }

                        let gc = GColor::rgb(fg[0], fg[1], fg[2]);
                        if run_color != Some(gc) {
                            if let Some(previous) = run_color.take() {
                                runs.push((run_start..screen_text.len(), previous));
                            }
                            run_start = screen_text.len();
                            run_color = Some(gc);
                        }
                        screen_text.push(cell.character);
                    }
                    if let Some(previous) = run_color.take() {
                        runs.push((run_start..screen_text.len(), previous));
                    }
                    let newline = screen_text.len();
                    screen_text.push('\n');
                    if let Some((range, _)) =
                        runs.last_mut().filter(|(range, _)| range.end == newline)
                    {
                        range.end = screen_text.len();
                    } else {
                        runs.push((
                            newline..screen_text.len(),
                            GColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
                        ));
                    }
                }
            }

            let title_fg = resolve(
                if pane.focused {
                    theme.focus_title
                } else {
                    theme.pane_title
                },
                DEFAULT_FG,
            );
            let title_width = pane.area.width.saturating_sub(2) as f32 * self.cell_w;
            self.title_buffers[index].set_size(Some(title_width.max(1.0)), Some(self.cell_h));
            self.title_buffers[index].set_text(
                &pane.title,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    title_fg[0],
                    title_fg[1],
                    title_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.title_buffers[index].shape_until_scroll(&mut self.font_system, false);

            let default_attrs = Attrs::new().family(Family::Monospace);
            let spans = runs.iter().map(|(range, color)| {
                (
                    &screen_text[range.clone()],
                    Attrs::new().family(Family::Monospace).color(*color),
                )
            });
            let content_width = inner.width as f32 * self.cell_w;
            let content_height = inner.height as f32 * self.cell_h;
            self.text_buffers[index].set_wrap(pane_plan.body_wrap);
            self.text_buffers[index]
                .set_size(Some(content_width.max(1.0)), Some(content_height.max(1.0)));
            self.text_buffers[index].set_rich_text(spans, &default_attrs, Shaping::Advanced, None);
            self.text_buffers[index].shape_until_scroll(&mut self.font_system, false);

            pane_paints.push(PanePaint {
                origin_x,
                origin_y,
                title_x: (pane.area.x.saturating_add(1)) as f32 * self.cell_w,
                title_y: pane.area.y as f32 * self.cell_h,
                title_width,
                title_fg,
            });
        }

        // Header, pane titles, status, and optional overlays all come from the
        // real scene contract; the native shell derives no product chrome.
        let header_x = scene.header.area.x as f32 * self.cell_w;
        let header_y = scene.header.area.y as f32 * self.cell_h;
        let header_fg = resolve(theme.header, DEFAULT_FG);
        self.header_buffer
            .set_size(Some(self.config.width as f32), Some(self.cell_h));
        self.header_buffer.set_text(
            &scene.header.text,
            &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                header_fg[0],
                header_fg[1],
                header_fg[2],
            )),
            Shaping::Advanced,
            None,
        );
        self.header_buffer
            .shape_until_scroll(&mut self.font_system, false);

        let status_x = scene.status.area.x as f32 * self.cell_w;
        let status_y = scene.status.area.y as f32 * self.cell_h;
        let status_width = scene.status.area.width as f32 * self.cell_w;
        let status = scene.status.text.as_str();
        let status_fg = resolve(theme.status, STATUS_FG);
        push_quad(
            &mut quads,
            status_x,
            status_y,
            status_width,
            self.cell_h,
            [STATUS_BG[0], STATUS_BG[1], STATUS_BG[2], 255],
        );

        self.status_buffer
            .set_size(Some(self.config.width as f32), Some(self.cell_h));
        self.status_buffer.set_text(
            status,
            &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                status_fg[0],
                status_fg[1],
                status_fg[2],
            )),
            Shaping::Advanced,
            None,
        );
        self.status_buffer
            .shape_until_scroll(&mut self.font_system, false);

        let mut overlay_position = None;
        let mut overlay_clip = None;
        if let Some(palette) = prepared.palette {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            push_quad(
                &mut quads,
                palette.area.x as f32 * self.cell_w,
                palette.area.y as f32 * self.cell_h,
                palette.area.width as f32 * self.cell_w,
                palette.area.height as f32 * self.cell_h,
                [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255],
            );
            let palette_inner = layout::pane_inner_rect(palette.area);
            let window =
                layout::palette_item_window(palette_inner, palette.items.len(), palette.selected);
            let mut lines = vec![String::new(); usize::from(palette.area.height)];
            if !lines.is_empty() {
                lines[0] = " Command Palette ".to_owned();
            }
            if lines.len() > 1 {
                lines[1] = format!("> {}_", palette.query);
            }
            for (row, index) in window.clone().enumerate() {
                let item = &palette.items[index];
                let hint = item
                    .key_hint
                    .as_deref()
                    .map_or(String::new(), |hint| format!("  {hint}"));
                let line = format!(" {}{hint}  {}", item.label, item.detail);
                if let Some(slot) = lines.get_mut(row + 2) {
                    *slot = line;
                }
                if palette.selected == Some(index) {
                    let selection = resolve(theme.palette_selection, [70, 100, 180]);
                    push_quad(
                        &mut quads,
                        palette_inner.x as f32 * self.cell_w,
                        (palette_inner.y + 1 + row as u16) as f32 * self.cell_h,
                        palette_inner.width as f32 * self.cell_w,
                        self.cell_h,
                        [selection[0], selection[1], selection[2], 190],
                    );
                }
            }
            if let Some(last) = lines.last_mut() {
                *last = palette.footer.clone();
            }
            let overlay_text = lines.join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer.set_size(
                Some(palette.area.width as f32 * self.cell_w),
                Some(palette.area.height as f32 * self.cell_h),
            );
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((
                palette.area.x as f32 * self.cell_w,
                palette.area.y as f32 * self.cell_h,
            ));
            overlay_clip = Some(TextBounds {
                left: (palette.area.x as f32 * self.cell_w).floor() as i32,
                top: (palette.area.y as f32 * self.cell_h).floor() as i32,
                right: (palette.area.right() as f32 * self.cell_w).ceil() as i32,
                bottom: (palette.area.bottom() as f32 * self.cell_h).ceil() as i32,
            });
        } else if let Some(menu) = prepared.context_menu {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let menu_x = menu.area.x as f32 * self.cell_w;
            let menu_y = menu.area.y as f32 * self.cell_h;
            let menu_width = menu.area.width as f32 * self.cell_w;
            let menu_height = menu.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                menu_x,
                menu_y,
                menu_width,
                menu_height,
                overlay_bg_rgba,
            );
            if menu.area.width > 0 && menu.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(&mut quads, menu_x, menu_y, menu_width, 1.0, border_rgba);
                push_quad(
                    &mut quads,
                    menu_x,
                    menu_y + menu_height - 1.0,
                    menu_width,
                    1.0,
                    border_rgba,
                );
                push_quad(&mut quads, menu_x, menu_y, 1.0, menu_height, border_rgba);
                push_quad(
                    &mut quads,
                    menu_x + menu_width - 1.0,
                    menu_y,
                    1.0,
                    menu_height,
                    border_rgba,
                );
            }

            let inner = layout::pane_inner_rect(menu.area);
            let visible_items = menu.items.iter().take(usize::from(inner.height));
            let overlay_text = visible_items
                .map(|item| context_menu_line(item, inner.width))
                .collect::<Vec<_>>()
                .join("\n");
            if menu.selected < menu.items.len() && menu.selected < usize::from(inner.height) {
                let selection = resolve(theme.palette_selection, [70, 100, 180]);
                push_quad(
                    &mut quads,
                    inner.x as f32 * self.cell_w,
                    (inner.y + menu.selected as u16) as f32 * self.cell_h,
                    inner.width as f32 * self.cell_w,
                    self.cell_h,
                    [selection[0], selection[1], selection[2], 190],
                );
            }
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer.set_size(
                Some(inner.width as f32 * self.cell_w),
                Some(inner.height as f32 * self.cell_h),
            );
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((inner.x as f32 * self.cell_w, inner.y as f32 * self.cell_h));
        } else if let Some(timeline) = prepared.timeline {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let timeline_x = timeline.area.x as f32 * self.cell_w;
            let timeline_y = timeline.area.y as f32 * self.cell_h;
            let timeline_width = timeline.area.width as f32 * self.cell_w;
            let timeline_height = timeline.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                timeline_x,
                timeline_y,
                timeline_width,
                timeline_height,
                overlay_bg_rgba,
            );
            if timeline.area.width > 0 && timeline.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(
                    &mut quads,
                    timeline_x,
                    timeline_y,
                    timeline_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    timeline_x,
                    timeline_y + timeline_height - 1.0,
                    timeline_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    timeline_x,
                    timeline_y,
                    1.0,
                    timeline_height,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    timeline_x + timeline_width - 1.0,
                    timeline_y,
                    1.0,
                    timeline_height,
                    border_rgba,
                );
            }

            let inner = layout::pane_inner_rect(timeline.area);
            let window =
                layout::palette_item_window(inner, timeline.items.len(), timeline.selected);
            for (row, index) in window.enumerate() {
                if timeline.selected == Some(index) {
                    let selection = resolve(theme.palette_selection, [70, 100, 180]);
                    push_quad(
                        &mut quads,
                        inner.x as f32 * self.cell_w,
                        (inner.y + 1 + row as u16) as f32 * self.cell_h,
                        inner.width as f32 * self.cell_w,
                        self.cell_h,
                        [selection[0], selection[1], selection[2], 190],
                    );
                }
            }

            let overlay_text = timeline_lines(timeline).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer.set_size(
                Some(timeline_width.max(1.0)),
                Some(timeline_height.max(1.0)),
            );
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((timeline_x, timeline_y));
            overlay_clip = Some(TextBounds {
                left: timeline_x.floor() as i32,
                top: timeline_y.floor() as i32,
                right: (timeline_x + timeline_width).ceil() as i32,
                bottom: (timeline_y + timeline_height).ceil() as i32,
            });
        } else if let Some(search) = prepared.search {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let search_x = search.area.x as f32 * self.cell_w;
            let search_y = search.area.y as f32 * self.cell_h;
            let search_width = search.area.width as f32 * self.cell_w;
            let search_height = search.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                search_x,
                search_y,
                search_width,
                search_height,
                overlay_bg_rgba,
            );
            if search.area.width > 0 && search.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(
                    &mut quads,
                    search_x,
                    search_y,
                    search_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    search_x,
                    search_y + search_height - 1.0,
                    search_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    search_x,
                    search_y,
                    1.0,
                    search_height,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    search_x + search_width - 1.0,
                    search_y,
                    1.0,
                    search_height,
                    border_rgba,
                );
            }

            let inner = layout::pane_inner_rect(search.area);
            let window = layout::palette_item_window(inner, search.items.len(), search.selected);
            for (row, index) in window.enumerate() {
                if search.selected == Some(index) {
                    let selection = resolve(theme.palette_selection, [70, 100, 180]);
                    push_quad(
                        &mut quads,
                        inner.x as f32 * self.cell_w,
                        (inner.y + 1 + row as u16) as f32 * self.cell_h,
                        inner.width as f32 * self.cell_w,
                        self.cell_h,
                        [selection[0], selection[1], selection[2], 190],
                    );
                }
            }
            if let Some((column, row)) = search_cursor_cell(search) {
                push_quad(
                    &mut quads,
                    column as f32 * self.cell_w,
                    row as f32 * self.cell_h,
                    self.cell_w,
                    self.cell_h,
                    CURSOR_BG,
                );
            }

            let overlay_text = search_lines(search).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer
                .set_size(Some(search_width.max(1.0)), Some(search_height.max(1.0)));
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((search_x, search_y));
            overlay_clip = Some(TextBounds {
                left: search_x.floor() as i32,
                top: search_y.floor() as i32,
                right: (search_x + search_width).ceil() as i32,
                bottom: (search_y + search_height).ceil() as i32,
            });
        } else if let Some(help) = prepared.help {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let help_x = help.area.x as f32 * self.cell_w;
            let help_y = help.area.y as f32 * self.cell_h;
            let help_width = help.area.width as f32 * self.cell_w;
            let help_height = help.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                help_x,
                help_y,
                help_width,
                help_height,
                overlay_bg_rgba,
            );
            if help.area.width > 0 && help.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(&mut quads, help_x, help_y, help_width, 1.0, border_rgba);
                push_quad(
                    &mut quads,
                    help_x,
                    help_y + help_height - 1.0,
                    help_width,
                    1.0,
                    border_rgba,
                );
                push_quad(&mut quads, help_x, help_y, 1.0, help_height, border_rgba);
                push_quad(
                    &mut quads,
                    help_x + help_width - 1.0,
                    help_y,
                    1.0,
                    help_height,
                    border_rgba,
                );
            }

            let inner = layout::pane_inner_rect(help.area);
            let window = layout::palette_item_window(inner, help.items.len(), help.selected);
            for (row, index) in window.enumerate() {
                if help.selected == Some(index) {
                    let selection = resolve(theme.palette_selection, [70, 100, 180]);
                    push_quad(
                        &mut quads,
                        inner.x as f32 * self.cell_w,
                        (inner.y + 1 + row as u16) as f32 * self.cell_h,
                        inner.width as f32 * self.cell_w,
                        self.cell_h,
                        [selection[0], selection[1], selection[2], 190],
                    );
                }
            }
            if let Some((column, row)) = help_cursor_cell(help) {
                push_quad(
                    &mut quads,
                    column as f32 * self.cell_w,
                    row as f32 * self.cell_h,
                    self.cell_w,
                    self.cell_h,
                    CURSOR_BG,
                );
            }

            let overlay_text = help_lines(help).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer
                .set_size(Some(help_width.max(1.0)), Some(help_height.max(1.0)));
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((help_x, help_y));
            overlay_clip = Some(TextBounds {
                left: help_x.floor() as i32,
                top: help_y.floor() as i32,
                right: (help_x + help_width).ceil() as i32,
                bottom: (help_y + help_height).ceil() as i32,
            });
        } else if let Some(welcome) = prepared.welcome {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let welcome_x = welcome.area.x as f32 * self.cell_w;
            let welcome_y = welcome.area.y as f32 * self.cell_h;
            let welcome_width = welcome.area.width as f32 * self.cell_w;
            let welcome_height = welcome.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                welcome_x,
                welcome_y,
                welcome_width,
                welcome_height,
                overlay_bg_rgba,
            );
            if welcome.area.width > 0 && welcome.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(
                    &mut quads,
                    welcome_x,
                    welcome_y,
                    welcome_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    welcome_x,
                    welcome_y + welcome_height - 1.0,
                    welcome_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    welcome_x,
                    welcome_y,
                    1.0,
                    welcome_height,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    welcome_x + welcome_width - 1.0,
                    welcome_y,
                    1.0,
                    welcome_height,
                    border_rgba,
                );
            }

            let overlay_text = welcome_lines(welcome).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer
                .set_size(Some(welcome_width.max(1.0)), Some(welcome_height.max(1.0)));
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((welcome_x, welcome_y));
            overlay_clip = Some(TextBounds {
                left: welcome_x.floor() as i32,
                top: welcome_y.floor() as i32,
                right: (welcome_x + welcome_width).ceil() as i32,
                bottom: (welcome_y + welcome_height).ceil() as i32,
            });
        } else if let Some(map) = prepared.session_map {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let map_x = map.area.x as f32 * self.cell_w;
            let map_y = map.area.y as f32 * self.cell_h;
            let map_width = map.area.width as f32 * self.cell_w;
            let map_height = map.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                map_x,
                map_y,
                map_width,
                map_height,
                overlay_bg_rgba,
            );
            if map.area.width > 0 && map.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(&mut quads, map_x, map_y, map_width, 1.0, border_rgba);
                push_quad(
                    &mut quads,
                    map_x,
                    map_y + map_height - 1.0,
                    map_width,
                    1.0,
                    border_rgba,
                );
                push_quad(&mut quads, map_x, map_y, 1.0, map_height, border_rgba);
                push_quad(
                    &mut quads,
                    map_x + map_width - 1.0,
                    map_y,
                    1.0,
                    map_height,
                    border_rgba,
                );
            }

            let inner = layout::pane_inner_rect(map.area);
            let window = layout::session_map_item_window(inner, map.rows.len(), Some(map.selected));
            for (row, index) in window.enumerate() {
                if map.selected == index {
                    let selection = resolve(theme.palette_selection, [70, 100, 180]);
                    push_quad(
                        &mut quads,
                        inner.x as f32 * self.cell_w,
                        (inner.y + row as u16) as f32 * self.cell_h,
                        inner.width as f32 * self.cell_w,
                        self.cell_h,
                        [selection[0], selection[1], selection[2], 190],
                    );
                }
            }

            let overlay_text = session_map_lines(map).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer
                .set_size(Some(map_width.max(1.0)), Some(map_height.max(1.0)));
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((map_x, map_y));
            overlay_clip = Some(TextBounds {
                left: map_x.floor() as i32,
                top: map_y.floor() as i32,
                right: (map_x + map_width).ceil() as i32,
                bottom: (map_y + map_height).ceil() as i32,
            });
        } else if let Some(prompt) = prepared.prompt {
            let overlay_bg = resolve(theme.overlay_background, DEFAULT_BG);
            let overlay_bg_rgba = [overlay_bg[0], overlay_bg[1], overlay_bg[2], 255];
            let prompt_x = prompt.area.x as f32 * self.cell_w;
            let prompt_y = prompt.area.y as f32 * self.cell_h;
            let prompt_width = prompt.area.width as f32 * self.cell_w;
            let prompt_height = prompt.area.height as f32 * self.cell_h;
            push_quad(
                &mut quads,
                prompt_x,
                prompt_y,
                prompt_width,
                prompt_height,
                overlay_bg_rgba,
            );
            if prompt.area.width > 0 && prompt.area.height > 0 {
                let border = resolve(theme.palette_border, DEFAULT_FG);
                let border_rgba = [border[0], border[1], border[2], 255];
                push_quad(
                    &mut quads,
                    prompt_x,
                    prompt_y,
                    prompt_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    prompt_x,
                    prompt_y + prompt_height - 1.0,
                    prompt_width,
                    1.0,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    prompt_x,
                    prompt_y,
                    1.0,
                    prompt_height,
                    border_rgba,
                );
                push_quad(
                    &mut quads,
                    prompt_x + prompt_width - 1.0,
                    prompt_y,
                    1.0,
                    prompt_height,
                    border_rgba,
                );
            }
            if let Some((column, row)) = prompt_cursor_cell(prompt) {
                push_quad(
                    &mut quads,
                    column as f32 * self.cell_w,
                    row as f32 * self.cell_h,
                    self.cell_w,
                    self.cell_h,
                    CURSOR_BG,
                );
            }

            let overlay_text = prompt_lines(prompt).join("\n");
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            self.overlay_buffer.set_wrap(Wrap::None);
            self.overlay_buffer
                .set_size(Some(prompt_width.max(1.0)), Some(prompt_height.max(1.0)));
            self.overlay_buffer.set_text(
                &overlay_text,
                &Attrs::new().family(Family::Monospace).color(GColor::rgb(
                    overlay_fg[0],
                    overlay_fg[1],
                    overlay_fg[2],
                )),
                Shaping::Advanced,
                None,
            );
            self.overlay_buffer
                .shape_until_scroll(&mut self.font_system, false);
            overlay_position = Some((prompt_x, prompt_y));
            overlay_clip = Some(TextBounds {
                left: prompt_x.floor() as i32,
                top: prompt_y.floor() as i32,
                right: (prompt_x + prompt_width).ceil() as i32,
                bottom: (prompt_y + prompt_height).ceil() as i32,
            });
        }

        // Upload quad instances (grow buffer if needed).
        if quads.len() > self.inst_capacity_floats {
            self.inst_capacity_floats = quads.len().next_power_of_two();
            self.inst_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad-instances"),
                size: (self.inst_capacity_floats * 4) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue
            .write_buffer(&self.inst_buf, 0, bytes_of_slice(&quads));
        let instance_count = (quads.len() / 8) as u32;

        let res = [
            self.config.width as f32,
            self.config.height as f32,
            0.0,
            0.0,
        ];
        self.queue.write_buffer(&self.res_buf, 0, bytes_of(&res));

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.config.width,
                height: self.config.height,
            },
        );
        let full = TextBounds {
            left: 0,
            top: 0,
            right: self.config.width as i32,
            bottom: self.config.height as i32,
        };
        let mut text_areas = Vec::new();
        for bounds in
            prepared.chrome_text_visible_bounds(scene.header.area, self.cell_w, self.cell_h)
        {
            text_areas.push(TextArea {
                buffer: &self.header_buffer,
                left: header_x,
                top: header_y,
                scale: 1.0,
                bounds,
                default_color: GColor::rgb(header_fg[0], header_fg[1], header_fg[2]),
                custom_glyphs: &[],
            });
        }
        let (floating_occlusion, opaque_overlay_occlusion) =
            prepared.text_occlusions(self.cell_w, self.cell_h);
        for (index, pane_paint) in pane_paints.iter().enumerate() {
            let title_bounds = TextBounds {
                left: pane_paint.title_x.floor() as i32,
                top: pane_paint.title_y.floor() as i32,
                right: (pane_paint.title_x + pane_paint.title_width).ceil() as i32,
                bottom: (pane_paint.title_y + self.cell_h).ceil() as i32,
            };
            for bounds in text_bounds_below_scene_occlusions(
                title_bounds,
                index,
                floating_occlusion,
                opaque_overlay_occlusion,
            ) {
                text_areas.push(TextArea {
                    buffer: &self.title_buffers[index],
                    left: pane_paint.title_x,
                    top: pane_paint.title_y,
                    scale: 1.0,
                    bounds,
                    default_color: GColor::rgb(
                        pane_paint.title_fg[0],
                        pane_paint.title_fg[1],
                        pane_paint.title_fg[2],
                    ),
                    custom_glyphs: &[],
                });
            }
            let visible_bounds = prepared
                .pane_text_visible_bounds(index, self.cell_w, self.cell_h)
                .expect("pane paint index must match the prepared pane");
            for bounds in visible_bounds {
                text_areas.push(TextArea {
                    buffer: &self.text_buffers[index],
                    left: pane_paint.origin_x,
                    top: pane_paint.origin_y,
                    scale: 1.0,
                    bounds,
                    default_color: GColor::rgb(DEFAULT_FG[0], DEFAULT_FG[1], DEFAULT_FG[2]),
                    custom_glyphs: &[],
                });
            }
        }
        for bounds in
            prepared.chrome_text_visible_bounds(scene.status.area, self.cell_w, self.cell_h)
        {
            text_areas.push(TextArea {
                buffer: &self.status_buffer,
                left: status_x + 6.0,
                top: status_y,
                scale: 1.0,
                bounds,
                default_color: GColor::rgb(status_fg[0], status_fg[1], status_fg[2]),
                custom_glyphs: &[],
            });
        }
        if let Some((left, top)) = overlay_position {
            let overlay_fg = resolve(theme.overlay_foreground, DEFAULT_FG);
            text_areas.push(TextArea {
                buffer: &self.overlay_buffer,
                left,
                top,
                scale: 1.0,
                bounds: overlay_clip.unwrap_or(full),
                default_color: GColor::rgb(overlay_fg[0], overlay_fg[1], overlay_fg[2]),
                custom_glyphs: &[],
            });
        }

        if self
            .text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .is_err()
        {
            return Ok(None);
        }

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => return Ok(None),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: DEFAULT_BG[0] as f64 / 255.0,
                            g: DEFAULT_BG[1] as f64 / 255.0,
                            b: DEFAULT_BG[2] as f64 / 255.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if instance_count > 0 {
                pass.set_pipeline(&self.quad_pipeline);
                pass.set_bind_group(0, &self.res_bind_group, &[]);
                pass.set_vertex_buffer(0, self.unit_buf.slice(..));
                pass.set_vertex_buffer(1, self.inst_buf.slice(..));
                pass.draw(0..4, 0..instance_count);
            }
            let _ = self
                .text_renderer
                .render(&self.atlas, &self.viewport, &mut pass);
        }
        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        Ok(Some(Instant::now()))
    }
}

/// Map a scene color onto RGB, using the given default for
/// `SceneColor::Default`, the standard xterm palette for ANSI/indexed colors,
/// and a passthrough for direct RGB.
fn resolve(color: SceneColor, default: [u8; 3]) -> [u8; 3] {
    match color {
        SceneColor::Default => default,
        SceneColor::Rgb(r, g, b) => [r, g, b],
        SceneColor::Ansi(i) => palette(i),
        SceneColor::Indexed(i) => palette(i),
    }
}

fn palette(i: u8) -> [u8; 3] {
    const BASE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [128, 128, 128],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match i {
        0..=15 => BASE[i as usize],
        16..=231 => {
            let n = i - 16;
            let steps = [0u8, 95, 135, 175, 215, 255];
            [
                steps[(n / 36) as usize],
                steps[((n / 6) % 6) as usize],
                steps[(n % 6) as usize],
            ]
        }
        _ => {
            let v = 8 + 10 * (i - 232);
            [v, v, v]
        }
    }
}

fn push_quad(buf: &mut Vec<f32>, x: f32, y: f32, w: f32, h: f32, rgba: [u8; 4]) {
    buf.extend_from_slice(&[
        x,
        y,
        w,
        h,
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    ]);
}

/// Measure a monospace advance width by shaping a run of identical glyphs and
/// dividing the laid-out line width by the glyph count.
fn measure_cell_width(font_system: &mut FontSystem, metrics: Metrics) -> f32 {
    let mut buffer = Buffer::new(font_system, metrics);
    let mono = Attrs::new().family(Family::Monospace);
    buffer.set_text("MMMMMMMMMMMMMMMMMMMM", &mono, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    let width = buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w)
        .unwrap_or(metrics.font_size * 0.6);
    (width / 20.0).max(1.0)
}

fn bytes_of<T: Copy>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) }
}

fn bytes_of_slice<T: Copy>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

const QUAD_WGSL: &str = r#"
struct Res { size: vec4<f32> };
@group(0) @binding(0) var<uniform> res: Res;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs(@location(0) unit: vec2<f32>,
      @location(1) rect: vec4<f32>,
      @location(2) color: vec4<f32>) -> VOut {
    let px = rect.xy + unit * rect.zw;
    let ndc = vec2<f32>(px.x / res.size.x * 2.0 - 1.0, 1.0 - px.y / res.size.y * 2.0);
    var out: VOut;
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use mandatum_scene::{
        AgentContent, AgentStatus, ContextMenuEntry, ContextMenuOverlay, EmptyContent, HeaderScene,
        HelpEntry, HelpOverlay, OverlayScene, PaneId, PaneSceneKind, PromptOverlay, SceneCell,
        SceneRect, SceneSize, SearchEntry, SearchOverlay, StatusScene, TaskContent, TimelineEntry,
        TimelineOverlay, WelcomeEntry, WelcomeOverlay,
    };

    fn terminal_content() -> PaneContent {
        PaneContent::Terminal(TerminalSurface {
            rows: vec![vec![SceneCell::default(); 2]],
            ..TerminalSurface::default()
        })
    }

    fn pane(kind: PaneSceneKind, content: PaneContent) -> PaneScene {
        PaneScene {
            id: PaneId::new("pane-1"),
            title: kind.label().to_owned(),
            kind,
            area: SceneRect::new(0, 0, 2, 1),
            focused: true,
            floating: false,
            stacked: false,
            zoomed: false,
            content,
        }
    }

    fn scene(panes: Vec<PaneScene>) -> WorkspaceScene {
        let focused_pane = panes
            .first()
            .map(|pane| pane.id.clone())
            .unwrap_or_else(|| PaneId::new("none"));
        WorkspaceScene {
            size: SceneSize::new(2, 2),
            header: HeaderScene {
                area: SceneRect::new(0, 0, 2, 0),
                workspace_name: "test".to_owned(),
                session_name: "session".to_owned(),
                pane_count: panes.len(),
                focused_pane: focused_pane.clone(),
                zoomed: false,
                connector_label: "none".to_owned(),
                text: "test header".to_owned(),
                attention: Vec::new(),
            },
            panes,
            overlay: None,
            status: StatusScene {
                area: SceneRect::new(0, 1, 2, 1),
                text: "test status".to_owned(),
            },
            focused_pane,
            hit_targets: Vec::new(),
            copy_mode: false,
        }
    }

    #[test]
    fn current_single_terminal_scene_is_supported_headlessly() {
        let scene = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();

        assert_eq!(prepared.panes[0].pane.id, PaneId::new("pane-1"));
        assert_eq!(prepared.panes[0].terminal.unwrap().rows.len(), 1);
        assert_eq!(scene.status.text, "test status");
    }

    #[test]
    fn task_plan_fits_each_detail_entry_to_one_row_and_keeps_output() {
        let output = TerminalSurface {
            rows: vec![vec![SceneCell::default(); 2]],
            ..TerminalSurface::default()
        };
        let mut task = pane(
            PaneSceneKind::Task,
            PaneContent::Task(TaskContent {
                command: "printf FIRST\nSECOND --verbose".to_owned(),
                cwd_label: "/tmp".to_owned(),
                recipe_label: None,
                status_label: Some("running".to_owned()),
                rerun_hint: None,
                output: Some(output),
            }),
        );
        task.area = SceneRect::new(0, 0, 24, 10);
        let expected_rows = task.detail_lines().len();
        let scene = scene(vec![task]);
        let theme = Theme::default();

        let prepared = prepare_scene(&scene, &theme).unwrap();
        let lines = prepared.pane_text().lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), expected_rows);
        assert!(lines[0].starts_with("command:"), "{:?}", lines[0]);
        assert!(lines[0].ends_with("--verbose"), "{:?}", lines[0]);
        assert_eq!(prepared.panes[0].pane_text_rows, expected_rows);
        assert_eq!(prepared.panes[0].body_wrap, Wrap::None);
        assert!(prepared.pane_surface().is_some());
    }

    #[test]
    fn agent_plan_preserves_wrapping_for_long_scene_detail() {
        let mut agent = pane(
            PaneSceneKind::Agent,
            PaneContent::Agent(AgentContent {
                objective: "inspect a deliberately long objective that needs wrapping".to_owned(),
                status_label: "draft".to_owned(),
                status_role: AgentStatus::Draft,
                pending_approvals: 0,
                changed_file_count: 0,
                changed_files: Vec::new(),
                latest_summary: None,
                current_action: None,
                last_error: None,
                relaunch_hint: None,
                pending_approval: None,
                output_tail: Vec::new(),
            }),
        );
        agent.area = SceneRect::new(0, 0, 20, 10);
        let scene = scene(vec![agent]);
        let theme = Theme::default();

        let prepared = prepare_scene(&scene, &theme).unwrap();

        assert!(prepared.pane_text().contains("deliberately long objective"));
        assert_eq!(prepared.panes[0].body_wrap, Wrap::WordOrGlyph);
        assert!(prepared.pane_surface().is_none());
    }

    #[test]
    fn context_menu_plan_preserves_scene_data_geometry_and_row_alignment() {
        let mut with_menu = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let menu = ContextMenuOverlay {
            area: SceneRect::new(3, 4, 26, 5),
            items: vec![
                ContextMenuEntry::new("Zoom pane", "ctrl+p z"),
                ContextMenuEntry::new("Close pane", "ctrl+p x"),
                ContextMenuEntry::new("Copy selection", ""),
            ],
            selected: 1,
        };
        with_menu.overlay = Some(OverlayScene::ContextMenu(menu.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_menu, &theme).unwrap();
        let prepared_menu = prepared
            .context_menu()
            .expect("context-menu scene data was not retained");

        assert_eq!(prepared_menu, &menu);
        let inner = layout::pane_inner_rect(prepared_menu.area);
        let lines = prepared_menu
            .items
            .iter()
            .map(|item| context_menu_line(item, inner.width))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), usize::from(inner.height));
        assert!(lines[0].starts_with(" Zoom pane"));
        assert!(lines[0].ends_with("ctrl+p z"));
        assert_eq!(lines[0].chars().count(), usize::from(inner.width) - 1);
    }

    #[test]
    fn timeline_plan_preserves_scene_data_geometry_and_row_alignment() {
        let mut with_timeline = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let timeline = TimelineOverlay {
            area: SceneRect::new(3, 4, 54, 8),
            query: "task".to_owned(),
            items: vec![
                TimelineEntry {
                    glyph: "✗".to_owned(),
                    when: "2m ago".to_owned(),
                    text: "task pane-2 failed: exit 3".to_owned(),
                    pane: Some(PaneId::new("pane-2")),
                },
                TimelineEntry {
                    glyph: "▶".to_owned(),
                    when: "3m ago".to_owned(),
                    text: "task pane-2 started".to_owned(),
                    pane: Some(PaneId::new("pane-2")),
                },
            ],
            selected: Some(1),
            skipped_malformed: 1,
            footer: "enter jump · esc close · 1 malformed line(s) skipped".to_owned(),
        };
        with_timeline.overlay = Some(OverlayScene::Timeline(timeline.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_timeline, &theme).unwrap();
        let prepared_timeline = prepared
            .timeline()
            .expect("timeline scene data was not retained");
        let lines = timeline_lines(prepared_timeline);

        assert_eq!(prepared_timeline, &timeline);
        assert_eq!(lines.len(), usize::from(timeline.area.height));
        assert_eq!(lines[0], " Timeline ");
        assert_eq!(lines[1], " > task_");
        assert!(lines[2].contains("✗"));
        assert!(lines[2].contains("2m ago"));
        assert!(lines[2].contains("failed: exit 3"));
        assert!(lines[lines.len() - 2].starts_with("  enter jump"));
        assert!(lines.iter().skip(1).all(|line| {
            line.chars().count() <= usize::from(layout::pane_inner_rect(timeline.area).width) + 1
        }));
        assert!(lines.last().unwrap().is_empty());
    }

    #[test]
    fn timeline_plan_paints_the_empty_filter_state_inside_the_inner_bounds() {
        let mut with_timeline = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let timeline = TimelineOverlay {
            area: SceneRect::new(3, 4, 24, 7),
            query: "missing".to_owned(),
            items: Vec::new(),
            selected: None,
            skipped_malformed: 0,
            footer: "footer text that must stay inside the overlay border".to_owned(),
        };
        with_timeline.overlay = Some(OverlayScene::Timeline(timeline.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_timeline, &theme).unwrap();
        let lines = timeline_lines(prepared.timeline().unwrap());
        let inner_width = usize::from(layout::pane_inner_rect(timeline.area).width);

        assert_eq!(lines[1], " > missing_");
        assert_eq!(lines[2], "  no matching events");
        assert!(lines[lines.len() - 2].starts_with(' '));
        assert!(
            lines
                .iter()
                .skip(1)
                .all(|line| line.chars().count() <= inner_width + 1)
        );
        assert!(lines.last().unwrap().is_empty());
    }

    #[test]
    fn search_plan_preserves_scene_data_grouping_matches_cursor_and_footer() {
        let mut with_search = scene(vec![pane(PaneSceneKind::Agent, terminal_content())]);
        let search = SearchOverlay {
            area: SceneRect::new(3, 4, 64, 8),
            query: "fail".to_owned(),
            items: vec![
                SearchEntry {
                    source: "agent · pane-2 (agent)".to_owned(),
                    text: "first failing check".to_owned(),
                    match_indices: vec![6, 7, 8, 9],
                    pane: Some(PaneId::new("pane-2")),
                },
                SearchEntry {
                    source: "agent · pane-2 (agent)".to_owned(),
                    text: "FAILED tests::search".to_owned(),
                    match_indices: vec![0, 1, 2, 3],
                    pane: Some(PaneId::new("pane-2")),
                },
                SearchEntry {
                    source: "timeline".to_owned(),
                    text: "task pane-3 failed: exit 3".to_owned(),
                    match_indices: vec![12, 13, 14, 15],
                    pane: None,
                },
            ],
            selected: Some(1),
            overflow: 3,
            footer: "+3 beyond cap (narrow the query) · enter jump · esc close".to_owned(),
        };
        with_search.overlay = Some(OverlayScene::Search(search.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_search, &theme).unwrap();
        let prepared_search = prepared
            .search()
            .expect("search scene data was not retained");
        let lines = search_lines(prepared_search);

        assert_eq!(prepared_search, &search);
        assert_eq!(prepared_search.items[1].match_indices, vec![0, 1, 2, 3]);
        assert_eq!(lines.len(), usize::from(search.area.height));
        assert_eq!(lines[0], " Search Session Output ");
        assert_eq!(lines[1], " > fail");
        assert!(lines[2].contains("agent · pane-2 (agent)"));
        assert!(lines[2].contains("first failing check"));
        assert!(lines[3].contains("FAILED tests::search"));
        assert!(!lines[3].contains("agent · pane-2 (agent)"));
        assert!(lines[4].contains("timeline"));
        assert!(lines[4].contains("failed: exit 3"));
        assert!(lines[lines.len() - 2].contains("+3 beyond cap"));
        assert!(lines[lines.len() - 2].contains("enter jump"));
        assert!(lines.last().unwrap().is_empty());
        assert!(lines.iter().skip(1).all(|line| {
            line.chars().count() <= usize::from(layout::pane_inner_rect(search.area).width) + 1
        }));
        assert_eq!(
            search_cursor_cell(prepared_search),
            Some((
                layout::pane_inner_rect(search.area)
                    .x
                    .saturating_add(2 + search.query.chars().count() as u16),
                layout::pane_inner_rect(search.area).y,
            ))
        );
    }

    #[test]
    fn search_plan_paints_empty_states_inside_the_inner_bounds() {
        let mut with_search = scene(vec![pane(PaneSceneKind::Agent, terminal_content())]);
        let search = SearchOverlay {
            area: SceneRect::new(3, 4, 96, 7),
            query: "missing".to_owned(),
            items: Vec::new(),
            selected: None,
            overflow: 0,
            footer: "type to search · enter jump · esc close".to_owned(),
        };
        with_search.overlay = Some(OverlayScene::Search(search.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_search, &theme).unwrap();
        let lines = search_lines(prepared.search().unwrap());
        let inner_width = usize::from(layout::pane_inner_rect(search.area).width);

        assert_eq!(lines[1], " > missing");
        assert_eq!(lines[2], "  no matches");
        assert!(lines[lines.len() - 2].starts_with(' '));
        assert!(
            lines
                .iter()
                .skip(1)
                .all(|line| line.chars().count() <= inner_width + 1)
        );

        let mut empty_query = search;
        empty_query.query.clear();
        with_search.overlay = Some(OverlayScene::Search(empty_query.clone()));
        let prepared = prepare_scene(&with_search, &theme).unwrap();
        let lines = search_lines(prepared.search().unwrap());
        assert!(lines[1].contains("type to search output"));
        assert!(lines[2].contains("searching this session"));
        assert_eq!(search_cursor_cell(&empty_query), None);
    }

    #[test]
    fn search_occlusion_keeps_base_text_outside_the_overlay_only() {
        let content = TextBounds {
            left: 0,
            top: 10,
            right: 100,
            bottom: 90,
        };
        let search = TextBounds {
            left: 20,
            top: 30,
            right: 80,
            bottom: 70,
        };

        assert_eq!(
            text_bounds_around_occlusion(content, search),
            vec![
                TextBounds {
                    bottom: 30,
                    ..content
                },
                TextBounds { top: 70, ..content },
                TextBounds {
                    top: 30,
                    right: 20,
                    bottom: 70,
                    ..content
                },
                TextBounds {
                    left: 80,
                    top: 30,
                    bottom: 70,
                    ..content
                },
            ]
        );
        assert_eq!(
            text_bounds_around_occlusion(
                content,
                TextBounds {
                    left: 110,
                    top: 10,
                    right: 120,
                    bottom: 20,
                }
            ),
            vec![content]
        );
    }

    fn opaque_overlay_cases(area: SceneRect) -> [(&'static str, OverlayScene); 8] {
        [
            ("palette", palette_overlay(area)),
            ("context menu", context_menu_overlay(area)),
            ("timeline", timeline_overlay(area)),
            ("search", search_overlay(area)),
            ("session map", session_map_overlay(area)),
            ("prompt", prompt_overlay(area)),
            ("help", help_overlay(area)),
            ("welcome", welcome_overlay(area)),
        ]
    }

    fn palette_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Palette(PaletteOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        })
    }

    fn context_menu_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::ContextMenu(ContextMenuOverlay {
            area,
            items: Vec::new(),
            selected: 0,
        })
    }

    fn timeline_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Timeline(TimelineOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            skipped_malformed: 0,
            footer: String::new(),
        })
    }

    fn search_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Search(SearchOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            overflow: 0,
            footer: String::new(),
        })
    }

    fn session_map_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::SessionMap(SessionMapOverlay {
            area,
            rows: Vec::new(),
            selected: 0,
            footer: String::new(),
        })
    }

    fn prompt_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Prompt(PromptOverlay {
            area,
            title: String::new(),
            input: String::new(),
            footer: String::new(),
        })
    }

    fn help_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Help(HelpOverlay {
            area,
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        })
    }

    fn welcome_overlay(area: SceneRect) -> OverlayScene {
        OverlayScene::Welcome(WelcomeOverlay {
            area,
            introduction: String::new(),
            entries: Vec::new(),
            dismissal: String::new(),
        })
    }

    #[test]
    fn opaque_surfaces_clip_pane_bodies_in_final_fractional_pixel_bounds() {
        let cell_width = 7.3;
        let cell_height = 13.5;
        let overlay_area = SceneRect::new(5, 3, 10, 5);

        for (label, overlay) in opaque_overlay_cases(overlay_area) {
            let mut base = pane(
                PaneSceneKind::StatusLog,
                PaneContent::Empty(EmptyContent {
                    cwd_label: "/tmp".to_owned(),
                    restart_generation: 0,
                }),
            );
            base.area = SceneRect::new(0, 1, 20, 10);
            let mut candidate = scene(vec![base]);
            candidate.size = SceneSize::new(20, 12);
            candidate.overlay = Some(overlay);

            let theme = Theme::default();
            let prepared = prepare_scene(&candidate, &theme).unwrap();
            let occlusion = scene_rect_to_text_bounds(overlay_area, cell_width, cell_height);
            let visible = prepared
                .pane_text_visible_bounds(0, cell_width, cell_height)
                .unwrap();

            assert!(
                visible
                    .iter()
                    .all(|bounds| !text_bounds_intersect(*bounds, occlusion)),
                "{label} left pane-body pixels inside its opaque surface: {visible:?}"
            );
            assert!(
                visible.iter().any(|bounds| bounds.right == occlusion.left),
                "{label} did not preserve the pixel column immediately left of the surface"
            );
        }
    }

    #[test]
    fn full_frame_overlay_clips_header_and_status_glyphs() {
        let size = SceneSize::new(3, 3);
        let mut base = pane(
            PaneSceneKind::StatusLog,
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            }),
        );
        base.area = layout::workspace_scene_area(size);
        let mut candidate = scene(vec![base]);
        candidate.size = size;
        candidate.header.area = layout::header_rect(size);
        candidate.status.area = layout::status_rect(size);
        candidate.overlay = Some(search_overlay(SceneRect::new(0, 0, 3, 3)));

        let theme = Theme::default();
        let prepared = prepare_scene(&candidate, &theme).unwrap();

        assert!(
            prepared
                .chrome_text_visible_bounds(candidate.header.area, 7.3, 13.5)
                .is_empty()
        );
        assert!(
            prepared
                .chrome_text_visible_bounds(candidate.status.area, 7.3, 13.5)
                .is_empty()
        );
    }

    #[test]
    fn floating_surface_clips_tiled_pane_body_in_final_fractional_pixel_bounds() {
        let cell_width = 7.3;
        let cell_height = 13.5;
        let size = SceneSize::new(20, 12);
        let workspace = layout::workspace_scene_area(size);
        let floating_area = layout::default_floating_pane_rect(size);
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut tiled = pane(PaneSceneKind::Terminal, empty());
        tiled.area = workspace;
        tiled.focused = false;
        let mut floating = pane(PaneSceneKind::Terminal, empty());
        floating.id = PaneId::new("pane-2");
        floating.title = "terminal 2".to_owned();
        floating.area = floating_area;
        floating.floating = true;
        let mut candidate = scene(vec![tiled, floating]);
        candidate.size = size;

        let theme = Theme::default();
        let prepared = prepare_scene(&candidate, &theme).unwrap();
        let occlusion = scene_rect_to_text_bounds(floating_area, cell_width, cell_height);
        let visible = prepared
            .pane_text_visible_bounds(0, cell_width, cell_height)
            .unwrap();

        assert!(
            visible
                .iter()
                .all(|bounds| !text_bounds_intersect(*bounds, occlusion)),
            "tiled pane-body pixels remain inside the opaque float: {visible:?}"
        );
        assert!(
            visible.iter().any(|bounds| bounds.right == occlusion.left),
            "the pixel column immediately left of the float was not preserved"
        );
    }

    #[test]
    fn palette_occlusion_clips_pane_titles_at_small_admitted_viewports() {
        let size = SceneSize::new(9, 5);
        let palette = layout::palette_overlay_rect(size);
        assert_eq!(palette, SceneRect::new(1, 1, 7, 3));
        let title = TextBounds {
            left: 1,
            top: 1,
            right: 3,
            bottom: 2,
        };
        let palette_bounds = TextBounds {
            left: i32::from(palette.x),
            top: i32::from(palette.y),
            right: i32::from(palette.right()),
            bottom: i32::from(palette.bottom()),
        };

        assert!(
            text_bounds_below_scene_occlusions(title, 0, None, Some(palette_bounds)).is_empty()
        );
    }

    #[test]
    fn floating_pane_occlusion_clips_long_wrapped_empty_detail_from_the_tiled_pane() {
        let empty = PaneContent::Empty(EmptyContent {
            cwd_label: format!("/{}", "long-project-segment/".repeat(8)),
            restart_generation: 0,
        });
        let mut tiled = pane(PaneSceneKind::StatusLog, empty);
        tiled.area = SceneRect::new(0, 1, 80, 22);
        let scene = scene(vec![tiled]);
        let theme = Theme::default();
        let prepared = prepare_scene(&scene, &theme).unwrap();
        assert!(
            prepared.pane_text().chars().count()
                > usize::from(layout::pane_inner_rect(scene.panes[0].area).width)
        );
        assert_eq!(prepared.panes[0].body_wrap, Wrap::WordOrGlyph);

        let tiled_body = TextBounds {
            left: 1,
            top: 2,
            right: 79,
            bottom: 22,
        };
        let floating_pane = TextBounds {
            left: 8,
            top: 5,
            right: 80,
            bottom: 23,
        };
        let visible = text_bounds_below_later_float(tiled_body, 0, Some((1, floating_pane)));

        assert_eq!(
            visible,
            vec![
                TextBounds {
                    bottom: 5,
                    ..tiled_body
                },
                TextBounds {
                    top: 5,
                    right: 8,
                    ..tiled_body
                },
            ]
        );
        assert!(visible.iter().all(|bounds| {
            bounds.right <= floating_pane.left
                || bounds.bottom <= floating_pane.top
                || bounds.left >= floating_pane.right
                || bounds.top >= floating_pane.bottom
        }));
        assert_eq!(
            text_bounds_below_later_float(tiled_body, 1, Some((1, floating_pane))),
            vec![tiled_body]
        );
    }

    #[test]
    fn help_plan_preserves_scene_data_grouping_keys_cursor_selection_and_footer() {
        let mut with_help = scene(vec![pane(
            PaneSceneKind::StatusLog,
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            }),
        )]);
        let help = HelpOverlay {
            area: SceneRect::new(3, 4, 56, 8),
            query: "search".to_owned(),
            items: vec![
                HelpEntry {
                    heading: true,
                    label: "App".to_owned(),
                    keys: String::new(),
                },
                HelpEntry {
                    heading: false,
                    label: "Search session output".to_owned(),
                    keys: "ctrl+shift+f".to_owned(),
                },
            ],
            selected: Some(1),
            footer: "type to filter · ↑/↓ scroll · esc close".to_owned(),
        };
        with_help.overlay = Some(OverlayScene::Help(help.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_help, &theme).unwrap();
        let prepared_help = prepared.help().expect("help scene data was not retained");
        let lines = help_lines(prepared_help);

        assert_eq!(prepared_help, &help);
        assert_eq!(lines.len(), usize::from(help.area.height));
        assert_eq!(lines[0], " Help ");
        assert_eq!(lines[1], " > search");
        assert_eq!(lines[2], "  App");
        assert!(lines[3].starts_with("    Search session output"));
        assert!(lines[3].ends_with("ctrl+shift+f"));
        assert_eq!(
            lines[lines.len() - 2],
            "  type to filter · ↑/↓ scroll · esc close"
        );
        assert!(lines.last().unwrap().is_empty());
        assert!(lines.iter().skip(1).all(|line| {
            line.chars().count() <= usize::from(layout::pane_inner_rect(help.area).width) + 1
        }));
        assert_eq!(
            help_cursor_cell(prepared_help),
            Some((
                layout::pane_inner_rect(help.area)
                    .x
                    .saturating_add(2 + help.query.chars().count() as u16),
                layout::pane_inner_rect(help.area).y,
            ))
        );

        let mut no_matches = help;
        no_matches.query.clear();
        no_matches.items.clear();
        no_matches.selected = None;
        with_help.overlay = Some(OverlayScene::Help(no_matches.clone()));
        let prepared = prepare_scene(&with_help, &theme).unwrap();
        let lines = help_lines(prepared.help().unwrap());
        assert!(lines[1].contains("type to filter the keymap"));
        assert_eq!(lines[2], "  no matching entries");
        assert_eq!(help_cursor_cell(&no_matches), None);
    }

    #[test]
    fn welcome_plan_preserves_scene_data_hierarchy_alignment_and_bounds() {
        let mut with_welcome = scene(vec![pane(
            PaneSceneKind::StatusLog,
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            }),
        )]);
        let welcome = WelcomeOverlay {
            area: SceneRect::new(3, 4, 48, 8),
            introduction: "A workspace for terminals, tasks, and agents.".to_owned(),
            entries: vec![
                WelcomeEntry {
                    keys: "ctrl+p".to_owned(),
                    description: "Command palette".to_owned(),
                },
                WelcomeEntry {
                    keys: "f1".to_owned(),
                    description: "Help".to_owned(),
                },
            ],
            dismissal: "Any key or click dismisses this note".to_owned(),
        };
        with_welcome.overlay = Some(OverlayScene::Welcome(welcome.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_welcome, &theme).unwrap();
        let prepared_welcome = prepared
            .welcome()
            .expect("welcome scene data was not retained");
        let lines = welcome_lines(prepared_welcome);

        assert_eq!(prepared_welcome, &welcome);
        assert_eq!(lines.len(), usize::from(welcome.area.height));
        assert_eq!(lines[0], " Mandatum ");
        assert_eq!(lines[1], " A workspace for terminals, tasks, and agents.");
        assert!(lines[2].is_empty());
        assert_eq!(lines[3], "   ctrl+p  Command palette");
        assert_eq!(lines[4], "   f1      Help");
        assert!(lines[5].is_empty());
        assert_eq!(lines[6], " Any key or click dismisses this note");
        assert!(lines.last().unwrap().is_empty());
        assert!(lines.iter().skip(1).all(|line| {
            line.chars().count() <= usize::from(layout::pane_inner_rect(welcome.area).width) + 1
        }));
    }

    #[test]
    fn session_map_plan_preserves_scene_data_geometry_and_row_alignment() {
        let mut with_map = scene(vec![pane(PaneSceneKind::Terminal, terminal_content())]);
        let map = SessionMapOverlay {
            area: SceneRect::new(3, 4, 64, 7),
            rows: vec![
                SessionMapRow {
                    depth: 0,
                    glyph: "▸".to_owned(),
                    label: "session-1 · project · 2 pane(s) (active)".to_owned(),
                    state: String::new(),
                    focused: false,
                    badges: String::new(),
                },
                SessionMapRow {
                    depth: 1,
                    glyph: "❯".to_owned(),
                    label: "pane-1 terminal".to_owned(),
                    state: "open".to_owned(),
                    focused: true,
                    badges: "zoom float".to_owned(),
                },
            ],
            selected: 1,
            footer: "↑/↓ move · enter focus · esc close".to_owned(),
        };
        with_map.overlay = Some(OverlayScene::SessionMap(map.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_map, &theme).unwrap();
        let prepared_map = prepared
            .session_map()
            .expect("session-map scene data was not retained");
        let lines = session_map_lines(prepared_map);

        assert_eq!(prepared_map, &map);
        assert_eq!(lines.len(), usize::from(map.area.height));
        assert_eq!(lines[0], " Sessions ");
        assert!(lines[1].contains("▸ session-1"));
        assert!(lines[2].contains("●  ❯ pane-1 terminal"));
        assert!(lines[2].contains("open"));
        assert!(lines[2].contains("[zoom float]"));
        assert!(lines[lines.len() - 2].starts_with("  ↑/↓ move"));
        assert!(lines.iter().skip(1).all(|line| {
            line.chars().count() <= usize::from(layout::pane_inner_rect(map.area).width) + 1
        }));
        assert!(lines.last().unwrap().is_empty());
    }

    #[test]
    fn prompt_plan_preserves_scene_data_geometry_cursor_and_footer() {
        let mut with_prompt = scene(vec![pane(PaneSceneKind::Agent, terminal_content())]);
        let prompt = PromptOverlay {
            area: SceneRect::new(3, 4, 42, 5),
            title: " Set agent objective — pane-1 ".to_owned(),
            input: "Inspect prompt paint".to_owned(),
            footer: "enter save · esc cancel".to_owned(),
        };
        with_prompt.overlay = Some(OverlayScene::Prompt(prompt.clone()));

        let theme = Theme::default();
        let prepared = prepare_scene(&with_prompt, &theme).unwrap();
        let prepared_prompt = prepared
            .prompt()
            .expect("prompt scene data was not retained");
        let lines = prompt_lines(prepared_prompt);

        assert_eq!(prepared_prompt, &prompt);
        assert_eq!(lines.len(), usize::from(prompt.area.height));
        assert_eq!(lines[0], prompt.title);
        assert_eq!(lines[1], " > Inspect prompt paint");
        assert_eq!(lines[lines.len() - 2], " enter save · esc cancel");
        assert!(lines.last().unwrap().is_empty());
        assert_eq!(
            prompt_cursor_cell(prepared_prompt),
            Some((
                layout::pane_inner_rect(prompt.area)
                    .x
                    .saturating_add(2 + prompt.input.chars().count() as u16),
                layout::pane_inner_rect(prompt.area).y,
            ))
        );
    }

    #[test]
    fn unsupported_two_pane_content_fails_explicitly() {
        let multiple = scene(vec![
            pane(PaneSceneKind::Terminal, terminal_content()),
            pane(PaneSceneKind::Terminal, terminal_content()),
        ]);
        assert_eq!(
            prepare_scene(&multiple, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );
    }

    #[test]
    fn horizontal_empty_palette_admission_requires_the_scene_resolved_area() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut first = pane(PaneSceneKind::Terminal, empty());
        first.area = SceneRect::new(0, 1, 40, 22);
        first.focused = false;
        let mut second = pane(PaneSceneKind::Terminal, empty());
        second.id = PaneId::new("pane-2");
        second.title = "terminal 2".to_owned();
        second.area = SceneRect::new(40, 1, 40, 22);
        let mut candidate = scene(vec![first, second]);
        candidate.size = SceneSize::new(80, 24);
        let mut palette = PaletteOverlay {
            area: layout::palette_overlay_rect(candidate.size),
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        };
        candidate.overlay = Some(OverlayScene::Palette(palette.clone()));
        assert!(prepare_scene(&candidate, &Theme::default()).is_ok());

        palette.area.x = palette.area.x.saturating_add(1);
        candidate.overlay = Some(OverlayScene::Palette(palette));
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );
    }

    #[test]
    fn horizontal_empty_admission_requires_usable_bordered_pane_interiors() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut first = pane(PaneSceneKind::Terminal, empty());
        first.area = SceneRect::new(0, 1, 3, 3);
        first.focused = false;
        let mut second = pane(PaneSceneKind::Terminal, empty());
        second.id = PaneId::new("pane-2");
        second.title = "terminal 2".to_owned();
        second.area = SceneRect::new(3, 1, 3, 3);
        let mut candidate = scene(vec![first, second]);
        candidate.size = SceneSize::new(6, 5);

        assert!(prepare_scene(&candidate, &Theme::default()).is_ok());

        candidate.size = SceneSize::new(5, 5);
        candidate.panes[0].area = SceneRect::new(0, 1, 3, 3);
        candidate.panes[1].area = SceneRect::new(3, 1, 2, 3);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );

        candidate.size = SceneSize::new(6, 4);
        candidate.panes[0].area = SceneRect::new(0, 1, 3, 2);
        candidate.panes[1].area = SceneRect::new(3, 1, 3, 2);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );

        candidate.size = SceneSize::new(80, 24);
        candidate.panes[0].area = SceneRect::new(0, 1, 2, 22);
        candidate.panes[1].area = SceneRect::new(2, 1, 78, 22);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );
    }

    #[test]
    fn vertical_empty_admission_rejects_every_unsupported_shape() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut first = pane(PaneSceneKind::Terminal, empty());
        first.area = SceneRect::new(0, 1, 80, 11);
        first.focused = false;
        let mut second = pane(PaneSceneKind::Terminal, empty());
        second.id = PaneId::new("pane-2");
        second.title = "terminal 2".to_owned();
        second.area = SceneRect::new(0, 12, 80, 11);
        let mut valid = scene(vec![first, second]);
        valid.size = SceneSize::new(80, 24);

        let mut cases = Vec::new();

        let mut overlay = valid.clone();
        overlay.overlay = Some(OverlayScene::Palette(PaletteOverlay {
            area: SceneRect::new(10, 5, 20, 5),
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        }));
        cases.push(("overlay", overlay));

        for (index, flag) in [
            (0, "first floating"),
            (1, "second floating"),
            (0, "first stacked"),
            (1, "second stacked"),
            (0, "first zoomed"),
            (1, "second zoomed"),
        ] {
            let mut flagged = valid.clone();
            match flag {
                "first floating" | "second floating" => flagged.panes[index].floating = true,
                "first stacked" | "second stacked" => flagged.panes[index].stacked = true,
                "first zoomed" | "second zoomed" => flagged.panes[index].zoomed = true,
                _ => unreachable!(),
            }
            cases.push((flag, flagged));
        }

        let mut gap = valid.clone();
        gap.panes[0].area.height = 10;
        cases.push(("non-adjacent gap", gap));

        let mut overlap = valid.clone();
        overlap.panes[1].area.y = 11;
        overlap.panes[1].area.height = 12;
        cases.push(("non-adjacent overlap", overlap));

        let mut off_workspace = valid.clone();
        off_workspace.panes[1].area.width = 79;
        cases.push(("off-workspace edge", off_workspace));

        let mut mixed_content = valid;
        mixed_content.panes[1].content = terminal_content();
        cases.push(("mixed content", mixed_content));

        for (label, candidate) in cases {
            assert_eq!(
                prepare_scene(&candidate, &Theme::default()).unwrap_err(),
                UnsupportedScene::Layout(
                    "only two horizontal or vertical tiled, or default floating Empty panes"
                ),
                "{label}"
            );
        }
    }

    #[test]
    fn vertical_empty_admission_requires_usable_bordered_pane_interiors() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut first = pane(PaneSceneKind::Terminal, empty());
        first.area = SceneRect::new(0, 1, 3, 3);
        first.focused = false;
        let mut second = pane(PaneSceneKind::Terminal, empty());
        second.id = PaneId::new("pane-2");
        second.title = "terminal 2".to_owned();
        second.area = SceneRect::new(0, 4, 3, 3);
        let mut candidate = scene(vec![first, second]);
        candidate.size = SceneSize::new(3, 8);

        assert!(prepare_scene(&candidate, &Theme::default()).is_ok());

        candidate.size = SceneSize::new(3, 7);
        candidate.panes[1].area = SceneRect::new(0, 4, 3, 2);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );

        candidate.size = SceneSize::new(2, 8);
        candidate.panes[0].area = SceneRect::new(0, 1, 2, 3);
        candidate.panes[1].area = SceneRect::new(0, 4, 2, 3);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );

        candidate.size = SceneSize::new(80, 24);
        candidate.panes[0].area = SceneRect::new(0, 1, 80, 2);
        candidate.panes[1].area = SceneRect::new(0, 3, 80, 20);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );
    }

    #[test]
    fn floating_empty_admission_rejects_every_unsupported_shape() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut first = pane(PaneSceneKind::Terminal, empty());
        first.area = SceneRect::new(0, 1, 80, 22);
        first.focused = false;
        let mut second = pane(PaneSceneKind::Terminal, empty());
        second.id = PaneId::new("pane-2");
        second.title = "terminal 2".to_owned();
        second.area = SceneRect::new(8, 5, 72, 18);
        second.floating = true;
        let mut valid = scene(vec![first, second]);
        valid.size = SceneSize::new(80, 24);

        let mut cases = Vec::new();

        let mut overlay = valid.clone();
        overlay.overlay = Some(OverlayScene::Palette(PaletteOverlay {
            area: SceneRect::new(10, 5, 20, 5),
            query: String::new(),
            items: Vec::new(),
            selected: None,
            footer: String::new(),
        }));
        cases.push(("overlay", overlay));

        for (index, flag) in [
            (0, "first floating"),
            (0, "first stacked"),
            (0, "first zoomed"),
            (1, "second tiled"),
            (1, "second stacked"),
            (1, "second zoomed"),
        ] {
            let mut flagged = valid.clone();
            match flag {
                "first floating" => flagged.panes[index].floating = true,
                "first stacked" | "second stacked" => flagged.panes[index].stacked = true,
                "first zoomed" | "second zoomed" => flagged.panes[index].zoomed = true,
                "second tiled" => flagged.panes[index].floating = false,
                _ => unreachable!(),
            }
            cases.push((flag, flagged));
        }

        for (label, index, area) in [
            (
                "tiled pane does not fill workspace",
                0,
                SceneRect::new(1, 1, 79, 22),
            ),
            ("floating x", 1, SceneRect::new(7, 5, 73, 18)),
            ("floating y", 1, SceneRect::new(8, 4, 72, 19)),
            ("floating width", 1, SceneRect::new(8, 5, 71, 18)),
            ("floating height", 1, SceneRect::new(8, 5, 72, 17)),
        ] {
            let mut geometry = valid.clone();
            geometry.panes[index].area = area;
            cases.push((label, geometry));
        }

        let mut mixed_first = valid.clone();
        mixed_first.panes[0].content = terminal_content();
        cases.push(("mixed first content", mixed_first));

        let mut mixed_second = valid;
        mixed_second.panes[1].content = terminal_content();
        cases.push(("mixed second content", mixed_second));

        for (label, candidate) in cases {
            assert_eq!(
                prepare_scene(&candidate, &Theme::default()).unwrap_err(),
                UnsupportedScene::Layout(
                    "only two horizontal or vertical tiled, or default floating Empty panes"
                ),
                "{label}"
            );
        }
    }

    #[test]
    fn floating_empty_admission_requires_usable_bordered_pane_interiors() {
        let empty = || {
            PaneContent::Empty(EmptyContent {
                cwd_label: "/tmp".to_owned(),
                restart_generation: 0,
            })
        };
        let mut tiled = pane(PaneSceneKind::Terminal, empty());
        tiled.area = SceneRect::new(0, 1, 11, 7);
        tiled.focused = false;
        let mut floating = pane(PaneSceneKind::Terminal, empty());
        floating.id = PaneId::new("pane-2");
        floating.title = "terminal 2".to_owned();
        floating.area = SceneRect::new(8, 5, 3, 3);
        floating.floating = true;
        let mut candidate = scene(vec![tiled, floating]);
        candidate.size = SceneSize::new(11, 9);

        assert!(prepare_scene(&candidate, &Theme::default()).is_ok());

        candidate.size = SceneSize::new(10, 9);
        candidate.panes[0].area = SceneRect::new(0, 1, 10, 7);
        candidate.panes[1].area = SceneRect::new(8, 5, 2, 3);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );

        candidate.size = SceneSize::new(11, 8);
        candidate.panes[0].area = SceneRect::new(0, 1, 11, 6);
        candidate.panes[1].area = SceneRect::new(8, 5, 3, 2);
        assert_eq!(
            prepare_scene(&candidate, &Theme::default()).unwrap_err(),
            UnsupportedScene::Layout(
                "only two horizontal or vertical tiled, or default floating Empty panes"
            )
        );
    }

    #[test]
    fn empty_plan_preserves_wrapping_for_scene_detail() {
        let empty = PaneContent::Empty(EmptyContent {
            cwd_label: "/tmp".to_owned(),
            restart_generation: 7,
        });
        let mut empty_pane = pane(PaneSceneKind::StatusLog, empty);
        empty_pane.area = SceneRect::new(0, 0, 20, 10);
        let scene = scene(vec![empty_pane]);
        let theme = Theme::default();

        let prepared = prepare_scene(&scene, &theme).unwrap();

        assert!(prepared.pane_text().contains("cwd: /tmp"));
        assert!(prepared.pane_text().contains("restart generation: 7"));
        assert!(
            prepared
                .pane_text()
                .contains("no live PTY grid is attached to this pane")
        );
        assert_eq!(prepared.panes[0].body_wrap, Wrap::WordOrGlyph);
        assert!(prepared.pane_surface().is_none());
    }
}
