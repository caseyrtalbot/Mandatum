//! The remappable keymap: workspace-control chords plus palette letters.
//!
//! Defaults reproduce the pre-config behavior exactly: Ctrl+Q quits,
//! Ctrl+P toggles the palette, no command has a global chord, and palette
//! letters come from the `palette_key` data column in
//! `mandatum_commands::BUILT_IN_COMMANDS`.
//!
//! Every global chord must carry an explicit command modifier (control, alt
//! or super): a bare character chord would steal typing from the focused
//! child terminal, which L5 forbids outside explicit workspace control.

use std::fmt::Write as _;

use mandatum_commands::{CommandId, PaletteBindings};
use mandatum_scene::input::{Key, KeyCode, Modifiers};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    pub quit: Key,
    pub toggle_palette: Key,
    command_chords: Vec<(Key, CommandId)>,
    pub palette: PaletteBindings,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            quit: Key::ctrl('q'),
            toggle_palette: Key::ctrl('p'),
            command_chords: Vec::new(),
            palette: PaletteBindings::default(),
        }
    }
}

/// What a workspace-control chord resolves to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordAction {
    Quit,
    TogglePalette,
    Dispatch(CommandId),
}

impl Keymap {
    /// The workspace-control action a key press triggers, if any.
    pub fn chord_action(&self, key: Key) -> Option<ChordAction> {
        if chord_matches(self.quit, key) {
            return Some(ChordAction::Quit);
        }
        if chord_matches(self.toggle_palette, key) {
            return Some(ChordAction::TogglePalette);
        }
        self.command_chords
            .iter()
            .find(|(chord, _)| chord_matches(*chord, key))
            .map(|(_, command_id)| ChordAction::Dispatch(*command_id))
    }

    /// The global chord bound to a command, if any.
    pub fn chord_for(&self, command_id: CommandId) -> Option<Key> {
        self.command_chords
            .iter()
            .find(|(_, bound)| *bound == command_id)
            .map(|(chord, _)| *chord)
    }

    /// Bind a global chord to a command. The command's previous chord is
    /// released; if the chord was already taken the later binding wins and
    /// the displaced command is returned so callers can surface a warning.
    pub fn bind_chord(&mut self, command_id: CommandId, chord: Key) -> Option<CommandId> {
        self.command_chords
            .retain(|(_, bound)| *bound != command_id);
        let displaced = self
            .command_chords
            .iter()
            .find(|(bound, _)| chord_matches(*bound, chord))
            .map(|(_, displaced)| *displaced);
        self.command_chords
            .retain(|(bound, _)| !chord_matches(*bound, chord));
        self.command_chords.push((chord, command_id));
        displaced
    }
}

/// Chord equality with character-case normalization: `ctrl+shift+r` matches
/// whether the terminal reports `Char('r')` or `Char('R')` alongside the
/// shift modifier.
fn chord_matches(chord: Key, key: Key) -> bool {
    if chord.mods != key.mods {
        return false;
    }
    match (chord.code, key.code) {
        (KeyCode::Char(a), KeyCode::Char(b)) => a == b || a.eq_ignore_ascii_case(&b),
        (a, b) => a == b,
    }
}

/// Parse a chord like `ctrl+shift+r` or `alt+f5` into a neutral [`Key`].
///
/// Errors name the exact problem so the config boundary can surface it.
pub fn parse_chord(text: &str) -> Result<Key, String> {
    let mut mods = Modifiers::NONE;
    let mut code = None;

    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("chord '{text}' has an empty segment"));
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.control = true,
            "shift" => mods.shift = true,
            "alt" | "option" => mods.alt = true,
            "super" | "cmd" | "meta" | "win" => mods.super_key = true,
            other => {
                if code.is_some() {
                    return Err(format!("chord '{text}' names more than one key"));
                }
                code = Some(parse_key_name(other, text)?);
            }
        }
    }

    let Some(code) = code else {
        return Err(format!("chord '{text}' names no key"));
    };
    let key = Key::new(code, mods);
    if !mods.has_command_modifier() && !matches!(code, KeyCode::Function(_)) {
        return Err(format!(
            "chord '{text}' must include a modifier (ctrl, alt or super); a bare \
             key would steal typing from the focused terminal"
        ));
    }
    Ok(key)
}

fn parse_key_name(name: &str, chord: &str) -> Result<KeyCode, String> {
    let code = match name {
        "enter" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "backtab" => KeyCode::BackTab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "insert" => KeyCode::Insert,
        "delete" => KeyCode::Delete,
        "space" => KeyCode::Char(' '),
        _ => {
            let mut characters = name.chars();
            match (characters.next(), characters.next()) {
                (Some('f'), Some(_)) if name.len() <= 3 => match name[1..].parse::<u8>() {
                    Ok(number @ 1..=24) => KeyCode::Function(number),
                    _ => return Err(format!("chord '{chord}': unknown key '{name}'")),
                },
                (Some(character), None) => KeyCode::Char(character),
                _ => return Err(format!("chord '{chord}': unknown key '{name}'")),
            }
        }
    };
    Ok(code)
}

/// Human-readable chord text, the inverse of [`parse_chord`], for palette
/// entries and warnings.
pub fn format_chord(key: Key) -> String {
    let mut text = String::new();
    if key.mods.control {
        text.push_str("ctrl+");
    }
    if key.mods.alt {
        text.push_str("alt+");
    }
    if key.mods.super_key {
        text.push_str("super+");
    }
    if key.mods.shift {
        text.push_str("shift+");
    }
    match key.code {
        KeyCode::Char(' ') => text.push_str("space"),
        KeyCode::Char(character) => text.push(character),
        KeyCode::Enter => text.push_str("enter"),
        KeyCode::Escape => text.push_str("escape"),
        KeyCode::Backspace => text.push_str("backspace"),
        KeyCode::Tab => text.push_str("tab"),
        KeyCode::BackTab => text.push_str("backtab"),
        KeyCode::Up => text.push_str("up"),
        KeyCode::Down => text.push_str("down"),
        KeyCode::Left => text.push_str("left"),
        KeyCode::Right => text.push_str("right"),
        KeyCode::Home => text.push_str("home"),
        KeyCode::End => text.push_str("end"),
        KeyCode::PageUp => text.push_str("pageup"),
        KeyCode::PageDown => text.push_str("pagedown"),
        KeyCode::Insert => text.push_str("insert"),
        KeyCode::Delete => text.push_str("delete"),
        KeyCode::Function(number) => {
            let _ = write!(text, "f{number}");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keymap_matches_the_pre_config_behavior() {
        let keymap = Keymap::default();
        assert_eq!(keymap.chord_action(Key::ctrl('q')), Some(ChordAction::Quit));
        assert_eq!(
            keymap.chord_action(Key::ctrl('p')),
            Some(ChordAction::TogglePalette)
        );
        assert_eq!(keymap.chord_action(Key::plain(KeyCode::Char('q'))), None);
        assert_eq!(keymap.chord_for(CommandId::SplitRight), None);
        assert_eq!(
            keymap.palette.resolve_char('v'),
            Some(CommandId::SplitRight)
        );
    }

    #[test]
    fn chords_parse_modifiers_keys_and_case_insensitive_matching() {
        let chord = parse_chord("ctrl+shift+r").unwrap();
        assert_eq!(
            chord,
            Key::new(
                KeyCode::Char('r'),
                Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::NONE
                }
            )
        );
        // A terminal reporting the shifted character still matches.
        assert!(chord_matches(
            chord,
            Key::new(
                KeyCode::Char('R'),
                Modifiers {
                    control: true,
                    shift: true,
                    ..Modifiers::NONE
                }
            )
        ));
        assert_eq!(
            parse_chord("alt+f5").unwrap(),
            Key::new(KeyCode::Function(5), Modifiers::ALT)
        );
        assert_eq!(format_chord(chord), "ctrl+shift+r");
    }

    #[test]
    fn bad_chords_are_rejected_with_the_exact_problem() {
        assert!(parse_chord("banana+q").unwrap_err().contains("banana"));
        assert!(parse_chord("ctrl+").unwrap_err().contains("empty segment"));
        assert!(parse_chord("ctrl").unwrap_err().contains("names no key"));
        assert!(
            parse_chord("ctrl+a+b")
                .unwrap_err()
                .contains("more than one key")
        );
        // A bare key (or shift alone) would steal terminal typing (L5).
        assert!(parse_chord("r").unwrap_err().contains("modifier"));
        assert!(parse_chord("shift+r").unwrap_err().contains("modifier"));
        // Function keys are workspace keys already: no modifier needed.
        assert!(parse_chord("f5").is_ok());
    }

    #[test]
    fn later_chord_binding_wins_and_reports_the_displaced_command() {
        let mut keymap = Keymap::default();
        assert_eq!(
            keymap.bind_chord(CommandId::SplitRight, parse_chord("ctrl+r").unwrap()),
            None
        );
        let displaced = keymap.bind_chord(CommandId::SplitDown, parse_chord("ctrl+r").unwrap());
        assert_eq!(displaced, Some(CommandId::SplitRight));
        assert_eq!(
            keymap.chord_action(Key::ctrl('r')),
            Some(ChordAction::Dispatch(CommandId::SplitDown))
        );
        assert_eq!(keymap.chord_for(CommandId::SplitRight), None);
    }
}
