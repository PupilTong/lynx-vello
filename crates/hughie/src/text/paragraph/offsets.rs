//! The read-only offset bridge between a laid-out paragraph's layout bytes
//! and its source units, for a host that addresses the layout in offsets of
//! the text it was built from — an editor's caret and selection.
//!
//! Parley indexes the display layout by UTF-8 bytes of the *normalized*
//! text: white space collapsed or kept by the paragraph's processing, an
//! inline box's edges and other consumer text spliced in, and for a
//! truncated Lynx paragraph a tail laid in after the cut. The host's own
//! offsets are source UTF-16 units: the pre-collapse flattened content, an
//! atomic box counting one ([`super::content`]). [`OffsetMap`] is the one
//! conversion between the two, read off the paragraph's own source map, so
//! it agrees with the line reports, the range boxes and the hit test.

use super::content::SourceMap;

/// One committed paragraph's offset bridge: its normalized text, its source
/// map, and where the source content ends in its display layout.
///
/// Both paragraph kinds hand one out
/// ([`crate::text::block::TextBlock::offset_map`],
/// [`crate::text::inline::InlineFlow::offset_map`]).
#[derive(Clone, Copy)]
pub struct OffsetMap<'a> {
    text: &'a str,
    map: &'a SourceMap,
    cut_byte: usize,
}

impl core::fmt::Debug for OffsetMap<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OffsetMap")
            .field("text", &self.text)
            .field("cut_byte", &self.cut_byte)
            .finish_non_exhaustive()
    }
}

impl<'a> OffsetMap<'a> {
    pub(in crate::text) const fn new(text: &'a str, map: &'a SourceMap, cut_byte: usize) -> Self {
        Self {
            text,
            map,
            cut_byte,
        }
    }

    /// The paragraph's normalized content text: the string whose UTF-8 byte
    /// offsets the display layout's clusters index, up to [`Self::cut_byte`].
    /// Past a truncation cut the display layout holds its tail instead.
    #[must_use]
    pub const fn display_text(&self) -> &'a str {
        self.text
    }

    /// How many source UTF-16 units the paragraph was built from.
    #[must_use]
    pub const fn source_len(&self) -> u32 {
        self.map.source_len()
    }

    /// Where the source content ends in the display layout's text: the
    /// text's length, or a truncation cut's byte, past which the display
    /// layout shows the dots or the inline-truncation content — no source
    /// text.
    #[must_use]
    pub const fn cut_byte(&self) -> usize {
        self.cut_byte
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
    /// byte.
    ///
    /// `None` at or past the end of the source content the text shows: the
    /// text's end has no unit of its own here ([`Self::unit_to_byte_or_end`]
    /// answers it), and a `<text>`'s `nowrap` cut drops everything after its
    /// first line from the text.
    #[must_use]
    pub fn unit_to_byte(&self, unit: u32) -> Option<usize> {
        self.map
            .unit_to_byte(self.text, unit)
            .map(|byte| byte as usize)
    }

    /// [`Self::unit_to_byte`], with the end of the shown source content —
    /// the source's end, unless a `<text>`'s `nowrap` cut dropped what
    /// follows its first line — answering the text's end. `None` past it.
    #[must_use]
    pub fn unit_to_byte_or_end(&self, unit: u32) -> Option<usize> {
        let shown = self.map.shown_len();
        match unit.cmp(&shown) {
            core::cmp::Ordering::Less => self.unit_to_byte(unit),
            core::cmp::Ordering::Equal => Some(self.text.len()),
            core::cmp::Ordering::Greater => None,
        }
    }

    /// The source unit of the display byte `byte` read as a *start*: the
    /// unit of the character at it. A byte of spliced text (an inline box's
    /// edge), which has no unit, answers the unit after it; a collapsed
    /// span's emitted byte answers the span's first unit. A byte inside a
    /// character counts as that character's start; at or past the text's end
    /// it is [`Self::unit_before`] of the end.
    #[must_use]
    pub fn byte_to_unit(&self, byte: usize) -> u32 {
        let byte = self.floor(byte);
        if byte >= self.text.len() {
            return self.unit_before(self.text.len());
        }
        self.map.byte_to_unit(self.text, Self::narrow(byte))
    }

    /// The source unit boundary at the display byte `byte` read as an
    /// *end*: every unit strictly before the byte, plus a collapsed span
    /// sitting at it, but never an atomic box at it. A byte inside a
    /// character counts as that character's start; past the text's end it
    /// is the end.
    #[must_use]
    pub fn unit_before(&self, byte: usize) -> u32 {
        let byte = self.floor(byte).min(self.text.len());
        self.map.unit_before(self.text, Self::narrow(byte))
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
