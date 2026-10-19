//! Inline box fragments: one per line a non-atomic inline box is on.
//!
//! A fragment's inline extent is everything of the box on its line — the
//! glyph runs of its content and of every box nested in it, the edge
//! clusters of nested boxes, and its own border and padding where its edges
//! are on that line — and its block extent the box's content
//! area: its first available font's ascent and descent around the line's
//! baseline (CSS 2.2 §10.6.1; vertical alignment of a non-atomic box is not
//! applied, so that baseline is the line's), grown by its vertical padding
//! and border. An inline box's margin is outside its border box and inside
//! its parent's, which is where it is counted.
//!
//! The walk is per glyph run, not per cluster: parley splits a
//! glyph run at every style change, every run is one item's, and every item
//! is in one innermost box. Collapsible whitespace hanging at the line's end
//! edge is no box's content (css-text-3 §4.1.3), so content extents are cut
//! at the line's hanging zone.

use parley::PositionedLayoutItem;

use super::edge::{EdgeRun, EdgeSide, SpanAt};
use super::{FlowSource, InlineFlow, InlineFragment};
use crate::geometry::{Point, Rect, Size};

/// Each box's inline extent on one line, if it has any.
struct Extents<'a> {
    spans: &'a [SpanAt],
    extents: Vec<Option<(f32, f32)>>,
}

impl Extents<'_> {
    /// Adds `[left, right)` to box `span` and every box it is nested in.
    fn add(&mut self, mut span: Option<u32>, left: f32, right: f32) {
        if right <= left {
            return;
        }
        while let Some(index) = span {
            let entry = &mut self.extents[index as usize];
            *entry =
                Some(entry.map_or((left, right), |(min, max)| (min.min(left), max.max(right))));
            span = self.spans[index as usize].parent;
        }
    }
}

impl InlineFlow {
    pub(super) fn compute_fragments(&self) -> Vec<InlineFragment> {
        let boxes = &self.boxes;
        if boxes.spans.is_empty() {
            return Vec::new();
        }
        let result = self.expect_result();
        let mut extents = Extents {
            spans: &boxes.spans,
            extents: vec![None; boxes.spans.len()],
        };
        // Per box, its fragments in line order.
        let mut by_span: Vec<Vec<InlineFragment>> = vec![Vec::new(); boxes.spans.len()];

        for (index, (line, geometry)) in self
            .paragraph
            .natural
            .lines()
            .zip(&result.lines)
            .enumerate()
        {
            extents.extents.fill(None);
            self.line_extents(&line, &mut extents);
            let line_index = u32::try_from(index).expect("line count fits u32");
            for (span, extent) in extents.extents.iter().enumerate() {
                let Some((left, right)) = *extent else {
                    continue;
                };
                let (_, _, edges) =
                    boxes.span_spec(u32::try_from(span).expect("box count fits u32"));
                let (ascent, descent) = boxes.areas[span];
                let top = geometry.baseline - ascent - edges.top_inset;
                let bottom = geometry.baseline + descent + edges.bottom_inset;
                by_span[span].push(InlineFragment {
                    item: boxes.spans[span].item,
                    line: line_index,
                    rect: Rect::new(Point::new(left, top), Size::new(right - left, bottom - top)),
                    first: false,
                    last: false,
                });
            }
        }

        let mut fragments = Vec::new();
        for mut list in by_span {
            if let Some(first) = list.first_mut() {
                first.first = true;
            }
            if let Some(last) = list.last_mut() {
                last.last = true;
            }
            fragments.append(&mut list);
        }
        fragments
    }

    /// Every box's inline extent on `line`.
    fn line_extents(
        &self,
        line: &parley::Line<'_, crate::style::TextBrush>,
        extents: &mut Extents<'_>,
    ) {
        let boxes = &self.boxes;
        // The hanging zone sits at the line's end edge, which is the
        // paragraph direction's.
        let mut low = f32::INFINITY;
        let mut high = f32::NEG_INFINITY;
        for item in line.items() {
            let (start, advance) = match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => {
                    (glyph_run.offset(), glyph_run.advance())
                }
                PositionedLayoutItem::InlineBox(inline_box) => (inline_box.x, inline_box.width),
            };
            low = low.min(start);
            high = high.max(start + advance);
        }
        let hanging = line.metrics().hanging_advance;
        let (visible_low, visible_high) = if self.paragraph.natural.is_rtl() {
            (low + hanging, high)
        } else {
            (low, high - hanging)
        };

        for item in line.items() {
            match item {
                PositionedLayoutItem::GlyphRun(glyph_run) => {
                    let left = glyph_run.offset();
                    let right = left + glyph_run.advance();
                    match self.source_at(glyph_run.style_index()) {
                        FlowSource::Strut => {}
                        FlowSource::Run(item) => extents.add(
                            boxes.item_span[item as usize],
                            left.max(visible_low),
                            right.min(visible_high),
                        ),
                        FlowSource::Edge(_) => {
                            let start = u32::try_from(glyph_run.run().text_range().start)
                                .expect("text fits u32");
                            if let Ok(edge) = boxes
                                .edges
                                .binary_search_by_key(&start, |edge| edge.bytes.start)
                            {
                                self.add_edge(
                                    extents,
                                    &boxes.edges[edge],
                                    left,
                                    right,
                                    glyph_run.run().is_rtl(),
                                );
                            }
                        }
                    }
                }
                // The flow holds no atomic box.
                PositionedLayoutItem::InlineBox(_) => {}
            }
        }
    }

    /// Lays the edges `edge` carries across `[left, right)`, in logical
    /// order from the side the cluster's run starts on: a start edge is its
    /// margin then its border and padding, an end edge the other way round.
    fn add_edge(
        &self,
        extents: &mut Extents<'_>,
        edge: &EdgeRun,
        left: f32,
        right: f32,
        rtl: bool,
    ) {
        let boxes = &self.boxes;
        let spans = &boxes.spans;
        let mut cursor = if rtl { right } else { left };
        for &(span, side) in &edge.members {
            let (_, _, edges) = boxes.span_spec(span);
            let parent = spans[span as usize].parent;
            let pieces = match side {
                EdgeSide::Start => [
                    (edges.start_margin, parent),
                    (edges.start_inset, Some(span)),
                ],
                EdgeSide::End => [(edges.end_inset, Some(span)), (edges.end_margin, parent)],
            };
            for (width, owner) in pieces {
                let (from, to) = if rtl {
                    (cursor - width, cursor)
                } else {
                    (cursor, cursor + width)
                };
                cursor = if rtl { from } else { to };
                extents.add(owner, from.min(to), from.max(to));
            }
        }
    }
}
