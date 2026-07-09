use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mandatum_commands::{
    CommandId, PaletteContext, PaletteInput, PaletteKey, resolve_palette_key_with_context,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeInput {
    Quit,
    TogglePalette,
    ClosePalette,
    Dispatch(CommandId),
    SendToTerminal(Vec<u8>),
    Noop,
}

pub fn key_to_input(key: KeyEvent, palette_open: bool) -> RuntimeInput {
    key_to_input_with_palette_context(key, palette_open, PaletteContext::default())
}

pub fn key_to_input_with_palette_context(
    key: KeyEvent,
    palette_open: bool,
    palette_context: PaletteContext,
) -> RuntimeInput {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        return RuntimeInput::Quit;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
        return RuntimeInput::TogglePalette;
    }

    if palette_open {
        return key_to_palette_input(key, palette_context);
    }

    key_to_terminal_input(key)
        .map(RuntimeInput::SendToTerminal)
        .unwrap_or(RuntimeInput::Noop)
}

pub fn key_to_terminal_input(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            control_byte(character).map(|byte| vec![byte])
        }
        KeyCode::Char(character) if key.modifiers.contains(KeyModifiers::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend(character.to_string().as_bytes());
            Some(bytes)
        }
        KeyCode::Char(character) => Some(character.to_string().into_bytes()),
        KeyCode::Enter => Some(b"\r".to_vec()),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(b"\t".to_vec()),
        KeyCode::Esc => Some(vec![0x1b]),
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

fn key_to_palette_input(key: KeyEvent, palette_context: PaletteContext) -> RuntimeInput {
    let Some(key) = palette_key_for(key) else {
        return RuntimeInput::Noop;
    };

    match resolve_palette_key_with_context(key, palette_context) {
        PaletteInput::Close => RuntimeInput::ClosePalette,
        PaletteInput::Quit => RuntimeInput::Quit,
        PaletteInput::Dispatch(command_id) => RuntimeInput::Dispatch(command_id),
        PaletteInput::Noop => RuntimeInput::Noop,
    }
}

fn palette_key_for(key: KeyEvent) -> Option<PaletteKey> {
    match key.code {
        KeyCode::Esc => Some(PaletteKey::Escape),
        KeyCode::Tab => Some(PaletteKey::Tab),
        KeyCode::BackTab => Some(PaletteKey::BackTab),
        KeyCode::Char(character) => Some(PaletteKey::Character(character)),
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
