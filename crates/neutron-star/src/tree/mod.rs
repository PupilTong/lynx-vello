//! The tree protocol: how the engine sees the host's node tree.
//!
//! neutron-star owns **no tree**. The host keeps nodes in whatever storage it
//! likes (a slab, an arena, an ECS, a retained DOM) and exposes them through
//! the traits here, addressed by opaque [`NodeId`]s. The traits form a
//! deliberate hierarchy of *capability*, so each engine function demands only
//! what it uses:
//!
//! ```text
//!  TraverseTree            child access               (rounding, debug)
//!      └── LayoutTree      + style views, layout IO   (all algorithms)
//!           ├── FlexTree   + flex style views         (flex algorithm, L1)
//!           └── GridTree   + grid style views         (grid algorithm, L2)
//!  CacheTree               measurement cache slots    (compute_cached_layout)
//!  RoundTree: TraverseTree unrounded → final layouts  (round_layout)
//! ```
//!
//! # The recursion contract
//!
//! Layout recursion deliberately round-trips through the host:
//!
//! ```text
//!  compute_root_layout(tree, root, …)
//!      └─▶ tree.compute_child_layout(root, input)           [host dispatch]
//!            └─▶ <algorithm>(tree, root, input)             [engine algo]
//!                  ├─▶ tree.compute_child_layout(child, …)  [host dispatch]
//!                  │     └─▶ … per that child's display …
//!                  └─▶ tree.set_unrounded_layout(child, …)
//! ```
//!
//! [`LayoutTree::compute_child_layout`] is the **dispatch point**: the host
//! inspects the child's `display` (which the engine deliberately has no enum
//! for) and routes to an engine algorithm entry point (flexbox is implemented
//! as a `fn(&mut Tree, NodeId, LayoutInput) -> LayoutOutput` in
//! [`compute`](crate::compute); grid follows in L2),
//! [`compute_leaf_layout`](crate::compute::compute_leaf_layout) (text/images
//! via its own measure closure), or *its own algorithm* — this is exactly how
//! lynx-vello's non-CSS `display: linear`/`display: relative` modes plug in
//! as peer algorithms. The host wraps the routing in
//! [`compute_cached_layout`](crate::compute::compute_cached_layout) so every
//! path shares one cache policy. See the `compute` module docs for the
//! canonical dispatch skeleton.
//!
//! Because recursion passes `&mut self` back and forth, the protocol is
//! single-threaded per tree by construction; the planned intra-layout
//! parallelism protocol (batched child requests, see the architecture doc)
//! will be additive.
//!
//! # Object safety — deliberately absent
//!
//! Every trait here uses generic-associated-type iterators or style views,
//! which makes them structurally **not object-safe**: `dyn LayoutTree` is a
//! compile error, not a style-guide rule. The entire host⇄engine boundary
//! monomorphizes and can inline.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutInput, LayoutOutput, RequestedAxis, RunMode, SizingMode,
};

use crate::geometry::Point;
use crate::style::value::CalcHandle;
use crate::style::{
    CoreStyle, FlexContainerStyle, FlexItemStyle, GridContainerStyle, GridItemStyle,
};

/// An opaque node handle, chosen by the host.
///
/// The engine never fabricates ids and attaches no meaning to the bits —
/// hosts encode slab indices, generational indices, or pointers as they see
/// fit. `u64` gives generational schemes room (index + generation) without
/// pointer-width games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct NodeId(u64);

impl NodeId {
    /// Wraps a host-chosen id.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the host-chosen id back.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for NodeId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<usize> for NodeId {
    fn from(id: usize) -> Self {
        Self(id as u64)
    }
}

impl From<NodeId> for u64 {
    fn from(id: NodeId) -> Self {
        id.0
    }
}

impl From<NodeId> for usize {
    /// Truncates on 32-bit targets; hosts using `usize` indices round-trip
    /// losslessly.
    fn from(id: NodeId) -> Self {
        #[allow(clippy::cast_possible_truncation)]
        {
            id.0 as Self
        }
    }
}

/// Read-only child access — the minimum capability, enough for rounding and
/// debug passes.
///
/// The children of a node are an ordered list in **document order** (the
/// order style `order` reordering starts from). All methods must be cheap
/// and repeatable: algorithms iterate children several times per pass.
pub trait TraverseTree {
    /// Borrowed iterator over a node's children, in document order.
    type ChildIter<'a>: Iterator<Item = NodeId>
    where
        Self: 'a;

    /// Iterates `parent`'s children in document order.
    fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_>;

    /// Number of children of `parent`.
    fn child_count(&self, parent: NodeId) -> usize;

    /// The `index`-th child (document order) of `parent`.
    ///
    /// Callers only pass indices `< child_count(parent)`.
    fn child_id(&self, parent: NodeId, index: usize) -> NodeId;
}

/// The core layout capability: style views, calc resolution, layout storage,
/// and the child-layout dispatch callback.
pub trait LayoutTree: TraverseTree {
    /// The borrowed style view handed to the generic machinery.
    ///
    /// Typically `&'a HostStyle` (blanket impls make any `&S` a view when
    /// `S: CoreStyle`) or a small wrapper struct translating host storage on
    /// each accessor.
    type CoreStyle<'a>: CoreStyle
    where
        Self: 'a;

    /// The style view of `node`.
    fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_>;

    /// Resolves a host-owned `calc()` expression against a percentage
    /// `basis` (in CSS pixels), returning CSS pixels.
    ///
    /// Called whenever an algorithm resolves a style value carrying a
    /// [`CalcHandle`]. Hosts whose styles never produce `Calc` values may
    /// implement this as `unreachable!()`.
    fn resolve_calc(&self, calc: CalcHandle, basis: f32) -> f32;

    /// Stores the durable layout of `node`.
    ///
    /// Called by algorithms for each child they position (and by
    /// [`compute_root_layout`](crate::compute::compute_root_layout) for the
    /// root). The values are **unrounded**; the host must keep them as-is
    /// for incremental relayout and let
    /// [`round_layout`](crate::compute::round_layout) derive the
    /// pixel-snapped copy through [`RoundTree`].
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout);

    /// Records the CSS **static position** of an out-of-flow child whose
    /// containing block is elsewhere
    /// ([`Position::AbsoluteHoisted`](crate::style::Position)).
    ///
    /// Called by the parent's algorithm during a `PerformLayout` run for
    /// each such child: `static_position` is the origin of the child's
    /// hypothetical margin box per the parent's formatting context (Flexbox
    /// §4.1 sole-item alignment; Grid §10.1 content-edge area), relative to
    /// the parent's border box — the same space as [`Layout::location`].
    /// The parent does *not* size or place the child.
    ///
    /// The host stores the value, converts it into containing-block space
    /// once in-flow layout is done (all unrounded layouts are available by
    /// then), and passes it to
    /// [`compute_absolute_layout`](crate::compute::compute_absolute_layout)
    /// in the positioned pass.
    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>);

    /// Invalidates any cached layout answers for `node` before hidden
    /// layout overwrites durable geometry.
    ///
    /// Hosts without caching may keep the default no-op. Hosts implementing
    /// [`CacheTree`] must delegate this hook to [`CacheTree::cache_clear`]: a
    /// later cache hit may otherwise restore only a subtree root's output
    /// while its descendants remain zeroed from `display:none`.
    fn invalidate_layout_cache(&mut self, node: NodeId) {
        let _ = node;
    }

    /// Lays out (or measures) `child`, returning its output — **the host
    /// dispatch point** (see the module docs for the contract and the
    /// `compute` module docs for the canonical skeleton).
    ///
    /// Implementations must:
    /// - handle [`BoxGenerationMode::None`](crate::style::BoxGenerationMode) and
    ///   [`RunMode::PerformHiddenLayout`] via
    ///   [`compute_hidden_layout`](crate::compute::compute_hidden_layout),
    /// - route other nodes by their display mode to an engine algorithm, a host algorithm, or leaf
    ///   measurement,
    /// - wrap the routing in [`compute_cached_layout`](crate::compute::compute_cached_layout),
    /// - be deterministic for identical inputs between cache clears.
    fn compute_child_layout(&mut self, child: NodeId, input: LayoutInput) -> LayoutOutput;
}

/// Adds flexbox style views — the tree bound of
/// [`compute_flexbox_layout`](crate::compute::compute_flexbox_layout).
pub trait FlexTree: LayoutTree {
    /// Borrowed flex-container style view.
    type ContainerStyle<'a>: FlexContainerStyle
    where
        Self: 'a;
    /// Borrowed flex-item style view.
    type ItemStyle<'a>: FlexItemStyle
    where
        Self: 'a;

    /// The flex-container style view of `container`.
    fn flex_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_>;

    /// The flex-item style view of `item` (a child of a flex container).
    fn flex_item_style(&self, item: NodeId) -> Self::ItemStyle<'_>;
}

/// Adds grid style views — the tree bound of the grid algorithm entry point
/// (`compute_grid_layout`, specified in the architecture doc and landing in
/// L2).
pub trait GridTree: LayoutTree {
    /// Borrowed grid-container style view.
    type ContainerStyle<'a>: GridContainerStyle
    where
        Self: 'a;
    /// Borrowed grid-item style view.
    type ItemStyle<'a>: GridItemStyle
    where
        Self: 'a;

    /// The grid-container style view of `container`.
    fn grid_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_>;

    /// The grid-item style view of `item` (a child of a grid container).
    fn grid_item_style(&self, item: NodeId) -> Self::ItemStyle<'_>;
}

/// Host-owned measurement/layout caching, consulted by
/// [`compute_cached_layout`](crate::compute::compute_cached_layout).
///
/// Hosts typically embed one [`Cache`](crate::cache::Cache) per node and
/// delegate these methods to it; the trait exists so storage stays
/// host-chosen (structure-of-arrays hosts can pack cache slots columnar).
///
/// # The key is the complete [`LayoutInput`]
///
/// Every field of the input can change the output, so none may be dropped
/// from matching: `sizing_mode` toggles whether the node's own
/// `size`/`min`/`max`/`aspect-ratio` apply, `parent_size` is the percentage
/// basis, and `requested_axis` scopes which axes of a `ComputeSize` answer
/// were actually computed. A matching policy may treat a stored entry as
/// usable for a request only under *provable* equivalences (see
/// [`cache`](crate::cache) for the contract) — never across differing
/// `sizing_mode` or percentage bases.
///
/// # Invalidation is the host's job
///
/// The engine only ever *reads* and *fills* slots. When a node's style,
/// content, or children change, the host must [`cache_clear`] it **and every
/// ancestor up to its relayout root** before the next layout — cached
/// entries encode children's contributions. A caching host must also
/// implement [`LayoutTree::invalidate_layout_cache`] by delegating to
/// [`cache_clear`]; hidden layout invokes that hook for every descendant so
/// later cache hits cannot leave zeroed geometry behind.
///
/// [`cache_clear`]: CacheTree::cache_clear
pub trait CacheTree {
    /// Looks up a previously-stored output usable for `input`.
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput>;

    /// Stores an output computed for `input`.
    fn cache_store(&mut self, node: NodeId, input: LayoutInput, layout_output: LayoutOutput);

    /// Drops every cached entry of `node`.
    fn cache_clear(&mut self, node: NodeId);
}

/// Read/write access for the pixel-snapping pass
/// ([`round_layout`](crate::compute::round_layout)).
///
/// Kept separate from [`LayoutTree`] so the rounded copy can live in a
/// different store (e.g. the render tree) than the unrounded layout the
/// engine needs for stable incremental relayout.
pub trait RoundTree: TraverseTree {
    /// The unrounded layout previously stored via
    /// [`LayoutTree::set_unrounded_layout`].
    fn unrounded_layout(&self, node: NodeId) -> Layout;

    /// Stores the final, pixel-snapped layout of `node`.
    fn set_final_layout(&mut self, node: NodeId, layout: &Layout);
}
