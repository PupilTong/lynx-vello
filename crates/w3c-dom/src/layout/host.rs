//! [`LayoutNode`] implemented **directly on `&Node<T>`**, plus the pass
//! pipeline built on it.
//!
//! The layout handle is the crate's one read handle — the same one-word
//! `Copy` `&Node` the stylo `TNode`/`TElement` traits use — so the engine
//! traverses the document itself. Topology comes off the node's child list
//! (the existing [`ChildrenIter`] already yields `&Node` and *is* the
//! protocol's child iterator); styles are fetched from the node when the
//! engine asks ([`StyleView::of`]); layout writes go through the node's
//! interior-mutable [`LayoutData`](super::LayoutData) in short scoped
//! borrows — never held across a recursive
//! [`compute_child_layout`](LayoutNode::compute_child_layout), per the
//! protocol's re-entrancy contract. The pass's only shared state — the
//! hoisted out-of-flow queue — lives in the **document node**'s
//! `LayoutData`, reachable from any handle through the slab backpointer.

use neutron_star::compute::{
    FnLeafMeasurer, compute_absolute_layout, compute_cached_layout, compute_flexbox_layout,
    compute_grid_layout, compute_leaf_layout, compute_linear_layout, compute_relative_layout,
    compute_root_layout, hide_subtree, round_layout,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::style::PositionProperty;
use neutron_star::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput,
};

use super::MeasureLeaf;
use super::style::{
    DisplayMode, StyleView, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block,
};
use crate::document::Document;
use crate::node::{ChildrenIter, Node};

impl<'dom, T: MeasureLeaf> LayoutNode for &'dom Node<T> {
    type Style = StyleView<'dom, T>;
    type ChildIter = ChildrenIter<'dom, T>;

    fn children(self) -> Self::ChildIter {
        // Fully qualified: the inherent method, not this trait method.
        Node::children(self)
    }

    fn child_count(self) -> usize {
        self.child_ids().len()
    }

    fn style(self) -> Self::Style {
        StyleView::of(self)
    }

    /// The canonical dispatch skeleton (see `neutron_star::compute`):
    /// `display: none` is hidden **before** the cache wrapper; every
    /// generated box routes to its algorithm inside it.
    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        // Text nodes carry no computed style: they lay out as leaves inside
        // an anonymous box (initial box values; content via the payload's
        // measurement hook).
        let display = if self.is_text_node() {
            DisplayMode::Leaf
        } else {
            self.computed_style().map_or(DisplayMode::Leaf, |style| {
                display_mode(style.clone_display())
            })
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
                        node.ext().measure_leaf(node, measure_input)
                    });
                    compute_leaf_layout(input, &view, &mut measurer)
                };
                // Flow/contents container layout is unimplemented (see
                // `DisplayMode::Leaf`): the box itself is a leaf, and any
                // children are zeroed so stale geometry cannot survive a
                // display change. Commit only — measurement stays free of
                // durable writes.
                if input.goal == LayoutGoal::Commit {
                    for grandchild in Node::children(node) {
                        hide_subtree(grandchild);
                    }
                }
                output
            }
        })
    }

    fn set_unrounded_layout(self, layout: &Layout) {
        self.layout_data.borrow_mut().unrounded = *layout;
    }

    fn unrounded_layout(self) -> Layout {
        self.layout_data.borrow().unrounded
    }

    fn set_final_layout(self, layout: &Layout) {
        self.layout_data.borrow_mut().rounded = *layout;
    }

    fn set_static_position(self, static_position: Point<f32>) {
        let mut data = self.layout_data.borrow_mut();
        data.static_position = static_position;
        // Queue the hoisted node — once per pass — on the document node's
        // slot, the pass-shared anchor every handle can reach.
        if !data.hoisted_recorded {
            data.hoisted_recorded = true;
            drop(data);
            self.owner_document()
                .layout_data
                .borrow_mut()
                .hoisted
                .push(self.id());
        }
    }

    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput> {
        self.layout_data.borrow().measure_cache.get(input)
    }

    fn cache_store(self, input: LayoutInput, output: LayoutOutput) {
        self.layout_data
            .borrow_mut()
            .measure_cache
            .store(input, output);
    }

    fn cache_clear(self) {
        self.layout_data.borrow_mut().measure_cache.clear();
    }
}

/// Run the full layout pipeline over a flushed document: in-flow root pass →
/// positioned pass for hoisted out-of-flow nodes → device-pixel rounding.
///
/// Takes the document as a shared borrow — the caller's `&mut Document`
/// (relinquished for the duration) is what guarantees the immutable pass;
/// all writes go through the nodes' `layout_data` cells.
pub(super) fn run_layout<T: MeasureLeaf>(document: &Document<T>, viewport: Size<f32>, scale: f32) {
    let Some(root) = document.root_element() else {
        return;
    };
    // A panicked earlier pass may have left queue entries behind; the queue
    // is per-pass state.
    document
        .root_node()
        .layout_data
        .borrow_mut()
        .hoisted
        .clear();

    compute_root_layout(
        root,
        Size::new(
            AvailableSpace::Definite(viewport.width),
            AvailableSpace::Definite(viewport.height),
        ),
    );
    run_positioned_pass(document, viewport);
    round_layout(root, scale);
}

// --- the positioned pass ------------------------------------------------------

/// Complete every hoisted out-of-flow node recorded during in-flow layout.
///
/// FIFO over the pass's queue: a hoisted node nested inside another hoisted
/// subtree is recorded while its formatting parent commits during the outer
/// node's `compute_absolute_layout`, i.e. strictly after its containing
/// block's own layout is stored — so each dequeued node can convert its
/// static position through already-final ancestor geometry.
fn run_positioned_pass<T: MeasureLeaf>(document: &Document<T>, viewport: Size<f32>) {
    let queue = |index: usize| {
        let data = document.root_node().layout_data.borrow();
        data.hoisted.get(index).copied()
    };
    let mut index = 0;
    while let Some(id) = queue(index) {
        index += 1;
        position_hoisted(document, id, viewport);
    }
    document
        .root_node()
        .layout_data
        .borrow_mut()
        .hoisted
        .clear();
}

fn position_hoisted<T: MeasureLeaf>(
    document: &Document<T>,
    id: crate::NodeId,
    viewport: Size<f32>,
) {
    let node = document
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
    let fixed = node
        .computed_style()
        .is_some_and(|style| style.clone_position() == PositionProperty::Fixed);

    // Resolve the containing block: the nearest qualifying element ancestor,
    // else the viewport (the initial containing block).
    let mut containing = None;
    let mut ancestor = node.parent();
    while let Some(current) = ancestor {
        let Some(style) = current.computed_style() else {
            break; // reached the document node
        };
        let establishes = if fixed {
            establishes_fixed_containing_block(&style)
        } else {
            establishes_absolute_containing_block(&style)
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
        None => (Point::ZERO, viewport),
    };

    // Convert the recorded static position (formatting-parent border-box
    // space) into containing-block padding-box space.
    let parent_origin = absolute_origin(parent);
    let static_position = node.layout_data.borrow().static_position;
    let static_in_cb = Point::new(
        parent_origin.x + static_position.x - containing_origin.x,
        parent_origin.y + static_position.y - containing_origin.y,
    );

    let mut layout = compute_absolute_layout(node, containing_size, static_in_cb);

    // Store in the formatting parent's space, keeping `Layout::location`'s
    // parent-relative contract intact for rounding and painting, with the
    // parent's order-modified paint index.
    layout.location = Point::new(
        containing_origin.x + layout.location.x - parent_origin.x,
        containing_origin.y + layout.location.y - parent_origin.y,
    );
    layout.order = sibling_paint_order(document, parent, id);
    LayoutNode::set_unrounded_layout(node, &layout);
}

/// The node's paint index among its siblings: document position after a
/// stable sort by style `order` — the same order-modified index the
/// algorithms assign to the in-flow children they place.
fn sibling_paint_order<T>(document: &Document<T>, parent: &Node<T>, id: crate::NodeId) -> u32 {
    let children = parent.child_ids();
    let order_of = |child: &crate::NodeId| {
        document
            .get(*child)
            .and_then(Node::computed_style)
            .map_or(0, |style| style.get_position().order)
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
