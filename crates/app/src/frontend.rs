//! Crossterm-to-neutral input translation for the terminal frontend.
//!
//! [L1-GATE] This module and `app_shell` are the only product modules that
//! may name crossterm types (enforced by the module scan in
//! `ci/conformance.sh`). Everything past this boundary — `app_state`, input
//! routing, command dispatch — consumes `mandatum_scene::input` values only,
//! so a native or GPU frontend plugs in by writing its own translation.

use crossterm::event::{
    Event, KeyCode as CtKeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use mandatum_scene::SceneSize;
use mandatum_scene::input::{
    InputEvent, Key, KeyCode, Modifiers, PointerButton, PointerEvent, PointerKind,
};

/// Translate one crossterm event into a neutral input event. `None` means
/// the event carries nothing the app consumes (key releases, unmapped keys).
pub(crate) fn translate_event(event: Event) -> Option<InputEvent> {
    match event {
        Event::Key(key) => translate_key(key).map(InputEvent::Key),
        Event::Mouse(mouse) => translate_mouse(mouse).map(InputEvent::Pointer),
        Event::Paste(text) => Some(InputEvent::Paste(text)),
        Event::Resize(columns, rows) => Some(InputEvent::Resize(SceneSize::new(columns, rows))),
        Event::FocusGained => Some(InputEvent::FocusGained),
        Event::FocusLost => Some(InputEvent::FocusLost),
    }
}

fn translate_key(key: KeyEvent) -> Option<Key> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let code = match key.code {
        CtKeyCode::Char(character) => KeyCode::Char(character),
        CtKeyCode::Enter => KeyCode::Enter,
        CtKeyCode::Esc => KeyCode::Escape,
        CtKeyCode::Backspace => KeyCode::Backspace,
        CtKeyCode::Tab => KeyCode::Tab,
        CtKeyCode::BackTab => KeyCode::BackTab,
        CtKeyCode::Up => KeyCode::Up,
        CtKeyCode::Down => KeyCode::Down,
        CtKeyCode::Left => KeyCode::Left,
        CtKeyCode::Right => KeyCode::Right,
        CtKeyCode::Home => KeyCode::Home,
        CtKeyCode::End => KeyCode::End,
        CtKeyCode::PageUp => KeyCode::PageUp,
        CtKeyCode::PageDown => KeyCode::PageDown,
        CtKeyCode::Insert => KeyCode::Insert,
        CtKeyCode::Delete => KeyCode::Delete,
        CtKeyCode::F(number) => KeyCode::Function(number),
        _ => return None,
    };
    Some(Key::new(code, translate_modifiers(key.modifiers)))
}

fn translate_modifiers(modifiers: KeyModifiers) -> Modifiers {
    Modifiers {
        shift: modifiers.contains(KeyModifiers::SHIFT),
        control: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT),
        super_key: modifiers.contains(KeyModifiers::SUPER)
            || modifiers.contains(KeyModifiers::META),
    }
}

fn translate_mouse(mouse: MouseEvent) -> Option<PointerEvent> {
    let (kind, button) = match mouse.kind {
        MouseEventKind::Down(button) => (PointerKind::Down, Some(translate_button(button))),
        MouseEventKind::Up(button) => (PointerKind::Up, Some(translate_button(button))),
        MouseEventKind::Drag(button) => (PointerKind::Drag, Some(translate_button(button))),
        MouseEventKind::Moved => (PointerKind::Move, None),
        MouseEventKind::ScrollDown => (PointerKind::Wheel { dx: 0, dy: 1 }, None),
        MouseEventKind::ScrollUp => (PointerKind::Wheel { dx: 0, dy: -1 }, None),
        MouseEventKind::ScrollLeft => (PointerKind::Wheel { dx: -1, dy: 0 }, None),
        MouseEventKind::ScrollRight => (PointerKind::Wheel { dx: 1, dy: 0 }, None),
    };
    Some(PointerEvent {
        kind,
        button,
        column: mouse.column,
        row: mouse.row,
        mods: translate_modifiers(mouse.modifiers),
    })
}

fn translate_button(button: MouseButton) -> PointerButton {
    match button {
        MouseButton::Left => PointerButton::Left,
        MouseButton::Right => PointerButton::Right,
        MouseButton::Middle => PointerButton::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_presses_translate_and_releases_are_dropped() {
        let pressed = KeyEvent::new(CtKeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            translate_event(Event::Key(pressed)),
            Some(InputEvent::Key(Key::ctrl('q')))
        );

        let mut released = KeyEvent::new(CtKeyCode::Char('q'), KeyModifiers::CONTROL);
        released.kind = KeyEventKind::Release;
        assert_eq!(translate_event(Event::Key(released)), None);

        assert_eq!(
            translate_event(Event::Key(KeyEvent::new(
                CtKeyCode::Esc,
                KeyModifiers::NONE
            ))),
            Some(InputEvent::Key(Key::plain(KeyCode::Escape)))
        );
    }

    #[test]
    fn backtab_keeps_the_shift_modifier_at_the_neutral_boundary() {
        assert_eq!(
            translate_event(Event::Key(KeyEvent::new(
                CtKeyCode::BackTab,
                KeyModifiers::SHIFT
            ))),
            Some(InputEvent::Key(Key::new(
                KeyCode::BackTab,
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                }
            )))
        );
    }

    #[test]
    fn paste_resize_and_mouse_translate_to_neutral_events() {
        assert_eq!(
            translate_event(Event::Paste("hi".to_owned())),
            Some(InputEvent::Paste("hi".to_owned()))
        );
        assert_eq!(
            translate_event(Event::Resize(100, 35)),
            Some(InputEvent::Resize(SceneSize::new(100, 35)))
        );
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 7,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            translate_event(Event::Mouse(click)),
            Some(InputEvent::Pointer(PointerEvent {
                kind: PointerKind::Down,
                button: Some(PointerButton::Left),
                column: 4,
                row: 7,
                mods: Modifiers::NONE,
            }))
        );
    }
}
