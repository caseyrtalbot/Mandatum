//! Help overlay content, generated — never hand-written prose that can
//! drift. Command rows come from `mandatum_commands::BUILT_IN_COMMANDS`
//! joined with the LIVE keymap (rebinds included); glyph legends come from
//! the same tables the session map and timeline draw from; only behavior
//! facts with no data source (palette fast-path rules, mouse gestures) are
//! written here, next to the code that implements them.

use mandatum_commands::{BUILT_IN_COMMANDS, CommandCategory, CommandId, fuzzy::fuzzy_match};
use mandatum_scene::HelpEntry;

use crate::keymap::{Keymap, format_chord};
use crate::session_map::SESSION_MAP_GLYPH_LEGEND;
use crate::timeline::TIMELINE_GLYPH_LEGEND;

/// Live overlay state while help is open. Runtime presentation only.
#[derive(Default)]
pub(crate) struct HelpViewState {
    pub(crate) query: String,
    pub(crate) selected: usize,
}

/// Section order and headings for the command categories.
const CATEGORY_SECTIONS: &[(CommandCategory, &str)] = &[
    (CommandCategory::Project, "Project"),
    (CommandCategory::Pane, "Panes"),
    (CommandCategory::Task, "Tasks"),
    (CommandCategory::Agent, "Agents"),
    (CommandCategory::Layout, "Layout"),
    (CommandCategory::Persistence, "Persistence"),
    (CommandCategory::Config, "Config"),
    (CommandCategory::App, "App"),
];

/// Every help row for the live keymap, unfiltered: workspace control, the
/// command table grouped by category with current key routes, palette
/// fast-path rules, mouse gestures (with the L5 note), and the glyph
/// legends.
pub(crate) fn help_rows(keymap: &Keymap) -> Vec<HelpEntry> {
    let mut rows = Vec::new();
    let palette_chord = format_chord(keymap.toggle_palette);

    rows.push(heading("Workspace control"));
    rows.push(entry("Command palette", palette_chord.clone()));
    rows.push(entry("Quit", format_chord(keymap.quit)));

    for (category, section) in CATEGORY_SECTIONS {
        let commands: Vec<_> = BUILT_IN_COMMANDS
            .iter()
            .filter(|command| command.category == *category)
            .collect();
        if commands.is_empty() {
            continue;
        }
        rows.push(heading(*section));
        for command in commands {
            rows.push(entry(command.label, key_route(command.id, keymap)));
        }
        if *category == CommandCategory::Agent {
            // Direct keys: an agent pane has no terminal input to shadow.
            rows.push(entry(
                "Approve directly (focused pane awaits approval)",
                "y".to_owned(),
            ));
            rows.push(entry(
                "Reject directly (focused pane awaits approval)",
                "n".to_owned(),
            ));
        }
    }

    rows.push(heading("Palette fast paths"));
    rows.push(entry(
        "With an empty input, a bound letter runs its command",
        String::new(),
    ));
    rows.push(entry(
        "Shift+letter (or any unbound key) starts the fuzzy filter",
        String::new(),
    ));
    rows.push(entry(
        "Tab / BackTab cycle pane focus while the input is empty",
        String::new(),
    ));
    rows.push(entry(
        "Up/Down or Ctrl+N/Ctrl+P move · Enter run · Esc close",
        String::new(),
    ));

    rows.push(heading("Mouse"));
    rows.push(entry(
        "Click focuses a pane; double-click zooms",
        String::new(),
    ));
    rows.push(entry(
        "Drag a split separator to resize (keys: Grow/Shrink pane)",
        String::new(),
    ));
    rows.push(entry(
        "Drag a floating pane's title to move it (keys: Move float)",
        String::new(),
    ));
    rows.push(entry(
        "Wheel scrolls history; drag selects text (keys: copy mode)",
        String::new(),
    ));
    rows.push(entry(
        "Right-click opens the pane menu; click the status strip for commands",
        String::new(),
    ));
    rows.push(entry(
        "When a child app captures the mouse, alt+click / alt+drag reaches the workspace",
        String::new(),
    ));

    rows.push(heading("Glyphs · session map"));
    for (glyph, meaning) in SESSION_MAP_GLYPH_LEGEND {
        rows.push(entry(format!("{glyph}  {meaning}"), String::new()));
    }
    rows.push(heading("Glyphs · timeline"));
    for (glyph, meaning) in TIMELINE_GLYPH_LEGEND {
        rows.push(entry(format!("{glyph}  {meaning}"), String::new()));
    }

    rows
}

/// Filter help rows with the palette input pattern: non-heading rows
/// fuzzy-match on label and keys; a heading survives when any row in its
/// section matches. An empty query keeps everything.
pub(crate) fn filter_help_rows(rows: &[HelpEntry], query: &str) -> Vec<HelpEntry> {
    let needle = query.trim();
    if needle.is_empty() {
        return rows.to_vec();
    }
    let mut filtered = Vec::new();
    let mut pending_heading: Option<&HelpEntry> = None;
    for row in rows {
        if row.heading {
            pending_heading = Some(row);
            continue;
        }
        let haystack = format!("{} {}", row.label, row.keys);
        if fuzzy_match(needle, &haystack).is_some() {
            if let Some(heading) = pending_heading.take() {
                filtered.push(heading.clone());
            }
            filtered.push(row.clone());
        }
    }
    filtered
}

/// The current keyboard route(s) to a command: its global chord if bound,
/// its palette letter spelled as "<palette chord> <letter>", else the honest
/// fallback (every command is reachable by typing in the palette).
fn key_route(command_id: CommandId, keymap: &Keymap) -> String {
    let mut routes = Vec::new();
    if command_id == CommandId::Quit {
        routes.push(format_chord(keymap.quit));
    }
    if let Some(chord) = keymap.chord_for(command_id) {
        routes.push(format_chord(chord));
    }
    // Dock rides the float letter (one toggle key for the pair).
    let letter_owner = if command_id == CommandId::DockPane {
        CommandId::FloatPane
    } else {
        command_id
    };
    if let Some(letter) = keymap.palette.key_for(letter_owner) {
        routes.push(format!("{} {letter}", format_chord(keymap.toggle_palette)));
    }
    if routes.is_empty() {
        routes.push("palette (type to search)".to_owned());
    }
    routes.join(" · ")
}

/// The shortest live route to Help itself, for the status-strip hint and
/// the first-run note.
pub(crate) fn help_route(keymap: &Keymap) -> String {
    if let Some(chord) = keymap.chord_for(CommandId::ShowHelp) {
        return format_chord(chord);
    }
    if let Some(letter) = keymap.palette.key_for(CommandId::ShowHelp) {
        return format!("{} {letter}", format_chord(keymap.toggle_palette));
    }
    "palette: help".to_owned()
}

/// The one-time first-run note: under 8 lines, generated from the live
/// keymap so a config that rebinds chords is never contradicted.
pub(crate) fn welcome_lines(keymap: &Keymap) -> Vec<String> {
    vec![
        "A workspace for terminals, tasks, and agents.".to_owned(),
        String::new(),
        format!(
            "  {}  command palette (every command, searchable)",
            format_chord(keymap.toggle_palette)
        ),
        "  right-click  pane menu".to_owned(),
        format!("  {}  help: keys, mouse, glyphs", help_route(keymap)),
        format!("  {}  quit", format_chord(keymap.quit)),
        String::new(),
        "any key or click dismisses this note".to_owned(),
    ]
}

fn heading(label: impl Into<String>) -> HelpEntry {
    HelpEntry {
        heading: true,
        label: label.into(),
        keys: String::new(),
    }
}

fn entry(label: impl Into<String>, keys: String) -> HelpEntry {
    HelpEntry {
        heading: false,
        label: label.into(),
        keys,
    }
}

#[cfg(test)]
mod tests {
    use mandatum_commands::command_for_id;
    use mandatum_scene::input::{Key, KeyCode};

    use super::*;
    use crate::keymap::parse_chord;

    fn row<'a>(rows: &'a [HelpEntry], label: &str) -> &'a HelpEntry {
        rows.iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("row {label:?} must be listed"))
    }

    #[test]
    fn every_built_in_command_is_listed_with_a_key_route() {
        let rows = help_rows(&Keymap::default());
        for command in BUILT_IN_COMMANDS {
            let entry = row(&rows, command.label);
            assert!(
                !entry.keys.is_empty(),
                "{} must name a route (a chord, a palette letter, or the search fallback)",
                command.label
            );
        }
        // Defaults: palette letters spell the palette chord, Help shows F1.
        assert_eq!(row(&rows, "Split pane right").keys, "ctrl+p v");
        assert_eq!(row(&rows, "Help").keys, "f1 · ctrl+p ?");
        assert_eq!(row(&rows, "Quit Mandatum").keys, "ctrl+q · ctrl+p q");
        // Commands with no letter state the honest fallback.
        assert_eq!(
            row(&rows, "Move float left").keys,
            "palette (type to search)"
        );
    }

    #[test]
    fn help_reflects_a_rebound_key_not_the_default() {
        let mut keymap = Keymap::default();
        keymap.bind_chord(CommandId::SplitRight, parse_chord("ctrl+shift+r").unwrap());
        keymap.palette.rebind(CommandId::SplitRight, 'u');
        // Rebinding Help off F1 must update its row and the help route.
        keymap.bind_chord(CommandId::ShowHelp, parse_chord("alt+h").unwrap());

        let rows = help_rows(&keymap);
        assert_eq!(
            row(&rows, "Split pane right").keys,
            "ctrl+shift+r · ctrl+p u"
        );
        assert_eq!(row(&rows, "Help").keys, "alt+h · ctrl+p ?");
        assert_eq!(help_route(&keymap), "alt+h");
        assert_eq!(
            keymap.chord_action(Key::plain(KeyCode::Function(1))),
            None,
            "the old F1 binding is released by the rebind"
        );
    }

    #[test]
    fn rows_cover_the_l5_mouse_note_and_both_glyph_legends() {
        let rows = help_rows(&Keymap::default());
        assert!(
            rows.iter().any(|row| row.label.contains("alt+click")),
            "the L5 mouse-capture override must be documented"
        );
        for (glyph, meaning) in SESSION_MAP_GLYPH_LEGEND.iter().chain(TIMELINE_GLYPH_LEGEND) {
            assert!(
                rows.iter()
                    .any(|row| row.label.contains(glyph) && row.label.contains(meaning)),
                "legend entry {glyph} {meaning} must appear in help"
            );
        }
    }

    #[test]
    fn filter_keeps_matching_rows_and_never_leaves_orphan_headings() {
        let rows = help_rows(&Keymap::default());
        let filtered = filter_help_rows(&rows, "split pane");
        assert!(!filtered.is_empty());
        assert!(filtered.len() < rows.len(), "the filter narrows");
        assert!(
            filtered
                .iter()
                .any(|row| row.heading && row.label == "Layout"),
            "the matching section's heading survives"
        );
        assert!(filtered.iter().any(|row| row.label == "Split pane right"));
        // Every surviving heading introduces at least one matched row: no
        // orphan section titles.
        for (index, row) in filtered.iter().enumerate() {
            if row.heading {
                assert!(
                    filtered.get(index + 1).is_some_and(|next| !next.heading),
                    "heading {:?} must be followed by a matched row",
                    row.label
                );
            }
        }
        // A query matching nothing yields an empty list, not stray headings.
        assert!(filter_help_rows(&rows, "zzzzzz").is_empty());
        // Empty query keeps everything.
        assert_eq!(filter_help_rows(&rows, "").len(), rows.len());
    }

    #[test]
    fn welcome_note_is_short_and_generated_from_the_live_keymap() {
        let lines = welcome_lines(&Keymap::default());
        assert!(lines.len() <= 8, "the first-run note stays under 8 lines");
        let all = lines.join("\n");
        assert!(all.contains("ctrl+p"));
        assert!(all.contains("right-click"));
        assert!(all.contains("f1"));
        assert!(all.contains("ctrl+q"));

        // A rebound palette chord is reflected, not contradicted.
        let mut keymap = Keymap::default();
        keymap.toggle_palette = parse_chord("ctrl+k").unwrap();
        let all = welcome_lines(&keymap).join("\n");
        assert!(all.contains("ctrl+k"));
        assert!(!all.contains("ctrl+p"));
    }

    #[test]
    fn quit_label_matches_the_command_table() {
        // The test above names the Quit row by label; keep it honest.
        assert_eq!(
            command_for_id(CommandId::Quit).unwrap().label,
            "Quit Mandatum"
        );
    }
}
