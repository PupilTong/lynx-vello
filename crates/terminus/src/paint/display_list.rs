//! The display list: what the paint walk records, what a committed frame
//! retains, and what the rasterizer replays (`render::sparse`).
//!
//! It holds drawing semantics, not a bit stream. A fill or stroke keeps its
//! shape as the shape it was recorded as — a rectangle stays a [`Rect`], a
//! rounded rectangle a [`RoundedRect`], anything else a path — together with
//! its transform, its paint and the paint's own transform. A clip keeps
//! whether it has to stay an isolated layer ([`Item::PushClip`]); a layer
//! keeps its blend mode and alpha; a glyph run keeps its font, size,
//! transforms and the synthetic bold amount in the units the glyph renderer
//! applies it in ([`GlyphRun::embolden`]); an image keeps its pixels' handle
//! and sampler; a gradient is shared by reference, so a list that copies it
//! ([`DisplayList::append`]) copies no stops.
//!
//! Paths, glyphs and variation coordinates live in arenas the items index,
//! so appending one list to another — the compose program's per-frame
//! assembly of a committed frame's fragments — copies items and arena
//! slices and allocates nothing per item.
//!
//! The recording calls mirror the drawing model the paint modules were
//! written against: shapes under a transform, a layer stack of clips and
//! group layers, and glyph runs built by a [`DrawGlyphs`] builder. Every
//! clip and layer pushed is popped by the matching [`DisplayList::pop_layer`].

use std::ops::Range;
use std::sync::Arc;

/// One positioned glyph of a run.
pub use glifo::Glyph;
use vello::kurbo::{Affine, PathEl, Rect, RoundedRect, Stroke};
use vello::peniko::{
    BlendMode, Brush, BrushRef, Color, Fill, FontData, Gradient, ImageBrushRef, ImageData,
    ImageSampler, StyleRef,
};

/// A variation-space coordinate of a variable font, as `F2Dot14` bits.
pub type NormalizedCoord = i16;

/// The tolerance a curved fill or clip shape is flattened to a path at.
pub(crate) const FILL_TOLERANCE: f64 = 0.1;

/// The tolerance a curved stroked shape is flattened to a path at: finer
/// than a fill's, since a stroke's error is seen from both sides.
pub(crate) const STROKE_TOLERANCE: f64 = 0.01;

/// The geometry one item draws or clips with, in the item's own space.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Shape {
    Rect(Rect),
    RoundedRect(RoundedRect),
    /// A path: a range of the list's path arena.
    Path {
        start: u32,
        end: u32,
    },
}

/// The paint a fill, stroke or glyph run applies.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Solid(Color),
    /// Shared, so a list that copies the draw copies a reference.
    Gradient(Arc<Gradient>),
    /// A bitmap and how it is sampled.
    Image(Arc<ImagePaint>),
}

/// A bitmap paint: the pixels' handle and how they are sampled.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImagePaint {
    pub(crate) image: ImageData,
    pub(crate) sampler: ImageSampler,
}

impl Paint {
    /// This paint with its alpha multiplied by `alpha`.
    fn multiply_alpha(self, alpha: f32) -> Self {
        #[expect(
            clippy::float_cmp,
            reason = "1 is exactly the alpha that changes nothing"
        )]
        if alpha == 1.0 {
            return self;
        }
        match self {
            Self::Solid(color) => Self::Solid(color.multiply_alpha(alpha)),
            Self::Gradient(gradient) => {
                Self::Gradient(Arc::new((*gradient).clone().multiply_alpha(alpha)))
            }
            Self::Image(image) => Self::Image(Arc::new(ImagePaint {
                image: image.image.clone(),
                sampler: image.sampler.multiply_alpha(alpha),
            })),
        }
    }
}

impl From<BrushRef<'_>> for Paint {
    fn from(brush: BrushRef<'_>) -> Self {
        match brush {
            BrushRef::Solid(color) => Self::Solid(color),
            BrushRef::Gradient(gradient) => gradient_paint(gradient),
            BrushRef::Image(image) => Self::Image(Arc::new(ImagePaint {
                image: image.image.clone(),
                sampler: image.sampler,
            })),
        }
    }
}

impl From<&Brush> for Paint {
    fn from(brush: &Brush) -> Self {
        BrushRef::from(brush).into()
    }
}

/// A gradient brush as a paint: one with no stops paints nothing and one
/// with a single stop is that stop's colour, which is what a gradient of
/// one colour draws (SVG 2 §14.3.3 says so of a paint server, and the CSS
/// gradient functions need two stops to parse).
fn gradient_paint(gradient: &Gradient) -> Paint {
    match gradient.stops.as_slice() {
        [] => Paint::Solid(Color::TRANSPARENT),
        [only] => Paint::Solid(only.color.to_alpha_color()),
        _ => Paint::Gradient(Arc::new(gradient.clone())),
    }
}

/// A fill or a stroke.
#[derive(Clone, Debug)]
pub(crate) struct Draw {
    pub(crate) rule: Fill,
    /// The shape's space to the list's.
    pub(crate) transform: Affine,
    pub(crate) shape: Shape,
    pub(crate) paint: Paint,
    /// The paint's space to the shape's; `None` is the identity.
    pub(crate) paint_transform: Option<Affine>,
}

/// A clip or a group layer: its shape and fill rule.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Region {
    pub(crate) rule: Fill,
    pub(crate) transform: Affine,
    pub(crate) shape: Shape,
}

/// A Gaussian-blurred rounded rectangle (`box-shadow`): the area
/// `std_dev × 2.5` around `rect` is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BlurredRoundedRect {
    pub(crate) transform: Affine,
    pub(crate) rect: Rect,
    pub(crate) color: Color,
    pub(crate) radius: f32,
    pub(crate) std_dev: f32,
}

/// One recorded drawing operation.
#[derive(Clone, Debug)]
pub(crate) enum Item {
    Fill(Draw),
    Stroke(Draw, Arc<Stroke>),
    /// A plain clip: everything up to the matching [`Item::Pop`] is cut to
    /// the region.
    ///
    /// `isolated` is set when a [`Item::PushLayer`] that blends by anything
    /// but source-over opens inside it, at any depth: such a clip is
    /// composited as a layer of its own, so its content and every blend in
    /// it run at full coverage and the clip applies once, when it
    /// composites. Every other clip is intersected into what it holds and
    /// needs no layer of its own (`render::sparse`).
    PushClip {
        region: Region,
        isolated: bool,
    },
    /// A group layer: its content composited by `blend` at `alpha`, cut to
    /// the region.
    PushLayer {
        region: Region,
        blend: BlendMode,
        alpha: f32,
    },
    Pop,
    /// [`DisplayList::runs`]`[index]`.
    Glyphs(u32),
    BlurredRoundedRect(BlurredRoundedRect),
}

/// How a glyph run's outlines are drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GlyphStyle {
    Fill,
    Stroke(Arc<Stroke>),
}

/// Synthetic bold: how far a run's outlines are widened, with the controls
/// of the widening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Embolden {
    /// Per-axis widening.
    pub x: f64,
    pub y: f64,
    pub join: vello::kurbo::Join,
    pub miter_limit: f64,
    pub tolerance: f64,
}

impl Default for Embolden {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            join: vello::kurbo::Join::Miter,
            miter_limit: 4.0,
            tolerance: 0.1,
        }
    }
}

impl Embolden {
    /// Widening by `amount` font-size units on both axes, with the default
    /// controls.
    #[must_use]
    pub fn uniform(amount: f64) -> Self {
        Self {
            x: amount,
            y: amount,
            ..Self::default()
        }
    }

    fn is_none(&self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }

    /// The same widening `scale` times as large, its tolerance with it.
    #[must_use]
    pub(crate) fn scaled(self, scale: f64) -> Self {
        Self {
            x: self.x * scale,
            y: self.y * scale,
            tolerance: self.tolerance * scale,
            ..self
        }
    }
}

/// One glyph run.
#[derive(Clone, Debug)]
pub(crate) struct GlyphRun {
    pub(crate) font: FontData,
    pub(crate) font_size: f32,
    /// The run's space to the list's.
    pub(crate) transform: Affine,
    /// Applied to each glyph's outline at its origin, in the run's y-down
    /// space (a synthetic oblique's shear).
    pub(crate) glyph_transform: Option<Affine>,
    pub(crate) paint: Paint,
    /// The paint's space to the run's; `None` is the identity.
    pub(crate) paint_transform: Option<Affine>,
    pub(crate) style: GlyphStyle,
    pub(crate) hint: bool,
    /// Synthetic bold, in the units the glyph renderer (glifo) widens the
    /// outline in.
    ///
    /// An unhinted fill is outlined at the font's units per em whatever the
    /// run's transform, so its amount is converted from font-size units
    /// when it is recorded. A stroked or hinted run is outlined at the font
    /// size times the uniform scale glifo folds into it from the composed
    /// transform — which a compose-time transform can change — so its
    /// amount stays in font-size units and the replay applies that scale
    /// ([`GlyphRun::outline_scale`]).
    pub(crate) embolden: Embolden,
    pub(crate) glyphs: Range<u32>,
    pub(crate) coords: Range<u32>,
}

impl GlyphRun {
    /// The factor between this run's stored synthetic bold and the units
    /// glifo outlines it in under `full`, the run's composed transform with
    /// its glyph transform: 1 for an unhinted fill, whose amount is already
    /// in those units, and otherwise the uniform scale glifo absorbs into
    /// the font size (`glifo`'s `prepare_glyph_run`), or 1 where it absorbs
    /// none.
    pub(crate) fn outline_scale(&self, full: Affine) -> f64 {
        const NEARLY_ZERO: f64 = 1.0 / 4096.0;
        let [a, b, c, d, ..] = full.as_coeffs();
        let uniform = (a - d).abs() <= NEARLY_ZERO && a > 0.0 && d > 0.0;
        let absorbs = if self.hint {
            uniform && b.abs() <= NEARLY_ZERO
        } else {
            uniform && b.abs() <= NEARLY_ZERO && c.abs() <= NEARLY_ZERO
        };
        match self.style {
            GlyphStyle::Fill if !self.hint => 1.0,
            _ if absorbs => d,
            _ => 1.0,
        }
    }
}

/// See the [module documentation](self).
#[derive(Default)]
pub struct DisplayList {
    items: Vec<Item>,
    paths: Vec<PathEl>,
    runs: Vec<GlyphRun>,
    glyphs: Vec<Glyph>,
    coords: Vec<NormalizedCoord>,
    /// The indices of the open clips and layers, innermost last.
    open: Vec<u32>,
    /// Whether a layer that blends by anything but source-over opens with
    /// no clip or layer around it, so that it blends with what is under the
    /// whole list.
    blends_at_root: bool,
}

impl std::fmt::Debug for DisplayList {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DisplayList")
            .field("items", &self.items.len())
            .field("open", &self.open.len())
            .finish_non_exhaustive()
    }
}

impl Clone for DisplayList {
    fn clone(&self) -> Self {
        let mut list = Self::new();
        list.append(self, None);
        list
    }
}

/// Whether `blend` is anything but plain source-over.
fn is_blend(blend: BlendMode) -> bool {
    blend != BlendMode::default()
}

impl DisplayList {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties the list, keeping its capacity.
    pub fn reset(&mut self) {
        self.items.clear();
        self.paths.clear();
        self.runs.clear();
        self.glyphs.clear();
        self.coords.clear();
        self.open.clear();
        self.blends_at_root = false;
    }

    /// Whether the list records nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many items the list records: draws, glyph runs, and every push
    /// and pop.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// How many clips and layers are open at the end of the list.
    #[must_use]
    pub fn open_layers(&self) -> usize {
        self.open.len()
    }

    /// How many clips and group layers the list pushes.
    #[must_use]
    pub fn layers(&self) -> usize {
        self.items
            .iter()
            .filter(|item| matches!(item, Item::PushClip { .. } | Item::PushLayer { .. }))
            .count()
    }

    /// How many glyph runs the list records.
    #[must_use]
    pub fn glyph_runs(&self) -> usize {
        self.runs.len()
    }

    /// How many fills and strokes paint an image.
    #[cfg(test)]
    pub(crate) fn image_draws(&self) -> usize {
        self.items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    Item::Fill(Draw {
                        paint: Paint::Image(_),
                        ..
                    }) | Item::Stroke(
                        Draw {
                            paint: Paint::Image(_),
                            ..
                        },
                        _
                    )
                )
            })
            .count()
    }

    pub(crate) fn items(&self) -> &[Item] {
        &self.items
    }

    pub(crate) fn runs(&self) -> &[GlyphRun] {
        &self.runs
    }

    /// The glyphs of `run`.
    pub(crate) fn glyphs_of(&self, run: &GlyphRun) -> &[Glyph] {
        &self.glyphs[run.glyphs.start as usize..run.glyphs.end as usize]
    }

    /// The variation coordinates of `run`.
    pub(crate) fn coords_of(&self, run: &GlyphRun) -> &[NormalizedCoord] {
        &self.coords[run.coords.start as usize..run.coords.end as usize]
    }

    /// The path elements of a [`Shape::Path`].
    pub(crate) fn path(&self, start: u32, end: u32) -> &[PathEl] {
        &self.paths[start as usize..end as usize]
    }

    /// Whether some layer blends by anything but source-over with what is
    /// under the whole list.
    pub(crate) fn blends_at_root(&self) -> bool {
        self.blends_at_root
    }

    /// Records `shape` as an item shape at `tolerance`; `None` for a shape
    /// with no segments.
    fn shape(&mut self, shape: &impl vello::kurbo::Shape, tolerance: f64) -> Option<Shape> {
        if let Some(rect) = shape.as_rect() {
            return Some(Shape::Rect(rect));
        }
        if let Some(rounded) = shape.as_rounded_rect() {
            return Some(Shape::RoundedRect(rounded));
        }
        let start = self.paths.len();
        self.paths.extend(shape.path_elements(tolerance));
        let end = self.paths.len();
        if start == end {
            return None;
        }
        Some(Shape::Path {
            start: index(start),
            end: index(end),
        })
    }

    /// Fills `shape` under `transform` with `brush`, placed by
    /// `brush_transform` relative to the shape's space.
    pub fn fill<'b>(
        &mut self,
        style: Fill,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl vello::kurbo::Shape,
    ) {
        let Some(shape) = self.shape(shape, FILL_TOLERANCE) else {
            return;
        };
        self.items.push(Item::Fill(Draw {
            rule: style,
            transform,
            shape,
            paint: brush.into().into(),
            paint_transform: brush_transform,
        }));
    }

    /// Strokes `shape` under `transform` with `brush`. A dash pattern on
    /// `style` is applied by the rasterizer.
    pub fn stroke<'b>(
        &mut self,
        style: &Stroke,
        transform: Affine,
        brush: impl Into<BrushRef<'b>>,
        brush_transform: Option<Affine>,
        shape: &impl vello::kurbo::Shape,
    ) {
        if style.width == 0.0 {
            return;
        }
        let Some(shape) = self.shape(shape, STROKE_TOLERANCE) else {
            return;
        };
        self.items.push(Item::Stroke(
            Draw {
                rule: Fill::NonZero,
                transform,
                shape,
                paint: brush.into().into(),
                paint_transform: brush_transform,
            },
            Arc::new(style.clone()),
        ));
    }

    /// The region of a clip or layer over `clip`; an empty shape clips
    /// everything.
    fn region(&mut self, rule: Fill, transform: Affine, clip: &impl vello::kurbo::Shape) -> Region {
        let shape = self
            .shape(clip, FILL_TOLERANCE)
            .unwrap_or(Shape::Rect(Rect::ZERO));
        Region {
            rule,
            transform,
            shape,
        }
    }

    /// Opens a group layer: what is drawn until the matching
    /// [`Self::pop_layer`] is composited by `blend` at `alpha`, cut to
    /// `clip` under `transform`.
    pub fn push_layer(
        &mut self,
        clip_style: Fill,
        blend: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl vello::kurbo::Shape,
    ) {
        let region = self.region(clip_style, transform, clip);
        self.open_layer(Item::PushLayer {
            region,
            blend: blend.into(),
            alpha: alpha.clamp(0.0, 1.0),
        });
    }

    /// Opens a clip: what is drawn until the matching [`Self::pop_layer`]
    /// is cut to `clip` under `transform`.
    pub fn push_clip_layer(
        &mut self,
        clip_style: Fill,
        transform: Affine,
        clip: &impl vello::kurbo::Shape,
    ) {
        let region = self.region(clip_style, transform, clip);
        self.open_layer(Item::PushClip {
            region,
            isolated: false,
        });
    }

    /// Records a push, keeping the open stack and the clips' isolation.
    fn open_layer(&mut self, item: Item) {
        if let Item::PushLayer { blend, .. } = &item
            && is_blend(*blend)
        {
            self.isolate_open_clips();
        }
        self.open.push(index(self.items.len()));
        self.items.push(item);
    }

    /// Marks every open clip isolated: a blend layer is opening inside
    /// them. A clip already marked was marked with every clip under it, so
    /// the walk down the stack stops there.
    fn isolate_open_clips(&mut self) {
        if self.open.is_empty() {
            self.blends_at_root = true;
        }
        for &open in self.open.iter().rev() {
            if let Item::PushClip { isolated, .. } = &mut self.items[open as usize] {
                if *isolated {
                    break;
                }
                *isolated = true;
            }
        }
    }

    /// Closes the innermost open clip or layer.
    pub fn pop_layer(&mut self) {
        if self.open.pop().is_some() {
            self.items.push(Item::Pop);
        }
    }

    /// Draws a rounded rectangle blurred by a Gaussian of `std_dev`.
    pub fn draw_blurred_rounded_rect(
        &mut self,
        transform: Affine,
        rect: Rect,
        brush: Color,
        radius: f64,
        std_dev: f64,
    ) {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the rasterizer takes the radius and deviation as f32"
        )]
        self.items
            .push(Item::BlurredRoundedRect(BlurredRoundedRect {
                transform,
                rect,
                color: brush,
                radius: radius as f32,
                std_dev: std_dev as f32,
            }));
    }

    /// Draws `image` at its natural size under `transform`.
    pub fn draw_image<'b>(&mut self, image: impl Into<ImageBrushRef<'b>>, transform: Affine) {
        let brush = image.into();
        let rect = Rect::new(
            0.0,
            0.0,
            f64::from(brush.image.width),
            f64::from(brush.image.height),
        );
        self.fill(Fill::NonZero, transform, brush, None, &rect);
    }

    /// A builder for one glyph run of `font`.
    pub fn draw_glyphs(&mut self, font: &FontData) -> DrawGlyphs<'_> {
        DrawGlyphs {
            run: GlyphRun {
                font: font.clone(),
                font_size: 16.0,
                transform: Affine::IDENTITY,
                glyph_transform: None,
                paint: Paint::Solid(Color::BLACK),
                paint_transform: None,
                style: GlyphStyle::Fill,
                hint: false,
                embolden: Embolden::default(),
                glyphs: 0..0,
                coords: 0..0,
            },
            coords_start: self.coords.len(),
            brush_alpha: 1.0,
            list: self,
        }
    }

    /// Appends `other`, every item's transform left-multiplied by
    /// `transform`.
    pub fn append(&mut self, other: &Self, transform: Option<Affine>) {
        let outer = |inner: Affine| transform.map_or(inner, |outer| outer * inner);
        let paths = index(self.paths.len());
        let runs = index(self.runs.len());
        let glyphs = index(self.glyphs.len());
        let coords = index(self.coords.len());
        self.paths.extend_from_slice(&other.paths);
        self.glyphs.extend_from_slice(&other.glyphs);
        self.coords.extend_from_slice(&other.coords);
        let shape = |shape: Shape| match shape {
            Shape::Path { start, end } => Shape::Path {
                start: start + paths,
                end: end + paths,
            },
            other => other,
        };
        let region = |region: Region| Region {
            transform: outer(region.transform),
            shape: shape(region.shape),
            ..region
        };
        let draw = |draw: &Draw| Draw {
            transform: outer(draw.transform),
            shape: shape(draw.shape),
            ..draw.clone()
        };
        self.runs.extend(other.runs.iter().map(|run| GlyphRun {
            transform: outer(run.transform),
            glyphs: run.glyphs.start + glyphs..run.glyphs.end + glyphs,
            coords: run.coords.start + coords..run.coords.end + coords,
            ..run.clone()
        }));
        self.items.reserve(other.items.len());
        for item in &other.items {
            match item {
                Item::Fill(fill) => self.items.push(Item::Fill(draw(fill))),
                Item::Stroke(stroke, style) => self
                    .items
                    .push(Item::Stroke(draw(stroke), Arc::clone(style))),
                Item::PushClip {
                    region: clip,
                    isolated,
                } => self.open_layer(Item::PushClip {
                    region: region(*clip),
                    isolated: *isolated,
                }),
                Item::PushLayer {
                    region: layer,
                    blend,
                    alpha,
                } => self.open_layer(Item::PushLayer {
                    region: region(*layer),
                    blend: *blend,
                    alpha: *alpha,
                }),
                Item::Pop => self.pop_layer(),
                Item::Glyphs(run) => self.items.push(Item::Glyphs(run + runs)),
                Item::BlurredRoundedRect(blur) => {
                    self.items
                        .push(Item::BlurredRoundedRect(BlurredRoundedRect {
                            transform: outer(blur.transform),
                            ..*blur
                        }));
                }
            }
        }
    }

    #[cfg(test)]
    /// Describes the first difference between this list and `other`, or
    /// `Ok` when they record the same drawing: the same items in the same
    /// order, with equal shapes, paints, transforms and glyphs. Floats
    /// compare with `==`.
    pub(crate) fn compare(&self, other: &Self) -> Result<(), String> {
        if self.items.len() != other.items.len() {
            return Err(format!(
                "{} items vs {}",
                self.items.len(),
                other.items.len()
            ));
        }
        for (position, (left, right)) in self.items.iter().zip(&other.items).enumerate() {
            if !self.same_item(left, other, right) {
                return Err(format!("item {position}: {left:?} vs {right:?}"));
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn same_shape(&self, left: Shape, other: &Self, right: Shape) -> bool {
        match (left, right) {
            (Shape::Rect(left), Shape::Rect(right)) => left == right,
            (Shape::RoundedRect(left), Shape::RoundedRect(right)) => left == right,
            (Shape::Path { start, end }, Shape::Path { start: s, end: e }) => {
                self.path(start, end) == other.path(s, e)
            }
            _ => false,
        }
    }

    #[cfg(test)]
    fn same_draw(&self, left: &Draw, other: &Self, right: &Draw) -> bool {
        left.rule == right.rule
            && left.transform == right.transform
            && left.paint == right.paint
            && left.paint_transform == right.paint_transform
            && self.same_shape(left.shape, other, right.shape)
    }

    #[cfg(test)]
    fn same_region(&self, left: &Region, other: &Self, right: &Region) -> bool {
        left.rule == right.rule
            && left.transform == right.transform
            && self.same_shape(left.shape, other, right.shape)
    }

    #[cfg(test)]
    #[expect(
        clippy::float_cmp,
        reason = "the same drawing records the same floats; the comparison is exact by design"
    )]
    fn same_item(&self, left: &Item, other: &Self, right: &Item) -> bool {
        match (left, right) {
            (Item::Fill(left), Item::Fill(right)) => self.same_draw(left, other, right),
            (Item::Stroke(left, style), Item::Stroke(right, other_style)) => {
                style == other_style && self.same_draw(left, other, right)
            }
            (
                Item::PushClip { region, isolated },
                Item::PushClip {
                    region: other_region,
                    isolated: other_isolated,
                },
            ) => isolated == other_isolated && self.same_region(region, other, other_region),
            (
                Item::PushLayer {
                    region,
                    blend,
                    alpha,
                },
                Item::PushLayer {
                    region: other_region,
                    blend: other_blend,
                    alpha: other_alpha,
                },
            ) => {
                blend == other_blend
                    && alpha == other_alpha
                    && self.same_region(region, other, other_region)
            }
            (Item::Pop, Item::Pop) => true,
            (Item::Glyphs(left), Item::Glyphs(right)) => {
                let (left, right) = (&self.runs[*left as usize], &other.runs[*right as usize]);
                left.font == right.font
                    && left.font_size == right.font_size
                    && left.transform == right.transform
                    && left.glyph_transform == right.glyph_transform
                    && left.paint == right.paint
                    && left.paint_transform == right.paint_transform
                    && left.style == right.style
                    && left.hint == right.hint
                    && left.embolden == right.embolden
                    && self.glyphs_of(left) == other.glyphs_of(right)
                    && self.coords_of(left) == other.coords_of(right)
            }
            (Item::BlurredRoundedRect(left), Item::BlurredRoundedRect(right)) => left == right,
            _ => false,
        }
    }
}

/// An arena index or item count as the `u32` the list stores.
fn index(length: usize) -> u32 {
    u32::try_from(length).expect("a display list holds fewer than 2^32 of anything")
}

/// The builder [`DisplayList::draw_glyphs`] answers. Unset, a run is 16 px,
/// unhinted, untransformed and solid black, with no synthetic bold.
#[must_use = "a glyph run is recorded by `draw`"]
#[derive(Debug)]
pub struct DrawGlyphs<'a> {
    list: &'a mut DisplayList,
    run: GlyphRun,
    coords_start: usize,
    brush_alpha: f32,
}

impl DrawGlyphs<'_> {
    pub fn font_size(mut self, size: f32) -> Self {
        self.run.font_size = size;
        self
    }

    /// The run's space to the list's.
    pub fn transform(mut self, transform: Affine) -> Self {
        self.run.transform = transform;
        self
    }

    /// Applied to each glyph's outline at its origin, in the run's y-down
    /// space.
    pub fn glyph_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.glyph_transform = transform;
        self
    }

    /// Synthetic bold, in font-size units.
    pub fn font_embolden(mut self, embolden: Embolden) -> Self {
        self.run.embolden = embolden;
        self
    }

    pub fn hint(mut self, hint: bool) -> Self {
        self.run.hint = hint;
        self
    }

    /// The variation coordinates of a variable font's instance.
    pub fn normalized_coords(self, coords: &[NormalizedCoord]) -> Self {
        self.list.coords.truncate(self.coords_start);
        self.list.coords.extend_from_slice(coords);
        self
    }

    pub fn brush<'b>(mut self, brush: impl Into<BrushRef<'b>>) -> Self {
        self.run.paint = brush.into().into();
        self
    }

    /// A further alpha the brush is multiplied by.
    pub fn brush_alpha(mut self, alpha: f32) -> Self {
        self.brush_alpha = alpha;
        self
    }

    /// The paint's space to the run's.
    pub fn brush_transform(mut self, transform: Option<Affine>) -> Self {
        self.run.paint_transform = transform;
        self
    }

    /// Records the run of `glyphs`, filled or stroked as `style` says. A run
    /// with no glyph records nothing.
    pub fn draw<'s>(self, style: impl Into<StyleRef<'s>>, glyphs: impl Iterator<Item = Glyph>) {
        let Self {
            list,
            mut run,
            coords_start,
            brush_alpha,
        } = self;
        let glyphs_start = list.glyphs.len();
        list.glyphs.extend(glyphs);
        if list.glyphs.len() == glyphs_start {
            list.coords.truncate(coords_start);
            return;
        }
        run.style = match style.into() {
            StyleRef::Fill(_) => GlyphStyle::Fill,
            StyleRef::Stroke(stroke) => GlyphStyle::Stroke(Arc::new(stroke.clone())),
        };
        run.paint = run.paint.multiply_alpha(brush_alpha);
        // glifo outlines an unhinted fill at the font's units per em, so a
        // widening in font-size units is that many times the size there.
        if run.style == GlyphStyle::Fill && !run.hint && !run.embolden.is_none() {
            let upem = units_per_em(run.font.data.data(), run.font.index);
            run.embolden = run.embolden.scaled(upem / f64::from(run.font_size));
        }
        run.glyphs = index(glyphs_start)..index(list.glyphs.len());
        run.coords = index(coords_start)..index(list.coords.len());
        list.items.push(Item::Glyphs(index(list.runs.len())));
        list.runs.push(run);
    }
}

/// The `unitsPerEm` of face `index` in `font`'s `head` table, or 1000 for a
/// font that has none.
pub(crate) fn units_per_em(font: &[u8], index: u32) -> f64 {
    let u16_at = |at: usize| {
        font.get(at..at + 2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
    };
    let u32_at = |at: usize| {
        font.get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let face = if font.get(..4) == Some(b"ttcf") {
        u32_at(12 + 4 * index as usize).map(|offset| offset as usize)
    } else {
        Some(0)
    };
    face.and_then(|face| {
        let tables = usize::from(u16_at(face + 4)?);
        (0..tables).find_map(|table| {
            let record = face + 12 + 16 * table;
            (font.get(record..record + 4)? == b"head")
                .then(|| u16_at(u32_at(record + 8)? as usize + 18))
                .flatten()
        })
    })
    .map_or(1000.0, f64::from)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use vello::kurbo::{Line, RoundedRect};
    use vello::peniko::{Compose, Mix};

    use super::*;

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 8.0, 8.0)
    }

    /// Shapes are kept as what they were recorded as: a rectangle and a
    /// rounded rectangle symbolically, anything else as a path.
    #[test]
    fn rectangles_stay_rectangles() {
        let mut list = DisplayList::new();
        list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        let rounded = RoundedRect::from_rect(rect(), 2.0);
        list.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &rounded,
        );
        list.stroke(
            &Stroke::new(1.0),
            Affine::IDENTITY,
            Color::BLACK,
            None,
            &Line::new((0.0, 0.0), (4.0, 0.0)),
        );
        let shapes: Vec<_> = list
            .items()
            .iter()
            .map(|item| match item {
                Item::Fill(draw) | Item::Stroke(draw, _) => draw.shape,
                _ => unreachable!("only draws were recorded"),
            })
            .collect();
        assert!(matches!(shapes[0], Shape::Rect(r) if r == rect()));
        assert!(matches!(shapes[1], Shape::RoundedRect(r) if r == rounded));
        assert!(matches!(shapes[2], Shape::Path { start: 0, end: 2 }));
    }

    /// A clip around a blend layer is isolated, and so is every clip around
    /// it; a clip around an opacity layer or a fill is not.
    #[test]
    fn a_clip_holding_a_blend_is_isolated() {
        let mut list = DisplayList::new();
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Saturation, Compose::SrcAtop),
            1.0,
            Affine::IDENTITY,
            &rect(),
        );
        list.fill(Fill::NonZero, Affine::IDENTITY, Color::BLACK, None, &rect());
        list.pop_layer();
        list.pop_layer();
        list.pop_layer();
        list.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        list.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &rect(),
        );
        list.pop_layer();
        list.pop_layer();
        let isolated: Vec<bool> = list
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::PushClip { isolated, .. } => Some(*isolated),
                _ => None,
            })
            .collect();
        assert_eq!(isolated, [true, true, false]);
        assert!(!list.blends_at_root());
    }

    /// Appending re-derives isolation across the seam: a fragment's blend
    /// isolates the clip its host has open around it.
    #[test]
    fn an_appended_blend_isolates_the_hosts_open_clip() {
        let mut fragment = DisplayList::new();
        fragment.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Multiply, Compose::SrcOver),
            1.0,
            Affine::IDENTITY,
            &rect(),
        );
        fragment.pop_layer();
        assert!(fragment.blends_at_root());
        let mut host = DisplayList::new();
        host.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &rect());
        host.append(&fragment, Some(Affine::translate((2.0, 0.0))));
        host.pop_layer();
        assert!(matches!(
            host.items()[0],
            Item::PushClip { isolated: true, .. }
        ));
        assert!(!host.blends_at_root());
        assert_eq!(host.open_layers(), 0);
    }

    /// An append maps every transform and lands the arenas at their new
    /// offsets, so the copy compares equal to the same drawing recorded in
    /// place.
    #[test]
    fn an_append_is_the_same_drawing_recorded_in_place() {
        let font = FontData::new(vello::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let record = |list: &mut DisplayList, outer: Affine| {
            list.stroke(
                &Stroke::new(1.0),
                outer,
                Color::BLACK,
                None,
                &Line::new((0.0, 0.0), (4.0, 0.0)),
            );
            list.draw_glyphs(&font)
                .transform(outer * Affine::translate((1.0, 2.0)))
                .normalized_coords(&[3, 4])
                .draw(
                    Fill::NonZero,
                    [Glyph {
                        id: 7,
                        x: 1.0,
                        y: 2.0,
                    }]
                    .into_iter(),
                );
        };
        let outer = Affine::translate((5.0, 6.0));
        let mut fragment = DisplayList::new();
        record(&mut fragment, Affine::IDENTITY);
        let mut appended = DisplayList::new();
        appended.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &rect());
        appended.append(&fragment, Some(outer));
        let mut expected = DisplayList::new();
        expected.fill(Fill::NonZero, Affine::IDENTITY, Color::WHITE, None, &rect());
        record(&mut expected, outer);
        assert_eq!(appended.compare(&expected), Ok(()));
    }

    /// A gradient of one stop is that colour, and one of none paints
    /// nothing.
    #[test]
    fn a_gradient_of_one_stop_is_its_colour() {
        let mut list = DisplayList::new();
        let one =
            Gradient::new_linear((0.0, 0.0), (1.0, 0.0)).with_stops([Color::WHITE].as_slice());
        list.fill(Fill::NonZero, Affine::IDENTITY, &one, None, &rect());
        let none = Gradient::new_linear((0.0, 0.0), (1.0, 0.0));
        list.fill(Fill::NonZero, Affine::IDENTITY, &none, None, &rect());
        let paints: Vec<_> = list
            .items()
            .iter()
            .map(|item| match item {
                Item::Fill(draw) => draw.paint.clone(),
                _ => unreachable!("only fills were recorded"),
            })
            .collect();
        assert_eq!(
            paints,
            [Paint::Solid(Color::WHITE), Paint::Solid(Color::TRANSPARENT)]
        );
    }

    /// `unitsPerEm` read from a font's `head` table, plain or in a
    /// collection, and 1000 for a font without one.
    #[test]
    #[expect(clippy::float_cmp, reason = "a unitsPerEm is an exact small integer")]
    fn units_per_em_come_from_the_head_table() {
        // An offset table of one record, `head` at 28, unitsPerEm at +18.
        let mut font = vec![0_u8; 28 + 54];
        font[4..6].copy_from_slice(&1_u16.to_be_bytes());
        font[12..16].copy_from_slice(b"head");
        font[20..24].copy_from_slice(&28_u32.to_be_bytes());
        font[28 + 18..28 + 20].copy_from_slice(&2048_u16.to_be_bytes());
        assert_eq!(units_per_em(&font, 0), 2048.0);
        // The same face as the second of a collection.
        let mut collection = vec![0_u8; 20];
        collection[..4].copy_from_slice(b"ttcf");
        collection[16..20].copy_from_slice(&20_u32.to_be_bytes());
        let mut face = font.clone();
        face[20..24].copy_from_slice(&(20_u32 + 28).to_be_bytes());
        collection.extend_from_slice(&face);
        assert_eq!(units_per_em(&collection, 1), 2048.0);
        assert_eq!(units_per_em(&[0; 12], 0), 1000.0);
    }

    /// An unhinted fill's synthetic bold is stored at the font's units per
    /// em; a stroke's stays in font-size units and scales with a uniform
    /// composed transform.
    #[test]
    #[expect(clippy::float_cmp, reason = "exact products of small powers of two")]
    fn synthetic_bold_is_stored_in_the_units_glifo_outlines_in() {
        // No `head` table: 1000 units per em.
        let font = FontData::new(vello::peniko::Blob::new(Arc::new([0_u8; 4])), 0);
        let glyph = [Glyph {
            id: 1,
            x: 0.0,
            y: 0.0,
        }];
        let mut list = DisplayList::new();
        for style in [
            StyleRef::Fill(Fill::NonZero),
            StyleRef::Stroke(&Stroke::new(1.0)),
        ] {
            list.draw_glyphs(&font)
                .font_size(20.0)
                .font_embolden(Embolden::uniform(0.5))
                .draw(style, glyph.into_iter());
        }
        let [fill, stroke] = list.runs() else {
            unreachable!("two runs were recorded")
        };
        assert_eq!(fill.embolden.x, 0.5 * 1000.0 / 20.0);
        assert_eq!(fill.outline_scale(Affine::scale(2.0)), 1.0);
        assert_eq!(stroke.embolden.x, 0.5);
        assert_eq!(stroke.outline_scale(Affine::scale(2.0)), 2.0);
        assert_eq!(
            stroke.outline_scale(Affine::scale_non_uniform(2.0, 1.0)),
            1.0
        );
    }
}
