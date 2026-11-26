//! The paint-order builder: CSS2 Appendix E over the laid-out tree,
//! collapsed for an engine with no floats.
//!
//! A paragraph — a Lynx text element, or a block container's inline
//! formatting context — paints as one record after its element's box: every
//! inline box background and border in tree order, then every glyph
//! (`paint::walker`). Its atomic inline boxes follow as records of their own,
//! each painted whole. That is Appendix E step 7's order for content that
//! does not overlap; it does not interleave the steps line by line. A
//! non-atomic inline box emits no record of its own and clips nothing
//! (`layout::inline_flow`).
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

use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};
use seldon::style::containment::effective_containment;
use seldon::style::{Contain, ContentVisibility, Overflow, PositionProperty, visibility};
use seldon::tree::{Layout, LayoutTree};
use stylo::computed_values::scroll_initial_target;
use stylo::properties::ComputedValues;
use stylo::values::computed::PointerEvents;
use stylo::values::computed::transform::Transform as ComputedTransform;

use super::anchored::AnchoredSlot;
use super::curves::TransformTrack;
use super::frame::MAX_MOVING_EXTENT_VIEWPORTS;
use super::geometry::{inner_radii, resolve_corner_radii};
use super::transform::{ContextMatrix, ParentPerspective, as_translation, same_bits};
use super::{
    AnimationSlot, AutoBox, ClipNode, CornerRadii, FrameBuffers, ItemTransform, LayerKind,
    PaintItem, PaintItemKind, PaintOrder, RenderLayer, ScrollSlot, SnapSlot, SnapSlotAxis, Space,
    SpaceKind, StickySlot, stacking, svg,
};
use crate::layout::inline_flow::is_inline_box;
use crate::layout::{
    DisplayMode, StyleView, box_parent, establishes_absolute_containing_block,
    establishes_fixed_containing_block, skips_contents,
};
use crate::scroll;
use crate::scroll::initial_target::InitialTarget;
use crate::scroll::{ScrollAxes, ScrollRequest, SnapAxisPositions, SnapPoint};
use crate::style::curve_export::ExportedComposite;
use crate::tree::document::{Document, DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;
use crate::tree::top_layer;

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
        stickies: buffers.stickies,
        spaces: buffers.spaces,
        anchored: buffers.anchored,
        auto_boxes: buffers.auto_boxes,
        snap_points: buffers.snap_points,
        matrices: buffers.matrices,
        initial_targets: Vec::new(),
        current_layer: None,
        scratch,
    };
    debug_assert!(
        builder.items.is_empty()
            && builder.clips.is_empty()
            && builder.layers.is_empty()
            && builder.slots.is_empty()
            && builder.animations.is_empty()
            && builder.stickies.is_empty()
            && builder.spaces.is_empty()
            && builder.anchored.is_empty()
            && buffers.anchor_links.is_empty()
            && buffers.anchor_clips.is_empty()
            && buffers.anchored_order.is_empty()
            && builder.auto_boxes.is_empty()
            && builder.snap_points.is_empty()
            && builder.matrices.is_empty(),
        "a recycled frame is emptied before it is handed back to the builder",
    );
    builder.scratch.assert_settled();

    let commit_id = document.commit_id();
    let root = document.document_element();
    if let Some(style) = StyleView::try_of(root)
        && style.display_mode() != DisplayMode::None
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
    let inert_floor = if tree.top_layer().is_empty() {
        None
    } else {
        builder.build_top_layer()
    };
    builder.scratch.scroll_stickies.clear();
    builder.scratch.assert_settled();

    let mut slot_index = buffers.slot_index;
    debug_assert!(slot_index.is_empty(), "a recycled index is emptied");
    slot_index.extend(builder.slots.iter().enumerate().map(|(index, slot)| {
        (
            slot.node,
            u32::try_from(index).expect("a frame cannot hold 2^32 scroll containers"),
        )
    }));
    let auto_scroll_requests = state
        .auto_scroll_requests
        .iter()
        .filter(|(node, _)| slot_index.contains_key(node))
        .copied()
        .collect();
    let mut order = PaintOrder {
        items: builder.items,
        clips: builder.clips,
        layers: builder.layers,
        slots: builder.slots,
        slot_index,
        animations: builder.animations,
        stickies: builder.stickies,
        spaces: builder.spaces,
        anchored: builder.anchored,
        anchor_links: buffers.anchor_links,
        anchor_clips: buffers.anchor_clips,
        anchored_order: buffers.anchored_order,
        auto_boxes: builder.auto_boxes,
        snap_points: builder.snap_points,
        matrices: builder.matrices,
        initial_targets: builder.initial_targets,
        auto_scroll_requests,
        inert_floor,
        commit_id,
    };
    order.bind_anchored(document);
    (order, builder.scratch)
}

#[derive(Debug, Default)]
pub(crate) struct BuildScratch {
    ranked: Vec<RankedChild>,
    members: Vec<Member>,
    stream: Vec<ItemRecord>,
    pseudo_pool: Vec<Vec<ItemRecord>>,
    pseudo_free: Vec<u32>,
    scroll_stickies: Vec<Option<u32>>,
}

impl BuildScratch {
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

    fn assert_settled(&self) {
        debug_assert!(
            self.ranked.is_empty(),
            "the child ranking stack is balanced"
        );
        debug_assert!(self.members.is_empty(), "the member stack is balanced");
        debug_assert!(
            self.scroll_stickies.is_empty(),
            "scrollport attachments are frame-local"
        );
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

#[derive(Debug, Clone, Copy)]
struct RankedChild {
    order: u32,
    index: u32,
    slot: NodeSlot,
}

#[derive(Debug, Clone, Copy)]
enum StreamTarget {
    Context,
    Pseudo(u32),
}

const fn ranks_children_as_items(mode: DisplayMode) -> bool {
    matches!(
        mode,
        DisplayMode::Flex | DisplayMode::Grid | DisplayMode::GridLanes
    )
}

#[derive(Debug, Clone, Copy)]
struct Cursor<'ctx> {
    node: NodeId,
    offset: Point2D<f32>,
    is_item_container: bool,
    ctx: &'ctx ClipContexts,
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct FlowContext {
    clip: Option<usize>,
    space: Option<u32>,
    chain: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default)]
struct ClipContexts {
    current: FlowContext,
    absolute: FlowContext,
    fixed: FlowContext,
}

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
    space: Option<u32>,
    svg_root: bool,
}

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
    Pseudo {
        stream: u32,
    },
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
    stickies: Vec<StickySlot>,
    spaces: Vec<Space>,
    anchored: Vec<AnchoredSlot>,
    auto_boxes: Vec<AutoBox>,
    snap_points: Vec<SnapPoint>,
    matrices: Vec<Transform3D<f32>>,
    initial_targets: Vec<InitialTarget>,
    current_layer: Option<usize>,
    scratch: BuildScratch,
}

impl<'doc, T: Sync> Builder<'doc, T> {
    fn node(&self, id: NodeId) -> &'doc Node<T> {
        self.tree.live(id)
    }

    fn rounded(&self, id: NodeId) -> &'doc Layout {
        &self.state.slot_at(self.tree.live_slot(id)).rounded
    }

    fn push_space(&mut self, parent: Option<u32>, kind: SpaceKind) -> u32 {
        self.spaces.push(Space { parent, kind });
        u32::try_from(self.spaces.len() - 1).expect("a frame cannot hold 2^32 spaces")
    }

    fn nearest_sticky(&self, space: Option<u32>) -> Option<u32> {
        super::space::nearest_sticky(&self.spaces, space)
    }

    fn enter_anchored(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        parent_world: &Transform3D<f32>,
        (space, clip): (Option<u32>, Option<usize>),
        fixed: &mut FlowContext,
    ) -> Option<u32> {
        if self.state.anchored.is_empty()
            || !matches!(
                *style.get_box().get_position(),
                PositionProperty::Absolute | PositionProperty::Fixed
            )
        {
            return space;
        }
        let Some(slot) = AnchoredSlot::allocate(self.document, node, style, parent_world, clip)
        else {
            return space;
        };
        let can_hide = slot.can_hide();
        self.anchored.push(slot);
        let index = u32::try_from(self.anchored.len() - 1)
            .expect("a frame cannot hold 2^32 anchored boxes");
        let anchored = self.push_space(space, SpaceKind::Anchored(index));
        if can_hide && !establishes_fixed_containing_block(self.node(node), style) {
            fixed.space = Some(self.push_space(fixed.space, SpaceKind::AnchoredVisibility(index)));
        }
        Some(anchored)
    }

    fn allocate_scroll_slot(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        parent: Option<u32>,
        sticky: Option<u32>,
        world: &Transform3D<f32>,
    ) -> Option<u32> {
        if !scroll::is_scroll_container(style) {
            return None;
        }
        let slot = self.tree.live_slot(node);
        let offset = self
            .state
            .scroll_offsets
            .get(&slot)
            .copied()
            .unwrap_or_default();
        let scroll_box = scroll::resolve(style, &self.state.slot_at(slot).rounded, offset)?;
        let snap = self
            .document
            .snap_positions_of(node, style, &scroll_box)
            .map_or_else(SnapSlot::default, |positions| SnapSlot {
                x: positions.x.map(|axis| self.push_snap_axis(&axis)),
                y: positions.y.map(|axis| self.push_snap_axis(&axis)),
            });
        let max_offset = scroll_box.max_offset();
        let request = self
            .state
            .scroll_requests
            .get(&node)
            .map(|request| ScrollRequest {
                target: scroll_box.admit(request.target),
                ..*request
            });
        self.scratch.scroll_stickies.push(sticky);
        self.slots.push(ScrollSlot {
            node,
            parent,
            user_scrollable: scroll_box.user_scrollable,
            chains: scroll_box.chains,
            bounce: scroll_box.bounce,
            circular: scroll_box.circular,
            capture: scroll_box.capture,
            snap,
            offset: scroll_box.offset,
            max_offset,
            scrollport: scroll_box.scrollport,
            viewport_axes: [
                euclid::vec2(world.m11, world.m12),
                euclid::vec2(world.m21, world.m22),
            ],
            request,
            anchor_shift: self.state.scroll_anchors.get(&slot).map_or_else(
                Vector2D::zero,
                crate::scroll::anchoring::ScrollAnchor::shift,
            ),
            reach: self
                .state
                .scroll_reaches
                .get(&slot)
                .copied()
                .unwrap_or_default(),
        });
        Some(
            u32::try_from(self.slots.len() - 1)
                .expect("a frame cannot hold 2^32 scroll containers"),
        )
    }

    fn push_snap_axis(&mut self, axis: &SnapAxisPositions) -> SnapSlotAxis {
        let start =
            u32::try_from(self.snap_points.len()).expect("a frame cannot hold 2^32 snap positions");
        self.snap_points.extend_from_slice(&axis.points);
        SnapSlotAxis {
            strictness: axis.strictness,
            start,
            end: start + u32::try_from(axis.points.len()).expect("bounded by the table"),
        }
    }

    fn allocate_sticky_slot(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        flow: FlowContext,
        parent_transform: &Transform3D<f32>,
    ) -> Option<u32> {
        if *style.get_box().get_position() != PositionProperty::Sticky {
            return None;
        }
        let mut scroll = [None; 2];
        let mut current = flow.chain;
        while let Some(index) = current {
            let entry = &self.slots[index as usize];
            let values = self
                .document
                .paint_style(entry.node)
                .expect("a recorded scroll container keeps its style through the build");
            if scroll[0].is_none() && values.get_overflow_x().is_scrollable() {
                scroll[0] = Some(index);
            }
            if scroll[1].is_none() && values.get_overflow_y().is_scrollable() {
                scroll[1] = Some(index);
            }
            current = entry.parent;
        }
        let axes = super::sticky::axes(
            self.document,
            node,
            scroll[0].map(|index| self.slots[index as usize].node),
            scroll[1].map(|index| self.slots[index as usize].node),
        );
        if axes.iter().all(|axis| axis.offset_bounds() == (0.0, 0.0)) {
            return None;
        }
        self.stickies.push(StickySlot {
            node,
            parent: self.nearest_sticky(flow.space),
            scroll,
            scroll_sticky: scroll
                .map(|index| index.and_then(|index| self.scratch.scroll_stickies[index as usize])),
            axes,
            parent_transform: *parent_transform,
        });
        Some(u32::try_from(self.stickies.len() - 1).expect("a frame cannot hold 2^32 sticky boxes"))
    }

    fn build_top_layer(&mut self) -> Option<usize> {
        debug_assert!(
            self.current_layer.is_none(),
            "the root context closed its layer"
        );
        let mut floor = None;
        for entry in self.tree.top_layer().entries() {
            if entry.blocks_document() {
                floor = Some(self.items.len());
            }
            if !top_layer::is_rendered(self.tree, self.state, entry.element()) {
                continue;
            }
            if top_layer::backdrop_generates_box(self.tree, entry.backdrop) {
                let style = StyleView::try_of(self.node(entry.backdrop))
                    .expect("a backdrop that generates a box has a style");
                let location = self.rounded(entry.backdrop).location;
                let first = self.items.len();
                self.build_stacking_context(
                    entry.backdrop,
                    &style,
                    Point2D::new(location.x, location.y),
                    &Transform3D::identity(),
                    None,
                    ClipContexts::default(),
                );
                for item in &mut self.items[first..] {
                    if item.node == entry.backdrop && item.kind == PaintItemKind::ElementBox {
                        item.kind = PaintItemKind::Backdrop {
                            element: entry.element(),
                        };
                    }
                }
            }
            let style = StyleView::try_of(self.node(entry.element()))
                .expect("a rendered top-layer element has a style");
            let location = self.viewport_origin(entry.element());
            self.build_stacking_context(
                entry.element(),
                &style,
                location,
                &Transform3D::identity(),
                None,
                ClipContexts::default(),
            );
        }
        floor
    }

    fn viewport_origin(&self, id: NodeId) -> Point2D<f32> {
        let mut origin = Point2D::zero();
        let mut current = Some(self.node(id));
        while let Some(node) = current.filter(|node| node.is_element()) {
            let location = self.rounded(node.id()).location;
            origin.x += location.x;
            origin.y += location.y;
            current = node.flat_parent();
        }
        origin
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one linear pass over a context's own box, then its members"
    )]
    fn build_stacking_context(
        &mut self,
        root: NodeId,
        style: &StyleView<'doc, T>,
        offset_in_parent: Point2D<f32>,
        parent_world: &Transform3D<f32>,
        parent_perspective: Option<ParentPerspective>,
        mut seed: ClipContexts,
    ) {
        let values = style.values();
        let mut box_space = self.enter_anchored(
            root,
            values,
            parent_world,
            (seed.current.space, seed.current.clip),
            &mut seed.fixed,
        );
        let own_sticky = self.allocate_sticky_slot(root, values, seed.current, parent_world);
        if let Some(index) = own_sticky {
            box_space = Some(self.push_space(box_space, SpaceKind::Sticky(index)));
        }
        let size = {
            let layout = self.rounded(root);
            Size2D::new(layout.size.width, layout.size.height)
        };
        let context = ContextMatrix::of(values, size, offset_in_parent, parent_perspective);
        let world = context
            .with_list(&values.get_box().transform)
            .then(parent_world);
        let own_animation = {
            let layout = self.rounded(root);
            allocate_animation_slot(
                self.document,
                self.node(root),
                Mover {
                    track: Some(TrackInputs {
                        context: &context,
                        parent_world,
                        world: &world,
                        committed: &values.get_box().transform,
                    }),
                    extent: Size2D::new(
                        layout.size.width.max(layout.content_size.width),
                        layout.size.height.max(layout.content_size.height),
                    ),
                    clip: seed.current.clip,
                },
                SlotTables {
                    spaces: &self.spaces,
                    clips: &self.clips,
                    layers: &self.layers,
                    current_layer: self.current_layer,
                    animations: &mut self.animations,
                },
            )
        };
        if let Some(index) = own_animation {
            box_space = Some(self.push_space(box_space, SpaceKind::Animation(index)));
        }
        let layer = self.open_layer(
            root,
            values,
            &world,
            size,
            (box_space, seed.current.clip),
            self.node(root).animates_opacity(),
        );

        let own_slot = self.allocate_scroll_slot(
            root,
            values,
            seed.current.chain,
            self.nearest_sticky(box_space),
            &world,
        );
        let own_scroll =
            own_slot.map(|slot| (slot, self.push_space(box_space, SpaceKind::Scroll(slot))));
        if *values.get_content_visibility() == ContentVisibility::Auto {
            let transform = intern(&mut self.matrices, world);
            self.auto_boxes.push(AutoBox {
                node: root,
                transform,
                size,
                clip: seed.current.clip,
                space: box_space,
                layer: self.current_layer,
            });
        }
        let (visible, hit_testable) = item_flags(values);
        let visible = visible && !is_inline_box(self.node(root));
        if visible {
            let transform = intern(&mut self.matrices, world);
            self.items.push(PaintItem {
                node: root,
                kind: PaintItemKind::ElementBox,
                transform,
                clip: seed.current.clip,
                size,
                radii: resolve_corner_radii(values, size),
                hit_testable,
                slot: own_slot.or(seed.current.chain),
                space: box_space,
            });
            if self.document.has_svg_roots() && self.node(root).is_replaced() {
                svg::plan_root(
                    self.document,
                    svg::Tables {
                        items: &mut self.items,
                        matrices: &mut self.matrices,
                        clips: &mut self.clips,
                        layers: &mut self.layers,
                        spaces: &mut self.spaces,
                        animations: &mut self.animations,
                        current_layer: self.current_layer,
                    },
                );
            }
        }

        self.record_initial_target(root, values, seed.current.chain);
        let mode = style.display_mode();
        if mode == DisplayMode::Leaf || skips_contents(self.node(root), values) {
            self.close_layer(layer);
            return;
        }
        let mut inner_seed = seed;
        inner_seed.current.space = box_space;
        let ctx = self.enter_element(root, values, &world, inner_seed, own_scroll);
        if crate::layout::paragraph::establishes_paragraph(mode) && visible {
            self.push_paragraph(root, &world, size, hit_testable, ctx);
        }

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
                is_item_container: ranks_children_as_items(mode),
                ctx: &ctx,
            },
            StreamTarget::Context,
            &mut collection,
        );
        let member_end = self.scratch.members.len();
        let stream_end = self.scratch.stream.len();

        let child_perspective = ParentPerspective::of(values, size);
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
            let Builder {
                document,
                items,
                matrices,
                clips,
                layers,
                spaces,
                animations,
                current_layer,
                scratch,
                ..
            } = self;
            for record in &scratch.stream[stream_base..stream_end] {
                push_record(items, matrices, record, &world);
                if record.svg_root {
                    svg::plan_root(
                        *document,
                        svg::Tables {
                            items,
                            matrices,
                            clips,
                            layers,
                            spaces,
                            animations,
                            current_layer: *current_layer,
                        },
                    );
                }
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

    fn open_layer(
        &mut self,
        node: NodeId,
        values: &ComputedValues,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
        (space, clip): (Option<u32>, Option<usize>),
        force_group: bool,
    ) -> Option<usize> {
        if !force_group && !stacking::needs_group_rendering(values) {
            return None;
        }
        let start = self.items.len();
        let transform = intern(&mut self.matrices, *world);
        self.layers.push(RenderLayer {
            parent: self.current_layer,
            node,
            transform,
            size,
            radii: resolve_corner_radii(values, size),
            space,
            clip,
            items: start..start,
            kind: LayerKind::Element,
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

    fn rank_children(&mut self, node: NodeId) -> usize {
        let base = self.scratch.ranked.len();
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

        let mut position = base;
        while position < end {
            let child_slot = self.scratch.ranked[position].slot;
            position += 1;
            self.collect_child(child_slot, cursor, target, collection);
        }
        self.scratch.ranked.truncate(base);
    }

    fn resolve_child(
        &self,
        child_node: &'doc Node<T>,
        cursor: Cursor<'_>,
    ) -> Option<ChildBox<'doc, T>> {
        let view = StyleView::try_of(child_node)?;
        let mode = view.display_mode();
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
        let position = *style.get_box().get_position();
        let clips = member_clip_contexts(position, *cursor.ctx);
        let z_applies = stacking::z_index_applies(position, cursor.is_item_container);
        Some(ChildBox {
            node,
            level: stacking::establishes_stacking_context(child_node, style, z_applies)
                .then(|| stacking::stack_level(style, z_applies)),
            offset,
            size,
            clips,
            position,
            mode,
            view,
        })
    }

    fn push_paragraph(
        &mut self,
        root: NodeId,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
        hit_testable: bool,
        ctx: ClipContexts,
    ) {
        let inset = self.rounded(root);
        let glyph_origin = world.then_translate(euclid::vec3(
            inset.border.left + inset.padding.left,
            inset.border.top + inset.padding.top,
            0.0,
        ));
        let transform = intern(&mut self.matrices, glyph_origin);
        self.items.push(PaintItem {
            node: root,
            kind: PaintItemKind::TextRun { element: root },
            transform,
            clip: ctx.current.clip,
            size,
            radii: CornerRadii::ZERO,
            hit_testable,
            slot: ctx.current.chain,
            space: ctx.current.space,
        });
        for fragment in self.fragment_records(root, Point2D::zero(), ctx.current) {
            let transform = intern(
                &mut self.matrices,
                glyph_origin.then_translate(euclid::vec3(
                    fragment.offset.x,
                    fragment.offset.y,
                    0.0,
                )),
            );
            self.items.push(PaintItem {
                node: fragment.node,
                kind: fragment.kind,
                transform,
                clip: fragment.clip,
                size: fragment.size,
                radii: fragment.radii,
                hit_testable: fragment.hit_testable,
                slot: fragment.slot,
                space: fragment.space,
            });
        }
    }

    fn fragment_records(
        &self,
        root: NodeId,
        content_offset: Point2D<f32>,
        content: FlowContext,
    ) -> Vec<ItemRecord> {
        let Some(paragraph) = self.document.painted_paragraph(root) else {
            return Vec::new();
        };
        paragraph
            .fragments()
            .iter()
            .filter_map(|fragment| {
                let node = *paragraph.sources.get(fragment.item as usize)?;
                let (visible, hit_testable) = item_flags(self.document.paint_style(node)?);
                visible.then(|| ItemRecord {
                    node,
                    kind: PaintItemKind::InlineFragment { paragraph: root },
                    offset: Point2D::new(
                        content_offset.x + fragment.rect.origin.x,
                        content_offset.y + fragment.rect.origin.y,
                    ),
                    clip: content.clip,
                    size: Size2D::new(fragment.rect.size.width, fragment.rect.size.height),
                    radii: CornerRadii::ZERO,
                    hit_testable,
                    slot: content.chain,
                    space: content.space,
                    svg_root: false,
                })
            })
            .collect()
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
            return;
        }
        if self.state.slot_at(child_slot).is_hidden() {
            return;
        }
        if !child_node.is_element() {
            return;
        }
        let top_layer = self.tree.top_layer();
        if !top_layer.is_empty() && top_layer.contains(child_node.id()) {
            return;
        }
        let Some(child) = self.resolve_child(child_node, cursor) else {
            return;
        };
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

    fn collect_in_context(
        &mut self,
        child: &ChildBox<'doc, T>,
        ctx: ClipContexts,
        target: StreamTarget,
        collection: &mut Collection<'_>,
    ) {
        let style = child.view.values();
        let (visible, hit_testable) = item_flags(style);
        let visible = visible && !is_inline_box(self.node(child.node));
        let descend =
            child.mode != DisplayMode::Leaf && !skips_contents(self.node(child.node), style);
        let is_item_container = ranks_children_as_items(child.mode);
        let pseudo = (child.position != PositionProperty::Static)
            .then(|| (collection.next_seq(), self.take_pseudo_stream()));
        let (target, mut outer) = match pseudo {
            Some((_, stream)) => (StreamTarget::Pseudo(stream), child.clips),
            None => (target, ctx),
        };
        outer.current.space = self.enter_anchored(
            child.node,
            style,
            collection.world,
            (outer.current.space, outer.current.clip),
            &mut outer.fixed,
        );

        let world = translated(collection.world, child.offset);
        let own_slot = self.allocate_scroll_slot(
            child.node,
            style,
            outer.current.chain,
            self.nearest_sticky(outer.current.space),
            &world,
        );
        let own_scroll = own_slot.map(|slot| {
            (
                slot,
                self.push_space(outer.current.space, SpaceKind::Scroll(slot)),
            )
        });
        self.record_initial_target(child.node, style, outer.current.chain);
        let inner =
            descend.then(|| self.enter_element(child.node, style, &world, outer, own_scroll));
        if visible {
            let mut record = element_record(
                child.node,
                style,
                child.offset,
                child.size,
                outer.current.clip,
                hit_testable,
                own_slot.or(outer.current.chain),
                outer.current.space,
            );
            record.svg_root = self.document.has_svg_roots() && self.node(child.node).is_replaced();
            self.push_stream(target, record);
            if crate::layout::paragraph::establishes_paragraph(child.mode) {
                let content = inner.map_or(outer.current, |inner| inner.current);
                let inset = self.rounded(child.node);
                let content_offset = Point2D::new(
                    child.offset.x + inset.border.left + inset.padding.left,
                    child.offset.y + inset.border.top + inset.padding.top,
                );
                self.push_stream(
                    target,
                    ItemRecord {
                        node: child.node,
                        kind: PaintItemKind::TextRun {
                            element: child.node,
                        },
                        offset: content_offset,
                        clip: content.clip,
                        size: child.size,
                        radii: CornerRadii::ZERO,
                        hit_testable,
                        slot: content.chain,
                        space: content.space,
                        svg_root: false,
                    },
                );
                for record in self.fragment_records(child.node, content_offset, content) {
                    self.push_stream(target, record);
                }
            }
        }
        if let Some(inner) = inner {
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
                let Builder {
                    document,
                    items,
                    matrices,
                    clips,
                    layers,
                    spaces,
                    animations,
                    current_layer,
                    scratch,
                    ..
                } = self;
                let buffer = &mut scratch.pseudo_pool[stream as usize];
                for record in buffer.iter() {
                    push_record(items, matrices, record, world);
                    if record.svg_root {
                        svg::plan_root(
                            *document,
                            svg::Tables {
                                items,
                                matrices,
                                clips,
                                layers,
                                spaces,
                                animations,
                                current_layer: *current_layer,
                            },
                        );
                    }
                }
                buffer.clear();
                scratch.pseudo_free.push(stream);
            }
        }
    }

    fn record_initial_target(&mut self, node: NodeId, style: &ComputedValues, chain: Option<u32>) {
        if *style.get_scroll_initial_target() == scroll_initial_target::T::Nearest
            && let Some(chain) = chain
        {
            self.initial_targets.push(InitialTarget { chain, node });
        }
    }

    fn enter_element(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        transform: &Transform3D<f32>,
        ctx: ClipContexts,
        own_scroll: Option<(u32, u32)>,
    ) -> ClipContexts {
        let mut inner = ctx;
        let clipped = clipped_axes(self.node(node), style);
        if clipped.x || clipped.y {
            let (rect, radii) = {
                let layout = self.rounded(node);
                let rect = unclipped_axes_unbounded(padding_box(layout), clipped);
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
            let transform = intern(&mut self.matrices, *transform);
            self.clips.push(ClipNode {
                parent: inner.current.clip,
                #[cfg(test)]
                node,
                transform,
                rect,
                radii,
                space: inner.current.space,
            });
            inner.current.clip = Some(self.clips.len() - 1);
        }
        if let Some((slot, space)) = own_scroll {
            inner.current.chain = Some(slot);
            inner.current.space = Some(space);
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
}

fn rounded_at(state: &DocumentLayoutState, slot: NodeSlot) -> &Layout {
    &state.slot_at(slot).rounded
}

fn push_record(
    items: &mut Vec<PaintItem>,
    matrices: &mut Vec<Transform3D<f32>>,
    record: &ItemRecord,
    world: &Transform3D<f32>,
) {
    items.push(PaintItem {
        node: record.node,
        kind: record.kind,
        transform: intern(matrices, translated(world, record.offset)),
        clip: record.clip,
        size: record.size,
        radii: record.radii,
        hit_testable: record.hit_testable,
        slot: record.slot,
        space: record.space,
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
    space: Option<u32>,
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
        space,
        svg_root: false,
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

pub(super) fn clipped_axes<T>(node: &Node<T>, style: &ComputedValues) -> ScrollAxes {
    if is_inline_box(node) {
        return ScrollAxes { x: false, y: false };
    }
    if effective_containment(
        *style.get_contain(),
        *style.get_content_visibility(),
        skips_contents(node, style),
        *style.get_container_type(),
    )
    .intersects(Contain::PAINT)
    {
        return ScrollAxes::BOTH;
    }
    ScrollAxes {
        x: !matches!(*style.get_overflow_x(), Overflow::Visible),
        y: !matches!(*style.get_overflow_y(), Overflow::Visible),
    }
}

pub(super) fn padding_box(layout: &Layout) -> Rect<f32> {
    Rect::new(
        Point2D::new(layout.border.left, layout.border.top),
        Size2D::new(
            (layout.size.width - layout.border.horizontal_sum()).max(0.0),
            (layout.size.height - layout.border.vertical_sum()).max(0.0),
        ),
    )
}

pub(super) fn unclipped_axes_unbounded(rect: Rect<f32>, clipped: ScrollAxes) -> Rect<f32> {
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
    let visible = matches!(*style.get_visibility(), visibility::T::Visible);
    let hit_testable = visible && !matches!(*style.get_pointer_events(), PointerEvents::None);
    (visible, hit_testable)
}

fn translated(world: &Transform3D<f32>, offset: Point2D<f32>) -> Transform3D<f32> {
    Transform3D::translation(offset.x, offset.y, 0.0).then(world)
}

pub(super) fn intern(
    matrices: &mut Vec<Transform3D<f32>>,
    matrix: Transform3D<f32>,
) -> ItemTransform {
    if let Some(offset) = as_translation(&matrix) {
        return ItemTransform::Translate(offset);
    }
    if !matrices.last().is_some_and(|last| same_bits(last, &matrix)) {
        matrices.push(matrix);
    }
    ItemTransform::Matrix(
        u32::try_from(matrices.len() - 1).expect("a frame cannot hold 2^32 matrices"),
    )
}

pub(super) struct TrackInputs<'a> {
    pub(super) context: &'a ContextMatrix,
    pub(super) parent_world: &'a Transform3D<f32>,
    pub(super) world: &'a Transform3D<f32>,
    pub(super) committed: &'a ComputedTransform,
}

pub(super) struct Mover<'a> {
    pub(super) track: Option<TrackInputs<'a>>,
    pub(super) extent: Size2D<f32>,
    pub(super) clip: Option<usize>,
}

pub(super) struct SlotTables<'a> {
    pub(super) spaces: &'a [Space],
    pub(super) clips: &'a [ClipNode],
    pub(super) layers: &'a [RenderLayer],
    pub(super) current_layer: Option<usize>,
    pub(super) animations: &'a mut Vec<AnimationSlot>,
}

pub(super) fn allocate_animation_slot<T: Sync>(
    document: &Document<T>,
    node: &Node<T>,
    mover: Mover<'_>,
    tables: SlotTables<'_>,
) -> Option<u32> {
    let SlotTables {
        spaces,
        clips,
        layers,
        current_layer,
        animations,
    } = tables;
    if !node.animates_opacity() && !node.animates_transform() {
        return None;
    }
    let ExportedComposite {
        mut curve,
        opacity,
        transform,
    } = document.composite_export(node)?;
    debug_assert!(!opacity || node.animates_opacity());
    debug_assert!(!transform || node.animates_transform());
    if transform {
        let inputs = mover.track?;
        let enclosing = current_layer.map(|layer| layers[layer].space);
        let bounded = enclosing.is_none_or(|group| {
            super::space::movers_bounded(spaces, clips, animations, mover.clip, group)
        });
        if !bounded || !moving_extent_fits(document, mover.extent) {
            return None;
        }
        let track = TransformTrack::new(
            &curve,
            inputs.context.clone(),
            *inputs.parent_world,
            inputs.world,
            inputs.committed,
        )?;
        if enclosing.is_some() && !track.reach.is_bounded() {
            return None;
        }
        curve.transform = Some(track);
    }
    animations.push(AnimationSlot {
        node: node.id(),
        curve,
    });
    Some(u32::try_from(animations.len() - 1).expect("a frame cannot hold 2^32 animation slots"))
}

fn moving_extent_fits<T>(document: &Document<T>, extent: Size2D<f32>) -> bool {
    let viewport = document.viewport_size();
    extent.width * extent.height <= MAX_MOVING_EXTENT_VIEWPORTS * viewport.width * viewport.height
}
