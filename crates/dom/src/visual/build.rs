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

use euclid::default::{Point2D, Rect, Size2D, Transform3D, Vector2D};
use hughie::style::containment::effective_containment;
use hughie::style::{Contain, CoreStyle, Overflow, PositionProperty, visibility};
use hughie::tree::{Layout, LayoutTree};
use stylo::properties::ComputedValues;
use stylo::values::computed::PointerEvents;

use super::geometry::{inner_radii, resolve_corner_radii};
use super::transform::{ParentPerspective, stacking_context_matrix};
use super::{ClipNode, CornerRadii, PaintItem, PaintItemKind, PaintOrder, RenderLayer, stacking};
use crate::layout::{
    DisplayMode, StyleView, box_parent, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, skips_contents,
};
use crate::scroll::ScrollAxes;
use crate::tree::document::{Document, DocumentLayoutState, TreeArenas};
use crate::tree::node::Node;
use crate::{NodeId, scroll};

pub(crate) fn build<T>(document: &Document<T>) -> PaintOrder {
    let scale = document.device().device_pixel_ratio().get();
    let (tree, state) = document.visual_parts();
    let mut builder = Builder {
        tree,
        state,
        scale,
        items: Vec::new(),
        clips: Vec::new(),
        layers: Vec::new(),
        current_layer: None,
    };
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
    PaintOrder {
        items: builder.items,
        clips: builder.clips,
        layers: builder.layers,
        visual_epoch,
    }
}

/// What a box inherits from its containing-block chain: the innermost clip,
/// and the translation the scroll containers along that chain have applied to
/// their contents.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct FlowContext {
    clip: Option<usize>,
    scroll: Vector2D<f32>,
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
struct ItemRecord {
    node: NodeId,
    kind: PaintItemKind,
    offset: Point2D<f32>,
    clip: Option<usize>,
    size: Size2D<f32>,
    radii: CornerRadii,
    hit_testable: bool,
}

struct Member {
    level: i32,
    seq: u32,
    payload: MemberPayload,
}

enum MemberPayload {
    Context {
        node: NodeId,
        offset: Point2D<f32>,
        clips: ClipContexts,
    },
    Pseudo {
        stream: Vec<ItemRecord>,
    },
}

struct Builder<'doc, T> {
    tree: &'doc TreeArenas<T>,
    state: &'doc DocumentLayoutState,
    scale: f32,
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
    layers: Vec<RenderLayer>,
    current_layer: Option<usize>,
}

impl<'doc, T> Builder<'doc, T> {
    fn node(&self, id: NodeId) -> &'doc Node<T> {
        self.tree.live(id)
    }

    fn rounded(&self, id: NodeId) -> &'doc Layout {
        &self.state.at(self.tree.live_slot(id)).slot.rounded
    }

    fn scroll_translation(&self, id: NodeId, style: &ComputedValues) -> Vector2D<f32> {
        let state = self.state.at(self.tree.live_slot(id));
        let Some(scroll_box) = scroll::resolve(style, &state.slot.rounded, state.scroll_offset)
        else {
            return Vector2D::zero();
        };
        Vector2D::new(
            -(scroll_box.offset.x * self.scale).round() / self.scale,
            -(scroll_box.offset.y * self.scale).round() / self.scale,
        )
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
        let layer = self.open_layer(root, values, &world, size);

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
            });
        }

        let mode = display_mode(style.display());
        if mode == DisplayMode::Leaf || skips_contents(values) {
            self.close_layer(layer);
            return;
        }
        let ctx = self.enter_element(root, values, &world, seed);

        let mut members = Vec::new();
        let mut stream = Vec::new();
        let mut seq = 0_u32;
        self.collect(
            root,
            (ctx.current.scroll - seed.current.scroll).to_point(),
            matches!(mode, DisplayMode::Flex | DisplayMode::Grid),
            ctx,
            &world,
            &mut members,
            &mut stream,
            &mut seq,
        );

        let child_perspective = ParentPerspective::of(values, size);
        members.sort_unstable_by_key(|member| (member.level, member.seq));
        let zero_and_above = members.split_off(members.partition_point(|member| member.level < 0));
        for member in members {
            self.emit_member(member, root, child_perspective, &world);
        }
        for record in stream {
            self.push_record(&record, &world);
        }
        for member in zero_and_above {
            self.emit_member(member, root, child_perspective, &world);
        }
        self.close_layer(layer);
    }

    fn open_layer(
        &mut self,
        node: NodeId,
        values: &ComputedValues,
        world: &Transform3D<f32>,
        size: Size2D<f32>,
    ) -> Option<usize> {
        if !stacking::needs_group_rendering(values) {
            return None;
        }
        let start = self.items.len();
        self.layers.push(RenderLayer {
            parent: self.current_layer,
            node,
            transform: *world,
            size,
            radii: resolve_corner_radii(values, size),
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

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn collect(
        &mut self,
        node: NodeId,
        node_offset: Point2D<f32>,
        node_is_item_container: bool,
        ctx: ClipContexts,
        world: &Transform3D<f32>,
        members: &mut Vec<Member>,
        stream: &mut Vec<ItemRecord>,
        seq: &mut u32,
    ) {
        let mut children: Vec<(u32, usize, NodeId)> = self
            .tree
            .flattened_children(node)
            .enumerate()
            .map(|(index, (child, _, _))| (self.rounded(child).order, index, child))
            .collect();
        children.sort_unstable_by_key(|&(order, index, _)| (order, index));

        for (_, _, child) in children {
            let child_node = self.node(child);
            if child_node.is_text_node() {
                if let Some(record) = self.text_record(child_node, node_offset, ctx) {
                    stream.push(record);
                }
                continue;
            }
            if !child_node.is_element() {
                continue;
            }
            let Some(view) = StyleView::try_of(child_node) else {
                continue;
            };
            let mode = display_mode(view.display());
            if mode == DisplayMode::None {
                continue;
            }
            debug_assert_ne!(
                mode,
                DisplayMode::Contents,
                "flattened_children never yields a box-less element",
            );
            let style = view.values();
            let (mut child_offset, size) = {
                let layout = self.rounded(child);
                (
                    Point2D::new(
                        node_offset.x + layout.location.x,
                        node_offset.y + layout.location.y,
                    ),
                    Size2D::new(layout.size.width, layout.size.height),
                )
            };
            let position = style.clone_position();
            let captured = member_clip_contexts(position, ctx);
            child_offset += captured.current.scroll - ctx.current.scroll;
            let z_applies = stacking::z_index_applies(position, node_is_item_container);

            if stacking::establishes_stacking_context(style, z_applies) {
                members.push(Member {
                    level: stacking::stack_level(style, z_applies),
                    seq: next(seq),
                    payload: MemberPayload::Context {
                        node: child,
                        offset: child_offset,
                        clips: captured,
                    },
                });
                continue;
            }

            let descend = mode != DisplayMode::Leaf && !skips_contents(style);
            let (visible, hit_testable) = item_flags(style);
            let is_item_container = matches!(mode, DisplayMode::Flex | DisplayMode::Grid);

            if position != PositionProperty::Static {
                let member_seq = next(seq);
                let mut pseudo_stream = Vec::new();
                if visible {
                    pseudo_stream.push(element_record(
                        child,
                        style,
                        child_offset,
                        size,
                        captured.current.clip,
                        hit_testable,
                    ));
                }
                if descend {
                    let inner = self.enter_element(
                        child,
                        style,
                        &translated(world, child_offset),
                        captured,
                    );
                    self.collect(
                        child,
                        child_offset + (inner.current.scroll - captured.current.scroll),
                        is_item_container,
                        inner,
                        world,
                        members,
                        &mut pseudo_stream,
                        seq,
                    );
                }
                members.push(Member {
                    level: 0,
                    seq: member_seq,
                    payload: MemberPayload::Pseudo {
                        stream: pseudo_stream,
                    },
                });
                continue;
            }

            if visible {
                stream.push(element_record(
                    child,
                    style,
                    child_offset,
                    size,
                    ctx.current.clip,
                    hit_testable,
                ));
            }
            if descend {
                let inner = self.enter_element(child, style, &translated(world, child_offset), ctx);
                self.collect(
                    child,
                    child_offset + (inner.current.scroll - ctx.current.scroll),
                    is_item_container,
                    inner,
                    world,
                    members,
                    stream,
                    seq,
                );
            }
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
                for record in stream {
                    self.push_record(&record, world);
                }
            }
        }
    }

    fn enter_element(
        &mut self,
        node: NodeId,
        style: &ComputedValues,
        transform: &Transform3D<f32>,
        ctx: ClipContexts,
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
            });
            inner.current.clip = Some(self.clips.len() - 1);
        }
        inner.current.scroll += self.scroll_translation(node, style);
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
        })
    }

    fn push_record(&mut self, record: &ItemRecord, world: &Transform3D<f32>) {
        self.items.push(PaintItem {
            node: record.node,
            kind: record.kind,
            transform: translated(world, record.offset),
            clip: record.clip,
            size: record.size,
            radii: record.radii,
            hit_testable: record.hit_testable,
        });
    }
}

fn element_record(
    node: NodeId,
    style: &ComputedValues,
    offset: Point2D<f32>,
    size: Size2D<f32>,
    clip: Option<usize>,
    hit_testable: bool,
) -> ItemRecord {
    ItemRecord {
        node,
        kind: PaintItemKind::ElementBox,
        offset,
        clip,
        size,
        radii: resolve_corner_radii(style, size),
        hit_testable,
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

fn next(seq: &mut u32) -> u32 {
    let current = *seq;
    *seq += 1;
    current
}
