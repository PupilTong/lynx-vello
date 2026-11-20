//! [`Canvas`]: the drawing state, and the raster that records draws into a
//! display list and replays it onto the canvas's GPU texture.

use std::sync::Arc;

use seldon::text::{ResolvedFont, ShapedLine, TextContext};

use super::image::Source;
use super::path::{self, builder_methods};
use super::style::{Paint, canvas_rect, image_quality};
use super::text::{self, Line};
use super::{
    CanvasError, CompositeOp, Direction, FillRule, Image, LineCap, LineJoin, Path,
    SmoothingQuality, Style, TextAlign, TextBaseline, TextMetrics,
};
use crate::html::canvas::{CanvasFrames, Committed};
use crate::kurbo::{
    self, Affine, BezPath, Cap, Join, ParamCurveNearest, PathEl, Point, Rect, Shape, Stroke,
    StrokeOpts,
};
use crate::paint::display_list::{DisplayList, Glyph, ListPaint, synthesized_outlines};
use crate::peniko::{BlendMode, Color, Compose, Extend, Fill, ImageQuality, ImageSampler, Mix};
use crate::render::canvas::{
    Init, MAX_SURFACE_SIDE, PendingRead, Surface, SurfaceError, SurfaceImage,
};

/// The largest `getImageData` area allocated, in pixels.
const MAX_AREA: u64 = 1 << 28;
/// Flattening tolerance for hit-testing a stroke outline, in user units.
const HIT_TOLERANCE: f64 = 0.01;
/// How far from a path's segment a hit-tested point still lies on it, in
/// canvas units.
const ON_PATH_TOLERANCE: f64 = 1e-9;
/// The most dashes a dash list cuts a path into: past it the path is
/// stroked undashed. Skia's `SkDashPath` limit (`kMaxDashCount`), which
/// Chromium's canvas follows; the standard sets none.
const MAX_DASH_COUNT: f64 = 1_000_000.0;
/// How many bytes of draws are recorded before they are replayed with no
/// read asking: Chromium's bound on a canvas's recorded ops, 2 MiB.
const MAX_RECORDED_BYTES: usize = 2 << 20;

/// A canvas bitmap with its `CanvasRenderingContext2D`.
///
/// The bitmap is premultiplied RGBA8 in a GPU texture, initially
/// transparent black. A size of zero or a side over 8,192 px (the longest
/// texture the device makes, `MAX_SURFACE_SIDE`)
/// has no bitmap: draws do nothing and reads answer transparent black.
#[derive(Debug)]
pub struct Canvas {
    width: u32,
    height: u32,
    state: State,
    stack: Vec<State>,
    /// The current default path, in canvas space.
    path: Path,
    raster: Option<Raster>,
    /// Reused for a path taken to user space.
    scratch: BezPath,
    /// The bitmap may have changed since the last commit.
    changed: bool,
}

/// The drawing state (HTML "The canvas state").
#[derive(Clone, Debug)]
struct State {
    transform: Affine,
    clip: Option<Arc<ClipNode>>,
    fill_style: Style,
    stroke_style: Style,
    global_alpha: f64,
    composite: CompositeOp,
    smoothing: bool,
    smoothing_quality: SmoothingQuality,
    line_width: f64,
    line_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f64,
    line_dash: Vec<f64>,
    line_dash_offset: f64,
    shadow_color: Color,
    shadow_blur: f64,
    shadow_offset_x: f64,
    shadow_offset_y: f64,
    /// Shared by the states `save` pushes, so a save copies no font.
    font: Arc<ResolvedFont>,
    text_align: TextAlign,
    text_baseline: TextBaseline,
    direction: Direction,
}

impl Default for State {
    fn default() -> Self {
        Self {
            transform: Affine::IDENTITY,
            clip: None,
            fill_style: Style::default(),
            stroke_style: Style::default(),
            global_alpha: 1.0,
            composite: CompositeOp::SourceOver,
            smoothing: true,
            smoothing_quality: SmoothingQuality::Low,
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: 10.0,
            line_dash: Vec::new(),
            line_dash_offset: 0.0,
            shadow_color: Color::TRANSPARENT,
            shadow_blur: 0.0,
            shadow_offset_x: 0.0,
            shadow_offset_y: 0.0,
            font: Arc::new(text::default_font()),
            text_align: TextAlign::Start,
            text_baseline: TextBaseline::Alphabetic,
            direction: Direction::Inherit,
        }
    }
}

impl State {
    /// The stroke the line styles describe, for a path `length` long in
    /// the space the stroke is computed in; `length` is called only when a
    /// dash list is set.
    fn stroke(&self, length: impl FnOnce() -> f64) -> Stroke {
        let cap = match self.line_cap {
            LineCap::Butt => Cap::Butt,
            LineCap::Round => Cap::Round,
            LineCap::Square => Cap::Square,
        };
        let join = match self.line_join {
            LineJoin::Round => Join::Round,
            LineJoin::Bevel => Join::Bevel,
            LineJoin::Miter => Join::Miter,
        };
        let stroke = Stroke::new(self.line_width)
            .with_caps(cap)
            .with_join(join)
            .with_miter_limit(self.miter_limit);
        // HTML "trace a path": a dash list summing to zero does not dash.
        // Nor does one that would cut the path into more than
        // `MAX_DASH_COUNT` dashes (a non-finite count included).
        let period = self.line_dash.iter().sum::<f64>();
        if period > 0.0 && length() / period <= MAX_DASH_COUNT {
            stroke.with_dashes(self.line_dash_offset, self.line_dash.iter().copied())
        } else {
            stroke
        }
    }

    fn alpha(&self) -> f32 {
        #[expect(clippy::cast_possible_truncation, reason = "paint alpha is f32")]
        let alpha = self.global_alpha as f32;
        alpha
    }

    fn paint(&self, style: &Style, canvas: Rect) -> Option<Paint> {
        let quality = image_quality(self.smoothing, self.smoothing_quality);
        style.paint(self.transform, self.alpha(), quality, canvas)
    }

    /// Whether a draw casts a shadow (HTML "Shadows"): the color is not
    /// transparent and the blur or an offset is not zero.
    fn casts_shadow(&self) -> bool {
        self.shadow_color.components[3] > 0.0
            && (self.shadow_blur > 0.0
                || self.shadow_offset_x != 0.0
                || self.shadow_offset_y != 0.0)
    }
}

/// One clip path in canvas space, intersected with every clip before it.
#[derive(Debug)]
struct ClipNode {
    path: BezPath,
    rule: Fill,
    /// Number of nodes on the chain, this one included.
    depth: usize,
    parent: Option<Arc<ClipNode>>,
}

/// What a draw outlines: a fill, a stroke, or glyphs.
enum Outline<'a> {
    Fill(&'a BezPath, Fill),
    Stroke(&'a BezPath, &'a Stroke),
    Rect(Rect),
    Glyphs {
        line: &'a ShapedLine,
        stroke: Option<&'a Stroke>,
    },
}

/// The bitmap's texture and the draws recorded and not yet replayed onto
/// it.
///
/// Draws are recorded into `list`; [`Raster::flush`] hands it to the
/// surface, which replays it as `init` says. A batch of `source-over` draws
/// is replayed onto the bitmap as it is. A draw composited by any other
/// operation, or one that samples the canvas itself, needs the bitmap as
/// its backdrop: `vello_gpu` composites such a layer at the root into a
/// transparent intermediate and cannot sample the texture it renders into.
/// That batch starts with the bitmap drawn as its first item and is
/// rendered into the surface's second texture ([`Init::Backdrop`]). A batch
/// that starts by clearing the canvas holds all of its content and needs
/// neither.
#[derive(Debug)]
struct Raster {
    surface: Surface,
    list: DisplayList,
    init: Init,
    /// The images `list` draws, kept until it is replayed.
    keep: Vec<SurfaceImage>,
    /// The clip paths open in `list`, outermost first.
    clips: Vec<Arc<ClipNode>>,
    clip_scratch: Vec<Arc<ClipNode>>,
    /// The canvas rectangle.
    size: Rect,
}

impl Raster {
    fn new(width: u32, height: u32) -> Option<Self> {
        Some(Self {
            surface: Surface::new(width, height)?,
            list: DisplayList::new(),
            init: Init::Load,
            keep: Vec::new(),
            clips: Vec::new(),
            clip_scratch: Vec::new(),
            size: canvas_rect(width, height),
        })
    }

    fn pending(&self) -> bool {
        self.init != Init::Load || !self.list.is_empty()
    }

    /// Replays the recorded draws onto the bitmap.
    fn flush(&mut self) {
        if !self.pending() {
            return;
        }
        for _ in self.clips.drain(..) {
            self.list.pop_layer();
        }
        self.surface.draw(&mut self.list, self.init, &mut self.keep);
        self.init = Init::Load;
    }

    /// Replays the recording when it has grown past [`MAX_RECORDED_BYTES`].
    fn bound(&mut self) {
        if self.list.recorded_bytes() > MAX_RECORDED_BYTES {
            self.flush();
        }
    }

    /// Drops the recording: the bitmap is cleared to transparent black first
    /// at the next replay.
    fn clear(&mut self) {
        self.list.reset();
        self.keep.clear();
        self.clips.clear();
        self.init = Init::Clear;
    }

    /// Makes the batch one that reads the bitmap: draws recorded so far are
    /// replayed, and the next batch starts with the bitmap drawn under the
    /// surface's own id.
    fn ensure_backdrop(&mut self) {
        if self.init != Init::Load {
            return;
        }
        self.flush();
        self.init = Init::Backdrop;
        let own = ListPaint::texture(
            self.surface.texture(),
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(ImageQuality::Low),
        );
        self.list
            .fill_with(Fill::NonZero, Affine::IDENTITY, &own, None, &self.size);
    }

    /// Makes the clips open in the recording the chain ending at `clip`.
    fn sync_clips(&mut self, clip: Option<&Arc<ClipNode>>) {
        let depth = clip.map_or(0, |node| node.depth);
        if depth == self.clips.len()
            && clip.is_none_or(|node| Arc::ptr_eq(node, &self.clips[depth - 1]))
        {
            return;
        }
        // Walk up from `clip` to the deepest node already open.
        self.clip_scratch.clear();
        let mut node = clip;
        let keep = loop {
            let Some(current) = node else { break 0 };
            if current.depth <= self.clips.len()
                && Arc::ptr_eq(current, &self.clips[current.depth - 1])
            {
                break current.depth;
            }
            self.clip_scratch.push(Arc::clone(current));
            node = current.parent.as_ref();
        };
        while self.clips.len() > keep {
            self.list.pop_layer();
            self.clips.pop();
        }
        while let Some(node) = self.clip_scratch.pop() {
            self.list
                .push_backdrop_clip(node.rule, Affine::IDENTITY, &node.path);
            self.clips.push(node);
        }
    }

    /// Records one draw (HTML "Drawing model"): the shadow, then the shape,
    /// each composited with the state's operation inside the clip.
    ///
    /// `geometry` maps the shape's coordinates to canvas space. `paint` of
    /// `None` paints transparent black, which still composites.
    fn draw(
        &mut self,
        state: &State,
        paint: Option<&Paint>,
        geometry: Affine,
        shape: &Outline<'_>,
    ) {
        let op = state.composite;
        if op == CompositeOp::SourceOver && paint.is_none() {
            return;
        }
        let live = paint.and_then(|paint| paint.live);
        if op != CompositeOp::SourceOver
            || live.is_some_and(|texture| texture.id() == self.surface.texture().id())
        {
            self.ensure_backdrop();
        }
        let blend = op.blend();
        let visible = paint.filter(|_| geometry.determinant() != 0.0);
        if let Some(paint) = visible
            && state.casts_shadow()
            && let Some(filter) = shadow_filter(state, self.size)
        {
            self.shadow(state, blend, filter, paint, geometry, shape);
        }
        self.sync_clips(state.clip.as_ref());
        let layered = op != CompositeOp::SourceOver;
        if layered {
            self.list
                .push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &self.size);
        }
        if let Some(paint) = visible {
            self.paint_shape(paint, geometry, shape);
        }
        if layered {
            self.list.pop_layer();
        }
        if let Some(keep) = paint.and_then(|paint| paint.keep.clone()) {
            self.keep.push(keep);
        }
        if live.is_some() {
            // The canvas it read may change before this one replays.
            self.flush();
        } else {
            self.bound();
        }
    }

    /// Records the shadow of a draw: cast by the unclipped shape, composited
    /// by `blend` inside the clip region.
    ///
    /// Clip paths applied to a layer's region cut what the layer composites
    /// and not what is drawn in it, while an open clip cuts both. Under
    /// `source-over` every clip path is a layer region of its own. Under
    /// another operation the blend has to read the bitmap, so the innermost
    /// path is the blend layer's region and the paths around it stay open
    /// clips, which cut the shape casting the shadow too.
    fn shadow(
        &mut self,
        state: &State,
        blend: BlendMode,
        filter: ShadowFilter,
        paint: &Paint,
        geometry: Affine,
        shape: &Outline<'_>,
    ) {
        let mut layers = 0;
        if state.composite == CompositeOp::SourceOver {
            self.sync_clips(None);
            let mut chain = Vec::new();
            let mut node = state.clip.as_ref();
            while let Some(current) = node {
                chain.push(Arc::clone(current));
                node = current.parent.as_ref();
            }
            for node in chain.iter().rev() {
                self.list.push_layer(
                    node.rule,
                    BlendMode::default(),
                    1.0,
                    Affine::IDENTITY,
                    &node.path,
                );
                layers += 1;
            }
        } else {
            let innermost = state.clip.as_ref();
            self.sync_clips(innermost.and_then(|node| node.parent.as_ref()));
            match innermost {
                Some(node) => {
                    self.list
                        .push_layer(node.rule, blend, 1.0, Affine::IDENTITY, &node.path);
                }
                None => {
                    self.list
                        .push_layer(Fill::NonZero, blend, 1.0, Affine::IDENTITY, &self.size);
                }
            }
            layers += 1;
        }
        self.list
            .push_drop_shadow(filter.dx, filter.dy, filter.std_dev, state.shadow_color);
        self.paint_shape(paint, geometry, shape);
        for _ in 0..=layers {
            self.list.pop_layer();
        }
    }

    fn paint_shape(&mut self, paint: &Paint, geometry: Affine, shape: &Outline<'_>) {
        let list = &mut self.list;
        if let Some(bounds) = &paint.bounds {
            list.push_backdrop_clip(Fill::NonZero, Affine::IDENTITY, bounds);
        }
        let brush = &paint.brush;
        let brush_transform = paint.brush_transform(geometry);
        match shape {
            Outline::Fill(path, rule) => {
                list.fill_with(*rule, geometry, brush, brush_transform, *path);
            }
            Outline::Stroke(path, stroke) => {
                list.stroke_with(stroke, geometry, brush, brush_transform, *path);
            }
            Outline::Rect(rect) => {
                list.fill_with(Fill::NonZero, geometry, brush, brush_transform, rect);
            }
            Outline::Glyphs { line, stroke } => {
                draw_glyphs(list, line, *stroke, geometry, brush, brush_transform);
            }
        }
        if paint.bounds.is_some() {
            list.pop_layer();
        }
    }
}

/// A drop shadow as the filter draws it, in canvas px.
#[derive(Clone, Copy, Debug)]
struct ShadowFilter {
    dx: f64,
    dy: f64,
    std_dev: f64,
}

/// Px a shadow's filter layer leaves of the device's texture side
/// ([`MAX_SURFACE_SIDE`]): 6 of atlas padding on each side (`vello_gpu`'s
/// `FILTER_ATLAS_PADDING`, half its 13-tap kernel), 16 for rounding four
/// lengths up to the 4 px tile, and 1 for the `f32` rounding of the
/// filter's parameters.
const SHADOW_LAYER_SLACK: f64 = 2.0 * 6.0 + 16.0 + 1.0;

/// The shadow `state` casts on a canvas of `size`, its standard deviation
/// (half `shadowBlur`) limited to what `vello_gpu` can filter; `None` when
/// not even an unblurred shadow at its offsets can be.
///
/// `vello_gpu` renders a drop-shadow layer into an intermediate texture no
/// longer than the device's texture side, and a render with a layer past
/// it fails whole. Along each axis that layer (`vello_common`
/// `FilterLayerPlacement::new`) is at most the canvas side, plus the
/// source padding on both sides (`Filter::source_expansion`) and the
/// filter padding on one (`Filter::filter_expansion`), each rounded up to
/// the 4 px tile. For a reach R = 3σ and an offset of length a along the
/// axis that is under side + 2(a + R) + max(0, R − a) + 16 px, whatever the
/// shape, so σ is kept to the largest value for which that fits beside
/// the atlas padding. Browsers bound the blur too (Skia limits its σ).
fn shadow_filter(state: &State, size: Rect) -> Option<ShadowFilter> {
    let reach = |side: f64, offset: f64| {
        let budget = f64::from(MAX_SURFACE_SIDE) - SHADOW_LAYER_SLACK - side;
        let a = offset.abs();
        if 2.0 * a > budget {
            None
        } else if 4.0 * a <= budget {
            Some((budget - a) / 3.0)
        } else {
            Some(budget / 2.0 - a)
        }
    };
    let rx = reach(size.width(), state.shadow_offset_x)?;
    let ry = reach(size.height(), state.shadow_offset_y)?;
    Some(ShadowFilter {
        dx: state.shadow_offset_x,
        dy: state.shadow_offset_y,
        std_dev: (state.shadow_blur / 2.0).min(rx.min(ry) / 3.0),
    })
}

/// Records every glyph run of `line` under `geometry`, its alphabetic
/// baseline at y = 0, with the run's synthetic bold and oblique.
fn draw_glyphs(
    list: &mut DisplayList,
    line: &ShapedLine,
    stroke: Option<&Stroke>,
    geometry: Affine,
    brush: &ListPaint,
    brush_transform: Option<Affine>,
) {
    for run in &line.runs {
        let (embolden, glyph_transform) =
            synthesized_outlines(run.synthesis.embolden, run.synthesis.skew, run.size);
        let builder = list
            .draw_glyphs(&run.font)
            .font_size(run.size)
            .transform(geometry)
            .glyph_transform(glyph_transform)
            .font_embolden(embolden)
            .normalized_coords(&run.normalized_coords)
            .hint(false)
            .paint(brush)
            .brush_transform(brush_transform);
        let glyphs = run.glyphs.iter().map(|glyph| Glyph {
            id: glyph.id,
            x: glyph.x,
            y: glyph.y,
        });
        match stroke {
            Some(stroke) => builder.draw(stroke, glyphs),
            None => builder.draw(Fill::NonZero, glyphs),
        }
    }
}

impl Canvas {
    /// A `width` × `height` canvas, transparent black, its context in the
    /// default state.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            state: State::default(),
            stack: Vec::new(),
            path: Path::new(),
            raster: Raster::new(width, height),
            scratch: BezPath::new(),
            changed: false,
        }
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Setting `width` or `height`: the bitmap takes the new size and, like
    /// `reset`, becomes transparent black with the context in its default
    /// state, even when the size is unchanged.
    pub fn set_size(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) {
            self.reset();
            return;
        }
        self.width = width;
        self.height = height;
        self.raster = Raster::new(width, height);
        self.reset_state();
        self.changed = true;
    }

    /// `reset()`: transparent black bitmap, empty path, empty state stack,
    /// default state.
    pub fn reset(&mut self) {
        if let Some(raster) = &mut self.raster {
            raster.clear();
        }
        self.reset_state();
        self.changed = true;
    }

    fn reset_state(&mut self) {
        self.state = State::default();
        self.stack.clear();
        self.path.clear();
    }

    // The state stack.

    /// `save()`.
    pub fn save(&mut self) {
        self.stack.push(self.state.clone());
    }

    /// `restore()`; nothing with an empty stack.
    pub fn restore(&mut self) {
        if let Some(state) = self.stack.pop() {
            self.state = state;
        }
    }

    // Transformations. A non-finite argument makes each a no-op.

    /// `scale(x, y)`.
    pub fn scale(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.state.transform *= Affine::scale_non_uniform(x, y);
        }
    }

    /// `rotate(angle)`, clockwise in radians.
    pub fn rotate(&mut self, angle: f64) {
        if angle.is_finite() {
            self.state.transform *= Affine::rotate(angle);
        }
    }

    /// `translate(x, y)`.
    pub fn translate(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.state.transform *= Affine::translate((x, y));
        }
    }

    /// `transform(a, b, c, d, e, f)`: the matrix multiplied onto the current
    /// one.
    pub fn transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let matrix = [a, b, c, d, e, f];
        if matrix.iter().all(|v| v.is_finite()) {
            self.state.transform *= Affine::new(matrix);
        }
    }

    /// `setTransform(a, b, c, d, e, f)`.
    pub fn set_transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let matrix = [a, b, c, d, e, f];
        if matrix.iter().all(|v| v.is_finite()) {
            self.state.transform = Affine::new(matrix);
        }
    }

    /// `getTransform()` as `[a, b, c, d, e, f]`.
    #[must_use]
    pub fn get_transform(&self) -> [f64; 6] {
        self.state.transform.as_coeffs()
    }

    /// `resetTransform()`.
    pub fn reset_transform(&mut self) {
        self.state.transform = Affine::IDENTITY;
    }

    // Compositing and image smoothing.

    #[must_use]
    pub fn global_alpha(&self) -> f64 {
        self.state.global_alpha
    }

    /// Ignored outside 0..=1 and for NaN.
    pub fn set_global_alpha(&mut self, alpha: f64) {
        if (0.0..=1.0).contains(&alpha) {
            self.state.global_alpha = alpha;
        }
    }

    #[must_use]
    pub fn global_composite_operation(&self) -> CompositeOp {
        self.state.composite
    }

    pub fn set_global_composite_operation(&mut self, op: CompositeOp) {
        self.state.composite = op;
    }

    #[must_use]
    pub fn image_smoothing_enabled(&self) -> bool {
        self.state.smoothing
    }

    pub fn set_image_smoothing_enabled(&mut self, enabled: bool) {
        self.state.smoothing = enabled;
    }

    #[must_use]
    pub fn image_smoothing_quality(&self) -> SmoothingQuality {
        self.state.smoothing_quality
    }

    pub fn set_image_smoothing_quality(&mut self, quality: SmoothingQuality) {
        self.state.smoothing_quality = quality;
    }

    // Fill and stroke styles.

    #[must_use]
    pub fn fill_style(&self) -> &Style {
        &self.state.fill_style
    }

    pub fn set_fill_style(&mut self, style: Style) {
        self.state.fill_style = style;
    }

    #[must_use]
    pub fn stroke_style(&self) -> &Style {
        &self.state.stroke_style
    }

    pub fn set_stroke_style(&mut self, style: Style) {
        self.state.stroke_style = style;
    }

    // Shadows.

    #[must_use]
    pub fn shadow_color(&self) -> Color {
        self.state.shadow_color
    }

    pub fn set_shadow_color(&mut self, color: Color) {
        self.state.shadow_color = color;
    }

    #[must_use]
    pub fn shadow_blur(&self) -> f64 {
        self.state.shadow_blur
    }

    /// Ignored for a negative or non-finite value.
    pub fn set_shadow_blur(&mut self, blur: f64) {
        if blur.is_finite() && blur >= 0.0 {
            self.state.shadow_blur = blur;
        }
    }

    #[must_use]
    pub fn shadow_offset_x(&self) -> f64 {
        self.state.shadow_offset_x
    }

    /// Ignored for a non-finite value.
    pub fn set_shadow_offset_x(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.shadow_offset_x = offset;
        }
    }

    #[must_use]
    pub fn shadow_offset_y(&self) -> f64 {
        self.state.shadow_offset_y
    }

    /// Ignored for a non-finite value.
    pub fn set_shadow_offset_y(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.shadow_offset_y = offset;
        }
    }

    // Line styles.

    #[must_use]
    pub fn line_width(&self) -> f64 {
        self.state.line_width
    }

    /// Ignored for zero, a negative or a non-finite value.
    pub fn set_line_width(&mut self, width: f64) {
        if width.is_finite() && width > 0.0 {
            self.state.line_width = width;
        }
    }

    #[must_use]
    pub fn line_cap(&self) -> LineCap {
        self.state.line_cap
    }

    pub fn set_line_cap(&mut self, cap: LineCap) {
        self.state.line_cap = cap;
    }

    #[must_use]
    pub fn line_join(&self) -> LineJoin {
        self.state.line_join
    }

    pub fn set_line_join(&mut self, join: LineJoin) {
        self.state.line_join = join;
    }

    #[must_use]
    pub fn miter_limit(&self) -> f64 {
        self.state.miter_limit
    }

    /// Ignored for zero, a negative or a non-finite value.
    pub fn set_miter_limit(&mut self, limit: f64) {
        if limit.is_finite() && limit > 0.0 {
            self.state.miter_limit = limit;
        }
    }

    /// `setLineDash(segments)`: ignored when any value is negative or not
    /// finite; an odd-length list is stored twice over.
    pub fn set_line_dash(&mut self, segments: &[f64]) {
        if !segments.iter().all(|v| v.is_finite() && *v >= 0.0) {
            return;
        }
        let dash = &mut self.state.line_dash;
        dash.clear();
        dash.extend_from_slice(segments);
        if segments.len() % 2 == 1 {
            dash.extend_from_slice(segments);
        }
    }

    /// `getLineDash()`.
    #[must_use]
    pub fn line_dash(&self) -> &[f64] {
        &self.state.line_dash
    }

    #[must_use]
    pub fn line_dash_offset(&self) -> f64 {
        self.state.line_dash_offset
    }

    /// Ignored for a non-finite value.
    pub fn set_line_dash_offset(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.line_dash_offset = offset;
        }
    }

    // Text styles.

    #[must_use]
    pub fn font(&self) -> &ResolvedFont {
        &self.state.font
    }

    pub fn set_font(&mut self, font: ResolvedFont) {
        self.state.font = Arc::new(font);
    }

    #[must_use]
    pub fn text_align(&self) -> TextAlign {
        self.state.text_align
    }

    pub fn set_text_align(&mut self, align: TextAlign) {
        self.state.text_align = align;
    }

    #[must_use]
    pub fn text_baseline(&self) -> TextBaseline {
        self.state.text_baseline
    }

    pub fn set_text_baseline(&mut self, baseline: TextBaseline) {
        self.state.text_baseline = baseline;
    }

    #[must_use]
    pub fn direction(&self) -> Direction {
        self.state.direction
    }

    pub fn set_direction(&mut self, direction: Direction) {
        self.state.direction = direction;
    }

    // Paths.

    /// `beginPath()`.
    pub fn begin_path(&mut self) {
        self.path.clear();
    }

    /// `closePath()`.
    pub fn close_path(&mut self) {
        self.path.close_path();
    }

    fn default_path(&mut self) -> &mut Path {
        &mut self.path
    }

    // Drawing.

    /// `fill(fillRule)` of the current default path.
    pub fn fill(&mut self, rule: FillRule) {
        let Some(raster) = &mut self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Fill(&self.path.elements, rule.into());
        raster.draw(&self.state, paint.as_ref(), Affine::IDENTITY, &shape);
        self.changed = true;
    }

    /// `fill(path, fillRule)`.
    pub fn fill_path(&mut self, path: &Path, rule: FillRule) {
        let Some(raster) = &mut self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Fill(&path.elements, rule.into());
        raster.draw(&self.state, paint.as_ref(), self.state.transform, &shape);
        self.changed = true;
    }

    /// `stroke()` of the current default path, with the line styles taken
    /// through the current transform.
    pub fn stroke(&mut self) {
        let Some(raster) = &mut self.raster else {
            return;
        };
        let ctm = self.state.transform;
        self.scratch.truncate(0);
        if ctm.determinant() != 0.0 {
            let inverse = ctm.inverse();
            self.scratch
                .extend(self.path.elements.iter().map(|element| inverse * element));
        }
        let paint = self.state.paint(&self.state.stroke_style, raster.size);
        let stroke = self.state.stroke(|| path::length(&self.scratch));
        let shape = Outline::Stroke(&self.scratch, &stroke);
        raster.draw(&self.state, paint.as_ref(), ctm, &shape);
        self.changed = true;
    }

    /// `stroke(path)`.
    pub fn stroke_path(&mut self, path: &Path) {
        let Some(raster) = &mut self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.stroke_style, raster.size);
        let stroke = self.state.stroke(|| path::length(&path.elements));
        let shape = Outline::Stroke(&path.elements, &stroke);
        raster.draw(&self.state, paint.as_ref(), self.state.transform, &shape);
        self.changed = true;
    }

    /// `clip(fillRule)`: the clip region intersected with the current
    /// default path.
    pub fn clip(&mut self, rule: FillRule) {
        let path = self.path.elements.clone();
        self.push_clip(path, rule);
    }

    /// `clip(path, fillRule)`.
    pub fn clip_path(&mut self, path: &Path, rule: FillRule) {
        let mut path = path.elements.clone();
        path.apply_affine(self.state.transform);
        self.push_clip(path, rule);
    }

    fn push_clip(&mut self, path: BezPath, rule: FillRule) {
        let parent = self.state.clip.take();
        self.state.clip = Some(Arc::new(ClipNode {
            path,
            rule: rule.into(),
            depth: parent.as_ref().map_or(0, |node| node.depth) + 1,
            parent,
        }));
    }

    /// `isPointInPath(x, y, fillRule)`; the point is in canvas space.
    #[must_use]
    pub fn is_point_in_path(&self, x: f64, y: f64, rule: FillRule) -> bool {
        x.is_finite() && y.is_finite() && contains(&self.path.closed(), Point::new(x, y), rule)
    }

    /// `isPointInPath(path, x, y, fillRule)`: `path` under the current
    /// transform.
    #[must_use]
    pub fn is_point_in_path2d(&self, path: &Path, x: f64, y: f64, rule: FillRule) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let mut closed = path.closed();
        closed.apply_affine(self.state.transform);
        contains(&closed, Point::new(x, y), rule)
    }

    /// `isPointInStroke(x, y)`: the stroke of the current default path with
    /// the current line styles.
    #[must_use]
    pub fn is_point_in_stroke(&self, x: f64, y: f64) -> bool {
        let ctm = self.state.transform;
        if !x.is_finite() || !y.is_finite() || ctm.determinant() == 0.0 {
            return false;
        }
        let inverse = ctm.inverse();
        let user = self.path.elements.iter().map(|element| inverse * element);
        self.in_stroke(user, inverse * Point::new(x, y))
    }

    /// `isPointInStroke(path, x, y)`.
    #[must_use]
    pub fn is_point_in_stroke_path(&self, path: &Path, x: f64, y: f64) -> bool {
        let ctm = self.state.transform;
        if !x.is_finite() || !y.is_finite() || ctm.determinant() == 0.0 {
            return false;
        }
        self.in_stroke(path.elements.iter(), ctm.inverse() * Point::new(x, y))
    }

    fn in_stroke<P>(&self, path: P, point: Point) -> bool
    where
        P: IntoIterator<Item = PathEl> + Clone,
    {
        let stroke = self.state.stroke(|| path::length(path.clone()));
        let outline = kurbo::stroke(path, &stroke, &StrokeOpts::default(), HIT_TOLERANCE);
        contains(&outline, point, FillRule::NonZero)
    }

    // Rectangles.

    /// `clearRect(x, y, w, h)`: the rectangle, inside the clip, becomes
    /// transparent black whatever the composite operation, alpha and
    /// shadow.
    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let Some(raster) = &mut self.raster else {
            return;
        };
        self.changed = true;
        let ctm = self.state.transform;
        let rect = Rect::new(x, y, x + w, y + h).abs();
        let [_, b, c, _, _, _] = ctm.as_coeffs();
        if self.state.clip.is_none()
            && b == 0.0
            && c == 0.0
            && ctm.transform_rect_bbox(rect).contains_rect(raster.size)
        {
            raster.clear();
            return;
        }
        // `destination-out` of opaque black: a blend, so the batch reads
        // the bitmap.
        raster.ensure_backdrop();
        raster.sync_clips(self.state.clip.as_ref());
        let size = raster.size;
        let list = &mut raster.list;
        list.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::DestOut),
            1.0,
            Affine::IDENTITY,
            &size,
        );
        list.fill(Fill::NonZero, ctm, Color::BLACK, None, &rect);
        list.pop_layer();
        raster.bound();
    }

    /// `fillRect(x, y, w, h)`: nothing when either side is zero (HTML
    /// "fillRect" step 2), whatever the composite operation.
    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let Some(raster) = &mut self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Rect(Rect::new(x, y, x + w, y + h).abs());
        raster.draw(&self.state, paint.as_ref(), self.state.transform, &shape);
        self.changed = true;
    }

    /// `strokeRect(x, y, w, h)`: nothing when both sides are zero, a line
    /// when one is.
    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || (w == 0.0 && h == 0.0) {
            return;
        }
        let Some(raster) = &mut self.raster else {
            return;
        };
        self.scratch.truncate(0);
        self.scratch.move_to((x, y));
        if w == 0.0 || h == 0.0 {
            self.scratch.line_to((x + w, y + h));
        } else {
            self.scratch.line_to((x + w, y));
            self.scratch.line_to((x + w, y + h));
            self.scratch.line_to((x, y + h));
            self.scratch.close_path();
        }
        let paint = self.state.paint(&self.state.stroke_style, raster.size);
        let stroke = self.state.stroke(|| path::length(&self.scratch));
        let shape = Outline::Stroke(&self.scratch, &stroke);
        raster.draw(&self.state, paint.as_ref(), self.state.transform, &shape);
        self.changed = true;
    }

    // Text.

    /// `fillText(text, x, y, maxWidth)`, shaped in `context`.
    pub fn fill_text(
        &mut self,
        context: &mut TextContext,
        text: &str,
        x: f64,
        y: f64,
        max_width: Option<f64>,
    ) {
        self.draw_text(context, text, x, y, max_width, false);
    }

    /// `strokeText(text, x, y, maxWidth)`, shaped in `context`.
    pub fn stroke_text(
        &mut self,
        context: &mut TextContext,
        text: &str,
        x: f64,
        y: f64,
        max_width: Option<f64>,
    ) {
        self.draw_text(context, text, x, y, max_width, true);
    }

    fn draw_text(
        &mut self,
        context: &mut TextContext,
        text: &str,
        x: f64,
        y: f64,
        max_width: Option<f64>,
        stroke: bool,
    ) {
        // HTML "fillText": a non-finite argument draws nothing; the text
        // preparation algorithm draws nothing for a max width of zero or
        // less.
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        if let Some(max) = max_width
            && (!max.is_finite() || max <= 0.0)
        {
            return;
        }
        let Some(raster) = &mut self.raster else {
            return;
        };
        let state = &self.state;
        let line = Line::shape(context, text, &state.font, state.direction);
        // Wider than the max width: squeeze horizontally to fit.
        let squeeze = max_width
            .filter(|max| line.width > *max)
            .map_or(1.0, |max| max / line.width);
        let anchor = line.width * text::align_fraction(state.text_align, state.direction);
        let rise = line.baseline_offset(state.text_baseline, f64::from(state.font.size));
        let geometry = state.transform
            * Affine::translate((x, y - rise))
            * Affine::scale_non_uniform(squeeze, 1.0)
            * Affine::translate((-anchor, 0.0));
        // glifo strokes the outlines at the font size times the uniform
        // scale it takes out of the transform, so their length there is at
        // most the length at the font size times the transform's scale.
        let line_stroke = stroke
            .then(|| state.stroke(|| line.outline_length() * path::scale_of(geometry).max(1.0)));
        let style = if stroke {
            &state.stroke_style
        } else {
            &state.fill_style
        };
        let paint = state.paint(style, raster.size);
        let shape = Outline::Glyphs {
            line: &line.shaped,
            stroke: line_stroke.as_ref(),
        };
        raster.draw(state, paint.as_ref(), geometry, &shape);
        self.changed = true;
    }

    /// `measureText(text)`, shaped in `context`.
    pub fn measure_text(&mut self, context: &mut TextContext, text: &str) -> TextMetrics {
        let state = &self.state;
        let line = Line::shape(context, text, &state.font, state.direction);
        let ink = line.ink_bounds();
        text::metrics(
            &line,
            ink,
            f64::from(state.font.size),
            state.text_align,
            state.text_baseline,
            state.direction,
        )
    }

    // Images.

    /// `drawImage(image, dx, dy)`. `InvalidState` for an image with no
    /// pixels.
    pub fn draw_image(&mut self, image: &Image, dx: f64, dy: f64) -> Result<(), CanvasError> {
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        self.draw_image_subrect(image, 0.0, 0.0, w, h, dx, dy, w, h)
    }

    /// `drawImage(image, dx, dy, dw, dh)`.
    pub fn draw_image_scaled(
        &mut self,
        image: &Image,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    ) -> Result<(), CanvasError> {
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        self.draw_image_subrect(image, 0.0, 0.0, w, h, dx, dy, dw, dh)
    }

    /// `drawImage(image, sx, sy, sw, sh, dx, dy, dw, dh)`. The source
    /// rectangle is clipped to the image and the destination in the same
    /// proportion.
    #[expect(clippy::too_many_arguments, reason = "the IDL method's arguments")]
    pub fn draw_image_subrect(
        &mut self,
        image: &Image,
        sx: f64,
        sy: f64,
        sw: f64,
        sh: f64,
        dx: f64,
        dy: f64,
        dw: f64,
        dh: f64,
    ) -> Result<(), CanvasError> {
        if image.is_empty() {
            return Err(CanvasError::InvalidState);
        }
        if ![sx, sy, sw, sh, dx, dy, dw, dh]
            .iter()
            .all(|v| v.is_finite())
            || sw == 0.0
            || sh == 0.0
        {
            return Ok(());
        }
        let Some(raster) = &mut self.raster else {
            return Ok(());
        };
        // Negative sizes name the same rectangles; they do not flip.
        let src = Rect::new(sx, sy, sx + sw, sy + sh).abs();
        let dst = Rect::new(dx, dy, dx + dw, dy + dh).abs();
        let pixels = Rect::new(
            0.0,
            0.0,
            f64::from(image.width()),
            f64::from(image.height()),
        );
        let clipped = src.intersect(pixels);
        if clipped.width() <= 0.0 || clipped.height() <= 0.0 {
            return Ok(());
        }
        let (kx, ky) = (dst.width() / src.width(), dst.height() / src.height());
        let dst = Rect::new(
            dst.x0 + (clipped.x0 - src.x0) * kx,
            dst.y0 + (clipped.y0 - src.y0) * ky,
            dst.x0 + (clipped.x1 - src.x0) * kx,
            dst.y0 + (clipped.y1 - src.y0) * ky,
        );
        let state = &self.state;
        let quality = image_quality(state.smoothing, state.smoothing_quality);
        let (brush, keep, live) = image.paint(
            ImageSampler::new()
                .with_extend(Extend::Pad)
                .with_quality(quality)
                .with_alpha(state.alpha()),
        );
        let paint = Paint {
            brush,
            transform: state.transform
                * Affine::translate((dst.x0, dst.y0))
                * Affine::scale_non_uniform(
                    dst.width() / clipped.width(),
                    dst.height() / clipped.height(),
                )
                * Affine::translate((-clipped.x0, -clipped.y0)),
            bounds: None,
            keep,
            live,
        };
        let shape = Outline::Rect(dst);
        raster.draw(state, Some(&paint), state.transform, &shape);
        self.changed = true;
        Ok(())
    }

    // Pixels.

    /// `getImageData(sx, sy, sw, sh)`: the `|sw|` × `|sh|` pixels from the
    /// rectangle's top-left corner as unpremultiplied RGBA8, transparent
    /// black outside the bitmap, waiting for the GPU's answer
    /// ([`Self::read_image_data`] then [`ImageDataRead::wait`]).
    ///
    /// # Errors
    ///
    /// As [`Self::read_image_data`] and [`ImageDataRead::wait`].
    pub fn get_image_data(
        &mut self,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
    ) -> Result<Vec<u8>, CanvasError> {
        self.read_image_data(sx, sy, sw, sh)?.wait()
    }

    /// Issues `getImageData(sx, sy, sw, sh)`'s readback, as of every draw
    /// recorded so far, and answers it unwaited, so a caller can wait on it
    /// in its own way.
    ///
    /// # Errors
    ///
    /// `IndexSize` for a zero side, `Range` for an area over 2^28 px, which
    /// is not allocated.
    pub fn read_image_data(
        &mut self,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
    ) -> Result<ImageDataRead, CanvasError> {
        if sw == 0 || sh == 0 {
            return Err(CanvasError::IndexSize);
        }
        let (x0, w) = normalize(sx, sw);
        let (y0, h) = normalize(sy, sh);
        let area = u64::try_from(w * h).map_err(|_| CanvasError::Range)?;
        if area > MAX_AREA {
            return Err(CanvasError::Range);
        }
        let len = usize::try_from(area * 4).map_err(|_| CanvasError::Range)?;
        let out = vec![0; len];
        let blank = ImageDataRead { out, read: None };
        let Some(raster) = &mut self.raster else {
            return Ok(blank);
        };
        let (bw, bh) = (i64::from(self.width), i64::from(self.height));
        let (cx0, cx1) = (x0.max(0), (x0 + w).min(bw));
        let (cy0, cy1) = (y0.max(0), (y0 + h).min(bh));
        if cx0 >= cx1 || cy0 >= cy1 {
            return Ok(blank);
        }
        raster.flush();
        let pending = raster.surface.read(
            (to_u32(cx0), to_u32(cy0)),
            (to_u32(cx1 - cx0), to_u32(cy1 - cy0)),
        );
        Ok(ImageDataRead {
            read: Some(RegionRead {
                pending,
                place: Placement {
                    x0: cx0 - x0,
                    y0: cy0 - y0,
                    span: cx1 - cx0,
                    stride: w,
                },
            }),
            ..blank
        })
    }

    /// `putImageData(imagedata, dx, dy, dirtyX, dirtyY, dirtyWidth,
    /// dirtyHeight)`: `data` is the image data's `width` × `height`
    /// unpremultiplied RGBA8, and `dirty` defaults to all of it. The
    /// pixels replace the bitmap's, ignoring the transform, clip, alpha,
    /// composite operation and shadow. `InvalidState` when `data` is not
    /// `width * height * 4` bytes.
    pub fn put_image_data(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        dx: i32,
        dy: i32,
        dirty: Option<[i32; 4]>,
    ) -> Result<(), CanvasError> {
        let (iw, ih) = (i64::from(width), i64::from(height));
        if usize::try_from(iw * ih * 4).ok() != Some(data.len()) {
            return Err(CanvasError::InvalidState);
        }
        let [mut x, mut y, mut w, mut h] = dirty.map_or([0, 0, iw, ih], |d| d.map(i64::from));
        // HTML "putImageData" steps 3-5.
        if w < 0 {
            x += w;
            w = -w;
        }
        if h < 0 {
            y += h;
            h = -h;
        }
        if x < 0 {
            w += x;
            x = 0;
        }
        if y < 0 {
            h += y;
            y = 0;
        }
        w = w.min(iw - x);
        h = h.min(ih - y);
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let Some(raster) = &mut self.raster else {
            return Ok(());
        };
        raster.flush();
        self.changed = true;
        let (bw, bh) = (i64::from(self.width), i64::from(self.height));
        let (dx, dy) = (i64::from(dx), i64::from(dy));
        // The dirty rectangle on the bitmap, clipped to it.
        let (cx0, cx1) = ((dx + x).max(0), (dx + x + w).min(bw));
        let (cy0, cy1) = ((dy + y).max(0), (dy + y + h).min(bh));
        if cx0 >= cx1 || cy0 >= cy1 {
            return Ok(());
        }
        let mut texels = Vec::with_capacity(index(0, (cx1 - cx0) * (cy1 - cy0), 0));
        for row in cy0..cy1 {
            let src = index(row - dy, cx0 - dx, iw);
            let src = src..src + index(0, cx1 - cx0, 0);
            texels.extend(data[src].chunks_exact(4).flat_map(premultiply));
        }
        raster.surface.write(
            (to_u32(cx0), to_u32(cy0)),
            (to_u32(cx1 - cx0), to_u32(cy1 - cy0)),
            texels,
        );
        Ok(())
    }

    // The bitmap.

    /// The bitmap as an image, for `drawImage(canvas)`, `createPattern` and
    /// `createImageBitmap`: the canvas's texture as it is when a draw of it
    /// is replayed. A draw of it replays at once, a pattern
    /// ([`super::Pattern::new`]) and a bitmap ([`Image::crop`]) copy it, so
    /// each sees the pixels as of this call. Replays what is recorded first.
    pub fn image(&mut self) -> Image {
        match &mut self.raster {
            Some(raster) => {
                raster.flush();
                Image(Source::Live(raster.surface.texture()))
            }
            None => Image::empty(),
        }
    }

    /// HTML's update of an `OffscreenCanvas`'s placeholder: the bitmap, with
    /// every draw recorded so far, becomes what the placeholder `<canvas>` of
    /// `frames` shows, and the canvas is marked unchanged. Answers what the
    /// commit owes; nothing for a canvas with no bitmap or a freed element.
    #[must_use = "a wake or a resize the commit owes is lost"]
    pub fn commit(&mut self, frames: &CanvasFrames) -> Committed {
        self.changed = false;
        let Some(raster) = &mut self.raster else {
            return Committed::default();
        };
        raster.flush();
        raster.surface.commit(frames)
    }

    /// Replays the draws recorded since the last replay onto the bitmap, so
    /// the recording does not grow for a canvas nothing reads.
    pub fn flush(&mut self) {
        if let Some(raster) = &mut self.raster {
            raster.flush();
        }
    }

    /// Whether draws are recorded and not yet replayed.
    #[must_use]
    pub fn has_pending_draws(&self) -> bool {
        self.raster.as_ref().is_some_and(Raster::pending)
    }

    /// Whether anything may have changed the bitmap since the last
    /// [`Canvas::commit`] (or since creation).
    #[must_use]
    pub fn has_changes(&self) -> bool {
        self.changed
    }

    /// `transferToImageBitmap()`: hands the bitmap's texture out and leaves
    /// the canvas transparent black, its context state untouched.
    pub fn take_image(&mut self) -> Image {
        self.changed = true;
        match &mut self.raster {
            Some(raster) => {
                raster.flush();
                Image(Source::Texture(raster.surface.take()))
            }
            None => Image::empty(),
        }
    }
}

/// A `getImageData` whose readback is issued ([`Canvas::read_image_data`]):
/// the answer's pixels once the GPU has sent them. Owns everything it needs,
/// so the canvas is free while it is waited on.
#[derive(Debug)]
#[must_use = "the pixels are only had by waiting"]
pub struct ImageDataRead {
    /// The answer, transparent black where the bitmap is not.
    out: Vec<u8>,
    /// The part inside the bitmap, `None` when there is none.
    read: Option<RegionRead>,
}

#[derive(Debug)]
struct RegionRead {
    pending: PendingRead,
    place: Placement,
}

/// Where the rows read go in the answer.
#[derive(Clone, Copy, Debug)]
struct Placement {
    /// The first pixel read, in the answer.
    x0: i64,
    y0: i64,
    /// Pixels per row read, and per row of the answer.
    span: i64,
    stride: i64,
}

impl ImageDataRead {
    /// Blocks the calling thread until the pixels are read.
    ///
    /// # Errors
    ///
    /// `InvalidState` when the GPU did not answer the readback: no device,
    /// a device lost, or, on wasm32, the Render Worker dropped it.
    pub fn wait(self) -> Result<Vec<u8>, CanvasError> {
        match self.read {
            None => Ok(self.out),
            Some(read) => {
                let pixels = read.pending.wait();
                read.place.write(self.out, pixels)
            }
        }
    }

    /// [`Self::wait`] for a caller that waits asynchronously.
    ///
    /// # Errors
    ///
    /// As [`Self::wait`].
    pub async fn wait_async(self) -> Result<Vec<u8>, CanvasError> {
        match self.read {
            None => Ok(self.out),
            Some(read) => {
                let pixels = read.pending.wait_async().await;
                read.place.write(self.out, pixels)
            }
        }
    }
}

impl Placement {
    /// Unpremultiplies the rows `read` answered into `out`.
    fn write(
        self,
        mut out: Vec<u8>,
        read: Result<Vec<u8>, SurfaceError>,
    ) -> Result<Vec<u8>, CanvasError> {
        let read = read.map_err(|_| CanvasError::InvalidState)?;
        let bytes = index(0, self.span, 0);
        for (row, from) in (self.y0..).zip(read.chunks_exact(bytes)) {
            let dst = index(row, self.x0, self.stride);
            for (to, from) in out[dst..dst + bytes]
                .chunks_exact_mut(4)
                .zip(from.chunks_exact(4))
            {
                to.copy_from_slice(&unpremultiply(from));
            }
        }
        Ok(out)
    }
}

builder_methods!(
    Canvas,
    |canvas: &Canvas| canvas.state.transform,
    default_path
);

/// Whether `point` is inside `path` under `rule`, a point on the path
/// itself included (HTML "is point in path" and "is point in stroke").
fn contains(path: &BezPath, point: Point, rule: FillRule) -> bool {
    let winding = path.winding(point);
    let inside = match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    };
    inside || on_path(path, point)
}

/// Whether `point` lies on one of `path`'s segments, within
/// [`ON_PATH_TOLERANCE`]. `BezPath::winding` counts a point on a segment
/// as inside on one side of the shape only.
fn on_path(path: &BezPath, point: Point) -> bool {
    path.segments().any(|segment| {
        segment
            .bounding_box()
            .inflate(ON_PATH_TOLERANCE, ON_PATH_TOLERANCE)
            .contains(point)
            && segment.nearest(point, ON_PATH_TOLERANCE).distance_sq
                <= ON_PATH_TOLERANCE * ON_PATH_TOLERANCE
    })
}

/// A rectangle's start and extent with a negative extent turned around.
fn normalize(start: i32, extent: i32) -> (i64, i64) {
    let (start, extent) = (i64::from(start), i64::from(extent));
    if extent < 0 {
        (start + extent, -extent)
    } else {
        (start, extent)
    }
}

/// A coordinate already clipped to the bitmap.
fn to_u32(value: i64) -> u32 {
    u32::try_from(value).expect("a coordinate inside the bitmap")
}

/// The byte offset of pixel `(x, y)` in rows `stride` pixels wide.
fn index(y: i64, x: i64, stride: i64) -> usize {
    usize::try_from((y * stride + x) * 4).expect("pixel inside the buffer")
}

fn unpremultiply(pixel: &[u8]) -> [u8; 4] {
    let a = u32::from(pixel[3]);
    match a {
        0 => [0; 4],
        255 => [pixel[0], pixel[1], pixel[2], 255],
        _ => {
            let channel =
                |c: u8| u8::try_from(((u32::from(c) * 255 + a / 2) / a).min(255)).unwrap_or(255);
            [
                channel(pixel[0]),
                channel(pixel[1]),
                channel(pixel[2]),
                pixel[3],
            ]
        }
    }
}

fn premultiply(pixel: &[u8]) -> [u8; 4] {
    let a = u32::from(pixel[3]);
    let channel = |c: u8| u8::try_from((u32::from(c) * a + 127) / 255).unwrap_or(255);
    [
        channel(pixel[0]),
        channel(pixel[1]),
        channel(pixel[2]),
        pixel[3],
    ]
}
