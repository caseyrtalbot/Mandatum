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
    match key.code {
        KeyCode::Char(character) if key.mods.control => {
            control_byte(character).map(|byte| vec![byte])
        }
        KeyCode::Char(character) if key.mods.alt => {
            let mut bytes = vec![0x1b];
            bytes.extend(character.to_string().as_bytes());
            Some(bytes)
        }
        KeyCode::Char(character) => Some(character.to_string().into_bytes()),
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        // xterm-256color's conventional BackTab sequence. Crossterm emits
        // Shift+Tab as `BackTab` + SHIFT, while another frontend may emit
        // `Tab` + SHIFT; both neutral forms must reach the child (L5).
        KeyCode::BackTab => Some(b"\x1b[Z".to_vec()),
        KeyCode::Tab if key.mods.shift => Some(b"\x1b[Z".to_vec()),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Escape => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}

fn control_byte(character: char) -> Option<u8> {
    let lower = character.to_ascii_lowercase();
    if lower.is_ascii_lowercase() {
        Some((lower as u8) - b'a' + 1)
    } else if character == '[' {
        Some(0x1b)
    } else {
        None
    }
}
