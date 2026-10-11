//! Source-range geometry: where a range of the paragraph's source text sits.
//!
//! The query Lynx's `getTextBoundingRect({start, end})` UI method is written
//! over. Its offsets are the layout event's: UTF-16 units of the
//! pre-collapse flattened source, an atomic box counting one
//! ([`super::content`]). The answer is one box per rendered line the range
//! shows content on, which is what iOS's `getTextBoundingBoxes:withEnd:`
//! reports (`lynx/platform/darwin/ios/lynx/ui/text/LynxTextView.m:205-235`,
//! the layout manager's enclosing rect per line fragment).

use parley::PositionedLayoutItem;

use super::TextBlock;
use crate::geometry::{Point, Rect, Size};

impl TextBlock {
    /// The boxes the source range `[start, end)` occupies in the committed
    /// layout, in the paragraph's own coordinates (the origin is the
    /// content box's top-left, as [`Self::size`] measures).
    ///
    /// One box per rendered line the range shows content on, in line order.
    /// A box spans from the inline start of the leftmost selected cluster or
    /// atomic box on that line to the inline end of the rightmost one, and
    /// takes its `top` and `height` from the line ([`super::LineInfo`]), so a
    /// box is the line's height whatever the selected glyphs' own extents.
    /// An atomic box's one unit selects its placed margin box's horizontal
    /// extent.
    ///
    /// What shows no content contributes no box: text past a truncation cut
    /// has no glyph, the dots and the inline-truncation content are no
    /// source text, and the collapsible whitespace a soft wrap or a preserved
    /// newline leaves at a line's end belongs to no line, as in
    /// [`Self::lines`]. A range that covers only such text answers an empty
    /// list. Collapsed whitespace that renders nothing at all — a dropped
    /// segment break between two CJK characters — counts as part of the
    /// cluster before it.
    ///
    /// `start == end` answers one box of zero width at `(0, 0)`, as tall as
    /// the line holding `start`: the natives' answer for an empty range
    /// (iOS's `CGRectMake(0, 0, 0, lineRect.size.height)`), kept for parity
    /// although it says nothing about where `start` is.
    ///
    /// `None` when `start > end`, when `end` is past the source length, or,
    /// for `start == end`, when the paragraph has no line at all.
    ///
    /// # Panics
    ///
    /// When the block has never been laid out; a host asks
    /// [`Self::has_committed`] first, as for every other reader.
    #[must_use]
    pub fn range_boxes(&self, start: u32, end: u32) -> Option<Vec<Rect<f32>>> {
        let block = &self.content.block;
        if start > end || end > block.map.source_len() {
            return None;
        }
        let lines = self.lines();
        if start == end {
            let line = lines
                .iter()
                .rev()
                .find(|line| line.source_start <= start)
                .or_else(|| lines.first())?;
            return Some(vec![Rect::new(
                Point::new(0.0, 0.0),
                Size::new(0.0, line.height),
            )]);
        }

        let text = &block.text;
        let (cut_byte, content_slots) = self
            .expect_result()
            .display
            .as_ref()
            .map_or((text.len(), block.boxes.len()), |part| {
                (part.cut_byte as usize, part.content_slots)
            });
        let mut boxes = Vec::new();
        // The display layout's lines are the reported lines, index for
        // index: `assemble` builds one from the other.
        for (line, info) in self.display().lines().zip(lines) {
            // Clipping to the line's own range is what drops its trailing
            // collapsible whitespace, which the range excludes.
            let low = start.max(info.source_start);
            let high = end.min(info.source_end);
            if low >= high {
                continue;
            }
            let mut extent: Option<(f32, f32)> = None;
            let mut include = |left: f32, right: f32| {
                extent = Some(
                    extent.map_or((left, right), |(min, max)| (min.min(left), max.max(right))),
                );
            };
            // A run is split into one glyph run per style; its clusters are
            // walked once, from the first glyph run's offset, which is where
            // the run starts.
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
                            let bytes = cluster.text_range();
                            if bytes.start < cut_byte {
                                let first = block.map.byte_to_unit(
                                    text,
                                    u32::try_from(bytes.start).expect("text fits u32"),
                                );
                                let last = block.map.unit_before(
                                    text,
                                    u32::try_from(bytes.end.min(cut_byte)).expect("text fits u32"),
                                );
                                if first < high && last > low {
                                    include(x, x + advance);
                                }
                            }
                            x += advance;
                        }
                    }
                    PositionedLayoutItem::InlineBox(inline_box) => {
                        let slot =
                            usize::try_from(inline_box.id).expect("slot ids are table indexes");
                        if slot < content_slots {
                            let unit = block.boxes[slot].unit;
                            if low <= unit && unit < high {
                                include(inline_box.x, inline_box.x + inline_box.width);
                            }
                        }
                    }
                }
            }
            if let Some((left, right)) = extent {
                boxes.push(Rect::new(
                    Point::new(left, info.top),
                    Size::new(right - left, info.height),
                ));
            }
        }
        Some(boxes)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem's em squares have exact metrics.

    use core::num::NonZeroU32;

    use super::super::tests::{ahem_style, text_context};
    use super::super::{
        BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, RunStyle, TextBlock, TextOverflow,
        TextRunItem, VerticalAlign,
    };
    use crate::geometry::{Point, Rect, Size};

    fn run<'a>(text: &'a str, style: &'a RunStyle) -> InlineItem<'a> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            preserve_newlines: false,
        })
    }

    /// One committed block over `items` at `width`.
    fn block(items: &[InlineItem<'_>], style: BlockStyle, width: Option<f32>) -> TextBlock {
        let mut context = text_context();
        let mut block = TextBlock::new(&mut context, style, items, None);
        block.commit(&mut context, BlockConstraint::new(width, 0.0));
        block
    }

    fn rect(left: f32, top: f32, width: f32, height: f32) -> Rect<f32> {
        Rect::new(Point::new(left, top), Size::new(width, height))
    }

    #[test]
    fn a_range_on_one_line_spans_its_clusters() {
        let style = ahem_style();
        let block = block(&[run("abcdef", &style)], BlockStyle::default(), None);
        let height = block.lines()[0].height;
        assert_eq!(height, 10.0);
        assert_eq!(
            block.range_boxes(0, 3),
            Some(vec![rect(0.0, 0.0, 30.0, 10.0)])
        );
        assert_eq!(
            block.range_boxes(2, 6),
            Some(vec![rect(20.0, 0.0, 40.0, 10.0)])
        );
    }

    #[test]
    fn a_wrapped_range_answers_one_box_per_line() {
        let style = ahem_style();
        // "abc " then "def" at 35px: the space hangs at the first line's end.
        let block = block(&[run("abc def", &style)], BlockStyle::default(), Some(35.0));
        assert_eq!(block.lines().len(), 2);
        assert_eq!(
            block.range_boxes(1, 6),
            Some(vec![
                rect(10.0, 0.0, 20.0, 10.0),
                rect(0.0, 10.0, 20.0, 10.0),
            ]),
            "from the start cluster to the first line's last glyph, then from 0 to the end \
             cluster; the hanging space adds nothing",
        );
        assert_eq!(
            block.range_boxes(3, 4),
            Some(Vec::new()),
            "the space a soft wrap consumed is on no line"
        );
    }

    #[test]
    fn an_empty_range_is_a_zero_box_as_tall_as_its_line() {
        let style = RunStyle {
            line_height: super::super::LineHeight::Px(16.0),
            ..ahem_style()
        };
        let block = block(&[run("abc def", &style)], BlockStyle::default(), Some(35.0));
        for start in [0, 2, 5, 7] {
            assert_eq!(
                block.range_boxes(start, start),
                Some(vec![rect(0.0, 0.0, 0.0, 16.0)]),
                "{start}"
            );
        }
    }

    #[test]
    fn a_surrogate_pair_counts_two_units() {
        let style = ahem_style();
        let block = block(&[run("a\u{1F600}b", &style)], BlockStyle::default(), None);
        let emoji = block.range_boxes(1, 3).expect("in range")[0];
        assert_eq!(emoji.origin.x, 10.0);
        assert!(emoji.size.width > 0.0);
        assert_eq!(
            block.range_boxes(1, 2),
            Some(vec![emoji]),
            "half the pair selects its cluster"
        );
        assert_eq!(
            block.range_boxes(3, 4),
            Some(vec![rect(10.0 + emoji.size.width, 0.0, 10.0, 10.0)]),
            "`b` is unit 3",
        );
        assert_eq!(block.range_boxes(4, 4).map(|boxes| boxes.len()), Some(1));
        assert_eq!(
            block.range_boxes(4, 5),
            None,
            "the source is four units long"
        );
    }

    #[test]
    fn an_atomic_box_is_one_unit_spanning_its_placed_box() {
        let style = ahem_style();
        let items = [
            run("ab", &style),
            InlineItem::Box(InlineBoxSpec {
                id: 7,
                size: Size::new(20.0, 10.0),
                baseline: None,
                vertical_align: VerticalAlign::Baseline,
            }),
            run("cd", &style),
        ];
        let block = block(&items, BlockStyle::default(), None);
        // The box's line, whatever the box does to its top and height.
        let line = block.lines()[0];
        let (top, height) = (line.top, line.height);
        assert_eq!(
            block.range_boxes(2, 3),
            Some(vec![rect(20.0, top, 20.0, height)])
        );
        assert_eq!(
            block.range_boxes(1, 4),
            Some(vec![rect(10.0, top, 40.0, height)])
        );
        assert_eq!(
            block.range_boxes(3, 5),
            Some(vec![rect(40.0, top, 20.0, height)])
        );
    }

    #[test]
    fn text_past_the_cut_has_no_box_and_the_marker_is_not_source_text() {
        let style = ahem_style();
        let block_style = BlockStyle {
            max_lines: NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        };
        // At 45px one line fits "a" and the three dots: the cut is unit 1.
        let block = block(&[run("aaaa bbbb", &style)], block_style, Some(45.0));
        assert!(block.truncated());
        assert_eq!(block.lines().len(), 1);
        assert_eq!(block.lines()[0].ellipsis_count, 3);
        assert_eq!(
            block.range_boxes(0, 4),
            Some(vec![rect(0.0, 0.0, 10.0, 10.0)])
        );
        assert_eq!(
            block.range_boxes(1, 9),
            Some(Vec::new()),
            "everything past the cut, and the dots drawn there"
        );
        assert_eq!(
            block.range_boxes(5, 9),
            Some(Vec::new()),
            "a line never shown"
        );
        assert_eq!(block.range_boxes(0, 10), None);
    }

    #[test]
    fn a_reversed_or_overlong_range_is_none() {
        let style = ahem_style();
        let block = block(&[run("abc", &style)], BlockStyle::default(), None);
        assert_eq!(block.range_boxes(2, 1), None);
        assert_eq!(block.range_boxes(0, 4), None);
        assert_eq!(block.range_boxes(3, 3).map(|boxes| boxes.len()), Some(1));
    }
}
