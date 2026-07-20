//! The layout **handle**: [`LayoutNode`] implemented on a two-word `Copy`
//! value, plus the pass pipeline built on it.
//!
//! [`LayoutHandle`] pairs a `&Node` with the pass-scoped [`LayoutContext`]
//! (materialized styles, the leaf measurement hook, the hoisted out-of-flow
//! queue). Topology and styles are read straight off the node/context;
//! layout writes go through the node's interior-mutable
//! [`LayoutData`](super::LayoutData) in short scoped borrows — never held
//! across a recursive [`compute_child_layout`](LayoutNode::compute_child_layout),
//! per the protocol's re-entrancy contract.

use std::cell::RefCell;

use neutron_star::compute::{
    FnLeafMeasurer, LeafMeasureInput, LeafMetrics, compute_absolute_layout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout,
    compute_relative_layout, compute_root_layout, hide_subtree, round_layout,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::style::PositionProperty;
use neutron_star::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput,
};
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

use super::style::{
    DisplayMode, StyleView, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block,
};
use crate::document::Document;
use crate::node::{ChildrenIter, Node};

/// The embedder's leaf content measurement hook (see
/// [`StyleEngine::layout_document_with_measurer`](crate::StyleEngine::layout_document_with_measurer)).
pub trait MeasureLeaf<T> {
    /// Measure one leaf's content under the given content-box constraints.
    fn measure(&mut self, node: &Node<T>, input: LeafMeasureInput) -> LeafMetrics;
}

impl<T, F: FnMut(&Node<T>, LeafMeasureInput) -> LeafMetrics> MeasureLeaf<T> for F {
    fn measure(&mut self, node: &Node<T>, input: LeafMeasureInput) -> LeafMetrics {
        self(node, input)
    }
}

/// One layout pass's shared context: everything a handle can reach besides
/// the node itself.
pub(super) struct LayoutContext<'dom, T, M> {
    document: &'dom Document<T>,
    /// Element styles by slab index, gathered once at the start of the pass
    /// (one `Arc` clone per node) so style views can lend `ComputedValues`
    /// references for the whole pass. `None` for text nodes.
    styles: Vec<Option<Arc<ComputedValues>>>,
    /// The fork's initial values, lent to text nodes as their anonymous-box
    /// style (see [`super::anonymous_style`]).
    anonymous: Arc<ComputedValues>,
    /// The embedder leaf measurement hook. Borrowed per `measure` call, so
    /// no borrow is held across engine recursion.
    measure: RefCell<M>,
    /// This pass's hoisted out-of-flow queue (FIFO; see the positioned pass).
    hoisted: RefCell<Vec<crate::NodeId>>,
    viewport: Size<f32>,
}

impl<T, M> LayoutContext<'_, T, M> {
    /// The style lent for `node`: its computed style, or the anonymous-box
    /// initial values for text nodes.
    fn style_of(&self, node: &Node<T>) -> &ComputedValues {
        self.styles
            .get(node.id())
            .and_then(Option::as_deref)
            .unwrap_or(&self.anonymous)
    }
}

/// The `Copy` node handle neutron-star traverses: a node plus its pass
/// context.
pub(super) struct LayoutHandle<'dom, T, M> {
    node: &'dom Node<T>,
    cx: &'dom LayoutContext<'dom, T, M>,
}

impl<T, M> Clone for LayoutHandle<'_, T, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, M> Copy for LayoutHandle<'_, T, M> {}

impl<T, M> std::fmt::Debug for LayoutHandle<'_, T, M> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("LayoutHandle")
            .field(&self.node.id())
            .finish()
    }
}

/// Children iterator yielding handles that share the pass context.
pub(super) struct Children<'dom, T, M> {
    inner: ChildrenIter<'dom, T>,
    cx: &'dom LayoutContext<'dom, T, M>,
}

impl<'dom, T, M> Iterator for Children<'dom, T, M> {
    type Item = LayoutHandle<'dom, T, M>;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.inner.next()?;
        Some(LayoutHandle { node, cx: self.cx })
    }
}

impl<'dom, T, M: MeasureLeaf<T>> LayoutNode for LayoutHandle<'dom, T, M> {
    type Style = StyleView<'dom, T>;
    type ChildIter = Children<'dom, T, M>;

    fn children(self) -> Self::ChildIter {
        Children {
            inner: self.node.children(),
            cx: self.cx,
        }
    }

    fn child_count(self) -> usize {
        self.node.child_ids().len()
    }

    fn style(self) -> Self::Style {
        StyleView {
            node: self.node,
            style: self.cx.style_of(self.node),
        }
    }

    /// The canonical dispatch skeleton (see `neutron_star::compute`):
    /// `display: none` is hidden **before** the cache wrapper; every
    /// generated box routes to its algorithm inside it.
    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        // Text nodes carry no computed style: they lay out as leaves inside
        // an anonymous box (initial box values; content via the measure
        // hook).
        let display = if self.node.is_text_node() {
            DisplayMode::Leaf
        } else {
            display_mode(self.cx.style_of(self.node).clone_display())
        };

        if display == DisplayMode::None {
            hide_subtree(self);
            return LayoutOutput::HIDDEN;
        }

        compute_cached_layout(self, input, |node, input| match display {
            DisplayMode::None => unreachable!("hidden nodes never reach the cache wrapper"),
            DisplayMode::Flex => compute_flexbox_layout(node, input),
            DisplayMode::Grid => compute_grid_layout(node, input),
            DisplayMode::Linear => compute_linear_layout(node, input),
            DisplayMode::Relative => compute_relative_layout(node, input),
            DisplayMode::Leaf => {
                let view = node.style();
                let output = {
                    let mut measurer = FnLeafMeasurer::new(|measure_input| {
                        node.cx
                            .measure
                            .borrow_mut()
                            .measure(node.node, measure_input)
                    });
                    compute_leaf_layout(input, &view, &mut measurer)
                };
                // Flow/contents container layout is unimplemented (see
                // `DisplayMode::Leaf`): the box itself is a leaf, and any
                // children are zeroed so stale geometry cannot survive a
                // display change. Commit only — measurement stays free of
                // durable writes.
                if input.goal == LayoutGoal::Commit {
                    for grandchild in node.children() {
                        hide_subtree(grandchild);
                    }
                }
                output
            }
        })
    }

    fn set_unrounded_layout(self, layout: &Layout) {
        self.node.layout_data.borrow_mut().unrounded = *layout;
    }

    fn unrounded_layout(self) -> Layout {
        self.node.layout_data.borrow().unrounded
    }

    fn set_final_layout(self, layout: &Layout) {
        self.node.layout_data.borrow_mut().rounded = *layout;
    }

    fn set_static_position(self, static_position: Point<f32>) {
        let mut data = self.node.layout_data.borrow_mut();
        data.static_position = static_position;
        // Queue the hoisted node for the positioned pass, once per pass.
        if !data.hoisted_recorded {
            data.hoisted_recorded = true;
            drop(data);
            self.cx.hoisted.borrow_mut().push(self.node.id());
        }
    }

    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.node.layout_data.borrow().measure_cache.get(input)
    }

    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.node
            .layout_data
            .borrow_mut()
            .measure_cache
            .store(input, output);
    }

    fn cache_clear(self) {
        self.node.layout_data.borrow_mut().measure_cache.clear();
    }
}

/// Run the full layout pipeline over a flushed document: in-flow root pass →
/// positioned pass for hoisted out-of-flow nodes → device-pixel rounding.
///
/// Takes the document as a shared borrow — the caller's `&mut Document`
/// (relinquished for the duration) is what guarantees the immutable pass;
/// all writes go through the nodes' `layout_data` cells.
pub(super) fn run_layout<T, M: MeasureLeaf<T>>(
    document: &Document<T>,
    measure: M,
    viewport: Size<f32>,
    scale: f32,
) {
    let Some(root) = document.root_element() else {
        return;
    };

    let mut styles = Vec::new();
    collect_styles(root, &mut styles);
    let cx = LayoutContext {
        document,
        styles,
        anonymous: super::anonymous_style(),
        measure: RefCell::new(measure),
        hoisted: RefCell::new(Vec::new()),
        viewport,
    };
    let root_handle = LayoutHandle {
        node: root,
        cx: &cx,
    };

    compute_root_layout(
        root_handle,
        Size::new(
            AvailableSpace::Definite(viewport.width),
            AvailableSpace::Definite(viewport.height),
        ),
    );
    run_positioned_pass(&cx);
    round_layout(root_handle, scale);
}

/// Materialize the layout root's subtree styles into the pass context —
/// one `Arc` clone per node, so style views can lend `ComputedValues`
/// references for the whole pass (the engine's materialize-once-and-lend
/// host pattern).
fn collect_styles<T>(node: &Node<T>, styles: &mut Vec<Option<Arc<ComputedValues>>>) {
    let id = node.id();
    if id >= styles.len() {
        styles.resize_with(id + 1, || None);
    }
    let style = if node.is_text_node() {
        None
    } else {
        // `display: none` descendants legitimately have no style data —
        // stylo prunes the hidden subtree from the restyle traversal and
        // drops what it had. Such nodes are only ever visited by
        // `hide_subtree`, which reads no styles, so the subtree is skipped
        // below and the anonymous fallback covers any stray read.
        node.computed_style()
    };
    let hidden = style
        .as_deref()
        .is_some_and(|style| display_mode(style.clone_display()) == DisplayMode::None);
    styles[id] = style;
    if hidden {
        return;
    }
    for child in node.children() {
        collect_styles(child, styles);
    }
}

// --- the positioned pass ------------------------------------------------------

/// Complete every hoisted out-of-flow node recorded during in-flow layout.
///
/// FIFO over the pass's queue: a hoisted node nested inside another hoisted
/// subtree is recorded while its formatting parent commits during the outer
/// node's `compute_absolute_layout`, i.e. strictly after its containing
/// block's own layout is stored — so each dequeued node can convert its
/// static position through already-final ancestor geometry.
fn run_positioned_pass<T, M: MeasureLeaf<T>>(cx: &LayoutContext<'_, T, M>) {
    let mut index = 0;
    loop {
        let id = {
            let queue = cx.hoisted.borrow();
            let Some(&id) = queue.get(index) else {
                break;
            };
            id
        };
        index += 1;
        position_hoisted(cx, id);
    }
}

fn position_hoisted<T, M: MeasureLeaf<T>>(cx: &LayoutContext<'_, T, M>, id: crate::NodeId) {
    let node = cx
        .document
        .get(id)
        .expect("hoisted node ids stay live for the whole layout pass");
    // Reset the dedupe flag as the node is dequeued, so the next pass can
    // queue it again.
    node.layout_data.borrow_mut().hoisted_recorded = false;
    let Some(parent) = node.parent() else {
        return; // the root is never hoisted; a detached node cannot be queued
    };
    // The *computed* position picks the containing-block rule; the style
    // view's scheme override already decided this node is hoisted.
    let fixed = cx.style_of(node).clone_position() == PositionProperty::Fixed;

    // Resolve the containing block: the nearest qualifying element ancestor,
    // else the viewport (the initial containing block).
    let mut containing = None;
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        if !current.is_element() {
            break; // reached the document node
        }
        let establishes = if fixed {
            establishes_fixed_containing_block(cx.style_of(current))
        } else {
            establishes_absolute_containing_block(cx.style_of(current))
        };
        if establishes {
            containing = Some(current);
            break;
        }
        ancestor = current.parent();
    }

    // Document-space border-box origin of a node (unrounded).
    let absolute_origin = |node: &Node<T>| -> Point<f32> {
        let mut origin = Point::ZERO;
        let mut current = Some(node);
        while let Some(step) = current {
            let location = step.layout_data.borrow().unrounded.location;
            origin.x += location.x;
            origin.y += location.y;
            current = step.parent();
        }
        origin
    };

    // The containing block's padding box (the abs-pos resolution basis).
    let (containing_origin, containing_size) = match containing {
        Some(block) => {
            let layout = block.layout_data.borrow().unrounded;
            let origin = absolute_origin(block);
            (
                Point::new(origin.x + layout.border.left, origin.y + layout.border.top),
                Size::new(
                    (layout.size.width - layout.border.horizontal_sum()).max(0.0),
                    (layout.size.height - layout.border.vertical_sum()).max(0.0),
                ),
            )
        }
        None => (Point::ZERO, cx.viewport),
    };

    // Convert the recorded static position (formatting-parent border-box
    // space) into containing-block padding-box space.
    let parent_origin = absolute_origin(parent);
    let static_position = node.layout_data.borrow().static_position;
    let static_in_cb = Point::new(
        parent_origin.x + static_position.x - containing_origin.x,
        parent_origin.y + static_position.y - containing_origin.y,
    );

    let handle = LayoutHandle { node, cx };
    let mut layout = compute_absolute_layout(handle, containing_size, static_in_cb);

    // Store in the formatting parent's space, keeping `Layout::location`'s
    // parent-relative contract intact for rounding and painting, with the
    // parent's order-modified paint index.
    layout.location = Point::new(
        containing_origin.x + layout.location.x - parent_origin.x,
        containing_origin.y + layout.location.y - parent_origin.y,
    );
    layout.order = sibling_paint_order(cx, parent, id);
    handle.set_unrounded_layout(&layout);
}

/// The node's paint index among its siblings: document position after a
/// stable sort by style `order` — the same order-modified index the
/// algorithms assign to the in-flow children they place.
fn sibling_paint_order<T, M>(
    cx: &LayoutContext<'_, T, M>,
    parent: &Node<T>,
    id: crate::NodeId,
) -> u32 {
    let children = parent.child_ids();
    let order_of = |child: &crate::NodeId| {
        cx.document
            .get(*child)
            .map_or(0, |node| cx.style_of(node).get_position().order)
    };
    let mut ranks: Vec<(i32, usize)> = children
        .iter()
        .enumerate()
        .map(|(index, child)| (order_of(child), index))
        .collect();
    ranks.sort_by_key(|&(order, index)| (order, index));
    let position = ranks
        .iter()
        .position(|&(_, index)| children[index] == id)
        .expect("hoisted node is a child of its formatting parent");
    u32::try_from(position).unwrap_or(u32::MAX)
}
