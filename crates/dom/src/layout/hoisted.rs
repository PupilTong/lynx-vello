//! Out-of-flow boxes laid out by a containing block that is not their box
//! parent — an `absolute` box under a non-positioned parent, a `fixed` one
//! under a transformed ancestor — which `StyleView::position` lowers to
//! `fixed` so their parent's algorithm records only their static position.
//!
//! css-position-3 lays such a box out with its containing block, after that
//! block's in-flow content and in tree order with its other out-of-flow
//! boxes; css-anchor-position-1 §2.3's acceptable anchors are exactly what
//! that order has placed before a box. `hughie` does it in the containing
//! block's own absolute pass, from the boxes this module reports
//! ([`LayoutTree::hoisted_children`](hughie::tree::LayoutTree::hoisted_children)):
//!
//! - **Registration** happens where the parent records the static position (`set_static_position`),
//!   which every committing run of that parent does: [`DocumentLayoutState::register_hoisted`]
//!   lists the box under the element that generates its containing block. A box whose containing
//!   block is the initial one is listed under none; the run's tail places it, after everything
//!   else, which is where the initial containing block's own out-of-flow phase falls.
//! - **Reads** re-derive each listed box's containing block and skip any box that no longer escapes
//!   to the one asking, or that sits under a box that lays none of its children out (`display:
//!   none`, replaced content, a paragraph that swallows its children). A listing can therefore go
//!   stale without misplacing anything.
//! - **Moves** are the one change that reaches no parent: a subtree inserted elsewhere keeps its
//!   caches, so the parents inside it never record again. [`Document::invalidate_hoisted_under`]
//!   lays every listed box inside a moved subtree out again, which re-registers it.
//! - **In-place relayout** of a subtree whose hoisted box escapes it reaches no containing block
//!   above it. The rounding tail places those boxes ([`places_late`]), with the same function the
//!   containing block's pass would have used, so a box comes out the same either way.

use hughie::compute::compute_hoisted_layout;
use hughie::geometry::Point;
use hughie::style::{CoreStyle, PositionProperty};
use hughie::tree::{HoistedChild, Layout, LayoutTree};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use super::anchors::{connected_chain, containing_block_generator, precedes};
use super::style::{DisplayMode, StyleView, box_parent, display_mode, resolve_position};
use crate::tree::document::{Document, DocumentLayoutState, NodeId, NodeSlot, TreeArenas};
use crate::tree::node::Node;

/// The element whose absolute pass lays `node` out, when `node` is a box
/// lowered to `fixed` whose containing block is an element's and every box
/// between them lays its children out; `None` otherwise — not hoisted, or
/// hoisted to the initial containing block.
pub(crate) fn hoisting_block<T>(node: &Node<T>) -> Option<NodeSlot> {
    let style = node.layout_computed_style()?;
    let display = style.clone_display();
    if display.is_none() || display.is_contents() {
        return None;
    }
    if !node.flat_parent().is_some_and(Node::is_element)
        || resolve_position(node, style) != PositionProperty::Fixed
    {
        return None;
    }
    let block = containing_block_generator(node)?;
    let mut current = node.flat_parent();
    while let Some(ancestor) = current {
        if ancestor.id() == block.id() {
            return Some(block.id());
        }
        let style = StyleView::try_of(ancestor)?;
        match display_mode(style.display()) {
            DisplayMode::None => return None,
            DisplayMode::Contents => {}
            DisplayMode::Text if super::text_block::replaces_children(ancestor) => return None,
            _ if ancestor.is_replaced() || style.skips_contents() => return None,
            _ => {}
        }
        current = ancestor.flat_parent();
    }
    None
}

/// What `hoisted_children` answers for `block`: the listed boxes that still
/// escape to it, in flat tree order, with the index of the flattened child
/// of `block` each sits under.
pub(super) fn children_of<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    block: NodeSlot,
) -> SmallVec<[HoistedChild<NodeSlot>; 2]> {
    let Some(listed) = state.hoisted_to.get(&block) else {
        return SmallVec::new();
    };
    let mut found: SmallVec<[(NodeSlot, NodeSlot, super::anchors::Chain); 2]> = SmallVec::new();
    for &node in listed {
        let Some(hoisted) = tree.get(node) else {
            continue;
        };
        if hoisting_block(hoisted) != Some(block) {
            continue;
        }
        let Some(chain) = connected_chain(hoisted) else {
            continue;
        };
        // The child of `block` the box sits under, as the flattened child
        // list names it: the first box below `block` on the chain, a
        // `display: contents` level being spliced into its parent's list.
        let Some(at) = chain.iter().position(|&id| id == block) else {
            continue;
        };
        let via = chain[..at]
            .iter()
            .rev()
            .copied()
            .find(|&id| {
                tree.get(id)
                    .and_then(Node::layout_computed_style)
                    .is_none_or(|style| !style.clone_display().is_contents())
            })
            .unwrap_or(node);
        found.push((node, via, chain));
    }
    if found.len() > 1 {
        found.sort_by(|(_, _, a), (_, _, b)| {
            if precedes(tree, a, b) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        });
    }
    // Tree order is the order of the vias' subtrees, so one walk over the
    // flattened children numbers them all.
    let mut children = SmallVec::with_capacity(found.len());
    let mut pending = found.iter().peekable();
    for (index, (child, ..)) in tree.flattened_children(block).enumerate() {
        while let Some(&&(node, via, _)) = pending.peek() {
            if via != child {
                break;
            }
            children.push(HoistedChild { node, via: index });
            pending.next();
        }
        if pending.peek().is_none() {
            break;
        }
    }
    children
}

/// `node`'s box parent's border-box origin in `block`'s border-box
/// coordinates, from this run's boxes. A `display: contents` level has no
/// box and offsets nothing.
pub(super) fn parent_offset<T>(
    tree: &TreeArenas<T>,
    state: &DocumentLayoutState,
    block: NodeSlot,
    node: NodeSlot,
) -> Point<f32> {
    let mut offset = Point::ZERO;
    let mut current = box_parent(tree.at(node));
    while let Some(ancestor) = current {
        if ancestor.id() == block {
            break;
        }
        let location = state.at(ancestor.id()).slot.unrounded.location;
        offset = Point::new(offset.x + location.x, offset.y + location.y);
        current = box_parent(ancestor);
    }
    offset
}

/// Stores a hoisted box's layout with its paint order among its parent's
/// children, and marks the boxes between it and `block` so the rounding
/// tail, which descends only through boxes something wrote, reaches it.
pub(super) fn commit<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    block: NodeSlot,
    node: NodeSlot,
    mut layout: Layout,
) {
    let hoisted = tree.at(node);
    let Some(parent) = hoisted.flat_parent_slot() else {
        tree.layout_mut(state, node).set_unrounded(layout);
        return;
    };
    let ordering_parent = box_parent(hoisted).map_or(parent, Node::slot);
    layout.order = super::host::sibling_paint_order(tree, ordering_parent, node);
    tree.layout_mut(state, node).set_unrounded(layout);
    if state.in_rounding_tail || !tree.layout(state, node).needs_rounding() {
        return;
    }
    let mut current = Some(parent);
    while let Some(ancestor) = current {
        if ancestor == block {
            break;
        }
        state.at_mut(ancestor).slot.mark_subtree_dirty();
        current = tree.at(ancestor).flat_parent_slot();
    }
}

/// Whether the rounding tail must place `node`, a box hoisted to the element
/// `block`, itself: a subtree between them was relaid in place, so `block`'s
/// absolute pass did not run for it this run.
pub(super) fn places_late<T>(
    tree: &TreeArenas<T>,
    node: NodeSlot,
    block: NodeSlot,
    relayed: &FxHashSet<NodeId>,
) -> bool {
    if relayed.is_empty() {
        return false;
    }
    let mut current = tree.at(node).flat_parent_slot();
    while let Some(ancestor) = current {
        if ancestor == block {
            return false;
        }
        if relayed.contains(&ancestor) {
            return true;
        }
        current = tree.at(ancestor).flat_parent_slot();
    }
    false
}

/// Places `node` against `block` from the tail; see [`places_late`].
pub(super) fn place_late<T>(
    tree: &TreeArenas<T>,
    state: &mut DocumentLayoutState,
    block: NodeSlot,
    node: NodeSlot,
) {
    compute_hoisted_layout(tree, state, block, node);
}

impl<T> Document<T> {
    /// Lays every listed hoisted box inside the subtree at `root` out again:
    /// the subtree moved — inserted elsewhere, or reslotted — with its
    /// caches, so the parents inside it will not record again, and its
    /// boxes may escape to another containing block now.
    pub(crate) fn invalidate_hoisted_under(&mut self, root: NodeId) {
        if self.layout_state().hoisted_to.is_empty() {
            return;
        }
        let inside: SmallVec<[NodeId; 4]> = {
            let tree = self.arenas();
            self.layout_state()
                .hoisted_to
                .values()
                .flatten()
                .copied()
                .filter(|&node| {
                    let mut current = tree.get(node);
                    while let Some(step) = current {
                        if step.id() == root {
                            return true;
                        }
                        current = step.flat_parent();
                    }
                    false
                })
                .collect()
        };
        for node in inside {
            self.invalidate_layout(node);
        }
    }
}
