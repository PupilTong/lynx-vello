//! The compose program: one commit's scene, split where the content's space
//! changes, so scroll offsets, sticky shifts and animation deltas apply at
//! composition instead of at encode.
//!
//! The walker's own layer discipline is preserved wholesale by construction:
//! every walker-level `push_layer`/`push_clip_layer`/`pop_layer` becomes a
//! program op carrying the space its shape rides, and everything painted
//! *between* those pushes — item fragments, mask patterns, filter
//! adjustments — lands in the current fragment, cut whenever the content's
//! space changes. Replaying the program with one instant's space maps
//! reproduces exactly the operation sequence the monolithic walk would have
//! encoded at those offsets: push ops re-encode their shapes under the mapped
//! transform, fragments append under the same map (`DisplayList::append`
//! left-multiplies the child's transform stream), and pops are pops.
//! Painter-internal pushes (background layers, text `SrcIn` sandwiches,
//! inset-shadow isolation) are balanced within one item and stay inside
//! fragments untouched.
//!
//! Two ops draw pictures whose pixels the composer supplies. A
//! [`ComposeOp::Image`] is a raster image: the frame carries its name and
//! geometry, and the painter reads its bitmap once per commit. A
//! [`ComposeOp::Vector`] is a vector image (an SVG document): the frame
//! carries its scene and geometry, and the painter's raster cache
//! ([`crate::render::vector_textures`]) bakes a texture of it at the draw's
//! device size, which the op draws through the same path as a bitmap. The
//! scene is never appended to a fragment, so the committed encoding costs
//! the same whatever the picture holds, and without a texture the op
//! encodes nothing.
//!
//! A space's map is [`SpaceSamples::css`]: its path's scroll, sticky and
//! animation nodes, root first (see [`crate::visual::space`]).
//!
//! One op pair is not a layer-stack operation: [`ComposeOp::PushFilter`] and
//! [`ComposeOp::PopFilter`] bracket the ops of a `filter: blur()` group. The
//! program carries both the bracket and the ops inside it, so the same
//! program serves two purposes — replayed with a baked texture for the group
//! the bracket is replaced by one `draw_image` and the range is skipped, and
//! replayed without one (no GPU, or a group over the memory budget) the range
//! plays raw and the frame simply is not blurred. The bake itself replays
//! that range into an offscreen scene; see
//! [`crate::visual::CommittedFrame::bake_filter`].
//!
//! [`ComposeOp::PushBackdrop`] is the `backdrop-filter` half of the same
//! machinery, and the one op whose range points *backwards*. Its entry sits
//! in the same side table and its range is everything already painted inside
//! the element's nearest Backdrop Root ancestor, so its bake reads program
//! ops that precede it rather than ops it brackets — there is no matching
//! pop. What that range is painted *over* is the entry's [`BackdropRoot`]:
//! the canvas when the root is the document's, which no program op draws and
//! the bake therefore takes as its base colour, and nothing when it is a
//! group. With a texture the op draws it, shaped by the element's own rounded
//! border box, as the first thing inside the element's group; without one it
//! draws nothing at all, and the unfiltered backdrop the op sits on top of
//! is what shows.
//!
//! Neither op clips. A group whose element has `filter` or `backdrop-filter`
//! opens inside the links of its own clip chain, which the walker records as
//! ordinary `Push` ops outside the group's layers, and its content pushes
//! only the links below them (see [`crate::paint::walker`]). So an
//! ancestor's `overflow` clip cuts the texture, the raw fallback and the
//! backdrop once, after the filter, and never lies inside an entry's range.
//!
//! A scroll container with `overscroll-behavior: circular` adds the seam
//! pass ([`replay_seams`]). While its scrollport straddles the seam — the
//! normalized offset is past `max_offset` — the frame shows the end of the
//! scrolling area followed by its start, and the fragments hold the content
//! only once. So the ops that ride the container's scroll node are replayed
//! a second time with its offset moved back by one period, right after the
//! last of them: the copy lands where the container's own content sits in
//! paint order, inside the same open layers (its scrollport clip among
//! them), and below whatever paints over the container later, such as a
//! positioned sibling. The second replay re-encodes every push and pop it
//! meets, so the layer stack stays balanced, but draws only what rides the
//! container; see [`replay_ops`]'s `riding`.

use std::ops::Range;
use std::sync::Arc;

use euclid::default::Vector2D;
use smallvec::SmallVec;

use crate::kurbo::{Affine, BezPath, Point, Rect, Shape, Size};
use crate::paint::display_list::{DisplayList, ImagePaint, ImageRef, Paint, Texture, may_reach};
use crate::paint::shape::{BoxShape, with_shape};
use crate::peniko::{BlendMode, Extend, Fill, ImageQuality, ImageSampler};
use crate::render::image::{
    AspectAlign, AspectRatio, ImageSizeHint, MAX_RENDERABLE_DIMENSION, renders_at,
};
use crate::render::picture::PictureKey;
use crate::visual::anchored::AnchoredSlot;
use crate::visual::space::{self, SeamPass, Space, SpaceSamples};
use crate::visual::{AnimationSamples, AnimationSlot};

#[derive(Debug)]
pub(crate) enum CapturedShape {
    Rect(Rect),
    Box(BoxShape),
    Path(Arc<BezPath>),
}

impl CapturedShape {
    fn bounding_box(&self) -> Rect {
        match self {
            Self::Rect(rect) => rect.abs(),
            Self::Box(shape) => shape.bounding_box().abs(),
            Self::Path(path) => path.bounding_box().abs(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum ImageArea {
    Fill(CapturedShape),
    Clipped { clip: BoxShape, draw: Rect },
}

impl ImageArea {
    fn reaches(&self, transform: Affine, within: Rect) -> bool {
        let bounds = match self {
            Self::Fill(shape) => shape.bounding_box(),
            Self::Clipped { draw, .. } => draw.abs(),
        };
        may_reach(bounds, transform, within)
    }
}

macro_rules! with_captured {
    ($shape:expr, |$s:ident| $body:expr) => {
        match $shape {
            CapturedShape::Rect($s) => $body,
            CapturedShape::Box(shape) => with_shape!(shape, |$s| $body),
            CapturedShape::Path(path) => {
                let $s: &BezPath = path;
                $body
            }
        }
    };
}

#[derive(Debug)]
pub(crate) struct ImageDraw {
    pub(crate) image: PictureKey,
    pub(crate) transform: Affine,
    pub(crate) anchor: Point,
    pub(crate) extent: Size,
    pub(crate) sampler: ImageSampler,
    pub(crate) area: ImageArea,
}

impl ImageDraw {
    pub(crate) fn size_hint(&self) -> ImageSizeHint {
        let [a, b, c, d, _, _] = self.transform.as_coeffs();
        ImageSizeHint::new(
            device_length(self.extent.width * a.hypot(b)),
            device_length(self.extent.height * c.hypot(d)),
        )
    }
}

fn device_length(length: f64) -> u32 {
    if !length.is_finite() {
        return u32::MAX;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u32 range immediately before the cast"
    )]
    let length = length.ceil().clamp(0.0, f64::from(u32::MAX)) as u32;
    length
}

pub(crate) struct VectorDraw {
    pub(crate) scene: Arc<DisplayList>,
    pub(crate) key: u64,
    pub(crate) viewport: (f32, f32),
    pub(crate) aspect: AspectRatio,
    pub(crate) transform: Affine,
    pub(crate) anchor: Point,
    pub(crate) extent: Size,
    pub(crate) sampler: ImageSampler,
    pub(crate) area: ImageArea,
}

impl std::fmt::Debug for VectorDraw {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VectorDraw")
            .field("key", &self.key)
            .field("viewport", &self.viewport)
            .field("transform", &self.transform)
            .field("anchor", &self.anchor)
            .field("extent", &self.extent)
            .field("area", &self.area)
            .finish_non_exhaustive()
    }
}

impl VectorDraw {
    pub(crate) fn device_extent(&self) -> Size {
        let [a, b, c, d, _, _] = self.transform.as_coeffs();
        Size::new(
            self.extent.width * a.hypot(b),
            self.extent.height * c.hypot(d),
        )
    }

    pub(crate) fn device_size(&self) -> (u32, u32) {
        let extent = self.device_extent();
        (
            device_length(extent.width).min(MAX_RENDERABLE_DIMENSION),
            device_length(extent.height).min(MAX_RENDERABLE_DIMENSION),
        )
    }

    pub(crate) fn placement(&self, size: Size, raster: Size) -> Affine {
        let ratio = |raster: f64, size: f64| if size > 0.0 { raster / size } else { 1.0 };
        Affine::scale_non_uniform(
            ratio(raster.width, size.width),
            ratio(raster.height, size.height),
        ) * aspect_transform(self.viewport, self.aspect, size)
    }
}

pub(crate) fn aspect_transform(viewport: (f32, f32), aspect: AspectRatio, size: Size) -> Affine {
    let (width, height) = (f64::from(viewport.0), f64::from(viewport.1));
    let (sx, sy) = (size.width / width, size.height / height);
    let Some((align_x, align_y)) = aspect.align else {
        return Affine::scale_non_uniform(sx, sy);
    };
    let scale = if aspect.slice { sx.max(sy) } else { sx.min(sy) };
    let offset = |align: AspectAlign, slack: f64| match align {
        AspectAlign::Min => 0.0,
        AspectAlign::Mid => slack / 2.0,
        AspectAlign::Max => slack,
    };
    Affine::new([
        scale,
        0.0,
        0.0,
        scale,
        offset(align_x, size.width - width * scale),
        offset(align_y, size.height - height * scale),
    ])
}

fn encode_vector(
    scene: &mut DisplayList,
    draws: &[VectorDraw],
    textures: &[Option<Texture>],
    index: u32,
    outer: Affine,
) {
    let index = index as usize;
    if let Some(Some(texture)) = textures.get(index) {
        let draw = &draws[index];
        encode_textured(
            scene,
            outer * draw.transform,
            draw.anchor,
            draw.extent,
            draw.sampler,
            &draw.area,
            Picture::Texture(*texture),
        );
    }
}

#[derive(Clone, Copy)]
enum Picture<'a> {
    Bitmap(&'a ImageRef),
    Texture(Texture),
}

impl Picture<'_> {
    fn size(self) -> (u32, u32) {
        match self {
            Self::Bitmap(image) => (image.descriptor.width, image.descriptor.height),
            Self::Texture(texture) => texture.size(),
        }
    }

    fn paint(self, sampler: ImageSampler) -> Paint {
        match self {
            Self::Bitmap(image) => Paint::Image(Arc::new(ImagePaint {
                image: image.clone(),
                sampler,
            })),
            Self::Texture(texture) => Paint::texture(texture, sampler),
        }
    }
}

pub(crate) fn encode_vector_inline(scene: &mut DisplayList, draw: &VectorDraw, outer: Affine) {
    let transform = outer * draw.transform;
    let layers = match &draw.area {
        ImageArea::Fill(shape) => {
            with_captured!(shape, |s| scene.push_clip_layer(
                Fill::NonZero,
                transform,
                s
            ));
            1
        }
        ImageArea::Clipped { clip, draw: rect } => {
            with_shape!(clip, |s| scene.push_clip_layer(Fill::NonZero, transform, s));
            scene.push_clip_layer(Fill::NonZero, transform, rect);
            2
        }
    };
    let placement = transform
        * Affine::translate(draw.anchor.to_vec2())
        * draw.placement(draw.extent, draw.extent);
    scene.append(&draw.scene, Some(placement));
    for _ in 0..layers {
        scene.pop_layer();
    }
}

fn encode_draw(
    scene: &mut DisplayList,
    draws: &[ImageDraw],
    images: &[Option<ImageRef>],
    index: u32,
    outer: Affine,
) {
    let index = index as usize;
    if let Some(Some(image)) = images.get(index) {
        encode_image(scene, &draws[index], outer, image);
    }
}

pub(crate) fn encode_image(
    scene: &mut DisplayList,
    draw: &ImageDraw,
    outer: Affine,
    image: &ImageRef,
) {
    if !renders_at(image.descriptor.width, image.descriptor.height) {
        return;
    }
    encode_textured(
        scene,
        outer * draw.transform,
        draw.anchor,
        draw.extent,
        draw.sampler,
        &draw.area,
        Picture::Bitmap(image),
    );
}

fn encode_textured(
    scene: &mut DisplayList,
    transform: Affine,
    anchor: Point,
    extent: Size,
    sampler: ImageSampler,
    area: &ImageArea,
    picture: Picture<'_>,
) {
    let (width, height) = picture.size();
    let brush_transform = Affine::translate(anchor.to_vec2())
        * Affine::scale_non_uniform(
            extent.width / f64::from(width),
            extent.height / f64::from(height),
        );
    let paint = picture.paint(sampler);
    match area {
        ImageArea::Fill(shape) => {
            with_captured!(shape, |s| scene.fill_paint(
                Fill::NonZero,
                transform,
                paint,
                Some(brush_transform),
                s
            ));
        }
        ImageArea::Clipped { clip, draw: rect } => {
            with_shape!(clip, |s| scene.push_clip_layer(Fill::NonZero, transform, s));
            scene.fill_paint(Fill::NonZero, transform, paint, Some(brush_transform), rect);
            scene.pop_layer();
        }
    }
}

struct RangeScan {
    scrolls: bool,
    animations: bool,
    pictures: Range<u32>,
    open_pushes: u32,
}

fn cover(covering: &mut Range<u32>, more: Range<u32>) {
    if more.start >= more.end {
        return;
    }
    *covering = if covering.start >= covering.end {
        more
    } else {
        covering.start.min(more.start)..covering.end.max(more.end)
    };
}

pub(crate) enum ComposeOp {
    Fragment {
        index: u32,
        space: Option<u32>,
    },
    Push {
        clip_only: bool,
        fill: Fill,
        blend: BlendMode,
        alpha: f32,
        transform: Affine,
        shape: CapturedShape,
        space: Option<u32>,
        alpha_animation: Option<u32>,
    },
    Pop,
    Image {
        index: u32,
        space: Option<u32>,
    },
    Vector {
        index: u32,
        space: Option<u32>,
    },
    PushFilter {
        index: u32,
    },
    PopFilter,
    PushBackdrop {
        index: u32,
    },
}

impl ComposeOp {
    #[expect(
        clippy::option_option,
        reason = "the outer level is whether the op rides a space at all; \
                  the inner is the root or a node"
    )]
    pub(crate) fn space(&self, groups: &[FilterGroup]) -> Option<Option<u32>> {
        match self {
            Self::Fragment { space, .. }
            | Self::Push { space, .. }
            | Self::Image { space, .. }
            | Self::Vector { space, .. } => Some(*space),
            Self::PushFilter { index } | Self::PushBackdrop { index } => {
                Some(groups[*index as usize].space)
            }
            Self::Pop | Self::PopFilter => None,
        }
    }
}

fn differs(
    spaces: &[Space],
    a: Option<u32>,
    b: Option<u32>,
    pick: fn(&[Space], Option<u32>) -> Option<u32>,
) -> bool {
    pick(spaces, a) != pick(spaces, b)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackdropRoot {
    Document,
    Group { content_start: u32 },
}

impl BackdropRoot {
    pub(crate) fn content_start(self) -> u32 {
        match self {
            Self::Document => 0,
            Self::Group { content_start } => content_start,
        }
    }
}

#[derive(Debug)]
pub(crate) struct Backdrop {
    pub(crate) shape: BoxShape,
    pub(crate) transform: Affine,
    pub(crate) before: Vec<crate::paint::filters::Pass>,
    pub(crate) after: Vec<crate::paint::filters::Pass>,
    pub(crate) root: BackdropRoot,
    pub(crate) open_pushes: u32,
}

#[derive(Debug)]
pub struct FilterGroup {
    pub sigma: f32,
    pub rect: Rect,
    pub(crate) space: Option<u32>,
    pub(crate) ops: Range<u32>,
    pub(crate) inner_chains: bool,
    pub(crate) inner_animations: bool,
    pub(crate) pictures: Range<u32>,
    pub(crate) parent: Option<u32>,
    pub(crate) backdrop: Option<Backdrop>,
}

impl FilterGroup {
    pub(crate) fn new(sigma: f32, rect: Rect, space: Option<u32>) -> Self {
        Self {
            sigma,
            rect,
            space,
            ops: 0..0,
            inner_chains: false,
            inner_animations: false,
            pictures: 0..0,
            parent: None,
            backdrop: None,
        }
    }

    pub(crate) fn with_backdrop(
        sigma: f32,
        rect: Rect,
        space: Option<u32>,
        backdrop: Backdrop,
    ) -> Self {
        Self {
            backdrop: Some(backdrop),
            ..Self::new(sigma, rect, space)
        }
    }

    #[must_use]
    pub fn is_backdrop(&self) -> bool {
        self.backdrop.is_some()
    }

    #[must_use]
    pub fn reads_canvas(&self) -> bool {
        self.backdrop
            .as_ref()
            .is_some_and(|backdrop| backdrop.root == BackdropRoot::Document)
    }

    #[must_use]
    pub fn samples_animations(&self) -> bool {
        self.inner_animations
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the walker records only finite, positive, integer-valued rects"
        )]
        {
            (self.rect.width() as u32, self.rect.height() as u32)
        }
    }
}

#[derive(Default)]
pub(crate) struct ComposeAssembly {
    pub(crate) fragments: Vec<DisplayList>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<ImageDraw>,
    pub(crate) vector_draws: Vec<VectorDraw>,
    pub(crate) filter_groups: Vec<FilterGroup>,
    #[expect(
        clippy::option_option,
        reason = "the outer level is whether a fragment is open; the inner \
                  is its space, the root or a node"
    )]
    current: Option<Option<u32>>,
    open_filter: Option<u32>,
    pool: Vec<DisplayList>,
}

impl std::fmt::Debug for ComposeAssembly {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ComposeAssembly")
            .field("fragments", &self.fragments.len())
            .field("program", &self.program.len())
            .finish_non_exhaustive()
    }
}

impl ComposeAssembly {
    pub(crate) fn with_storage(
        fragments: Vec<DisplayList>,
        program: Vec<ComposeOp>,
        image_draws: Vec<ImageDraw>,
        vector_draws: Vec<VectorDraw>,
        filter_groups: Vec<FilterGroup>,
        pool: Vec<DisplayList>,
    ) -> Self {
        debug_assert!(
            fragments.is_empty()
                && program.is_empty()
                && image_draws.is_empty()
                && vector_draws.is_empty()
                && filter_groups.is_empty(),
            "recycled containers are emptied before they are handed back",
        );
        Self {
            fragments,
            program,
            image_draws,
            vector_draws,
            filter_groups,
            current: None,
            open_filter: None,
            pool,
        }
    }

    pub(crate) fn fragment_for(&mut self, space: Option<u32>) -> &mut DisplayList {
        if self.current != Some(space) {
            self.seal_fragment();
            let mut scene = self.pool.pop().unwrap_or_default();
            scene.reset();
            self.fragments.push(scene);
            self.current = Some(space);
        }
        self.fragments
            .last_mut()
            .expect("an open fragment was just ensured")
    }

    pub(crate) fn push_op(&mut self, op: ComposeOp) {
        self.seal_fragment();
        self.program.push(op);
    }

    pub(crate) fn push_image(&mut self, space: Option<u32>, draw: ImageDraw) {
        let index = u32::try_from(self.image_draws.len()).expect("a frame cannot hold 2^32 images");
        self.image_draws.push(draw);
        self.push_op(ComposeOp::Image { index, space });
    }

    pub(crate) fn content_draws(&self) -> usize {
        self.image_draws.len() + self.vector_draws.len()
    }

    pub(crate) fn push_vector(&mut self, space: Option<u32>, draw: VectorDraw) {
        let index =
            u32::try_from(self.vector_draws.len()).expect("a frame cannot hold 2^32 vector draws");
        self.vector_draws.push(draw);
        self.push_op(ComposeOp::Vector { index, space });
    }

    fn seal_fragment(&mut self) {
        let Some(space) = self.current.take() else {
            return;
        };
        let fragment = self.fragments.last().expect("an open fragment has a list");
        debug_assert_eq!(
            fragment.open_layers(),
            0,
            "a fragment must never be cut inside a painter-internal layer",
        );
        if fragment.is_empty() {
            let scene = self.fragments.pop().expect("the open fragment exists");
            self.pool.push(scene);
            return;
        }
        self.fragments
            .last_mut()
            .expect("the open fragment exists")
            .seal_bounds();
        let index =
            u32::try_from(self.fragments.len() - 1).expect("a frame cannot hold 2^32 fragments");
        self.program.push(ComposeOp::Fragment { index, space });
    }

    pub(crate) fn push_filter(&mut self, mut group: FilterGroup) -> u32 {
        self.seal_fragment();
        let index =
            u32::try_from(self.filter_groups.len()).expect("a frame cannot hold 2^32 filters");
        let start =
            u32::try_from(self.program.len() + 1).expect("a frame cannot hold 2^32 program ops");
        group.parent = self.open_filter;
        group.ops = start..start;
        self.filter_groups.push(group);
        self.program.push(ComposeOp::PushFilter { index });
        self.open_filter = Some(index);
        index
    }

    pub(crate) fn content_boundary(&mut self) -> u32 {
        self.seal_fragment();
        u32::try_from(self.program.len()).expect("a frame cannot hold 2^32 program ops")
    }

    pub(crate) fn push_backdrop(
        &mut self,
        mut entry: FilterGroup,
        end: u32,
        spaces: &[Space],
        slots: &[AnimationSlot],
        anchored: &[AnchoredSlot],
    ) -> bool {
        self.seal_fragment();
        let Some(root) = entry.backdrop.as_ref().map(|backdrop| backdrop.root) else {
            debug_assert!(false, "push_backdrop is only called with a backdrop entry");
            return false;
        };
        let ops = root.content_start()..end;
        if ops.is_empty() && root != BackdropRoot::Document {
            return false;
        }
        debug_assert!(
            ops.end as usize <= self.program.len(),
            "a backdrop's range ends at or before the op that draws it",
        );
        let scan = self.scan_range(&ops, entry.space, spaces, slots, anchored);
        let index =
            u32::try_from(self.filter_groups.len()).expect("a frame cannot hold 2^32 filters");
        entry.parent = self.open_filter;
        entry.ops = ops;
        entry.inner_chains = scan.scrolls;
        entry.inner_animations = scan.animations;
        entry.pictures = scan.pictures;
        if let Some(backdrop) = entry.backdrop.as_mut() {
            backdrop.open_pushes = scan.open_pushes;
        }
        self.filter_groups.push(entry);
        self.program.push(ComposeOp::PushBackdrop { index });
        true
    }

    fn scan_range(
        &self,
        ops: &Range<u32>,
        own: Option<u32>,
        spaces: &[Space],
        slots: &[AnimationSlot],
        anchored: &[AnchoredSlot],
    ) -> RangeScan {
        let mut scrolls = false;
        let mut animations = false;
        let mut pictures = 0..0;
        let mut depth = 0_i64;
        for op in &self.program[ops.start as usize..ops.end as usize] {
            match op {
                ComposeOp::Push { .. } => depth += 1,
                ComposeOp::Pop => depth -= 1,
                ComposeOp::Image { index, .. } => cover(&mut pictures, *index..*index + 1),
                ComposeOp::PushBackdrop { index } => {
                    let drawn = &self.filter_groups[*index as usize];
                    scrolls |= drawn.inner_chains;
                    animations |= drawn.inner_animations;
                    cover(&mut pictures, drawn.pictures.clone());
                }
                _ => {}
            }
            if let Some(op) = op.space(&self.filter_groups) {
                scrolls |= differs(spaces, op, own, space::nearest_scroll)
                    || differs(spaces, op, own, space::nearest_sticky)
                    || differs(spaces, op, own, space::nearest_anchored);
                animations |= space::sampled_against(spaces, slots, anchored, op, own);
            }
        }
        debug_assert!(depth >= 0, "a range pops a layer it never pushed");
        RangeScan {
            scrolls,
            animations,
            pictures,
            open_pushes: u32::try_from(depth.max(0)).expect("a frame cannot nest 2^32 layers"),
        }
    }

    pub(crate) fn pop_filter(
        &mut self,
        spaces: &[Space],
        slots: &[AnimationSlot],
        anchored: &[AnchoredSlot],
    ) {
        self.seal_fragment();
        let Some(index) = self.open_filter else {
            debug_assert!(false, "pop_filter is only called with an open filter group");
            return;
        };
        let end = u32::try_from(self.program.len()).expect("a frame cannot hold 2^32 program ops");
        let (own, parent, start) = {
            let group = &mut self.filter_groups[index as usize];
            group.ops.end = end;
            (group.space, group.parent, group.ops.start)
        };
        let scan = self.scan_range(&(start..end), own, spaces, slots, anchored);
        let group = &mut self.filter_groups[index as usize];
        group.inner_chains = scan.scrolls;
        group.inner_animations = scan.animations;
        group.pictures = scan.pictures;
        self.open_filter = parent;
        self.program.push(ComposeOp::PopFilter);
    }

    pub(crate) fn finish(mut self) -> Finished {
        self.seal_fragment();
        debug_assert!(
            self.open_filter.is_none(),
            "every filter group the walk opened is closed before the frame is sealed",
        );
        Finished {
            fragments: self.fragments,
            program: self.program,
            image_draws: self.image_draws,
            vector_draws: self.vector_draws,
            filter_groups: self.filter_groups,
            pool: self.pool,
        }
    }
}

pub(crate) struct Finished {
    pub(crate) fragments: Vec<DisplayList>,
    pub(crate) program: Vec<ComposeOp>,
    pub(crate) image_draws: Vec<ImageDraw>,
    pub(crate) vector_draws: Vec<VectorDraw>,
    pub(crate) filter_groups: Vec<FilterGroup>,
    pub(crate) pool: Vec<DisplayList>,
}

pub(crate) fn snap_offset(offset: Vector2D<f32>, ratio: f32) -> Vector2D<f32> {
    if ratio.is_finite() && ratio > 0.0 {
        Vector2D::new(
            (offset.x * ratio).round() / ratio,
            (offset.y * ratio).round() / ratio,
        )
    } else {
        offset
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one replay's full inputs: the program, its side tables, and the samples"
)]
pub(crate) fn replay(
    scene: &mut DisplayList,
    visible: Rect,
    fragments: &[DisplayList],
    program: &[ComposeOp],
    image_draws: &[ImageDraw],
    images: &[Option<ImageRef>],
    vector_draws: &[VectorDraw],
    vectors: &[Option<Texture>],
    filter_groups: &[FilterGroup],
    filtered: &[Option<Texture>],
    samples: &SpaceSamples<'_>,
    seams: &[SeamPass],
) {
    replay_seams(
        scene,
        Tables {
            fragments,
            program,
            image_draws,
            images,
            vector_draws,
            vectors,
            filter_groups,
            filtered,
            spaces: samples.spaces,
            samples: samples.animations,
        },
        0..program.len(),
        samples,
        seams,
        ReplayTarget::Frame { visible },
    );
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ReplayTarget {
    Frame {
        visible: Rect,
    },
    Bake {
        entry: Option<u32>,
        origin: (f64, f64),
        size: (u32, u32),
    },
}

impl ReplayTarget {
    fn with_map<R>(
        self,
        samples: &SpaceSamples<'_>,
        body: impl FnOnce(&dyn Fn(Option<u32>) -> Affine) -> R,
    ) -> R {
        match self {
            Self::Frame { .. } => body(&samples.device_cached()),
            Self::Bake { entry, origin, .. } => body(&samples.bake_map(entry, origin)),
        }
    }

    fn visible(self) -> Rect {
        match self {
            Self::Frame { visible } => visible,
            Self::Bake { size, .. } => Rect::new(0.0, 0.0, f64::from(size.0), f64::from(size.1)),
        }
    }

    fn within(self) -> Option<u32> {
        match self {
            Self::Frame { .. } => None,
            Self::Bake { entry, .. } => entry,
        }
    }
}

struct OpWalk {
    ops: Vec<(usize, u32)>,
    depth: u32,
}

impl OpWalk {
    fn new(tables: Tables<'_>, ops: Range<usize>) -> Self {
        let mut walk = Vec::new();
        let mut depth = 0_u32;
        let mut index = ops.start;
        while index < ops.end {
            walk.push((index, depth));
            index = match &tables.program[index] {
                ComposeOp::Push { .. } => {
                    depth += 1;
                    index + 1
                }
                ComposeOp::Pop => {
                    depth = depth.saturating_sub(1);
                    index + 1
                }
                ComposeOp::PushFilter { index: group }
                    if matches!(tables.filtered.get(*group as usize), Some(Some(_))) =>
                {
                    (tables.filter_groups[*group as usize].ops.end as usize).max(index + 1)
                }
                _ => index + 1,
            };
        }
        Self { ops: walk, depth }
    }

    fn next(&self, position: usize, end: usize) -> usize {
        self.ops.get(position + 1).map_or(end, |&(index, _)| index)
    }

    fn depth_after(&self, position: usize) -> u32 {
        self.ops
            .get(position + 1)
            .map_or(self.depth, |&(_, depth)| depth)
    }

    fn seam_run(&self, tables: Tables<'_>, slot: u32, shift: Vector2D<f32>) -> Option<SeamRun> {
        let walk = &self.ops;
        let draws = |&(index, _): &(usize, u32)| draws_riding(tables, &tables.program[index], slot);
        let first = walk.iter().position(draws)?;
        let last = walk.iter().rposition(draws)?;
        let mut stack: SmallVec<[usize; 8]> = SmallVec::new();
        for &(index, _) in &walk[..first] {
            match &tables.program[index] {
                ComposeOp::Push { .. } => stack.push(index),
                ComposeOp::Pop => {
                    stack.pop();
                }
                _ => {}
            }
        }
        let riding_layer = stack.iter().position(|&index| {
            matches!(&tables.program[index], ComposeOp::Push { space, .. }
                if space::rides(tables.spaces, *space, slot))
        });
        let floor = walk[first..=last]
            .iter()
            .map(|&(_, depth)| depth)
            .chain(std::iter::once(self.depth_after(last)))
            .chain(riding_layer.and_then(|layer| u32::try_from(layer).ok()))
            .min()
            .unwrap_or(0);
        let start = walk[..=first]
            .iter()
            .rposition(|&(_, depth)| depth == floor)
            .unwrap_or(first);
        let stop = (last..walk.len())
            .find(|&position| self.depth_after(position) == floor)
            .unwrap_or(walk.len() - 1);
        Some(SeamRun {
            slot,
            shift,
            first: start,
            last: stop,
            open: self.depth_after(stop).saturating_sub(floor),
        })
    }
}

struct SeamRun {
    slot: u32,
    shift: Vector2D<f32>,
    first: usize,
    last: usize,
    open: u32,
}

pub(crate) fn replay_seams(
    scene: &mut DisplayList,
    tables: Tables<'_>,
    ops: Range<usize>,
    samples: &SpaceSamples<'_>,
    seams: &[SeamPass],
    target: ReplayTarget,
) {
    let within = target.within();
    let visible = target.visible();
    let passes: SmallVec<[&SeamPass; 2]> = seams
        .iter()
        .filter(|pass| !space::rides(tables.spaces, within, pass.slot))
        .collect();
    if passes.is_empty() {
        target.with_map(samples, |map| {
            replay_ops(scene, tables, ops, map, visible, None);
        });
        return;
    }

    let end = ops.end.min(tables.program.len());
    let walk = OpWalk::new(tables, ops.start.min(end)..end);
    let mut runs: SmallVec<[(SeamRun, &SeamPass); 2]> = passes
        .into_iter()
        .filter_map(|pass| {
            walk.seam_run(tables, pass.slot, pass.shift)
                .map(|run| (run, pass))
        })
        .collect();
    runs.sort_by_key(|(run, _)| run.last);
    let next = |position: usize| walk.next(position, end);
    let walk = &walk.ops;

    target.with_map(samples, |map| {
        let mut cursor = ops.start.min(end);
        for (run, pass) in &runs {
            let after = next(run.last);
            replay_ops(scene, tables, cursor..after, map, visible, None);
            cursor = cursor.max(after);
            let offsets = samples.shifted_offsets(run.slot, run.shift);
            let shifted = SpaceSamples {
                offset_of: &offsets,
                animations: &pass.animations,
                ..*samples
            };
            let tables = Tables {
                samples: &pass.animations,
                ..tables
            };
            target.with_map(&shifted, |shifted_map| {
                replay_ops(
                    scene,
                    tables,
                    walk[run.first].0..after,
                    shifted_map,
                    visible,
                    Some(run.slot),
                );
            });
            for _ in 0..run.open {
                scene.pop_layer();
            }
        }
        replay_ops(scene, tables, cursor..end, map, visible, None);
    });
}

fn draws_riding(tables: Tables<'_>, op: &ComposeOp, slot: u32) -> bool {
    match op {
        ComposeOp::Fragment { space, .. }
        | ComposeOp::Image { space, .. }
        | ComposeOp::Vector { space, .. } => space::rides(tables.spaces, *space, slot),
        ComposeOp::PushFilter { index } => {
            matches!(tables.filtered.get(*index as usize), Some(Some(_)))
                && space::rides(
                    tables.spaces,
                    tables.filter_groups[*index as usize].space,
                    slot,
                )
        }
        ComposeOp::PushBackdrop { index } => space::rides(
            tables.spaces,
            tables.filter_groups[*index as usize].space,
            slot,
        ),
        ComposeOp::Push { .. } | ComposeOp::Pop | ComposeOp::PopFilter => false,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Tables<'a> {
    pub(crate) fragments: &'a [DisplayList],
    pub(crate) program: &'a [ComposeOp],
    pub(crate) image_draws: &'a [ImageDraw],
    pub(crate) images: &'a [Option<ImageRef>],
    pub(crate) vector_draws: &'a [VectorDraw],
    pub(crate) vectors: &'a [Option<Texture>],
    pub(crate) filter_groups: &'a [FilterGroup],
    pub(crate) filtered: &'a [Option<Texture>],
    pub(crate) spaces: &'a [Space],
    pub(crate) samples: &'a AnimationSamples,
}

pub(crate) fn replay_ops(
    scene: &mut DisplayList,
    tables: Tables<'_>,
    ops: Range<usize>,
    device_transform: &dyn Fn(Option<u32>) -> Affine,
    visible: Rect,
    riding: Option<u32>,
) {
    let Tables {
        fragments,
        program,
        image_draws,
        images,
        vector_draws,
        vectors,
        filter_groups,
        filtered,
        spaces,
        samples,
    } = tables;
    let draws = |space: Option<u32>| riding.is_none_or(|slot| space::rides(spaces, space, slot));
    let end = ops.end.min(program.len());
    let mut index = ops.start.min(end);
    let mut reach: SmallVec<[Rect; 16]> = SmallVec::new();
    reach.push(visible);
    while index < end {
        let within = *reach
            .last()
            .expect("the target's own rectangle is never popped");
        match &program[index] {
            ComposeOp::Fragment {
                index: fragment,
                space,
            } => {
                if draws(*space) {
                    let transform = device_transform(*space);
                    scene.append_visible(&fragments[*fragment as usize], transform, within);
                }
            }
            op @ ComposeOp::Push {
                transform,
                shape,
                space,
                ..
            } => {
                let transform = device_transform(*space) * *transform;
                let bounds = shape.bounding_box();
                if !may_reach(bounds, transform, within)
                    && let Some(pop) = matching_pop(program, index, end)
                {
                    index = pop + 1;
                    continue;
                }
                reach.push(cut_reach(within, bounds, transform));
                encode_push(scene, op, transform, samples);
            }
            ComposeOp::Image { index: draw, space } => {
                let transform = device_transform(*space);
                let image = &image_draws[*draw as usize];
                if draws(*space) && image.area.reaches(transform * image.transform, within) {
                    encode_draw(scene, image_draws, images, *draw, transform);
                }
            }
            ComposeOp::Vector { index: draw, space } => {
                let transform = device_transform(*space);
                let vector = &vector_draws[*draw as usize];
                if draws(*space) && vector.area.reaches(transform * vector.transform, within) {
                    encode_vector(scene, vector_draws, vectors, *draw, transform);
                }
            }
            ComposeOp::Pop => {
                if reach.len() > 1 {
                    reach.pop();
                }
                scene.pop_layer();
            }
            ComposeOp::PushFilter { index: group } => {
                let group_index = *group as usize;
                let group = &filter_groups[group_index];
                if let Some(&Some(texture)) = filtered.get(group_index) {
                    if draws(group.space)
                        && reaches_filtered(group, texture, device_transform, within)
                    {
                        draw_filtered(scene, group, texture, spaces, device_transform);
                    }
                    index = group.ops.end as usize;
                    continue;
                }
            }
            ComposeOp::PopFilter => {}
            ComposeOp::PushBackdrop { index: slot } => {
                let slot = *slot as usize;
                let entry = &filter_groups[slot];
                if let (Some(&Some(texture)), Some(backdrop)) =
                    (filtered.get(slot), &entry.backdrop)
                    && draws(entry.space)
                {
                    let animated = space::nearest_animation(spaces, entry.space).is_some();
                    draw_backdrop(scene, entry, backdrop, texture, animated, device_transform);
                }
            }
        }
        index += 1;
    }
}

fn encode_push(
    scene: &mut DisplayList,
    op: &ComposeOp,
    transform: Affine,
    samples: &AnimationSamples,
) {
    let ComposeOp::Push {
        clip_only,
        fill,
        blend,
        alpha,
        shape,
        alpha_animation,
        ..
    } = op
    else {
        return;
    };
    let alpha = alpha_animation
        .and_then(|slot| samples.get(slot).alpha)
        .unwrap_or(*alpha);
    if *clip_only {
        with_captured!(shape, |s| scene.push_clip_layer(*fill, transform, s));
    } else {
        with_captured!(shape, |s| scene
            .push_layer(*fill, *blend, alpha, transform, s));
    }
}

fn matching_pop(program: &[ComposeOp], push: usize, end: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (at, op) in program.iter().enumerate().take(end).skip(push + 1) {
        match op {
            ComposeOp::Push { .. } => depth += 1,
            ComposeOp::Pop if depth == 0 => return Some(at),
            ComposeOp::Pop => depth -= 1,
            _ => {}
        }
    }
    None
}

fn cut_reach(within: Rect, bounds: Rect, transform: Affine) -> Rect {
    if !bounds.is_finite() || !transform.is_finite() {
        return within;
    }
    let mapped = transform.transform_rect_bbox(bounds);
    if mapped.is_finite() {
        mapped.intersect(within)
    } else {
        within
    }
}

fn reaches_filtered(
    group: &FilterGroup,
    texture: Texture,
    device_transform: &dyn Fn(Option<u32>) -> Affine,
    within: Rect,
) -> bool {
    let (width, height) = texture.size();
    may_reach(
        Rect::new(
            group.rect.x0,
            group.rect.y0,
            group.rect.x0 + f64::from(width),
            group.rect.y0 + f64::from(height),
        ),
        device_transform(group.space),
        within,
    )
}

fn draw_filtered(
    scene: &mut DisplayList,
    group: &FilterGroup,
    texture: Texture,
    spaces: &[Space],
    device_transform: &dyn Fn(Option<u32>) -> Affine,
) {
    let quality = if space::nearest_animation(spaces, group.space).is_none() {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    };
    let (width, height) = texture.size();
    scene.fill_paint(
        Fill::NonZero,
        device_transform(group.space) * Affine::translate((group.rect.x0, group.rect.y0)),
        Paint::texture(
            texture,
            ImageSampler {
                x_extend: Extend::Pad,
                y_extend: Extend::Pad,
                quality,
                alpha: 1.0,
            },
        ),
        None,
        &Rect::new(0.0, 0.0, f64::from(width), f64::from(height)),
    );
}

fn draw_backdrop(
    scene: &mut DisplayList,
    entry: &FilterGroup,
    backdrop: &Backdrop,
    texture: Texture,
    animated: bool,
    device_transform: &dyn Fn(Option<u32>) -> Affine,
) {
    let outer = device_transform(entry.space);
    let transform = outer * backdrop.transform;
    let brush_transform =
        backdrop.transform.inverse() * Affine::translate((entry.rect.x0, entry.rect.y0));
    let quality = if !animated && is_integer_translation(outer) {
        ImageQuality::Low
    } else {
        ImageQuality::Medium
    };
    let paint = Paint::texture(
        texture,
        ImageSampler {
            x_extend: Extend::Pad,
            y_extend: Extend::Pad,
            quality,
            alpha: 1.0,
        },
    );
    if !backdrop.after.is_empty() {
        let bounds = backdrop.shape.bounding_box();
        with_shape!(&backdrop.shape, |s| scene.push_clip_layer(
            Fill::NonZero,
            transform,
            s
        ));
        scene.fill_paint(
            Fill::NonZero,
            transform,
            paint,
            Some(brush_transform),
            &bounds,
        );
        crate::paint::filters::draw_passes_rect(scene, &backdrop.after, bounds, transform);
        scene.pop_layer();
        return;
    }
    with_shape!(&backdrop.shape, |s| scene.fill_paint(
        Fill::NonZero,
        transform,
        paint,
        Some(brush_transform),
        s
    ));
    crate::paint::filters::draw_passes_shape(scene, &backdrop.after, &backdrop.shape, transform);
}

fn is_integer_translation(affine: Affine) -> bool {
    let coefficients = affine.as_coeffs();
    coefficients[..4] == [1.0, 0.0, 0.0, 1.0]
        && coefficients[4].fract() == 0.0
        && coefficients[5].fract() == 0.0
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::peniko::{Compose, Mix};

    fn assembly() -> ComposeAssembly {
        ComposeAssembly::with_storage(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    fn push(space: Option<u32>) -> ComposeOp {
        ComposeOp::Push {
            clip_only: true,
            fill: Fill::NonZero,
            blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
            alpha: 1.0,
            transform: Affine::IDENTITY,
            shape: CapturedShape::Rect(Rect::ZERO),
            space,
            alpha_animation: None,
        }
    }

    fn group(space: Option<u32>) -> FilterGroup {
        FilterGroup::new(2.0, Rect::new(0.0, 0.0, 8.0, 8.0), space)
    }

    const SPACES: [Space; 2] = [
        Space {
            parent: None,
            kind: crate::visual::SpaceKind::Scroll(0),
        },
        Space {
            parent: None,
            kind: crate::visual::SpaceKind::Animation(0),
        },
    ];
    const SCROLLED: Option<u32> = Some(0);
    const ANIMATED: Option<u32> = Some(1);

    fn slots(moves: bool) -> Vec<AnimationSlot> {
        use crate::visual::curves::{CompositeCurve, TransformTrack};
        use crate::visual::reach::Reach;
        let transform = moves.then(|| TransformTrack::with_reach(Reach::still()));
        vec![AnimationSlot {
            node: crate::tree::document::NodeId::from_bits(1).expect("a handle's bits"),
            curve: CompositeCurve::inert(transform),
        }]
    }

    #[test]
    fn nested_filter_brackets_pair_and_bound_their_own_ops() {
        let mut assembly = assembly();
        let outer = assembly.push_filter(group(None));
        assembly.push_op(push(None));
        let inner = assembly.push_filter(group(None));
        assembly.push_op(push(None));
        assembly.pop_filter(&SPACES, &slots(true), &[]);
        assembly.pop_filter(&SPACES, &slots(true), &[]);
        let finished = assembly.finish();

        assert_eq!((outer, inner), (0, 1), "groups are numbered in push order");
        assert_eq!(
            finished.program.len(),
            6,
            "PushFilter, Push, PushFilter, Push, PopFilter, PopFilter",
        );
        assert!(matches!(
            finished.program[0],
            ComposeOp::PushFilter { index: 0 }
        ));
        assert!(matches!(
            finished.program[2],
            ComposeOp::PushFilter { index: 1 }
        ));
        assert!(matches!(finished.program[4], ComposeOp::PopFilter));
        assert!(matches!(finished.program[5], ComposeOp::PopFilter));
        assert_eq!(finished.filter_groups[0].ops, 1..5, "the outer range");
        assert_eq!(finished.filter_groups[1].ops, 3..4, "the inner range");
        assert_eq!(finished.filter_groups[1].parent, Some(0));
        assert_eq!(finished.filter_groups[0].parent, None);
        assert!(
            finished.filter_groups[1].ops.end < finished.filter_groups[0].ops.end,
            "bake order is increasing range end, so the inner group bakes first",
        );
    }

    #[test]
    fn inner_chains_reports_only_a_differing_scroll_chain() {
        let mut same = assembly();
        same.push_filter(group(SCROLLED));
        same.push_op(push(SCROLLED));
        same.pop_filter(&SPACES, &slots(true), &[]);
        assert!(!same.finish().filter_groups[0].inner_chains);

        let mut differing = assembly();
        differing.push_filter(group(None));
        differing.push_op(push(SCROLLED));
        differing.pop_filter(&SPACES, &slots(true), &[]);
        assert!(differing.finish().filter_groups[0].inner_chains);

        let mut reverse = assembly();
        reverse.push_filter(group(SCROLLED));
        reverse.push_op(push(None));
        reverse.pop_filter(&SPACES, &slots(true), &[]);
        assert!(reverse.finish().filter_groups[0].inner_chains);

        let mut animated = assembly();
        animated.push_filter(group(None));
        animated.push_op(push(ANIMATED));
        animated.pop_filter(&SPACES, &slots(true), &[]);
        assert!(!animated.finish().filter_groups[0].inner_chains);
    }

    const EVERYWHERE: Rect = Rect::new(
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::INFINITY,
    );

    fn fragment() -> DisplayList {
        let mut scene = DisplayList::default();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            crate::peniko::Color::from_rgb8(0, 128, 128),
            None,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        scene
    }

    fn replay_program(
        program: &[ComposeOp],
        groups: &[FilterGroup],
        filtered: &[Option<Texture>],
    ) -> DisplayList {
        let fragments = [fragment()];
        let mut scene = DisplayList::default();
        replay(
            &mut scene,
            EVERYWHERE,
            &fragments,
            program,
            &[],
            &[],
            &[],
            &[],
            groups,
            filtered,
            &SpaceSamples {
                spaces: &SPACES,
                slots: &[],
                animations: &crate::visual::AnimationSamples::default(),
                stickies: &crate::visual::StickySamples::default(),
                anchored: &crate::visual::anchored::AnchoredSamples::new(),
                ratio: 1.0,
                offset_of: &|_| None,
            },
            &[],
        );
        scene
    }

    fn circular_slot() -> crate::visual::ScrollSlot {
        use crate::scroll::{CaptureAxes, ScrollAxes};
        crate::visual::ScrollSlot {
            node: crate::tree::document::NodeId::from_bits(1).expect("a handle's bits"),
            parent: None,
            user_scrollable: ScrollAxes { x: false, y: true },
            chains: ScrollAxes::NONE,
            bounce: ScrollAxes::NONE,
            circular: ScrollAxes { x: false, y: true },
            capture: CaptureAxes::default(),
            snap: crate::visual::SnapSlot::default(),
            offset: Vector2D::zero(),
            max_offset: Vector2D::new(0.0, 100.0),
            scrollport: euclid::default::Size2D::new(100.0, 100.0),
            viewport_axes: [Vector2D::new(1.0, 0.0), Vector2D::new(0.0, 1.0)],
            request: None,
            anchor_shift: Vector2D::zero(),
            reach: Vector2D::zero(),
        }
    }

    fn replay_circular(program: &[ComposeOp], offset: Vector2D<f32>) -> DisplayList {
        let fragments = [fragment()];
        let slots = [circular_slot()];
        let offset_of = move |_: &crate::visual::ScrollSlot| Some(offset);
        let mut scene = DisplayList::default();
        let samples = SpaceSamples {
            spaces: &SPACES,
            slots: &slots,
            animations: &crate::visual::AnimationSamples::default(),
            stickies: &crate::visual::StickySamples::default(),
            anchored: &crate::visual::anchored::AnchoredSamples::new(),
            ratio: 1.0,
            offset_of: &offset_of,
        };
        replay(
            &mut scene,
            EVERYWHERE,
            &fragments,
            program,
            &[],
            &[],
            &[],
            &[],
            &[],
            &[],
            &samples,
            &samples.seam_passes_with(&[], None),
        );
        scene
    }

    #[test]
    fn a_straddling_circular_slot_draws_its_content_again_one_period_back() {
        let program = [
            push(None),
            ComposeOp::Fragment {
                index: 0,
                space: SCROLLED,
            },
            ComposeOp::Pop,
        ];
        let expected = |copies: &[f64]| {
            let mut scene = DisplayList::default();
            scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
            for &y in copies {
                scene.append(&fragment(), Some(Affine::translate((0.0, y))));
            }
            scene.pop_layer();
            scene
        };

        crate::paint::equivalence::assert_scenes_identical(
            &replay_circular(&program, Vector2D::new(0.0, 150.0)),
            &expected(&[-150.0, 50.0]),
        );
        crate::paint::equivalence::assert_scenes_identical(
            &replay_circular(&program, Vector2D::new(0.0, 50.0)),
            &expected(&[-50.0]),
        );
    }

    #[test]
    fn a_seam_copy_is_drawn_before_what_paints_over_the_container() {
        let program = [
            push(None),
            ComposeOp::Fragment {
                index: 0,
                space: SCROLLED,
            },
            ComposeOp::Pop,
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
        ];
        let mut expected = DisplayList::default();
        expected.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
        expected.append(&fragment(), Some(Affine::translate((0.0, -150.0))));
        expected.append(&fragment(), Some(Affine::translate((0.0, 50.0))));
        expected.pop_layer();
        expected.append(&fragment(), Some(Affine::IDENTITY));
        crate::paint::equivalence::assert_scenes_identical(
            &replay_circular(&program, Vector2D::new(0.0, 150.0)),
            &expected,
        );
    }

    #[test]
    fn a_seam_copy_pushes_its_own_riding_layers_again() {
        let area = Rect::new(0.0, -400.0, 100.0, 400.0);
        let push = |space| clip_in(space, area);
        let program = [
            push(None),
            push(SCROLLED),
            ComposeOp::Fragment {
                index: 0,
                space: SCROLLED,
            },
            ComposeOp::Pop,
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
        ];
        let mut expected = DisplayList::default();
        expected.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &area);
        for y in [-150.0, 50.0] {
            expected.push_clip_layer(Fill::NonZero, Affine::translate((0.0, y)), &area);
            expected.append(&fragment(), Some(Affine::translate((0.0, y))));
            expected.pop_layer();
        }
        expected.append(&fragment(), Some(Affine::IDENTITY));
        expected.pop_layer();
        crate::paint::equivalence::assert_scenes_identical(
            &replay_circular(&program, Vector2D::new(0.0, 150.0)),
            &expected,
        );
    }

    #[test]
    fn a_filter_bracket_encodes_nothing_without_a_baked_texture() {
        let bare = [
            push(None),
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
        ];
        let bracketed = [
            push(None),
            ComposeOp::PushFilter { index: 0 },
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::PopFilter,
            ComposeOp::Pop,
        ];
        let mut group = group(None);
        group.ops = 2..3;
        let groups = [group];

        let without = replay_program(&bare, &[], &[]);
        for filtered in [&[][..], &[None][..]] {
            let with = replay_program(&bracketed, &groups, filtered);
            crate::paint::equivalence::assert_scenes_identical(&with, &without);
        }
    }

    #[test]
    fn a_baked_group_is_drawn_once_and_its_ops_skipped() {
        let program = [
            push(None),
            ComposeOp::PushFilter { index: 0 },
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::PopFilter,
            ComposeOp::Pop,
        ];
        let mut group = group(None);
        group.ops = 2..3;
        let groups = [group];
        let baked = texture(8, 8);

        let raw = replay_program(&program, &groups, &[None]);
        let composed = replay_program(&program, &groups, &[Some(baked)]);
        assert_eq!(composed.image_draws(), 1, "the group's texture, once");
        assert_eq!(raw.image_draws(), 0, "and none in the fallback");
        assert_eq!(
            composed.open_layers(),
            0,
            "skipping the range leaves the layer stack balanced",
        );
        assert_eq!(
            composed.len(),
            raw.len(),
            "one image fill replaces one content fill",
        );
    }

    fn backdrop_entry(space: Option<u32>) -> FilterGroup {
        backdrop_entry_in(space, BackdropRoot::Document)
    }

    fn backdrop_entry_in(space: Option<u32>, root: BackdropRoot) -> FilterGroup {
        FilterGroup::with_backdrop(
            2.0,
            Rect::new(0.0, 0.0, 8.0, 8.0),
            space,
            Backdrop {
                shape: BoxShape::Rect(Rect::new(0.0, 0.0, 8.0, 8.0)),
                transform: Affine::IDENTITY,
                before: Vec::new(),
                after: Vec::new(),
                root,
                open_pushes: 0,
            },
        )
    }

    #[test]
    fn a_backdrop_records_the_program_behind_it() {
        let mut assembly = assembly();
        assert_eq!(
            assembly.content_boundary(),
            BackdropRoot::Document.content_start(),
            "nothing precedes the frame root",
        );
        assembly.push_op(push(None));
        assembly.push_op(push(SCROLLED));
        assembly.push_op(ComposeOp::Pop);
        let end = assembly.content_boundary();
        assert!(
            assembly.push_backdrop(backdrop_entry(None), end, &SPACES, &slots(true), &[]),
            "a non-empty range records an entry",
        );
        let finished = assembly.finish();

        let entry = &finished.filter_groups[0];
        assert_eq!(entry.ops, 0..3, "the whole prefix");
        assert!(entry.is_backdrop());
        assert!(
            entry.reads_canvas(),
            "the document's root is over the canvas"
        );
        assert!(
            entry.inner_chains,
            "the prefix holds an op on another scroll chain",
        );
        assert!(
            !entry.samples_animations(),
            "and none on another animation node",
        );
        let backdrop = entry.backdrop.as_ref().expect("a backdrop entry");
        assert_eq!(
            backdrop.open_pushes, 1,
            "two pushes and one pop leave one layer open",
        );
        assert!(matches!(
            finished.program[3],
            ComposeOp::PushBackdrop { index: 0 }
        ));
        assert_eq!(
            finished.program.len(),
            4,
            "and the op opens nothing it has to close",
        );
    }

    #[test]
    fn inner_animations_reports_only_a_curve_between_op_and_entry() {
        for moves in [true, false] {
            let slots = slots(moves);
            for (space, op, expected) in [
                (None, ANIMATED, true),
                (ANIMATED, ANIMATED, false),
                (None, SCROLLED, false),
                (ANIMATED, None, moves),
            ] {
                let mut backdrop = assembly();
                backdrop.push_op(push(op));
                let end = backdrop.content_boundary();
                backdrop.push_backdrop(backdrop_entry(space), end, &SPACES, &slots, &[]);
                assert_eq!(
                    backdrop.finish().filter_groups[0].samples_animations(),
                    expected,
                    "backdrop in {space:?} over an op in {op:?}, moving: {moves}",
                );

                let mut blur = assembly();
                blur.push_filter(FilterGroup::new(2.0, Rect::new(0.0, 0.0, 8.0, 8.0), space));
                blur.push_op(push(op));
                blur.push_op(ComposeOp::Pop);
                blur.pop_filter(&SPACES, &slots, &[]);
                assert_eq!(
                    blur.finish().filter_groups[0].samples_animations(),
                    expected,
                    "blur group in {space:?} over an op in {op:?}, moving: {moves}",
                );
            }
        }
    }

    #[test]
    fn an_empty_range_in_a_group_records_no_backdrop_at_all() {
        let mut assembly = assembly();
        assembly.push_op(push(None));
        let content_start = assembly.content_boundary();
        assert!(!assembly.push_backdrop(
            backdrop_entry_in(None, BackdropRoot::Group { content_start }),
            content_start,
            &SPACES,
            &slots(true),
            &[]
        ));
        let finished = assembly.finish();
        assert!(finished.filter_groups.is_empty());
        assert_eq!(finished.program.len(), 1, "the group's own push and no op");
    }

    #[test]
    fn an_empty_range_at_the_document_root_still_records_a_backdrop() {
        let mut assembly = assembly();
        let end = assembly.content_boundary();
        assert!(assembly.push_backdrop(backdrop_entry(None), end, &SPACES, &slots(true), &[]));
        let finished = assembly.finish();
        let [entry] = &finished.filter_groups[..] else {
            panic!("one backdrop entry");
        };
        assert_eq!(entry.ops, 0..0);
        assert!(entry.reads_canvas());
        assert!(matches!(
            finished.program[..],
            [ComposeOp::PushBackdrop { index: 0 }]
        ));
    }

    #[test]
    fn a_backdrop_op_encodes_nothing_without_a_baked_texture() {
        let bare = [ComposeOp::Fragment {
            index: 0,
            space: None,
        }];
        let with_op = [
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::PushBackdrop { index: 0 },
        ];
        let mut entry = backdrop_entry(None);
        entry.ops = 0..1;
        let entries = [entry];

        let without = replay_program(&bare, &[], &[]);
        for filtered in [&[][..], &[None][..]] {
            let with = replay_program(&with_op, &entries, filtered);
            crate::paint::equivalence::assert_scenes_identical(&with, &without);
        }
    }

    #[test]
    fn a_baked_backdrop_draws_one_fill_and_its_after_passes() {
        let program = [
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::PushBackdrop { index: 0 },
        ];
        let baked = texture(8, 8);

        let plain = {
            let mut entry = backdrop_entry(None);
            entry.ops = 0..1;
            let entries = [entry];
            replay_program(&program, &entries, &[Some(baked)])
        };
        assert_eq!(plain.image_draws(), 1, "the entry's texture, once");
        assert_eq!(
            plain.open_layers(),
            0,
            "and the op leaves the layer stack balanced",
        );

        let with_passes = {
            let mut entry = backdrop_entry(None);
            entry.ops = 0..1;
            entry.backdrop.as_mut().expect("a backdrop").after = crate::paint::filters::passes(
                &[stylo::values::computed::effects::Filter::Brightness(
                    stylo::values::generics::NonNegative(0.5),
                )],
                0..1,
            );
            let entries = [entry];
            replay_program(&program, &entries, &[Some(baked)])
        };
        assert_eq!(
            with_passes.len(),
            plain.len() + 3 + 2,
            "one post-blur pass is a blend layer, its flat fill, and the pop, \
             inside the clip and its pop",
        );
        assert_eq!(
            with_passes.open_layers(),
            0,
            "and the passes leave the layer stack balanced",
        );
    }

    #[test]
    fn a_nested_group_reports_its_own_chain_to_the_group_around_it() {
        let mut assembly = assembly();
        assembly.push_filter(group(None));
        assembly.push_filter(group(SCROLLED));
        assembly.pop_filter(&SPACES, &slots(true), &[]);
        assembly.pop_filter(&SPACES, &slots(true), &[]);
        let finished = assembly.finish();
        assert!(
            finished.filter_groups[0].inner_chains,
            "the outer group's range holds a bracket on another chain",
        );
    }

    #[test]
    fn a_blur_group_takes_on_what_the_backdrop_inside_it_reads() {
        for behind in [SCROLLED, ANIMATED] {
            let mut assembly = assembly();
            assembly.push_op(push(behind));
            assembly.push_op(ComposeOp::Pop);
            let end = assembly.content_boundary();
            assembly.push_filter(group(None));
            assert!(assembly.push_backdrop(backdrop_entry(None), end, &SPACES, &slots(true), &[]));
            assembly.push_op(push(None));
            assembly.push_op(ComposeOp::Pop);
            assembly.pop_filter(&SPACES, &slots(true), &[]);
            let finished = assembly.finish();
            let [blur, backdrop] = &finished.filter_groups[..] else {
                panic!("a blur group and a backdrop");
            };
            assert!(backdrop.is_backdrop() && !blur.is_backdrop());
            assert!(
                blur.ops.start >= backdrop.ops.end,
                "the backdrop's range lies outside the group's",
            );
            let reads = |entry: &FilterGroup| (entry.inner_chains, entry.inner_animations);
            let expected = (behind == SCROLLED, behind == ANIMATED);
            assert_eq!(reads(backdrop), expected, "backdrop over {behind:?}");
            assert_eq!(reads(blur), expected, "blur group around it, {behind:?}");
        }
    }

    #[test]
    fn a_child_backdrop_takes_on_what_its_roots_backdrop_reads() {
        let mut assembly = assembly();
        assembly.push_op(push(ANIMATED));
        assembly.push_op(ComposeOp::Pop);
        let end = assembly.content_boundary();
        let content_start = assembly.content_boundary();
        assert!(assembly.push_backdrop(backdrop_entry(None), end, &SPACES, &slots(true), &[]));
        assembly.push_op(push(None));
        assembly.push_op(ComposeOp::Pop);
        let child_end = assembly.content_boundary();
        assert!(assembly.push_backdrop(
            backdrop_entry_in(None, BackdropRoot::Group { content_start }),
            child_end,
            &SPACES,
            &slots(true),
            &[]
        ));
        let finished = assembly.finish();
        let [root, child] = &finished.filter_groups[..] else {
            panic!("two backdrops");
        };
        assert!(
            root.reads_canvas() && !child.reads_canvas(),
            "only the document's root is over the canvas",
        );
        assert!(root.samples_animations(), "the root's prefix animates");
        assert!(
            child.samples_animations(),
            "and the child's range draws the root's texture",
        );
        assert!(!child.inner_chains);
    }

    fn vector_draw(transform: Affine, area: ImageArea) -> VectorDraw {
        VectorDraw {
            scene: Arc::new(fragment()),
            key: 7,
            viewport: (8.0, 8.0),
            aspect: AspectRatio::default(),
            transform,
            anchor: Point::ZERO,
            extent: Size::new(8.0, 8.0),
            sampler: ImageSampler::default(),
            area,
        }
    }

    fn whole() -> ImageArea {
        ImageArea::Fill(CapturedShape::Rect(Rect::new(0.0, 0.0, 8.0, 8.0)))
    }

    fn texture(width: u32, height: u32) -> Texture {
        Texture::new(width, height, width, height)
    }

    fn replay_vectors(
        program: &[ComposeOp],
        draws: &[VectorDraw],
        vectors: &[Option<Texture>],
    ) -> DisplayList {
        let fragments = [fragment()];
        let mut scene = DisplayList::default();
        replay(
            &mut scene,
            EVERYWHERE,
            &fragments,
            program,
            &[],
            &[],
            draws,
            vectors,
            &[],
            &[],
            &SpaceSamples {
                spaces: &SPACES,
                slots: &[],
                animations: &crate::visual::AnimationSamples::default(),
                stickies: &crate::visual::StickySamples::default(),
                anchored: &crate::visual::anchored::AnchoredSamples::new(),
                ratio: 1.0,
                offset_of: &|_| None,
            },
            &[],
        );
        scene
    }

    #[test]
    fn a_vector_op_encodes_nothing_without_a_texture() {
        let bare = [ComposeOp::Fragment {
            index: 0,
            space: None,
        }];
        let with_op = [
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Vector {
                index: 0,
                space: None,
            },
        ];
        let draws = [vector_draw(Affine::IDENTITY, whole())];
        let without = replay_vectors(&bare, &[], &[]);
        for vectors in [&[][..], &[None][..]] {
            let with = replay_vectors(&with_op, &draws, vectors);
            crate::paint::equivalence::assert_scenes_identical(&with, &without);
        }
    }

    #[test]
    fn a_vector_op_with_a_texture_draws_it_as_one_image_fill() {
        let program = [ComposeOp::Vector {
            index: 0,
            space: None,
        }];
        let baked = texture(8, 8);
        let plain = replay_vectors(
            &program,
            &[vector_draw(Affine::IDENTITY, whole())],
            &[Some(baked)],
        );
        let mut expected = DisplayList::default();
        encode_textured(
            &mut expected,
            Affine::IDENTITY,
            Point::ZERO,
            Size::new(8.0, 8.0),
            ImageSampler::default(),
            &whole(),
            Picture::Texture(baked),
        );
        crate::paint::equivalence::assert_scenes_identical(&plain, &expected);
        assert_eq!(plain.image_draws(), 1, "the texture");

        let clipped = replay_vectors(
            &program,
            &[vector_draw(
                Affine::IDENTITY,
                ImageArea::Clipped {
                    clip: BoxShape::Rect(Rect::new(0.0, 0.0, 8.0, 8.0)),
                    draw: Rect::new(0.0, 0.0, 4.0, 4.0),
                },
            )],
            &[Some(texture(8, 8))],
        );
        assert_eq!(
            clipped.len(),
            3,
            "the clip's begin, the fill, and the clip's end",
        );
        assert_eq!(clipped.open_layers(), 0);
    }

    #[test]
    #[allow(clippy::float_cmp, reason = "the expectations are exact")]
    fn a_vector_textures_brush_scale_is_the_extent_over_its_size() {
        let program = [ComposeOp::Vector {
            index: 0,
            space: None,
        }];
        let scene = replay_vectors(
            &program,
            &[vector_draw(Affine::IDENTITY, whole())],
            &[Some(texture(4, 4))],
        );
        let Some(crate::paint::display_list::Item::Fill(fill)) = scene.items().last() else {
            panic!("the texture is the list's last fill");
        };
        let brush = scene.transform(fill.transform)
            * scene
                .paint_transform(fill.paint_transform)
                .unwrap_or_default();
        assert_eq!(brush.as_coeffs(), [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    }

    #[test]
    #[allow(clippy::float_cmp, reason = "the expectations are exact")]
    fn the_aspect_map_stretches_meets_or_slices() {
        let viewport = (48.0, 24.0);
        let square = Size::new(96.0, 96.0);
        let none = AspectRatio {
            align: None,
            slice: false,
        };
        assert_eq!(
            aspect_transform(viewport, none, square).as_coeffs(),
            [2.0, 0.0, 0.0, 4.0, 0.0, 0.0],
        );
        assert_eq!(
            aspect_transform(viewport, AspectRatio::default(), square).as_coeffs(),
            [2.0, 0.0, 0.0, 2.0, 0.0, 24.0],
            "xMidYMid meet: the smaller ratio, centred on the slack axis",
        );
        let slice = AspectRatio {
            align: Some((AspectAlign::Mid, AspectAlign::Mid)),
            slice: true,
        };
        assert_eq!(
            aspect_transform(viewport, slice, square).as_coeffs(),
            [4.0, 0.0, 0.0, 4.0, -48.0, 0.0],
            "xMidYMid slice: the larger ratio, the overflow centred",
        );
        let corner = AspectRatio {
            align: Some((AspectAlign::Min, AspectAlign::Max)),
            slice: false,
        };
        assert_eq!(
            aspect_transform(viewport, corner, square).as_coeffs(),
            [2.0, 0.0, 0.0, 2.0, 0.0, 48.0],
            "xMinYMax meet",
        );
    }

    #[test]
    fn a_clamped_texture_holds_the_picture_placed_in_the_real_extent() {
        let draw = VectorDraw {
            viewport: (200.0, 1.0),
            extent: Size::new(20_000.0, 100.0),
            ..vector_draw(Affine::IDENTITY, whole())
        };
        let (width, height) = draw.device_size();
        assert_eq!((width, height), (MAX_RENDERABLE_DIMENSION, 100));
        let placed = draw
            .placement(
                draw.device_extent(),
                Size::new(f64::from(width), f64::from(height)),
            )
            .transform_rect_bbox(Rect::new(0.0, 0.0, 200.0, 1.0));
        let texture = Rect::new(0.0, 0.0, f64::from(width), f64::from(height));
        for (actual, expected) in [
            (placed.x0, texture.x0),
            (placed.y0, texture.y0),
            (placed.x1, texture.x1),
            (placed.y1, texture.y1),
        ] {
            assert!(
                (actual - expected).abs() < 1e-9,
                "the viewport fills the texture: {placed:?}",
            );
        }
    }

    #[test]
    fn a_draws_device_size_is_its_scaled_extent_rounded_up_and_bounded() {
        let at = |transform| VectorDraw {
            extent: Size::new(10.5, 4.0),
            ..vector_draw(transform, whole())
        };
        assert_eq!(at(Affine::scale(3.0)).device_size(), (32, 12));
        assert_eq!(
            at(Affine::scale(10_000.0)).device_size(),
            (MAX_RENDERABLE_DIMENSION, MAX_RENDERABLE_DIMENSION),
        );
        assert_eq!(at(Affine::scale(0.0)).device_size(), (0, 0));
    }

    #[test]
    fn the_inline_vector_encoding_places_the_scene_inside_its_own_layers() {
        let draw = VectorDraw {
            anchor: Point::new(2.0, 2.0),
            extent: Size::new(16.0, 8.0),
            ..vector_draw(Affine::IDENTITY, whole())
        };
        let mut actual = DisplayList::default();
        encode_vector_inline(&mut actual, &draw, Affine::IDENTITY);
        let mut expected = DisplayList::default();
        expected.push_clip_layer(
            Fill::NonZero,
            Affine::IDENTITY,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        expected.append(&fragment(), Some(Affine::translate((6.0, 2.0))));
        expected.pop_layer();
        crate::paint::equivalence::assert_scenes_identical(&actual, &expected);
        assert_eq!(actual.open_layers(), 0);
    }

    fn square_at(x: f64, y: f64) -> DisplayList {
        let mut scene = DisplayList::default();
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            crate::peniko::Color::from_rgb8(0, 128, 128),
            None,
            &Rect::new(x, y, x + 8.0, y + 8.0),
        );
        scene.seal_bounds();
        scene
    }

    fn clip(rect: Rect) -> ComposeOp {
        clip_in(None, rect)
    }

    fn clip_in(space: Option<u32>, rect: Rect) -> ComposeOp {
        ComposeOp::Push {
            clip_only: true,
            fill: Fill::NonZero,
            blend: BlendMode::new(Mix::Normal, Compose::SrcOver),
            alpha: 1.0,
            transform: Affine::IDENTITY,
            shape: CapturedShape::Rect(rect),
            space,
            alpha_animation: None,
        }
    }

    const TARGET: Rect = Rect::new(0.0, 0.0, 100.0, 100.0);

    fn replay_culled(
        fragments: &[DisplayList],
        program: &[ComposeOp],
        groups: &[FilterGroup],
        filtered: &[Option<Texture>],
        offset: Vector2D<f32>,
    ) -> DisplayList {
        use crate::scroll::ScrollAxes;
        let slots = [crate::visual::ScrollSlot {
            circular: ScrollAxes::NONE,
            ..circular_slot()
        }];
        let offset_of = move |_: &crate::visual::ScrollSlot| Some(offset);
        let samples = SpaceSamples {
            spaces: &SPACES,
            slots: &slots,
            animations: &crate::visual::AnimationSamples::default(),
            stickies: &crate::visual::StickySamples::default(),
            anchored: &crate::visual::anchored::AnchoredSamples::new(),
            ratio: 1.0,
            offset_of: &offset_of,
        };
        let mut scene = DisplayList::default();
        replay(
            &mut scene,
            TARGET,
            fragments,
            program,
            &[],
            &[],
            &[],
            &[],
            groups,
            filtered,
            &samples,
            &samples.seam_passes_with(&[], None),
        );
        scene
    }

    #[test]
    fn a_fragment_off_the_target_draws_nothing_and_one_partly_on_it_draws() {
        let program = [ComposeOp::Fragment {
            index: 0,
            space: None,
        }];
        let off = replay_culled(
            &[square_at(200.0, 0.0)],
            &program,
            &[],
            &[],
            Vector2D::zero(),
        );
        assert!(off.is_empty(), "{off:?}");
        let partly = replay_culled(
            &[square_at(96.0, 96.0)],
            &program,
            &[],
            &[],
            Vector2D::zero(),
        );
        let mut expected = DisplayList::default();
        expected.append(&square_at(96.0, 96.0), Some(Affine::IDENTITY));
        assert_eq!(partly.compare(&expected), Ok(()));
    }

    #[test]
    fn a_fragment_is_culled_where_the_scroll_offset_puts_it() {
        let program = [ComposeOp::Fragment {
            index: 0,
            space: SCROLLED,
        }];
        let fragments = [square_at(0.0, 150.0)];
        let still = replay_culled(&fragments, &program, &[], &[], Vector2D::zero());
        assert!(still.is_empty(), "{still:?}");
        let scrolled = replay_culled(&fragments, &program, &[], &[], Vector2D::new(0.0, 100.0));
        assert_eq!(scrolled.len(), 1);
    }

    #[test]
    fn a_fragment_its_clip_excludes_is_skipped() {
        let program = [
            clip(Rect::new(0.0, 0.0, 50.0, 50.0)),
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
        ];
        let scene = replay_culled(
            &[square_at(60.0, 60.0)],
            &program,
            &[],
            &[],
            Vector2D::zero(),
        );
        assert_eq!(scene.len(), 2, "the clip's push and pop alone");
        assert_eq!(scene.layers(), 1);
        assert_eq!(scene.open_layers(), 0);
    }

    #[test]
    fn a_push_off_the_target_is_skipped_up_to_its_matching_pop() {
        let program = [
            clip(Rect::new(200.0, 0.0, 300.0, 100.0)),
            clip(Rect::new(0.0, 0.0, 300.0, 100.0)),
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
            ComposeOp::Pop,
            ComposeOp::Fragment {
                index: 1,
                space: None,
            },
        ];
        let fragments = [square_at(210.0, 0.0), square_at(0.0, 0.0)];
        let scene = replay_culled(&fragments, &program, &[], &[], Vector2D::zero());
        assert_eq!(scene.len(), 1, "the second fragment alone");
        assert_eq!(scene.layers(), 0);
    }

    #[test]
    fn a_skipped_chunk_holding_a_layer_keeps_the_stack_balanced() {
        let mut fragment = DisplayList::default();
        fragment.push_layer(
            Fill::NonZero,
            BlendMode::default(),
            0.5,
            Affine::IDENTITY,
            &Rect::new(0.0, 200.0, 50.0, 250.0),
        );
        for _ in 0..4 {
            fragment.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                crate::peniko::Color::BLACK,
                None,
                &Rect::new(0.0, 200.0, 50.0, 250.0),
            );
        }
        fragment.pop_layer();
        fragment.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            crate::peniko::Color::BLACK,
            None,
            &Rect::new(0.0, 0.0, 8.0, 8.0),
        );
        fragment.seal_bounds();
        let program = [
            clip(Rect::new(0.0, 0.0, 100.0, 300.0)),
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
        ];
        let scene = replay_culled(&[fragment], &program, &[], &[], Vector2D::zero());
        assert_eq!(scene.open_layers(), 0);
        assert_eq!(
            scene.layers(),
            1,
            "the clip alone; the fragment's layer was left out"
        );
        assert_eq!(scene.len(), 3, "push, the fill on the target, pop");
    }

    #[test]
    fn a_push_whose_pop_lies_past_the_range_is_encoded() {
        let program = [
            clip(Rect::new(200.0, 0.0, 300.0, 100.0)),
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::Pop,
        ];
        let fragments = [square_at(210.0, 0.0)];
        let mut scene = DisplayList::default();
        replay_ops(
            &mut scene,
            Tables {
                fragments: &fragments,
                program: &program,
                image_draws: &[],
                images: &[],
                vector_draws: &[],
                vectors: &[],
                filter_groups: &[],
                filtered: &[],
                spaces: &SPACES,
                samples: &crate::visual::AnimationSamples::default(),
            },
            0..2,
            &|_| Affine::IDENTITY,
            TARGET,
            None,
        );
        assert_eq!(scene.open_layers(), 1);
        assert_eq!(scene.len(), 1, "the push alone");
    }

    #[test]
    fn a_baked_groups_outset_reaching_the_target_draws_it() {
        let program = [
            ComposeOp::PushFilter { index: 0 },
            ComposeOp::Fragment {
                index: 0,
                space: None,
            },
            ComposeOp::PopFilter,
        ];
        let fragments = [square_at(106.0, 0.0)];
        let baked = |x: f64| {
            let mut group = FilterGroup::new(2.0, Rect::new(x, 0.0, x + 20.0, 20.0), None);
            group.ops = 1..2;
            replay_culled(
                &fragments,
                &program,
                &[group],
                &[Some(texture(20, 20))],
                Vector2D::zero(),
            )
        };
        assert_eq!(baked(100.0).image_draws(), 1);
        assert!(baked(140.0).is_empty());
    }
}
