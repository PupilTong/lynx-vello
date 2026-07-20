//! The tree protocol: how the engine sees the host's node tree.
//!
//! neutron-star owns **no tree and no layout store**. The host hands the
//! engine a [`LayoutNode`]: a cheap-to-copy **node handle** borrowed from the
//! host's tree for the duration of one layout flush — a plain `&'dom Node`,
//! or a `(&'dom Tree, index)` pair, or an FFI pointer wrapper. The handle is
//! the node's identity; the engine never fabricates handles and never
//! attaches meaning to their contents. This is the same shape stylo demands
//! of its DOM (`TNode`/`TElement` implemented on `&'dom Node`), so a host
//! tree can serve both engines with one set of one-word handles.
//!
//! Everything the engine reads through a handle — topology and style views —
//! is immutable for the whole flush. Everything the
//! engine writes — unrounded/final layouts, static positions, cache slots —
//! goes through the handle into **host-owned interior-mutable per-node
//! slots** (`Cell`/`RefCell`, or `AtomicRefCell`/`UnsafeCell` under the
//! host's own discipline, exactly as stylo hosts already store per-element
//! style data). There is no `&mut` anywhere in the protocol, so a borrowed
//! style view can trivially stay live across recursive child layout — the
//! borrow checker never sees a conflicting mutable borrow.
//!
//! # The recursion contract
//!
//! Layout recursion round-trips through the host on every node:
//!
//! ```text
//!  compute_root_layout(root, …)
//!      └─▶ root.compute_child_layout(input)                 [host dispatch]
//!            └─▶ <algorithm>(root, input)                   [engine algo]
//!                  ├─▶ child.compute_child_layout(input)    [host dispatch]
//!                  │     └─▶ … per that child's display …
//!                  └─▶ child.set_unrounded_layout(…)
//! ```
//!
//! [`LayoutNode::compute_child_layout`] is the **dispatch point**: the host
//! inspects the child's `display` (which the engine deliberately has no enum
//! for) and routes to an engine algorithm entry point (Flexbox, Grid, and
//! Starlight Linear and Relative are implemented in
//! [`compute`](crate::compute)),
//! [`compute_leaf_layout`](crate::compute::compute_leaf_layout) (text/images
//! via a generic [`LeafMeasurer`](crate::compute::LeafMeasurer)), or a
//! host-provided additional algorithm. The host handles `display: none` first
//! with [`hide_subtree`](crate::compute::hide_subtree), then wraps
//! visible-node routing in
//! [`compute_cached_layout`](crate::compute::compute_cached_layout) so every
//! sizing path shares one cache policy. See the `compute` module docs for the
//! canonical dispatch skeleton.
//!
//! # The layout epoch
//!
//! A layout flush is an **immutable epoch**: topology, computed style, and
//! display dispatch inputs must not change until the flush
//! finishes. Handles must stay valid for the whole flush. Per-node layout,
//! cache, and measurement state may mutate through the handles — that is the
//! host's interior-mutability discipline, and layout is single-threaded, so
//! `Cell`/`RefCell` suffice. Two rules keep runtime borrow tracking trivial:
//! the host must not hold a per-node slot borrow across a recursive
//! [`compute_child_layout`](LayoutNode::compute_child_layout) call, and the
//! engine never re-enters a node's cache while that node's leaf measurer is
//! live.
//!
//! # Style views are borrowed, copy-free
//!
//! [`LayoutNode::style`] returns [`LayoutNode::Style`] — typically
//! `&'dom S` (blanket impls make any `&S` a view when `S` implements the
//! style traits) or a small `Copy` wrapper projecting references into host
//! storage. Because the handle carries the tree lifetime, the view is
//! **not** tied to a borrow of the handle: it can be held across child
//! recursion, stored in a local, and re-fetched at will. One view type
//! serves every algorithm; each entry point narrows it with the style-trait
//! bounds it actually needs (e.g.
//! `N::Style: FlexContainerStyle + FlexItemStyle` for Flexbox), so hosts
//! implement the style traits once, on one type.
//!
//! One discipline note: accessors returning **borrowed values** — the
//! `LengthPercentage`-family geometry properties (`Edges<&Margin>`,
//! `Size<&StyleSize>`, `&FlexBasis`, …) and the borrowed sequences (grid
//! templates, font families) — borrow from the view **value**, so bind the
//! view first — `let style = node.style();` — before reading. Never return
//! a style-derived borrow or iterator from a helper. Consequently a view
//! cannot lend values it synthesizes on the fly: a host that computes style
//! per call must materialize the computed values in per-node storage (once
//! per style change) and lend references from there.
//!
//! # Static dispatch — no `dyn`
//!
//! Engine entry points are generic over the concrete handle type and provide
//! no erased fallback. `LayoutNode` is structurally dyn-incompatible (a
//! `Copy` supertrait plus associated types without defaults), so host⇄engine
//! calls monomorphize and can inline.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

use crate::geometry::Point;
use crate::style::CoreStyle;

/// A copy-free node handle borrowed from the host's tree for one layout
/// epoch.
///
/// Implement this on a cheap `Copy` value that can reach both the node's
/// immutable data (topology, style) and its interior-mutable
/// layout slots: a plain `&'dom Node`, a `(&'dom Tree, index)` pair, or an
/// equivalent wrapper. Keep handles at most two words — they are stored in
/// per-item scratch by every algorithm.
///
/// All immutable accessors must be cheap and repeatable: algorithms
/// re-fetch style views and re-iterate children several times per pass
/// instead of caching them in scratch structs.
pub trait LayoutNode: Copy + core::fmt::Debug {
    /// The borrowed style view handed to the generic machinery — typically
    /// `&'dom S` for a host style type `S`, or a lazily-translating `Copy`
    /// wrapper. One type serves every algorithm; entry points bound it with
    /// the container/item style traits they need.
    type Style: CoreStyle;

    /// Iterator over this node's children, in **document order** (the order
    /// style `order` reordering starts from).
    type ChildIter: Iterator<Item = Self>;

    /// Iterates this node's children in document order.
    fn children(self) -> Self::ChildIter;

    /// Number of children. Used for scratch preallocation; override the
    /// counting default when the host knows the count in O(1).
    fn child_count(self) -> usize {
        self.children().count()
    }

    /// The style view of this node (see [`Style`](Self::Style)).
    fn style(self) -> Self::Style;

    /// Lays out or measures this node, returning the result to the caller —
    /// the host's **display dispatch**, and the point where layout
    /// recursion round-trips through the host.
    ///
    /// Implementations must handle a non-generated box (`display: none`)
    /// first — call [`hide_subtree`](crate::compute::hide_subtree) and
    /// return [`LayoutOutput::HIDDEN`] **before** consulting the cache —
    /// then wrap visible-node routing in
    /// [`compute_cached_layout`](crate::compute::compute_cached_layout).
    /// Routing must be deterministic between cache clears.
    ///
    /// Re-entrancy contract: the implementation must not hold any per-node
    /// slot borrow across the engine algorithm it routes to (the algorithm
    /// recurses back into other nodes' `compute_child_layout`). Borrows
    /// needed by a leaf measurer are node-scoped and end before the cache
    /// wrapper stores the result.
    fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput;

    /// Stores the durable, unrounded layout of this node.
    ///
    /// Called by algorithms for each child they position and by the root
    /// entry point. Hosts retain this copy for incremental relayout and let
    /// [`round_layout`](crate::compute::round_layout) derive pixel-snapped
    /// output separately.
    fn set_unrounded_layout(self, layout: &Layout);

    /// The unrounded layout previously stored via
    /// [`set_unrounded_layout`](Self::set_unrounded_layout). Read by the
    /// rounding pass.
    fn unrounded_layout(self) -> Layout;

    /// Stores the final, pixel-snapped layout of this node, written by
    /// [`round_layout`](crate::compute::round_layout). The implementation
    /// chooses the target store — it may be a different store than the
    /// unrounded copy (e.g. the paint-facing side of a widget tree).
    fn set_final_layout(self, layout: &Layout);

    /// Records the CSS static position of an out-of-flow node whose
    /// containing block is elsewhere
    /// ([`PositionProperty::Fixed`](crate::style::PositionProperty)).
    ///
    /// During a [`LayoutGoal::Commit`] run this is the origin of the node's
    /// hypothetical margin box in its formatting parent's border-box space.
    /// The host later converts it into the real containing block's
    /// padding-box space for the positioned pass.
    fn set_static_position(self, static_position: Point<f32>);

    /// Looks up a previously-stored output usable for `input`.
    ///
    /// The **complete [`LayoutInput`] is the cache key**; a stored entry may
    /// satisfy a request only under the provable equivalences documented in
    /// [`cache`](crate::cache). Hosts typically embed one
    /// [`Cache`](crate::cache::Cache) per node in an interior-mutable slot
    /// and delegate. A host that deliberately disables caching returns
    /// `None` here and makes the write/clear methods no-ops — all three
    /// methods are required so a partially-cached host (which would break
    /// the [`hide_subtree`](crate::compute::hide_subtree) invalidation
    /// invariant) cannot compile.
    ///
    /// Invalidation is the host's job: when a node's style, content, or
    /// children change, clear it **and every ancestor up to its relayout
    /// root** before the next flush — cached entries encode children's
    /// contributions.
    fn cache_get(self, input: LayoutInput) -> Option<LayoutOutput>;

    /// Stores an output computed for `input` (see
    /// [`cache_get`](Self::cache_get)).
    fn cache_store(self, input: LayoutInput, output: LayoutOutput);

    /// Drops every cached entry of this node (see
    /// [`cache_get`](Self::cache_get)).
    fn cache_clear(self);
}
