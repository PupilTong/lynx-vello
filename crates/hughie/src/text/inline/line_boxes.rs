//! CSS 2.2 §10.8.1 line boxes over parley's committed lines.
//!
//! Every inline box contributes its A + D (ascent plus descent) with the
//! leading L = `line-height` − (A + D) split in half above and below, all
//! boxes sharing one baseline; a line box reaches from the highest box top
//! to the lowest box bottom. Text runs here are the only content, so the
//! boxes on one line are:
//!
//! - the strut: a zero-width box with the block container's first available font and `line-height`,
//!   on every line;
//! - each run with content on the line, at its own first available font and `line-height`;
//! - with `line-height: normal`, each font the run's glyphs actually use, at that font's own
//!   ascent, descent and line gap. Chrome includes the used fonts in a normal line height; with a
//!   numeric or length `line-height` only the first available font counts, and glyphs from a
//!   fallback font overflow the line box.
//!
//! A run's first available font is the first font of its `font-family` list
//! that has a glyph for U+0020 (css-fonts-4 §5.4), found by shaping one
//! space; `line-height: normal` is that font's ascent + descent + line gap,
//! Chrome's value, unrounded.
//!
//! Whitespace that ends a line — a collapsible space hanging after a soft
//! wrap, the forced break itself — is not on the line for this purpose: it is
//! removed or hangs (css-text-3 §4.1.2), so its run adds nothing unless the
//! line holds nothing else, as a line that is only a preserved newline does.

use parley::Layout;

use crate::style::TextBrush;
use crate::text::TextContext;
use crate::text::paragraph::shape::{self, NaturalLine, ShapeSpan};
use crate::text::paragraph::{BreakStyle, LineGeometry, LineHeight, Paragraph, RunStyle};

/// The vertical metrics of one font at one size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct FontMetrics {
    ascent: f32,
    descent: f32,
    line_gap: f32,
}

/// The strut's first available font, and each run's, shaping one space per
/// distinct font.
pub(super) fn first_available_fonts<'a>(
    context: &mut TextContext,
    strut: &RunStyle,
    runs: impl Iterator<Item = &'a RunStyle>,
) -> (FontMetrics, Vec<FontMetrics>) {
    let strut_font = first_available_font(context, strut);
    let mut measured: Vec<(&RunStyle, FontMetrics)> = Vec::new();
    let runs = runs
        .map(|style| {
            if same_font(style, strut) {
                return strut_font;
            }
            if let Some(&(_, metrics)) = measured.iter().find(|(seen, _)| same_font(seen, style)) {
                return metrics;
            }
            let metrics = first_available_font(context, style);
            measured.push((style, metrics));
            metrics
        })
        .collect();
    (strut_font, runs)
}

/// Whether two run styles select the same font at the same size: the inputs
/// a first available font and its metrics depend on.
fn same_font(first: &RunStyle, second: &RunStyle) -> bool {
    first.font_family == second.font_family
        && first.font_size.to_bits() == second.font_size.to_bits()
        && first.font_weight == second.font_weight
        && first.font_style == second.font_style
        && first.font_variations == second.font_variations
}

/// The metrics of the font one space in `style` is shaped with.
fn first_available_font(context: &mut TextContext, style: &RunStyle) -> FontMetrics {
    const SPACE: &str = " ";
    let spans = [ShapeSpan {
        bytes: 0..SPACE.len(),
        style,
        breaking: BreakStyle::default(),
        source: (),
        extra_spacing: 0.0,
    }];
    let mut probe: Layout<TextBrush> =
        shape::shape(context, SPACE, &spans, Vec::new(), &mut Vec::new());
    probe.break_all_lines(None);
    probe
        .lines()
        .next()
        .and_then(|line| line.runs().next().map(|run| *run.metrics()))
        .map_or_else(FontMetrics::default, |metrics| FontMetrics {
            ascent: metrics.ascent,
            descent: metrics.descent,
            line_gap: metrics.leading,
        })
}

/// What a box with `font`'s metrics and `style`'s `line-height` reaches
/// above and below the baseline.
fn half_leading(style: &RunStyle, font: FontMetrics) -> (f32, f32) {
    let content = font.ascent + font.descent;
    let line_height = match style.line_height {
        LineHeight::Normal => content + font.line_gap,
        LineHeight::Number(factor) => factor * style.font_size,
        LineHeight::Px(px) => px,
    };
    let half = (line_height - content) / 2.0;
    (font.ascent + half, font.descent + half)
}

/// One [`LineGeometry`] per committed line of `paragraph`'s natural layout,
/// stacked from the top, after alignment.
pub(super) fn line_boxes(
    paragraph: &Paragraph<u32>,
    natural_lines: &[NaturalLine],
    strut: &RunStyle,
    strut_font: FontMetrics,
    run_fonts: &[FontMetrics],
) -> Vec<LineGeometry> {
    let layout = &paragraph.natural;
    let flow = &paragraph.content;
    let text = &flow.block.text;
    let text_len = u32::try_from(text.len()).expect("text fits u32");
    let strut_box = half_leading(strut, strut_font);

    let mut lines = Vec::with_capacity(natural_lines.len());
    let mut top = 0.0_f32;
    for (index, natural_line) in natural_lines.iter().enumerate() {
        let Some(line) = layout.get(index) else {
            break;
        };
        let visible_end =
            flow.block
                .map
                .visible_end(text, natural_line.start_byte, natural_line.end_byte);
        let has_content = visible_end > natural_line.start_byte;
        let (mut above, mut below) = strut_box;
        for run in line.runs() {
            let used = run.metrics();
            let mut last_item = None;
            for cluster in run.clusters() {
                let start = u32::try_from(cluster.text_range().start).expect("text fits u32");
                if start >= text_len || (has_content && start >= visible_end) {
                    continue;
                }
                let item = flow.item_at(start);
                let style = flow.run_style(item);
                if last_item != Some(item) {
                    let (run_above, run_below) = half_leading(style, run_fonts[item as usize]);
                    above = above.max(run_above);
                    below = below.max(run_below);
                    last_item = Some(item);
                }
                if style.line_height == LineHeight::Normal {
                    above = above.max(used.ascent + used.leading / 2.0);
                    below = below.max(used.descent + used.leading / 2.0);
                }
            }
        }
        let height = above + below;
        let baseline = top + above;
        lines.push(LineGeometry {
            top,
            height,
            baseline,
            shift: baseline - line.metrics().baseline,
        });
        top += height;
    }
    lines
}
