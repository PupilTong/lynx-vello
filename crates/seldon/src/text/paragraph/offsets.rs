//! The read-only offset bridge between a laid-out paragraph's layout bytes
//! and its source units, for a host that addresses the layout in offsets of
//! the text it was built from — an editor's caret and selection.
//!
//! Parley indexes the display layout by UTF-8 bytes of the text it was built
//! over: the *normalized* text — white space collapsed or kept by the
//! paragraph's processing, an inline box's edges and other consumer text
//! spliced in, and for a truncated Lynx paragraph a tail laid in after the
//! cut — with a U+FFFC before the byte of each atomic box, which is how
//! parley 0.13 holds an inline object (`super::shape::Objects`). The host's
//! own offsets are source UTF-16 units: the pre-collapse flattened content,
//! an atomic box counting one ([`super::content`]). [`OffsetMap`] is the one
//! conversion between the two, read off the paragraph's own source map, so
//! it agrees with the line reports, the range boxes and the hit test.

use super::content::{BoxAt, SourceMap};

/// The UTF-8 length of the U+FFFC each atomic box adds to the layout's text.
const OBJECT_LEN: usize = 3;

/// One committed paragraph's offset bridge: its normalized text, its source
/// map, its atomic boxes, and where the source content ends in its display
/// layout.
///
/// Both paragraph kinds hand one out
/// ([`crate::text::block::TextBlock::offset_map`],
/// [`crate::text::inline::InlineFlow::offset_map`]).
#[derive(Clone, Copy)]
pub struct OffsetMap<'a> {
    text: &'a str,
    map: &'a SourceMap,
    /// Where the source content ends in the normalized text.
    cut_byte: usize,
    /// The content's atomic boxes, in text order.
    boxes: &'a [BoxAt],
    /// How many of `boxes` the display layout holds: those before the cut,
    /// which come first in its text.
    shown_boxes: usize,
}

impl core::fmt::Debug for OffsetMap<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OffsetMap")
            .field("text", &self.text)
            .field("cut_byte", &self.cut_byte)
            .field("shown_boxes", &self.shown_boxes)
            .finish_non_exhaustive()
    }
}

impl<'a> OffsetMap<'a> {
    pub(in crate::text) fn new(
        text: &'a str,
        map: &'a SourceMap,
        cut_byte: usize,
        boxes: &'a [BoxAt],
        shown_boxes: usize,
    ) -> Self {
        Self {
            text,
            map,
            cut_byte,
            boxes,
            shown_boxes: shown_boxes.min(boxes.len()),
        }
    }

    /// The paragraph's normalized content text, up to [`Self::cut_byte`] the
    /// text the display layout was built over, but for the U+FFFC the
    /// layout holds before the byte of each atomic box. Past a truncation
    /// cut the display layout holds its tail instead.
    #[must_use]
    pub const fn display_text(&self) -> &'a str {
        self.text
    }

    /// How many source UTF-16 units the paragraph was built from.
    #[must_use]
    pub const fn source_len(&self) -> u32 {
        self.map.source_len()
    }

    /// How many atomic inline boxes the source units before `unit` hold.
    /// Every box counts one source unit and is no text, so
    /// `unit - boxes_before(unit)` is `unit` as an offset into the
    /// paragraph's text content alone. A box past the end of the shown
    /// source content — after a `<text>`'s `nowrap` cut — is not counted.
    #[must_use]
    pub fn boxes_before(&self, unit: u32) -> u32 {
        self.map.boxes_before(unit)
    }

    /// Where the source content ends in the display layout's text: the
    /// text's end, or a truncation cut's byte, past which the display layout
    /// shows the dots or the inline-truncation content — no source text.
    #[must_use]
    pub const fn cut_byte(&self) -> usize {
        self.cut_byte + OBJECT_LEN * self.shown_boxes
    }

    /// Whether a truncation cut replaced the content's tail in the display
    /// layout: then a byte at or past [`Self::cut_byte`] is no source text.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.cut_byte < self.text.len()
    }

    /// The display byte of the source unit `unit`, read as a position before
    /// that unit's character. A unit inside a surrogate pair rounds down to
    /// the character's start, one inside a collapsed white-space span lands
    /// on the byte the span emitted, and one on an atomic box on the box's
    /// object.
    ///
    /// `None` at or past the end of the source content the text shows: the
    /// text's end has no unit of its own here ([`Self::unit_to_byte_or_end`]
    /// answers it), and a `<text>`'s `nowrap` cut drops everything after its
    /// first line from the text.
    #[must_use]
    pub fn unit_to_byte(&self, unit: u32) -> Option<usize> {
        let byte = self.map.unit_to_byte(self.text, unit)? as usize;
        let shown = &self.boxes[..self.shown_boxes];
        if let Ok(slot) = shown.binary_search_by_key(&unit, |entry| entry.unit) {
            return Some(self.object_byte(slot));
        }
        // The boxes at the byte come before its character.
        Some(byte + OBJECT_LEN * shown.partition_point(|entry| entry.byte as usize <= byte))
    }

    /// [`Self::unit_to_byte`], with the end of the shown source content —
    /// the source's end, unless a `<text>`'s `nowrap` cut dropped what
    /// follows its first line — answering the text's end. `None` past it.
    #[must_use]
    pub fn unit_to_byte_or_end(&self, unit: u32) -> Option<usize> {
        let shown = self.map.shown_len();
        match unit.cmp(&shown) {
            core::cmp::Ordering::Less => self.unit_to_byte(unit),
            core::cmp::Ordering::Equal => Some(self.text.len() + OBJECT_LEN * self.shown_boxes),
            core::cmp::Ordering::Greater => None,
        }
    }

    /// The source unit of the display byte `byte` read as a *start*: the
    /// unit of the character at it. A byte of spliced text (an inline box's
    /// edge), which has no unit, answers the unit after it; a collapsed
    /// span's emitted byte answers the span's first unit; an atomic box's
    /// object answers the box's unit. A byte inside a character counts as
    /// that character's start; at or past the text's end it is
    /// [`Self::unit_before`] of the end.
    #[must_use]
    pub fn byte_to_unit(&self, byte: usize) -> u32 {
        let (byte, object) = self.text_byte(byte);
        if let Some(slot) = object {
            return self.boxes[slot].unit;
        }
        let byte = self.floor(byte);
        if byte >= self.text.len() {
            return self
                .map
                .unit_before(self.text, Self::narrow(self.text.len()));
        }
        self.map.byte_to_unit(self.text, Self::narrow(byte))
    }

    /// The source unit boundary at the display byte `byte` read as an
    /// *end*: every unit strictly before the byte, plus a collapsed span
    /// sitting at it, but never an atomic box at it — a box whose object
    /// lies before the byte is before it. A byte inside a character counts
    /// as that character's start; past the text's end it is the end.
    #[must_use]
    pub fn unit_before(&self, byte: usize) -> u32 {
        let (text_byte, object) = self.text_byte(byte);
        // The boxes whose objects end at or before the byte.
        let boxes_before = object.unwrap_or_else(|| self.objects_before(byte));
        let text_byte = self.floor(text_byte).min(self.text.len());
        let unit = self.map.unit_before(self.text, Self::narrow(text_byte));
        boxes_before
            .checked_sub(1)
            .map(|slot| &self.boxes[slot])
            .filter(|entry| entry.byte as usize == text_byte)
            .map_or(unit, |entry| unit.max(entry.unit + 1))
    }

    /// The display byte of shown box `slot`'s object.
    fn object_byte(&self, slot: usize) -> usize {
        self.boxes[slot].byte as usize + OBJECT_LEN * slot
    }

    /// How many shown boxes' objects start before display byte `byte`: a
    /// binary search over the slots, whose object bytes ascend with them.
    fn objects_before(&self, byte: usize) -> usize {
        let mut low = 0;
        let mut high = self.shown_boxes;
        while low < high {
            let mid = low + (high - low) / 2;
            if self.object_byte(mid) < byte {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }

    /// The normalized byte at display byte `byte`, and the shown box whose
    /// object holds the byte, if one does: a byte inside an object counts as
    /// its start.
    fn text_byte(&self, byte: usize) -> (usize, Option<usize>) {
        let before = self.objects_before(byte + 1);
        if let Some(slot) = before.checked_sub(1)
            && byte < self.object_byte(slot) + OBJECT_LEN
        {
            return (self.boxes[slot].byte as usize, Some(slot));
        }
        (byte - OBJECT_LEN * before, None)
    }

    /// `byte` moved down to a character boundary of the text.
    fn floor(&self, byte: usize) -> usize {
        if byte >= self.text.len() {
            return byte;
        }
        let mut byte = byte;
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    fn narrow(byte: usize) -> u32 {
        u32::try_from(byte).expect("paragraph text fits u32")
    }
}
