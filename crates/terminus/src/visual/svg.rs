//! An SVG recording as items and group layers of a paint order.
//!
//! [`Document::plan_svg_image`] plans the recording an SVG image's isolated
//! document filed for its root ([`plan_recording`]), in the recording's
//! order, which is document order and SVG's paint order: no `z-index` and
//! no stacking context applies below the root.
//!
//! - Each path and text item becomes one [`PaintItem`] of kind [`PaintItemKind::Svg`], whose node
//!   is the element it was recorded from. Its world matrix is the item's absolute transform, then
//!   the origin of its ink rectangle ([`crate::svg::ink_extent`]), whose size is the item's:
//!   culling and group bounds read `matrix × size`, so they hold the item's strokes and text. It is
//!   not hit-testable.
//! - Each layer push and its matching pop become one [`RenderLayer`] of kind [`LayerKind::Svg`]
//!   around the contiguous run of items between them, nested in the layers open around it. Its
//!   matrix places its shape's bounding box as an item's places its ink rectangle.
//!
//! The plan is under the identity map and world, with no root box item
//! before the items and no clip, space, scroll slot or animation slot. The
//! image's root is not a CSS box, and nothing lays it out.

use euclid::default::{Size2D, Transform3D};
use smallvec::SmallVec;

use super::build::intern;
use super::transform::planar;
use super::{CornerRadii, LayerKind, PaintItem, PaintItemKind, PaintOrder, RenderLayer};
use crate::kurbo::{Affine, Rect};
use crate::paint::svg::layer;
use crate::svg::ink_extent;
use crate::svg::recording::{Item, Recording};
use crate::tree::document::{Document, NodeId};

/// The tables of the frame being built that [`plan_recording`] appends to,
/// and the group layer open where it is called.
pub(super) struct Tables<'a> {
    pub(super) items: &'a mut Vec<PaintItem>,
    pub(super) matrices: &'a mut Vec<Transform3D<f32>>,
    pub(super) layers: &'a mut Vec<RenderLayer>,
    pub(super) current_layer: Option<usize>,
}

impl<T: Sync> Document<T> {
    /// The plan of an SVG image: the recording filed for `root`, this
    /// isolated document's `svg` element (`crate::svg::image`), as paint
    /// items and group layers ([module docs](self)), in the recording's
    /// viewport units. No box is behind them: no root box item
    /// precedes them, and they take no clip, space, scroll slot or animation
    /// slot. Claims a commit id, which the paint walk checks the plan
    /// against; the document is built, styled and recorded, never laid out.
    pub(crate) fn plan_svg_image(&mut self, root: NodeId) -> PaintOrder {
        let mut order = PaintOrder::empty();
        order.commit_id = self.next_commit_id();
        if let Some(recording) = self.svg_recording(root) {
            let PaintOrder {
                items,
                matrices,
                layers,
                ..
            } = &mut order;
            plan_recording(
                recording,
                Placed {
                    root,
                    map: Affine::IDENTITY,
                    world: &Transform3D::identity(),
                    clip: None,
                    space: None,
                    slot: None,
                },
                Tables {
                    items,
                    matrices,
                    layers,
                    current_layer: None,
                },
            );
        }
        order
    }
}

/// Where [`plan_recording`] puts a recording's items and layers.
#[derive(Clone, Copy)]
struct Placed<'a> {
    /// The outermost `svg` element the recording is filed under.
    root: NodeId,
    /// The recording's viewport units to the space `world` maps from.
    map: Affine,
    /// The world matrix of that space.
    world: &'a Transform3D<f32>,
    /// The clip chain, compose space and scroll slot every item and layer
    /// takes.
    clip: Option<usize>,
    space: Option<u32>,
    slot: Option<u32>,
}

/// Appends `recording`'s path and text items and its layer pushes, placed
/// by `placed`, to `tables`. Leaves the open group layer as it found it.
fn plan_recording(recording: &Recording, placed: Placed<'_>, tables: Tables<'_>) {
    let Tables {
        items,
        matrices,
        layers,
        current_layer,
    } = tables;
    let Placed {
        root,
        map,
        world: root_world,
        clip,
        space,
        slot,
    } = placed;
    let mut current = current_layer;
    let mut open: SmallVec<[usize; 8]> = SmallVec::new();
    for (index, item) in recording.items.iter().enumerate() {
        let index = u32::try_from(index).expect("a recording holds fewer than 2^32 items");
        let kind = PaintItemKind::Svg { root, index };
        match item {
            Item::Path { .. } | Item::Text(_) => {
                let Some(extent) = ink_extent(item) else {
                    continue;
                };
                let local = map * item.transform() * Affine::translate(extent.origin().to_vec2());
                let transform = intern(matrices, world(root_world, local));
                items.push(PaintItem {
                    node: item.node().expect("a path or text item names its element"),
                    kind,
                    transform,
                    clip,
                    size: size(extent),
                    radii: CornerRadii::ZERO,
                    hit_testable: false,
                    slot,
                    space,
                });
            }
            Item::PushLayer { .. } | Item::PushClip { .. } => {
                let pushed = layer(item).expect("a layer push item is a layer");
                let bounds = pushed.shape.bounds();
                let local = map * pushed.transform * Affine::translate(bounds.origin().to_vec2());
                let transform = intern(matrices, world(root_world, local));
                let start = items.len();
                layers.push(RenderLayer {
                    parent: current,
                    node: item.node().expect("a layer push names its element"),
                    transform,
                    size: size(bounds),
                    radii: CornerRadii::ZERO,
                    space,
                    clip,
                    items: start..start,
                    kind: LayerKind::Svg { root, index },
                });
                let opened = layers.len() - 1;
                open.push(opened);
                current = Some(opened);
            }
            Item::Pop => {
                if let Some(opened) = open.pop() {
                    current = close(items, layers, opened);
                }
            }
        }
    }
    // A recording's pushes and pops balance; a layer left open closes here
    // all the same, so the frame's groups nest.
    while let Some(opened) = open.pop() {
        current = close(items, layers, opened);
    }
    debug_assert_eq!(
        current, current_layer,
        "the plan closes every layer it opens"
    );
}

/// Ends layer `opened` at the current item, or drops it when it encloses
/// no item, and answers the layer open around it.
fn close(items: &[PaintItem], layers: &mut Vec<RenderLayer>, opened: usize) -> Option<usize> {
    let parent = layers[opened].parent;
    let end = items.len();
    if end == layers[opened].items.start {
        debug_assert_eq!(opened, layers.len() - 1, "an empty layer is the last one");
        layers.pop();
    } else {
        layers[opened].items.end = end;
    }
    parent
}

/// `local`, a map into the root's border box, followed by the root's world
/// matrix: composed in f64 when the world is planar, which is every root
/// outside a 3D transform, so the item's matrix is rounded to f32 once.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the frame's world matrices are f32"
)]
fn world(root_world: &Transform3D<f32>, local: Affine) -> Transform3D<f32> {
    let to_3d = |affine: Affine| {
        let [m11, m12, m21, m22, m41, m42] = affine.as_coeffs().map(|value| value as f32);
        Transform3D::new_2d(m11, m12, m21, m22, m41, m42)
    };
    if root_world.is_2d() {
        to_3d(planar(root_world) * local)
    } else {
        to_3d(local).then(root_world)
    }
}

#[expect(clippy::cast_possible_truncation, reason = "CSS px fit an f32")]
fn size(rect: Rect) -> Size2D<f32> {
    Size2D::new(rect.width().max(0.0) as f32, rect.height().max(0.0) as f32)
}
