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

const OBJECT_LEN: usize = 3;

#[derive(Clone, Copy)]
pub struct OffsetMap<'a> {
    text: &'a str,
    map: &'a SourceMap,
    cut_byte: usize,
    boxes: &'a [BoxAt],
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

    #[must_use]
    pub const fn display_text(&self) -> &'a str {
        self.text
    }

    #[must_use]
    pub const fn source_len(&self) -> u32 {
        self.map.source_len()
    }

    #[must_use]
    pub fn boxes_before(&self, unit: u32) -> u32 {
        self.map.boxes_before(unit)
    }

    #[must_use]
    pub const fn cut_byte(&self) -> usize {
        self.cut_byte + OBJECT_LEN * self.shown_boxes
    }

    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.cut_byte < self.text.len()
    }

    #[must_use]
    pub fn unit_to_byte(&self, unit: u32) -> Option<usize> {
        let byte = self.map.unit_to_byte(self.text, unit)? as usize;
        let shown = &self.boxes[..self.shown_boxes];
        if let Ok(slot) = shown.binary_search_by_key(&unit, |entry| entry.unit) {
            return Some(self.object_byte(slot));
        }
        Some(byte + OBJECT_LEN * shown.partition_point(|entry| entry.byte as usize <= byte))
    }

    #[must_use]
    pub fn unit_to_byte_or_end(&self, unit: u32) -> Option<usize> {
        let shown = self.map.shown_len();
        match unit.cmp(&shown) {
            core::cmp::Ordering::Less => self.unit_to_byte(unit),
            core::cmp::Ordering::Equal => Some(self.text.len() + OBJECT_LEN * self.shown_boxes),
            core::cmp::Ordering::Greater => None,
        }
    }

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

    #[must_use]
    pub fn unit_before(&self, byte: usize) -> u32 {
        let (text_byte, object) = self.text_byte(byte);
        let boxes_before = object.unwrap_or_else(|| self.objects_before(byte));
        let text_byte = self.floor(text_byte).min(self.text.len());
        let unit = self.map.unit_before(self.text, Self::narrow(text_byte));
        boxes_before
            .checked_sub(1)
            .map(|slot| &self.boxes[slot])
            .filter(|entry| entry.byte as usize == text_byte)
            .map_or(unit, |entry| unit.max(entry.unit + 1))
    }

    fn object_byte(&self, slot: usize) -> usize {
        self.boxes[slot].byte as usize + OBJECT_LEN * slot
    }

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

    fn text_byte(&self, byte: usize) -> (usize, Option<usize>) {
        let before = self.objects_before(byte + 1);
        if let Some(slot) = before.checked_sub(1)
            && byte < self.object_byte(slot) + OBJECT_LEN
        {
            return (self.boxes[slot].byte as usize, Some(slot));
        }
        (byte - OBJECT_LEN * before, None)
    }

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
