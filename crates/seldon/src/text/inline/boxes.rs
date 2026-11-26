//! The non-atomic inline boxes of one inline formatting context: what each
//! box is — its font, direction and edges — where it sits in the shaped
//! text, and the edge clusters that carry its inline-axis margin, border and
//! padding (`super::edge`).

use parley::WhiteSpaceCollapse as ParleyWhiteSpace;

use super::edge::{EdgeRun, EdgeSide, SpanAt};
use crate::text::TextContext;
use crate::text::paragraph::shape::{self, ShapeSpan};
use crate::text::paragraph::{BreakStyle, Direction, LineHeight, RunStyle};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct InlineEdges {
    pub start_margin: f32,
    pub start_inset: f32,
    pub end_inset: f32,
    pub end_margin: f32,
    pub top_inset: f32,
    pub bottom_inset: f32,
}

impl InlineEdges {
    #[must_use]
    pub fn start(&self) -> f32 {
        self.start_margin + self.start_inset
    }

    #[must_use]
    pub fn end(&self) -> f32 {
        self.end_inset + self.end_margin
    }

    pub(super) fn has_start(&self) -> bool {
        self.start_margin != 0.0 || self.start_inset != 0.0
    }

    pub(super) fn has_end(&self) -> bool {
        self.end_margin != 0.0 || self.end_inset != 0.0
    }
}

#[derive(Debug)]
pub(super) struct BoxSpec {
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

    pub(super) fn edge_style(&self) -> RunStyle {
        RunStyle {
            font_size: 0.0,
            line_height: LineHeight::Px(0.0),
            letter_spacing: 0.0,
            ..self.style.clone()
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct InlineBoxes {
    pub(super) specs: Vec<BoxSpec>,
    pub(super) spans: Vec<SpanAt>,
    pub(super) edges: Vec<EdgeRun>,
    pub(super) item_span: Vec<Option<u32>>,
    pub(super) slot_edges: Vec<(Option<u32>, Option<u32>)>,
    pub(super) areas: Vec<(f32, f32)>,
}

impl InlineBoxes {
    pub(super) fn span_spec(&self, span: u32) -> (&RunStyle, Direction, &InlineEdges) {
        let spec = &self.specs[span as usize];
        (&spec.style, spec.direction, &spec.edges)
    }
}

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
