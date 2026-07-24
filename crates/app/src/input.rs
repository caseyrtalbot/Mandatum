//! Neutral input routing: `mandatum_scene::input` values to runtime intents.
//!
//! No platform event type appears here; the terminal frontend translates
//! crossterm events into neutral values in `crate::frontend` before they
//! reach this module. Workspace-control chords come from the remappable
//! [`Keymap`]; everything unbound flows to the focused child terminal (L5).
//! Palette-mode keys are routed by `app_state` against the live palette
//! model (see `crate::palette` for the interaction contract).

use mandatum_commands::CommandId;
use mandatum_scene::input::{Key, KeyCode};

use crate::keymap::{ChordAction, Keymap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeInput {
    Quit,
    TogglePalette,
    Dispatch(CommandId),
    SendToTerminal(Vec<u8>),
    Noop,
}

pub fn key_to_input(key: Key) -> RuntimeInput {
    key_to_input_with_keymap(key, &Keymap::default())
}

pub fn key_to_input_with_keymap(key: Key, keymap: &Keymap) -> RuntimeInput {
    // Chords are explicit workspace control: they are the only keys the
    // workspace intercepts ahead of the focused child terminal (L5), and the
    // config boundary rejects chords without a command modifier.
    match keymap.chord_action(key) {
        Some(ChordAction::Quit) => return RuntimeInput::Quit,
        Some(ChordAction::TogglePalette) => return RuntimeInput::TogglePalette,
        Some(ChordAction::Dispatch(command_id)) => return RuntimeInput::Dispatch(command_id),
        None => {}
    }

    key_to_terminal_input(key)
        .map(RuntimeInput::SendToTerminal)
        .unwrap_or(RuntimeInput::Noop)
}

pub fn key_to_terminal_input(key: Key) -> Option<Vec<u8>> {
    // The platform command modifier is workspace-only. A configured workspace
    // chord already had first refusal above; an unbound Super/Windows-key chord
    // must not leak its character into the child terminal.
    if key.mods.super_key {
        return None;
    }

    let mut bytes = match key.code {
        KeyCode::Char(character) if key.mods.control => vec![control_byte(character)?],
        KeyCode::Char(character) => character.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        // xterm-256color's conventional BackTab sequence. Crossterm emits
        // Shift+Tab as `BackTab` + SHIFT, while another frontend may emit
        // `Tab` + SHIFT; both neutral forms must reach the child (L5).
        KeyCode::BackTab => modified_backtab(key.mods),
        KeyCode::Tab if key.mods.shift => modified_backtab(key.mods),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Escape => vec![0x1b],
        KeyCode::Up => modified_csi_letter('A', key.mods),
        KeyCode::Down => modified_csi_letter('B', key.mods),
        KeyCode::Right => modified_csi_letter('C', key.mods),
        KeyCode::Left => modified_csi_letter('D', key.mods),
        KeyCode::Home => modified_csi_letter('H', key.mods),
        KeyCode::End => modified_csi_letter('F', key.mods),
        KeyCode::PageUp => modified_csi_tilde(5, key.mods),
        KeyCode::PageDown => modified_csi_tilde(6, key.mods),
        KeyCode::Insert => modified_csi_tilde(2, key.mods),
        KeyCode::Delete => modified_csi_tilde(3, key.mods),
        KeyCode::Function(number) => function_key_bytes(number, key.mods)?,
    };
    // Character/control and one-byte key families use the traditional Meta
    // prefix. CSI/SS3 families encode Alt in their xterm modifier parameter.
    if key.mods.alt
        && !(key.code == KeyCode::Tab && key.mods.shift)
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Tab | KeyCode::Escape
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

fn modifier_parameter(mods: mandatum_scene::input::Modifiers) -> Option<u8> {
    let parameter = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.control);
    (parameter > 1).then_some(parameter)
}

fn modified_csi_letter(letter: char, mods: mandatum_scene::input::Modifiers) -> Vec<u8> {
    modifier_parameter(mods).map_or_else(
        || format!("\x1b[{letter}").into_bytes(),
        |parameter| format!("\x1b[1;{parameter}{letter}").into_bytes(),
    )
}

fn modified_csi_tilde(code: u8, mods: mandatum_scene::input::Modifiers) -> Vec<u8> {
    modifier_parameter(mods).map_or_else(
        || format!("\x1b[{code}~").into_bytes(),
        |parameter| format!("\x1b[{code};{parameter}~").into_bytes(),
    )
}

fn modified_backtab(mut mods: mandatum_scene::input::Modifiers) -> Vec<u8> {
    // BackTab already carries Shift in its key identity; only additional
    // modifiers belong in xterm's parameter.
    mods.shift = false;
    modifier_parameter(mods).map_or_else(
        || b"\x1b[Z".to_vec(),
        |parameter| format!("\x1b[1;{parameter}Z").into_bytes(),
    )
}

fn function_key_bytes(number: u8, mods: mandatum_scene::input::Modifiers) -> Option<Vec<u8>> {
    if (1..=4).contains(&number) {
        let letter = char::from(b'P' + number - 1);
        return Some(modifier_parameter(mods).map_or_else(
            || format!("\x1bO{letter}").into_bytes(),
            |parameter| format!("\x1b[1;{parameter}{letter}").into_bytes(),
        ));
    }
    if (5..=12).contains(&number) {
        let code = [15, 17, 18, 19, 20, 21, 23, 24][usize::from(number - 5)];
        return Some(modified_csi_tilde(code, mods));
    }

    // The child advertises xterm-256color. Its F13-F24 capabilities are
    // Shift+F1 through Shift+F12. Winit may report either semantic F13-F24 or
    // F1-F12 plus Shift, so accept both representations.
    if (13..=24).contains(&number) {
        let base = number - 12;
        let shifted = mandatum_scene::input::Modifiers {
            shift: true,
            ..mods
        };
        return function_key_bytes(base, shifted);
    }
    None
}

fn control_byte(character: char) -> Option<u8> {
    let lower = character.to_ascii_lowercase();
    Some(match lower {
        '@' | ' ' | '2' | '`' => 0x00,
        'a'..='z' => (lower as u8) - b'a' + 1,
        '[' | '{' | '3' => 0x1b,
        '\\' | '|' | '4' => 0x1c,
        ']' | '}' | '5' => 0x1d,
        '^' | '~' | '6' => 0x1e,
        '_' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use mandatum_scene::input::{Key, KeyCode, Modifiers};

    use super::{RuntimeInput, key_to_input_with_keymap, key_to_terminal_input};
    use crate::Keymap;

    #[test]
    fn workspace_chord_precedes_terminal_fallback_and_unbound_super_does_not_leak() {
        let mut keymap = Keymap::default();
        let super_c = Key::new(
            KeyCode::Char('c'),
            Modifiers {
                super_key: true,
                ..Modifiers::NONE
            },
        );
        keymap.bind_chord(mandatum_commands::CommandId::CopySelection, super_c);
        assert_eq!(
            key_to_input_with_keymap(super_c, &keymap),
            RuntimeInput::Dispatch(mandatum_commands::CommandId::CopySelection)
        );

        let unbound = Key::new(
            KeyCode::Char('x'),
            Modifiers {
                super_key: true,
                ..Modifiers::NONE
            },
        );
        assert_eq!(
            key_to_input_with_keymap(unbound, &keymap),
            RuntimeInput::Noop
        );
    }

    #[test]
    fn baseline_terminal_keyboard_sequences_are_complete() {
        let cases = [
            (KeyCode::BackTab, b"\x1b[Z".as_slice()),
            (KeyCode::PageUp, b"\x1b[5~".as_slice()),
            (KeyCode::PageDown, b"\x1b[6~".as_slice()),
            (KeyCode::Insert, b"\x1b[2~".as_slice()),
            (KeyCode::Delete, b"\x1b[3~".as_slice()),
            (KeyCode::Function(1), b"\x1bOP".as_slice()),
            (KeyCode::Function(12), b"\x1b[24~".as_slice()),
            (KeyCode::Function(24), b"\x1b[24;2~".as_slice()),
        ];
        for (code, expected) in cases {
            assert_eq!(
                key_to_terminal_input(Key::plain(code)),
                Some(expected.to_vec()),
                "missing baseline sequence for {code:?}"
            );
        }
        assert_eq!(
            key_to_terminal_input(Key::plain(KeyCode::Function(0))),
            None
        );
        assert_eq!(
            key_to_terminal_input(Key::plain(KeyCode::Function(25))),
            None
        );
        assert_eq!(
            key_to_terminal_input(Key::new(
                KeyCode::Function(1),
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                }
            )),
            Some(b"\x1b[1;2P".to_vec())
        );
    }

    #[test]
    fn modifier_sequences_preserve_meta_control_and_named_key_identity() {
        let ctrl_alt = Modifiers {
            control: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            key_to_terminal_input(Key::new(KeyCode::Char('a'), ctrl_alt)),
            Some(vec![0x1b, 0x01])
        );
        assert_eq!(
            key_to_terminal_input(Key::new(KeyCode::Right, Modifiers::CTRL)),
            Some(b"\x1b[1;5C".to_vec())
        );
        assert_eq!(
            key_to_terminal_input(Key::new(KeyCode::Left, Modifiers::ALT)),
            Some(b"\x1b[1;3D".to_vec())
        );
        assert_eq!(
            key_to_terminal_input(Key::new(
                KeyCode::BackTab,
                Modifiers {
                    shift: true,
                    control: true,
                    ..Modifiers::NONE
                }
            )),
            Some(b"\x1b[1;5Z".to_vec())
        );
        assert_eq!(
            key_to_terminal_input(Key::new(KeyCode::Function(13), ctrl_alt)),
            Some(b"\x1b[1;8P".to_vec())
        );
    }

    #[test]
    fn complete_ascii_control_family_is_encoded() {
        let cases = [
            ('@', 0x00),
            (' ', 0x00),
            ('2', 0x00),
            ('`', 0x00),
            ('a', 0x01),
            ('z', 0x1a),
            ('[', 0x1b),
            ('{', 0x1b),
            ('3', 0x1b),
            ('\\', 0x1c),
            ('|', 0x1c),
            ('4', 0x1c),
            (']', 0x1d),
            ('}', 0x1d),
            ('5', 0x1d),
            ('^', 0x1e),
            ('~', 0x1e),
            ('6', 0x1e),
            ('_', 0x1f),
            ('/', 0x1f),
            ('7', 0x1f),
            ('?', 0x7f),
            ('8', 0x7f),
        ];
        for (character, expected) in cases {
            assert_eq!(
                key_to_terminal_input(Key::new(KeyCode::Char(character), Modifiers::CTRL)),
                Some(vec![expected]),
                "missing control mapping for {character:?}"
            );
        }
    }

    #[test]
    fn alt_character_is_meta_prefixed() {
        assert_eq!(
            key_to_terminal_input(Key::new(KeyCode::Char('x'), Modifiers::ALT)),
            Some(b"\x1bx".to_vec())
        );
        assert_eq!(
            key_to_terminal_input(Key::new(
                KeyCode::Tab,
                Modifiers {
                    shift: true,
                    alt: true,
                    ..Modifiers::NONE
                }
            )),
            Some(b"\x1b[1;3Z".to_vec())
        );
    }
}
