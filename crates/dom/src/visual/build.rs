//! The paint-order builder: CSS2 Appendix E over the laid-out tree,
//! collapsed for an engine with no floats and no inline-level boxes other
//! than text leaves.
//!
//! Per stacking context `E` the emitted order is:
//! 1. `E`'s own item;
//! 2. members with negative stack level, by `(level, seq)`;
//! 3. `E`'s in-flow stream — non-positioned, non-context descendants in order-modified document
//!    order, each element followed by its content;
//! 4. members with stack level ≥ 0, by `(level, seq)`.
//!
//! A *member* is either a real stacking context (painted atomically via
//! recursion) or a pseudo-stacking context — a positioned box with
//! `z-index: auto` and no other trigger, which paints its own item and
//! in-flow content here while its positioned/context descendants surface as
//! separate members of `E` (they interleave; CSS2 §E.2 step 8). `seq` is one
//! monotone counter over the collection walk, which iterates the **flattened
//! box-tree children** (`LayoutTree::flattened_children` — the same
//! `display: contents` dissolution layout used) by `(Layout::order, flattened
//! index)` at every level, so tree-order tiebreaks are order-modified
//! document order over exactly the sibling space the engine ranked.
//!
//! Clip and scroll state is tracked as three [`FlowContext`]s: the current one
//! plus the two as seen from the nearest absolute- and fixed-containing-block
//! ancestors. A member keyed by **computed** position swaps the appropriate one
//! in as its own — that is precisely the containing-block escape rule
//! (CSS2 §11.1.1): boxes are only clipped by, and only scroll with, ancestors
//! in their containing-block chain. Both escapes are the same rule, which is
//! why one struct carries both.

use euclid::default::{Point2D, Rect, Size2D, Transform3D};
use hughie::style::containment::effective_containment;
use hughie::style::{Contain, CoreStyle, Overflow, PositionProperty, visibility};
use hughie::tree::{Layout, LayoutTree};
use stylo::properties::ComputedValues;
use stylo::values::computed::{CSSPixelLength, PointerEvents};

use super::geometry::{inner_radii, resolve_corner_radii};
use super::transform::{ParentPerspective, stacking_context_matrix};
use super::{
    AnimationSlot, ClipNode, CornerRadii, FrameBuffers, PaintItem, PaintItemKind, PaintOrder,
    RenderLayer, ScrollSlot, stacking,
};
use crate::layout::{
    DisplayMode, StyleView, box_parent, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, skips_contents,
};
use crate::scroll::ScrollAxes;
use crate::tree::document::{Document, DocumentLayoutState, NodeSlot, TreeArenas};
use crate::tree::node::Node;
use crate::vello::kurbo::Affine;
use crate::{NodeId, scroll};

/// Builds one frame's paint order into `buffers`, using and returning
/// `scratch`.
///
/// Both come from the painter: `buffers` is the storage of the frame it last
/// retired and `scratch` is the working set of the last build, so a document
/// whose page shape has stopped growing allocates nothing here per frame.
pub(crate) fn build<T: Sync>(
    document: &Document<T>,
    scratch: BuildScratch,
    buffers: FrameBuffers,
) -> (PaintOrder, BuildScratch) {
    let (tree, state) = document.visual_parts();
    let mut builder = Builder {
        document,
        tree,
        state,
        items: buffers.items,
        clips: buffers.clips,
        layers: buffers.layers,
        slots: buffers.slots,
        animations: buffers.animations,
        current_layer: None,
        scratch,
    };
    debug_assert!(
        builder.items.is_empty()
            && builder.clips.is_empty()
            && builder.layers.is_empty()
            && builder.slots.is_empty()
            && builder.animations.is_empty(),
        "a recycled frame is emptied before it is handed back to the builder",
    );
    builder.scratch.assert_settled();

    let visual_epoch = document.visual_epoch();
    let root = document.document_element();
    if let Some(style) = StyleView::try_of(root)
        && display_mode(style.display()) != DisplayMode::None
    {
        let location = builder.rounded(root.id()).location;
        builder.build_stacking_context(
            root.id(),
            &style,
            Point2D::new(location.x, location.y),
            &Transform3D::identity(),
            None,
            ClipContexts::default(),
        );
    }
    builder.scratch.assert_settled();

    (
        PaintOrder {
            items: builder.items,
            clips: builder.clips,
            layers: builder.layers,
            slots: builder.slots,
            animations: builder.animations,
            visual_epoch,
        },
        builder.scratch,
    )
}

/// The working buffers one paint-order build fills, retained across frames.
///
/// Three of them are stacks. `collect` and `build_stacking_context` append
/// their level's entries at the tail and truncate back to where they started
/// on the way out, so one buffer serves every level of the recursion instead
/// of one buffer per level.
///
/// The fourth cannot be a stack, for two independent reasons. A
/// pseudo-stacking context's records are produced while the enclosing context
/// is still producing its own, so on one buffer the two runs would interleave
/// and neither would be contiguous. And every pseudo of one context stays
/// live until that context emits its sorted member list, so they are all
/// open at once. Each pseudo therefore takes a whole buffer out of the pool
/// and returns it when its member is emitted.
#[derive(Debug, Default)]
pub(crate) struct BuildScratch {
    ranked: Vec<RankedChild>,
    members: Vec<Member>,
    stream: Vec<ItemRecord>,
    pseudo_pool: Vec<Vec<ItemRecord>>,
    pseudo_free: Vec<u32>,
}

impl BuildScratch {
    /// The working buffers' capacities: children ranking, members, in-flow
    /// records, the pseudo-context pool's length, then one entry per pooled
    /// buffer.
    #[cfg(test)]
    pub(crate) fn capacities(&self) -> Vec<usize> {
        let mut out = vec![
            self.ranked.capacity(),
            self.members.capacity(),
            self.stream.capacity(),
            self.pseudo_pool.len(),
        ];
        out.extend(self.pseudo_pool.iter().map(Vec::capacity));
        out
    }

    /// Every stack is balanced and every pooled buffer is back in the pool.
    ///
    /// Checked on the way in as well as on the way out: a build that left a
    /// level behind would otherwise corrupt the next frame rather than the
    /// one that made the mistake.
    fn assert_settled(&self) {
        debug_assert!(
            self.ranked.is_empty(),
            "the child ranking stack is balanced"
        );
        debug_assert!(self.members.is_empty(), "the member stack is balanced");
        debug_assert!(
            self.stream.is_empty(),
            "the in-flow stream stack is balanced"
        );
        debug_assert_eq!(
            self.pseudo_free.len(),
            self.pseudo_pool.len(),
            "every pooled pseudo-context buffer was returned",
        );
    }
}

/// One flattened child, keyed for the order-modified document-order sort.
#[derive(Debug, Clone, Copy)]
struct RankedChild {
    order: u32,
    index: u32,
    slot: NodeSlot,
}

/// Where the records a collection level produces are appended.
#[derive(Debug, Clone, Copy)]
enum StreamTarget {
    /// The enclosing stacking context's in-flow stream.
    Context,
    /// One pseudo-stacking context's pooled buffer, by pool index.
    Pseudo(u32),
}

/// The box a collection level is descending into: whose flattened children
/// are being collected, the origin their locations count from, whether it
/// ranks them as flex/grid items, and the clip and scroll state in force
/// inside it.
#[derive(Debug, Clone, Copy)]
struct Cursor<'ctx> {
    node: NodeId,
    offset: Point2D<f32>,
    is_item_container: bool,
    ctx: &'ctx ClipContexts,
}

/// One flattened child resolved into what the collection walk decides on.
///
/// `level` doubles as the stacking-context predicate: `Some` is the stack
/// level of a real stacking context, `None` is a box that paints into the
/// enclosing one.
#[derive(Debug)]
struct ChildBox<'doc, T> {
    node: NodeId,
    level: Option<i32>,
    offset: Point2D<f32>,
    size: Size2D<f32>,
    clips: ClipContexts,
    position: PositionProperty,
    mode: DisplayMode,
    view: StyleView<'doc, T>,
}

/// The state one stacking context's whole collection walk shares: the
/// context's world matrix, and the counter that puts its members in
/// order-modified document order.
#[derive(Debug)]
struct Collection<'ctx> {
    world: &'ctx Transform3D<f32>,
    seq: u32,
}

impl Collection<'_> {
    fn next_seq(&mut self) -> u32 {
        let current = self.seq;
        self.seq += 1;
        current
    }
}

/// What a box inherits from its containing-block chain: the innermost clip
/// and the nearest scroll container on that chain as a scroll-slot index.
///
/// Scroll translations are no longer folded into item transforms here: the
/// frame is baked in *unscrolled* coordinates, and every consumer — the
/// composed scene, hit testing, culling — applies the chain's translations
/// at use time from the slot table. What the escape rule used to do for the
/// folded translation it now does for `chain`: a member keyed `absolute` or
/// `fixed` swaps in its containing block's context, and its slot chain swaps
/// with it (the containing-block escape of CSS2 §11.1.1).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct FlowContext {
    clip: Option<usize>,
    /// The nearest scroll container on this chain, in the frame's slot table.
    chain: Option<u32>,
    /// The nearest composite-animated ancestor-or-self, in the frame's
    /// animation-slot table. Content under it composes through that slot's
    /// sampled delta.
    animation: Option<u32>,
}

/// The flow contexts visible at one point of the walk: the in-flow one plus
/// those captured at the nearest absolute-/fixed-containing-block ancestors
/// (what an escaping positioned descendant swaps in).
#[derive(Debug, Clone, Copy, Default)]
struct ClipContexts {
    current: FlowContext,
    absolute: FlowContext,
    fixed: FlowContext,
}

/// A finished paint item minus its final matrix: `offset` is the border-box
/// origin relative to the enclosing stacking context's origin.
#[derive(Debug)]
struct ItemRecord {
    node: NodeId,
    kind: PaintItemKind,
    offset: Point2D<f32>,
    clip: Option<usize>,
    size: Size2D<f32>,
    radii: CornerRadii,
    hit_testable: bool,
    slot: Option<u32>,
    animation: Option<u32>,
}

/// One member of a stacking context, awaiting the `(level, seq)` sort.
///
/// `Copy`, so the emission loop can lift one out of the shared member stack
/// and release the borrow before recursing — which the `Context` payload
/// does, back into `build_stacking_context`.
#[derive(Debug, Clone, Copy)]
struct Member {
    level: i32,
    seq: u32,
    payload: MemberPayload,
}

#[derive(Debug, Clone, Copy)]
enum MemberPayload {
    Context {
        node: NodeId,
        offset: Point2D<f32>,
        clips: ClipContexts,
    },
    /// A positioned box with `z-index: auto` and no other trigger. `stream`
    /// indexes the pooled buffer holding the records it and its non-escaping
    /// descendants produced.
    Pseudo { stream: u32 },
}

struct Builder<'doc, T> {
    document: &'doc Document<T>,
    tree: &'doc TreeArenas<T>,
    state: &'doc DocumentLayoutState,
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    slots: Vec<ScrollSlot>,
    animations: Vec<AnimationSlot>,
    current_layer: Option<usize>,
    scratch: BuildScratch,
}

impl<'doc, T: Sync> Builder<'doc, T> {
    fn node(&self, id: NodeId) -> &'doc Node<T> {
        self.tree.live(id)
    }

    fn rounded(&self, id: NodeId) -> &'doc Layout {
        &self.state.at(self.tree.live_slot(id)).slot.rounded
    }

    /// Records `node` in the frame's scroll-slot table when it is a scroll
    /// container, linked to the nearest container on its containing-block
    /// chain, and returns its slot.
    ///
    /// Allocated beside the element's own item — before any descend, and
    /// whether or not the box is visible or a leaf — so every container the
    /// frame lays out is a chain target, including one whose contents are
    /// never painted.
    fn allocate_scroll_slot(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        parent: Option<u32>,
    ) -> Option<u32> {
        let state = self.state.at(self.tree.live_slot(node));
        let scroll_box = scroll::resolve(style, &state.slot.rounded, state.scroll_offset)?;
        self.slots.push(ScrollSlot {
            node,
            parent,
            user_scrollable: scroll_box.user_scrollable,
            offset: scroll_box.offset,
            max_offset: scroll_box.max_offset(),
            scrollport: scroll_box.scrollport,
        });
        Some(
            u32::try_from(self.slots.len() - 1)
                .expect("a frame cannot hold 2^32 scroll containers"),
        )
    }

    /// Records `node` in the frame's animation-slot table when it carries a
    /// composite-exportable animation, linked to the nearest animated
    /// ancestor.
    ///
    /// Structural refusals live here beside the geometric attach: an
    /// element inside a composited group cannot export (the group's bounds
    /// were computed for the committed geometry), and a transform track
    /// needs a 2D, invertible decomposition of the element's world matrix
    /// with no individual transforms, motion path, or inherited
    /// perspective in the way. A refusal allocates nothing; the element
    /// keeps animating through main-thread ticks.
    fn allocate_animation_slot(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
        parent_perspective: Option<ParentPerspective>,
        parent: Option<u32>,
    ) -> Option<u32> {
        let node_ref = self.node(node);
        if !node_ref.may_have_animations() || self.current_layer.is_some() {
            return None;
        }
        let export = self.document.composite_export(node_ref)?;
        let mut curve = export.curve;
        if let Some(track) = export.transform_track {
            curve.transform = Some(self.attach_transform_track(
                track,
                style,
                world,
                size,
                parent_perspective,
                &export.committed_transform,
            )?);
        }
        self.animations.push(AnimationSlot {
            node,
            parent,
            curve: Some(curve),
        });
        Some(
            u32::try_from(self.animations.len() - 1)
                .expect("a frame cannot hold 2^32 animation slots"),
        )
    }

    /// Attaches the geometry a transform track's delta needs: with the
    /// element's world `W = pre · L · origin⁻¹` — which holds exactly when
    /// nothing but the transform list and origin contribute — the constant
    /// factor is `pre = W · origin · Lc⁻¹`, and the compose-time delta is
    /// `pre · L(t) · Lc⁻¹ · pre⁻¹`.
    #[expect(
        clippy::unused_self,
        reason = "kept beside the slot allocation it completes"
    )]
    fn attach_transform_track(
        &self,
        track: crate::visual::curves::Track<crate::visual::curves::TransformList>,
        style: &ComputedValues,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
        parent_perspective: Option<ParentPerspective>,
        committed: &crate::visual::curves::TransformList,
    ) -> Option<crate::visual::curves::TransformTrack> {
        use crate::visual::curves::transform_list_matrix;
        let box_style = style.get_box();
        let individual_transforms_present =
            !matches!(box_style.scale, stylo::values::computed::Scale::None)
                || !matches!(box_style.rotate, stylo::values::computed::Rotate::None)
                || !matches!(
                    box_style.translate,
                    stylo::values::computed::Translate::None
                );
        if parent_perspective.is_some()
            || individual_transforms_present
            || super::motion::offset_sample(style, size).is_some()
        {
            return None;
        }
        let origin = &box_style.transform_origin;
        if origin.depth.px() != 0.0 {
            return None;
        }
        let origin_affine = Affine::translate((
            f64::from(
                origin
                    .horizontal
                    .resolve(CSSPixelLength::new(size.width))
                    .px(),
            ),
            f64::from(
                origin
                    .vertical
                    .resolve(CSSPixelLength::new(size.height))
                    .px(),
            ),
        ));
        let world = affine_2d(world)?;
        let committed_matrix = transform_list_matrix(committed);
        if committed_matrix.determinant().abs() < 1e-9 || world.determinant().abs() < 1e-9 {
            return None;
        }
        let committed_inverse = committed_matrix.inverse();
        let pre = world * origin_affine * committed_inverse;
        Some(crate::visual::curves::TransformTrack {
            track,
            pre,
            pre_inverse: pre.inverse(),
            committed_inverse,
        })
    }

    /// Sets every slot on `chain` back to the committed values: something
    /// under the animated subtree — a clip, a scroll container — cannot ride
    /// a sampled delta, so the whole chain falls back to main-thread ticks.
    fn kill_animation_chain(&mut self, chain: Option<u32>) {
        let mut current = chain;
        while let Some(index) = current {
            let slot = &mut self.animations[index as usize];
            slot.curve = None;
            current = slot.parent;
        }
    }

    fn build_stacking_context(
        &mut self,
        root: NodeId,
        style: &StyleView<'doc, T>,
        offset_in_parent: Point2D<f32>,
        parent_world: &Transform3D<f32>,
        parent_perspective: Option<ParentPerspective>,
        seed: ClipContexts,
    ) {
        let values = style.values();
        let size = {
            let layout = self.rounded(root);
            Size2D::new(layout.size.width, layout.size.height)
        };
        let world = stacking_context_matrix(values, size, offset_in_parent, parent_perspective)
            .then(parent_world);
        let own_animation = self.allocate_animation_slot(
            root,
            values,
            &world,
            size,
            parent_perspective,
            seed.current.animation,
        );
        let animation = own_animation.or(seed.current.animation);
        let force_group = own_animation.is_some_and(|index| {
            self.animations[index as usize]
                .curve
                .as_ref()
                .is_some_and(|curve| curve.opacity.is_some())
        });
        let layer = self.open_layer(
            root,
            values,
            &world,
            size,
            seed.current.chain,
            animation,
            force_group,
        );

        let own_slot = self.allocate_scroll_slot(root, values, seed.current.chain);
        if own_slot.is_some() {
            // The animated element is itself a scroll container: its own
            // clip and its content's scroll translation cannot ride a
            // sampled delta.
            self.kill_animation_chain(animation);
        }
        let (visible, hit_testable) = item_flags(values);
        if visible {
            self.items.push(PaintItem {
                node: root,
                kind: PaintItemKind::ElementBox,
                transform: world,
                clip: seed.current.clip,
                size,
                radii: resolve_corner_radii(values, size),
                hit_testable,
                slot: own_slot.or(seed.current.chain),
                animation,
            });
        }

        let mode = display_mode(style.display());
        if mode == DisplayMode::Leaf || skips_contents(values) {
            self.close_layer(layer);
            return;
        }
        let ctx = self.enter_element(root, values, &world, seed, own_slot, own_animation);

        // This context's members and in-flow records occupy the tail of the
        // two shared stacks. Anything a nested context pushes lands above
        // `member_end` / `stream_end` and is truncated away before it
        // returns, so these two ranges stay put across the emission below.
        let member_base = self.scratch.members.len();
        let stream_base = self.scratch.stream.len();
        let mut collection = Collection {
            world: &world,
            seq: 0,
        };
        self.collect(
            Cursor {
                node: root,
                offset: Point2D::zero(),
                is_item_container: matches!(mode, DisplayMode::Flex | DisplayMode::Grid),
                ctx: &ctx,
            },
            StreamTarget::Context,
            &mut collection,
        );
        let member_end = self.scratch.members.len();
        let stream_end = self.scratch.stream.len();

        let child_perspective = ParentPerspective::of(values, size);
        // `(level, seq)` is a total order — `seq` is one monotone counter over
        // this context's collection walk — so the permutation is unique and an
        // unstable sort is exact. `partition_point` then names the boundary
        // the in-flow stream paints at, with nothing moved.
        let split = {
            let members = &mut self.scratch.members[member_base..member_end];
            members.sort_unstable_by_key(|member| (member.level, member.seq));
            member_base + members.partition_point(|member| member.level < 0)
        };

        let mut index = member_base;
        while index < split {
            let member = self.scratch.members[index];
            index += 1;
            self.emit_member(member, root, child_perspective, &world);
        }
        {
            // Two disjoint field borrows: nothing in this loop recurses, so
            // the records are read in place instead of copied out.
            let Builder { items, scratch, .. } = self;
            for record in &scratch.stream[stream_base..stream_end] {
                push_record(items, record, &world);
            }
        }
        while index < member_end {
            let member = self.scratch.members[index];
            index += 1;
            self.emit_member(member, root, child_perspective, &world);
        }

        self.scratch.members.truncate(member_base);
        self.scratch.stream.truncate(stream_base);
        self.close_layer(layer);
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a group captures exactly the element facts the stacking \
                  walk already holds"
    )]
    fn open_layer(
        &mut self,
        node: NodeId,
        values: &ComputedValues,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
        slot: Option<u32>,
        animation: Option<u32>,
        force_group: bool,
    ) -> Option<usize> {
        if !force_group && !stacking::needs_group_rendering(values) {
            return None;
        }
        let start = self.items.len();
        self.layers.push(RenderLayer {
            parent: self.current_layer,
            node,
            transform: *world,
            size,
            radii: resolve_corner_radii(values, size),
            slot,
            animation,
            items: start..start,
        });
        let index = self.layers.len() - 1;
        self.current_layer = Some(index);
        Some(index)
    }

    fn close_layer(&mut self, opened: Option<usize>) {
        let Some(index) = opened else { return };
        self.current_layer = self.layers[index].parent;
        let end = self.items.len();
        if end == self.layers[index].items.start {
            debug_assert_eq!(index, self.layers.len() - 1);
            self.layers.pop();
        } else {
            self.layers[index].items.end = end;
        }
    }

    /// Appends `node`'s flattened children to the ranking stack in
    /// order-modified document order, and returns where the run starts.
    ///
    /// `Layout::order` is the order-modified paint index the layout algorithm
    /// assigned over this same flattened sibling space. When no sibling
    /// carries a non-zero CSS `order`, that index *is* the flattened index,
    /// so the run arrives strictly increasing in `(order, index)` and the
    /// sort would be the identity permutation. The fill scan detects that and
    /// skips it: `index` increases by construction, so a non-decreasing
    /// `order` makes the whole key strictly increasing.
    fn rank_children(&mut self, node: NodeId) -> usize {
        let base = self.scratch.ranked.len();
        // Copied out first: both are `&'doc`, so the iterator borrows the
        // arenas rather than the builder, leaving the pushes below free to
        // take `&mut self.scratch`.
        let tree = self.tree;
        let state = self.state;
        let mut previous = 0_u32;
        let mut sorted = true;
        for (index, (child, _, _)) in tree.flattened_children(tree.live_slot(node)).enumerate() {
            let order = rounded_at(state, child).order;
            sorted &= order >= previous;
            previous = order;
            self.scratch.ranked.push(RankedChild {
                order,
                index: u32::try_from(index).expect("a box cannot hold 2^32 flattened children"),
                slot: child,
            });
        }
        if !sorted {
            self.scratch.ranked[base..].sort_unstable_by_key(|child| (child.order, child.index));
        }
        base
    }

    fn collect(
        &mut self,
        cursor: Cursor<'_>,
        target: StreamTarget,
        collection: &mut Collection<'_>,
    ) {
        let base = self.rank_children(cursor.node);
        let end = self.scratch.ranked.len();

        // Indices rather than an iterator: the recursion below pushes onto the
        // ranking stack above `end` and truncates back to it, which may
        // reallocate. Entries in `base..end` are never touched, so
        // re-subscripting each step is both sound and stable.
        let mut position = base;
        while position < end {
            let child_slot = self.scratch.ranked[position].slot;
            position += 1;
            self.collect_child(child_slot, cursor, target, collection);
        }
        self.scratch.ranked.truncate(base);
    }

    /// Resolves one flattened child into the facts the collection walk keys
    /// on, or `None` when it produces no box at all.
    ///
    /// `offset` already carries the containing-block escape: a member keyed
    /// `absolute` or `fixed` counts from its containing block's scroll frame,
    /// not from the frame it was found in.
    fn resolve_child(
        &self,
        child_node: &'doc Node<T>,
        cursor: Cursor<'_>,
    ) -> Option<ChildBox<'doc, T>> {
        let view = StyleView::try_of(child_node)?;
        let mode = display_mode(view.display());
        if mode == DisplayMode::None {
            return None;
        }
        debug_assert_ne!(
            mode,
            DisplayMode::Contents,
            "flattened_children never yields a box-less element",
        );
        let node = child_node.id();
        let style = view.values();
        let (offset, size) = {
            let layout = self.rounded(node);
            (
                Point2D::new(
                    cursor.offset.x + layout.location.x,
                    cursor.offset.y + layout.location.y,
                ),
                Size2D::new(layout.size.width, layout.size.height),
            )
        };
        let position = style.clone_position();
        let clips = member_clip_contexts(position, *cursor.ctx);
        let z_applies = stacking::z_index_applies(position, cursor.is_item_container);
        // An element whose running animation moves only composite
        // properties paints as a stacking context even where its committed
        // style would not make one — the same rule browsers apply to
        // animated `opacity`/`transform` — so its subtree is one atomic,
        // retargetable unit.
        let forced_context = child_node.may_have_animations()
            && self.document.animates_composite_properties(child_node);
        Some(ChildBox {
            node,
            level: (stacking::establishes_stacking_context(style, z_applies) || forced_context)
                .then(|| stacking::stack_level(style, z_applies)),
            offset,
            size,
            clips,
            position,
            mode,
            view,
        })
    }

    fn collect_child(
        &mut self,
        child_slot: NodeSlot,
        cursor: Cursor<'_>,
        target: StreamTarget,
        collection: &mut Collection<'_>,
    ) {
        let ctx = *cursor.ctx;
        let child_node = self.tree.at(child_slot);
        if child_node.is_text_node() {
            if let Some(record) = self.text_record(child_node, cursor.offset, ctx) {
                self.push_stream(target, record);
            }
            return;
        }
        if !child_node.is_element() {
            return;
        }
        let Some(child) = self.resolve_child(child_node, cursor) else {
            return;
        };
        // A real stacking context paints atomically and out of order, so it is
        // only recorded here; `emit_member` recurses into it once the whole
        // enclosing context has been collected and sorted.
        if let Some(level) = child.level {
            let seq = collection.next_seq();
            self.scratch.members.push(Member {
                level,
                seq,
                payload: MemberPayload::Context {
                    node: child.node,
                    offset: child.offset,
                    clips: child.clips,
                },
            });
            return;
        }
        self.collect_in_context(&child, ctx, target, collection);
    }

    /// Collects a child that paints inside the enclosing stacking context.
    ///
    /// Two shapes. A positioned box with `z-index: auto` and no other trigger
    /// is a *pseudo*-stacking context: it paints its own item and its in-flow
    /// content into a buffer of its own and surfaces as one level-0 member, so
    /// that its positioned descendants can still interleave with the enclosing
    /// context's members (CSS2 §E.2 step 8). Everything else appends straight
    /// to `target`.
    fn collect_in_context(
        &mut self,
        child: &ChildBox<'doc, T>,
        ctx: ClipContexts,
        target: StreamTarget,
        collection: &mut Collection<'_>,
    ) {
        let style = child.view.values();
        let (visible, hit_testable) = item_flags(style);
        let descend = child.mode != DisplayMode::Leaf && !skips_contents(style);
        let is_item_container = matches!(child.mode, DisplayMode::Flex | DisplayMode::Grid);
        // A pseudo-context takes its sequence number before descending, so it
        // sorts where it was *found*, not where it finished; a static box takes
        // none at all, because it is not a member.
        let pseudo = (child.position != PositionProperty::Static)
            .then(|| (collection.next_seq(), self.take_pseudo_stream()));
        let (target, outer) = match pseudo {
            Some((_, stream)) => (StreamTarget::Pseudo(stream), child.clips),
            None => (target, ctx),
        };

        let own_slot = self.allocate_scroll_slot(child.node, style, outer.current.chain);
        if own_slot.is_some() {
            // A scroll container inside an animated subtree: its content's
            // scroll translation cannot compose inside a sampled delta.
            self.kill_animation_chain(outer.current.animation);
        }
        if visible {
            self.push_stream(
                target,
                element_record(
                    child.node,
                    style,
                    child.offset,
                    child.size,
                    outer.current.clip,
                    hit_testable,
                    own_slot.or(outer.current.chain),
                    outer.current.animation,
                ),
            );
        }
        if descend {
            let inner = self.enter_element(
                child.node,
                style,
                &translated(collection.world, child.offset),
                outer,
                own_slot,
                None,
            );
            self.collect(
                Cursor {
                    node: child.node,
                    offset: child.offset,
                    is_item_container,
                    ctx: &inner,
                },
                target,
                collection,
            );
        }
        if let Some((seq, stream)) = pseudo {
            self.scratch.members.push(Member {
                level: 0,
                seq,
                payload: MemberPayload::Pseudo { stream },
            });
        }
    }

    /// An empty pooled record buffer for one pseudo-stacking context.
    fn take_pseudo_stream(&mut self) -> u32 {
        if let Some(index) = self.scratch.pseudo_free.pop() {
            debug_assert!(self.scratch.pseudo_pool[index as usize].is_empty());
            return index;
        }
        self.scratch.pseudo_pool.push(Vec::new());
        u32::try_from(self.scratch.pseudo_pool.len() - 1)
            .expect("a document cannot hold 2^32 open pseudo-stacking contexts")
    }

    fn push_stream(&mut self, target: StreamTarget, record: ItemRecord) {
        match target {
            StreamTarget::Context => self.scratch.stream.push(record),
            StreamTarget::Pseudo(index) => self.scratch.pseudo_pool[index as usize].push(record),
        }
    }

    fn emit_member(
        &mut self,
        member: Member,
        context_root: NodeId,
        context_perspective: Option<ParentPerspective>,
        world: &Transform3D<f32>,
    ) {
        match member.payload {
            MemberPayload::Context {
                node: member_node,
                offset,
                clips,
            } => {
                let node = self.node(member_node);
                let style = StyleView::try_of(node)
                    .expect("stacking members keep their style for the whole build");
                let perspective = (box_parent(node).map(Node::id) == Some(context_root))
                    .then_some(context_perspective)
                    .flatten();
                self.build_stacking_context(member_node, &style, offset, world, perspective, clips);
            }
            MemberPayload::Pseudo { stream } => {
                // Three disjoint field borrows at once; nothing here recurses,
                // so the records are drained in place.
                let Builder { items, scratch, .. } = self;
                let buffer = &mut scratch.pseudo_pool[stream as usize];
                for record in buffer.iter() {
                    push_record(items, record, world);
                }
                buffer.clear();
                scratch.pseudo_free.push(stream);
            }
        }
    }

    fn enter_element(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        transform: &Transform3D<f32>,
        ctx: ClipContexts,
        own_slot: Option<u32>,
        own_animation: Option<u32>,
    ) -> ClipContexts {
        let mut inner = ctx;
        let clipped = clipped_axes(style);
        if clipped.x || clipped.y {
            let (rect, radii) = {
                let layout = self.rounded(node);
                let padding_box = Rect::new(
                    Point2D::new(layout.border.left, layout.border.top),
                    Size2D::new(
                        (layout.size.width - layout.border.horizontal_sum()).max(0.0),
                        (layout.size.height - layout.border.vertical_sum()).max(0.0),
                    ),
                );
                let rect = unclipped_axes_unbounded(padding_box, clipped);
                let radii = if clipped.x && clipped.y {
                    let outer = resolve_corner_radii(
                        style,
                        Size2D::new(layout.size.width, layout.size.height),
                    );
                    inner_radii(outer, &layout.border)
                } else {
                    CornerRadii::ZERO
                };
                (rect, radii)
            };
            self.clips.push(ClipNode {
                parent: inner.current.clip,
                #[cfg(test)]
                node,
                transform: *transform,
                rect,
                radii,
                slot: inner.current.chain,
            });
            inner.current.clip = Some(self.clips.len() - 1);
            // A clip rect never rides a sampled delta; anything animated
            // around it falls back to main-thread ticks.
            self.kill_animation_chain(own_animation.or(ctx.current.animation));
        }
        if own_slot.is_some() {
            inner.current.chain = own_slot;
        }
        if own_animation.is_some() {
            inner.current.animation = own_animation;
        }
        let node_ref = self.node(node);
        if establishes_absolute_containing_block(node_ref, style) {
            inner.absolute = inner.current;
        }
        if establishes_fixed_containing_block(node_ref, style) {
            inner.fixed = inner.current;
        }
        inner
    }

    fn text_record(
        &self,
        child: &Node<T>,
        node_offset: Point2D<f32>,
        ctx: ClipContexts,
    ) -> Option<ItemRecord> {
        let parent = child.flat_parent()?;
        let (visible, hit_testable) = item_flags(StyleView::try_of(parent)?.values());
        if !visible {
            return None;
        }
        let layout = self.rounded(child.id());
        Some(ItemRecord {
            node: child.id(),
            kind: PaintItemKind::TextRun {
                element: parent.id(),
            },
            offset: Point2D::new(
                node_offset.x + layout.location.x,
                node_offset.y + layout.location.y,
            ),
            clip: ctx.current.clip,
            size: Size2D::new(layout.size.width, layout.size.height),
            radii: CornerRadii::ZERO,
            hit_testable,
            slot: ctx.current.chain,
            animation: ctx.current.animation,
        })
    }
}

/// The 2D affine of a world matrix, if it is one.
fn affine_2d(matrix: &Transform3D<f32>) -> Option<Affine> {
    if !matrix.is_2d() {
        return None;
    }
    Some(Affine::new([
        f64::from(matrix.m11),
        f64::from(matrix.m12),
        f64::from(matrix.m21),
        f64::from(matrix.m22),
        f64::from(matrix.m41),
        f64::from(matrix.m42),
    ]))
}

/// The rounded layout of a node the walk already holds a slot for. The paint
/// walk descends through [`hughie::tree::LayoutTree`], which runs in slot
/// space, so its children arrive resolved.
///
/// A free function, not a method, so the ranking loop can read layouts while
/// it appends to the builder's own scratch.
fn rounded_at(state: &DocumentLayoutState, slot: NodeSlot) -> &Layout {
    &state.at(slot).slot.rounded
}

/// A free function, not a method, so a caller can hold a disjoint borrow of
/// the record buffer it is draining while pushing into `items`.
fn push_record(items: &mut Vec<PaintItem>, record: &ItemRecord, world: &Transform3D<f32>) {
    items.push(PaintItem {
        node: record.node,
        kind: record.kind,
        transform: translated(world, record.offset),
        clip: record.clip,
        size: record.size,
        radii: record.radii,
        hit_testable: record.hit_testable,
        slot: record.slot,
        animation: record.animation,
    });
}

#[expect(
    clippy::too_many_arguments,
    reason = "a record is a flat capture of the walk's per-item state"
)]
fn element_record(
    node: NodeId,
    style: &ComputedValues,
    offset: Point2D<f32>,
    size: Size2D<f32>,
    clip: Option<usize>,
    hit_testable: bool,
    slot: Option<u32>,
    animation: Option<u32>,
) -> ItemRecord {
    ItemRecord {
        node,
        kind: PaintItemKind::ElementBox,
        offset,
        clip,
        size,
        radii: resolve_corner_radii(style, size),
        hit_testable,
        slot,
        animation,
    }
}

fn member_clip_contexts(position: PositionProperty, ctx: ClipContexts) -> ClipContexts {
    match position {
        PositionProperty::Absolute => ClipContexts {
            current: ctx.absolute,
            ..ctx
        },
        PositionProperty::Fixed => ClipContexts {
            current: ctx.fixed,
            ..ctx
        },
        _ => ctx,
    }
}

fn clipped_axes(style: &ComputedValues) -> ScrollAxes {
    if effective_containment(
        style.clone_contain(),
        style.clone_content_visibility(),
        skips_contents(style),
    )
    .intersects(Contain::PAINT)
    {
        return ScrollAxes::BOTH;
    }
    ScrollAxes {
        x: !matches!(style.clone_overflow_x(), Overflow::Visible),
        y: !matches!(style.clone_overflow_y(), Overflow::Visible),
    }
}

fn unclipped_axes_unbounded(rect: Rect<f32>, clipped: ScrollAxes) -> Rect<f32> {
    const UNBOUNDED: f32 = 1.0e7;
    let mut rect = rect;
    if !clipped.x {
        rect.origin.x = -UNBOUNDED;
        rect.size.width = 2.0 * UNBOUNDED;
    }
    if !clipped.y {
        rect.origin.y = -UNBOUNDED;
        rect.size.height = 2.0 * UNBOUNDED;
    }
    rect
}

fn item_flags(style: &ComputedValues) -> (bool, bool) {
    let visible = matches!(style.clone_visibility(), visibility::T::Visible);
    let hit_testable = visible && !matches!(style.clone_pointer_events(), PointerEvents::None);
    (visible, hit_testable)
}

fn translated(world: &Transform3D<f32>, offset: Point2D<f32>) -> Transform3D<f32> {
    Transform3D::translation(offset.x, offset.y, 0.0).then(world)
}
