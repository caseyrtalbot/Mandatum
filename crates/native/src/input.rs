//! Pure winit-to-scene input and geometry translation.

use mandatum_scene::{
    BackingScale, LogicalSize, PhysicalSize, ViewportMetrics, WorkspaceScene,
    input::{
        CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, PointerButton, TextRange,
    },
};
use winit::{
    event::{Ime, MouseButton},
    keyboard::{Key, ModifiersState, NamedKey},
};

pub(super) enum PlatformAction {
    Input(InputEvent),
    PasteShortcut(InputKey),
    CopyShortcut(InputKey),
    Ignore,
}

pub(super) fn neutral_modifiers(modifiers: ModifiersState) -> Modifiers {
    Modifiers {
        control: modifiers.control_key(),
        alt: modifiers.alt_key(),
        shift: modifiers.shift_key(),
        super_key: modifiers.super_key(),
    }
}

pub(super) fn neutral_button(button: MouseButton) -> Option<PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        MouseButton::Back | MouseButton::Forward | MouseButton::Other(_) => None,
    }
}

#[cfg(test)]
pub(super) fn scene_size_from_metrics(
    pixel_width: u32,
    pixel_height: u32,
    cell_width: f32,
    cell_height: f32,
) -> Option<mandatum_scene::SceneSize> {
    viewport_metrics_from_renderer(pixel_width, pixel_height, 1.0, cell_width, cell_height)
        .map(ViewportMetrics::scene_size)
}

/// Freeze one coherent physical/logical/cell snapshot at the native boundary.
///
/// Renderer cell metrics are physical pixels because font metrics already
/// include backing scale. Scene presentation retains their logical-pixel
/// values so layout identity stays stable across 1x/2x materialization.
pub(super) fn viewport_metrics_from_renderer(
    pixel_width: u32,
    pixel_height: u32,
    scale: f32,
    physical_cell_width: f32,
    physical_cell_height: f32,
) -> Option<ViewportMetrics> {
    if pixel_width == 0
        || pixel_height == 0
        || !scale.is_finite()
        || scale <= 0.0
        || !physical_cell_width.is_finite()
        || !physical_cell_height.is_finite()
        || physical_cell_width <= 0.0
        || physical_cell_height <= 0.0
    {
        return None;
    }
    let scale = f64::from(scale);
    let viewport = ViewportMetrics::new(
        LogicalSize::from_pixels(
            f64::from(pixel_width) / scale,
            f64::from(pixel_height) / scale,
        )
        .ok()?,
        PhysicalSize::new(pixel_width, pixel_height),
        BackingScale::new(scale).ok()?,
        LogicalSize::from_pixels(
            f64::from(physical_cell_width) / scale,
            f64::from(physical_cell_height) / scale,
        )
        .ok()?,
    )
    .ok()?;
    let size = viewport.scene_size();
    // One pane needs a 3x3 bordered interior between the one-row header and
    // status strips. Suspend scene production while a minimized/tiny window
    // cannot satisfy that structural contract.
    (size.width >= 3 && size.height >= 5).then_some(viewport)
}

pub(super) fn translate_key(key: &Key, modifiers: ModifiersState) -> PlatformAction {
    let mods = neutral_modifiers(modifiers);
    let exact_platform_shortcut = mods.super_key && !mods.shift && !mods.control && !mods.alt;
    if exact_platform_shortcut
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("v"))
    {
        return PlatformAction::PasteShortcut(InputKey::new(KeyCode::Char('v'), mods));
    }
    if exact_platform_shortcut
        && matches!(key, Key::Character(value) if value.eq_ignore_ascii_case("c"))
    {
        return PlatformAction::CopyShortcut(InputKey::new(KeyCode::Char('c'), mods));
    }
    if let Key::Character(value) = key
        && value.chars().nth(1).is_some()
    {
        return if !mods.control && !mods.alt && !mods.super_key {
            PlatformAction::Input(InputEvent::Composition(CompositionEvent::Commit(
                value.to_string(),
            )))
        } else {
            PlatformAction::Ignore
        };
    }

    let code = match key {
        Key::Named(named) => named_key_code(*named, mods.shift),
        Key::Character(value) => value.chars().next().map(|character| {
            let character = if mods.shift && character.is_ascii_lowercase() {
                character.to_ascii_uppercase()
            } else {
                character
            };
            KeyCode::Char(character)
        }),
        _ => None,
    };
    code.map_or(PlatformAction::Ignore, |code| {
        PlatformAction::Input(InputEvent::Key(InputKey::new(code, mods)))
    })
}

pub(super) fn translate_ime(ime: Ime) -> Option<CompositionEvent> {
    match ime {
        Ime::Enabled => None,
        Ime::Disabled => Some(CompositionEvent::Cancel),
        Ime::Commit(text) => Some(CompositionEvent::Commit(text)),
        Ime::Preedit(text, cursor) => {
            let cursor = match cursor {
                Some((start, end)) => match TextRange::new(&text, start, end) {
                    Some(range) => Some(range),
                    None => return Some(CompositionEvent::Cancel),
                },
                None => None,
            };
            Some(CompositionEvent::Preedit { text, cursor })
        }
    }
}

pub(super) fn ime_event_is_accepted(window_focused: bool, ime_allowed: bool) -> bool {
    window_focused && ime_allowed
}

fn named_key_code(key: NamedKey, shift: bool) -> Option<KeyCode> {
    Some(match key {
        NamedKey::Enter => KeyCode::Enter,
        NamedKey::Escape => KeyCode::Escape,
        NamedKey::Backspace => KeyCode::Backspace,
        NamedKey::Space => KeyCode::Char(' '),
        NamedKey::Tab if shift => KeyCode::BackTab,
        NamedKey::Tab => KeyCode::Tab,
        NamedKey::ArrowUp => KeyCode::Up,
        NamedKey::ArrowDown => KeyCode::Down,
        NamedKey::ArrowLeft => KeyCode::Left,
        NamedKey::ArrowRight => KeyCode::Right,
        NamedKey::Home => KeyCode::Home,
        NamedKey::End => KeyCode::End,
        NamedKey::PageUp => KeyCode::PageUp,
        NamedKey::PageDown => KeyCode::PageDown,
        NamedKey::Insert => KeyCode::Insert,
        NamedKey::Delete => KeyCode::Delete,
        NamedKey::F1 => KeyCode::Function(1),
        NamedKey::F2 => KeyCode::Function(2),
        NamedKey::F3 => KeyCode::Function(3),
        NamedKey::F4 => KeyCode::Function(4),
        NamedKey::F5 => KeyCode::Function(5),
        NamedKey::F6 => KeyCode::Function(6),
        NamedKey::F7 => KeyCode::Function(7),
        NamedKey::F8 => KeyCode::Function(8),
        NamedKey::F9 => KeyCode::Function(9),
        NamedKey::F10 => KeyCode::Function(10),
        NamedKey::F11 => KeyCode::Function(11),
        NamedKey::F12 => KeyCode::Function(12),
        NamedKey::F13 => KeyCode::Function(13),
        NamedKey::F14 => KeyCode::Function(14),
        NamedKey::F15 => KeyCode::Function(15),
        NamedKey::F16 => KeyCode::Function(16),
        NamedKey::F17 => KeyCode::Function(17),
        NamedKey::F18 => KeyCode::Function(18),
        NamedKey::F19 => KeyCode::Function(19),
        NamedKey::F20 => KeyCode::Function(20),
        NamedKey::F21 => KeyCode::Function(21),
        NamedKey::F22 => KeyCode::Function(22),
        NamedKey::F23 => KeyCode::Function(23),
        NamedKey::F24 => KeyCode::Function(24),
        _ => return None,
    })
}

pub(super) fn scene_is_suspended_by_tiled_minimum(scene: &WorkspaceScene) -> bool {
    scene.panes.iter().any(|pane| {
        pane_geometry_is_suspended(
            pane.floating,
            pane.area.width,
            pane.area.height,
            scene.size.width,
            scene.size.height,
        )
    })
}

fn pane_geometry_is_suspended(
    floating: bool,
    pane_width: u16,
    pane_height: u16,
    frame_width: u16,
    frame_height: u16,
) -> bool {
    let unusable = pane_width < 3 || pane_height < 3;
    unusable && (!floating || frame_width < 11 || frame_height < 9)
}

pub(super) fn key_for_platform_translation(
    logical: &Key,
    _without_modifiers: &Key,
    modifiers: ModifiersState,
) -> Key {
    if !modifiers.shift_key() || !(modifiers.alt_key() || modifiers.super_key()) {
        return logical.clone();
    }
    // winit has already applied `OptionAsAlt::OnlyRight` on macOS: Right
    // Option arrives as the base logical key while Left Option retains native
    // dead-key/composed characters. Rebuild only the ASCII Shift layer and
    // otherwise trust the platform logical key.
    match logical {
        Key::Character(value) => {
            let shifted: String = value.chars().map(shift_meta_character).collect();
            Key::Character(shifted.into())
        }
        _ => logical.clone(),
    }
}

fn shift_meta_character(character: char) -> char {
    match character {
        '`' => '~',
        '1' => '!',
        '2' => '@',
        '3' => '#',
        '4' => '$',
        '5' => '%',
        '6' => '^',
        '7' => '&',
        '8' => '*',
        '9' => '(',
        '0' => ')',
        '-' => '_',
        '=' => '+',
        '[' => '{',
        ']' => '}',
        '\\' => '|',
        ';' => ':',
        '\'' => '"',
        ',' => '<',
        '.' => '>',
        '/' => '?',
        ascii if ascii.is_ascii_lowercase() => ascii.to_ascii_uppercase(),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PlatformAction, ime_event_is_accepted, key_for_platform_translation,
        pane_geometry_is_suspended, scene_size_from_metrics, shift_meta_character, translate_ime,
        translate_key, viewport_metrics_from_renderer,
    };
    use mandatum_scene::{
        PhysicalSize, SceneSize,
        input::{CompositionEvent, InputEvent, Key as InputKey, KeyCode, Modifiers, TextRange},
    };
    use winit::{
        event::Ime,
        keyboard::{Key, ModifiersState, NamedKey},
    };

    #[test]
    fn scene_size_rejects_suspended_or_invalid_metrics() {
        assert_eq!(scene_size_from_metrics(0, 720, 8.0, 16.0), None);
        assert_eq!(scene_size_from_metrics(1280, 720, 0.0, 16.0), None);
        assert_eq!(scene_size_from_metrics(16, 64, 8.0, 16.0), None);
        assert_eq!(
            scene_size_from_metrics(1280, 720, 8.0, 16.0),
            Some(SceneSize::new(160, 45))
        );
    }

    #[test]
    fn viewport_snapshot_preserves_logical_geometry_at_one_and_two_x() {
        let one_x =
            viewport_metrics_from_renderer(800, 480, 1.0, 8.0, 16.0).expect("valid 1x metrics");
        let two_x =
            viewport_metrics_from_renderer(1600, 960, 2.0, 16.0, 32.0).expect("valid 2x metrics");

        assert_eq!(one_x.logical_size, two_x.logical_size);
        assert_eq!(one_x.measured_cell_metrics, two_x.measured_cell_metrics);
        assert_eq!(one_x.scene_size(), SceneSize::new(100, 30));
        assert_eq!(two_x.scene_size(), SceneSize::new(100, 30));
        assert_eq!(two_x.physical_size, PhysicalSize::new(1600, 960));
    }

    #[test]
    fn tiled_minimum_suspends_but_usable_floating_pane_does_not() {
        assert!(pane_geometry_is_suspended(false, 2, 2, 80, 24));
        assert!(!pane_geometry_is_suspended(true, 2, 2, 80, 24));
        assert!(pane_geometry_is_suspended(true, 2, 2, 10, 8));
    }

    #[test]
    fn ime_events_require_focus_and_scene_permission() {
        assert!(ime_event_is_accepted(true, true));
        assert!(!ime_event_is_accepted(false, true));
        assert!(!ime_event_is_accepted(true, false));
    }

    #[test]
    fn option_shift_translation_preserves_ascii_shift_layer() {
        assert_eq!(shift_meta_character('a'), 'A');
        assert_eq!(shift_meta_character('1'), '!');
        assert_eq!(shift_meta_character('/'), '?');
        assert_eq!(shift_meta_character('界'), '界');
    }

    #[test]
    fn native_key_translation_preserves_backtab_shortcuts_and_composition() {
        let cases = [
            (
                Key::Named(NamedKey::Tab),
                ModifiersState::SHIFT,
                InputKey::new(
                    KeyCode::BackTab,
                    Modifiers {
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
            ),
            (
                Key::Character("x".into()),
                ModifiersState::ALT | ModifiersState::SHIFT,
                InputKey::new(
                    KeyCode::Char('X'),
                    Modifiers {
                        alt: true,
                        shift: true,
                        ..Modifiers::NONE
                    },
                ),
            ),
            (
                Key::Named(NamedKey::F24),
                ModifiersState::empty(),
                InputKey::plain(KeyCode::Function(24)),
            ),
            (
                Key::Named(NamedKey::Insert),
                ModifiersState::empty(),
                InputKey::plain(KeyCode::Insert),
            ),
        ];
        for (platform, modifiers, expected) in cases {
            let PlatformAction::Input(InputEvent::Key(actual)) =
                translate_key(&platform, modifiers)
            else {
                panic!("native key did not become neutral input");
            };
            assert_eq!(actual, expected);
        }

        let PlatformAction::PasteShortcut(shortcut) =
            translate_key(&Key::Character("v".into()), ModifiersState::SUPER)
        else {
            panic!("Command+V did not retain its neutral key for chord preflight");
        };
        assert_eq!(
            shortcut,
            InputKey::new(
                KeyCode::Char('v'),
                Modifiers {
                    super_key: true,
                    ..Modifiers::NONE
                }
            )
        );

        let PlatformAction::Input(InputEvent::Key(modified_super)) = translate_key(
            &Key::Character("C".into()),
            ModifiersState::SUPER | ModifiersState::SHIFT,
        ) else {
            panic!("modified Command+C incorrectly used the native copy fallback");
        };
        assert_eq!(
            modified_super,
            InputKey::new(
                KeyCode::Char('C'),
                Modifiers {
                    shift: true,
                    super_key: true,
                    ..Modifiers::NONE
                }
            )
        );

        assert!(matches!(
            translate_key(&Key::Character("👩‍💻".into()), ModifiersState::empty()),
            PlatformAction::Input(InputEvent::Composition(CompositionEvent::Commit(text)))
                if text == "👩‍💻"
        ));
    }

    #[test]
    fn ime_translation_validates_preedit_ranges() {
        assert_eq!(
            translate_ime(Ime::Preedit("界".to_owned(), Some((0, 3)))),
            Some(CompositionEvent::Preedit {
                text: "界".to_owned(),
                cursor: Some(TextRange { start: 0, end: 3 }),
            })
        );
        assert_eq!(
            translate_ime(Ime::Preedit("界".to_owned(), Some((1, 2)))),
            Some(CompositionEvent::Cancel)
        );
        assert_eq!(
            translate_ime(Ime::Commit("👩‍💻".to_owned())),
            Some(CompositionEvent::Commit("👩‍💻".to_owned()))
        );
    }

    #[test]
    fn option_translation_preserves_native_left_and_meta_right_semantics() {
        assert_eq!(
            key_for_platform_translation(
                &Key::Character("1".into()),
                &Key::Character("1".into()),
                ModifiersState::ALT | ModifiersState::SHIFT,
            ),
            Key::Character("!".into())
        );
        assert_eq!(
            key_for_platform_translation(
                &Key::Character("å".into()),
                &Key::Character("a".into()),
                ModifiersState::ALT,
            ),
            Key::Character("å".into())
        );
        assert_eq!(
            key_for_platform_translation(
                &Key::Dead(Some('´')),
                &Key::Character("e".into()),
                ModifiersState::ALT,
            ),
            Key::Dead(Some('´'))
        );
    }
}
