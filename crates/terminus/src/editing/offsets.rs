//! The one conversion between the two offset spaces editing speaks.
//!
//! Every offset that crosses `terminus`'s public editing surface — a command's
//! range, [`super::TextInputState`]'s selection and composition — is a count
//! of UTF-16 code units into the host's value, the unit the DOM, the Input
//! Events specification and every platform input method use. Everything
//! inside — the value's `String`, parley's cursors — is a UTF-8 byte offset.
//! These functions are the only place one becomes the other, so a surrogate
//! pair is decided once.

pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

pub(crate) fn unit_of_byte(text: &str, byte: usize) -> usize {
    let mut units = 0;
    for (start, character) in text.char_indices() {
        if start + character.len_utf8() > byte {
            break;
        }
        units += character.len_utf16();
    }
    units
}

pub(crate) fn byte_of_unit(text: &str, unit: usize) -> Option<usize> {
    let mut units = 0;
    for (start, character) in text.char_indices() {
        if units == unit {
            return Some(start);
        }
        units += character.len_utf16();
        if units > unit {
            return None;
        }
    }
    (units == unit).then_some(text.len())
}

pub(crate) fn byte_of_unit_floor(text: &str, unit: usize) -> usize {
    let mut units = 0;
    for (start, character) in text.char_indices() {
        units += character.len_utf16();
        if units > unit {
            return start;
        }
    }
    text.len()
}

pub(crate) fn floor_char_boundary(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut byte = byte;
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const MIXED: &str = "a😀👩\u{200d}💻e\u{301}中\r\nz";

    #[test]
    fn lengths_count_code_units_not_characters_or_bytes() {
        assert_eq!(utf16_len(""), 0);
        assert_eq!(utf16_len("ab"), 2);
        assert_eq!(utf16_len("😀"), 2);
        assert_eq!(utf16_len("👩\u{200d}💻"), 5);
        assert_eq!(utf16_len("e\u{301}"), 2);
        assert_eq!(utf16_len("中"), 1);
        assert_eq!(utf16_len("\r\n"), 2);
        assert_eq!(utf16_len(MIXED), 1 + 2 + 5 + 2 + 1 + 2 + 1);
    }

    #[test]
    fn every_boundary_round_trips() {
        let mut unit = 0;
        for (byte, character) in MIXED.char_indices() {
            assert_eq!(byte_of_unit(MIXED, unit), Some(byte));
            assert_eq!(byte_of_unit_floor(MIXED, unit), byte);
            assert_eq!(unit_of_byte(MIXED, byte), unit);
            unit += character.len_utf16();
        }
        assert_eq!(byte_of_unit(MIXED, unit), Some(MIXED.len()));
        assert_eq!(unit_of_byte(MIXED, MIXED.len()), unit);
    }

    #[test]
    fn a_unit_inside_a_surrogate_pair_is_refused_or_rounded_down() {
        let text = "a😀";
        assert_eq!(byte_of_unit(text, 2), None);
        assert_eq!(byte_of_unit_floor(text, 2), 1);
        assert_eq!(byte_of_unit(text, 3), Some(5));
        let zwj = "👩\u{200d}💻";
        assert_eq!(byte_of_unit(zwj, 1), None);
        assert_eq!(byte_of_unit(zwj, 2), Some(4));
        assert_eq!(byte_of_unit(zwj, 3), Some(7));
        assert_eq!(byte_of_unit(zwj, 4), None);
    }

    #[test]
    fn combining_marks_and_crlf_are_ordinary_code_units() {
        let text = "e\u{301}";
        assert_eq!(byte_of_unit(text, 1), Some(1));
        assert_eq!(unit_of_byte(text, 3), 2);
        assert_eq!(byte_of_unit("\r\n", 1), Some(1));
    }

    #[test]
    fn offsets_past_the_end_are_refused_or_clamped() {
        assert_eq!(byte_of_unit("ab", 3), None);
        assert_eq!(byte_of_unit_floor("ab", 3), 2);
        assert_eq!(unit_of_byte("ab", 9), 2);
        assert_eq!(unit_of_byte("中z", 1), 0);
        assert_eq!(floor_char_boundary("中z", 2), 0);
        assert_eq!(floor_char_boundary("中z", 9), 4);
    }
}
