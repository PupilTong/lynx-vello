//! Protocol machinery entry points — free generic functions over the tree
//! traits.
//!
//! There is deliberately no engine object: everything callable is a function
//! so that hosts compose them freely inside their
//! [`compute_child_layout`](crate::tree::LayoutTree::compute_child_layout)
//! dispatch, and so that unused entry points (and their monomorphizations)
//! never exist in the host's binary.
//!
//! This module currently contains only the **generic machinery** (root
//! entry, cache wrapper, hidden zeroing, leaf boxing, rounding). The
//! algorithm entry points — `compute_flexbox_layout<Tree: FlexTree>` (L1)
//! and `compute_grid_layout<Tree: GridTree>` (L2) — are specified in
//! `docs/layout-architecture.md` and land as sibling functions here with
//! the same shape: `fn(&mut Tree, NodeId, LayoutInput) -> LayoutOutput`.
//!
//! # The canonical dispatch skeleton
//!
//! Every host implements the same shape once; this is the whole integration
//! surface of the engine (a host with custom layout modes — e.g. lynx-vello's
//! `display: linear`/`relative` — adds arms that call its own algorithms):
//!
//! ```
//! use neutron_star::compute::{compute_cached_layout, compute_hidden_layout};
//! use neutron_star::tree::{CacheTree, FlexTree, GridTree, LayoutInput, LayoutOutput, NodeId};
//!
//! # #[derive(Clone, Copy)]
//! enum Display {
//!     Flex,
//!     Grid,
//!     Hidden,
//! }
//!
//! fn dispatch<Tree>(tree: &mut Tree, node: NodeId, input: LayoutInput) -> LayoutOutput
//! where
//!     Tree: FlexTree + GridTree + CacheTree,
//! {
//!     compute_cached_layout(tree, node, input, |tree, node, input| {
//!         match host_display_of(tree, node) {
//!             Display::Hidden => compute_hidden_layout(tree, node),
//!             // L1: Display::Flex => compute_flexbox_layout(tree, node, input),
//!             // L2: Display::Grid => compute_grid_layout(tree, node, input),
//!             // host: Display::Linear => host_linear_layout(tree, node, input),
//!             // host: Display::Leaf => compute_leaf_layout(input, &style, resolve, measure),
//!             _ => unimplemented!("algorithm arms land in L1/L2"),
//!         }
//!     })
//! }
//! # fn host_display_of<T>(_: &T, _: NodeId) -> Display { Display::Flex }
//! ```
//!
//! The host's `LayoutTree::compute_child_layout` implementation simply calls
//! its `dispatch`. Algorithms call back into `compute_child_layout` for each
//! child, so the same routing (and the same cache) applies at every level of
//! the tree.
//!
//! # Pass structure
//!
//! A full layout run is two host-initiated passes:
//!
//! 1. [`compute_root_layout`] — sizes and positions the whole (dirty part of the) tree in unrounded
//!    CSS pixels.
//! 2. [`round_layout`] — derives the pixel-snapped layouts. Optional but recommended for crisp
//!    rendering; kept separate so relayout always starts from unrounded values (re-rounding rounded
//!    values drifts).
//!
//! # Status
//!
//! Protocol milestone (L0): every function below is a documented, callable,
//! `todo!()`-bodied stub. The rustdoc on each function — inputs, outputs,
//! invariants — *is* the specification its implementation must meet.

mod leaf;

pub use leaf::compute_leaf_layout;

use crate::geometry::Size;
use crate::tree::{
    AvailableSpace, CacheTree, LayoutInput, LayoutOutput, LayoutTree, NodeId, RoundTree,
};

/// Lays out the tree under `root` into `available_space`.
///
/// The host's entry point for a layout flush. Builds the root
/// [`LayoutInput`] (`PerformLayout`, no known dimensions, `parent_size` from
/// the definite parts of `available_space`), routes it through
/// [`compute_child_layout`](LayoutTree::compute_child_layout) — so the root
/// dispatches like any other node — resolves the root's own margins, and
/// stores the root's [`Layout`](crate::tree::Layout) (at location `(0, 0)`
/// plus resolved margins) via
/// [`set_unrounded_layout`](LayoutTree::set_unrounded_layout).
///
/// Incrementality: this walks — and pays for — only what caches miss. For a
/// clean subtree the recursion is answered from [`CacheTree`] storage at its
/// root.
///
/// # Panics
///
/// Protocol stub — implemented in milestone L1; calling this currently
/// panics with `todo!`.
pub fn compute_root_layout<Tree: LayoutTree>(
    tree: &mut Tree,
    root: NodeId,
    available_space: Size<AvailableSpace>,
) {
    let _ = (tree, root, available_space);
    todo!("L1: root layout entry (see rustdoc for the contract)")
}

/// Wraps one node's layout computation in the shared caching policy.
///
/// The host calls this at the top of its dispatch (see the module docs);
/// `compute_uncached` is the actual routing closure. On a usable cached
/// entry (matching per the [`cache`](crate::cache) module's contract) the
/// closure is skipped entirely; otherwise its result is stored before being
/// returned. [`RunMode::PerformHiddenLayout`](crate::tree::RunMode) requests
/// bypass the cache in both directions.
///
/// # Panics
///
/// Protocol stub — implemented in milestone L1; calling this currently
/// panics with `todo!`.
pub fn compute_cached_layout<Tree, ComputeFn>(
    tree: &mut Tree,
    node: NodeId,
    input: LayoutInput,
    compute_uncached: ComputeFn,
) -> LayoutOutput
where
    Tree: CacheTree,
    ComputeFn: FnOnce(&mut Tree, NodeId, LayoutInput) -> LayoutOutput,
{
    let _ = (tree, node, input, compute_uncached);
    todo!("L1: cache consult/fill wrapper (see rustdoc for the contract)")
}

/// Zeroes the layout of a `display: none` node and its whole subtree.
///
/// Stores an all-zero [`Layout`](crate::tree::Layout) for `node`'s children
/// and recurses through
/// [`compute_child_layout`](LayoutTree::compute_child_layout) with
/// [`RunMode::PerformHiddenLayout`](crate::tree::RunMode), so previously
/// laid-out geometry can't leak out of a subtree that just became hidden.
/// Returns [`LayoutOutput::HIDDEN`].
///
/// # Panics
///
/// Protocol stub — implemented in milestone L1; calling this currently
/// panics with `todo!`.
pub fn compute_hidden_layout<Tree: LayoutTree>(tree: &mut Tree, node: NodeId) -> LayoutOutput {
    let _ = (tree, node);
    todo!("L1: hidden-subtree zeroing (see rustdoc for the contract)")
}

/// Derives pixel-snapped final layouts from the unrounded layouts under
/// `root`.
///
/// Walks the tree via [`RoundTree`], reading each
/// [`unrounded_layout`](RoundTree::unrounded_layout) and writing a rounded
/// copy through [`set_final_layout`](RoundTree::set_final_layout).
///
/// Rounding contract (cumulative-error-free): positions are rounded in
/// *accumulated* (root-relative) space and sizes derived as
/// `round(pos + size) - round(pos)`, so adjacent edges land on the same
/// physical pixel and a box's rounded size never drifts more than one pixel
/// from its unrounded size — at the cost that equal unrounded sizes may
/// round to sizes differing by one pixel (the standard trade-off, also made
/// by browsers). Idempotent given unchanged unrounded inputs.
///
/// # Panics
///
/// Protocol stub — implemented in milestone L1; calling this currently
/// panics with `todo!`.
pub fn round_layout<Tree: RoundTree>(tree: &mut Tree, root: NodeId) {
    let _ = (tree, root);
    todo!("L1: pixel-snapping pass (see rustdoc for the contract)")
}
