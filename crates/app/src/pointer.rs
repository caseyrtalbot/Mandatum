//! Pointer routing helpers: encoding pointer events for mouse-capturing
//! children (L5 forwarding), and split-ratio math for drag-resize.
//!
//! Everything here is pure; `app_state` owns the routing decisions.

use mandatum_core::SplitAxis;
use mandatum_scene::SceneRect;
use mandatum_scene::input::{Modifiers, PointerButton, PointerEvent, PointerKind};
use mandatum_terminal_vt::{MouseMode, MouseTracking};

/// Encode one pointer event as the byte sequence a mouse-tracking child
/// expects on its PTY. `column`/`row` are 0-based cells inside the child's
/// grid. `None` means the child's tracking granularity does not report this
/// event (the workspace still must not act on it — the child owns the
/// pointer while tracking is on).
pub(crate) fn encode_mouse_event(
    mode: MouseMode,
    event: &PointerEvent,
    column: u16,
    row: u16,
) -> Option<Vec<u8>> {
    if !mode.wants_mouse() {
        return None;
    }

    let (code, release) = match event.kind {
        PointerKind::Down => (button_code(event.button?), false),
        PointerKind::Up => (button_code(event.button?), true),
        PointerKind::Drag => {
            if mode.tracking < MouseTracking::ButtonEvent {
                return None;
            }
            (button_code(event.button?) + 32, false)
        }
        PointerKind::Move => {
            if mode.tracking < MouseTracking::AnyEvent {
                return None;
            }
            // Motion with no button: code 3 plus the motion flag.
            (3 + 32, false)
        }
        PointerKind::Wheel { dx, dy } => {
            let code = if dy < 0 {
                64
            } else if dy > 0 {
                65
            } else if dx < 0 {
                66
            } else if dx > 0 {
                67
            } else {
                return None;
            };
            let ticks = usize::from(dy.unsigned_abs().max(dx.unsigned_abs()).max(1));
            let one = encode_single(mode, code + modifier_bits(event.mods), false, column, row);
            return Some(one.repeat(ticks));
        }
    };

    Some(encode_single(
        mode,
        code + modifier_bits(event.mods),
        release,
        column,
        row,
    ))
}

fn button_code(button: PointerButton) -> u8 {
    match button {
        PointerButton::Left => 0,
        PointerButton::Middle => 1,
        PointerButton::Right => 2,
    }
}

fn modifier_bits(mods: Modifiers) -> u8 {
    let mut bits = 0;
    if mods.shift {
        bits += 4;
    }
    if mods.alt {
        bits += 8;
    }
    if mods.control {
        bits += 16;
    }
    bits
}

fn encode_single(mode: MouseMode, code: u8, release: bool, column: u16, row: u16) -> Vec<u8> {
    // 1-based coordinates on the wire in both encodings.
    let x = u32::from(column) + 1;
    let y = u32::from(row) + 1;
    if mode.sgr {
        let suffix = if release { 'm' } else { 'M' };
        format!("\x1b[<{code};{x};{y}{suffix}").into_bytes()
    } else {
        // Legacy X10 bytes: releases collapse to button code 3, and
        // coordinates saturate at the encoding's ceiling (223).
        let code = if release { (code & !0b11) | 3 } else { code };
        let clamp = |value: u32| -> u8 { (value.min(223) + 32) as u8 };
        vec![0x1b, b'[', b'M', 32 + code, clamp(x), clamp(y)]
    }
}

/// The first-side percentage a drag to (`column`, `row`) asks of a split
/// whose node covers `split_area`. Clamped to 5..=95 so a drag can never
/// crush a pane to nothing.
pub(crate) fn split_percent_for_pointer(
    axis: SplitAxis,
    split_area: SceneRect,
    column: u16,
    row: u16,
) -> Option<u8> {
    let (offset, length) = match axis {
        SplitAxis::Horizontal => (column.saturating_sub(split_area.x), split_area.width),
        SplitAxis::Vertical => (row.saturating_sub(split_area.y), split_area.height),
    };
    if length < 2 {
        return None;
    }
    let percent = ((u32::from(offset.min(length)) * 100) / u32::from(length)) as u8;
    Some(percent.clamp(5, 95))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: PointerKind, button: Option<PointerButton>, mods: Modifiers) -> PointerEvent {
        PointerEvent {
            kind,
            button,
            column: 0,
            row: 0,
            mods,
        }
    }

    const SGR_ANY: MouseMode = MouseMode {
        tracking: MouseTracking::AnyEvent,
        sgr: true,
    };
    const SGR_NORMAL: MouseMode = MouseMode {
        tracking: MouseTracking::Normal,
        sgr: true,
    };
    const X10_NORMAL: MouseMode = MouseMode {
        tracking: MouseTracking::Normal,
        sgr: false,
    };

    #[test]
    fn sgr_press_release_drag_and_wheel_encode_the_x11_button_codes() {
        let down = event(
            PointerKind::Down,
            Some(PointerButton::Left),
            Modifiers::NONE,
        );
        assert_eq!(
            encode_mouse_event(SGR_NORMAL, &down, 1, 1),
            Some(b"\x1b[<0;2;2M".to_vec())
        );

        let up = event(PointerKind::Up, Some(PointerButton::Left), Modifiers::NONE);
        assert_eq!(
            encode_mouse_event(SGR_NORMAL, &up, 1, 1),
            Some(b"\x1b[<0;2;2m".to_vec())
        );

        let drag = event(
            PointerKind::Drag,
            Some(PointerButton::Right),
            Modifiers::NONE,
        );
        assert_eq!(
            encode_mouse_event(SGR_ANY, &drag, 4, 9),
            Some(b"\x1b[<34;5;10M".to_vec())
        );

        let wheel_up = event(PointerKind::Wheel { dx: 0, dy: -1 }, None, Modifiers::NONE);
        assert_eq!(
            encode_mouse_event(SGR_NORMAL, &wheel_up, 0, 0),
            Some(b"\x1b[<64;1;1M".to_vec())
        );
        let wheel_down = event(PointerKind::Wheel { dx: 0, dy: 2 }, None, Modifiers::NONE);
        assert_eq!(
            encode_mouse_event(SGR_NORMAL, &wheel_down, 0, 0),
            Some(b"\x1b[<65;1;1M\x1b[<65;1;1M".to_vec())
        );
    }

    #[test]
    fn tracking_granularity_gates_motion_events() {
        let drag = event(
            PointerKind::Drag,
            Some(PointerButton::Left),
            Modifiers::NONE,
        );
        assert_eq!(encode_mouse_event(SGR_NORMAL, &drag, 0, 0), None);
        let motion = event(PointerKind::Move, None, Modifiers::NONE);
        assert_eq!(encode_mouse_event(SGR_NORMAL, &motion, 0, 0), None);
        assert_eq!(
            encode_mouse_event(SGR_ANY, &motion, 0, 0),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
        let off = MouseMode::default();
        let down = event(
            PointerKind::Down,
            Some(PointerButton::Left),
            Modifiers::NONE,
        );
        assert_eq!(encode_mouse_event(off, &down, 0, 0), None);
    }

    #[test]
    fn modifier_bits_ride_on_the_button_code() {
        let down = event(
            PointerKind::Down,
            Some(PointerButton::Left),
            Modifiers {
                shift: true,
                control: true,
                ..Modifiers::NONE
            },
        );
        assert_eq!(
            encode_mouse_event(SGR_NORMAL, &down, 0, 0),
            Some(b"\x1b[<20;1;1M".to_vec())
        );
    }

    #[test]
    fn x10_encoding_offsets_bytes_and_saturates_coordinates() {
        let down = event(
            PointerKind::Down,
            Some(PointerButton::Left),
            Modifiers::NONE,
        );
        assert_eq!(
            encode_mouse_event(X10_NORMAL, &down, 1, 1),
            Some(vec![0x1b, b'[', b'M', 32, 34, 34])
        );
        // Releases collapse to button code 3 in X10.
        let up = event(PointerKind::Up, Some(PointerButton::Left), Modifiers::NONE);
        assert_eq!(
            encode_mouse_event(X10_NORMAL, &up, 1, 1),
            Some(vec![0x1b, b'[', b'M', 32 + 3, 34, 34])
        );
        // Coordinates past the X10 ceiling clamp instead of wrapping.
        let far = encode_mouse_event(X10_NORMAL, &down, 500, 500).unwrap();
        assert_eq!(&far[4..], &[255, 255]);
    }

    #[test]
    fn split_percent_follows_the_pointer_and_clamps() {
        let area = SceneRect::new(0, 1, 120, 38);
        assert_eq!(
            split_percent_for_pointer(SplitAxis::Horizontal, area, 30, 0),
            Some(25)
        );
        assert_eq!(
            split_percent_for_pointer(SplitAxis::Horizontal, area, 0, 0),
            Some(5)
        );
        assert_eq!(
            split_percent_for_pointer(SplitAxis::Horizontal, area, 119, 0),
            Some(95)
        );
        let tall = SceneRect::new(60, 1, 60, 38);
        assert_eq!(
            split_percent_for_pointer(SplitAxis::Vertical, tall, 0, 20),
            Some(50)
        );
        assert_eq!(
            split_percent_for_pointer(SplitAxis::Vertical, SceneRect::new(0, 0, 10, 1), 0, 0),
            None
        );
    }
}
