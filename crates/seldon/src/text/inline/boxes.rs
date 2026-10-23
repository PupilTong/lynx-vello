//! The non-atomic inline boxes of one inline formatting context: what each
//! box is — its font, direction and edges — where it sits in the shaped
//! text, and the edge clusters that carry its inline-axis margin, border and
//! padding (`super::edge`).

use parley::WhiteSpaceCollapse as ParleyWhiteSpace;

use super::edge::{EdgeRun, EdgeSide, SpanAt};
use crate::text::TextContext;
use crate::text::paragraph::shape::{self, ShapeSpan};
use crate::text::paragraph::{BreakStyle, Direction, LineHeight, RunStyle};

/// An inline box's used margin, border and padding, in px.
///
/// The inline axis is logical — start and end follow the box's own
/// `direction`, which the host resolves to physical sides — and the block
/// axis physical, the one writing mode the engine lays out. Inline-axis
/// margins may be negative; nothing here clamps them.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InlineEdges {
    pub start_margin: f32,
    /// Border plus padding at the inline start.
    pub start_inset: f32,
    /// Border plus padding at the inline end.
    pub end_inset: f32,
    pub end_margin: f32,
    /// Border plus padding above the content area. Painted, and no input to
    /// the line box (CSS 2.2 §10.6.1, §10.8.1).
    pub top_inset: f32,
    /// Border plus padding below the content area, likewise.
    pub bottom_inset: f32,
}

impl InlineEdges {
    /// The advance the start edge takes on the line it sits on.
    #[must_use]
    pub fn start(&self) -> f32 {
        self.start_margin + self.start_inset
    }

    /// The advance the end edge takes.
    #[must_use]
    pub fn end(&self) -> f32 {
        self.end_inset + self.end_margin
    }

    /// Whether the start edge takes part in the line at all: a nonzero
    /// margin, border or padding there (CSS 2.2 §9.4.2), even one a negative
    /// margin cancels out.
    pub(super) fn has_start(&self) -> bool {
        self.start_margin != 0.0 || self.start_inset != 0.0
    }

    /// The same for the end edge.
    pub(super) fn has_end(&self) -> bool {
        self.end_margin != 0.0 || self.end_inset != 0.0
    }
}

/// One inline box's own parameters, owned.
#[derive(Debug)]
pub(super) struct BoxSpec {
    /// The box's own font, whose content area its fragments cover.
    pub(super) style: RunStyle,
    pub(super) direction: Direction,
    pub(super) edges: InlineEdges,
}

impl BoxSpec {
    pub(super) fn new(style: &RunStyle, direction: Direction, edges: InlineEdges) -> Self {
        Self {
            style: style.clone(),
            direction,
            edges,
        }
    }

    /// The style the box's edge clusters are shaped in: its font family at
    /// size 0 with no line height and no spacing of its own, so a cluster
    /// adds nothing but the advance it is given. Read once per edge cluster,
    /// when the edges are laid in.
    pub(super) fn edge_style(&self) -> RunStyle {
        RunStyle {
            font_size: 0.0,
            line_height: LineHeight::Px(0.0),
            letter_spacing: 0.0,
            ..self.style.clone()
        }
    }
}

/// The inline boxes of one flow.
#[derive(Debug, Default)]
pub(super) struct InlineBoxes {
    /// Each box's parameters, in open order — tree order, so a box comes
    /// after every box it is nested in.
    pub(super) specs: Vec<BoxSpec>,
    /// Each box's place in the shaped text, indexed like `specs`.
    pub(super) spans: Vec<SpanAt>,
    /// The edge clusters laid into the text, ascending.
    pub(super) edges: Vec<EdgeRun>,
    /// The innermost box around each item, indexed like the items; an `Open`
    /// or `Close` answers its own box. Empty without boxes.
    pub(super) item_span: Vec<Option<u32>>,
    /// The boxes whose start and whose end edge each atom carries, indexed
    /// like the paragraph's atom slots: an edge next to an atom is added to
    /// the atom's advance, which is unbreakable already. Empty without boxes.
    pub(super) slot_edges: Vec<(Option<u32>, Option<u32>)>,
    /// Each box's content area above and below the baseline: its first
    /// available font's ascent and descent (CSS 2.2 §10.6.1), indexed like
    /// `specs`.
    pub(super) areas: Vec<(f32, f32)>,
}

impl InlineBoxes {
    /// Box `span`'s own font, direction and edges.
    pub(super) fn span_spec(&self, span: u32) -> (&RunStyle, Direction, &InlineEdges) {
        let spec = &self.specs[span as usize];
        (&spec.style, spec.direction, &spec.edges)
    }
}

/// The edge widths atom slot `slot` carries, as the advance laid before it
/// and after it in visual order: a start edge goes on the side its box's
/// direction starts from.
pub(super) fn slot_edge_sides(
    specs: &[BoxSpec],
    slot_edges: &[(Option<u32>, Option<u32>)],
    slot: usize,
) -> (f32, f32) {
    let Some(&(start, end)) = slot_edges.get(slot) else {
        return (0.0, 0.0);
    };
    let mut sides = (0.0, 0.0);
    if let Some(span) = start {
        let spec = &specs[span as usize];
        match spec.direction {
            Direction::Ltr => sides.0 += spec.edges.start(),
            Direction::Rtl => sides.1 += spec.edges.start(),
        }
    }
    if let Some(span) = end {
        let spec = &specs[span as usize];
        match spec.direction {
            Direction::Ltr => sides.1 += spec.edges.end(),
            Direction::Rtl => sides.0 += spec.edges.end(),
        }
    }
    sides
}

/// The advance an edge cluster with `edge`'s members takes.
pub(super) fn edge_advance(specs: &[BoxSpec], edge: &EdgeRun) -> f32 {
    edge.members
        .iter()
        .map(|&(span, side)| {
            let edges = &specs[span as usize].edges;
            match side {
                EdgeSide::Start => edges.start(),
                EdgeSide::End => edges.end(),
            }
        })
        .sum()
}

/// Each box's content area: the ascent and descent of its first available
/// font at its size (CSS 2.2 §10.6.1), which parley's metrics of a probe's
/// root span are.
///
/// One probe per distinct box font, at construction: the areas never change
/// without a rebuild, and a flow without inline boxes probes nothing.
pub(super) fn content_areas(context: &mut TextContext, specs: &[BoxSpec]) -> Vec<(f32, f32)> {
    const PROBE: &str = "0";
    let mut measured: Vec<(&RunStyle, (f32, f32))> = Vec::new();
    specs
        .iter()
        .map(|spec| {
            let style = &spec.style;
            if let Some(&(_, area)) = measured.iter().find(|(seen, _)| *seen == style) {
                return area;
            }
            let spans = [ShapeSpan {
                bytes: 0..PROBE.len(),
                style,
                breaking: BreakStyle::default(),
                hanging: ParleyWhiteSpace::Collapse,
                source: (),
                extra_spacing: 0.0,
            }];
            let area = shape::with_probe(context, PROBE, &spans, Vec::new(), |layout| {
                layout
                    .root_span_metrics()
                    .map(|metrics| (metrics.ascent, metrics.descent))
            })
            .flatten()
            .unwrap_or((style.font_size, 0.0));
            measured.push((style, area));
            area
        })
        .collect()
}
