//! **neutron-star** — a trait-first, statically-dispatched CSS **flexbox**
//! engine for host-owned trees, with CSS **Grid** host contracts reserved for
//! its next algorithm milestone.
//!
//! Built as lynx-vello's from-scratch successor to the Lynx C++ engine's
//! `starlight`, but deliberately Lynx-agnostic and standalone-publishable:
//! zero required dependencies, no assumptions about the host's DOM, style
//! engine, or storage.
//!
//! # Architecture
//!
//! The engine owns **algorithms and vocabulary**; the host owns **the tree,
//! the styles, and all storage**:
//!
//! ```text
//!            host owns                          engine owns
//!  ┌───────────────────────────┐   traits    ┌───────────────────────────┐
//!  │ immutable source:         │◀───────────▶│ compute_root_layout       │
//!  │ · topology + styles       │  NodeId +   │ compute_leaf_layout       │
//!  │ mutable session:          │  POD values │ cache/hide/abs-pos/round  │
//!  │ · layouts/cache/dispatch  │◀───────────▶│ flex algo; grid contracts │
//!  └───────────────────────────┘  recursion  └───────────────────────────┘
//! ```
//!
//! - [`tree`] — the tree protocol: [`NodeId`](tree::NodeId), traversal, style views, the layout
//!   wire format ([`LayoutInput`](tree::LayoutInput)/[`LayoutOutput`](tree::LayoutOutput)/
//!   [`Layout`](tree::Layout)), caching and rounding capabilities, and the **recursion contract**
//!   (start there).
//! - [`style`] — the style protocol: engine-owned value types plus the `CoreStyle`/container/item
//!   traits hosts implement as cheap views over their computed styles.
//! - [`compute`] — the machinery entry points hosts call from their dispatch (root, cache wrapper,
//!   subtree hiding, leaf, the positioned pass, rounding), including the canonical dispatch
//!   skeleton and the implemented flexbox entry point. Grid layout remains the L2 milestone.
//! - [`cache`] — the embeddable per-node measurement cache and its matching contract.
//! - [`geometry`] — `Copy`/`#[repr(C)]` geometry primitives.
//!
//! Layout recursion round-trips through the host's
//! [`compute_child_layout`](tree::LayoutSession::compute_child_layout) on
//! every node. The immutable [`LayoutSource`](tree::LayoutSource) is passed
//! separately from the mutable session, so borrowed computed-style views can
//! remain live across recursion. The host routes each node to a neutron-star
//! algorithm or to its own (Lynx's non-CSS `linear` and `relative` modes are
//! ordinary peer algorithms in the host, invisible to this crate).
//!
//! # No `dyn`, by construction
//!
//! Every host boundary is generic. Source/measurement protocols use GATs
//! (borrowed iterators, style views, and measurement views), while mutable
//! capability traits explicitly require `Sized`; none can be erased to a
//! trait object, and every engine⇄host call monomorphizes and can inline.
//! There is no erased fallback and none is planned:
//!
//! ```compile_fail
//! // GAT-based protocols cannot be made into trait objects:
//! fn erased(tree: &dyn neutron_star::tree::TraverseTree) {}
//! // Mutable protocol capabilities are also explicitly Sized:
//! fn erased_state(state: &mut dyn neutron_star::tree::LayoutState) {}
//! ```
//!
//! # Status: flexbox implemented (milestone L1)
//!
//! The generic protocol and machinery are implemented together with the CSS
//! Flexbox Level 1 algorithm. The Grid style/tree contracts are final-shaped,
//! but their algorithm remains L2. See `docs/layout-architecture.md` in the
//! lynx-vello repository for the design rationale and remaining milestones.
//!
//! # Dependencies and feature flags
//!
//! None, deliberately: the flex and grid protocols are core, unconditional
//! API, and the crate compiles with zero dependencies.
//!
//! # Minimal host sketch
//!
//! A slab-backed host implementing the core protocol (style traits via the
//! blanket `&S` impls):
//!
//! ```
//! use neutron_star::prelude::*;
//! use neutron_star::style::CalcHandle;
//!
//! #[derive(Default)]
//! struct Style; // your computed-style type
//! impl CoreStyle for Style {} // CSS initial values from the defaults
//!
//! struct SourceNode {
//!     style: Style,
//!     children: Vec<NodeId>,
//! }
//!
//! struct Source {
//!     nodes: Vec<SourceNode>,
//! }
//!
//! impl Source {
//!     fn node(&self, id: NodeId) -> &SourceNode {
//!         &self.nodes[usize::from(id)]
//!     }
//! }
//!
//! impl TraverseTree for Source {
//!     type ChildIter<'a> = std::iter::Copied<std::slice::Iter<'a, NodeId>>;
//!
//!     fn child_ids(&self, parent: NodeId) -> Self::ChildIter<'_> {
//!         self.node(parent).children.iter().copied()
//!     }
//!
//!     fn child_count(&self, parent: NodeId) -> usize {
//!         self.node(parent).children.len()
//!     }
//!
//!     fn child_id(&self, parent: NodeId, index: usize) -> NodeId {
//!         self.node(parent).children[index]
//!     }
//! }
//!
//! impl LayoutSource for Source {
//!     type CoreStyle<'a> = &'a Style;
//!
//!     fn core_style(&self, node: NodeId) -> Self::CoreStyle<'_> {
//!         &self.node(node).style
//!     }
//!
//!     fn resolve_calc(&self, _calc: CalcHandle, _basis: f32) -> f32 {
//!         unreachable!("this host's styles never carry calc()")
//!     }
//! }
//!
//! struct Session {
//!     layouts: Vec<Layout>,
//! }
//!
//! impl LayoutState for Session {
//!     fn set_unrounded_layout(&mut self, node: NodeId, layout: &Layout) {
//!         self.layouts[usize::from(node)] = *layout;
//!     }
//!
//!     fn set_static_position(&mut self, child: NodeId, static_position: Point<f32>) {
//!         // This toy has no hoisted out-of-flow nodes; real hosts store
//!         // this for the positioned pass (compute_absolute_layout).
//!         let _ = (child, static_position);
//!     }
//! }
//!
//! impl CacheState for Session {
//!     fn cache_get(&self, _: NodeId, _: LayoutInput) -> Option<LayoutOutput> {
//!         None
//!     }
//!     fn cache_store(&mut self, _: NodeId, _: LayoutInput, _: LayoutOutput) {}
//!     fn cache_clear(&mut self, _: NodeId) {}
//! }
//!
//! impl LayoutSession<Source> for Session {
//!     fn compute_child_layout(
//!         &mut self,
//!         source: &Source,
//!         child: NodeId,
//!         input: LayoutInput,
//!     ) -> LayoutOutput {
//!         // Real hosts handle display:none before compute_cached_layout,
//!         // then dispatch visible nodes (see the `compute` module docs).
//!         // This toy treats every node as an empty visible leaf:
//!         let _ = source.core_style(child);
//!         LayoutOutput::new(input.known_dimensions.unwrap_or(Size::ZERO), Size::ZERO)
//!     }
//! }
//!
//! let source = Source {
//!     nodes: vec![SourceNode {
//!         style: Style,
//!         children: vec![],
//!     }],
//! };
//! let mut session = Session {
//!     layouts: vec![Layout::default()],
//! };
//! let root = NodeId::from(0_usize);
//! let output = session.compute_child_layout(&source, root, LayoutInput::default());
//! assert_eq!(output.size, Size::ZERO);
//! ```

pub mod cache;
pub mod compute;
pub mod geometry;
pub mod style;
pub mod tree;

/// One-stop imports for implementing a host: every protocol trait plus the
/// types that appear in their signatures.
///
/// Value-type vocabulary that only appears *inside* style accessors
/// (`Dimension`, alignment enums, grid track types, …) is not re-exported
/// here — pull it from [`style`] as needed.
pub mod prelude {
    pub use crate::compute::{
        FnLeafMeasurer, LeafMeasureInput, LeafMeasurement, LeafMeasurer, LeafMetrics,
    };
    pub use crate::geometry::{Edges, Line, Point, Size};
    pub use crate::style::{
        CoreStyle, FlexContainerStyle, FlexItemStyle, GridContainerStyle, GridItemStyle,
        GridTemplateRepetition,
    };
    pub use crate::tree::{
        AvailableSpace, CacheState, FlexSource, GridSource, Layout, LayoutGoal, LayoutInput,
        LayoutOutput, LayoutSession, LayoutSource, LayoutState, NodeId, RequestedAxis, RoundState,
        SizingMode, TraverseTree,
    };
}
