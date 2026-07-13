//! The tree protocol: how the engine sees the host's node tree.
//!
//! neutron-star owns **no tree or layout store**. The host exposes immutable
//! topology/style through a source object and mutable layout/cache storage
//! through a separate session object, both addressed by opaque [`NodeId`]s.
//! The traits form a deliberate hierarchy of capability, so each engine
//! function demands only what it uses:
//!
//! ```text
//!  TraverseTree                       child access
//!      └── LayoutSource               + core style views / calc resolution
//!           ├── FlexSource            + flex style views
//!           ├── GridSource            + grid style views
//!           ├── LinearSource          + linear style views
//!           └── RelativeSource        + relative style views
//!  LayoutState                        unrounded layout / static-position writes
//!  CacheState                         measurement and commit cache slots
//!      └── LayoutSession<Source>       + host display dispatch
//!  RoundState                         unrounded reads → final-layout writes
//! ```
//!
//! # The recursion contract
//!
//! Layout recursion deliberately round-trips through the host session while
//! the source remains immutably borrowed:
//!
//! ```text
//!  compute_root_layout(source, session, root, …)
//!      └─▶ session.compute_child_layout(source, root, input)          [host dispatch]
//!            └─▶ <algorithm>(source, session, root, input)            [engine algo]
//!                  ├─▶ session.compute_child_layout(source, child, …) [host dispatch]
//!                  │     └─▶ … per that child's display …
//!                  └─▶ session.set_unrounded_layout(child, …)
//! ```
//!
//! [`LayoutSession::compute_child_layout`] is the **dispatch point**: the host
//! inspects the child's `display` (which the engine deliberately has no enum
//! for) and routes to an engine algorithm entry point (Flexbox, Grid, and
//! Starlight Linear and Relative are implemented in [`compute`](crate::compute)),
//! [`compute_leaf_layout`](crate::compute::compute_leaf_layout) (text/images
//! via a generic [`LeafMeasurer`](crate::compute::LeafMeasurer)), or a
//! host-provided additional algorithm. The host handles `display: none` first with
//! [`hide_subtree`](crate::compute::hide_subtree), then wraps visible-node
//! routing in [`compute_cached_layout`](crate::compute::compute_cached_layout)
//! so every sizing path shares one cache policy. See the `compute` module docs
//! for the canonical dispatch skeleton.
//!
//! A layout flush is an **immutable source epoch**: topology, computed style,
//! display dispatch inputs, and calc data must not change until the flush
//! finishes. Measurement may mutate session-owned text caches and layout
//! state, but not the source. The source and session must be two independent
//! Rust objects (normally backed by disjoint host fields); this lets a borrowed
//! GAT style view remain live while recursive layout mutably borrows only the
//! session.
//!
//! # Static dispatch — no `dyn`
//!
//! Engine entry points are generic over concrete source and session types and
//! provide no erased fallback. The source traits carry GAT iterators/style
//! views and are structurally not object-safe; host⇄engine calls therefore
//! monomorphize and can inline. Mutable storage/session traits explicitly
//! require `Sized`, so they cannot be substituted with `dyn` either.

mod io;

pub use io::{
    AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutOutput, RequestedAxis, SizingMode,
};

use crate::geometry::Point;
use crate::style::value::CalcHandle;
use crate::style::{
    CoreStyle, FlexContainerStyle, FlexItemStyle, GridContainerStyle, GridItemStyle,
    LinearContainerStyle, LinearItemStyle, RelativeContainerStyle, RelativeItemStyle,
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
pub trait TraverseTree: Sized {
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

/// Immutable topology and core-style access for one layout epoch.
///
/// A source must not change while a layout call is in progress. Its borrowed
/// style views may remain live across recursive calls because all layout,
/// cache, and measurement mutation goes through a separate [`LayoutSession`].
pub trait LayoutSource: TraverseTree {
    /// The borrowed style view handed to the generic machinery.
    ///
    /// Typically `&'a HostStyle` (blanket impls make any `&S` a view when
    /// `S: CoreStyle`) or a small wrapper translating host storage lazily.
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
}

/// Adds flexbox style views to an immutable [`LayoutSource`].
pub trait FlexSource: LayoutSource {
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

/// Adds Grid style views to an immutable [`LayoutSource`].
pub trait GridSource: LayoutSource {
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

/// Adds Starlight relative-layout style views to an immutable
/// [`LayoutSource`].
pub trait RelativeSource: LayoutSource {
    /// Borrowed relative-container style view.
    type ContainerStyle<'a>: RelativeContainerStyle
    where
        Self: 'a;
    /// Borrowed relative-item style view.
    type ItemStyle<'a>: RelativeItemStyle
    where
        Self: 'a;

    /// The relative-container style view of `container`.
    fn relative_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_>;

    /// The relative-item style view of `item` (a direct child of a relative
    /// container).
    fn relative_item_style(&self, item: NodeId) -> Self::ItemStyle<'_>;
}

/// Adds Starlight linear-container and linear-item style views to an
/// immutable [`LayoutSource`].
pub trait LinearSource: LayoutSource {
    /// Borrowed linear-container style view.
    type ContainerStyle<'a>: LinearContainerStyle
    where
        Self: 'a;
    /// Borrowed linear-item style view.
    type ItemStyle<'a>: LinearItemStyle
    where
        Self: 'a;

    /// The linear-container style view of `container`.
    fn linear_container_style(&self, container: NodeId) -> Self::ContainerStyle<'_>;

    /// The linear-item style view of `item` (a child of a linear container).
    fn linear_item_style(&self, item: NodeId) -> Self::ItemStyle<'_>;
}

/// Host-owned mutable layout output for the current layout epoch.
///
/// This state is a separate object from its [`LayoutSource`], so writing
/// geometry never invalidates a borrowed source style view.
pub trait LayoutState: Sized {
    /// Stores the durable, unrounded layout of `node`.
    ///
    /// Called by algorithms for each child they position and by the root
    /// entry point. Hosts retain this copy for incremental relayout and let
    /// [`round_layout`](crate::compute::round_layout) derive pixel-snapped
    /// output separately.
    fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout);

    /// Records the CSS static position of an out-of-flow child whose
    /// containing block is elsewhere
    /// ([`Position::AbsoluteHoisted`](crate::style::Position)).
    ///
    /// During a [`LayoutGoal::Commit`] run this is the origin of the child's
    /// hypothetical margin box in its formatting parent's border-box space.
    /// The host later converts it into the real containing block's padding-box
    /// space for the positioned pass.
    fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>);
}

/// Host-owned measurement/layout caching, consulted by
/// [`compute_cached_layout`](crate::compute::compute_cached_layout).
///
/// Hosts typically embed one [`Cache`](crate::cache::Cache) per node and
/// delegate these methods to it; the trait exists so storage stays
/// host-chosen (structure-of-arrays hosts can pack cache slots columnar).
/// A host that deliberately disables caching returns `None` from
/// [`cache_get`](Self::cache_get) and makes the write/clear methods no-ops.
///
/// # The key is the complete [`LayoutInput`]
///
/// Every field of the input can change the output, so none may be dropped
/// from matching: `goal` distinguishes committing layout from measurement
/// (and carries a measurement's requested axes), `sizing_mode` toggles whether the node's own
/// `size`/`min`/`max`/`aspect-ratio` apply, `definite_dimensions` distinguishes
/// decided geometry from a definite percentage basis, `parent_size` is the
/// parent percentage basis, and the remaining fields define the sizing constraints. A matching
/// policy may treat a stored entry as usable for a request only under
/// *provable* equivalences (see [`cache`](crate::cache) for the contract) —
/// never across differing `sizing_mode` or percentage bases. A committed
/// result may satisfy a compatible both-axis measurement, but a measurement
/// can never satisfy a commit.
///
/// # Invalidation is the host's job
///
/// The engine only ever reads, fills, and explicitly clears slots. When a node's style,
/// content, or children change, the host must [`cache_clear`] it **and every
/// ancestor up to its relayout root** before the next layout — cached
/// entries encode children's contributions. [`hide_subtree`](crate::compute::hide_subtree)
/// clears every descendant directly so later cache hits cannot leave zeroed
/// geometry behind.
///
/// [`cache_clear`]: CacheState::cache_clear
pub trait CacheState: Sized {
    /// Looks up a previously-stored output usable for `input`.
    fn cache_get(&self, node: NodeId, input: LayoutInput) -> Option<LayoutOutput>;

    /// Stores an output computed for `input`.
    fn cache_store(&mut self, node: NodeId, input: LayoutInput, layout_output: LayoutOutput);

    /// Drops every cached entry of `node`.
    fn cache_clear(&mut self, node: NodeId);
}

/// Mutable layout/cache state plus the host's open display dispatch.
///
/// `Source` and `Self` must be independent objects. Implementations inspect
/// the immutable source to handle `display: none` before caching, then route a
/// generated box to Flexbox, Grid, Linear, Relative, leaf measurement, or a
/// host-private algorithm.
/// The concrete source/session pair is statically dispatched; neutron-star
/// does not erase either side behind `dyn`.
pub trait LayoutSession<Source: LayoutSource>: LayoutState + CacheState {
    /// Lays out or measures `child`, returning the result to its parent.
    ///
    /// Implementations must call [`hide_subtree`](crate::compute::hide_subtree)
    /// and return [`LayoutOutput::HIDDEN`] for a non-generated box **before**
    /// consulting the cache. Visible-node routing is wrapped in
    /// [`compute_cached_layout`](crate::compute::compute_cached_layout) and
    /// must be deterministic between cache clears.
    fn compute_child_layout(
        &mut self,
        source: &Source,
        child: NodeId,
        input: LayoutInput,
    ) -> LayoutOutput;
}

/// Read/write access for the pixel-snapping pass.
///
/// Topology is supplied independently by a [`TraverseTree`]; this state only
/// reads the unrounded copy and writes the final copy, which may live in a
/// different host store.
pub trait RoundState: Sized {
    /// The unrounded layout previously stored via
    /// [`LayoutState::set_unrounded_layout`].
    fn unrounded_layout(&self, node: NodeId) -> Layout;

    /// Stores the final, pixel-snapped layout of `node`.
    fn set_final_layout(&mut self, node: NodeId, layout: &Layout);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn node_ids_round_trip_each_supported_host_integer() {
        let direct = NodeId::new(0xfedc_ba98_7654_3210);
        assert_eq!(direct.get(), 0xfedc_ba98_7654_3210);

        let from_u64 = NodeId::from(42_u64);
        assert_eq!(u64::from(from_u64), 42);

        let from_index = NodeId::from(17_usize);
        assert_eq!(usize::from(from_index), 17);
    }
}
