#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    reason = "CSS/style geometry is f32 while Vello/Kurbo geometry is f64"
)]

//! The scene walker: flat paint-order items → vello layer stack.
//!
//! Three stack disciplines interleave on vello's single layer stack:
//!
//! 1. **Item clip chains** ([`crate::visual::ClipNode`]) — pushed lazily
//!    per item by diffing the item's chain against what is on the stack,
//!    so runs of items sharing clips pay nothing. Chains restart inside
//!    every group scope: an item's full chain is (re-)pushed inside its
//!    innermost group, which keeps escape semantics (a fixed descendant of
//!    an opacity group is grouped but not clipped by the group's ancestors)
//!    and keeps group blend layers from opening inside a clip layer
//!    (vello [#1198](https://github.com/linebender/vello/issues/1198) —
//!    re-pushing intersecting clips is idempotent, so correctness is
//!    unaffected). The precise #1198 invariant maintained crate-wide: a
//!    blend layer's *immediate* enclosing layer is always a real
//!    (isolating) layer, never a clip layer — clip layers share their
//!    parent's buffer, so a blend directly inside one reads pixels outside
//!    the clip. Fragment painters that need a blend under an item clip
//!    (inset shadows) interpose their own full `SrcOver` layer first.
//! 2. **Group scopes** ([`crate::visual::RenderLayer`]) — a stacking context with group effects
//!    pushes, outermost to innermost: the effect layer (blend mode + `opacity` alpha, clipped to
//!    the group's prepass-computed content bounds), a `clip-path` layer (a full `push_layer`, not a
//!    clip layer, per the #1198 rule above), and for `mask-image` the alpha-mask sandwich — mask
//!    pattern drawn first, then a `Compose::SrcIn` layer holding the content. Filter adjustments
//!    draw at scope close, inside the innermost layer, after in-scope clips pop. Composite order on
//!    pop is therefore filter → mask → clip-path → opacity/blend — clip and mask are both
//!    intersective alpha ops, so the swap versus the spec's filter → clip → mask is unobservable.
//! 3. **Fragments** — per element box: outset shadows, background, inset shadows, replaced content
//!    (above the inset shadows — css-backgrounds-3 §7.4.1 paints inner shadows immediately above
//!    the background, below content, which is why an inset shadow on an `<img>` is invisible in
//!    browsers), border, outline (outline painting with its element rather than Appendix E step 10
//!    is a recorded v1 limit); per text run: the retained Parley layout under the parent element's
//!    style.

use crate::paint::shape::{BoxShape, with_shape};
use crate::paint::{
    BoxFragment, PathScratch, background, border, convert, filters, mask, shadow, text,
};
use crate::vello::Scene;
use crate::vello::kurbo::{Affine, Point, Rect};
use crate::vello::peniko::{BlendMode, Compose, Fill, Mix};
use crate::visual::frame::{Frame, ItemContent};
use crate::visual::{ClipNode, PaintItem, RenderLayer};
use crate::{ImageStore, Vector2D};

/// Reused per-frame buffers.
#[derive(Debug, Default)]
pub(crate) struct Scratch {
    /// Clip indices currently pushed as vello clip layers, outermost first.
    clip_stack: Vec<usize>,
    /// Per-item chain buffer, outermost first.
    chain: Vec<usize>,
    /// Open group scopes, outermost first.
    scopes: Vec<Scope>,
    /// Per-layer content bounds in viewport CSS px (prepass).
    layer_bounds: Vec<Rect>,
    /// Prepass open-layer stack.
    open_layers: Vec<usize>,
    /// Prepass per-layer bounds accumulator (`None` = nothing yet).
    bounds_acc: Vec<Option<Rect>>,
    /// Reused path buffers for the border/shadow painters.
    paths: PathScratch,
}

#[derive(Debug)]
struct Scope {
    layer: usize,
    /// `clip_stack` length at open; in-scope item clips pop back to it.
    base: usize,
    /// vello layers pushed for this scope.
    pushed: u32,
    /// Draw filter adjustments at close.
    filtered: bool,
}

pub(crate) fn walk(
    scene: &mut Scene,
    scratch: &mut Scratch,
    frame: &Frame,
    images: &ImageStore,
    offsets: &[Vector2D<f32>],
) {
    scratch.clip_stack.clear();
    scratch.chain.clear();
    scratch.scopes.clear();
    compute_layer_bounds(scratch, frame, offsets);

    // Single-sourced from the document's device, carried on the frame: the
    // same scale that rounded these layouts becomes the one CSS px ->
    // device px transform.
    let scale = Affine::scale(f64::from(frame.device_pixel_ratio()));
    let items = frame.items();
    let layers = frame.layers();
    let mut next_open = 0_usize;

    for (index, item) in items.iter().enumerate() {
        while scratch
            .scopes
            .last()
            .is_some_and(|scope| layers[scope.layer].items.end == index)
        {
            close_scope(scene, scratch, frame, scale);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            open_scope(scene, scratch, frame, next_open, images, scale, offsets);
            next_open += 1;
        }
        paint_item(scene, scratch, frame, index, item, images, scale, offsets);
    }
    while !scratch.scopes.is_empty() {
        close_scope(scene, scratch, frame, scale);
    }
    pop_clips_to(scene, scratch, 0);
}

/// Opens one group scope: effect layer, optional clip-path layer, optional
/// mask sandwich. In-scope item clips pop first so no blend layer ever
/// nests inside a clip layer.
#[allow(
    clippy::too_many_arguments,
    reason = "one walker call, all of it per-frame state"
)]
fn open_scope(
    scene: &mut Scene,
    scratch: &mut Scratch,
    frame: &Frame,
    layer_index: usize,
    images: &ImageStore,
    scale: Affine,
    offsets: &[Vector2D<f32>],
) {
    let layer = &frame.layers()[layer_index];
    let paint = frame.layer_paint(layer_index);
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    pop_clips_to(scene, scratch, base);

    let style = paint
        .style
        .as_deref()
        .expect("a group-effect stacking context keeps its style for the frame");
    let bounds = scratch.layer_bounds[layer_index];
    let effects = style.get_effects();
    let blend = blend_mode(style);
    let mut pushed = 1_u32;
    scene.push_layer(
        Fill::NonZero,
        blend,
        effects.opacity.clamp(0.0, 1.0),
        scale,
        &bounds,
    );

    let local = placed(frame, offsets, &layer.transform, layer.size, layer.scroll);
    let fragment = paint.metrics.map(|metrics| {
        BoxFragment::new(
            layer.node,
            scale * local.unwrap_or_default(),
            layer.size,
            layer.radii,
            metrics,
        )
    });

    if let Some(fragment) = fragment.as_ref()
        && let Some((clip_shape, fill)) =
            crate::paint::shape::clip_path_shape(style, &fragment.reference_boxes())
    {
        // A full layer, not a clip layer: the mask sandwich below may push
        // blend layers inside (#1198).
        match local {
            Some(local) => with_shape!(&clip_shape, |s| scene.push_layer(
                fill,
                BlendMode::new(Mix::Normal, Compose::SrcOver),
                1.0,
                scale * local,
                s,
            )),
            // Singular group transform: suppress the whole group.
            None => scene.push_layer(
                Fill::NonZero,
                BlendMode::new(Mix::Normal, Compose::SrcOver),
                1.0,
                Affine::IDENTITY,
                &Rect::ZERO,
            ),
        }
        pushed += 1;
    }

    if mask::has_mask(style) {
        if let Some(fragment) = fragment.as_ref() {
            mask::paint(scene, style, fragment, images);
        }
        scene.push_layer(
            Fill::NonZero,
            BlendMode::new(Mix::Normal, Compose::SrcIn),
            1.0,
            scale,
            &bounds,
        );
        pushed += 1;
    }

    scratch.scopes.push(Scope {
        layer: layer_index,
        base,
        pushed,
        filtered: !effects.filter.0.is_empty(),
    });
}

fn close_scope(scene: &mut Scene, scratch: &mut Scratch, frame: &Frame, scale: Affine) {
    let scope = scratch
        .scopes
        .pop()
        .expect("close_scope is only called with an open scope");
    pop_clips_to(scene, scratch, scope.base);
    if scope.filtered
        && let Some(style) = frame.layer_paint(scope.layer).style.as_deref()
    {
        filters::apply(scene, style, scratch.layer_bounds[scope.layer], scale);
    }
    for _ in 0..scope.pushed {
        scene.pop_layer();
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one walker call, all of it per-frame state"
)]
fn paint_item(
    scene: &mut Scene,
    scratch: &mut Scratch,
    frame: &Frame,
    index: usize,
    item: &PaintItem,
    images: &ImageStore,
    scale: Affine,
    offsets: &[Vector2D<f32>],
) {
    let Some(local) = placed(frame, offsets, &item.transform, item.size, item.scroll) else {
        return;
    };
    let paint = frame.item_paint(index);
    let Some(style) = paint.style.as_deref() else {
        return;
    };
    sync_clips(scene, scratch, frame, item, scale, offsets);
    let transform = scale * local;

    match &paint.content {
        ItemContent::Absent => {}
        ItemContent::Element(element) => {
            let fragment =
                BoxFragment::new(item.node, transform, item.size, item.radii, element.metrics);
            shadow::paint_outset(scene, &mut scratch.paths, style, &fragment);
            // `background-clip: text` clips its layers to the element's
            // descendant glyph silhouettes, which the frame resolved.
            background::paint(
                scene,
                style,
                &fragment,
                images,
                element.text_clip.as_deref(),
            );
            // Inner shadows sit immediately above the background, below the
            // element's content (css-backgrounds-3 §7.4.1) — replaced pixels
            // cover them, the browser-observable order.
            shadow::paint_inset(scene, &mut scratch.paths, style, &fragment);
            background::paint_replaced_content(
                scene,
                style,
                &fragment,
                images,
                element.natural_size,
            );
            border::paint(scene, &mut scratch.paths, style, &fragment);
            border::paint_outline(scene, &mut scratch.paths, style, &fragment);
        }
        ItemContent::Text(run) => {
            // Decorations propagate from ancestor decorating boxes
            // (css-text-decor-3 §2), not through inheritance; the frame
            // carries the chain, each entry still in its own style/color.
            let decorations = text::decorations_of(&run.decorations);
            // A gradient-valued `color` anchors to the styled element's
            // padding box. The frame leaves it `None` both for solid text
            // (which never reads it) and for a parent with no box to anchor
            // to — in the latter case the run's own box stands in.
            let gradient_box = run.gradient_box.map_or_else(
                || {
                    Rect::new(
                        0.0,
                        0.0,
                        f64::from(item.size.width),
                        f64::from(item.size.height),
                    )
                },
                |area| {
                    Rect::new(
                        f64::from(area.origin.x),
                        f64::from(area.origin.y),
                        f64::from(area.origin.x + area.size.width),
                        f64::from(area.origin.y + area.size.height),
                    )
                },
            );
            text::paint(
                scene,
                style,
                &run.layout,
                transform,
                &decorations,
                Some(gradient_box),
            );
        }
    }
}

/// Diffs the item's clip chain against the pushed stack within the current
/// scope: pop to the longest common prefix, push the rest.
fn sync_clips(
    scene: &mut Scene,
    scratch: &mut Scratch,
    frame: &Frame,
    item: &PaintItem,
    scale: Affine,
    offsets: &[Vector2D<f32>],
) {
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    scratch.chain.clear();
    let mut next = item.clip;
    while let Some(index) = next {
        scratch.chain.push(index);
        next = frame.clips()[index].parent;
    }
    scratch.chain.reverse();

    let common = scratch.clip_stack[base..]
        .iter()
        .zip(scratch.chain.iter())
        .take_while(|(pushed, wanted)| pushed == wanted)
        .count();
    pop_clips_to(scene, scratch, base + common);
    for position in common..scratch.chain.len() {
        let index = scratch.chain[position];
        push_clip(scene, frame, &frame.clips()[index], scale, offsets);
        scratch.clip_stack.push(index);
    }
}

fn push_clip(
    scene: &mut Scene,
    frame: &Frame,
    clip: &ClipNode,
    scale: Affine,
    offsets: &[Vector2D<f32>],
) {
    let size = crate::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let Some(local) = placed(frame, offsets, &clip.transform, size, clip.scroll) else {
        // Singular clip space: nothing inside it can render.
        scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &Rect::ZERO);
        return;
    };
    let rect = Rect::new(
        clip.rect.origin.x as f64,
        clip.rect.origin.y as f64,
        (clip.rect.origin.x + clip.rect.size.width) as f64,
        (clip.rect.origin.y + clip.rect.size.height) as f64,
    );
    let shape = BoxShape::new(rect, &clip.radii);
    with_shape!(&shape, |s| scene.push_clip_layer(
        Fill::NonZero,
        scale * local,
        s
    ));
}

fn pop_clips_to(scene: &mut Scene, scratch: &mut Scratch, len: usize) {
    while scratch.clip_stack.len() > len {
        scratch.clip_stack.pop();
        scene.pop_layer();
    }
}

/// Per-layer conservative content bounds in viewport CSS px, in one sweep
/// over the items (mirroring
/// [`walk`]'s open/close logic, so each item is measured exactly once no
/// matter how deeply layers nest; a closing layer's bounds fold into its
/// parent's).
///
/// Bounds are the union of the enclosed items' border boxes inflated by
/// their paint extents (shadows, outline, a font-size-proportional
/// glyph-overshoot pad for text), seeded with the establishing element's
/// own box (mask/clip-path geometry paints there even when the root box has
/// no item), and clamped to the viewport at close. Corners map through the
/// same affine fit [`paint_item`] draws with, so what is painted is what is
/// bounded — over-approximation costs tiles, never pixels.
fn compute_layer_bounds(scratch: &mut Scratch, frame: &Frame, offsets: &[Vector2D<f32>]) {
    let layers = frame.layers();
    let items = frame.items();
    scratch.layer_bounds.clear();
    scratch.open_layers.clear();
    if layers.is_empty() {
        return;
    }
    scratch.layer_bounds.resize(layers.len(), Rect::ZERO);
    let frame_viewport = frame.viewport();
    let viewport = Rect::new(
        0.0,
        0.0,
        f64::from(frame_viewport.width),
        f64::from(frame_viewport.height),
    );
    scratch.bounds_acc.clear();
    scratch.bounds_acc.resize(layers.len(), None);
    let mut next_open = 0_usize;

    let close = |scratch: &mut Scratch| {
        let closed = scratch
            .open_layers
            .pop()
            .expect("close is only called with an open layer");
        scratch.layer_bounds[closed] =
            scratch.bounds_acc[closed].map_or(Rect::ZERO, |rect| rect.intersect(viewport));
        if let (Some(bounds), Some(&parent)) =
            (scratch.bounds_acc[closed], scratch.open_layers.last())
        {
            scratch.bounds_acc[parent] =
                Some(scratch.bounds_acc[parent].map_or(bounds, |united| united.union(bounds)));
        }
    };

    for (index, item) in items.iter().enumerate() {
        while scratch
            .open_layers
            .last()
            .is_some_and(|&top| layers[top].items.end == index)
        {
            close(scratch);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            scratch.bounds_acc[next_open] = layer_root_rect(frame, offsets, &layers[next_open]);
            scratch.open_layers.push(next_open);
            next_open += 1;
        }
        let Some(&top) = scratch.open_layers.last() else {
            continue;
        };
        let paint = frame.item_paint(index);
        let extent = match (&paint.content, paint.style.as_deref()) {
            (ItemContent::Element(_), Some(style)) => {
                shadow::extent(style).max(border::outline_extent(style))
            }
            // Glyph ink overhang (italic slant, negative side bearings,
            // swashes) scales with font size and is not covered by the
            // layout box; half an em is a generous conservative pad.
            (ItemContent::Text(_), Some(style)) => {
                0.5 * f64::from(style.get_font().clone_font_size().computed_size().px())
                    + text::extent(style)
            }
            (ItemContent::Text(_), None) => 4.0,
            (ItemContent::Absent, _) | (ItemContent::Element(_), None) => 0.0,
        };
        if let Some(rect) = item_viewport_rect(frame, offsets, item, extent) {
            scratch.bounds_acc[top] =
                Some(scratch.bounds_acc[top].map_or(rect, |united| united.union(rect)));
        }
    }
    while !scratch.open_layers.is_empty() {
        close(scratch);
    }
}

/// The viewport-space AABB of a layer root's border box, mapped through the
/// same affine fit the group's mask/clip-path geometry is painted with.
fn layer_root_rect(frame: &Frame, offsets: &[Vector2D<f32>], layer: &RenderLayer) -> Option<Rect> {
    let affine = placed(frame, offsets, &layer.transform, layer.size, layer.scroll)?;
    Some(affine_rect(
        affine,
        Rect::new(0.0, 0.0, layer.size.width as f64, layer.size.height as f64),
    ))
}

/// The viewport-space AABB of an item's border box inflated by `extent`
/// CSS px on every side, or `None` for non-rendering transforms — mapped
/// through the affine fit [`paint_item`] draws with (for projective
/// matrices the fit's fourth corner can lie outside the true projection's
/// AABB, so bounding the fit is what keeps painted content unclipped).
fn item_viewport_rect(
    frame: &Frame,
    offsets: &[Vector2D<f32>],
    item: &PaintItem,
    extent: f64,
) -> Option<Rect> {
    let affine = placed(frame, offsets, &item.transform, item.size, item.scroll)?;
    Some(affine_rect(
        affine,
        Rect::new(
            -extent,
            -extent,
            item.size.width as f64 + extent,
            item.size.height as f64 + extent,
        ),
    ))
}

/// The paint transform for one frame element, with the renderer's own scroll
/// position folded in.
///
/// The frame's matrices carry the offsets the document knew; a renderer that
/// has scrolled past them corrects by a viewport-space pre-translation, which
/// is exactly what a scroll chain's correction is (see `dom::visual::
/// ScrollNode`). With `offsets` empty this is `convert::item_affine`
/// unchanged, so a caller that does not scroll ahead pays one branch.
fn placed(
    frame: &Frame,
    offsets: &[Vector2D<f32>],
    transform: &euclid::default::Transform3D<f32>,
    size: crate::Size2D<f32>,
    scroll: Option<usize>,
) -> Option<Affine> {
    let local = convert::item_affine(transform, size)?;
    if offsets.is_empty() {
        return Some(local);
    }
    let correction = frame.scroll_correction(scroll, offsets);
    Some(Affine::translate((f64::from(correction.x), f64::from(correction.y))) * local)
}

/// The AABB of `rect`'s four corners under `affine`.
fn affine_rect(affine: Affine, rect: Rect) -> Rect {
    let corners = [
        affine * Point::new(rect.x0, rect.y0),
        affine * Point::new(rect.x1, rect.y0),
        affine * Point::new(rect.x0, rect.y1),
        affine * Point::new(rect.x1, rect.y1),
    ];
    let mut mapped = Rect::from_points(corners[0], corners[0]);
    for corner in &corners[1..] {
        mapped = mapped.union_pt(*corner);
    }
    mapped
}

/// stylo `mix-blend-mode` → peniko blend (storage-only in the fork today;
/// mapped so it goes live on a grammar rebase). `plus-lighter` is a compose
/// op, not a mix (compositing-2).
fn blend_mode(style: &stylo::properties::ComputedValues) -> BlendMode {
    use stylo::computed_values::mix_blend_mode::T as MixBlendMode;
    let mix = match style.get_effects().mix_blend_mode {
        MixBlendMode::Normal => Mix::Normal,
        MixBlendMode::Multiply => Mix::Multiply,
        MixBlendMode::Screen => Mix::Screen,
        MixBlendMode::Overlay => Mix::Overlay,
        MixBlendMode::Darken => Mix::Darken,
        MixBlendMode::Lighten => Mix::Lighten,
        MixBlendMode::ColorDodge => Mix::ColorDodge,
        MixBlendMode::ColorBurn => Mix::ColorBurn,
        MixBlendMode::HardLight => Mix::HardLight,
        MixBlendMode::SoftLight => Mix::SoftLight,
        MixBlendMode::Difference => Mix::Difference,
        MixBlendMode::Exclusion => Mix::Exclusion,
        MixBlendMode::Hue => Mix::Hue,
        MixBlendMode::Saturation => Mix::Saturation,
        MixBlendMode::Color => Mix::Color,
        MixBlendMode::Luminosity => Mix::Luminosity,
        MixBlendMode::PlusLighter => {
            return BlendMode::new(Mix::Normal, Compose::Plus);
        }
    };
    BlendMode::new(mix, Compose::SrcOver)
}
