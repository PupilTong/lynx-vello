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

#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::compute_leaf_layout_with_measurement_for_testing;
use neutron_star::compute::{
    compute_absolute_layout, compute_boundary_relayout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_leaf_layout, compute_linear_layout,
    compute_relative_layout, compute_root_layout, compute_skipped_contents_layout, hide_subtree,
    round_layout,
};
use neutron_star::geometry::{Point, Size};
use neutron_star::invalidate::is_relayout_boundary;
use neutron_star::style::{CoreStyle, PositionProperty, TextRun};
use neutron_star::text::TextMeasurer;
use neutron_star::tree::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput,
};

use super::style::{
    DisplayMode, StyleView, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, resolve_position, skips_contents,
};
use crate::document::Document;
use crate::node::{ChildrenIter, Node};

impl<'dom, T> LayoutNode for &'dom Node<T> {
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
    /// `display: none` is hidden and `content-visibility: hidden` skips its
    /// contents **before** the cache wrapper (their subtree-hiding must bypass
    /// the cache); every generated, non-skipping box routes to its algorithm
    /// inside it.
    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
        // Text nodes carry no computed box style: they lay out through the
        // concrete Parley path inside an anonymous box, with inherited text
        // style read from their parent.
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

        // `content-visibility: hidden` skips its contents: the box is sized from
        // its own styles + `contain-intrinsic-size` and its subtree is hidden,
        // laying out no children. Like `display: none`, this routes **before**
        // the cache wrapper (the child-hiding must bypass the cache), right after
        // the `display: none` check — the host dispatch contract
        // `compute_skipped_contents_layout` documents. Text nodes are lent the
        // anonymous initial values (`content-visibility: visible`), so this never
        // fires for them.
        if self.style().skips_contents() {
            return compute_skipped_contents_layout(self, input);
        }

        compute_cached_layout(self, input, |node, input| match display {
            DisplayMode::None => unreachable!("hidden nodes never reach the cache wrapper"),
            DisplayMode::Flex => compute_flexbox_layout(node, input),
            DisplayMode::Grid => compute_grid_layout(node, input),
            DisplayMode::Linear => compute_linear_layout(node, input),
            DisplayMode::Relative => compute_relative_layout(node, input),
            DisplayMode::Leaf => {
                let view = node.style();
                let output = if node.is_text_node() {
                    let run = TextRun {
                        text: node.text().unwrap_or_default(),
                        style: &view,
                        preserve_newlines: false,
                    };
                    let mut context = node.text_context().borrow_mut();
                    let mut layout_data = node.layout_data.borrow_mut();
                    let artifacts = layout_data.text_artifacts.get_or_insert_with(Box::default);
                    let mut measurer =
                        TextMeasurer::new(&mut context, artifacts, &view, std::iter::once(run));
                    measurer.compute_layout(input)
                } else {
                    #[cfg(feature = "layout-test-utils")]
                    if let Some(metrics) = node.layout_data.borrow().test_leaf_metrics {
                        compute_leaf_layout_with_measurement_for_testing(
                            input,
                            &view,
                            None,
                            |_measure_input| metrics,
                        )
                    } else {
                        compute_leaf_layout(input, &view, node.natural_size())
                    }
                    #[cfg(not(feature = "layout-test-utils"))]
                    compute_leaf_layout(input, &view, node.natural_size())
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

    #[inline]
    fn set_unrounded_layout(self, layout: Layout) {
        self.layout_data.borrow_mut().unrounded = layout;
    }

    #[inline]
    fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
        let data = self.layout_data.borrow();
        read(&data.unrounded)
    }

    #[inline]
    fn clone_unrounded_layout(self) -> Layout {
        self.layout_data.borrow().unrounded.clone()
    }

    #[inline]
    fn set_final_layout(self, layout: Layout) {
        self.layout_data.borrow_mut().rounded = layout;
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
        self.layout_data.borrow_mut().invalidate_measurement();
    }
}

/// Run the full layout pipeline over a flushed document: parked-boundary
/// re-runs → in-flow root pass → positioned pass for hoisted out-of-flow
/// nodes → device-pixel rounding.
///
/// Takes the document as a shared borrow — the caller's `&mut Document`
/// (relinquished for the duration) is what guarantees the immutable pass;
/// all writes go through the nodes' `layout_data` cells.
///
/// # Parked relayout boundaries run first, deepest-first
///
/// A boundary-stopped [`invalidate_layout`](Document::invalidate_layout) parks
/// each `contain: strict` / skipped `content-visibility` boundary it stops at,
/// paired with the committed [`LayoutInput`] that preserves the boundary's
/// parent-imposed outer size. Those boundaries are re-run in place **before**
/// [`compute_root_layout`]: their ancestors kept warm caches, so the root pass
/// answers those ancestors from cache and never descends into a boundary's
/// interior — the in-place re-run is what refreshes it. The root pass still
/// keeps the final say for any boundary a change *above* it also cleared to the
/// document root (it re-runs that boundary with its now-current input and
/// overwrites this preview).
///
/// The parked boundaries are re-run **deepest-first** (greatest tree depth
/// first). When one flush parks nested boundaries — an outer `B1` and an inner
/// `B2` inside it — `B1`'s re-run re-lays-out its whole interior, `B2` and
/// `B2`'s subtree included, at `B2`'s *current* parent-imposed size, whereas
/// `B2`'s own re-run only replays its stale committed input. Running the inner
/// boundary first and the outer last lets the outer win, so an interior whose
/// imposed size changed ends at the new size instead of being overwritten by
/// the inner boundary's stale replay. The inner boundary is still re-run (never
/// dropped as redundant): the outer re-run can cache-hit before it reaches the
/// inner one when the path between them was not invalidated, and then only the
/// inner boundary's own re-run refreshes its interior. Independent boundaries
/// have unordered depths and are order-insensitive, so a plain depth sort
/// suffices.
pub(super) fn run_layout<T>(document: &Document<T>, viewport: Size<f32>, scale: f32) {
    let Some(root) = document.root_element() else {
        return;
    };
    // Re-run parked relayout boundaries deepest-first, before the root pass
    // (see this function's docs for why both matter). Depth orders them; the
    // inner boundary is kept, not deduped. A parked root a later flush turned
    // non-boundary (its `contain` was removed) is skipped — that flush already
    // cleared its cache toward the root, so the root pass covers it.
    let mut parked: Vec<(usize, crate::NodeId, LayoutInput)> = document
        .relayout_roots()
        .iter()
        .map(|&(id, input)| (boundary_depth(document, id), id, input))
        .collect();
    parked.sort_by_key(|&(depth, ..)| std::cmp::Reverse(depth));
    for (_, id, input) in parked {
        if let Some(node) = document.get(id)
            && node.is_element()
            && is_relayout_boundary(&StyleView::of(node))
        {
            let output = compute_boundary_relayout(node, input);
            // `compute_boundary_relayout` deliberately does not restore the
            // boundary's own `Layout`: by the relayout-boundary theorem its
            // outer `size` and parent-relative `location` cannot change from an
            // interior mutation, so that record stays owned by the still-warm
            // parent. But `content_size` (scrollable overflow) IS derived from
            // the interior that just re-arranged, so the stored value is now
            // stale — merge only that field into the stored unrounded layout,
            // before `round_layout` below snaps it, so scroll ranges track the
            // new interior. Every other `Layout` field
            // (order/size/location/border/padding/margin, and there is no
            // scrollbar-size field — Lynx scrollbars are overlay-only) depends
            // solely on the boundary's own unchanged style and its
            // parent-imposed input, so it stays valid without a merge.
            node.layout_data.borrow_mut().unrounded.content_size = output.content_size;
        }
    }
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

/// The number of ancestor links from `id` up to (and including) the document
/// node — the key that orders parked relayout boundaries **deepest-first** in
/// [`run_layout`] (see its docs). Walks real parent links (the host owns them),
/// so this is a cheap spine walk, not a search. A vacant `id` reports depth 0
/// and is harmlessly skipped by the re-run loop.
fn boundary_depth<T>(document: &Document<T>, id: crate::NodeId) -> usize {
    let mut depth = 0;
    let mut current = document.get(id).and_then(Node::parent);
    while let Some(node) = current {
        depth += 1;
        current = node.parent();
    }
    depth
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
fn position_hoisted_subtree<T>(node: &Node<T>, viewport: Size<f32>) {
    let Some(style) = node.computed_style() else {
        return; // text nodes are never positioned and have no children
    };
    let display = display_mode(style.clone_display());
    if display == DisplayMode::None {
        return; // hidden subtrees are zeroed, not positioned
    }
    // The root element is laid out by `compute_root_layout`, never hoisted
    // (it has no element formatting parent).
    if node.parent().is_some_and(Node::is_element)
        && resolve_position(node, &style) == PositionProperty::Fixed
    {
        position_hoisted(node, viewport);
    }
    // Two cases generate no boxes for their contents, so the walk must not
    // descend and revive a hoisted descendant inside them:
    //   * the leaf fallback (flow/contents containers) zeroes its children;
    //   * a skipped-contents box (`content-visibility: hidden`) had its whole subtree hidden by
    //     `compute_skipped_contents_layout` on Commit.
    // Pruning here mirrors the `display: none` early return above: the node
    // itself may still be a hoisted box (handled just above), but its skipped
    // contents cannot — a `position: fixed` descendant of skipped contents
    // produces no positioned box (css-contain-2 skipping).
    if display == DisplayMode::Leaf || skips_contents(&style) {
        return;
    }
    for child in Node::children(node) {
        position_hoisted_subtree(child, viewport);
    }
}

fn position_hoisted<T>(node: &Node<T>, viewport: Size<f32>) {
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
            let origin = absolute_origin(block);
            let data = block.layout_data.borrow();
            let layout = &data.unrounded;
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
    LayoutNode::set_unrounded_layout(node, layout);
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
