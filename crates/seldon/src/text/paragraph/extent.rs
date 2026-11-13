//! Inline extents in source units: the walk every range and hit query over a
//! laid-out paragraph line is built on.

use core::ops::Range;

use parley::{Line, PositionedLayoutItem};

use super::shape::Objects;
use super::{Additions, Paragraph};
use crate::style::TextBrush;

impl<S, A: Additions> Paragraph<S, A> {
    /// Calls `visit(units, left, right)` once per cluster or atomic box of
    /// `line` that shows source content, in visual order, with the source
    /// units it covers and its inline extent.
    ///
    /// An inserted segment's cluster covers no source unit and is skipped;
    /// an atom's extent is its own margin box, without the advance laid
    /// beside it ([`Additions::box_advance`]).
    ///
    /// `line` belongs to a layout whose text starts with this paragraph's
    /// normalized content: the natural layout, or one a consumer built over a
    /// prefix of the content followed by text of its own. `cut_byte` is where
    /// the content ends in that layout's text and `content_slots` how many of
    /// its box slots are content boxes, which come first. What lies past
    /// either is no source text and is skipped here, once, for every reader.
    /// Clipping to the line's own source range, which is what drops the
    /// whitespace hanging at its end, is the caller's.
    ///
    /// `objects` are that layout's ([`Objects::of`]): its cluster bytes
    /// count each box's object, which `cut_byte` and the text do not.
    pub(in crate::text) fn for_each_extent(
        &self,
        line: &Line<'_, TextBrush>,
        objects: &Objects,
        cut_byte: usize,
        content_slots: usize,
        mut visit: impl FnMut(Range<u32>, f32, f32),
    ) {
        let block = &self.content.block;
        let text = &block.text;
        // A run is split into one glyph run per style; its clusters are
        // walked once, from the first glyph run's offset, which is where the
        // run starts. A box's object is a run of its own, which parley
        // yields as the box rather than as a glyph run.
        let mut walked_run = None;
        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => {
                    let run = glyph_run.run();
                    if walked_run == Some(run.index()) {
                        continue;
                    }
                    walked_run = Some(run.index());
                    let mut x = glyph_run.offset();
                    for cluster in run.visual_clusters() {
                        let advance = cluster.advance();
                        let layout_bytes = cluster.text_range();
                        let bytes = objects.text_byte(layout_bytes.start) as usize
                            ..objects.text_byte(layout_bytes.end) as usize;
                        if bytes.start < cut_byte {
                            let first = block.map.byte_to_unit(
                                text,
                                u32::try_from(bytes.start).expect("text fits u32"),
                            );
                            let last = block.map.unit_before(
                                text,
                                u32::try_from(bytes.end.min(cut_byte)).expect("text fits u32"),
                            );
                            if first < last {
                                visit(first..last, x, x + advance);
                            }
                        }
                        x += advance;
                    }
                }
                PositionedLayoutItem::InlineBox(inline_box) => {
                    let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
                    if slot < content_slots {
                        let unit = block.boxes[slot].unit;
                        let (before, after) = self.content.additions.box_advance(slot);
                        visit(
                            unit..unit + 1,
                            inline_box.x + before,
                            inline_box.x + inline_box.width - after,
                        );
                    }
                }
            }
        }
    }
}
