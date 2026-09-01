//! Backgrounds: `background-color` and the `background-image` layer stack
//! (css-backgrounds-3), plus the shared CSS-gradient → peniko resolution
//! `mask.rs` reuses.
//!
//! Spec sketch (docs/tracking/css-visual.md is the authority for the fork's
//! grammar surface):
//! - Paint `background-color` first (bottom), clipped to the used `background-clip` box shape
//!   (`border-box`/`padding-box`/`content-box`; `text` clips through the glyph-silhouette `SrcIn`
//!   sandwich in [`text_clip_sandwich`]; Lynx's `border-area` is a recorded v1 limit — skip the
//!   layer. The authoritative support/limit matrix lives in the crate docs, `lib.rs`).
//! - Then image layers **last-specified first** (CSS lists the first layer topmost): for each
//!   non-`None` layer resolve origin box (`background-origin`), positioning area, `background-size`
//!   (`auto`/`cover`/`contain`/lengths), `background-position`, and `background-repeat` per axis.
//!   `url(…)` layers look up [`ImageStore::peek`] (missing, zero-area, or past the atlas bound →
//!   skip); gradients resolve via [`gradient_brush`].
//! - Repeat via `peniko::Extend::Repeat` on the image sampler where the tile grid is uniform;
//!   gradients restart per tile, so when more than one tile is visible they are drawn as an
//!   explicit tile loop. `space` is approximated as `repeat` (recorded v1 limit) and `round`
//!   rescales the tile so a whole number fits.
//! - Every draw is clipped to the `background-clip` shape — directly (the shape is the fill), by
//!   rect intersection, or via `scene.push_clip_layer` + fill. Only plain fills ever go inside a
//!   clip layer (vello #1198 forbids blend layers there, not fills).
//! - `background-attachment` does not exist in the fork's grammar (Lynx has no viewport-fixed
//!   backgrounds); layers are always origin-box anchored.

use std::f64::consts::{FRAC_PI_2, PI, SQRT_2, TAU};

use stylo::computed_values::background_origin::single_value::T as BackgroundOrigin;
use stylo::computed_values::object_fit::T as ObjectFit;
use stylo::properties::ComputedValues;
use stylo::url::ComputedUrl;
use stylo::values::computed::image::EndingShape;
use stylo::values::computed::{
    Angle, AngleOrPercentage, BackgroundClip, BackgroundRepeat, BackgroundSize,
    Color as StyloColor, Gradient, Image, Length, LengthPercentage, LineDirection, Position,
};
use stylo::values::generics::NonNegative;
use stylo::values::generics::image::{
    Circle, Ellipse, GenericGradientItem, GradientCompatMode, GradientFlags, ShapeExtent,
};
use stylo::values::generics::length::LengthPercentageOrAuto;
use stylo::values::specified::background::BackgroundRepeatKeyword;
use stylo::values::specified::position::{HorizontalPositionKeyword, VerticalPositionKeyword};

use crate::layout::NaturalSize;
use crate::paint::compose::{ComposeChain, ImageArea, ImageDraw};
use crate::paint::convert::resolve_color;
use crate::paint::shape::{BoxShape, inner_radii, with_shape};
use crate::paint::walker::WalkSink;
use crate::paint::{BoxFragment, TextClip};
use crate::render::image::{ImageRef, ImageRegistry};
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect, Size, Vec2};
use crate::vello::peniko::{self, BrushRef, Color, Extend, Fill, ImageQuality, ImageSampler};

const EPS: f64 = 1e-6;

const MAX_TILES_PER_AXIS: f64 = 4096.0;
const MAX_TILE_FILLS: f64 = 16384.0;

pub(crate) fn needs_text_clip(style: &ComputedValues) -> bool {
    style
        .get_background()
        .background_clip
        .0
        .iter()
        .any(|clip| matches!(clip, BackgroundClip::Text))
}

pub(crate) fn paint(
    sink: &mut WalkSink<'_>,
    chain: ComposeChain,
    style: &ComputedValues,
    fragment: &BoxFragment,
    images: &ImageRegistry,
    text_clip: Option<&TextClip<'_>>,
) {
    let scene = sink.scene_for(chain);
    let background = style.get_background();
    let layers = background.background_image.0.as_slice();

    let color = resolve_color(style, &background.background_color);
    if color.components[3] > 0.0 {
        let clip = *cycled(
            &background.background_clip.0,
            layers.len().saturating_sub(1),
        );
        match used_clip(fragment, clip) {
            Some(UsedClip::Shape(shape)) => {
                let bounds = shape.bounding_box();
                if bounds.width() > 0.0 && bounds.height() > 0.0 {
                    with_shape!(&shape, |s| scene.fill(
                        Fill::NonZero,
                        fragment.transform,
                        color,
                        None,
                        s
                    ));
                }
            }
            Some(UsedClip::Text) => {
                text_clip_sandwich(scene, fragment, text_clip, |scene| {
                    let shape = level_shape(fragment, BoxLevel::Border);
                    with_shape!(&shape, |s| scene.fill(
                        Fill::NonZero,
                        fragment.transform,
                        color,
                        None,
                        s
                    ));
                });
            }
            None => {}
        }
    }

    for index in (0..layers.len()).rev() {
        let image = &layers[index];
        if matches!(image, Image::None) {
            continue;
        }
        let Some(clip) = used_clip(fragment, *cycled(&background.background_clip.0, index)) else {
            continue;
        };
        let make_layer = |clip: BoxShape| PatternLayer {
            image,
            position_x: cycled(&background.background_position_x.0, index),
            position_y: cycled(&background.background_position_y.0, index),
            size: cycled(&background.background_size.0, index),
            repeat: cycled(&background.background_repeat.0, index),
            origin: level_rect(
                fragment,
                origin_level(*cycled(&background.background_origin.0, index)),
            ),
            clip,
        };
        match clip {
            UsedClip::Shape(shape) => {
                let layer = make_layer(shape);
                paint_pattern_layer(sink, chain, style, fragment, images, &layer, None);
            }
            UsedClip::Text => {
                let layer = make_layer(level_shape(fragment, BoxLevel::Border));
                paint_pattern_layer(sink, chain, style, fragment, images, &layer, text_clip);
            }
        }
    }
}

/// Opens the `background-clip: text` sandwich as program ops rather than
/// inside one fragment, so an image fill can sit between the two pushes.
///
/// The op sequence is exactly what the inline sandwich encodes — a `SrcOver`
/// group over the border shape, the glyph silhouettes, then a `SrcIn` group
/// over the border box — but expressed so a fragment cut may legally land in
/// the middle of it. Returns whether it opened; a caller that gets `true`
/// owes two pops.
fn open_text_clip_ops(
    sink: &mut WalkSink<'_>,
    chain: ComposeChain,
    fragment: &BoxFragment,
    text_clip: Option<&TextClip<'_>>,
) -> bool {
    let Some(text_clip) = text_clip.filter(|clip| !clip.is_empty()) else {
        return false;
    };
    sink.push_layer_box(
        chain,
        Fill::NonZero,
        peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
        1.0,
        fragment.transform,
        level_shape(fragment, BoxLevel::Border),
    );
    let scene = sink.scene_for(chain);
    for (offset, layout) in &text_clip.runs {
        crate::paint::text::paint_silhouette(
            scene,
            layout,
            fragment.transform * Affine::translate(*offset),
        );
    }
    sink.push_layer_rect(
        chain,
        None,
        Fill::NonZero,
        peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcIn),
        1.0,
        fragment.transform,
        fragment.border_box,
    );
    true
}

fn text_clip_sandwich(
    scene: &mut Scene,
    fragment: &BoxFragment,
    text_clip: Option<&TextClip<'_>>,
    f: impl FnOnce(&mut Scene),
) {
    let Some(text_clip) = text_clip.filter(|clip| !clip.is_empty()) else {
        return;
    };
    let border_shape = level_shape(fragment, BoxLevel::Border);
    with_shape!(&border_shape, |s| scene.push_layer(
        Fill::NonZero,
        peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcOver),
        1.0,
        fragment.transform,
        s,
    ));
    for (offset, layout) in &text_clip.runs {
        crate::paint::text::paint_silhouette(
            scene,
            layout,
            fragment.transform * Affine::translate(*offset),
        );
    }
    scene.push_layer(
        Fill::NonZero,
        peniko::BlendMode::new(peniko::Mix::Normal, peniko::Compose::SrcIn),
        1.0,
        fragment.transform,
        &fragment.border_box,
    );
    f(scene);
    scene.pop_layer();
    scene.pop_layer();
}

pub(crate) fn paint_replaced_content(
    sink: &mut WalkSink<'_>,
    chain: ComposeChain,
    style: &ComputedValues,
    fragment: &BoxFragment,
    images: &ImageRegistry,
    source: &str,
    natural: NaturalSize,
) {
    let Some((image, _)) = images.resolve(source) else {
        return;
    };
    let content = fragment.content_box;
    if content.width() <= 0.0 || content.height() <= 0.0 {
        return;
    }

    let position = &style.get_position();
    let (object_width, object_height) = concrete_object_size(natural, content);
    let (draw_width, draw_height) =
        fitted_size(position.object_fit, object_width, object_height, content);
    if draw_width <= 0.0 || draw_height <= 0.0 {
        return;
    }
    let destination = Rect::from_origin_size(
        (
            content.x0
                + position_offset(
                    &position.object_position.horizontal,
                    content.width(),
                    draw_width,
                ),
            content.y0
                + position_offset(
                    &position.object_position.vertical,
                    content.height(),
                    draw_height,
                ),
        ),
        (draw_width, draw_height),
    );

    let shape = level_shape(fragment, BoxLevel::Content);
    let Some(area) = image_area(&shape, destination) else {
        return;
    };
    // The destination comes from the node's natural size and the brush scale
    // from the decoded bitmap. The two may disagree — a store decoding at
    // reduced scale, or a generation whose dimensions changed — and the image
    // stretches, which is the same asymmetry the inline path had.
    sink.image(
        chain,
        ImageDraw {
            image,
            transform: fragment.transform,
            anchor: destination.origin(),
            extent: destination.size(),
            sampler: ImageSampler::default().with_quality(image_quality(style)),
            area,
        },
    );
}

fn concrete_object_size(natural: NaturalSize, content: Rect) -> (f64, f64) {
    let dimensions = natural.dimensions();
    let width = dimensions.width.map(f64::from).filter(|value| *value > 0.0);
    let height = dimensions
        .height
        .map(f64::from)
        .filter(|value| *value > 0.0);
    let ratio = natural
        .aspect_ratio()
        .map(f64::from)
        .filter(|value| *value > 0.0 && value.is_finite());

    match (width, height, ratio) {
        (Some(width), Some(height), _) => (width, height),
        (Some(width), None, Some(ratio)) => (width, width / ratio),
        (None, Some(height), Some(ratio)) => (height * ratio, height),
        (None, None, Some(ratio)) => {
            let by_width = (content.width(), content.width() / ratio);
            if by_width.1 <= content.height() {
                by_width
            } else {
                (content.height() * ratio, content.height())
            }
        }
        (Some(width), None, None) => (width, content.height()),
        (None, Some(height), None) => (content.width(), height),
        (None, None, None) => (content.width(), content.height()),
    }
}

fn fitted_size(fit: ObjectFit, width: f64, height: f64, content: Rect) -> (f64, f64) {
    let (available_width, available_height) = (content.width(), content.height());
    if width <= 0.0 || height <= 0.0 {
        return (0.0, 0.0);
    }
    let uniform = |scale: f64| (width * scale, height * scale);
    match fit {
        ObjectFit::Fill => (available_width, available_height),
        ObjectFit::Contain => uniform((available_width / width).min(available_height / height)),
        ObjectFit::Cover => uniform((available_width / width).max(available_height / height)),
        ObjectFit::None => (width, height),
        ObjectFit::ScaleDown => uniform(
            (available_width / width)
                .min(available_height / height)
                .min(1.0),
        ),
    }
}

/// One fully specified image layer (background or mask), ready to resolve
/// and tile. `origin` is the positioning area; `clip` bounds every draw.
pub(super) struct PatternLayer<'a> {
    pub image: &'a Image,
    pub position_x: &'a LengthPercentage,
    pub position_y: &'a LengthPercentage,
    pub size: &'a BackgroundSize,
    pub repeat: &'a BackgroundRepeat,
    pub origin: Rect,
    pub clip: BoxShape,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BoxLevel {
    Border,
    Padding,
    Content,
}

pub(super) fn cycled<T>(list: &[T], index: usize) -> &T {
    &list[index % list.len()]
}

pub(super) fn level_rect(fragment: &BoxFragment, level: BoxLevel) -> Rect {
    match level {
        BoxLevel::Border => fragment.border_box,
        BoxLevel::Padding => fragment.padding_box,
        BoxLevel::Content => fragment.content_box,
    }
}

pub(super) fn level_shape(fragment: &BoxFragment, level: BoxLevel) -> BoxShape {
    match level {
        BoxLevel::Border => BoxShape::new(fragment.border_box, &fragment.radii),
        BoxLevel::Padding => BoxShape::new(
            fragment.padding_box,
            &inner_radii(&fragment.radii, &fragment.border_widths),
        ),
        BoxLevel::Content => {
            let padding = inner_radii(&fragment.radii, &fragment.border_widths);
            BoxShape::new(
                fragment.content_box,
                &inner_radii(&padding, &fragment.padding_widths),
            )
        }
    }
}

pub(super) fn paint_pattern_layer(
    sink: &mut WalkSink<'_>,
    chain: ComposeChain,
    style: &ComputedValues,
    fragment: &BoxFragment,
    images: &ImageRegistry,
    layer: &PatternLayer<'_>,
    text_clip: Option<&TextClip<'_>>,
) {
    let clip_bounds = layer.clip.bounding_box();
    if clip_bounds.width() <= 0.0 || clip_bounds.height() <= 0.0 {
        return;
    }
    let Some(source) = resolve_source(style, images, layer.image) else {
        return;
    };
    let intrinsic = match &source {
        Source::Raster(_, intrinsic) => Some(*intrinsic),
        Source::Gradient(_) | Source::Solid(_) => None,
    };
    let area = layer.origin;
    let (mut tile_w, mut tile_h) = tile_size(layer.size, (area.width(), area.height()), intrinsic);
    if matches!(layer.repeat.0, BackgroundRepeatKeyword::Round) {
        tile_w = round_tile(area.width(), tile_w);
    }
    if matches!(layer.repeat.1, BackgroundRepeatKeyword::Round) {
        tile_h = round_tile(area.height(), tile_h);
    }
    if tile_w <= EPS || tile_h <= EPS {
        return;
    }
    let grid = TileGrid {
        origin: Point::new(
            area.x0 + position_offset(layer.position_x, area.width(), tile_w),
            area.y0 + position_offset(layer.position_y, area.height(), tile_h),
        ),
        tile: Size::new(tile_w, tile_h),
        repeat_x: !matches!(layer.repeat.0, BackgroundRepeatKeyword::NoRepeat),
        repeat_y: !matches!(layer.repeat.1, BackgroundRepeatKeyword::NoRepeat),
    };

    // A raster layer becomes a program op, because its pixels are not
    // reachable from here. Everything else still encodes inline.
    if let Source::Raster(image, _) = &source {
        let Some(area) = image_area(&layer.clip, grid.draw_rect(clip_bounds)) else {
            return;
        };
        let opened = open_text_clip_ops(sink, chain, fragment, text_clip);
        if text_clip.is_some() && !opened {
            return;
        }
        sink.image(
            chain,
            ImageDraw {
                image: *image,
                transform: fragment.transform,
                anchor: grid.origin,
                extent: grid.tile,
                sampler: ImageSampler {
                    x_extend: extend_for(grid.repeat_x),
                    y_extend: extend_for(grid.repeat_y),
                    quality: image_quality(style),
                    alpha: 1.0,
                },
                area,
            },
        );
        if opened {
            sink.pop();
            sink.pop();
        }
        return;
    }

    let scene = sink.scene_for(chain);
    let inline = |scene: &mut Scene| match &source {
        Source::Raster(..) => unreachable!("the raster case returned above"),
        Source::Solid(color) => fill_area(
            scene,
            fragment.transform,
            &layer.clip,
            grid.draw_rect(clip_bounds),
            BrushRef::Solid(*color),
            None,
        ),
        Source::Gradient(gradient) => match gradient_brush(style, gradient, grid.tile) {
            Some(GradientBrush::Solid(color)) if color.components[3] > 0.0 => fill_area(
                scene,
                fragment.transform,
                &layer.clip,
                grid.draw_rect(clip_bounds),
                BrushRef::Solid(color),
                None,
            ),
            None | Some(GradientBrush::Solid(_)) => {}
            Some(GradientBrush::Gradient { gradient, local }) => {
                fill_gradient_tiles(
                    scene,
                    fragment.transform,
                    &layer.clip,
                    &grid,
                    &gradient,
                    local,
                );
            }
        },
    };
    // Gradients and solids keep the inline sandwich: they carry no late
    // input, so nothing needs to cut the fragment around them.
    if text_clip.is_some() {
        text_clip_sandwich(scene, fragment, text_clip, inline);
    } else {
        inline(scene);
    }
}

enum UsedClip {
    Shape(BoxShape),
    Text,
}

fn used_clip(fragment: &BoxFragment, clip: BackgroundClip) -> Option<UsedClip> {
    match clip {
        BackgroundClip::BorderBox => Some(UsedClip::Shape(level_shape(fragment, BoxLevel::Border))),
        BackgroundClip::PaddingBox => {
            Some(UsedClip::Shape(level_shape(fragment, BoxLevel::Padding)))
        }
        BackgroundClip::ContentBox => {
            Some(UsedClip::Shape(level_shape(fragment, BoxLevel::Content)))
        }
        BackgroundClip::Text => Some(UsedClip::Text),
        BackgroundClip::BorderArea => None,
    }
}

fn origin_level(origin: BackgroundOrigin) -> BoxLevel {
    match origin {
        BackgroundOrigin::BorderBox => BoxLevel::Border,
        BackgroundOrigin::PaddingBox => BoxLevel::Padding,
        BackgroundOrigin::ContentBox => BoxLevel::Content,
    }
}

fn extend_for(repeat: bool) -> Extend {
    if repeat { Extend::Repeat } else { Extend::Pad }
}

fn image_url(url: &ComputedUrl) -> &str {
    match url {
        ComputedUrl::Valid(url) => url.as_str(),
        ComputedUrl::Invalid(original) => original.as_str(),
    }
}

enum Source<'a> {
    /// The image's name and its own intrinsic dimensions — never its pixels.
    /// The dimensions come from the registry, which learned them from the
    /// store's load report, so the whole tile grid resolves here without any
    /// bitmap being reachable.
    Raster(ImageRef, (f64, f64)),
    Gradient(&'a Gradient),
    Solid(Color),
}

fn resolve_source<'a>(
    style: &ComputedValues,
    images: &ImageRegistry,
    image: &'a Image,
) -> Option<Source<'a>> {
    match image {
        Image::Url(url) => images
            .resolve(image_url(url))
            .map(|(reference, intrinsic)| Source::Raster(reference, intrinsic)),
        Image::Gradient(gradient) => Some(Source::Gradient(gradient)),
        Image::Image(color) => {
            let color = resolve_color(style, color);
            (color.components[3] > 0.0).then_some(Source::Solid(color))
        }
        _ => None,
    }
}

fn image_quality(style: &ComputedValues) -> ImageQuality {
    use stylo::values::computed::ImageRendering;
    match style.get_inherited_box().image_rendering {
        ImageRendering::Auto => ImageQuality::Medium,
        ImageRendering::CrispEdges | ImageRendering::Pixelated => ImageQuality::Low,
    }
}

/// The layer's resolved tile grid: first-tile origin, tile size, and
/// per-axis repetition, all in item-local px.
struct TileGrid {
    origin: Point,
    tile: Size,
    repeat_x: bool,
    repeat_y: bool,
}

impl TileGrid {
    fn draw_rect(&self, clip_bounds: Rect) -> Rect {
        let (x0, x1) = if self.repeat_x {
            (clip_bounds.x0, clip_bounds.x1)
        } else {
            (self.origin.x, self.origin.x + self.tile.width)
        };
        let (y0, y1) = if self.repeat_y {
            (clip_bounds.y0, clip_bounds.y1)
        } else {
            (self.origin.y, self.origin.y + self.tile.height)
        };
        Rect::new(x0, y0, x1, y1)
    }

    fn tile_rect(&self, ix: f64, iy: f64) -> Rect {
        Rect::from_origin_size(
            (
                self.origin.x + ix * self.tile.width,
                self.origin.y + iy * self.tile.height,
            ),
            self.tile,
        )
    }
}

/// Which shape a clipped fill resolves to, decided without looking at what
/// is being painted.
///
/// Every input is geometry: the clip shape and the rect to be covered. No
/// branch reads a brush, a gradient or a pixel — which is what lets an image
/// fill be described on one thread and encoded on another, with the pixels
/// supplied only at encode time.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum FillPlan {
    /// Degenerate: a zero-area draw, or a clip the draw misses entirely.
    /// Nothing is painted.
    None,
    /// The draw covers the whole clip, so the clip shape is filled directly.
    Shape,
    /// A rectangular clip the draw only partly covers: fill the
    /// intersection, no layer needed.
    Rect(Rect),
    /// A rounded clip the draw only partly covers: the fill needs a real
    /// clip layer around it.
    Clipped(Rect),
}

/// The shape a fill of `draw` inside `clip` resolves to.
pub(super) fn fill_plan(clip: &BoxShape, draw: Rect) -> FillPlan {
    if draw.width() <= 0.0 || draw.height() <= 0.0 {
        return FillPlan::None;
    }
    let bounds = clip.bounding_box();
    let covers = draw.x0 <= bounds.x0 + EPS
        && draw.y0 <= bounds.y0 + EPS
        && draw.x1 >= bounds.x1 - EPS
        && draw.y1 >= bounds.y1 - EPS;
    if covers {
        FillPlan::Shape
    } else if let BoxShape::Rect(rect) = clip {
        let both = rect.intersect(draw);
        if both.width() > 0.0 && both.height() > 0.0 {
            FillPlan::Rect(both)
        } else {
            FillPlan::None
        }
    } else {
        FillPlan::Clipped(draw)
    }
}

/// The [`ImageArea`] a raster fill of `draw` inside `clip` resolves to, or
/// `None` when the plan paints nothing at all.
fn image_area(clip: &BoxShape, draw: Rect) -> Option<ImageArea> {
    match fill_plan(clip, draw) {
        FillPlan::None => None,
        FillPlan::Shape => Some(ImageArea::Fill(crate::paint::compose::CapturedShape::Box(
            clip.clone(),
        ))),
        FillPlan::Rect(both) => Some(ImageArea::Fill(crate::paint::compose::CapturedShape::Rect(
            both,
        ))),
        FillPlan::Clipped(draw) => Some(ImageArea::Clipped {
            clip: clip.clone(),
            draw,
        }),
    }
}

fn fill_area(
    scene: &mut Scene,
    transform: Affine,
    clip: &BoxShape,
    draw: Rect,
    brush: BrushRef<'_>,
    brush_transform: Option<Affine>,
) {
    match fill_plan(clip, draw) {
        FillPlan::None => {}
        FillPlan::Shape => {
            with_shape!(clip, |s| scene.fill(
                Fill::NonZero,
                transform,
                brush,
                brush_transform,
                s
            ));
        }
        FillPlan::Rect(both) => {
            scene.fill(Fill::NonZero, transform, brush, brush_transform, &both);
        }
        FillPlan::Clipped(draw) => {
            with_shape!(clip, |s| scene.push_clip_layer(Fill::NonZero, transform, s));
            scene.fill(Fill::NonZero, transform, brush, brush_transform, &draw);
            scene.pop_layer();
        }
    }
}

fn fill_gradient_tiles(
    scene: &mut Scene,
    transform: Affine,
    clip: &BoxShape,
    grid: &TileGrid,
    gradient: &peniko::Gradient,
    local: Affine,
) {
    let bounds = clip.bounding_box();
    let (x_first, x_count) = tile_span(
        grid.origin.x,
        grid.tile.width,
        bounds.x0,
        bounds.x1,
        grid.repeat_x,
    );
    let (y_first, mut y_count) = tile_span(
        grid.origin.y,
        grid.tile.height,
        bounds.y0,
        bounds.y1,
        grid.repeat_y,
    );
    if x_count <= 0.0 || y_count <= 0.0 {
        return;
    }
    if x_count <= 1.0 && y_count <= 1.0 {
        let tile = grid.tile_rect(x_first, y_first);
        let brush_transform = Affine::translate(tile.origin().to_vec2()) * local;
        fill_area(
            scene,
            transform,
            clip,
            tile,
            BrushRef::Gradient(gradient),
            Some(brush_transform),
        );
        return;
    }
    if x_count * y_count > MAX_TILE_FILLS {
        y_count = (MAX_TILE_FILLS / x_count).floor().max(1.0);
    }
    let rect_clip = if let BoxShape::Rect(rect) = clip {
        Some(*rect)
    } else {
        None
    };
    if rect_clip.is_none() {
        with_shape!(clip, |s| scene.push_clip_layer(Fill::NonZero, transform, s));
    }
    let mut iy = 0.0;
    while iy < y_count {
        let mut ix = 0.0;
        while ix < x_count {
            let tile = grid.tile_rect(x_first + ix, y_first + iy);
            let brush_transform = Some(Affine::translate(tile.origin().to_vec2()) * local);
            match rect_clip {
                Some(rect) => {
                    let draw = rect.intersect(tile);
                    if draw.width() > 0.0 && draw.height() > 0.0 {
                        scene.fill(Fill::NonZero, transform, gradient, brush_transform, &draw);
                    }
                }
                None => scene.fill(Fill::NonZero, transform, gradient, brush_transform, &tile),
            }
            ix += 1.0;
        }
        iy += 1.0;
    }
    if rect_clip.is_none() {
        scene.pop_layer();
    }
}

fn tile_span(origin: f64, step: f64, lo: f64, hi: f64, repeat: bool) -> (f64, f64) {
    if !repeat {
        return (0.0, 1.0);
    }
    let first = ((lo - origin) / step).floor();
    let count = (((hi - origin) / step).ceil() - first).max(0.0);
    (first, count.min(MAX_TILES_PER_AXIS))
}

pub(crate) enum GradientBrush {
    Gradient {
        gradient: peniko::Gradient,
        local: Affine,
    },
    Solid(Color),
}

pub(crate) fn gradient_brush(
    style: &ComputedValues,
    gradient: &Gradient,
    tile: Size,
) -> Option<GradientBrush> {
    match gradient {
        Gradient::Linear {
            direction,
            items,
            flags,
            compat_mode,
            ..
        } => linear_gradient(
            style,
            *direction,
            *compat_mode,
            items,
            flags.contains(GradientFlags::REPEATING),
            tile,
        ),
        Gradient::Radial {
            shape,
            position,
            items,
            flags,
            ..
        } => radial_gradient(
            style,
            shape,
            position,
            items,
            flags.contains(GradientFlags::REPEATING),
            tile,
        ),
        Gradient::Conic {
            angle,
            position,
            items,
            flags,
            ..
        } => conic_gradient(
            style,
            *angle,
            position,
            items,
            flags.contains(GradientFlags::REPEATING),
            tile,
        ),
    }
}

fn linear_gradient(
    style: &ComputedValues,
    direction: LineDirection,
    compat_mode: GradientCompatMode,
    items: &[GenericGradientItem<StyloColor, LengthPercentage>],
    repeating: bool,
    tile: Size,
) -> Option<GradientBrush> {
    let angle = direction_angle(direction, compat_mode, tile.width, tile.height);
    let (start, end) = linear_endpoints(angle, tile.width, tile.height);
    let length = (end - start).hypot();
    if length <= EPS {
        return None;
    }
    let stops = fixup_stops(&gradient_stops(
        style,
        items,
        |position: &LengthPercentage| {
            f64::from(position.resolve(Length::new(length as f32)).px()) / length
        },
    ));
    let &(first, first_color) = stops.first()?;
    let &(last, last_color) = stops.last()?;
    if stops.len() == 1 {
        return Some(GradientBrush::Solid(first_color));
    }
    if repeating || first < 0.0 || last > 1.0 {
        let span = last - first;
        if span <= EPS {
            let color = if !repeating && first > 1.0 {
                first_color
            } else {
                last_color
            };
            return Some(GradientBrush::Solid(color));
        }
        let remapped: Vec<(f32, Color)> = stops
            .iter()
            .map(|&(p, c)| (((p - first) / span) as f32, c))
            .collect();
        let gradient = peniko::Gradient::new_linear(start.lerp(end, first), start.lerp(end, last))
            .with_stops(remapped.as_slice())
            .with_extend(if repeating {
                Extend::Repeat
            } else {
                Extend::Pad
            });
        return Some(GradientBrush::Gradient {
            gradient,
            local: Affine::IDENTITY,
        });
    }
    let plain: Vec<(f32, Color)> = stops.iter().map(|&(p, c)| (p as f32, c)).collect();
    Some(GradientBrush::Gradient {
        gradient: peniko::Gradient::new_linear(start, end).with_stops(plain.as_slice()),
        local: Affine::IDENTITY,
    })
}

fn radial_gradient(
    style: &ComputedValues,
    shape: &EndingShape,
    position: &Position,
    items: &[GenericGradientItem<StyloColor, LengthPercentage>],
    repeating: bool,
    tile: Size,
) -> Option<GradientBrush> {
    let center = Point::new(
        f64::from(
            position
                .horizontal
                .resolve(Length::new(tile.width as f32))
                .px(),
        ),
        f64::from(
            position
                .vertical
                .resolve(Length::new(tile.height as f32))
                .px(),
        ),
    );
    let (rx, ry) = radial_radii(shape, center, tile);
    let stop_basis = rx.max(EPS);
    let stops = fixup_stops(&gradient_stops(
        style,
        items,
        |position: &LengthPercentage| {
            f64::from(position.resolve(Length::new(stop_basis as f32)).px()) / stop_basis
        },
    ));
    let &(first, _) = stops.first()?;
    let &(last, last_color) = stops.last()?;
    if stops.len() == 1 {
        return Some(GradientBrush::Solid(last_color));
    }
    if rx <= EPS || ry <= EPS {
        return Some(GradientBrush::Solid(last_color));
    }
    let local = if (rx - ry).abs() > EPS {
        Affine::translate(center.to_vec2())
            * Affine::scale_non_uniform(1.0, ry / rx)
            * Affine::translate(-center.to_vec2())
    } else {
        Affine::IDENTITY
    };
    if repeating {
        let span = last - first;
        if span <= EPS {
            return Some(GradientBrush::Solid(last_color));
        }
        let shift = (-first / span).ceil().max(0.0) * span;
        let (first, last) = (first + shift, last + shift);
        let remapped: Vec<(f32, Color)> = stops
            .iter()
            .map(|&(p, c)| ((((p + shift) - first) / span) as f32, c))
            .collect();
        let gradient = peniko::Gradient::new_two_point_radial(
            center,
            (stop_basis * first) as f32,
            center,
            (stop_basis * last) as f32,
        )
        .with_stops(remapped.as_slice())
        .with_extend(Extend::Repeat);
        return Some(GradientBrush::Gradient { gradient, local });
    }
    let stops = synthesize_zero_radius_stop(stops);
    if stops.len() == 1 {
        return Some(GradientBrush::Solid(stops[0].1));
    }
    let extent = last.max(1.0);
    let scaled: Vec<(f32, Color)> = stops
        .iter()
        .map(|&(p, c)| ((p / extent) as f32, c))
        .collect();
    let gradient = peniko::Gradient::new_radial(center, (stop_basis * extent) as f32)
        .with_stops(scaled.as_slice());
    Some(GradientBrush::Gradient { gradient, local })
}

fn synthesize_zero_radius_stop(stops: Vec<(f64, Color)>) -> Vec<(f64, Color)> {
    let Some(cross) = stops.iter().position(|&(position, _)| position >= 0.0) else {
        let &(_, last_color) = stops.last().expect("caller checked non-empty");
        return vec![(0.0, last_color)];
    };
    if cross == 0 {
        return stops;
    }
    let (before, before_color) = stops[cross - 1];
    let (after, after_color) = stops[cross];
    let t = -before / (after - before);
    let mut out = Vec::with_capacity(stops.len() - cross + 1);
    out.push((0.0, lerp_srgb(before_color, after_color, t)));
    out.extend_from_slice(&stops[cross..]);
    out
}

fn lerp_srgb(a: Color, b: Color, t: f64) -> Color {
    let t = t as f32;
    let mut components = a.components;
    for (component, &target) in components.iter_mut().zip(b.components.iter()) {
        *component += (target - *component) * t;
    }
    Color::new(components)
}

fn conic_gradient(
    style: &ComputedValues,
    angle: Angle,
    position: &Position,
    items: &[GenericGradientItem<StyloColor, AngleOrPercentage>],
    repeating: bool,
    tile: Size,
) -> Option<GradientBrush> {
    let center = Point::new(
        f64::from(
            position
                .horizontal
                .resolve(Length::new(tile.width as f32))
                .px(),
        ),
        f64::from(
            position
                .vertical
                .resolve(Length::new(tile.height as f32))
                .px(),
        ),
    );
    let base = angle.radians64() - FRAC_PI_2;
    let stops = fixup_stops(&gradient_stops(
        style,
        items,
        |position: &AngleOrPercentage| match *position {
            AngleOrPercentage::Percentage(percentage) => f64::from(percentage.0),
            AngleOrPercentage::Angle(angle) => f64::from(angle.degrees()) / 360.0,
        },
    ));
    let &(_, last_color) = stops.last()?;
    if stops.len() == 1 {
        return Some(GradientBrush::Solid(last_color));
    }
    let stops = if repeating {
        match unroll_conic_period(&stops) {
            Some(unrolled) => unrolled,
            None => return Some(GradientBrush::Solid(last_color)),
        }
    } else {
        stops
    };
    let window = conic_window_stops(&stops);
    let gradient = peniko::Gradient::new_sweep(center, base as f32, (base + TAU) as f32)
        .with_stops(window.as_slice())
        .with_extend(Extend::Repeat);
    Some(GradientBrush::Gradient {
        gradient,
        local: Affine::IDENTITY,
    })
}

fn gradient_stops<T>(
    style: &ComputedValues,
    items: &[GenericGradientItem<StyloColor, T>],
    mut resolve: impl FnMut(&T) -> f64,
) -> Vec<(Option<f64>, Color)> {
    let mut stops = Vec::with_capacity(items.len());
    for item in items {
        match item {
            GenericGradientItem::SimpleColorStop(color) => {
                stops.push((None, resolve_color(style, color)));
            }
            GenericGradientItem::ComplexColorStop { color, position } => {
                stops.push((Some(resolve(position)), resolve_color(style, color)));
            }
            GenericGradientItem::InterpolationHint(_) => {}
        }
    }
    stops
}

fn fixup_stops(raw: &[(Option<f64>, Color)]) -> Vec<(f64, Color)> {
    let positions: Vec<Option<f64>> = raw.iter().map(|&(position, _)| position).collect();
    let offsets = resolve_stop_offsets(&positions);
    offsets
        .into_iter()
        .zip(raw.iter().map(|&(_, color)| color))
        .collect()
}

fn resolve_stop_offsets(positions: &[Option<f64>]) -> Vec<f64> {
    let mut resolved: Vec<Option<f64>> = positions.to_vec();
    let Some(first) = resolved.first_mut() else {
        return Vec::new();
    };
    first.get_or_insert(0.0);
    resolved
        .last_mut()
        .expect("non-empty by the check above")
        .get_or_insert(1.0);
    let mut running = f64::NEG_INFINITY;
    for position in resolved.iter_mut().flatten() {
        *position = position.max(running);
        running = *position;
    }
    let mut out = vec![0.0; resolved.len()];
    let mut index = 0;
    while index < resolved.len() {
        if let Some(position) = resolved[index] {
            out[index] = position;
            index += 1;
        } else {
            let run_start = index;
            let mut run_end = index;
            while resolved[run_end].is_none() {
                run_end += 1;
            }
            let before = out[run_start - 1];
            let after = resolved[run_end].expect("runs end at a positioned stop");
            let segments = small_f64(run_end - run_start + 1);
            for (step, slot) in out[run_start..run_end].iter_mut().enumerate() {
                *slot = before + (after - before) * small_f64(step + 1) / segments;
            }
            index = run_end;
        }
    }
    out
}

fn unroll_conic_period(stops: &[(f64, Color)]) -> Option<Vec<(f64, Color)>> {
    let first = stops.first()?.0;
    let last = stops[stops.len() - 1].0;
    let span = last - first;
    if span <= EPS {
        return None;
    }
    let k_min = (-last / span).ceil();
    let k_max = ((1.0 - first) / span).floor();
    let copies = k_max - k_min + 1.0;
    if copies <= 0.0 || copies * small_f64(stops.len()) > 512.0 {
        return None;
    }
    let mut out = Vec::new();
    let mut k = k_min;
    while k <= k_max {
        for &(position, color) in stops {
            out.push((position + k * span, color));
        }
        k += 1.0;
    }
    Some(out)
}

fn conic_window_stops(stops: &[(f64, Color)]) -> Vec<(f32, Color)> {
    let mut out = Vec::with_capacity(stops.len() + 2);
    if let (Some(&(first, first_color)), Some(&(last, last_color))) = (stops.first(), stops.last())
    {
        if first > 0.0 {
            out.push((0.0, first_color));
        }
        for &(position, color) in stops {
            out.push((position.clamp(0.0, 1.0) as f32, color));
        }
        if last < 1.0 {
            out.push((1.0, last_color));
        }
    }
    out
}

fn direction_angle(
    direction: LineDirection,
    compat_mode: GradientCompatMode,
    width: f64,
    height: f64,
) -> f64 {
    let flip = if matches!(compat_mode, GradientCompatMode::Modern) {
        0.0
    } else {
        PI
    };
    match direction {
        LineDirection::Angle(angle) => angle.radians64(),
        LineDirection::Horizontal(HorizontalPositionKeyword::Left) => -FRAC_PI_2 + flip,
        LineDirection::Horizontal(HorizontalPositionKeyword::Right) => FRAC_PI_2 + flip,
        LineDirection::Vertical(VerticalPositionKeyword::Top) => flip,
        LineDirection::Vertical(VerticalPositionKeyword::Bottom) => PI + flip,
        LineDirection::Corner(x, y) => {
            let top_right_angle = width.atan2(height);
            let modern = match (x, y) {
                (HorizontalPositionKeyword::Right, VerticalPositionKeyword::Top) => top_right_angle,
                (HorizontalPositionKeyword::Right, VerticalPositionKeyword::Bottom) => {
                    PI - top_right_angle
                }
                (HorizontalPositionKeyword::Left, VerticalPositionKeyword::Bottom) => {
                    PI + top_right_angle
                }
                (HorizontalPositionKeyword::Left, VerticalPositionKeyword::Top) => -top_right_angle,
            };
            modern + flip
        }
    }
}

fn linear_endpoints(angle: f64, width: f64, height: f64) -> (Point, Point) {
    let (sin, cos) = angle.sin_cos();
    let length = (width * sin).abs() + (height * cos).abs();
    let center = Point::new(width / 2.0, height / 2.0);
    let half = Vec2::new(sin, -cos) * (length / 2.0);
    (center - half, center + half)
}

fn radial_radii(shape: &EndingShape, center: Point, tile: Size) -> (f64, f64) {
    let near_x = center.x.abs().min((tile.width - center.x).abs());
    let far_x = center.x.abs().max((tile.width - center.x).abs());
    let near_y = center.y.abs().min((tile.height - center.y).abs());
    let far_y = center.y.abs().max((tile.height - center.y).abs());
    match shape {
        EndingShape::Circle(Circle::Radius(radius)) => {
            let r = f64::from(radius.0.px());
            (r, r)
        }
        &EndingShape::Circle(Circle::Extent(extent)) => {
            let r = match extent {
                ShapeExtent::ClosestSide | ShapeExtent::Contain => near_x.min(near_y),
                ShapeExtent::FarthestSide => far_x.max(far_y),
                ShapeExtent::ClosestCorner => near_x.hypot(near_y),
                ShapeExtent::FarthestCorner | ShapeExtent::Cover => far_x.hypot(far_y),
            };
            (r, r)
        }
        EndingShape::Ellipse(Ellipse::Radii(x, y)) => (
            f64::from(x.0.resolve(Length::new(tile.width as f32)).px()),
            f64::from(y.0.resolve(Length::new(tile.height as f32)).px()),
        ),
        &EndingShape::Ellipse(Ellipse::Extent(extent)) => match extent {
            ShapeExtent::ClosestSide | ShapeExtent::Contain => (near_x, near_y),
            ShapeExtent::FarthestSide => (far_x, far_y),
            ShapeExtent::ClosestCorner => (near_x * SQRT_2, near_y * SQRT_2),
            ShapeExtent::FarthestCorner | ShapeExtent::Cover => (far_x * SQRT_2, far_y * SQRT_2),
        },
    }
}

fn tile_size(size: &BackgroundSize, area: (f64, f64), intrinsic: Option<(f64, f64)>) -> (f64, f64) {
    let (area_w, area_h) = area;
    let ratio_fit = |cover: bool| -> (f64, f64) {
        match intrinsic {
            Some((iw, ih)) if iw > 0.0 && ih > 0.0 => {
                let sx = area_w / iw;
                let sy = area_h / ih;
                let scale = if cover { sx.max(sy) } else { sx.min(sy) };
                (iw * scale, ih * scale)
            }
            _ => (area_w, area_h),
        }
    };
    match size {
        BackgroundSize::Cover => ratio_fit(true),
        BackgroundSize::Contain => ratio_fit(false),
        BackgroundSize::ExplicitSize { width, height } => {
            let w = explicit_axis(width, area_w);
            let h = explicit_axis(height, area_h);
            match (w, h) {
                (Some(w), Some(h)) => (w, h),
                (Some(w), None) => match intrinsic {
                    Some((iw, ih)) if iw > 0.0 && ih > 0.0 => (w, w * ih / iw),
                    _ => (w, area_h),
                },
                (None, Some(h)) => match intrinsic {
                    Some((iw, ih)) if iw > 0.0 && ih > 0.0 => (h * iw / ih, h),
                    _ => (area_w, h),
                },
                (None, None) => intrinsic.unwrap_or((area_w, area_h)),
            }
        }
    }
}

fn explicit_axis(
    axis: &LengthPercentageOrAuto<NonNegative<LengthPercentage>>,
    basis: f64,
) -> Option<f64> {
    match axis {
        LengthPercentageOrAuto::Auto => None,
        LengthPercentageOrAuto::LengthPercentage(value) => {
            Some(f64::from(value.0.resolve(Length::new(basis as f32)).px()).max(0.0))
        }
    }
}

fn position_offset(position: &LengthPercentage, area: f64, tile: f64) -> f64 {
    f64::from(position.resolve(Length::new((area - tile) as f32)).px())
}

fn round_tile(area: f64, tile: f64) -> f64 {
    if area <= 0.0 || tile <= 0.0 {
        return tile;
    }
    let count = (area / tile).round().max(1.0);
    area / count
}

#[allow(clippy::cast_precision_loss)]
fn small_f64(value: usize) -> f64 {
    value as f64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo::values::computed::Percentage;

    use super::*;

    fn px(value: f32) -> LengthPercentage {
        LengthPercentage::new_length(Length::new(value))
    }

    fn percent(value: f32) -> LengthPercentage {
        LengthPercentage::new_percent(Percentage(value))
    }

    fn explicit(value: LengthPercentage) -> LengthPercentageOrAuto<NonNegative<LengthPercentage>> {
        LengthPercentageOrAuto::LengthPercentage(NonNegative(value))
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn unpositioned_stops_distribute_evenly() {
        let offsets = resolve_stop_offsets(&[None, None, None]);
        assert_close(offsets[0], 0.0);
        assert_close(offsets[1], 0.5);
        assert_close(offsets[2], 1.0);
    }

    #[test]
    fn interior_runs_interpolate_between_anchors() {
        let offsets = resolve_stop_offsets(&[Some(0.2), None, None, Some(0.8)]);
        assert_close(offsets[0], 0.2);
        assert_close(offsets[1], 0.4);
        assert_close(offsets[2], 0.6);
        assert_close(offsets[3], 0.8);
    }

    #[test]
    fn out_of_order_stops_clamp_monotonically() {
        let offsets = resolve_stop_offsets(&[Some(0.8), Some(0.2), None]);
        assert_close(offsets[0], 0.8);
        assert_close(offsets[1], 0.8);
        assert_close(offsets[2], 1.0);
    }

    #[test]
    fn single_unpositioned_stop_lands_at_zero() {
        let offsets = resolve_stop_offsets(&[None]);
        assert_close(offsets[0], 0.0);
    }

    #[test]
    fn out_of_range_positions_survive_fixup() {
        let offsets = resolve_stop_offsets(&[Some(-0.5), None, Some(1.5)]);
        assert_close(offsets[0], -0.5);
        assert_close(offsets[1], 0.5);
        assert_close(offsets[2], 1.5);
    }

    #[test]
    fn to_top_runs_bottom_center_to_top_center() {
        let angle = direction_angle(
            LineDirection::Vertical(VerticalPositionKeyword::Top),
            GradientCompatMode::Modern,
            100.0,
            50.0,
        );
        let (start, end) = linear_endpoints(angle, 100.0, 50.0);
        assert_close(start.x, 50.0);
        assert_close(start.y, 50.0);
        assert_close(end.x, 50.0);
        assert_close(end.y, 0.0);
    }

    #[test]
    fn to_right_runs_left_to_right() {
        let angle = direction_angle(
            LineDirection::Horizontal(HorizontalPositionKeyword::Right),
            GradientCompatMode::Modern,
            100.0,
            50.0,
        );
        let (start, end) = linear_endpoints(angle, 100.0, 50.0);
        assert_close(start.x, 0.0);
        assert_close(start.y, 25.0);
        assert_close(end.x, 100.0);
        assert_close(end.y, 25.0);
    }

    #[test]
    fn corner_endpoint_perpendicular_passes_through_the_corner() {
        let angle = direction_angle(
            LineDirection::Corner(
                HorizontalPositionKeyword::Right,
                VerticalPositionKeyword::Top,
            ),
            GradientCompatMode::Modern,
            100.0,
            50.0,
        );
        let (start, end) = linear_endpoints(angle, 100.0, 50.0);
        let direction = end - start;
        let corner = Point::new(100.0, 0.0);
        let dot = (corner - end).dot(direction);
        assert!(dot.abs() < 1e-9, "perpendicular test failed: {dot}");
        assert!(end.x > 50.0 && end.y < 25.0);
    }

    #[test]
    fn explicit_angles_match_the_css_convention() {
        let (start, end) = linear_endpoints(PI, 100.0, 50.0);
        assert_close(start.y, 0.0);
        assert_close(end.y, 50.0);
        assert_close(start.x, 50.0);
        assert_close(end.x, 50.0);
    }

    #[test]
    fn legacy_prefixed_keywords_name_the_starting_side() {
        let angle = direction_angle(
            LineDirection::Vertical(VerticalPositionKeyword::Top),
            GradientCompatMode::WebKit,
            100.0,
            100.0,
        );
        assert_close(angle, PI);
    }

    #[test]
    fn auto_uses_the_intrinsic_size() {
        let (w, h) = tile_size(&BackgroundSize::auto(), (200.0, 100.0), Some((40.0, 30.0)));
        assert_close(w, 40.0);
        assert_close(h, 30.0);
    }

    #[test]
    fn auto_gradients_fill_the_positioning_area() {
        let (w, h) = tile_size(&BackgroundSize::auto(), (200.0, 100.0), None);
        assert_close(w, 200.0);
        assert_close(h, 100.0);
    }

    #[test]
    fn cover_scales_to_the_larger_ratio() {
        let (w, h) = tile_size(&BackgroundSize::Cover, (200.0, 100.0), Some((100.0, 100.0)));
        assert_close(w, 200.0);
        assert_close(h, 200.0);
    }

    #[test]
    fn contain_scales_to_the_smaller_ratio() {
        let (w, h) = tile_size(
            &BackgroundSize::Contain,
            (200.0, 100.0),
            Some((100.0, 100.0)),
        );
        assert_close(w, 100.0);
        assert_close(h, 100.0);
    }

    #[test]
    fn one_auto_axis_preserves_the_intrinsic_ratio() {
        let size = BackgroundSize::ExplicitSize {
            width: explicit(percent(0.5)),
            height: LengthPercentageOrAuto::Auto,
        };
        let (w, h) = tile_size(&size, (200.0, 100.0), Some((80.0, 40.0)));
        assert_close(w, 100.0);
        assert_close(h, 50.0);
    }

    #[test]
    fn explicit_lengths_and_percentages_resolve_against_the_area() {
        let size = BackgroundSize::ExplicitSize {
            width: explicit(px(30.0)),
            height: explicit(percent(0.25)),
        };
        let (w, h) = tile_size(&size, (200.0, 100.0), None);
        assert_close(w, 30.0);
        assert_close(h, 25.0);
    }

    #[test]
    fn percentage_position_centers_the_remaining_space() {
        assert_close(position_offset(&percent(0.5), 200.0, 50.0), 75.0);
        assert_close(position_offset(&percent(1.0), 200.0, 50.0), 150.0);
    }

    #[test]
    fn length_position_offsets_from_the_origin_edge() {
        assert_close(position_offset(&px(12.0), 200.0, 50.0), 12.0);
    }

    #[test]
    fn round_repeat_snaps_to_whole_tiles() {
        assert_close(round_tile(100.0, 30.0), 100.0 / 3.0);
        assert_close(round_tile(100.0, 260.0), 100.0);
    }

    #[test]
    fn conic_stops_pad_the_full_turn() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let window = conic_window_stops(&[(0.25, red), (0.75, blue)]);
        assert_eq!(window.len(), 4);
        assert!((window[0].0 - 0.0).abs() < 1e-6 && window[0].1 == red);
        assert!((window[1].0 - 0.25).abs() < 1e-6 && window[1].1 == red);
        assert!((window[2].0 - 0.75).abs() < 1e-6 && window[2].1 == blue);
        assert!((window[3].0 - 1.0).abs() < 1e-6 && window[3].1 == blue);
    }

    #[test]
    fn conic_out_of_range_stops_clamp_to_the_seam() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let window = conic_window_stops(&[(-0.5, red), (1.5, blue)]);
        assert_eq!(window.len(), 2);
        assert!((window[0].0 - 0.0).abs() < 1e-6 && window[0].1 == red);
        assert!((window[1].0 - 1.0).abs() < 1e-6 && window[1].1 == blue);
    }

    #[test]
    fn repeating_conic_periods_unroll_across_the_turn() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let unrolled = unroll_conic_period(&[(0.25, red), (0.5, blue)]).expect("finite unroll");
        assert_eq!(unrolled.len(), 12);
        assert_close(unrolled[0].0, -0.25);
        assert_close(unrolled[11].0, 1.25);
        assert!(unrolled.first().unwrap().0 <= 0.0);
        assert!(unrolled.last().unwrap().0 >= 1.0);
        for pair in unrolled.windows(2) {
            assert!(pair[0].0 <= pair[1].0 + 1e-12);
        }
    }

    #[test]
    fn corner_ellipses_scale_side_radii_by_sqrt_two() {
        let shape = EndingShape::Ellipse(Ellipse::Extent(ShapeExtent::FarthestCorner));
        let (rx, ry) = radial_radii(&shape, Point::new(25.0, 25.0), Size::new(100.0, 50.0));
        assert_close(rx, 75.0 * SQRT_2);
        assert_close(ry, 25.0 * SQRT_2);
    }

    #[test]
    fn closest_side_circles_use_the_nearest_side_distance() {
        let shape = EndingShape::Circle(Circle::Extent(ShapeExtent::ClosestSide));
        let (rx, ry) = radial_radii(&shape, Point::new(30.0, 20.0), Size::new(100.0, 50.0));
        assert_close(rx, 20.0);
        assert_close(ry, 20.0);
    }

    #[test]
    fn sub_zero_radial_stops_synthesize_the_zero_crossing_color() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let stops = synthesize_zero_radius_stop(vec![(-1.0, red), (1.0, blue)]);
        assert_eq!(stops.len(), 2);
        assert_close(stops[0].0, 0.0);
        assert_eq!(stops[0].1, Color::new([0.5, 0.0, 0.5, 1.0]));
        assert_close(stops[1].0, 1.0);
        assert_eq!(stops[1].1, blue);
    }

    #[test]
    fn all_negative_radial_stops_collapse_to_the_last_color() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let stops = synthesize_zero_radius_stop(vec![(-2.0, red), (-1.0, blue)]);
        assert_eq!(stops.len(), 1);
        assert_close(stops[0].0, 0.0);
        assert_eq!(stops[0].1, blue);
    }

    #[test]
    fn non_negative_radial_stops_pass_through_untouched() {
        let red = Color::new([1.0, 0.0, 0.0, 1.0]);
        let blue = Color::new([0.0, 0.0, 1.0, 1.0]);
        let stops = synthesize_zero_radius_stop(vec![(0.0, red), (0.25, blue), (1.5, red)]);
        assert_eq!(stops.len(), 3);
        assert_close(stops[0].0, 0.0);
        assert_eq!(stops[0].1, red);
        assert_close(stops[1].0, 0.25);
        assert_eq!(stops[1].1, blue);
        assert_close(stops[2].0, 1.5);
        assert_eq!(stops[2].1, red);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod object_fit_tests {
    use stylo::computed_values::object_fit::T as ObjectFit;

    use super::{concrete_object_size, fitted_size};
    use crate::layout::{NaturalSize, Size as LayoutSize};
    use crate::vello::kurbo::Rect;

    fn content() -> Rect {
        Rect::new(0.0, 0.0, 200.0, 100.0)
    }

    fn natural(width: f32, height: f32) -> NaturalSize {
        NaturalSize::from_size(LayoutSize::new(width, height))
    }

    fn assert_close(actual: (f64, f64), expected: (f64, f64)) {
        assert!(
            (actual.0 - expected.0).abs() < 1e-6 && (actual.1 - expected.1).abs() < 1e-6,
            "expected {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn fill_stretches_to_the_content_box_ignoring_the_ratio() {
        let (width, height) = concrete_object_size(natural(40.0, 20.0), content());
        assert_close(
            fitted_size(ObjectFit::Fill, width, height, content()),
            (200.0, 100.0),
        );
    }

    #[test]
    fn contain_fits_inside_and_cover_fills_past_the_box() {
        let (width, height) = concrete_object_size(natural(40.0, 20.0), content());
        assert_close(
            fitted_size(ObjectFit::Contain, width, height, content()),
            (200.0, 100.0),
        );

        let (width, height) = concrete_object_size(natural(40.0, 40.0), content());
        assert_close(
            fitted_size(ObjectFit::Contain, width, height, content()),
            (100.0, 100.0),
        );
        assert_close(
            fitted_size(ObjectFit::Cover, width, height, content()),
            (200.0, 200.0),
        );
    }

    #[test]
    fn none_keeps_the_natural_size_whether_it_overflows_or_not() {
        let (width, height) = concrete_object_size(natural(40.0, 20.0), content());
        assert_close(
            fitted_size(ObjectFit::None, width, height, content()),
            (40.0, 20.0),
        );

        let (width, height) = concrete_object_size(natural(400.0, 800.0), content());
        assert_close(
            fitted_size(ObjectFit::None, width, height, content()),
            (400.0, 800.0),
        );
    }

    #[test]
    fn scale_down_is_none_when_small_and_contain_when_large() {
        let (width, height) = concrete_object_size(natural(40.0, 20.0), content());
        assert_close(
            fitted_size(ObjectFit::ScaleDown, width, height, content()),
            fitted_size(ObjectFit::None, width, height, content()),
        );

        let (width, height) = concrete_object_size(natural(400.0, 800.0), content());
        assert_close(
            fitted_size(ObjectFit::ScaleDown, width, height, content()),
            fitted_size(ObjectFit::Contain, width, height, content()),
        );
        assert_close(
            fitted_size(ObjectFit::ScaleDown, width, height, content()),
            (50.0, 100.0),
        );
    }

    #[test]
    fn a_ratio_without_dimensions_resolves_against_the_content_box() {
        let ratio_only = NaturalSize::new(LayoutSize::new(None, None), Some(4.0));
        assert_close(concrete_object_size(ratio_only, content()), (200.0, 50.0));

        let square = NaturalSize::new(LayoutSize::new(None, None), Some(1.0));
        assert_close(concrete_object_size(square, content()), (100.0, 100.0));
    }

    #[test]
    fn one_axis_plus_a_ratio_determines_the_other() {
        let width_and_ratio = NaturalSize::new(LayoutSize::new(Some(80.0), None), Some(2.0));
        assert_close(
            concrete_object_size(width_and_ratio, content()),
            (80.0, 40.0),
        );

        let height_and_ratio = NaturalSize::new(LayoutSize::new(None, Some(30.0)), Some(3.0));
        assert_close(
            concrete_object_size(height_and_ratio, content()),
            (90.0, 30.0),
        );
    }

    #[test]
    fn a_zero_or_absent_natural_axis_falls_back_to_the_content_box() {
        for degenerate in [
            NaturalSize::NONE,
            natural(0.0, 0.0),
            natural(0.0, 50.0),
            natural(50.0, 0.0),
        ] {
            let (width, height) = concrete_object_size(degenerate, content());
            assert!(
                width > 0.0 && height > 0.0,
                "{degenerate:?} must not yield a non-positive object size"
            );
            for fit in [
                ObjectFit::Fill,
                ObjectFit::Contain,
                ObjectFit::Cover,
                ObjectFit::None,
                ObjectFit::ScaleDown,
            ] {
                let (drawn_width, drawn_height) = fitted_size(fit, width, height, content());
                assert!(
                    drawn_width.is_finite() && drawn_height.is_finite(),
                    "{fit:?} on {degenerate:?} produced {drawn_width}x{drawn_height}"
                );
            }
        }
    }

    #[test]
    fn a_zero_area_content_box_never_produces_a_nan() {
        let empty = Rect::new(0.0, 0.0, 0.0, 0.0);
        let (width, height) = concrete_object_size(NaturalSize::NONE, empty);
        for fit in [ObjectFit::Contain, ObjectFit::Cover, ObjectFit::ScaleDown] {
            let (drawn_width, drawn_height) = fitted_size(fit, width, height, empty);
            assert!(drawn_width.is_finite() && drawn_height.is_finite());
        }
    }
}
