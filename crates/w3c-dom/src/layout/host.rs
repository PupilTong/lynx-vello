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
//! protocol's re-entrancy contract. The pipeline has no pass-shared state
//! at all: the positioned pass re-walks the tree instead of consuming a
//! queue (see [`position_hoisted_subtree`]).

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
    establishes_fixed_containing_block, resolve_position,
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
        // The recorded value persists across passes on purpose: it is
        // relative to the formatting parent, so it stays valid exactly as
        // long as the parent's own layout answers from its cache — the
        // positioned pass re-reads it every pass.
        self.layout_data.borrow_mut().static_position = static_position;
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
    compute_root_layout(
        root,
        Size::new(
            AvailableSpace::Definite(viewport.width),
            AvailableSpace::Definite(viewport.height),
        ),
    );
    position_hoisted_subtree(root, viewport);
    round_layout(root, scale);
}

// --- the positioned pass ------------------------------------------------------

/// Complete every hoisted out-of-flow node in the visible tree.
///
/// This is a fresh pre-order walk **every pass**, deliberately not a queue
/// filled during in-flow layout: a hoisted node whose formatting parent
/// answered from its measurement cache is never re-visited by the
/// algorithms, yet its viewport-anchored position must still be recomputed
/// when an *ancestor* moved. The static position recorded on the node is
/// parent-relative, so it stays valid exactly as long as the parent's
/// cached layout does; this walk re-derives everything else from current
/// ancestor geometry. Pre-order also gives hoisted-inside-hoisted nesting
/// for free: an outer hoisted ancestor is finalized before any hoisted
/// descendant converts its static position through it.
fn position_hoisted_subtree<T: MeasureLeaf>(node: &Node<T>, viewport: Size<f32>) {
    let Some(style) = node.computed_style() else {
        return; // text nodes are never positioned and have no children
    };
    if display_mode(style.clone_display()) == DisplayMode::None {
        return; // hidden subtrees are zeroed, not positioned
    }
    // The root element is laid out by `compute_root_layout`, never hoisted
    // (it has no element formatting parent).
    if node.parent().is_some_and(Node::is_element)
        && resolve_position(node, &style) == PositionProperty::Fixed
    {
        position_hoisted(node, viewport);
    }
    for child in Node::children(node) {
        position_hoisted_subtree(child, viewport);
    }
}

fn position_hoisted<T: MeasureLeaf>(node: &Node<T>, viewport: Size<f32>) {
    let Some(parent) = node.parent() else {
        return;
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
            establishes_fixed_containing_block(current, &style)
        } else {
            establishes_absolute_containing_block(current, &style)
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
    layout.order = sibling_paint_order(parent, node.id());
    LayoutNode::set_unrounded_layout(node, &layout);
}

/// The node's paint index among its siblings, per the engine's paint-key
/// rule (`sort_and_assign_layout_order`): non-generated (`display: none`)
/// children are excluded, **out-of-flow children participate with effective
/// `order` 0** (their authored `order` deliberately does not reorder them),
/// and ties break by document index — the same order-modified index the
/// algorithms assign to the children they place.
fn sibling_paint_order<T>(parent: &Node<T>, target: crate::NodeId) -> u32 {
    let mut keys: Vec<(i32, usize)> = Vec::new();
    let mut target_key = None;
    for (index, child) in Node::children(parent).enumerate() {
        let effective_order = match child.computed_style() {
            Some(style) => {
                if display_mode(style.clone_display()) == DisplayMode::None {
                    continue; // no box generated: not part of the paint order
                }
                if matches!(
                    style.clone_position(),
                    PositionProperty::Absolute | PositionProperty::Fixed
                ) {
                    0
                } else {
                    style.get_position().order
                }
            }
            // Text nodes (anonymous boxes): in-flow, initial `order`.
            None => 0,
        };
        let key = (effective_order, index);
        if child.id() == target {
            target_key = Some(key);
        }
        keys.push(key);
    }
    let Some(target_key) = target_key else {
        return 0;
    };
    keys.sort_unstable();
    let position = keys
        .iter()
        .position(|&key| key == target_key)
        .expect("the target's own key was pushed");
    u32::try_from(position).unwrap_or(u32::MAX)
}
