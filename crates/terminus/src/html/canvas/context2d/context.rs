//! [`Context2d`]: HTML's `OffscreenCanvasRenderingContext2D` of one
//! [`OffscreenCanvas`](crate::html::canvas::OffscreenCanvas), with the
//! drawing state `save()` pushes and the current default path. Its draws
//! are recorded through the canvas's [`Raster`].

use std::sync::Arc;

use seldon::text::{ResolvedFont, TextContext};

use super::css::{parse_color, parse_font, serialize_color};
use super::path::{self, builder_methods};
use super::pixels::{ImageDataRead, Placement, RegionRead, index, normalize, premultiply, to_u32};
use super::raster::{ClipNode, Outline, Raster};
use super::style::{Paint, image_quality};
use super::text::{self, Line};
use super::{
    CanvasError, CompositeOp, Direction, FillRule, Image, LineCap, LineJoin, Path,
    SmoothingQuality, Style, TextAlign, TextBaseline, TextMetrics,
};
use crate::kurbo::{
    self, Affine, BezPath, Cap, Join, ParamCurveNearest, PathEl, Point, Rect, Shape, Stroke,
    StrokeOpts,
};
use crate::peniko::{BlendMode, Color, Compose, Extend, Fill, ImageSampler, Mix};

const MAX_AREA: u64 = 1 << 28;
const HIT_TOLERANCE: f64 = 0.01;
const ON_PATH_TOLERANCE: f64 = 1e-9;
const MAX_DASH_COUNT: f64 = 1_000_000.0;

#[derive(Debug)]
pub(in crate::html::canvas) struct Context2dData {
    state: State,
    stack: Vec<State>,
    path: Path,
    scratch: BezPath,
}

impl Context2dData {
    pub(in crate::html::canvas) fn new() -> Self {
        Self {
            state: State::default(),
            stack: Vec::new(),
            path: Path::new(),
            scratch: BezPath::new(),
        }
    }
}

#[derive(Debug)]
pub struct Context2d<'a> {
    width: u32,
    height: u32,
    raster: &'a mut Option<Raster>,
    changed: &'a mut bool,
    state: &'a mut State,
    stack: &'a mut Vec<State>,
    path: &'a mut Path,
    scratch: &'a mut BezPath,
}

#[derive(Clone, Debug)]
pub(super) struct State {
    transform: Affine,
    pub(super) clip: Option<Arc<ClipNode>>,
    fill_style: Style,
    stroke_style: Style,
    global_alpha: f64,
    pub(super) composite: CompositeOp,
    smoothing: bool,
    smoothing_quality: SmoothingQuality,
    line_width: f64,
    line_cap: LineCap,
    line_join: LineJoin,
    miter_limit: f64,
    line_dash: Vec<f64>,
    line_dash_offset: f64,
    pub(super) shadow_color: Color,
    pub(super) shadow_blur: f64,
    pub(super) shadow_offset_x: f64,
    pub(super) shadow_offset_y: f64,
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

    pub(super) fn casts_shadow(&self) -> bool {
        self.shadow_color.components[3] > 0.0
            && (self.shadow_blur > 0.0
                || self.shadow_offset_x != 0.0
                || self.shadow_offset_y != 0.0)
    }
}

impl<'a> Context2d<'a> {
    pub(in crate::html::canvas) fn new(
        data: &'a mut Context2dData,
        width: u32,
        height: u32,
        raster: &'a mut Option<Raster>,
        changed: &'a mut bool,
    ) -> Self {
        let Context2dData {
            state,
            stack,
            path,
            scratch,
        } = data;
        Self {
            width,
            height,
            raster,
            changed,
            state,
            stack,
            path,
            scratch,
        }
    }
}

impl Context2d<'_> {
    pub fn reset(&mut self) {
        if let Some(raster) = &mut *self.raster {
            raster.clear();
        }
        self.reset_state();
        *self.changed = true;
    }

    pub(in crate::html::canvas) fn reset_state(&mut self) {
        *self.state = State::default();
        self.stack.clear();
        self.path.clear();
    }

    pub fn save(&mut self) {
        self.stack.push(self.state.clone());
    }

    pub fn restore(&mut self) {
        if let Some(state) = self.stack.pop() {
            *self.state = state;
        }
    }

    pub fn scale(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.state.transform *= Affine::scale_non_uniform(x, y);
        }
    }

    pub fn rotate(&mut self, angle: f64) {
        if angle.is_finite() {
            self.state.transform *= Affine::rotate(angle);
        }
    }

    pub fn translate(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() {
            self.state.transform *= Affine::translate((x, y));
        }
    }

    pub fn transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let matrix = [a, b, c, d, e, f];
        if matrix.iter().all(|v| v.is_finite()) {
            self.state.transform *= Affine::new(matrix);
        }
    }

    pub fn set_transform(&mut self, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
        let matrix = [a, b, c, d, e, f];
        if matrix.iter().all(|v| v.is_finite()) {
            self.state.transform = Affine::new(matrix);
        }
    }

    #[must_use]
    pub fn get_transform(&self) -> [f64; 6] {
        self.state.transform.as_coeffs()
    }

    pub fn reset_transform(&mut self) {
        self.state.transform = Affine::IDENTITY;
    }

    #[must_use]
    pub fn global_alpha(&self) -> f64 {
        self.state.global_alpha
    }

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

    #[must_use]
    pub fn fill_style(&self) -> &Style {
        &self.state.fill_style
    }

    pub fn set_fill_style(&mut self, style: Style) {
        self.state.fill_style = style;
    }

    pub fn set_fill_style_css(&mut self, css: &str) -> Option<String> {
        let color = parse_color(css)?;
        self.set_fill_style(Style::Color(color));
        Some(serialize_color(color))
    }

    #[must_use]
    pub fn stroke_style(&self) -> &Style {
        &self.state.stroke_style
    }

    pub fn set_stroke_style(&mut self, style: Style) {
        self.state.stroke_style = style;
    }

    pub fn set_stroke_style_css(&mut self, css: &str) -> Option<String> {
        let color = parse_color(css)?;
        self.set_stroke_style(Style::Color(color));
        Some(serialize_color(color))
    }

    #[must_use]
    pub fn shadow_color(&self) -> Color {
        self.state.shadow_color
    }

    pub fn set_shadow_color(&mut self, color: Color) {
        self.state.shadow_color = color;
    }

    pub fn set_shadow_color_css(&mut self, css: &str) -> Option<String> {
        let color = parse_color(css)?;
        self.set_shadow_color(color);
        Some(serialize_color(color))
    }

    #[must_use]
    pub fn shadow_blur(&self) -> f64 {
        self.state.shadow_blur
    }

    pub fn set_shadow_blur(&mut self, blur: f64) {
        if blur.is_finite() && blur >= 0.0 {
            self.state.shadow_blur = blur;
        }
    }

    #[must_use]
    pub fn shadow_offset_x(&self) -> f64 {
        self.state.shadow_offset_x
    }

    pub fn set_shadow_offset_x(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.shadow_offset_x = offset;
        }
    }

    #[must_use]
    pub fn shadow_offset_y(&self) -> f64 {
        self.state.shadow_offset_y
    }

    pub fn set_shadow_offset_y(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.shadow_offset_y = offset;
        }
    }

    #[must_use]
    pub fn line_width(&self) -> f64 {
        self.state.line_width
    }

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

    pub fn set_miter_limit(&mut self, limit: f64) {
        if limit.is_finite() && limit > 0.0 {
            self.state.miter_limit = limit;
        }
    }

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

    #[must_use]
    pub fn line_dash(&self) -> &[f64] {
        &self.state.line_dash
    }

    #[must_use]
    pub fn line_dash_offset(&self) -> f64 {
        self.state.line_dash_offset
    }

    pub fn set_line_dash_offset(&mut self, offset: f64) {
        if offset.is_finite() {
            self.state.line_dash_offset = offset;
        }
    }

    #[must_use]
    pub fn font(&self) -> &ResolvedFont {
        &self.state.font
    }

    pub fn set_font(&mut self, font: ResolvedFont) {
        self.state.font = Arc::new(font);
    }

    pub fn set_font_css(&mut self, css: &str) -> Option<String> {
        let parsed = parse_font(css)?;
        self.set_font(parsed.font);
        Some(parsed.css)
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

    pub fn begin_path(&mut self) {
        self.path.clear();
    }

    pub fn close_path(&mut self) {
        self.path.close_path();
    }

    fn default_path(&mut self) -> &mut Path {
        self.path
    }

    pub fn fill(&mut self, rule: FillRule) {
        let Some(raster) = &mut *self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Fill(&self.path.elements, rule.into());
        raster.draw(self.state, paint.as_ref(), Affine::IDENTITY, &shape);
        *self.changed = true;
    }

    pub fn fill_path(&mut self, path: &Path, rule: FillRule) {
        let Some(raster) = &mut *self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Fill(&path.elements, rule.into());
        raster.draw(self.state, paint.as_ref(), self.state.transform, &shape);
        *self.changed = true;
    }

    pub fn stroke(&mut self) {
        let Some(raster) = &mut *self.raster else {
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
        let stroke = self.state.stroke(|| path::length(&*self.scratch));
        let shape = Outline::Stroke(self.scratch, &stroke);
        raster.draw(self.state, paint.as_ref(), ctm, &shape);
        *self.changed = true;
    }

    pub fn stroke_path(&mut self, path: &Path) {
        let Some(raster) = &mut *self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.stroke_style, raster.size);
        let stroke = self.state.stroke(|| path::length(&path.elements));
        let shape = Outline::Stroke(&path.elements, &stroke);
        raster.draw(self.state, paint.as_ref(), self.state.transform, &shape);
        *self.changed = true;
    }

    pub fn clip(&mut self, rule: FillRule) {
        let path = self.path.elements.clone();
        self.push_clip(path, rule);
    }

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

    #[must_use]
    pub fn is_point_in_path(&self, x: f64, y: f64, rule: FillRule) -> bool {
        x.is_finite() && y.is_finite() && contains(&self.path.closed(), Point::new(x, y), rule)
    }

    #[must_use]
    pub fn is_point_in_path2d(&self, path: &Path, x: f64, y: f64, rule: FillRule) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let mut closed = path.closed();
        closed.apply_affine(self.state.transform);
        contains(&closed, Point::new(x, y), rule)
    }

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

    pub fn clear_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let Some(raster) = &mut *self.raster else {
            return;
        };
        *self.changed = true;
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

    pub fn fill_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || w == 0.0 || h == 0.0 {
            return;
        }
        let Some(raster) = &mut *self.raster else {
            return;
        };
        let paint = self.state.paint(&self.state.fill_style, raster.size);
        let shape = Outline::Rect(Rect::new(x, y, x + w, y + h).abs());
        raster.draw(self.state, paint.as_ref(), self.state.transform, &shape);
        *self.changed = true;
    }

    pub fn stroke_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        if ![x, y, w, h].iter().all(|v| v.is_finite()) || (w == 0.0 && h == 0.0) {
            return;
        }
        let Some(raster) = &mut *self.raster else {
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
        let stroke = self.state.stroke(|| path::length(&*self.scratch));
        let shape = Outline::Stroke(self.scratch, &stroke);
        raster.draw(self.state, paint.as_ref(), self.state.transform, &shape);
        *self.changed = true;
    }

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
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        if let Some(max) = max_width
            && (!max.is_finite() || max <= 0.0)
        {
            return;
        }
        let Some(raster) = &mut *self.raster else {
            return;
        };
        let state = &self.state;
        let line = Line::shape(context, text, &state.font, state.direction);
        let squeeze = max_width
            .filter(|max| line.width > *max)
            .map_or(1.0, |max| max / line.width);
        let anchor = line.width * text::align_fraction(state.text_align, state.direction);
        let rise = line.baseline_offset(state.text_baseline, f64::from(state.font.size));
        let geometry = state.transform
            * Affine::translate((x, y - rise))
            * Affine::scale_non_uniform(squeeze, 1.0)
            * Affine::translate((-anchor, 0.0));
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
        *self.changed = true;
    }

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

    pub fn draw_image(&mut self, image: &Image, dx: f64, dy: f64) -> Result<(), CanvasError> {
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        self.draw_image_subrect(image, 0.0, 0.0, w, h, dx, dy, w, h)
    }

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
        let Some(raster) = &mut *self.raster else {
            return Ok(());
        };
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
        *self.changed = true;
        Ok(())
    }

    pub fn get_image_data(
        &mut self,
        sx: i32,
        sy: i32,
        sw: i32,
        sh: i32,
    ) -> Result<Vec<u8>, CanvasError> {
        self.read_image_data(sx, sy, sw, sh)?.wait()
    }

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
        let Some(raster) = &mut *self.raster else {
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
        let Some(raster) = &mut *self.raster else {
            return Ok(());
        };
        raster.flush();
        *self.changed = true;
        let (bw, bh) = (i64::from(self.width), i64::from(self.height));
        let (dx, dy) = (i64::from(dx), i64::from(dy));
        let (cx0, cx1) = ((dx + x).max(0), (dx + x + w).min(bw));
        let (cy0, cy1) = ((dy + y).max(0), (dy + y + h).min(bh));
        if cx0 >= cx1 || cy0 >= cy1 {
            return Ok(());
        }
        let mut texels = Vec::with_capacity(index(0, (cx1 - cx0) * (cy1 - cy0), 0));
        for row in cy0..cy1 {
            let src = index(row - dy, cx0 - dx, iw);
            let src = src..src + index(0, cx1 - cx0, 0);
            texels.extend(
                data[src]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .copied()
                    .flat_map(premultiply),
            );
        }
        raster.surface.write(
            (to_u32(cx0), to_u32(cy0)),
            (to_u32(cx1 - cx0), to_u32(cy1 - cy0)),
            texels,
        );
        Ok(())
    }
}

builder_methods!(
    Context2d<'_>,
    |context: &Context2d<'_>| context.state.transform,
    default_path
);

fn contains(path: &BezPath, point: Point, rule: FillRule) -> bool {
    let winding = path.winding(point);
    let inside = match rule {
        FillRule::NonZero => winding != 0,
        FillRule::EvenOdd => winding % 2 != 0,
    };
    inside || on_path(path, point)
}

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
