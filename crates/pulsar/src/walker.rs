//! The scene walker: flat paint-order items → vello layer stack.
//!
//! Three stack disciplines interleave on vello's single layer stack:
//!
//! 1. **Item clip chains** ([`w3c_dom::visual::ClipNode`]) — pushed lazily
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
//! 2. **Group scopes** ([`w3c_dom::visual::RenderLayer`]) — a stacking context with group effects
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

use vello::Scene;
use vello::kurbo::{Affine, Point, Rect};
use vello::peniko::{BlendMode, Compose, Fill, Mix};
use w3c_dom::Document;
use w3c_dom::visual::{ClipNode, PaintItem, PaintItemKind, PaintOrder, RenderLayer};

use crate::paint::{BoxFragment, PathScratch, background, border, filters, mask, shadow, text};
use crate::shape::{BoxShape, with_shape};
use crate::{ImageStore, convert};

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

pub(crate) fn walk<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    images: &ImageStore,
) {
    // Fail closed on any visual staleness: painting resolves the frame's
    // geometry snapshot against live styles/layouts/text, so both recycled
    // NodeIds (like hit testing) and post-build mutations of any kind
    // (which hit testing tolerates — its snapshot is self-contained) make
    // the mix incoherent.
    frame.assert_visually_fresh(document);
    scratch.clip_stack.clear();
    scratch.chain.clear();
    scratch.scopes.clear();
    compute_layer_bounds(scratch, document, frame);

    // Single-sourced from the document's device: the same scale that
    // rounded the layouts becomes the one CSS px -> device px transform.
    let scale = Affine::scale(f64::from(document.device().device_pixel_ratio().get()));
    let items = frame.items();
    let layers = frame.layers();
    let mut next_open = 0_usize;

    for (index, item) in items.iter().enumerate() {
        while scratch
            .scopes
            .last()
            .is_some_and(|scope| layers[scope.layer].items.end == index)
        {
            close_scope(scene, scratch, document, frame, scale);
        }
        while next_open < layers.len() && layers[next_open].items.start == index {
            open_scope(scene, scratch, document, frame, next_open, images, scale);
            next_open += 1;
        }
        paint_item(scene, scratch, document, frame, item, images, scale);
    }
    while !scratch.scopes.is_empty() {
        close_scope(scene, scratch, document, frame, scale);
    }
    pop_clips_to(scene, scratch, 0);
}

/// Opens one group scope: effect layer, optional clip-path layer, optional
/// mask sandwich. In-scope item clips pop first so no blend layer ever
/// nests inside a clip layer.
fn open_scope<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    layer_index: usize,
    images: &ImageStore,
    scale: Affine,
) {
    let layer = &frame.layers()[layer_index];
    let base = scratch.scopes.last().map_or(0, |scope| scope.base);
    pop_clips_to(scene, scratch, base);

    let style = document
        .paint_style(layer.node)
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

    let local = convert::item_affine(&layer.transform, layer.size);
    let fragment = document.rounded_layout(layer.node).map(|layout| {
        BoxFragment::new(
            layer.node,
            scale * local.unwrap_or_default(),
            layer.size,
            layer.radii,
            layout,
        )
    });

    if let Some(fragment) = fragment.as_ref()
        && let Some((clip_shape, fill)) =
            crate::shape::clip_path_shape(style, &fragment.reference_boxes())
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

fn close_scope<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    scale: Affine,
) {
    let scope = scratch
        .scopes
        .pop()
        .expect("close_scope is only called with an open scope");
    pop_clips_to(scene, scratch, scope.base);
    if scope.filtered {
        let layer = &frame.layers()[scope.layer];
        if let Some(style) = document.paint_style(layer.node) {
            filters::apply(scene, style, scratch.layer_bounds[scope.layer], scale);
        }
    }
    for _ in 0..scope.pushed {
        scene.pop_layer();
    }
}

fn paint_item<T>(
    scene: &mut Scene,
    scratch: &mut Scratch,
    document: &Document<T>,
    frame: &PaintOrder,
    item: &PaintItem,
    images: &ImageStore,
    scale: Affine,
) {
    let Some(local) = convert::item_affine(&item.transform, item.size) else {
        return;
    };
    sync_clips(scene, scratch, frame, item, scale);
    let transform = scale * local;

    match item.kind {
        PaintItemKind::ElementBox => {
            let Some(style) = document.paint_style(item.node) else {
                return;
            };
            let Some(layout) = document.rounded_layout(item.node) else {
                return;
            };
            let fragment = BoxFragment::new(item.node, transform, item.size, item.radii, layout);
            // `background-clip: text` clips its layers to the element's
            // descendant glyph silhouettes; collect them only when the
            // style asks (rare property, per-item Vec accepted).
            let text_clip =
                background::needs_text_clip(style).then(|| collect_text_clip(document, item.node));
            shadow::paint_outset(scene, &mut scratch.paths, style, &fragment);
            background::paint(scene, style, &fragment, images, text_clip.as_ref());
            // Inner shadows sit immediately above the background, below the
            // element's content (css-backgrounds-3 §7.4.1) — replaced pixels
            // cover them, the browser-observable order.
            shadow::paint_inset(scene, &mut scratch.paths, style, &fragment);
            background::paint_replaced_content(scene, &fragment, images);
            border::paint(scene, &mut scratch.paths, style, &fragment);
            border::paint_outline(scene, &mut scratch.paths, style, &fragment);
        }
        PaintItemKind::TextRun { element } => {
            let Some(style) = document.paint_style(element) else {
                return;
            };
            let Some(layout) = document.text_layout(item.node) else {
                return;
            };
            // Decorations propagate from ancestor decorating boxes
            // (css-text-decor-3 §2), not through inheritance — collect the
            // chain, each entry in its originating box's style/color.
            let decorations = text::propagated_decorations(document, element);
            text::paint(scene, style, layout, transform, &decorations);
        }
    }
}

/// Collects the committed text layouts under `element` for its
/// `background-clip: text` silhouette, with offsets accumulated from the
/// element's border-box origin. `Layout.location` is box-parent-relative
/// and the layout host zeroes boxless (`display: contents`) elements'
/// slots, so unconditional accumulation composes correctly; `display:
/// none` subtrees contribute nothing because their text never commits a
/// layout. Recorded v1 limits: descendant `transform`s are ignored (the
/// silhouette uses layout positions only) and `visibility: hidden` text is
/// skipped via its parent's style.
fn collect_text_clip<T>(
    document: &Document<T>,
    element: w3c_dom::NodeId,
) -> crate::paint::TextClip<'_> {
    let mut clip = crate::paint::TextClip::default();
    collect_text_clip_under(document, element, vello::kurbo::Vec2::ZERO, &mut clip);
    clip
}

fn collect_text_clip_under<'doc, T>(
    document: &'doc Document<T>,
    node: w3c_dom::NodeId,
    offset: vello::kurbo::Vec2,
    clip: &mut crate::paint::TextClip<'doc>,
) {
    use vello::kurbo::Vec2;
    let Some(node_ref) = document.get(node) else {
        return;
    };
    for child in node_ref.children() {
        if child.is_text_node() {
            let visible = document.paint_style(node).is_none_or(|style| {
                matches!(
                    style.clone_visibility(),
                    stylo::computed_values::visibility::T::Visible
                )
            });
            if !visible {
                continue;
            }
            if let (Some(layout), Some(text)) = (
                document.rounded_layout(child.id()),
                document.text_layout(child.id()),
            ) {
                clip.runs.push((
                    offset + Vec2::new(f64::from(layout.location.x), f64::from(layout.location.y)),
                    text,
                ));
            }
        } else if child.is_element() {
            let child_offset = document
                .rounded_layout(child.id())
                .map_or(offset, |layout| {
                    offset + Vec2::new(f64::from(layout.location.x), f64::from(layout.location.y))
                });
            collect_text_clip_under(document, child.id(), child_offset, clip);
        }
    }
}

/// Diffs the item's clip chain against the pushed stack within the current
/// scope: pop to the longest common prefix, push the rest.
fn sync_clips(
    scene: &mut Scene,
    scratch: &mut Scratch,
    frame: &PaintOrder,
    item: &PaintItem,
    scale: Affine,
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
        push_clip(scene, &frame.clips()[index], scale);
        scratch.clip_stack.push(index);
    }
}

fn push_clip(scene: &mut Scene, clip: &ClipNode, scale: Affine) {
    let size = w3c_dom::visual::Size2D::new(clip.rect.size.width, clip.rect.size.height);
    let Some(local) = convert::item_affine(&clip.transform, size) else {
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
fn compute_layer_bounds<T>(scratch: &mut Scratch, document: &Document<T>, frame: &PaintOrder) {
    let layers = frame.layers();
    let items = frame.items();
    scratch.layer_bounds.clear();
    scratch.open_layers.clear();
    if layers.is_empty() {
        return;
    }
    scratch.layer_bounds.resize(layers.len(), Rect::ZERO);
    let device_viewport = document.device().viewport_size();
    let viewport = Rect::new(
        0.0,
        0.0,
        f64::from(device_viewport.width),
        f64::from(device_viewport.height),
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
            scratch.bounds_acc[next_open] = layer_root_rect(&layers[next_open]);
            scratch.open_layers.push(next_open);
            next_open += 1;
        }
        let Some(&top) = scratch.open_layers.last() else {
            continue;
        };
        let extent = match item.kind {
            PaintItemKind::ElementBox => document.paint_style(item.node).map_or(0.0, |style| {
                shadow::extent(style).max(border::outline_extent(style))
            }),
            PaintItemKind::TextRun { element } => {
                // Glyph ink overhang (italic slant, negative side bearings,
                // swashes) scales with font size and is not covered by the
                // layout box; half an em is a generous conservative pad.
                document.paint_style(element).map_or(4.0, |style| {
                    0.5 * f64::from(style.get_font().clone_font_size().computed_size().px())
                        + text::extent(style)
                })
            }
        };
        if let Some(rect) = item_viewport_rect(item, extent) {
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
fn layer_root_rect(layer: &RenderLayer) -> Option<Rect> {
    let affine = convert::item_affine(&layer.transform, layer.size)?;
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
fn item_viewport_rect(item: &PaintItem, extent: f64) -> Option<Rect> {
    let affine = convert::item_affine(&item.transform, item.size)?;
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
