//! An outermost `svg` element's recording as items of the paint order.
//!
//! The element is a leaf box (`crate::svg::root`); its descendants have no
//! box and no item of their own in the stacking walk. After the element's
//! own box item, [`plan_root`] appends what its recording
//! (`crate::svg::present`) draws, in the recording's order, which is
//! document order and SVG's paint order: no `z-index` and no stacking
//! context applies below the root.
//!
//! - Each path and text item becomes one [`PaintItem`] of kind [`PaintItemKind::Svg`], whose node
//!   is the element it was recorded from. Its world matrix is the root's border-box world, then the
//!   map from the recording's viewport onto the border box ([`RootPlacement::map`]), then the
//!   item's absolute transform, then the origin of its ink rectangle ([`crate::svg::ink_extent`]),
//!   whose size is the item's: culling and group bounds read `matrix × size`, so they hold the
//!   item's strokes and text. It is not hit-testable, and takes the root's space and scroll slot.
//! - Each layer push and its matching pop become one [`RenderLayer`] of kind [`LayerKind::Svg`]
//!   around the contiguous run of items between them, nested in the layers open around it, the
//!   root's own group included. Its matrix places its shape's bounding box as an item's places its
//!   ink rectangle.
//! - Every item and layer is clipped to what the recording is clipped to as an image of the root's
//!   natural size: the content box's shape and the destination rectangle
//!   ([`RootPlacement::clips`]), one or two [`ClipNode`]s inside the root's own clip chain, in the
//!   root box's space.
//! - Each element the recording marks as animating `transform` or `opacity` ([`Animated`]) asks for
//!   an animation slot as a CSS box that is a stacking context does ([`allocate_animation_slot`]):
//!   its curve exported, its track folded by the matrix builder its recorded transform came from
//!   ([`context_matrix`]) onto the root's world, the placement map and its parent's absolute
//!   transform, and its ink extent standing for a box's size. With a slot, the items and layers of
//!   its subtree take an animation space inside the one around them, so spaces nest as the elements
//!   do, and its recorded compositing layer is the layer the curve's opacity drives
//!   (`paint::walker`, `open_svg_scope`). A refusal leaves its items where they were, and the
//!   element animates through main-thread ticks.
//!
//! An SVG image is planned by the same function ([`plan_recording`]), from
//! [`Document::plan_svg_image`]: the recording its isolated document filed
//! for its root, under the identity map and world, with no root box item
//! before the items and no clip, space, scroll slot or animation slot. The
//! image's root is not a CSS box, and nothing lays it out.

use euclid::default::{Point2D, Size2D, Transform3D};
use smallvec::SmallVec;

use super::build::{Mover, SlotTables, TrackInputs, allocate_animation_slot, intern};
use super::transform::planar;
use super::{
    AnimationSlot, ClipNode, CornerRadii, LayerKind, PaintItem, PaintItemKind, PaintOrder,
    RenderLayer, Space, SpaceKind,
};
use crate::kurbo::{Affine, Rect};
use crate::paint::svg::layer;
use crate::svg::present::RootPlacement;
use crate::svg::recording::{Animated, Item, Recording};
use crate::svg::{context_matrix, ink_extent};
use crate::tree::document::{Document, NodeId};

/// The tables of the frame being built that [`plan_root`] appends to, and
/// the group layer open where it is called.
pub(super) struct Tables<'a> {
    pub(super) items: &'a mut Vec<PaintItem>,
    pub(super) matrices: &'a mut Vec<Transform3D<f32>>,
    pub(super) clips: &'a mut Vec<ClipNode>,
    pub(super) layers: &'a mut Vec<RenderLayer>,
    pub(super) spaces: &'a mut Vec<Space>,
    pub(super) animations: &'a mut Vec<AnimationSlot>,
    pub(super) current_layer: Option<usize>,
}

/// Appends the recording of the outermost `svg` element whose box item is
/// the last item of `tables`, when it has one that draws (see the module
/// documentation): placed by the root's border-box world and placement map,
/// clipped to its placement's clips inside the root's own clip chain, in the
/// root's space and scroll slot.
pub(super) fn plan_root<T: Sync>(document: &Document<T>, tables: Tables<'_>) {
    let Some(root_item) = tables.items.last() else {
        return;
    };
    debug_assert_eq!(root_item.kind, PaintItemKind::ElementBox);
    let root = root_item.node;
    let Some(recording) = document.svg_recording(root) else {
        return;
    };
    let Some(RootPlacement { map, clips: area }) = document.svg_root_placement(root, recording)
    else {
        return;
    };
    let root_transform = root_item.transform;
    let world = root_transform.resolve(tables.matrices);
    let (space, slot) = (root_item.space, root_item.slot);
    let mut clip = root_item.clip;
    for (rect, radii) in area {
        tables.clips.push(ClipNode {
            parent: clip,
            #[cfg(test)]
            node: root,
            transform: root_transform,
            rect: euclid_rect(rect),
            radii,
            space,
        });
        clip = Some(tables.clips.len() - 1);
    }
    plan_recording(
        document,
        recording,
        Placed {
            root,
            map,
            world: &world,
            clip,
            space,
            slot,
        },
        tables,
    );
}

impl<T: Sync> Document<T> {
    /// The plan of an SVG image: the recording filed for `root`, this
    /// isolated document's `svg` element (`crate::svg::image`), as the
    /// items and group layers an inline root's recording becomes, in the
    /// recording's viewport units. No box is behind them: no root box item
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
                clips,
                layers,
                spaces,
                animations,
                ..
            } = &mut order;
            plan_recording(
                self,
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
                    clips,
                    layers,
                    spaces,
                    animations,
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
    /// takes, an animated element's subtree a space nested in `space`.
    clip: Option<usize>,
    space: Option<u32>,
    slot: Option<u32>,
}

/// Appends `recording`'s path and text items and its layer pushes, placed
/// by `placed`, to `tables`, giving each element the recording marks as
/// animated the slot `allocate_animation_slot` answers. Leaves the open
/// group layer as it found it.
#[expect(
    clippy::too_many_lines,
    reason = "one linear pass over the recording's items, layers and animated elements"
)]
fn plan_recording<T: Sync>(
    document: &Document<T>,
    recording: &Recording,
    placed: Placed<'_>,
    tables: Tables<'_>,
) {
    let Tables {
        items,
        matrices,
        clips,
        layers,
        spaces,
        animations,
        current_layer,
    } = tables;
    let Placed {
        root,
        map,
        world: root_world,
        clip,
        mut space,
        slot,
    } = placed;
    let mut current = current_layer;
    let mut open: SmallVec<[usize; 8]> = SmallVec::new();
    // The animation spaces open around the items: where each element's items
    // end, and the space around it.
    let mut moving: SmallVec<[(usize, Option<u32>); 4]> = SmallVec::new();
    let mut marked = recording.animated.iter().peekable();
    for (index, item) in recording.items.iter().enumerate() {
        while let Some(&(end, outside)) = moving.last()
            && end == index
        {
            moving.pop();
            space = outside;
        }
        while let Some(entry) = marked.next_if(|entry| entry.items.start == index) {
            if entry.items.is_empty() {
                continue;
            }
            let asked = animate(
                document,
                entry,
                recording,
                (root_world, map, clip),
                SlotTables {
                    spaces,
                    clips,
                    layers,
                    current_layer: current,
                    animations,
                },
            );
            if let Some(slot) = asked {
                moving.push((entry.items.end, space));
                spaces.push(Space {
                    parent: space,
                    kind: SpaceKind::Animation(slot),
                });
                space =
                    Some(u32::try_from(spaces.len() - 1).expect("a frame cannot hold 2^32 spaces"));
            }
        }
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

/// Asks for the animation slot of `entry`, an element of `recording` the
/// recorder marked, whose root has world `root_world`, placement map `map`
/// and clip chain `clip`.
fn animate<T: Sync>(
    document: &Document<T>,
    entry: &Animated,
    recording: &Recording,
    (root_world, map, clip): (&Transform3D<f32>, Affine, Option<usize>),
    tables: SlotTables<'_>,
) -> Option<u32> {
    let node = document.get(entry.node)?;
    let style = document.paint_style(entry.node)?;
    let committed = &style.get_box().transform;
    // The track's factors, from what the recording baked: the reference box
    // the element's transform was folded against, and its parent's
    // absolute transform under the root's world and placement.
    let folded = entry
        .reference
        .filter(|_| node.animates_transform())
        .map(|reference| {
            let context = context_matrix(style, reference);
            let parent_world = world(root_world, map * entry.parent);
            let world = context.with_list(committed).then(&parent_world);
            (context, parent_world, world)
        });
    let track = folded
        .as_ref()
        .map(|(context, parent_world, world)| TrackInputs {
            context,
            parent_world,
            world,
            committed,
        });
    let extent = if track.is_some() {
        subtree_extent(recording, entry, map)
    } else {
        Size2D::zero()
    };
    allocate_animation_slot(
        document,
        node,
        Mover {
            track,
            extent,
            clip,
        },
        tables,
    )
}

/// The size of the rectangle, in the root's border-box px, that the path
/// and text items of `entry`'s subtree can put ink in.
fn subtree_extent(recording: &Recording, entry: &Animated, map: Affine) -> Size2D<f32> {
    recording.items[entry.items.clone()]
        .iter()
        .filter_map(|item| {
            ink_extent(item).map(|extent| (map * item.transform()).transform_rect_bbox(extent))
        })
        .reduce(|a, b| a.union(b))
        .map_or_else(Size2D::zero, size)
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

#[expect(clippy::cast_possible_truncation, reason = "CSS px fit an f32")]
fn euclid_rect(rect: Rect) -> euclid::default::Rect<f32> {
    euclid::default::Rect::new(
        Point2D::new(rect.x0 as f32, rect.y0 as f32),
        Size2D::new(rect.width() as f32, rect.height() as f32),
    )
}
