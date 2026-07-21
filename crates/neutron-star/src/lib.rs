#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! **neutron-star** — a trait-first, statically-dispatched Flexbox, Grid, and
//! Starlight Linear/Relative engine for host-owned trees.
//!
//! Built as lynx-vello's from-scratch successor to the Lynx C++ engine's
//! `starlight`. The engine owns no tree and no styles, but it **speaks
//! stylo's computed-value vocabulary**: style accessors return the lynx
//! stylo fork's computed types directly, so a stylo-backed host serves style
//! views without any translation layer. (The former zero-dependency pillar
//! is retired; `stylo` is a required dependency.)
//!
//! # Architecture
//!
//! The engine owns **algorithms and vocabulary**; the host owns **the tree,
//! the styles, and all storage**. The host hands the engine `Copy` **node
//! handles** borrowed from its tree — the same shape stylo demands of its
//! DOM — and the engine lays out through them copy-free:
//!
//! ```text
//!            host owns                          engine owns
//!  ┌───────────────────────────┐  LayoutNode ┌───────────────────────────┐
//!  │ the tree:                 │◀───────────▶│ compute_root_layout       │
//!  │ · topology + styles       │   handles + │ compute_leaf_layout       │
//!  │ · interior-mutable        │  POD values │ cache/hide/abs-pos/round  │
//!  │   layout/cache slots      │◀───────────▶│ flex/grid/linear/relative │
//!  └───────────────────────────┘  recursion  └───────────────────────────┘
//! ```
//!
//! - [`tree`] — the tree protocol: the [`LayoutNode`](tree::LayoutNode) handle (traversal, style
//!   views, dispatch, layout/cache slots), the layout wire format
//!   ([`LayoutInput`](tree::LayoutInput)/[`LayoutOutput`](tree::LayoutOutput)/
//!   [`Layout`](tree::Layout)), and the **recursion contract** (start there).
//! - [`style`] — the style protocol: the `CoreStyle`/container/item traits hosts implement as cheap
//!   views over their computed styles, speaking stylo computed values re-exported there.
//! - [`compute`] — the machinery entry points hosts call from their dispatch (root, cache wrapper,
//!   subtree hiding, leaf, the positioned pass, rounding), the canonical dispatch skeleton, and the
//!   implemented Flexbox, Grid, Linear, and Relative entry points.
//! - [`cache`] — the embeddable per-node measurement cache and its matching contract.
//! - [`invalidate`] — containment-bounded, damage-driven cache invalidation (relayout-boundary
//!   detection and the ancestor-walking invalidator).
//! - [`geometry`] — `Copy`/`#[repr(C)]` geometry primitives.
//!
//! Layout recursion round-trips through the host's
//! [`compute_child_layout`](tree::LayoutNode::compute_child_layout) on every
//! node. Handles and the style views borrowed through them stay valid across
//! recursion because all mutation flows into host-owned interior-mutable
//! per-node slots — the protocol has no `&mut` anywhere. The host routes
//! each node to a neutron-star algorithm or to its own additional layout
//! mode. Starlight's non-CSS `display: linear` and relative layout are
//! first-class algorithms in this crate.
//!
//! # No `dyn`, by construction
//!
//! Every host boundary is generic over the concrete handle type.
//! [`LayoutNode`](tree::LayoutNode) is structurally dyn-incompatible (a
//! `Copy` supertrait plus associated types without defaults), so every
//! engine⇄host call monomorphizes and can inline. There is no erased
//! fallback and none is planned:
//!
//! ```compile_fail
//! // The Copy supertrait and associated types keep the protocol static:
//! fn erased(node: &dyn neutron_star::tree::LayoutNode) {}
//! ```
//!
//! # Status: Flexbox, Grid, Linear, Relative, and text measurement implemented
//!
//! The generic protocol and machinery are implemented together with CSS
//! Flexbox Level 1, numeric CSS Grid Level 2 (excluding subgrid and named
//! areas), Starlight Linear layout, Starlight Relative Layout Level 1, and an
//! optional Parley shaping/line-breaking measurement core.
//! See `docs/layout-architecture.md` in the lynx-vello repository for the
//! design rationale, represented conformance surface, and remaining parity
//! milestones.
//!
//! # Dependencies and feature flags
//!
//! The style protocol requires the workspace's `stylo` fork (building it
//! needs the vendored submodule and `python3` for stylo's build script; a
//! cold build takes minutes). Default builds additionally enable the `text`
//! feature and its optional Parley dependency; `default-features = false`
//! keeps the protocol and box-layout core only.
//!
//! # Minimal host sketch
//!
//! A slab-backed host implementing the protocol (style traits via the
//! blanket `&S` impls; per-node layout slots as `RefCell`s):
//!
//! ```
//! use std::cell::RefCell;
//!
//! use neutron_star::prelude::*;
//! use neutron_star::style::Display;
//!
//! #[derive(Default)]
//! struct Style; // your computed-style type
//! impl CoreStyle for Style {
//!     // Every other accessor defaults to the fork's initial value.
//!     fn display(&self) -> Display {
//!         Display::Flex
//!     }
//! }
//!
//! struct Node {
//!     style: Style,
//!     children: Vec<usize>,
//!     // Host-owned interior-mutable layout slots, written through handles.
//!     layout: RefCell<Layout>,
//!     final_layout: RefCell<Layout>,
//! }
//!
//! struct Tree {
//!     nodes: Vec<Node>,
//! }
//!
//! /// The Copy node handle: a borrow of the tree plus a slab index.
//! /// (A `Box`-per-node host would use `&'dom Node` directly.)
//! #[derive(Clone, Copy)]
//! struct NodeRef<'dom> {
//!     tree: &'dom Tree,
//!     index: usize,
//! }
//!
//! // Identify the node, not the whole tree, in debug output.
//! impl std::fmt::Debug for NodeRef<'_> {
//!     fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         formatter.debug_tuple("NodeRef").field(&self.index).finish()
//!     }
//! }
//!
//! impl<'dom> NodeRef<'dom> {
//!     fn node(self) -> &'dom Node {
//!         &self.tree.nodes[self.index]
//!     }
//! }
//!
//! struct Children<'dom> {
//!     tree: &'dom Tree,
//!     ids: std::slice::Iter<'dom, usize>,
//! }
//!
//! impl<'dom> Iterator for Children<'dom> {
//!     type Item = NodeRef<'dom>;
//!
//!     fn next(&mut self) -> Option<NodeRef<'dom>> {
//!         let index = *self.ids.next()?;
//!         Some(NodeRef {
//!             tree: self.tree,
//!             index,
//!         })
//!     }
//! }
//!
//! impl<'dom> LayoutNode for NodeRef<'dom> {
//!     type Style = &'dom Style;
//!     type ChildIter = Children<'dom>;
//!
//!     fn children(self) -> Children<'dom> {
//!         Children {
//!             tree: self.tree,
//!             ids: self.node().children.iter(),
//!         }
//!     }
//!
//!     fn child_count(self) -> usize {
//!         self.node().children.len()
//!     }
//!
//!     fn style(self) -> &'dom Style {
//!         &self.node().style
//!     }
//!
//!     fn compute_child_layout(self, input: LayoutInput) -> LayoutOutput {
//!         // Real hosts route on display: handle display:none with
//!         // hide_subtree (and content-visibility skipping via skips_contents
//!         // → compute_skipped_contents_layout) before compute_cached_layout,
//!         // then dispatch visible nodes inside it (see the `compute` module
//!         // docs). This toy treats every node as an empty visible leaf:
//!         let _ = self.style();
//!         LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
//!     }
//!
//!     fn set_unrounded_layout(self, layout: Layout) {
//!         *self.node().layout.borrow_mut() = layout;
//!     }
//!
//!     fn with_unrounded_layout<R>(self, read: impl FnOnce(&Layout) -> R) -> R {
//!         let layout = self.node().layout.borrow();
//!         read(&layout)
//!     }
//!
//!     fn set_final_layout(self, layout: Layout) {
//!         *self.node().final_layout.borrow_mut() = layout;
//!     }
//!
//!     fn set_static_position(self, static_position: Point<f32>) {
//!         // This toy has no hoisted out-of-flow nodes; real hosts store
//!         // this for the positioned pass (compute_absolute_layout).
//!         let _ = static_position;
//!     }
//!
//!     // Caching deliberately disabled; real hosts embed one
//!     // `RefCell<neutron_star::cache::Cache>` per node and delegate.
//!     fn cache_get(self, _input: LayoutInput) -> Option<LayoutOutput> {
//!         None
//!     }
//!
//!     fn cache_store(self, _input: LayoutInput, _output: LayoutOutput) {}
//!
//!     fn cache_clear(self) {}
//! }
//!
//! let tree = Tree {
//!     nodes: vec![Node {
//!         style: Style,
//!         children: vec![],
//!         layout: RefCell::new(Layout::default()),
//!         final_layout: RefCell::new(Layout::default()),
//!     }],
//! };
//! let root = NodeRef {
//!     tree: &tree,
//!     index: 0,
//! };
//! let output = root.compute_child_layout(LayoutInput::default());
//! assert_eq!(output.size, Size::ZERO);
//! ```

pub mod cache;
pub mod compute;
pub mod geometry;
pub mod invalidate;
pub mod style;
#[cfg(feature = "text")]
pub mod text;
pub mod tree;

/// One-stop imports for implementing a host: the node-handle trait plus the
/// types that appear in its signatures.
///
/// Value-type vocabulary that only appears *inside* style accessors (stylo
/// computed sizes, alignment wrappers, grid track types, …) is not
/// re-exported here — pull it from [`style`] (or the stylo crate) as needed.
pub mod prelude {
    pub use crate::compute::{LeafMeasureInput, LeafMetrics, NaturalSize};
    pub use crate::geometry::{Edges, Line, Point, Size};
    pub use crate::style::{
        CoreStyle, FlexContainerStyle, FlexItemStyle, GridContainerStyle, GridItemStyle,
        LinearContainerStyle, LinearItemStyle, RelativeContainerStyle, RelativeItemStyle,
        TextContainerStyle, TextRunStyle,
    };
    pub use crate::tree::{
        AvailableSpace, Layout, LayoutGoal, LayoutInput, LayoutNode, LayoutOutput, RequestedAxis,
        SizingMode,
    };
}
