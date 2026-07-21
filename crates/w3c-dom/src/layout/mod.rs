//! Box layout over the document tree — the concrete [`neutron_star`] host.
//!
//! This module implements neutron-star's handle protocol on the crate's one
//! read handle: [`LayoutNode`](neutron_star::tree::LayoutNode) is
//! implemented **directly on `&Node<T>`** — the same one-word `Copy` value
//! the stylo `TNode`/`TElement` traits already use — so the engine traverses
//! the document itself, with no wrapper handle, no adapter objects, and no
//! engine-side tree or style copies. Flexbox, Grid, and Starlight
//! Linear/Relative containers dispatch to their neutron-star algorithms;
//! `display: none` subtrees are hidden; leaves are measured through the
//! embedder payload's [`MeasureLeaf`] hook.
//!
//! ```text
//!  Document<T> + ComputedValues          (immutable for the pass)
//!        │  impl LayoutNode for &Node<T> — children/style/dispatch/slots
//!        ▼
//!  neutron-star algorithms  ◀──▶  per-node Node::layout_data (interior-mutable)
//!        │                         dispatch: flex │ grid │ linear │ relative │ leaf
//!        ▼
//!  positioned pass (fixed/hoisted absolute) → device-pixel rounding
//! ```
//!
//! # Styles are fetched when the engine asks
//!
//! [`LayoutNode::style`](neutron_star::tree::LayoutNode::style) reads the
//! node's computed style **at request time** — an `Arc` clone out of the
//! node's own style data, nothing is pre-collected — and the view lends
//! `ComputedValues` field references from that `Arc` for as long as the
//! engine holds the view. The values lent are the per-node storage the
//! style flush wrote (materialized once per *style change*, per the
//! protocol's lending rule); the `Arc` in the view only keeps that storage
//! borrowable. Text nodes are lent the fork's initial values — the
//! anonymous box CSS wraps a text run in; their content comes from the
//! measurement hook.
//!
//! # Layout results live on the node
//!
//! Every [`Node`](crate::Node) carries its own [`LayoutData`] (an
//! `AtomicRefCell`, the same shape Servo uses for per-node layout data):
//! the measurement cache, the unrounded and device-snapped [`Layout`]s, and
//! the out-of-flow bookkeeping. Read results with
//! [`Node::layout`](crate::Node::layout) /
//! [`Node::unrounded_layout`](crate::Node::unrounded_layout). Because the
//! state lives **on** the node, it is created and dropped with the node —
//! there is no side table to keep in sync with the tree. The positioned
//! pass for hoisted out-of-flow nodes is a fresh tree walk every pass (no
//! queue), so fixed-position geometry stays correct even when a hoisted
//! node's formatting parent answers from its measurement cache.
//!
//! # Phases: style first, then layout
//!
//! [`StyleEngine::layout_document`] runs the style flush itself (a no-op
//! when nothing is scheduled) and only then lays out — layout reads computed
//! styles strictly after the restyle traversal has finished, mirroring the
//! style → layout phase barrier every production engine uses. The `&mut
//! Document` it takes is what guarantees the tree cannot change mid-pass.
//!
//! # Leaf content measures through the payload
//!
//! Content measurement (text runs, images, replaced content) is the
//! embedder's: implement [`MeasureLeaf`] on the document's payload type
//! `T`. The hook receives the node and the engine's content-box
//! constraints; the default measures nothing (`()` implements it). Hooks
//! run through `&self` — an embedder retaining artifacts (a shaped
//! paragraph for painting) uses interior mutability in its payload, keyed
//! by [`LeafMeasureInput::goal`].
//!
//! # Using it
//!
//! ```
//! use w3c_dom::StyleEngine;
//! # use euclid::{Scale, Size2D};
//! # use stylo::context::QuirksMode;
//! # use stylo::device::Device;
//! # use stylo::media_queries::MediaType;
//! # use stylo::properties::ComputedValues;
//! # use stylo::properties::style_structs::Font;
//! # use stylo::queries::values::PrefersColorScheme;
//! # use stylo::servo::media_features::PointerCapabilities;
//! # use stylo::device::servo::FontMetricsProvider;
//! # use stylo::font_metrics::FontMetrics;
//! # use stylo::values::computed::{CSSPixelLength, Length};
//! # use stylo::values::computed::font::GenericFontFamily;
//! # use stylo::values::specified::font::{FONT_MEDIUM_PX, QueryFontMetricsFlags};
//! # #[derive(Debug)]
//! # struct NoFonts;
//! # impl FontMetricsProvider for NoFonts {
//! #     fn query_font_metrics(
//! #         &self,
//! #         _: bool,
//! #         _: &Font,
//! #         _: CSSPixelLength,
//! #         _: QueryFontMetricsFlags,
//! #     ) -> FontMetrics {
//! #         FontMetrics::default()
//! #     }
//! #     fn base_size_for_generic(&self, _: GenericFontFamily) -> Length {
//! #         Length::new(FONT_MEDIUM_PX)
//! #     }
//! # }
//! # let device = Device::new(
//! #     MediaType::screen(),
//! #     QuirksMode::NoQuirks,
//! #     Size2D::new(800.0, 600.0),
//! #     Size2D::new(800.0, 600.0),
//! #     Scale::new(1.0),
//! #     Box::new(NoFonts),
//! #     ComputedValues::initial_values_with_font_override(Font::initial_values()),
//! #     PrefersColorScheme::Light,
//! #     PointerCapabilities::empty(),
//! #     PointerCapabilities::empty(),
//! # );
//! let mut engine = StyleEngine::new(device);
//! engine.add_stylesheet_str(
//!     "page { display: flex; width: 100px; height: 50px; } view { flex-grow: 1; }",
//!     w3c_dom::StylesheetOrigin::Author,
//! );
//! let mut document = engine.new_document();
//! let root = document.create_element("page", ());
//! document.append_child(root);
//! let child = document.create_element("view", ());
//! document.append(root, child);
//!
//! engine.layout_document(&mut document); // flushes styles, then lays out
//! assert_eq!(document.get(child).unwrap().layout().size.width, 100.0);
//! ```
//!
//! # Invalidation is the embedder's job (like snapshots, inverted)
//!
//! Layout only fills and reads each node's measurement cache; whenever a
//! node's **layout inputs** change between passes — computed style,
//! children, character data — call [`Document::invalidate_layout`] with
//! that node (for a removal: the old parent). It clears the node's cache
//! and every ancestor's, per neutron-star's dirty-path contract, so the
//! next pass recomputes exactly the dirty spine while clean subtrees answer
//! from their caches. When in doubt,
//! [`Document::invalidate_layout_all`] is always correct, merely slower.
//!
//! # What is deliberately *not* here (yet)
//!
//! - **Text shaping.** The Parley-backed text engine stays outside this crate (`neutron-star`'s
//!   `text` feature) and plugs in through the [`MeasureLeaf`] hook.
//! - **Flow (block/inline) container layout.** neutron-star has no flow algorithm yet; a flow or
//!   `display: contents` node lays out as a leaf and its children are zeroed. Lynx trees give every
//!   element a supported display via the embedder's UA sheet, so this arm is a generic-DOM
//!   fallback, not a Lynx path.
//! - **`position: sticky` offsets** (scroll-time host post-pass) and the `<list>` staggered mode.
//!
//! # `position: fixed` follows the W3C rule
//!
//! The engine's protocol encodes the positioning *scheme* in
//! [`position()`](neutron_star::style::CoreStyle::position): `absolute`
//! means "containing block = layout parent" and `fixed` means "hoisted to
//! the host's positioned pass". This host's style view therefore reports
//! the scheme after resolving the **real W3C containing-block rule**: a
//! computed `fixed` (or an `absolute` escaping an unpositioned parent) is
//! hoisted to the viewport **unless** an ancestor establishes a fixed
//! containing block (`transform`, `perspective`, `filter`, qualifying
//! `will-change`, `contain: layout/paint`) — not Lynx's unconditional
//! escape-to-root.

mod host;
mod style;

use std::sync::LazyLock;

use neutron_star::cache::Cache;
pub use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
pub use neutron_star::geometry::{Edges, Point, Size};
pub use neutron_star::tree::Layout;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub use self::style::StyleView;
use crate::document::Document;
use crate::engine::StyleEngine;
use crate::ext::ExternalState;
use crate::node::Node;

/// The embedder's leaf content measurement hook, implemented by the
/// document's payload type `T` (the layout-side sibling of
/// [`ExternalState`]'s matching hooks).
///
/// Consulted for every leaf-laid node — childless boxes and the
/// flow-container fallback — whose size is not already fully determined by
/// its box styles: text runs, images, and other replaced content measure
/// here. Measurement must not touch the document; it may retain
/// embedder-side artifacts (a shaped paragraph for painting) in the payload
/// via interior mutability, keyed by [`LeafMeasureInput::goal`]. The
/// default measures nothing.
pub trait MeasureLeaf: Sized {
    /// Measure `node`'s content under the engine's content-box constraints.
    ///
    /// `self` is `node`'s own payload (`node.ext()`), passed as the
    /// receiver so payload state is directly at hand.
    fn measure_leaf(&self, node: &Node<Self>, input: LeafMeasureInput) -> LeafMetrics {
        let _ = (node, input);
        LeafMetrics::default()
    }
}

/// The no-op payload measures nothing.
impl MeasureLeaf for () {}

/// One node's layout state, carried by the node itself
/// ([`Node::layout_data`](crate::Node)): created with the node, dropped with
/// the node — no side table to synchronize with the tree.
///
/// Lives behind an `AtomicRefCell` (the Servo per-node layout-data pattern):
/// the node stays shareable for stylo's parallel restyle traversal, while
/// the (single-threaded, post-style) layout pass takes short scoped borrows.
pub(crate) struct LayoutData {
    /// The neutron-star **measurement cache** — not a copy of the final
    /// result, but memoized answers to the different constraint questions
    /// (`LayoutInput`) the parent algorithms ask during sizing. This is the
    /// engine's asymptotic mechanism: without it, nested flex/grid sizing
    /// probes recurse exponentially. Its committed-layout slot is also what
    /// lets a clean subtree answer relayout without being walked.
    pub(crate) measure_cache: Cache,
    /// The durable unrounded layout (CSS pixels) — what relayout derives
    /// from; re-rounding rounded values is how engines drift.
    pub(crate) unrounded: Layout,
    /// The device-pixel-snapped layout — what painting consumes.
    pub(crate) rounded: Layout,
    /// The static position recorded for a hoisted out-of-flow node by its
    /// formatting parent (border-box space), consumed by the positioned
    /// pass. Persists across passes deliberately: it is parent-relative, so
    /// it stays valid exactly as long as the parent's cached layout does —
    /// the positioned pass re-reads it even when the parent's algorithm
    /// answered from its cache.
    pub(crate) static_position: Point<f32>,
}

impl Default for LayoutData {
    fn default() -> Self {
        Self {
            measure_cache: Cache::new(),
            unrounded: Layout::default(),
            rounded: Layout::default(),
            static_position: Point::ZERO,
        }
    }
}

/// The style lent to text nodes (and any style-less node): the fork's
/// initial computed values — exactly the box style CSS gives the anonymous
/// box wrapping a text run. Content sizing comes from the [`MeasureLeaf`]
/// hook, not from box properties. Process-wide, like the engine's own
/// lendable defaults.
pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl StyleEngine {
    /// Flush pending styles, then lay the document out against this engine's
    /// viewport and device-pixel ratio. Leaf content measures through the
    /// payload's [`MeasureLeaf`] hook.
    ///
    /// Results land on the nodes: read them with
    /// [`Node::layout`](crate::Node::layout).
    ///
    /// # Panics
    ///
    /// Panics when `document` was created by a different engine.
    pub fn layout_document<T: ExternalState + MeasureLeaf>(&self, document: &mut Document<T>) {
        // Phase barrier: layout reads computed styles only after the restyle
        // traversal has completed (no-op when nothing is scheduled). Also
        // asserts this engine owns the document.
        self.flush_document(document);
        let viewport = self.device().viewport_size();
        let scale = self.device().device_pixel_ratio().get();
        host::run_layout(document, Size::new(viewport.width, viewport.height), scale);
    }
}

impl<T> Document<T> {
    /// Record that `id`'s layout inputs changed since the last layout pass:
    /// clears its measurement cache and every ancestor's up to the document
    /// node (cached entries encode children's contributions — neutron-star's
    /// dirty-path invalidation contract).
    ///
    /// Call it for style, character-data, and child-list changes; for a
    /// removed node, invalidate its **old parent** instead (the removed
    /// node's layout state died with it).
    ///
    /// # Panics
    ///
    /// Panics when `id` is vacant (the let-it-crash mutation contract).
    pub fn invalidate_layout(&mut self, id: crate::NodeId) {
        let mut current = Some(id);
        while let Some(id) = current {
            let node = self
                .tree_mut()
                .get_mut(id)
                .expect("vacant NodeId passed to Document::invalidate_layout");
            node.layout_data.get_mut().measure_cache.clear();
            current = node.parent_id();
        }
    }

    /// Drop every node's measurement cache (all layouts stay readable). The
    /// always-correct, never-incremental fallback.
    pub fn invalidate_layout_all(&mut self) {
        for (_, node) in self.tree_mut().iter_mut() {
            node.layout_data.get_mut().measure_cache.clear();
        }
    }
}
