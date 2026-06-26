//! OSC 52 clipboard write helper.
//!
//! Copy mode writes selected terminal text to the host terminal's clipboard via
//! the OSC 52 escape sequence. This is terminal-native (works over SSH) and
//! needs no platform clipboard dependency. The host terminal must support OSC 52
//! for the copy to reach the system clipboard; that requirement is the
//! documented baseline.

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(BASE64_ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(BASE64_ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Build an OSC 52 "set system clipboard" sequence for `text`.
pub fn osc52_sequence(text: &str) -> Vec<u8> {
    let mut sequence = Vec::new();
    sequence.extend_from_slice(b"\x1b]52;c;");
    sequence.extend_from_slice(base64_encode(text.as_bytes()).as_bytes());
    sequence.push(0x07);
    sequence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    }

    #[test]
    fn osc52_wraps_base64_payload() {
        let sequence = osc52_sequence("hi");
        assert_eq!(sequence.first(), Some(&0x1b));
        assert_eq!(sequence.last(), Some(&0x07));
        let rendered = String::from_utf8(sequence).unwrap();
        assert!(rendered.starts_with("\x1b]52;c;"));
        assert!(rendered.contains("aGk=")); // base64("hi")
    }
}
