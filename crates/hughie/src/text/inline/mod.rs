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
//! - **Line boxes** follow CSS 2.2 §10.8.1, as parley 0.12 builds them: every line starts with the
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
use parley::{AlignmentOptions, BaseDirection, Layout, PositionedLayoutItem};

use crate::geometry::{Point, Rect, Size};
use crate::style::{TextBrush, TextContainerStyle, TextRunStyle};
use crate::text::TextContext;
use crate::text::block::PlacedBox;
use crate::text::paragraph::content::WhiteSpaceProcessing;
use crate::text::paragraph::memo::{BreakMemo, Retained};
use crate::text::paragraph::query::{Displayed, LineSpan};
use crate::text::paragraph::shape::{self, NaturalLine};
use crate::text::paragraph::style::resolve_alignment;
use crate::text::paragraph::{
    Additions, BlockConstraint, BlockMetrics, BreakStyle, Direction, Flow, IndentKeywords,
    InlineBoxSpec, InlineItem, LineGeometry, OffsetMap, OverflowWrap, OwnedItem, Paragraph,
    ParagraphRoot, RunBreaking, RunStyle, TextAlign, TextRunItem, TextSecurity, VerticalAlign,
    WhiteSpaceCollapse,
};

/// The block container's own parameters: CSS properties only.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InlineFlowStyle {
    pub text_align: TextAlign,
    /// The paragraph's bidi base level, which also resolves `text-align:
    /// start`/`end`.
    pub direction: Direction,
    /// The block container's own font and `line-height`: the strut every
    /// line box starts with (CSS 2.2 §10.8.1). Its font fields and
    /// `line_height` are read; its `letter_spacing` is not.
    pub strut: RunStyle,
    /// The `text-indent` keywords. The indent's length is a
    /// [`BlockConstraint`] input, because a percentage resolves against the
    /// definite inline size only the caller knows.
    pub indent: IndentKeywords,
    /// Whether a block with no content — no text once white space is
    /// processed, no atomic box and no inline box edge — lays out one empty
    /// line of the strut rather than none: set for an editing host's
    /// paragraph, which is never shorter than one line, so that its caret
    /// has a line to sit on. A shaping input, like the strut.
    pub strut_line_when_empty: bool,
}

impl InlineFlowStyle {
    /// Builds the parameters from the block container's computed style:
    /// `container` for its text properties, `own_font` for the font and
    /// `line-height` of the strut — the same computed values, seen as a run.
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

/// One text run of the block's content, carrying the computed style of the
/// element its characters are in.
#[derive(Clone, Copy, Debug)]
pub struct InlineRun<'src> {
    pub text: &'src str,
    pub style: &'src RunStyle,
    /// The run's `white-space-collapse`.
    pub white_space: WhiteSpaceCollapse,
    /// The run's `-webkit-text-security` ([`TextRunItem::text_security`]).
    pub text_security: TextSecurity,
    /// The run's `word-break`, `text-wrap-mode` and `overflow-wrap`.
    pub breaking: BreakStyle,
}

/// One item of the block's inline content, in document order.
#[derive(Clone, Copy, Debug)]
pub enum InlineFlowItem<'src> {
    Run(InlineRun<'src>),
    /// An atomic inline box (CSS 2.2 §9.2.2): one unbreakable piece of the
    /// line, its margin box written in by [`InlineFlow::set_atom_size`].
    Atom(InlineAtom),
    /// The start of a non-atomic inline box: every item up to the matching
    /// [`Self::Close`] is its content.
    Open(InlineBoxOpen<'src>),
    /// The end of the innermost open inline box.
    Close,
}

impl<'src> From<InlineRun<'src>> for InlineFlowItem<'src> {
    fn from(run: InlineRun<'src>) -> Self {
        Self::Run(run)
    }
}

/// An atomic inline box, before the host has measured it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineAtom {
    /// A [`VerticalAlign::Percent`] here is of the root inline box's line
    /// height. CSS takes it of the atom's own `line-height` (CSS 2.2
    /// §10.8.1): a host resolves it into a [`VerticalAlign::Length`] with
    /// [`crate::text::paragraph::used_line_height`] first.
    pub vertical_align: VerticalAlign,
}

/// A non-atomic inline box's own parameters.
#[derive(Clone, Copy, Debug)]
pub struct InlineBoxOpen<'src> {
    /// The box's own font and `line-height`: the content area its fragments
    /// cover, and the font its edges are shaped in.
    pub style: &'src RunStyle,
    /// The box's own line-breaking values, which its edges are shaped with.
    pub breaking: BreakStyle,
    /// The box's `direction`, which says which physical side its start edge
    /// is on.
    pub direction: Direction,
    /// Its used margin, border and padding.
    pub edges: InlineEdges,
}

/// What a parley style index of [`InlineFlow::display`] was shaped from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowSource {
    /// The strut, which no text is shaped in. The one glyph run that
    /// carries it is the substitute space parley shapes for a block with no
    /// content that keeps one line
    /// ([`InlineFlowStyle::strut_line_when_empty`]), which draws nothing.
    Strut,
    /// A text run: its index in the items the flow was built from.
    Run(u32),
    /// An edge cluster: no source text and nothing to draw, the inline-axis
    /// margin, border and padding of an inline box, named by its `Open`
    /// item.
    Edge(u32),
}

/// One line's piece of a non-atomic inline box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineFragment {
    /// The box's `Open` item.
    pub item: u32,
    pub line: u32,
    /// The fragment's border box, in the paragraph's coordinates: its
    /// content area grown by its vertical padding and border, from its first
    /// to its last content on the line, plus the edges it owns there.
    pub rect: Rect<f32>,
    /// Whether this is the box's first fragment, which holds its start edge,
    /// and its last, which holds its end edge (`box-decoration-break:
    /// slice`).
    pub first: bool,
    pub last: bool,
}

/// What one layout produced.
struct FlowResult {
    constraint: BlockConstraint,
    lines: Vec<LineGeometry>,
    /// The same lines in source space.
    natural: Vec<NaturalLine>,
    metrics: BlockMetrics,
    /// Every atomic box's placement.
    atoms: Vec<PlacedBox>,
    /// The inline boxes' fragments, worked out on first read: a probe needs
    /// none of them.
    fragments: OnceLock<Vec<InlineFragment>>,
}

/// The intrinsic widths at one indent.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Intrinsic {
    indent: f32,
    min: f32,
    max: f32,
}

/// A shaped, breakable inline formatting context.
pub struct InlineFlow {
    style: InlineFlowStyle,
    /// The content flow and its natural layout; a parley style index's paint
    /// identity is the index of the item it was shaped from — a run, or an
    /// inline box's `Open` for an edge cluster — `None` for the strut's.
    paragraph: Paragraph<Option<u32>, EdgeAdditions>,
    /// Each item's line-breaking values, indexed like the items.
    breaking: Vec<BreakStyle>,
    /// The non-atomic inline boxes: their parameters, their places in the
    /// shaped text and their edge clusters.
    boxes: InlineBoxes,
    result: Option<FlowResult>,
    memo: BreakMemo,
    /// The intrinsic widths at the indent last asked about. Independent of
    /// the constraint, so one entry serves every width a pass asks at.
    intrinsic: Option<Intrinsic>,
    /// Whether the content is of the kind whose intrinsic widths parley
    /// measures in one pass, without a break (`one_pass_content`). Fixed at
    /// construction; the atoms' sizes and the indent are checked per call.
    one_pass: bool,
    /// Whether the intrinsic widths' break at zero left the natural layout
    /// with line storage for a line per word, which is freed after the next
    /// break at a constraint.
    excess_lines: bool,
    /// Every atom's margin box and baseline as the last commit laid them out.
    ///
    /// An atom's size depends on the width it is laid out at (the host
    /// shrinks it to fit, CSS 2.2 §10.3.9), so a probe after a commit can
    /// leave other sizes written in; putting the committed layout back puts
    /// these back first.
    committed_atoms: Vec<(Size<f32>, Option<f32>)>,
}

impl fmt::Debug for InlineFlow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("InlineFlow").finish_non_exhaustive()
    }
}

impl InlineFlow {
    /// Builds and shapes the block's content: `items` in document order. An
    /// atom's id, for [`Self::set_atom_size`] and in [`Self::atoms`], is its
    /// index in `items`.
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

    /// Replaces the parameters without re-shaping.
    ///
    /// Sound only while the strut, the direction and
    /// `strut_line_when_empty` are unchanged: all are shaping inputs — the
    /// strut is parley's style 0, the direction the bidi paragraph level,
    /// and the last whether empty content is shaped at all — so a change to
    /// any needs a new flow, which the host decides because it holds the old
    /// style to compare. Alignment and the indent keywords are applied at
    /// break time.
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

    /// Reports `constraint`, answering from the memo when the same
    /// constraint was already asked, without moving the committed lines.
    pub fn probe(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        Retained::probe(self, context, constraint)
    }

    /// Lays the flow out for real: a commit's lines are the ones paint reads.
    pub fn commit(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> BlockMetrics {
        let metrics = Retained::commit(self, context, constraint);
        self.committed_atoms = self.atom_sizes().collect();
        metrics
    }

    /// Whether a commit has ever produced this flow's retained result.
    #[must_use]
    pub const fn has_committed(&self) -> bool {
        self.memo.committed().is_some()
    }

    /// Whether a probe left the retained result somewhere the committed
    /// constraint did not put it.
    #[must_use]
    pub fn is_probe_dirty(&self) -> bool {
        Retained::is_probe_dirty(self)
    }

    /// Puts the retained layout back where the last commit left it. Answers
    /// whether parley was re-entered.
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

    /// Every atom's current margin box and baseline, in item order.
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

    /// The min-content width at `indent` (css-sizing-3): the widest
    /// line when every soft wrap opportunity is taken, the indent counted on
    /// the lines it applies to.
    ///
    /// May move the retained layout; a host restores it as after a probe.
    pub fn min_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).min
    }

    /// The max-content width at `indent`: the widest line when only forced
    /// breaks are taken, the indent counted on the lines it applies to.
    ///
    /// May move the retained layout; a host restores it as after a probe.
    pub fn max_content_width(&mut self, context: &mut TextContext, indent: f32) -> f32 {
        self.intrinsic(context, indent).max
    }

    /// How many times parley has broken these lines. Tests assert on it;
    /// nothing else should.
    #[must_use]
    pub const fn break_count(&self) -> u32 {
        self.memo.breaks()
    }

    /// The laid-out paragraph: glyph runs placed on the CSS line boxes
    /// [`Self::line_geometry`] reports.
    #[must_use]
    pub fn display(&self) -> &Layout<TextBrush> {
        self.expect_result();
        &self.paragraph.natural
    }

    /// The offset bridge between [`Self::display`]'s bytes and the source
    /// units the flow was built from ([`OffsetMap`]). An inline box's edges
    /// are spliced text with no source unit; nothing is ever cut, so the
    /// source content runs to the text's end.
    #[must_use]
    pub fn offset_map(&self) -> OffsetMap<'_> {
        self.expect_result();
        let block = &self.paragraph.content.block;
        OffsetMap::new(&block.text, &block.map, block.text.len())
    }

    /// The CSS line boxes, one per laid-out line.
    #[must_use]
    pub fn line_geometry(&self) -> &[LineGeometry] {
        &self.expect_result().lines
    }

    /// The reported size: the fit-content width and the stacked line boxes'
    /// height. No line box, no height.
    #[must_use]
    pub fn size(&self) -> Size<f32> {
        self.expect_result().metrics.size
    }

    /// The first line box's baseline; `None` without a line box.
    #[must_use]
    pub fn first_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.first_baseline
    }

    /// The last line box's baseline; `None` without a line box.
    #[must_use]
    pub fn last_baseline(&self) -> Option<f32> {
        self.expect_result().metrics.last_baseline
    }

    /// How many parley style indices the layout has: the range
    /// [`Self::source_of`] is defined over.
    #[must_use]
    pub fn style_count(&self) -> usize {
        self.expect_result();
        self.paragraph.natural_sources.len()
    }

    /// What a parley `style_index` of [`Self::display`] was shaped from.
    #[must_use]
    pub fn source_of(&self, style_index: u16) -> FlowSource {
        self.expect_result();
        self.source_at(style_index)
    }

    fn source_at(&self, style_index: u16) -> FlowSource {
        match self.paragraph.natural_sources[usize::from(style_index)] {
            None => FlowSource::Strut,
            // A marker's paint identity names an edge cluster: the core
            // shapes no marker, only the segments laid in for one.
            Some(item) => match self.paragraph.content.items[item as usize] {
                OwnedItem::Marker => FlowSource::Edge(item),
                _ => FlowSource::Run(item),
            },
        }
    }

    /// Updates one atomic box's measured margin box and the distance from
    /// its top to its baseline (`None`: its bottom margin edge sits on the
    /// baseline). The next [`Self::probe`] or [`Self::commit`] re-breaks
    /// without re-shaping.
    ///
    /// Panics on an id that names no atom: a correct host cannot produce one.
    pub fn set_atom_size(&mut self, item: u32, size: Size<f32>, baseline: Option<f32>) {
        let Some(OwnedItem::Box(spec)) = self.paragraph.content.items.get_mut(item as usize) else {
            panic!("set_atom_size addresses an atom this flow was built with");
        };
        if spec.size == size && spec.baseline == baseline {
            // The measure/place round trip repeats every pass; a repeat is no
            // change and must not defeat the memo.
            return;
        }
        spec.size = size;
        spec.baseline = baseline;
        self.memo.mark_boxes_dirty();
        // The break and both intrinsic widths depend on the atom's size.
        // `committed` survives: it names where the layout must be put back.
        self.memo.forget();
        self.intrinsic = None;
    }

    /// Where every atomic box ended up, in line order; each id is the atom's
    /// item index.
    #[must_use]
    pub fn atoms(&self) -> &[PlacedBox] {
        &self.expect_result().atoms
    }

    /// The static position (CSS 2.2 §10.3.7, §10.6.4) of an out-of-flow box
    /// the content holds right before item `item` — after every item when
    /// `item` is the item count — in the paragraph's own coordinates: where
    /// its hypothetical box would start on the line, the inline-start edge of
    /// the content that follows it (the right side of right-to-left
    /// content), and the top of that line box. Without a line box it is the
    /// origin.
    ///
    /// Such a box is no item: it takes nothing from the line and does not
    /// interrupt white space collapsing, so it sits where the content after
    /// it begins. After a soft wrap that is the next line, where Blink's
    /// line breaker puts an out-of-flow box too (an out-of-flow item is not
    /// "trailable" there, `IsTrailableItemType` in `line_breaker.cc`).
    #[must_use]
    pub fn static_position(&self, item: u32) -> Point<f32> {
        let result = self.expect_result();
        let content = &self.paragraph.content;
        let byte = self.item_start_byte(item);
        let Some(last) = result.natural.len().checked_sub(1) else {
            return Point::ZERO;
        };
        // The line holding `byte`: the first that ends after it, or holds
        // nothing but boxes there; past the text, the last.
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
        // What starts at or holds `byte`, by its inline-start edge: an atom
        // there that is not before `item` first, as a box sits before the
        // text at its byte; else the cluster. Failing both, the inline-end
        // edge of the content logically last before it on the line, ordered
        // by where it ends, an atom after the text ending at its byte.
        let mut atom_at: Option<(u32, f32)> = None;
        let mut cluster_at = None;
        let mut before: Option<((u32, u8, u32), f32)> = None;
        let mut note_before = |key: (u32, u8, u32), edge: f32| {
            if before.is_none_or(|(seen, _)| key >= seen) {
                before = Some((key, edge));
            }
        };
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
                        let start = u32::try_from(bytes.start).expect("text fits u32");
                        let end = u32::try_from(bytes.end).expect("text fits u32");
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
        Point::new(x, result.lines[index].top)
    }

    /// Every non-atomic inline box's fragments, by box in tree order and,
    /// within a box, by line. A box with no content and no nonzero edge has
    /// none.
    #[must_use]
    pub fn fragments(&self) -> &[InlineFragment] {
        let result = self.expect_result();
        result.fragments.get_or_init(|| self.compute_fragments())
    }

    /// The source units item `item` contributed: a run's whole pre-collapse
    /// text, an atom's one unit, an inline box boundary none. Fixed when the
    /// flow is built.
    ///
    /// `None` when `item` is past the items the flow was built from.
    #[must_use]
    pub fn item_source_range(&self, item: u32) -> Option<Range<u32>> {
        // The core holds a marker more per box the items left open.
        if item as usize >= self.breaking.len() {
            return None;
        }
        self.paragraph.item_source_range(item)
    }

    /// Where the content from item `item` on begins in the shaped text: the
    /// first of those items with a place there answers — a run's first kept
    /// byte, an atom's byte, an inline box's start (where its start edge
    /// begins) or its end (where its end edge begins). Past every item it is
    /// the end of the text.
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
                                // An `Open`: where its start edge begins.
                                at.bytes.start
                            } else {
                                // A `Close`: where its end edge begins.
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

    /// The boxes the source range `[start, end)` occupies in the committed
    /// layout, in the paragraph's own coordinates: the contract of
    /// [`crate::text::block::TextBlock::range_boxes`], over this paragraph's
    /// lines. An inline box's edges are no source text and select nothing.
    #[must_use]
    pub fn range_boxes(&self, start: u32, end: u32) -> Option<Vec<Rect<f32>>> {
        let lines = self.line_spans();
        self.paragraph
            .range_boxes_in(&self.displayed(&lines), start, end)
    }

    /// The source unit under `point`, in the paragraph's own coordinates:
    /// the contract of [`crate::text::block::TextBlock::index_at_point`]. A
    /// point on an inline box's edges is on no content.
    #[must_use]
    pub fn index_at_point(&self, point: Point<f32>) -> Option<u32> {
        let lines = self.line_spans();
        self.paragraph
            .index_at_point_in(&self.displayed(&lines), point)
    }

    fn line_spans(&self) -> Vec<LineSpan> {
        self.expect_result()
            .natural
            .iter()
            .map(|line| LineSpan {
                start: line.start_unit,
                end: line.end_unit,
            })
            .collect()
    }

    fn displayed<'a>(&'a self, lines: &'a [LineSpan]) -> Displayed<'a> {
        let block = &self.paragraph.content.block;
        Displayed {
            layout: &self.paragraph.natural,
            lines,
            geometry: &self.expect_result().lines,
            cut_byte: block.text.len(),
            content_slots: block.boxes.len(),
        }
    }

    /// Every atom on the first `line_count` lines, where parley put it:
    /// `vertical-align` applied, and moved past an inline box edge laid in
    /// before it.
    fn place_atoms(&self, line_count: usize) -> Vec<PlacedBox> {
        let content = &self.paragraph.content;
        if content.block.boxes.is_empty() {
            return Vec::new();
        }
        let mut atoms = Vec::with_capacity(content.block.boxes.len());
        // Only a line holding atoms is walked for their positions, up to its
        // last atom.
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

    /// Breaks the natural layout at `constraint`, counting the break.
    fn break_at(
        &mut self,
        context: &mut TextContext,
        constraint: BlockConstraint,
    ) -> Vec<NaturalLine> {
        self.memo.begin_break();
        self.paragraph.break_lines(
            context,
            RunBreaking::PerItem(&self.breaking),
            constraint,
            None,
            self.style.indent,
        )
    }

    /// The intrinsic widths at `indent`.
    ///
    /// For the content `one_pass_content` admits, at indent zero and with no
    /// atom of negative width, they are parley's content widths, which it
    /// measures in one pass over the shaped content without breaking it
    /// ([`Self::break_widths`] answers the same there; a debug build checks
    /// it). Otherwise they come from the two breaks.
    fn intrinsic(&mut self, context: &mut TextContext, indent: f32) -> Intrinsic {
        if let Some(intrinsic) = self.intrinsic
            && intrinsic.indent.to_bits() == indent.to_bits()
        {
            return intrinsic;
        }
        if self.paragraph.is_empty_strut_line() {
            // The one empty line a block with no content keeps shows
            // nothing, at any indent: zero wide whatever parley measured
            // its substitute space at.
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
        // The natural layout is now broken at zero, or has the boxes and the
        // indent written in that no result's lines were broken at.
        self.result = None;
        self.intrinsic = Some(intrinsic);
        intrinsic
    }

    /// The intrinsic widths at `indent`, from two breaks of the natural
    /// layout: unconstrained for max-content, at zero for min-content.
    /// `counted` counts them in the memo, as every break a layout makes is;
    /// a debug build's check of the one-pass widths leaves the memo alone.
    ///
    /// Both are read as parley's widest line, trailing whitespace excluded
    /// and the indent added on the lines it applies to, which is what makes
    /// the indent count. At zero every soft wrap opportunity is taken; an
    /// arbitrary break `overflow-wrap: break-word` allows would also be
    /// taken there, and contributes nothing to min-content (css-text-3
    /// §5.5), so parley's own minimum — which honours that — is the floor.
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
        // A hanging indent can make the narrowest layout wider than the
        // unconstrained one; max-content is never below min-content.
        (min, unconstrained.max(min))
    }
}

/// Builds the content flow of `items` and lays their inline boxes' edges
/// into it, unshaped.
///
/// The paragraph core sees every `Open` and every `Close` as a marker, and
/// one marker more at the end of the content for each box the items leave
/// open, which closes it there. Its places after white space collapsing
/// decide where each edge goes (`edge::place`).
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

/// Whether parley's one-pass content widths are the intrinsic widths the
/// two breaks find (`InlineFlow::break_widths`) for this content, at indent
/// zero and with no atom of negative width.
///
/// The two agree but for content with one of these, and an indent other
/// than zero, which the caller checks; `tests/inline_flow.rs` and
/// `tests/inline_boxes.rs` pin an input of each where they differ:
///
/// - A preserved tab: its advance is settled per break, from where it falls on its line
///   (`paragraph::tab`).
/// - A run that is `overflow-wrap: anywhere`, whose opportunities the one pass and the break at
///   zero take differently around white space and in bidi text.
/// - A negative advance: a negative `letter-spacing`, or an inline box edge with a negative margin.
///   The one pass lets a negative running width skip a soft wrap opportunity.
/// - A bidi level other than zero in any of the content: a right-to-left paragraph, or a character
///   that is right-to-left, an Arabic number or an explicit embedding, override or isolate.
///   Parley's one pass only lets white space at the paragraph's own level hang, and documents its
///   widths as possibly wrong for mixed directions. Only a right-to-left paragraph was found to
///   differ; the rest is excluded with it.
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
        if self.memo.holds(self.retained(), constraint) {
            return;
        }
        let intrinsic = self.intrinsic(context, constraint.indent);
        let natural = self.break_at(context, constraint);
        if core::mem::take(&mut self.excess_lines) {
            self.paragraph.release_line_capacity();
        }
        let alignment = resolve_alignment(self.style.text_align, self.style.direction);
        self.paragraph
            .natural
            .align(alignment, AlignmentOptions::default());
        // Parley's lines are the CSS line boxes; the blank trailing lines it
        // commits (`capture_lines`) are none of them.
        let lines: Vec<LineGeometry> = self
            .paragraph
            .natural
            .lines()
            .take(natural.len())
            .map(|line| LineGeometry::of(&line))
            .collect();
        let atoms = self.place_atoms(natural.len());
        // css-sizing-3's fit-content size: the available width clamped between
        // the intrinsic widths. Max-content is the unconstrained answer.
        let width = constraint.max_advance().map_or(intrinsic.max, |available| {
            intrinsic.max.min(intrinsic.min.max(available))
        });
        let height = lines.last().map_or(0.0, |line| line.top + line.height);
        let metrics = BlockMetrics {
            size: Size::new(width, height),
            first_baseline: lines.first().map(|line| line.baseline),
            last_baseline: lines.last().map(|line| line.baseline),
        };
        self.result = Some(FlowResult {
            constraint,
            lines,
            natural,
            metrics,
            atoms,
            fragments: OnceLock::new(),
        });
    }

    fn reported(&self) -> BlockMetrics {
        self.expect_result().metrics
    }
}
