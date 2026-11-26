//! Source-range geometry: where a range of the paragraph's source text sits.
//!
//! The query Lynx's `getTextBoundingRect({start, end})` UI method is written
//! over. Its offsets are the layout event's: UTF-16 units of the
//! pre-collapse flattened source, an atomic box counting one
//! ([`super::content`]). The answer is one box per rendered line the range
//! shows content on, which is what iOS's `getTextBoundingBoxes:withEnd:`
//! reports (`lynx/platform/darwin/ios/lynx/ui/text/LynxTextView.m:205-235`,
//! the layout manager's enclosing rect per line fragment).
//!
//! The inverse query, a point to the source unit drawn under it, walks the
//! same clusters with the same exclusions, so the two cannot disagree about
//! which unit a pixel belongs to.

use core::ops::Range;

use super::{LineInfo, TextBlock};
use crate::geometry::{Point, Rect};
use crate::text::paragraph::query::Displayed;

impl TextBlock {
    #[must_use]
    pub fn range_boxes(&self, start: u32, end: u32) -> Option<Vec<Rect<f32>>> {
        self.paragraph.range_boxes_in(&self.displayed(), start, end)
    }

    #[must_use]
    pub fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        self.paragraph.index_at_point_in(&self.displayed(), point)
    }

    #[must_use]
    pub fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        self.paragraph.item_source_range(item)
    }

    fn displayed(&self) -> Displayed<'_, LineInfo> {
        let block = &self.paragraph.content.block;
        let (cut_byte, content_slots) = self
            .expect_result()
            .display
            .as_ref()
            .map_or((block.text.len(), block.boxes.len()), |part| {
                (part.cut_byte as usize, part.content_slots)
            });
        Displayed {
            layout: self.display(),
            lines: self.lines(),
            cut_byte,
            content_slots,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::float_cmp)]

    use core::num::NonZeroU32;

    use super::super::tests::{ahem_style, text_context};
    use super::super::{
        BlockConstraint, BlockStyle, InlineBoxSpec, InlineItem, RunStyle, TextBlock, TextOverflow,
        TextRunItem, TextSecurity, VerticalAlign, WhiteSpaceCollapse,
    };
    use crate::geometry::{Point, Rect, Size};

    fn run<'a>(text: &'a str, style: &'a RunStyle) -> InlineItem<'a> {
        InlineItem::Run(TextRunItem {
            text,
            style,
            white_space: WhiteSpaceCollapse::Collapse,
            text_security: TextSecurity::None,
        })
    }

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

    fn at(block: &TextBlock, x: f32, y: f32) -> Option<u32> {
        block.index_at_point(Point::new(x, y))
    }

    #[test]
    fn a_point_answers_the_cluster_under_it_on_its_line() {
        let style = RunStyle {
            line_height: super::super::LineHeight::Px(16.0),
            ..ahem_style()
        };
        let block = block(&[run("abc def", &style)], BlockStyle::default(), Some(35.0));
        assert_eq!(block.lines().len(), 2);
        assert_eq!(at(&block, 0.0, 0.0), Some(0), "the first glyph's corner");
        assert_eq!(at(&block, 29.9, 8.0), Some(2), "the line's last glyph");
        assert_eq!(
            at(&block, 5.0, 15.9),
            Some(0),
            "the half-leading under a glyph is its line's"
        );
        assert_eq!(at(&block, 5.0, 16.0), Some(4), "a line owns its top edge");
        assert_eq!(
            at(&block, 25.0, 20.0),
            Some(6),
            "the second line's last glyph"
        );
        assert_eq!(
            at(&block, 32.0, 8.0),
            None,
            "the space hanging at the soft wrap is no line's content"
        );
        assert_eq!(at(&block, 30.0, 20.0), None, "past the line's end");
        assert_eq!(at(&block, 5.0, -0.1), None, "above the first line");
        assert_eq!(at(&block, 5.0, 32.0), None, "below the last line");
        assert_eq!(at(&block, -0.1, 8.0), None, "before the line's start");
    }

    #[test]
    fn an_aligned_line_misses_its_start_and_its_kept_spaces_hit() {
        let style = ahem_style();
        let block = block(
            &[run("a  b", &style)],
            BlockStyle {
                text_align: super::super::TextAlign::Center,
                ..BlockStyle::default()
            },
            Some(100.0),
        );
        assert_eq!(at(&block, 29.9, 5.0), None);
        assert_eq!(at(&block, 30.0, 5.0), Some(0));
        assert_eq!(at(&block, 45.0, 5.0), Some(1));
        assert_eq!(at(&block, 55.0, 5.0), Some(2));
        assert_eq!(at(&block, 65.0, 5.0), Some(3));
        assert_eq!(at(&block, 75.0, 5.0), None);
    }

    #[test]
    fn a_point_on_an_atomic_box_answers_its_unit() {
        let style = ahem_style();
        let items = [
            run("ab", &style),
            InlineItem::Box(InlineBoxSpec {
                id: 7,
                size: Size::new(20.0, 30.0),
                baseline: None,
                vertical_align: VerticalAlign::Baseline,
            }),
            run("cd", &style),
        ];
        let block = block(&items, BlockStyle::default(), None);
        let line = block.lines()[0];
        let middle = line.top + line.height / 2.0;
        assert_eq!(at(&block, 19.9, middle), Some(1));
        assert_eq!(at(&block, 20.0, middle), Some(2), "the box's left edge");
        assert_eq!(
            at(&block, 39.9, line.top),
            Some(2),
            "the box's line, top to bottom"
        );
        assert_eq!(at(&block, 40.0, middle), Some(3));
    }

    #[test]
    fn what_a_truncation_draws_is_no_hit() {
        let style = ahem_style();
        let block_style = BlockStyle {
            max_lines: NonZeroU32::new(1),
            overflow: TextOverflow::Ellipsis,
            ..BlockStyle::default()
        };
        let block = block(&[run("aaaa bbbb", &style)], block_style, Some(45.0));
        assert!(block.truncated());
        assert_eq!(at(&block, 5.0, 5.0), Some(0));
        assert_eq!(at(&block, 15.0, 5.0), None, "the dots");
        assert_eq!(at(&block, 5.0, 15.0), None, "the line never shown");
    }

    #[test]
    fn an_item_answers_its_pre_collapse_source_units() {
        let style = ahem_style();
        let items = [
            run("a \n", &style),
            InlineItem::Box(InlineBoxSpec {
                id: 7,
                size: Size::new(20.0, 10.0),
                baseline: None,
                vertical_align: VerticalAlign::Baseline,
            }),
            run("bc", &style),
        ];
        let block = block(&items, BlockStyle::default(), Some(15.0));
        assert_eq!(block.item_source_range(0), Some(0..3));
        assert_eq!(block.item_source_range(1), Some(3..4));
        assert_eq!(block.item_source_range(2), Some(4..6));
        assert_eq!(block.item_source_range(3), None);
    }
}
