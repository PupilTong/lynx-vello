//! The inline formatting context of a CSS block container.
//!
//! A block container (CSS 2.2 §9.2.1, css-display-3 `block flow`) lays its
//! inline-level content out in line boxes (CSS 2.2 §9.4.2). That content is
//! text runs, non-atomic inline boxes around them (§9.2.2), and atomic inline
//! boxes, so the paragraph is the core's ([`crate::text::paragraph`]) with CSS
//! semantics on top, and nothing the Lynx `<text>` block adds is
//! representable: no line or length limit, no truncation, no custom
//! truncation content.
//!
//! What this module does that the core leaves to its consumer:
//!
//! - **White space** is css-text-3 §4.1 in full, including §4.1.2 rule 1 at the start of the
//!   paragraph: a collapsible sequence there is removed. It collapses across inline box boundaries
//!   as across none (§4.1.1).
//! - **Line breaking** reads each run's own `text-wrap-mode`, `word-break` and `overflow-wrap`.
//!   Parley takes its line-break override once per paragraph, so a paragraph with any `break-all`
//!   run is analysed without it (`docs/tracking/deviations.md`).
//! - **`text-indent`** honours its `each-line` and `hanging` keywords and counts toward the
//!   min-content and max-content widths.
//! - **Line boxes** follow CSS 2.2 §10.8.1, as parley 0.13 builds them: every line starts with the
//!   root inline box's strut — the container's first available font at its `line-height`, parley's
//!   style 0 — and every run on it adds its own half-leading box around the shared baseline; with
//!   `line-height: normal` a run shaped in a fallback font adds that font's box too.
//! - **Inline boxes** ([`InlineFlowItem::Open`]) are the full inline box model of CSS 2.2 §10.3.1
//!   and §10.6.1 with `box-decoration-break: slice`: the inline-start margin, border and padding
//!   are advance on the line before the box's first content and the inline-end ones after its last,
//!   never a soft wrap opportunity of their own (`edge`); vertical margins do nothing, and vertical
//!   padding and border are painted without changing the line box. A box with no content still
//!   takes part in the line when an inline-axis margin, border or padding of it is nonzero
//!   (§9.4.2). Each box reports one fragment per line it is on ([`InlineFragment`]): its content
//!   area — its own font's ascent and descent — grown by its vertical padding and border, from its
//!   first to its last content on that line plus the edges it owns there.
//! - **Atomic inline boxes** ([`InlineFlowItem::Atom`]) are placed by their margin box and
//!   `vertical-align` as the core places a Lynx atom (`crate::text::paragraph::position`); the host
//!   lays each out by its own display and writes the size in ([`InlineFlow::set_atom_size`]). The
//!   size may move with the width the host lays a line out at — CSS shrinks an auto-width atom to
//!   fit it — so a commit records the sizes it used and [`InlineFlow::restore_committed`] writes
//!   them back.
//! - **Direction** is the container's `direction`, handed to parley as the bidi paragraph level
//!   (UAX #9 P3 overridden, as css-writing-modes-3 §2.1 takes it from the block): `text-align:
//!   start` and the side trailing whitespace hangs on follow it, and a mixed line orders its runs
//!   by it.
//! - **Sizing** reports the CSS intrinsic widths, and for a definite available width the
//!   fit-content size `min(max-content, max(min-content, available))` (css-sizing-3) rather than
//!   the widest broken line.
//!
//! `text-overflow` is not applied: it takes effect only on a block whose
//! `overflow` is not `visible` (css-overflow-3 `text-overflow`), and the
//! producers of this context — the anonymous block box around loose text,
//! and the block containers of HTML content the engine builds — compute
//! `overflow: visible` or are unreachable by author CSS. A block container
//! that clipped would apply it per overflowing line box, after the break.
//!
//! Not implemented, recorded in `docs/tracking/deviations.md`: block-in-inline
//! splitting (a block-level box among inline content is an atomic inline
//! here), `vertical-align` on a non-atomic inline box, and the line-height
//! of an inline box whose own font has no text on a line.

mod boxes;
mod edge;
mod fragment;

use core::fmt;
use core::ops::Range;
use std::sync::OnceLock;

pub use boxes::InlineEdges;
use boxes::{BoxSpec, InlineBoxes};
use edge::EdgeAdditions;
use parley::{Alignment, BaseDirection, Layout, PositionedLayoutItem};

use crate::geometry::{Point, Rect, Size};
use crate::style::{TextBrush, TextContainerStyle, TextRunStyle};
use crate::text::TextContext;
use crate::text::block::PlacedBox;
use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::memo::{BreakMemo, Retained};
use crate::text::paragraph::query::Displayed;
use crate::text::paragraph::shape::{self, NaturalLine, Objects};
use crate::text::paragraph::style::resolve_alignment;
use crate::text::paragraph::{
    Additions, BlockConstraint, BlockMetrics, BreakStyle, Direction, Flow, IndentKeywords,
    InlineBoxSpec, InlineItem, LineGeometry, OffsetMap, OverflowWrap, OwnedItem, Paragraph,
    ParagraphRoot, RunBreaking, RunStyle, TextAlign, TextRunItem, TextSecurity, VerticalAlign,
    WhiteSpaceCollapse,
};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InlineFlowStyle {
    pub text_align: TextAlign,
    pub direction: Direction,
    pub strut: RunStyle,
    pub indent: IndentKeywords,
    pub strut_line_when_empty: bool,
}

impl InlineFlowStyle {
    #[must_use]
    pub fn from_container_style<C: TextContainerStyle, R: TextRunStyle>(
        container: &C,
        own_font: &R,
    ) -> Self {
        let indent = container.text_indent();
        Self {
            text_align: container.text_align().into(),
            direction: container.direction().into(),
            strut: RunStyle::from_run_style(own_font),
            indent: IndentKeywords {
                each_line: indent.each_line,
                hanging: indent.hanging,
            },
            strut_line_when_empty: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InlineRun<'src> {
    pub text: &'src str,
    pub style: &'src RunStyle,
    pub white_space: WhiteSpaceCollapse,
    pub text_security: TextSecurity,
    pub breaking: BreakStyle,
}

#[derive(Clone, Copy, Debug)]
pub enum InlineFlowItem<'src> {
    Run(InlineRun<'src>),
    Atom(InlineAtom),
    Open(InlineBoxOpen<'src>),
    Close,
}

impl<'src> From<InlineRun<'src>> for InlineFlowItem<'src> {
    fn from(run: InlineRun<'src>) -> Self {
        Self::Run(run)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineAtom {
    pub vertical_align: VerticalAlign,
}

#[derive(Clone, Copy, Debug)]
pub struct InlineBoxOpen<'src> {
    pub style: &'src RunStyle,
    pub breaking: BreakStyle,
    pub direction: Direction,
    pub edges: InlineEdges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowSource {
    Strut,
    Run(u32),
    Edge(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineFragment {
    pub item: u32,
    pub line: u32,
    pub rect: Rect<f32>,
    pub first: bool,
    pub last: bool,
}

struct FlowResult {
    constraint: BlockConstraint,
    natural: Vec<NaturalLine>,
    metrics: BlockMetrics,
    atoms: Vec<PlacedBox>,
    fragments: OnceLock<Vec<InlineFragment>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Intrinsic {
    indent: f32,
    min: f32,
    max: f32,
}

pub struct InlineFlow {
    style: InlineFlowStyle,
    paragraph: Paragraph<Option<u32>, EdgeAdditions>,
    breaking: Vec<BreakStyle>,
    boxes: InlineBoxes,
    result: Option<FlowResult>,
    memo: BreakMemo,
    intrinsic: Option<Intrinsic>,
    one_pass: bool,
    excess_lines: bool,
    committed_atoms: Vec<(Size<f32>, Option<f32>)>,
}

impl fmt::Debug for InlineFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InlineFlow").finish_non_exhaustive()
    }
}

impl InlineFlow {
    #[must_use]
    pub fn new(
        context: &mut TextContext,
        style: InlineFlowStyle,
        items: &[InlineFlowItem<'_>],
    ) -> Self {
        let breaking: Vec<BreakStyle> = items
            .iter()
            .map(|item| match item {
                InlineFlowItem::Run(run) => run.breaking,
                InlineFlowItem::Open(open) => open.breaking,
                InlineFlowItem::Atom(_) | InlineFlowItem::Close => BreakStyle::default(),
            })
            .collect();
        let (flow, mut boxes) = lay_in(items);
        let root = ParagraphRoot {
            strut: Some((style.strut.clone(), None)),
            direction: match style.direction {
                Direction::Ltr => BaseDirection::Ltr,
                Direction::Rtl => BaseDirection::Rtl,
            },
            strut_line_when_empty: style.strut_line_when_empty,
        };
        let paragraph = Paragraph::from_flow(
            context,
            RunBreaking::PerItem(&breaking),
            WhiteSpaceProcessing::Css,
            flow,
            Some,
            root,
        );
        boxes.areas = boxes::content_areas(context, &boxes.specs);
        let one_pass = one_pass_content(&paragraph.content, &breaking, style.direction);
        Self {
            style,
            paragraph,
            breaking,
            boxes,
            result: None,
            memo: BreakMemo::new(),
            intrinsic: None,
            one_pass,
            excess_lines: false,
            committed_atoms: Vec::new(),
        }
    }

    pub fn set_style(&mut self, style: InlineFlowStyle) {
        if self.style == style {
            return;
        }
        debug_assert!(
            self.style.strut == style.strut
                && self.style.direction == style.direction
                && self.style.strut_line_when_empty == style.strut_line_when_empty,
            "a strut, direction or empty-line change needs a new flow"
        );
        self.style = style;
        self.memo.forget();
        self.result = None;
        self.intrinsic = None;
    }

    #[must_use]
    pub fn shaped_with(&self, style: &InlineFlowStyle, items: &[InlineFlowItem<'_>]) -> bool {
        if self.style.strut != style.strut
            || self.style.direction != style.direction
            || self.style.strut_line_when_empty != style.strut_line_when_empty
            || items.len() != self.breaking.len()
        {
            return false;
        }
        let built = &self.paragraph.content.items;
        let mut specs = self.boxes.specs.iter();
        for (index, item) in items.iter().enumerate() {
            let breaking = self.breaking[index];
            let same = match (item, &built[index]) {
                (InlineFlowItem::Run(run), OwnedItem::Run { style, .. }) => {
                    run.breaking == breaking && style == run.style
                }
                (InlineFlowItem::Atom(atom), OwnedItem::Box(spec)) => {
                    spec.vertical_align == atom.vertical_align
                }
                (InlineFlowItem::Open(open), OwnedItem::Marker) => {
                    specs.next().is_some_and(|spec| {
                        open.breaking == breaking
                            && spec.style == *open.style
                            && spec.direction == open.direction
                            && spec.edges == open.edges
                    })
                }
                (InlineFlowItem::Close, OwnedItem::Marker) => true,
                _ => false,
            };
            if !same {
                return false;
            }
        }
        specs.next().is_none()
    }

    pub fn probe(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::probe(self, context, constraint)
    }

    pub fn commit(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        let metrics = Retained::commit(self, context, constraint);
        self.committed_atoms = self.atom_sizes().collect();
        metrics
    }

    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.memo.committed().is_some()
    }

    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        Retained::is_probe_dirty(self)
    }

    pub fn restore_committed(&mut self, context: &mut TextContext) -> bool {
        if self.has_committed() && !self.atom_sizes().eq(self.committed_atoms.iter().copied()) {
            let committed = core::mem::take(&mut self.committed_atoms);
            let atoms = self
                .paragraph
                .content
                .items
                .iter_mut()
                .filter_map(|item| match item {
                    OwnedItem::Box(spec) => Some(spec),
                    _ => None,
                });
            for (spec, &(size, baseline)) in atoms.zip(&committed) {
                spec.size = size;
                spec.baseline = baseline;
            }
            self.committed_atoms = committed;
            self.memo.mark_boxes_dirty();
            self.memo.forget();
            self.intrinsic = None;
        }
        Retained::restore_committed(self, context)
    }

    fn atom_sizes(&self) -> impl Iterator<Item = (Size<f32>, Option<f32>)> + '_ {
        self.paragraph
            .content
            .items
            .iter()
            .filter_map(|item| match item {
                OwnedItem::Box(spec) => Some((spec.size, spec.baseline)),
                _ => None,
            })
    }

    pub fn min_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).min
    }

    pub fn max_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).max
    }

    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.memo.breaks()
    }

    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        self.expect_result();
        &self.paragraph.natural
    }

    #[must_use]
    pub fn offset_map(&self) -> OffsetMap<'_> {
        self.expect_result();
        let block = &self.paragraph.content.block;
        OffsetMap::new(
            &block.text,
            &block.map,
            block.text.len(),
            &block.boxes,
            block.boxes.len(),
        )
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.expect_result().natural.len()
    }

    #[must_use]
    pub fn line_geometry(&self) -> Vec<LineGeometry> {
        self.paragraph
            .natural
            .lines()
            .take(self.line_count())
            .map(|line| LineGeometry::of(&line))
            .collect()
    }

    #[must_use]
    pub fn size(&self) -> Size<f32> {
        self.expect_result().metrics.size
    }

    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.first_baseline
    }

    #[must_use]
    pub fn last_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.last_baseline
    }

    #[must_use]
    pub fn style_count(&self) -> usize {
        self.expect_result();
        self.paragraph.natural_sources.len()
    }

    #[must_use]
    pub fn source_of(&self, style_index: u16) -> FlowSource {
        self.expect_result();
        self.source_at(style_index)
    }

    fn source_at(&self, style_index: u16) -> FlowSource {
        match self.paragraph.natural_sources[usize::from(style_index)] {
            None => FlowSource::Strut,
            Some(item) => match self.paragraph.content.items[item as usize] {
                OwnedItem::Marker => FlowSource::Edge(item),
                _ => FlowSource::Run(item),
            },
        }
    }

    pub fn set_atom_size(&mut self, item: u32, size: Size<f32>, baseline: Option<f32>) {
        let Some(OwnedItem::Box(spec)) = self.paragraph.content.items.get_mut(item as usize) else {
            panic!("set_atom_size addresses an atom this flow was built with");
        };
        if spec.size == size && spec.baseline == baseline {
            return;
        }
        spec.size = size;
        spec.baseline = baseline;
        self.memo.mark_boxes_dirty();
        self.memo.forget();
        self.intrinsic = None;
    }

    #[must_use]
    pub fn atoms(&self) -> &[PlacedBox] {
        &self.expect_result().atoms
    }

    #[must_use]
    pub fn static_position(&self, item: u32) -> Point<f32> {
        let result = self.expect_result();
        let content = &self.paragraph.content;
        let byte = self.item_start_byte(item);
        let Some(last) = result.natural.len().checked_sub(1) else {
            return Point::ZERO;
        };
        let index = result
            .natural
            .iter()
            .position(|line| {
                byte < line.end_byte
                    || (line.start_byte == line.end_byte && line.start_byte == byte)
            })
            .unwrap_or(last);
        let line = self
            .paragraph
            .natural
            .lines()
            .nth(index)
            .expect("a natural line per reported line");
        let rtl_base = self.style.direction == Direction::Rtl;
        let mut atom_at: Option<(u32, f32)> = None;
        let mut cluster_at = None;
        let mut before: Option<((u32, u8, u32), f32)> = None;
        let mut note_before = |key: (u32, u8, u32), edge: f32| {
            if before.is_none_or(|(seen, _)| key >= seen) {
                before = Some((key, edge));
            }
        };
        let objects = Objects::of(&self.paragraph.natural);
        let mut walked_run = None;
        for entry in line.items() {
            match entry {
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
                        let start = objects.text_byte(bytes.start);
                        let end = objects.text_byte(bytes.end);
                        let (leading, trailing) = if cluster.is_rtl() {
                            (x + advance, x)
                        } else {
                            (x, x + advance)
                        };
                        if cluster_at.is_none() && start <= byte && byte < end {
                            cluster_at = Some(leading);
                        }
                        if end <= byte {
                            note_before((end, 0, 0), trailing);
                        }
                        x += advance;
                    }
                }
                PositionedLayoutItem::InlineBox(inline_box) => {
                    let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
                    let entry = content.block.boxes[slot];
                    let (left, right) = (inline_box.x, inline_box.x + inline_box.width);
                    let (leading, trailing) = if rtl_base {
                        (right, left)
                    } else {
                        (left, right)
                    };
                    if entry.byte < byte || (entry.byte == byte && entry.item < item) {
                        note_before((entry.byte, 1, entry.item), trailing);
                    } else if entry.byte == byte
                        && atom_at.is_none_or(|(first, _)| entry.item < first)
                    {
                        atom_at = Some((entry.item, leading));
                    }
                }
            }
        }
        let x = atom_at
            .map(|(_, edge)| edge)
            .or(cluster_at)
            .or(before.map(|(_, edge)| edge))
            .unwrap_or_else(|| line.metrics().offset);
        Point::new(x, LineGeometry::of(&line).top)
    }

    #[must_use]
    pub fn fragments(&self) -> &[InlineFragment] {
        let result = self.expect_result();
        result.fragments.get_or_init(|| self.compute_fragments())
    }

    #[must_use]
    pub fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        if item as usize >= self.breaking.len() {
            return None;
        }
        self.paragraph.item_source_range(item)
    }

    fn item_start_byte(&self, item: u32) -> u32 {
        let block = &self.paragraph.content.block;
        let boxes = &self.boxes;
        let count = u32::try_from(self.breaking.len()).expect("item count fits u32");
        for index in item..count {
            let place =
                match &self.paragraph.content.items[index as usize] {
                    OwnedItem::Run { .. } => block
                        .ranges
                        .iter()
                        .find(|range| range.item == index)
                        .map(|range| range.bytes.start),
                    OwnedItem::Box(_) => block
                        .boxes
                        .iter()
                        .find(|entry| entry.item == index)
                        .map(|entry| entry.byte),
                    OwnedItem::Marker => boxes
                        .item_span
                        .get(index as usize)
                        .copied()
                        .flatten()
                        .map(|span| {
                            let at = &boxes.spans[span as usize];
                            if at.item == index {
                                at.bytes.start
                            } else {
                                boxes
                                    .edges
                                    .iter()
                                    .find(|edge| {
                                        edge.members.contains(&(span, edge::EdgeSide::End))
                                    })
                                    .map_or(at.bytes.end, |edge| edge.bytes.start)
                            }
                        }),
                };
            if let Some(byte) = place {
                return byte;
            }
        }
        u32::try_from(block.text.len()).expect("text fits u32")
    }

    #[must_use]
    pub fn range_boxes(&self, start: u32, end: u32) -> Option<Vec<Rect<f32>>> {
        self.paragraph.range_boxes_in(&self.displayed(), start, end)
    }

    #[must_use]
    pub fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        self.paragraph.index_at_point_in(&self.displayed(), point)
    }

    fn displayed(&self) -> Displayed<'_, NaturalLine> {
        let block = &self.paragraph.content.block;
        Displayed {
            layout: &self.paragraph.natural,
            lines: &self.expect_result().natural,
            cut_byte: block.text.len(),
            content_slots: block.boxes.len(),
        }
    }

    fn place_atoms(&self, line_count: usize) -> Vec<PlacedBox> {
        let content = &self.paragraph.content;
        if content.block.boxes.is_empty() {
            return Vec::new();
        }
        let mut atoms = Vec::with_capacity(content.block.boxes.len());
        let mut line_boxes = shape::line_boxes(&self.paragraph.natural);
        for (index, line) in self.paragraph.natural.lines().take(line_count).enumerate() {
            let count = line_boxes.on(&line).len();
            if count == 0 {
                continue;
            }
            let placed = line.items().filter_map(|item| match item {
                PositionedLayoutItem::InlineBox(inline_box) => Some(inline_box),
                PositionedLayoutItem::GlyphRun(_) => None,
            });
            for inline_box in placed.take(count) {
                let slot = usize::try_from(inline_box.id).expect("slot ids are table indexes");
                let spec = content.box_spec(content.block.boxes[slot].item);
                let (before, _) = content.additions.box_advance(slot);
                atoms.push(PlacedBox::Visible {
                    id: spec.id,
                    line: u32::try_from(index).expect("line count fits u32"),
                    origin: Point::new(inline_box.x + before, inline_box.y),
                    size: spec.size,
                });
            }
        }
        atoms
    }

    fn expect_result(&self) -> &FlowResult {
        self.result
            .as_ref()
            .expect("an InlineFlow is read after a layout")
    }

    fn break_at(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> Vec<NaturalLine> {
        self.memo.begin_break();
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.paragraph.break_lines(
            context,
            RunBreaking::PerItem(&self.breaking),
            constraint,
            None,
            self.style.indent,
            alignment,
        )
    }

    fn intrinsic(&mut self, context: &mut TextContext, indent: f32) -> Intrinsic {
        if let Some(intrinsic) = self.intrinsic
            && intrinsic.indent.to_bits() == indent.to_bits()
        {
            return intrinsic;
        }
        if self.paragraph.is_empty_strut_line() {
            let intrinsic = Intrinsic {
                indent,
                min: 0.0,
                max: 0.0,
            };
            self.intrinsic = Some(intrinsic);
            return intrinsic;
        }
        let one_pass =
            self.one_pass && indent == 0.0 && self.paragraph.box_widths().all(|width| width >= 0.0);
        let (min, max) = if one_pass {
            let widths = self.paragraph.content_widths_at(indent, self.style.indent);
            #[cfg(debug_assertions)]
            {
                let (min, max) = self.break_widths(context, indent, false);
                assert_eq!(
                    (widths.min.to_bits(), widths.max.to_bits()),
                    (min.to_bits(), max.to_bits()),
                    "parley's one-pass content widths are the two breaks' here",
                );
            }
            (widths.min, widths.max)
        } else {
            self.excess_lines = true;
            self.break_widths(context, indent, true)
        };
        let intrinsic = Intrinsic { indent, min, max };
        self.result = None;
        self.intrinsic = Some(intrinsic);
        intrinsic
    }

    fn break_widths(
        &mut self,
        context: &mut TextContext,
        indent: f32,
        counted: bool,
    ) -> (f32, f32) {
        let breaking = RunBreaking::PerItem(&self.breaking);
        let keywords = self.style.indent;
        let mut break_at = |paragraph: &mut Paragraph<Option<u32>, EdgeAdditions>, width| {
            if counted {
                self.memo.begin_break();
            }
            paragraph.break_lines(
                context,
                breaking,
                BlockConstraint::new(width, indent),
                None,
                keywords,
                Alignment::Start,
            );
        };
        break_at(&mut self.paragraph, None);
        let unconstrained = self.paragraph.natural.width();
        break_at(&mut self.paragraph, Some(0.0));
        let min = self
            .paragraph
            .content_widths()
            .min
            .max(self.paragraph.natural.width());
        (min, unconstrained.max(min))
    }
}

fn lay_in(items: &[InlineFlowItem<'_>]) -> (Flow<EdgeAdditions>, InlineBoxes) {
    let mut specs = Vec::new();
    let mut open = 0_usize;
    let mut core: Vec<InlineItem<'_>> = items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            InlineFlowItem::Run(run) => InlineItem::Run(TextRunItem {
                text: run.text,
                style: run.style,
                white_space: run.white_space,
                text_security: run.text_security,
            }),
            InlineFlowItem::Atom(atom) => InlineItem::Box(InlineBoxSpec {
                id: index as u64,
                size: Size::ZERO,
                baseline: None,
                vertical_align: atom.vertical_align,
            }),
            InlineFlowItem::Open(spec) => {
                specs.push(BoxSpec::new(spec.style, spec.direction, spec.edges));
                open += 1;
                InlineItem::Marker
            }
            InlineFlowItem::Close => {
                open = open.saturating_sub(1);
                InlineItem::Marker
            }
        })
        .collect();
    core.extend(core::iter::repeat_n(InlineItem::Marker, open));
    let (mut flow, places) = Flow::build(&core, WhiteSpaceProcessing::Css);
    let (boxes, additions) = edge::place(&mut flow.block, places, items, specs);
    flow.additions = additions;
    (flow, boxes)
}

fn one_pass_content(
    flow: &Flow<EdgeAdditions>,
    breaking: &[BreakStyle],
    direction: Direction,
) -> bool {
    use icu_properties::CodePointMapData;
    use icu_properties::props::BidiClass;

    let bidi = CodePointMapData::<BidiClass>::new();
    direction == Direction::Ltr
        && flow.block.tabs.is_empty()
        && breaking
            .iter()
            .all(|breaking| breaking.overflow_wrap != OverflowWrap::Anywhere)
        && flow.items.iter().all(|item| match item {
            OwnedItem::Run { style, .. } => style.letter_spacing >= 0.0,
            OwnedItem::Box(_) | OwnedItem::Marker => true,
        })
        && flow
            .additions
            .inserted()
            .iter()
            .all(|segment| segment.style.letter_spacing + segment.extra_spacing >= 0.0)
        && !flow.block.text.chars().any(|character| {
            matches!(
                bidi.get(character),
                BidiClass::RightToLeft
                    | BidiClass::ArabicLetter
                    | BidiClass::ArabicNumber
                    | BidiClass::LeftToRightEmbedding
                    | BidiClass::LeftToRightOverride
                    | BidiClass::RightToLeftEmbedding
                    | BidiClass::RightToLeftOverride
                    | BidiClass::LeftToRightIsolate
                    | BidiClass::RightToLeftIsolate
                    | BidiClass::FirstStrongIsolate
            )
        })
}

impl Retained for InlineFlow {
    fn memo(&self) -> &BreakMemo {
        &self.memo
    }

    fn memo_mut(&mut self) -> &mut BreakMemo {
        &mut self.memo
    }

    fn retained(&self) -> Option<BlockConstraint> {
        self.result.as_ref().map(|result| result.constraint)
    }

    fn layout_at(&mut self, context: &mut TextContext, constraint: BlockConstraint) {
        let left =
            resolve_alignment(self.style.text_align, self.style.direction) == Alignment::Left;
        if self.memo.holds(
            self.retained(),
            self.fits_from().filter(|_| left),
            constraint,
        ) {
            if let Some(result) = &mut self.result {
                result.constraint = constraint;
            }
            return;
        }
        let intrinsic = self.intrinsic(context, constraint.indent);
        let natural = self.break_at(context, constraint);
        if core::mem::take(&mut self.excess_lines) {
            self.paragraph.release_line_capacity();
        }
        let atoms = self.place_atoms(natural.len());
        let width = constraint.max_advance().map_or(intrinsic.max, |available| {
            intrinsic.max.min(intrinsic.min.max(available))
        });
        let mut lines = self
            .paragraph
            .natural
            .lines()
            .take(natural.len())
            .map(|line| LineGeometry::of(&line));
        let first = lines.next();
        let last = lines.next_back().or(first);
        let metrics = BlockMetrics {
            size: Size::new(width, last.map_or(0.0, |line| line.top + line.height)),
            first_baseline: first.map(|line| line.baseline),
            last_baseline: last.map(|line| line.baseline),
        };
        self.result = Some(FlowResult {
            constraint,
            natural,
            metrics,
            atoms,
            fragments: OnceLock::new(),
        });
    }

    fn reported(&self) -> BlockMetrics {
        self.expect_result().metrics
    }

    fn fits_from(&self) -> Option<f32> {
        let result = self.result.as_ref()?;
        result
            .constraint
            .max_advance()
            .is_none()
            .then(|| self.paragraph.natural.width())
    }
}
