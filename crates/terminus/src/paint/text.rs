//! Text runs: retained Parley layouts as vello glyph runs, plus
//! decorations, `text-shadow` (offset + color, no blur — recorded v1
//! limit), and `text-stroke`.
//!
//! Spec sketch:
//! - Walk `layout.lines()` → `line.items()` → `PositionedLayoutItem::GlyphRun`; for each run:
//!   `scene.draw_glyphs(run.font())` with `font_size`, the run's `normalized_coords` (parley coord
//!   i16s convert to `vello::NormalizedCoord`), `hint(false)` (transforms are arbitrary), `brush` =
//!   the element's used `color`, glyphs mapped from `glyph_run.positioned_glyphs()`.
//! - Synthesis (fake bold/oblique) from `run.synthesis()`: embolden via
//!   `DrawGlyphs::font_embolden`, oblique via `glyph_transform` skew.
//! - `text-shadow`: repeat the glyph pass per shadow (last-specified first, under the main pass),
//!   offset by the shadow offset, shadow color.
//! - `text-stroke` (`-webkit-text-stroke` semantics): a second glyph pass drawn with
//!   `StyleRef::Stroke` of the stroke width and color, over the fill (`WebKit` and Lynx native
//!   convention: fill first, stroke on top).
//! - Decorations from the element style (`text-decoration-line`/`style`/ thickness defaults): per
//!   line, per run — underline/line-through rects from `run.metrics()` (offsets are
//!   baseline-relative); `wavy` builds one sine-period path tiled across the run advance; `double`
//!   draws two lines a thickness apart. The stylo fork compiles `overline` out of
//!   `text-decoration-line` under the `lynx` feature (Lynx's decoration bitflags have no overline),
//!   so only underline and line-through can reach the painter.
//! - Inline backgrounds: a nested `<text>`/`<span>` scope has no layout box of its own — the
//!   paragraph is flattened and its slot hidden — so `background-color`/`background-image` declared
//!   on it paints as an *inline box*: one fragment per line, behind that scope's own glyphs
//!   ([`inline_background_fragments`]). The fragments are handed back to the walker, which paints
//!   them with the same `background::paint` an element box uses. That is a Lynx paragraph's: a CSS
//!   inline formatting context's inline boxes report fragments of their own, border and padding
//!   included, which the walker paints (`paint_inline_boxes`), and an inline box's edge cluster is
//!   advance with no ink ([`PaintedParagraph::draws`]), skipped here.
//! - The whole painter works in the text item's local space (origin at the text box's top-left,
//!   which is also the Parley layout origin); `transform` already includes the device scale.
//! - Everything parley positioned on a line — glyphs, decoration bands, background fragments,
//!   gradient tiles — is already on that line's box: parley 0.12 builds CSS 2.2 §10.8.1 line boxes
//!   for both paragraph kinds.

use parley::{GlyphRun, Layout, PositionedLayoutItem};
use seldon::text::block::PlacedBox;
use smallvec::SmallVec;
use stylo::computed_values::text_decoration_style::T as TextDecorationStyle;
use stylo::properties::ComputedValues;
use stylo::values::computed::{ColorPropertyValue, Image, TextDecorationLine};
use vello::kurbo::{Affine, BezPath, Diagonal2, Line, Rect, Stroke};
use vello::peniko::{BrushRef, Color, Fill, StyleRef};
use vello::{FontEmbolden, Scene, peniko};

use crate::layout::paragraph::PaintedParagraph;
use crate::paint::background::{GradientBrush, gradient_brush};
use crate::paint::convert;

enum TextFill {
    Solid(Color),
    Gradient {
        gradient: peniko::Gradient,
        brush_transform: Affine,
    },
}

impl TextFill {
    fn brush(&self) -> BrushRef<'_> {
        match self {
            Self::Solid(color) => BrushRef::Solid(*color),
            Self::Gradient { gradient, .. } => BrushRef::Gradient(gradient),
        }
    }

    const fn brush_transform(&self) -> Option<Affine> {
        match self {
            Self::Solid(_) => None,
            Self::Gradient {
                brush_transform, ..
            } => Some(*brush_transform),
        }
    }
}

pub(crate) fn needs_gradient_box(style: &ComputedValues) -> bool {
    matches!(
        style.get_inherited_text().color,
        ColorPropertyValue::Gradient(..)
    )
}

fn text_fill(style: &ComputedValues, gradient_box: Option<Rect>) -> TextFill {
    let solid = || TextFill::Solid(convert::current_color(style));
    let (Some(gradient_box), ColorPropertyValue::Gradient(gradient)) =
        (gradient_box, &style.get_inherited_text().color)
    else {
        return solid();
    };
    let tile = gradient_box.size();
    if tile.width <= 0.0 || tile.height <= 0.0 {
        return solid();
    }
    match gradient_brush(style, gradient.as_ref(), tile) {
        Some(GradientBrush::Gradient { gradient, local }) => TextFill::Gradient {
            gradient,
            brush_transform: Affine::translate(gradient_box.origin().to_vec2()) * local,
        },
        Some(GradientBrush::Solid(color)) => TextFill::Solid(color),
        None => TextFill::Solid(Color::TRANSPARENT),
    }
}

/// Everything one glyph run paints with, resolved once per paint item.
///
/// A paragraph is flattened from a subtree, so a glyph run can belong to a
/// nested scope with its own `color`, `text-shadow`, `-webkit-text-stroke` and
/// decorations. parley splits runs at every style-index change, so this is
/// indexed by that index and the lookup is O(1).
///
/// The establishing element answers for anything with no element of its own —
/// an index the block cannot resolve — which is also what the whole paragraph
/// used to paint with.
pub(crate) struct RunPaints<'doc> {
    by_style: Vec<RunPaint<'doc>>,
    fallback: RunPaint<'doc>,
    /// Which nested scopes paint a background behind the runs of one parley
    /// style index, outermost ancestor first.
    ///
    /// One entry per *index that has such a chain*, not one per index: almost
    /// every paragraph has none at all, and the ones that do have a handful, so
    /// the per-line pass scans a list that is as long as the paragraph has
    /// backgrounded scopes rather than allocating a table the width of the
    /// style space.
    backgrounds: SmallVec<[(usize, BackgroundChain); 2]>,
    /// The same for atomic inline boxes, keyed by the [`PlacedBox`] id the
    /// paragraph placed them under. An atom paints its *own* background as an
    /// element box, so its chain starts at its parent.
    atom_backgrounds: SmallVec<[(u64, BackgroundChain); 1]>,
}

/// The inline scopes that paint a background behind one run, outermost first.
type BackgroundChain = SmallVec<[crate::tree::document::NodeId; 2]>;

pub(crate) struct RunPaint<'doc> {
    style: &'doc ComputedValues,
    /// The style the ink fill comes from, separately from everything else.
    ///
    /// The two differ for exactly one run: the truncation marker under Lynx
    /// `tail-color-convert`, which swaps the dots' foreground colour for the
    /// establishing element's and changes nothing else — not the font, so the
    /// marker's geometry never moves, and not its shadow, stroke or
    /// decorations, which stay the cut run's.
    fill_style: &'doc ComputedValues,
    decorations: SmallVec<[Decorations; 2]>,
    /// The tile this run's gradient-valued `color` fills from.
    ///
    /// A per-run decision, because `color` is a per-run property: the
    /// establishing element's ramp spans its padding box, while a nested
    /// element's spans that element's own line fragments, which is the box a
    /// background on it would cover. `None` where no tile could be resolved,
    /// which falls the run back to a solid fill.
    gradient_box: Option<Rect>,
}

impl<'doc> RunPaints<'doc> {
    pub(crate) fn resolve<T>(
        document: &'doc crate::tree::document::Document<T>,
        element: crate::tree::document::NodeId,
        paragraph: &PaintedParagraph<'_>,
        gradient_box: Option<Rect>,
    ) -> Self {
        use seldon::text::block::SourceItem;

        let block_style = document
            .paint_style(element)
            .unwrap_or_else(|| unreachable!("the caller resolved this style already"));
        let fallback = RunPaint {
            style: block_style,
            fill_style: block_style,
            decorations: propagated_decorations(document, element),
            gradient_box,
        };
        // Native Lynx defaults this off: the dots wear the cut run's colour
        // (`lynx/js_libraries/types/skills/text.md`, Android
        // `TextRenderer.convertTailColor`, iOS
        // `LynxTextRenderer.m overrideTruncatedAttrIfNeed`). web-core inverts
        // the default; the 2026-09-15 ruling follows native.
        let convert_tail = crate::layout::converts_tail_color(block_style);
        let sources = paragraph.sources;
        let truncation_sources = paragraph.truncation_sources;
        // An inline formatting context's inline boxes paint their own
        // fragments (`paint::walker`); only a Lynx paragraph's scopes paint
        // backgrounds from their runs.
        let scope_backgrounds = paragraph.paints_scope_backgrounds();
        let mut by_style = Vec::new();
        // The style indices a nested element's own ramp paints, with that
        // element. Empty for every paragraph whose runs are solid-coloured or
        // whose only gradient is the block's own, so the layout pass below
        // never runs for them.
        let mut nested: SmallVec<[(usize, crate::tree::document::NodeId); 2]> = SmallVec::new();
        let mut backgrounds: SmallVec<[(usize, BackgroundChain); 2]> = SmallVec::new();
        for index in 0..paragraph.style_count() {
            // The dots are shaped in the run holding the last visible byte, so
            // that run's element answers for them exactly as it answers for its
            // own glyphs — unless `tail-color-convert` reclaims the fill below.
            let source = paragraph.source_of(u16::try_from(index).unwrap_or(u16::MAX));
            let converted = convert_tail && matches!(source, SourceItem::Ellipsis { .. });
            let resolved = match source {
                // The strut: no glyph run carries it.
                SourceItem::Strut => None,
                SourceItem::Content(item) | SourceItem::Ellipsis { item } => sources
                    .get(item as usize)
                    .copied()
                    .and_then(|node| document.paint_style(node).map(|style| (node, style))),
                // The truncation flow has its own element subtree, indexed in
                // its own space.
                SourceItem::Truncation(item) => truncation_sources
                    .get(item as usize)
                    .copied()
                    .and_then(|node| document.paint_style(node).map(|style| (node, style))),
            };
            by_style.push(match resolved {
                Some((node, style)) => {
                    if !converted && node != element && needs_gradient_box(style) {
                        nested.push((index, node));
                    }
                    if node != element && scope_backgrounds {
                        let chain = background_chain(document, node, element);
                        if !chain.is_empty() {
                            backgrounds.push((index, chain));
                        }
                    }
                    RunPaint {
                        style,
                        fill_style: if converted { block_style } else { style },
                        decorations: propagated_decorations(document, node),
                        gradient_box,
                    }
                }
                None => RunPaint {
                    style: fallback.style,
                    fill_style: fallback.style,
                    decorations: fallback.decorations.clone(),
                    gradient_box,
                },
            });
        }
        if !nested.is_empty() {
            assign_nested_tiles(paragraph, &mut by_style, &nested);
        }
        // An atom is a box of its own, so its *own* background already paints
        // through the element-box path; what it can still sit inside is a
        // backgrounded scope, which is why the walk starts at its parent.
        let mut atom_backgrounds: SmallVec<[(u64, BackgroundChain); 1]> = SmallVec::new();
        for placed in paragraph.boxes().iter().filter(|_| scope_backgrounds) {
            let PlacedBox::Visible { id, .. } = *placed else {
                continue;
            };
            let Some(parent) = box_source(id, sources, truncation_sources)
                .and_then(|node| document.get(node))
                .and_then(crate::tree::node::Node::flat_parent_id)
            else {
                continue;
            };
            let chain = background_chain(document, parent, element);
            if !chain.is_empty() {
                atom_backgrounds.push((id, chain));
            }
        }
        Self {
            by_style,
            fallback,
            backgrounds,
            atom_backgrounds,
        }
    }

    fn at(&self, style_index: usize) -> &RunPaint<'doc> {
        self.by_style.get(style_index).unwrap_or(&self.fallback)
    }

    fn block_style_run(&self) -> &RunPaint<'doc> {
        &self.fallback
    }

    /// Whether any nested scope in this paragraph paints a background, and the
    /// per-line fragment pass therefore has to run at all.
    pub(crate) fn has_inline_backgrounds(&self) -> bool {
        !self.backgrounds.is_empty() || !self.atom_backgrounds.is_empty()
    }

    fn background_chain(&self, style_index: usize) -> &[crate::tree::document::NodeId] {
        self.backgrounds
            .iter()
            .find(|(index, _)| *index == style_index)
            .map_or(&[][..], |(_, chain)| chain)
    }

    fn atom_background_chain(&self, id: u64) -> &[crate::tree::document::NodeId] {
        self.atom_backgrounds
            .iter()
            .find(|(placed, _)| *placed == id)
            .map_or(&[][..], |(_, chain)| chain)
    }
}

/// The node behind one placed atomic box.
///
/// `InlineBoxSpec::id` is the flattened item index, with the truncation flow's
/// ids offset past the content flow's
/// (`crates/terminus/src/layout/text_block.rs:410-411`), so the two source tables
/// concatenate into that one id space.
fn box_source(
    id: u64,
    sources: &[crate::tree::document::NodeId],
    truncation: &[crate::tree::document::NodeId],
) -> Option<crate::tree::document::NodeId> {
    let index = usize::try_from(id).ok()?;
    match sources.get(index) {
        Some(node) => Some(*node),
        None => truncation.get(index - sources.len()).copied(),
    }
}

/// The inline scopes strictly between `node` and the establishing `element`
/// (inclusive of `node`, exclusive of `element`) that paint a background of
/// their own, outermost first.
///
/// Outermost first is the paint order css-backgrounds-3 wants: an inner
/// scope's background covers its ancestor's.
///
/// The scopes come from [`inline_scopes`](crate::layout::text_block::inline_scopes),
/// the same walk `Document::inline_source_range` answers from, so the
/// element a background fragment paints for is the element that owns those
/// source units.
fn background_chain<T>(
    document: &crate::tree::document::Document<T>,
    node: crate::tree::document::NodeId,
    element: crate::tree::document::NodeId,
) -> BackgroundChain {
    let mut chain: BackgroundChain =
        crate::layout::text_block::inline_scopes(document.arenas(), node, element)
            .filter(|&id| document.paint_style(id).is_some_and(paints_background))
            .collect();
    chain.reverse();
    chain
}

/// Whether an inline scope has a background worth a fragment.
fn paints_background(style: &ComputedValues) -> bool {
    // A `display: contents` element — what the compiled `wrapper` carrier
    // computes to — generates no box at all, so it has no background painting
    // area either (css-display-3 3.3).
    if crate::layout::generates_no_box(style) {
        return false;
    }
    let background = style.get_background();
    convert::resolve_color(style, &background.background_color).components[3] > 0.0
        || background
            .background_image
            .0
            .iter()
            .any(|image| !matches!(image, Image::None))
}

/// Gives each nested run with a gradient-valued `color` the tile its ramp fills
/// from: the union of that element's line fragments, in paragraph-local space.
///
/// Matches web-core, which rewrites `color: <gradient>` into
/// `color: transparent; background-clip: text` plus a gradient background *on
/// that element* (`packages/web-platform/web-core/src/style_transformer/
/// rules.rs:259-291`), so the ramp spans the inline box's own background area:
/// horizontally each fragment's advance, vertically its whole line box.
///
/// A fragment that only *inherits* the gradient from an enclosing scope gets a
/// union of its own rather than the ancestor's tile. The two agree wherever the
/// inner scope spans the outer one, which is the shape the reference's fixtures
/// take.
fn assign_nested_tiles(
    paragraph: &PaintedParagraph<'_>,
    by_style: &mut [RunPaint<'_>],
    nested: &[(usize, crate::tree::document::NodeId)],
) {
    // One entry per gradient-coloured element, not per style index: parley
    // splits a run at every style change and at every line break, so one
    // element's ink can arrive as several indices and must still sample one
    // ramp. Linear scans over a list that is as long as the paragraph has
    // gradient elements.
    let mut tiles: SmallVec<[(crate::tree::document::NodeId, Rect); 2]> = SmallVec::new();
    for line in paragraph.layout.lines() {
        let metrics = line.metrics();
        let top = f64::from(metrics.block_min_coord);
        let bottom = f64::from(metrics.block_max_coord);
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            if glyph_run.glyphs().next().is_none() {
                continue;
            }
            let index = usize::from(glyph_run.style_index());
            let Some(&(_, node)) = nested.iter().find(|(wanted, _)| *wanted == index) else {
                continue;
            };
            let x = f64::from(glyph_run.offset());
            let fragment = Rect::new(x, top, x + f64::from(glyph_run.advance()), bottom);
            match tiles.iter_mut().find(|(seen, _)| *seen == node) {
                Some((_, tile)) => *tile = tile.union(fragment),
                None => tiles.push((node, fragment)),
            }
        }
    }
    for &(index, node) in nested {
        // A style index with no glyph run of its own — an empty run, or one
        // truncation removed — keeps the block's tile it was built with.
        if let Some((_, tile)) = tiles.iter().find(|(seen, _)| *seen == node) {
            by_style[index].gradient_box = Some(*tile);
        }
    }
}

/// The background fragments every nested scope of this paragraph paints, in
/// paragraph-local space and in paint order.
///
/// CSS geometry of an inline box's background (css-backgrounds-3 2, CSS 2.1
/// 10.6.1), which is what web-core gets for free by making a nested
/// `x-text`/`inline-text` `display: inline`
/// (`packages/web-platform/web-elements/src/elements/XText/x-text.css:52-67`
/// adds nothing but `background-clip: inherit`): the scope is split into one
/// fragment per line, and each fragment covers
///
/// * horizontally, that scope's own run of the line — the union of the advances of the glyph runs
///   on the line whose source element is the scope or a descendant of it;
/// * vertically, the *content area*: the font's ascent above and descent below the baseline. Not
///   the line box — `line-height: 40px` on a 20px font leaves the half-leading unpainted, which is
///   what a browser draws. This is deliberately a different box from the gradient tile
///   [`assign_nested_tiles`] builds, which spans the whole line box because web-core's `color:
///   <gradient>` rewrite makes the ramp a background of the *box*, not of the text's content area.
///
/// Native Lynx fills the line box instead (Android `BackgroundColorSpan` /
/// `LynxTextBackgroundSpan`, iOS `NSBackgroundColorAttributeName`); the
/// 2026-09-16 ruling follows web-core. `docs/tracking/web-text-test-replication
/// .md` records the conflict.
///
/// Two deliberate approximations, both of the `box-decoration-break` family:
///
/// * each fragment is painted as a whole box, so `border-radius` rounds every fragment rather than
///   only the run's two outer ends — `clone` where the web default is `slice`. The fork has no
///   `box-decoration-break` property to say otherwise.
/// * an atomic inline box under the scope is unioned into the fragment on both axes, so a tall atom
///   grows the background band. A browser keeps the band at the inline box's own font metrics and
///   lets the atom overflow it. The union is what keeps a scope whose only content *is* an atom —
///   which has no glyph run to take metrics from — painting anything at all.
pub(crate) fn inline_background_fragments(
    paragraph: &PaintedParagraph<'_>,
    runs: &RunPaints<'_>,
    out: &mut Vec<(crate::tree::document::NodeId, Rect)>,
) {
    // Insertion order *is* paint order: a chain is walked outermost first and
    // every chain that holds a descendant holds its ancestors too, so an
    // ancestor is always pushed before the scope nested in it.
    let mut scopes: SmallVec<[(crate::tree::document::NodeId, Rect); 4]> = SmallVec::new();
    let atoms = !runs.atom_backgrounds.is_empty();
    for (index, line) in paragraph.layout.lines().enumerate() {
        scopes.clear();
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            // parley splits a run at every style change, so the glyph run's
            // style index answers for the whole run — the same lookup
            // `paint_pass` makes.
            if glyph_run.glyphs().next().is_none() {
                continue;
            }
            let chain = runs.background_chain(usize::from(glyph_run.style_index()));
            if chain.is_empty() {
                continue;
            }
            let metrics = glyph_run.run().font_metrics();
            let baseline = f64::from(glyph_run.baseline());
            let x = f64::from(glyph_run.offset());
            union_into(
                &mut scopes,
                chain,
                Rect::new(
                    x,
                    baseline - f64::from(metrics.ascent),
                    x + f64::from(glyph_run.advance()),
                    baseline + f64::from(metrics.descent),
                ),
            );
        }
        if atoms {
            let line_index = u32::try_from(index).unwrap_or(u32::MAX);
            for placed in paragraph.boxes() {
                let PlacedBox::Visible {
                    id,
                    line: placed_line,
                    origin,
                    size,
                } = *placed
                else {
                    continue;
                };
                if placed_line != line_index {
                    continue;
                }
                let chain = runs.atom_background_chain(id);
                if chain.is_empty() {
                    continue;
                }
                union_into(
                    &mut scopes,
                    chain,
                    Rect::new(
                        f64::from(origin.x),
                        f64::from(origin.y),
                        f64::from(origin.x + size.width),
                        f64::from(origin.y + size.height),
                    ),
                );
            }
        }
        out.extend(scopes.iter().copied());
    }
}

fn union_into(
    scopes: &mut SmallVec<[(crate::tree::document::NodeId, Rect); 4]>,
    chain: &[crate::tree::document::NodeId],
    fragment: Rect,
) {
    for &node in chain {
        match scopes.iter_mut().find(|(seen, _)| *seen == node) {
            Some((_, rect)) => *rect = rect.union(fragment),
            None => scopes.push((node, fragment)),
        }
    }
}

pub(crate) fn paint(
    scene: &mut Scene,
    paragraph: &PaintedParagraph<'_>,
    transform: Affine,
    runs: &RunPaints<'_>,
) {
    // Two sweeps, not one per run: every shadow in the paragraph paints under
    // every glyph in it, which is what a single-run block already did and what
    // keeps one run's ink from landing under the next run's shadow.
    paint_shadows(scene, paragraph, transform, runs);
    paint_pass(scene, paragraph, transform, runs, Layer::Ink);
}

fn paint_shadows(
    scene: &mut Scene,
    paragraph: &PaintedParagraph<'_>,
    transform: Affine,
    runs: &RunPaints<'_>,
) {
    // Depth first: the last-specified shadow sits deepest, so it paints first.
    let depth = runs
        .by_style
        .iter()
        .chain(std::iter::once(&runs.fallback))
        .map(|run| run.style.get_inherited_text().text_shadow.0.len())
        .max()
        .unwrap_or(0);
    for level in (0..depth).rev() {
        paint_pass(scene, paragraph, transform, runs, Layer::Shadow(level));
    }
}

/// Which of a run's two appearances is being drawn.
#[derive(Clone, Copy)]
enum Layer {
    /// The `text-shadow` at this depth, if the run has one that deep.
    Shadow(usize),
    /// The glyphs themselves, with the run's fill, stroke and decorations.
    Ink,
}

pub(crate) fn propagated_decorations<T>(
    document: &crate::tree::document::Document<T>,
    element: crate::tree::document::NodeId,
) -> SmallVec<[Decorations; 2]> {
    use stylo::computed_values::position::T as Position;
    let mut out = SmallVec::new();
    let mut current = Some(element);
    while let Some(id) = current {
        let Some(node) = document.get(id) else { break };
        if !node.is_element() {
            break;
        }
        let Some(style) = document.paint_style(id) else {
            break;
        };
        if let Some(deco) = decorations(style) {
            out.push(deco);
        }
        if matches!(
            style.get_box().position,
            Position::Absolute | Position::Fixed
        ) {
            break;
        }
        current = node.flat_parent_id();
    }
    out
}

pub(crate) fn extent(style: &ComputedValues) -> f64 {
    let shadow_reach = style
        .get_inherited_text()
        .text_shadow
        .0
        .iter()
        .map(|shadow| f64::from(shadow.horizontal.px().abs().max(shadow.vertical.px().abs())))
        .fold(0.0, f64::max);
    let stroke_reach = text_stroke(style).map_or(0.0, |(width, _)| width / 2.0);
    shadow_reach + stroke_reach
}

/// One decorating box's paint: which lines it draws, in its style/color.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decorations {
    underline: bool,
    line_through: bool,
    style: TextDecorationStyle,
    color: Color,
}

fn decorations(style: &ComputedValues) -> Option<Decorations> {
    let text = style.get_text();
    let line = text.text_decoration_line;
    let underline = line.contains(TextDecorationLine::UNDERLINE);
    let line_through = line.contains(TextDecorationLine::LINE_THROUGH);
    if !(underline || line_through)
        || matches!(text.text_decoration_style, TextDecorationStyle::None)
    {
        return None;
    }
    Some(Decorations {
        underline,
        line_through,
        style: text.text_decoration_style,
        color: convert::resolve_color(style, &text.text_decoration_color),
    })
}

fn text_stroke(style: &ComputedValues) -> Option<(f64, Color)> {
    let inherited = style.get_inherited_text();
    let width = inherited._webkit_text_stroke_width.to_f64_px();
    (width > 0.0).then(|| {
        (
            width,
            convert::resolve_color(style, &inherited._webkit_text_stroke_color),
        )
    })
}

/// Draws the paragraph's glyph ink as an opaque mask.
///
/// `background-clip: text` needs the shape, not the appearance, so this takes
/// no per-run styles and no decorations: every run contributes the same solid
/// coverage whatever colour it would otherwise paint in.
pub(crate) fn paint_silhouette(
    scene: &mut Scene,
    layout: &Layout<crate::layout::TextBrush>,
    transform: Affine,
) {
    let fill = TextFill::Solid(Color::BLACK);
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                draw_glyph_run(scene, &glyph_run, transform, Fill::NonZero.into(), &fill);
            }
        }
    }
}

fn paint_pass(
    scene: &mut Scene,
    paragraph: &PaintedParagraph<'_>,
    transform: Affine,
    runs: &RunPaints<'_>,
    layer: Layer,
) {
    for line in paragraph.layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            // An inline box's edge cluster is advance, not ink.
            if !paragraph.draws(glyph_run.style_index()) {
                continue;
            }
            // parley splits a glyph run at every style-index change, so the
            // glyph run's style index answers for every glyph in it.
            let run = if glyph_run.glyphs().next().is_some() {
                runs.at(usize::from(glyph_run.style_index()))
            } else {
                runs.block_style_run()
            };
            let (transform, fill, stroke, decorations) = match layer {
                Layer::Shadow(level) => {
                    let shadows = &run.style.get_inherited_text().text_shadow.0;
                    let Some(shadow) = shadows.get(level) else {
                        continue;
                    };
                    let color = convert::resolve_color(run.style, &shadow.color);
                    let shadowed: SmallVec<[Decorations; 2]> = run
                        .decorations
                        .iter()
                        .map(|deco| Decorations { color, ..*deco })
                        .collect();
                    (
                        transform
                            * Affine::translate((
                                f64::from(shadow.horizontal.px()),
                                f64::from(shadow.vertical.px()),
                            )),
                        TextFill::Solid(color),
                        None,
                        shadowed,
                    )
                }
                Layer::Ink => (
                    transform,
                    text_fill(run.fill_style, run.gradient_box),
                    text_stroke(run.style),
                    run.decorations.clone(),
                ),
            };
            let (fill, decorations) = (&fill, &decorations[..]);
            let metrics = *glyph_run.run().font_metrics();
            let baseline = f64::from(glyph_run.baseline());
            let x = f64::from(glyph_run.offset());
            let width = f64::from(glyph_run.advance());

            for deco in decorations.iter().rev().filter(|deco| deco.underline) {
                let band = band(
                    x,
                    width,
                    baseline,
                    f64::from(metrics.underline_offset),
                    f64::from(metrics.underline_size),
                );
                paint_band(
                    scene,
                    transform,
                    deco.color,
                    deco.style,
                    DecorationKind::Underline,
                    &band,
                );
            }

            draw_glyph_run(scene, &glyph_run, transform, Fill::NonZero.into(), fill);
            if let Some((stroke_width, stroke_color)) = stroke {
                let stroke_style = Stroke::new(stroke_width);
                draw_glyph_run(
                    scene,
                    &glyph_run,
                    transform,
                    (&stroke_style).into(),
                    &TextFill::Solid(stroke_color),
                );
            }

            for deco in decorations.iter().rev().filter(|deco| deco.line_through) {
                let band = band(
                    x,
                    width,
                    baseline,
                    f64::from(metrics.strikethrough_offset),
                    f64::from(metrics.strikethrough_size),
                );
                paint_band(
                    scene,
                    transform,
                    deco.color,
                    deco.style,
                    DecorationKind::LineThrough,
                    &band,
                );
            }
        }
    }
}

fn draw_glyph_run(
    scene: &mut Scene,
    glyph_run: &GlyphRun<'_, ()>,
    transform: Affine,
    style: StyleRef<'_>,
    fill: &TextFill,
) {
    let run = glyph_run.run();
    // vello takes the coordinates as raw F2Dot14 bits; parley hands them out
    // typed. Empty for every font that is not variable.
    let normalized_coords: SmallVec<[i16; 4]> = run
        .normalized_coords()
        .iter()
        .map(|coord| coord.to_bits())
        .collect();
    let synthesis = run.synthesis();
    let embolden = if synthesis.embolden() {
        let amount = f64::from(run.font_size()) / 48.0;
        FontEmbolden::new(Diagonal2::new(amount, amount))
    } else {
        FontEmbolden::default()
    };
    let glyph_transform = synthesis
        .skew()
        .map(|degrees| Affine::skew(f64::from(degrees).to_radians().tan(), 0.0));
    scene
        .draw_glyphs(run.font())
        .font_size(run.font_size())
        .transform(transform)
        .glyph_transform(glyph_transform)
        .font_embolden(embolden)
        .normalized_coords(&normalized_coords)
        .hint(false)
        .brush(fill.brush())
        .brush_transform(fill.brush_transform())
        .draw(
            style,
            glyph_run.positioned_glyphs().map(|glyph| vello::Glyph {
                id: glyph.id,
                x: glyph.x,
                y: glyph.y,
            }),
        );
}

/// One decoration line's geometry in item-local space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DecorationBand {
    x: f64,
    width: f64,
    top: f64,
    thickness: f64,
}

impl DecorationBand {
    fn rect(&self, top: f64) -> Rect {
        Rect::new(self.x, top, self.x + self.width, top + self.thickness)
    }

    fn centerline(&self) -> f64 {
        self.top + self.thickness / 2.0
    }
}

fn band(x: f64, width: f64, baseline: f64, offset: f64, thickness: f64) -> DecorationBand {
    DecorationBand {
        x,
        width,
        top: baseline - offset,
        thickness,
    }
}

#[derive(Clone, Copy, Debug)]
enum DecorationKind {
    Underline,
    LineThrough,
}

fn double_tops(kind: DecorationKind, top: f64, thickness: f64) -> [f64; 2] {
    match kind {
        DecorationKind::Underline => [top, top + 2.0 * thickness],
        DecorationKind::LineThrough => [top - thickness, top + thickness],
    }
}

fn paint_band(
    scene: &mut Scene,
    transform: Affine,
    color: Color,
    line_style: TextDecorationStyle,
    kind: DecorationKind,
    band: &DecorationBand,
) {
    if band.width <= 0.0 || band.thickness <= 0.0 {
        return;
    }
    match line_style {
        TextDecorationStyle::Solid => {
            scene.fill(Fill::NonZero, transform, color, None, &band.rect(band.top));
        }
        TextDecorationStyle::Double => {
            for top in double_tops(kind, band.top, band.thickness) {
                scene.fill(Fill::NonZero, transform, color, None, &band.rect(top));
            }
        }
        TextDecorationStyle::Dotted | TextDecorationStyle::Dashed => {
            let segment = if matches!(line_style, TextDecorationStyle::Dotted) {
                band.thickness
            } else {
                3.0 * band.thickness
            };
            let stroke = Stroke::new(band.thickness).with_dashes(0.0, [segment, segment]);
            let centerline = Line::new(
                (band.x, band.centerline()),
                (band.x + band.width, band.centerline()),
            );
            scene.stroke(&stroke, transform, color, None, &centerline);
        }
        TextDecorationStyle::Wavy => {
            scene.stroke(
                &Stroke::new(band.thickness),
                transform,
                color,
                None,
                &wavy_path(band),
            );
        }
        TextDecorationStyle::None => {}
    }
}

fn wavy_path(band: &DecorationBand) -> BezPath {
    let amplitude = band.thickness;
    let half_period = 3.0 * band.thickness;
    let centerline = band.centerline();
    let end = band.x + band.width;

    let mut path = BezPath::new();
    path.move_to((band.x, centerline));
    let mut start = band.x;
    let mut sign = -1.0;
    while end - start > 1e-6 {
        let arch_width = half_period.min(end - start);
        let lift = sign * amplitude * (4.0 / 3.0) * (arch_width / half_period);
        path.curve_to(
            (start + arch_width / 3.0, centerline + lift),
            (start + 2.0 * arch_width / 3.0, centerline + lift),
            (start + arch_width, centerline),
        );
        start += arch_width;
        sign = -sign;
    }
    path
}

/// The selection highlight's colour: a UA constant until `::selection` is
/// supported (deferred in `docs/editable-text-design.md`). Chromium's light
/// blue for a focused selection, opaque, painted behind the glyphs.
const SELECTION_BACKGROUND: Color = Color::from_rgba8(0xB4, 0xD5, 0xFE, 0xFF);

/// What the selection host paints under and over its paragraph
/// (`crate::editing`): the selection highlight, and for the focused editing
/// host the caret and the IME composition's underline.
pub(crate) struct EditingPaint {
    selection: parley::Selection,
    composition: Option<core::ops::Range<usize>>,
    /// The caret's colour; `None` while the selection is a range, which
    /// shows no caret, and for a selectable element selected without focus,
    /// which has none.
    caret: Option<Color>,
    /// The composition underline's colour: the host's text colour.
    underline: Color,
    /// How tall the caret is in a paragraph with no line to measure.
    empty_line_height: f32,
}

impl EditingPaint {
    /// The editing state the paragraph `node` paints: `node` is the
    /// selection host's paragraph — the host itself, or the anonymous block
    /// box around its text ([`crate::tree::anonymous`]) — and `style` its
    /// style.
    ///
    /// The session's offsets are its selection text's — a focused host's
    /// value, a selectable element's paragraph source text; they reach the
    /// layout through the paragraph's source map, an offset the layout does
    /// not show (past a truncation cut) painted at the end of what it shows.
    pub(crate) fn of<T>(
        document: &crate::tree::document::Document<T>,
        node: crate::tree::document::NodeId,
        style: &ComputedValues,
    ) -> Option<Self> {
        let session = document.editing.session.as_ref()?;
        let host = session.host;
        if node != host && document.arenas().anonymous_container(node) != Some(host) {
            return None;
        }
        let paragraph = document.session_paragraph(session)?;
        if paragraph.node != node {
            return None;
        }
        let value = document.session_text(session);
        let selection = session.selection_in(&paragraph, &value);
        let caret = (session.focused && selection.is_collapsed()).then(|| caret_color(style));
        let composition = session.composition.as_ref().map(|range| {
            paragraph.layout_byte_clamped(&value, range.start)
                ..paragraph.layout_byte_clamped(&value, range.end)
        });
        Some(Self {
            selection,
            composition,
            caret,
            underline: convert::current_color(style),
            empty_line_height: crate::editing::empty_line_height(style),
        })
    }
}

/// The caret of the focused host when no paragraph holds its text — a host
/// that establishes no paragraph of its own (a flex `view`) and is empty, or
/// holds white space it does not render. A host that establishes one has a
/// line even when empty, where its caret paints with its text:
/// [`crate::editing::CARET_WIDTH`] wide and one used line tall, at the
/// content box's origin, in the host's border-box space. `None` for every
/// other element, and while the selection is a range.
pub(crate) fn empty_host_caret<T>(
    document: &crate::tree::document::Document<T>,
    node: crate::tree::document::NodeId,
    style: &ComputedValues,
) -> Option<(Rect, Color)> {
    let session = document.focused_session()?;
    if session.host != node || document.editable_paragraph(node).is_some() {
        return None;
    }
    if session
        .pending
        .is_some_and(|pending| pending.anchor != pending.focus)
    {
        return None;
    }
    let inset = document.content_inset(node);
    let height = crate::editing::empty_line_height(style);
    Some((
        Rect::new(
            f64::from(inset.x),
            f64::from(inset.y),
            f64::from(inset.x + crate::editing::CARET_WIDTH),
            f64::from(inset.y + height),
        ),
        caret_color(style),
    ))
}

/// The caret's colour: `caret-color: auto`, which is the text's own
/// colour. The stylo fork compiles `caret-color` for Gecko only (its
/// longhand has no `servo_pref`), so no other value can reach here yet; the
/// gap is recorded in `docs/editable-text-design.md`.
fn caret_color(style: &ComputedValues) -> Color {
    convert::current_color(style)
}

/// The selection highlight, one box per line the selection covers, under
/// everything the paragraph paints.
pub(crate) fn paint_selection(
    scene: &mut Scene,
    layout: &Layout<crate::layout::TextBrush>,
    transform: Affine,
    editing: &EditingPaint,
) {
    editing.selection.geometry_with(layout, |rect, _| {
        scene.fill(
            Fill::NonZero,
            transform,
            SELECTION_BACKGROUND,
            None,
            &Rect::new(rect.x0, rect.y0, rect.x1, rect.y1),
        );
    });
}

/// The caret and the composition underline, over the glyphs. The caret does
/// not blink (deferred in `docs/editable-text-design.md`).
pub(crate) fn paint_caret_and_composition(
    scene: &mut Scene,
    layout: &Layout<crate::layout::TextBrush>,
    transform: Affine,
    editing: &EditingPaint,
) {
    if let Some(range) = &editing.composition {
        let composed = parley::Selection::new(
            parley::Cursor::from_byte_index(layout, range.start, parley::Affinity::Downstream),
            parley::Cursor::from_byte_index(layout, range.end, parley::Affinity::Upstream),
        );
        composed.geometry_with(layout, |rect, line| {
            let Some(line) = layout.get(line) else {
                return;
            };
            let metrics = line.metrics();
            // The line's deepest font descent: parley 0.12's line metrics
            // are the CSS line box, so the descent is the glyph runs'.
            let descent = line
                .items()
                .filter_map(|item| match item {
                    PositionedLayoutItem::GlyphRun(glyph_run) => {
                        Some(glyph_run.run().font_metrics().descent)
                    }
                    PositionedLayoutItem::InlineBox(_) => None,
                })
                .fold(0.0_f32, f32::max);
            // One CSS px under the baseline by half the descent, kept inside
            // the line box.
            let y =
                f64::from((metrics.baseline + descent * 0.5).min(metrics.block_max_coord - 1.0));
            scene.fill(
                Fill::NonZero,
                transform,
                editing.underline,
                None,
                &Rect::new(rect.x0, y, rect.x1, y + 1.0),
            );
        });
    }
    if let Some(color) = editing.caret {
        let caret = crate::editing::caret_box(
            layout,
            editing.selection.focus(),
            crate::editing::CARET_WIDTH,
            editing.empty_line_height,
        );
        scene.fill(
            Fill::NonZero,
            transform,
            color,
            None,
            &Rect::new(
                f64::from(caret.min_x()),
                f64::from(caret.min_y()),
                f64::from(caret.max_x()),
                f64::from(caret.max_y()),
            ),
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use vello::kurbo::{CubicBez, ParamCurve, PathEl, Point};

    use super::*;

    const AHEM: &[u8] = include_bytes!("../../../seldon/tests/fixtures/Ahem.ttf");

    /// Every glyph `scene` encodes, as `(x, y)` in the scene's space.
    fn glyph_origins(scene: &Scene) -> Vec<(f32, f32)> {
        let resources = &scene.encoding().resources;
        resources
            .glyph_runs
            .iter()
            .flat_map(|run| {
                resources.glyphs[run.glyphs.clone()].iter().map(|glyph| {
                    (
                        run.transform.translation[0] + glyph.x,
                        run.transform.translation[1] + glyph.y,
                    )
                })
            })
            .collect()
    }

    #[test]
    fn an_inline_formatting_context_paints_its_glyphs_on_the_css_baseline() {
        // A 20px Ahem block holding 10px Ahem text: the strut puts the line's
        // baseline at 16 (CSS 2.2 §10.8.1), where parley alone puts it at 8.
        let mut doc = crate::tree::document::Document::new(
            crate::tree::document::tests::device(),
            "page",
            (),
        );
        doc.register_fonts(seldon::text::FontBlob::from_static(AHEM));
        doc.add_stylesheet(
            "page { display: flex; align-items: flex-start; width: 200px; height: 200px;
                    font-family: Ahem; font-size: 20px; }
             small { display: contents; font-size: 10px; }",
            crate::style::engine::StylesheetOrigin::Author,
        );
        let block = doc.create_element("anonymous", ());
        doc.append_child(doc.document_element().id(), block);
        let small = doc.create_element("small", ());
        doc.append_child(block, small);
        let text = doc.create_text_node("aa", ());
        doc.append_child(small, text);
        doc.layout();
        doc.make_anonymous_block(block);
        doc.layout();

        let paragraph = doc.painted_paragraph(block).expect("a committed flow");
        let runs = RunPaints::resolve(&doc, block, &paragraph, None);
        let mut scene = Scene::new();
        paint(&mut scene, &paragraph, Affine::IDENTITY, &runs);
        assert_eq!(glyph_origins(&scene), [(0.0, 16.0), (10.0, 16.0)]);
    }

    fn assert_near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    fn cubics(path: &BezPath) -> Vec<CubicBez> {
        let mut segments = Vec::new();
        let mut current = Point::ZERO;
        for element in path.elements() {
            match *element {
                PathEl::MoveTo(point) => current = point,
                PathEl::CurveTo(p1, p2, p3) => {
                    segments.push(CubicBez::new(current, p1, p2, p3));
                    current = p3;
                }
                _ => panic!("wavy paths contain only moves and cubics"),
            }
        }
        segments
    }

    #[test]
    fn wavy_waves_tile_whole_arches_that_peak_at_the_amplitude() {
        let band = DecorationBand {
            x: 10.0,
            width: 12.0,
            top: 100.0,
            thickness: 2.0,
        };
        let path = wavy_path(&band);
        let segments = cubics(&path);
        assert_eq!(segments.len(), 2);

        let centerline = 101.0;
        assert_eq!(segments[0].p0, Point::new(10.0, centerline));
        assert_eq!(segments[0].p3, Point::new(16.0, centerline));
        assert_eq!(segments[1].p3, Point::new(22.0, centerline));
        assert_near(segments[0].eval(0.5).y, centerline - 2.0);
        assert_near(segments[1].eval(0.5).y, centerline + 2.0);
    }

    #[test]
    fn wavy_final_partial_arches_compress_to_the_band_edge() {
        let band = DecorationBand {
            x: 0.0,
            width: 9.0,
            top: 0.0,
            thickness: 2.0,
        };
        let path = wavy_path(&band);
        let segments = cubics(&path);
        assert_eq!(segments.len(), 2);
        let centerline = 1.0;
        assert_eq!(segments[1].p3, Point::new(9.0, centerline));
        assert_near(segments[1].eval(0.5).y, centerline + 1.0);
    }

    #[test]
    fn double_underlines_grow_down_and_double_strikes_straddle() {
        let [first, second] = double_tops(DecorationKind::Underline, 10.0, 2.0);
        assert_near(first, 10.0);
        assert_near(second, 14.0);
        let [above, below] = double_tops(DecorationKind::LineThrough, 10.0, 2.0);
        assert_near(above, 8.0);
        assert_near(below, 12.0);
    }

    #[test]
    fn bands_convert_parley_y_up_offsets_to_y_down_tops() {
        let underline = band(5.0, 40.0, 20.0, -3.0, 1.5);
        assert_near(underline.top, 23.0);
        assert_eq!(
            underline.rect(underline.top),
            Rect::new(5.0, 23.0, 45.0, 24.5)
        );
        let strike = band(5.0, 40.0, 20.0, 8.0, 1.0);
        assert_near(strike.top, 12.0);
        assert_near(strike.centerline(), 12.5);
    }

    /// The selection highlight paints for a selectable element's session,
    /// which never shows a caret; a focused host's collapsed selection does.
    #[test]
    fn a_selectable_root_paints_its_selection_and_no_caret() {
        let mut doc =
            crate::link::Document::new(crate::tree::document::tests::device(), "page", ());
        crate::test_common::define_lynx_text(&mut doc, "text");
        doc.register_fonts(crate::link::FontBlob::from_static(AHEM));
        doc.add_stylesheet(
            "page { display: flex; align-items: flex-start; font-family: Ahem; \
             font-size: 10px; line-height: 10px; }",
            crate::link::StylesheetOrigin::Author,
        );
        let root = doc.create_element("text", ());
        doc.set_inline_style(root, "user-select: text;");
        doc.append_child(doc.document_element().id(), root);
        let text = doc.create_text_node("hello", ());
        doc.append_child(root, text);
        doc.layout();
        assert!(doc.begin_selection(root));
        doc.layout();
        let paint = |doc: &crate::link::Document<()>| {
            EditingPaint::of(doc, root, doc.paint_style(root).expect("styled"))
                .expect("the selection host paints its selection")
        };
        let collapsed = paint(&doc);
        assert!(collapsed.selection.is_collapsed());
        assert!(collapsed.caret.is_none(), "collapsed, but not focused");
        assert!(
            doc.apply_editing(crate::editing::Command::SetSelection { range: 1..3 })
                .applied
        );
        let range = paint(&doc);
        assert_eq!(range.selection.text_range(), 1..3);
        assert!(range.caret.is_none());

        doc.set_attribute(root, "contenteditable", "");
        assert!(doc.focus(root));
        doc.layout();
        assert!(paint(&doc).caret.is_some(), "a focused host's caret");
    }
}
