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
//! monotone counter over the collection walk, which iterates children by
//! `(Layout::order, dom index)` at every level, so tree-order tiebreaks are
//! order-modified document order throughout.
//!
//! Clip state is tracked as three chains: the current one plus the chains as
//! seen from the nearest absolute- and fixed-containing-block ancestors.
//! A member keyed by **computed** position swaps the appropriate chain in as
//! its own — that is precisely the containing-block clip-escape rule
//! (CSS2 §11.1.1): boxes are only clipped by ancestors in their
//! containing-block chain.

use euclid::default::{Point2D, Rect, Size2D, Transform3D};
use neutron_star::style::{Contain, CoreStyle, Overflow, PositionProperty, visibility};
use stylo::properties::ComputedValues;
use stylo::values::computed::PointerEvents;

use super::geometry::{inner_radii, resolve_corner_radii};
use super::transform::{ParentPerspective, stacking_context_matrix};
use super::{ClipNode, CornerRadii, PaintItem, PaintItemKind, PaintOrder, stacking};
use crate::NodeId;
use crate::contain::effective_containment;
use crate::document::Document;
use crate::layout::{
    DisplayMode, StyleView, box_tree_children, box_tree_parent, display_mode,
    establishes_absolute_containing_block, establishes_fixed_containing_block, skips_contents,
};
use crate::node::Node;

pub(crate) fn build<T>(document: &Document<T>) -> PaintOrder {
    let mut builder = Builder {
        items: Vec::new(),
        clips: Vec::new(),
    };
    if let Some(root) = document.root_element()
        && let Some(style) = StyleView::try_of(root)
        && display_mode(style.display()) != DisplayMode::None
    {
        let location = root.rounded_layout().location;
        builder.build_stacking_context(
            root,
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
    }
}

/// The clip chains visible at one point of the walk: the in-flow chain plus
/// the chains captured at the nearest absolute-/fixed-containing-block
/// ancestors (what an escaping positioned descendant swaps in).
#[derive(Debug, Clone, Copy, Default)]
struct ClipContexts {
    current: Option<usize>,
    absolute: Option<usize>,
    fixed: Option<usize>,
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

struct Member<'doc, T> {
    level: i32,
    seq: u32,
    node: &'doc Node<T>,
    offset: Point2D<f32>,
    clips: ClipContexts,
    payload: MemberPayload,
}

enum MemberPayload {
    /// A real stacking context: recursed atomically at emission.
    Context,
    /// A pseudo-stacking context: its own item plus its in-flow content,
    /// already collected.
    Pseudo { stream: Vec<ItemRecord> },
}

struct Builder {
    items: Vec<PaintItem>,
    clips: Vec<ClipNode>,
}

impl Builder {
    fn build_stacking_context<'doc, T>(
        &mut self,
        root: &'doc Node<T>,
        style: &StyleView<'doc, T>,
        offset_in_parent: Point2D<f32>,
        parent_world: &Transform3D<f32>,
        parent_perspective: Option<ParentPerspective>,
        seed: ClipContexts,
    ) {
        let values = style.values();
        let size = {
            let layout = root.rounded_layout();
            Size2D::new(layout.size.width, layout.size.height)
        };
        let world = stacking_context_matrix(values, size, offset_in_parent, parent_perspective)
            .then(parent_world);

        let (visible, hit_testable) = item_flags(values);
        if visible {
            self.items.push(PaintItem {
                node: root.id(),
                kind: PaintItemKind::ElementBox,
                transform: world,
                clip: seed.current,
                size,
                radii: resolve_corner_radii(values, size),
                hit_testable,
            });
        }

        let mode = display_mode(style.display());
        if mode == DisplayMode::Leaf || skips_contents(values) {
            return;
        }
        let ctx = self.enter_element(root, values, &world, seed);

        let mut members = Vec::new();
        let mut stream = Vec::new();
        let mut seq = 0_u32;
        self.collect(
            root,
            Point2D::zero(),
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
            self.emit_member(member, root.id(), child_perspective, &world);
        }
        for record in stream {
            self.push_record(&record, &world);
        }
        for member in zero_and_above {
            self.emit_member(member, root.id(), child_perspective, &world);
        }
    }

    /// One pre-order pass under `node` (children by `(Layout::order, dom
    /// index)`): buffers in-flow content into `stream`, surfaces positioned
    /// boxes and stacking contexts into `members`, and threads the clip
    /// contexts down with member-site escape resolution.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn collect<'doc, T>(
        &mut self,
        node: &'doc Node<T>,
        node_offset: Point2D<f32>,
        node_is_item_container: bool,
        ctx: ClipContexts,
        world: &Transform3D<f32>,
        members: &mut Vec<Member<'doc, T>>,
        stream: &mut Vec<ItemRecord>,
        seq: &mut u32,
    ) {
        // Box-tree children: display:contents levels dissolve here, so
        // dissolved grandchildren interleave with direct children in one
        // (order, index) space — the same space the layout algorithms
        // assigned their Layout.order ranks in.
        let mut children: Vec<(u32, usize, &'doc Node<T>)> = box_tree_children(node)
            .enumerate()
            .map(|(index, child)| (child.rounded_layout().order, index, child))
            .collect();
        children.sort_unstable_by_key(|&(order, index, _)| (order, index));

        for (_, _, child) in children {
            if child.is_text_node() {
                if let Some(record) = text_record(child, node_offset, ctx) {
                    stream.push(record);
                }
                continue;
            }
            if !child.is_element() {
                continue;
            }
            let Some(view) = StyleView::try_of(child) else {
                // Descendant of a display:none root: stylo cleared its data
                // and layout zeroed it.
                continue;
            };
            let mode = display_mode(view.display());
            if mode == DisplayMode::None {
                continue;
            }
            if mode == DisplayMode::Contents {
                debug_assert!(
                    false,
                    "box_tree_children dissolves display:contents elements"
                );
                continue;
            }
            let style = view.values();
            let (child_offset, size) = {
                let layout = child.rounded_layout();
                (
                    Point2D::new(
                        node_offset.x + layout.location.x,
                        node_offset.y + layout.location.y,
                    ),
                    Size2D::new(layout.size.width, layout.size.height),
                )
            };
            let position = style.clone_position();
            let z_applies = stacking::z_index_applies(position, node_is_item_container);

            if stacking::establishes_stacking_context(style, z_applies) {
                members.push(Member {
                    level: stacking::stack_level(style, z_applies),
                    seq: next(seq),
                    node: child,
                    offset: child_offset,
                    clips: member_clip_contexts(position, ctx),
                    payload: MemberPayload::Context,
                });
                continue;
            }

            let descend = mode != DisplayMode::Leaf && !skips_contents(style);
            let (visible, hit_testable) = item_flags(style);
            let is_item_container = matches!(mode, DisplayMode::Flex | DisplayMode::Grid);

            if position != PositionProperty::Static {
                // Pseudo-stacking context (computed `relative` or `absolute`
                // here — `fixed` and `sticky` always form real contexts).
                let captured = member_clip_contexts(position, ctx);
                let member_seq = next(seq);
                let mut pseudo_stream = Vec::new();
                if visible {
                    pseudo_stream.push(element_record(
                        child.id(),
                        style,
                        child_offset,
                        size,
                        captured.current,
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
                        child_offset,
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
                    node: child,
                    offset: child_offset,
                    clips: captured,
                    payload: MemberPayload::Pseudo {
                        stream: pseudo_stream,
                    },
                });
                continue;
            }

            // In-flow content.
            if visible {
                stream.push(element_record(
                    child.id(),
                    style,
                    child_offset,
                    size,
                    ctx.current,
                    hit_testable,
                ));
            }
            if descend {
                let inner = self.enter_element(child, style, &translated(world, child_offset), ctx);
                self.collect(
                    child,
                    child_offset,
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

    fn emit_member<T>(
        &mut self,
        member: Member<'_, T>,
        context_root: NodeId,
        context_perspective: Option<ParentPerspective>,
        world: &Transform3D<f32>,
    ) {
        match member.payload {
            MemberPayload::Context => {
                let style = StyleView::try_of(member.node)
                    .expect("stacking members keep their style for the whole build");
                // The perspective property applies to direct box-tree
                // children only (display:contents levels dissolve; any
                // deeper element carrying perspective is itself the nearer
                // stacking context).
                let perspective = box_tree_parent(member.node)
                    .is_some_and(|parent| parent.id() == context_root)
                    .then_some(context_perspective)
                    .flatten();
                self.build_stacking_context(
                    member.node,
                    &style,
                    member.offset,
                    world,
                    perspective,
                    member.clips,
                );
            }
            MemberPayload::Pseudo { stream } => {
                for record in stream {
                    self.push_record(&record, world);
                }
            }
        }
    }

    /// Applies an element's own clip and containing-block captures to the
    /// contexts its children see.
    fn enter_element<T>(
        &mut self,
        node: &Node<T>,
        style: &ComputedValues,
        transform: &Transform3D<f32>,
        ctx: ClipContexts,
    ) -> ClipContexts {
        let mut inner = ctx;
        if is_clipping(style) {
            let (rect, radii) = {
                let layout = node.rounded_layout();
                let rect = Rect::new(
                    Point2D::new(layout.border.left, layout.border.top),
                    Size2D::new(
                        (layout.size.width - layout.border.horizontal_sum()).max(0.0),
                        (layout.size.height - layout.border.vertical_sum()).max(0.0),
                    ),
                );
                let outer =
                    resolve_corner_radii(style, Size2D::new(layout.size.width, layout.size.height));
                (rect, inner_radii(outer, &layout.border))
            };
            self.clips.push(ClipNode {
                parent: inner.current,
                node: node.id(),
                transform: *transform,
                rect,
                radii,
            });
            inner.current = Some(self.clips.len() - 1);
        }
        if establishes_absolute_containing_block(node, style) {
            inner.absolute = inner.current;
        }
        if establishes_fixed_containing_block(node, style) {
            inner.fixed = inner.current;
        }
        inner
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

/// A text leaf's run record; `None` when the DOM parent is invisible (text
/// has no style of its own — it inherits from its parent, which under
/// dissolution may be a display:contents element rather than the box the
/// walk is iterating). The hit target is that DOM parent even when boxless:
/// the singular text-hit rule (matching Chrome's elementFromPoint for text
/// inside display:contents) chosen deliberately over Chrome's
/// self-inconsistent plural API.
fn text_record<T>(
    child: &Node<T>,
    node_offset: Point2D<f32>,
    ctx: ClipContexts,
) -> Option<ItemRecord> {
    let parent = child.parent().filter(|parent| parent.is_element())?;
    let view = StyleView::try_of(parent)?;
    let (visible, hit_testable) = item_flags(view.values());
    if !visible {
        return None;
    }
    let layout = child.rounded_layout();
    Some(ItemRecord {
        node: child.id(),
        kind: PaintItemKind::TextRun {
            element: parent.id(),
        },
        offset: Point2D::new(
            node_offset.x + layout.location.x,
            node_offset.y + layout.location.y,
        ),
        clip: ctx.current,
        size: Size2D::new(layout.size.width, layout.size.height),
        radii: CornerRadii::ZERO,
        hit_testable,
    })
}

/// The clip contexts a member adopts, keyed by **computed** position: an
/// absolute box is clipped as seen from its absolute containing block, a
/// fixed box from its fixed one; relative and sticky boxes stay in the
/// normal flow of clips.
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

fn is_clipping(style: &ComputedValues) -> bool {
    !matches!(style.clone_overflow_x(), Overflow::Visible)
        || !matches!(style.clone_overflow_y(), Overflow::Visible)
        || effective_containment(style, skips_contents(style)).intersects(Contain::PAINT)
}

fn item_flags(style: &ComputedValues) -> (bool, bool) {
    let visible = matches!(style.clone_visibility(), visibility::T::Visible);
    // Every value other than `none` behaves as `auto` on non-SVG content
    // (SVG2 pointer-events); the fork's enum is `Auto | None` today.
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
