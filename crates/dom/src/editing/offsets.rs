//! The one conversion between the two offset spaces editing speaks.
//!
//! Every offset that crosses `dom`'s public editing surface — a command's
//! range, [`super::TextInputState`]'s selection and composition — is a count
//! of UTF-16 code units into the host's value, the unit the DOM, the Input
//! Events specification and every platform input method use. Everything
//! inside — the value's `String`, parley's cursors — is a UTF-8 byte offset.
//! These functions are the only place one becomes the other, so a surrogate
//! pair is decided once.

/// How many UTF-16 code units `text` is.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// The UTF-16 offset of the byte offset `byte`.
///
/// A `byte` inside a character counts that character's units as not yet
/// reached, so a caller holding a stale offset gets the start of the
/// character rather than a panic; past the end answers the whole length.
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

/// The byte offset of the UTF-16 offset `unit`, or `None` when `unit` falls
/// between the two halves of a surrogate pair or past the end of `text`.
///
/// The exact form, for the commands that must refuse a range splitting a
/// character (`DeleteSurrounding`, `ExtendSelectionAndDelete`).
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

/// The byte offset of the UTF-16 offset `unit`, rounding a split surrogate
/// pair down to the start of its character and an offset past the end down
/// to the end.
///
/// The lenient form, for selections, which a platform may place anywhere and
/// which are clamped rather than refused.
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

/// `byte` moved down to the nearest character boundary of `text`, clamped
/// to its length.
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

    /// ASCII, a surrogate pair, a ZWJ sequence of surrogate pairs, a
    /// combining mark, a CJK character (three UTF-8 bytes, one UTF-16 unit)
    /// and a CRLF pair.
    const MIXED: &str = "a😀👩\u{200d}💻e\u{301}中\r\nz";

    #[test]
    fn lengths_count_code_units_not_characters_or_bytes() {
        assert_eq!(utf16_len(""), 0);
        assert_eq!(utf16_len("ab"), 2);
        assert_eq!(utf16_len("😀"), 2);
        // Woman, ZWJ, laptop: two pairs and one BMP joiner.
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
        // "a😀": the emoji's units are 1 and 2, its bytes 1..5.
        let text = "a😀";
        assert_eq!(byte_of_unit(text, 2), None);
        assert_eq!(byte_of_unit_floor(text, 2), 1);
        assert_eq!(byte_of_unit(text, 3), Some(5));
        // Inside the ZWJ sequence, each pair is split independently, while
        // the joiner between them is a boundary of its own.
        let zwj = "👩\u{200d}💻";
        assert_eq!(byte_of_unit(zwj, 1), None);
        assert_eq!(byte_of_unit(zwj, 2), Some(4));
        assert_eq!(byte_of_unit(zwj, 3), Some(7));
        assert_eq!(byte_of_unit(zwj, 4), None);
    }

    #[test]
    fn combining_marks_and_crlf_are_ordinary_code_units() {
        // A combining mark is its own unit and its own boundary: the offset
        // space does not know about grapheme clusters; the commands that
        // delete by cluster ask the layout.
        let text = "e\u{301}";
        assert_eq!(byte_of_unit(text, 1), Some(1));
        assert_eq!(unit_of_byte(text, 3), 2);
        // CRLF is two units, and the boundary between them is one.
        assert_eq!(byte_of_unit("\r\n", 1), Some(1));
    }

    #[test]
    fn offsets_past_the_end_are_refused_or_clamped() {
        assert_eq!(byte_of_unit("ab", 3), None);
        assert_eq!(byte_of_unit_floor("ab", 3), 2);
        assert_eq!(unit_of_byte("ab", 9), 2);
        // A byte inside a character answers that character's start.
        assert_eq!(unit_of_byte("中z", 1), 0);
        assert_eq!(floor_char_boundary("中z", 2), 0);
        assert_eq!(floor_char_boundary("中z", 9), 4);
    }
}
