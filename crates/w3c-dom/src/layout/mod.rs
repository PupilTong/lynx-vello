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
//! Every [`Node`] carries its own `LayoutData` (an
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
//! when nothing is scheduled), **consumes the flush's own restyle damage**
//! into layout invalidation (see below), and only then lays out — layout
//! reads computed styles strictly after the restyle traversal has finished,
//! mirroring the style → layout phase barrier every production engine uses.
//! The `&mut Document` it takes is what guarantees the tree cannot change
//! mid-pass.
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
//! # Style-driven relayout is automatic; content/structure stays the embedder's
//!
//! Layout only fills and reads each node's measurement cache; between passes
//! a node's **layout inputs** must be re-derived wherever they changed. The
//! flush [`StyleEngine::layout_document`] runs first classifies exactly the
//! style-visible half of that: it streams the restyle
//! [`StyleDamage`] and, for every node that
//! [`needs_relayout`](crate::StyleDamage::needs_relayout), runs
//! [`Document::invalidate_layout`] on it (and its parent on a
//! reconstruct) — so **any change stylo can see drives relayout on its own**,
//! with no embedder call. That invalidation is boundary-stopped: it clears the
//! dirty spine up to the nearest `contain: strict` / skipped
//! `content-visibility` ancestor, leaving that boundary's ancestors' caches
//! warm and re-running the boundary's interior in place (see
//! [`Document::invalidate_layout`]).
//!
//! What the flush **cannot** see stays the embedder's job — call
//! [`Document::invalidate_layout`] directly for it (for a removal: the old
//! parent):
//!
//! - **character-data / child-list** mutations that leave every computed style identical (stylo
//!   emits no damage for them);
//! - **external-state measurement inputs** — anything a [`MeasureLeaf`] hook reads that is not a
//!   computed style (a font load, an image's intrinsic size arriving, a locale flip).
//!
//! When in doubt, [`Document::invalidate_layout_all`] is always correct,
//! merely slower.
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
//! `will-change`, effective `contain: layout/paint` — including the
//! containment `content-visibility: hidden`/`auto` implies) — not Lynx's
//! unconditional escape-to-root.

mod host;
mod style;

use std::sync::LazyLock;

use neutron_star::cache::Cache;
pub use neutron_star::compute::{LeafMeasureInput, LeafMetrics};
pub use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::invalidate::is_relayout_boundary;
use neutron_star::style::CoreStyle;
pub use neutron_star::tree::Layout;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub use self::style::StyleView;
use crate::damage::StyleDamage;
use crate::document::Document;
use crate::engine::StyleEngine;
use crate::ext::ExternalState;
use crate::flush::Parallelism;
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
    /// Flush pending styles — consuming the flush's own restyle damage into
    /// layout invalidation — then lay the document out against this engine's
    /// viewport and device-pixel ratio. Leaf content measures through the
    /// payload's [`MeasureLeaf`] hook.
    ///
    /// Style-driven relayout is **automatic**: the internal flush streams its
    /// per-node [`StyleDamage`] and, for each node whose damage
    /// [`needs_relayout`](StyleDamage::needs_relayout), runs a boundary-stopped
    /// [`Document::invalidate_layout`] on it (plus its parent when the damage
    /// [`is_reconstruct`](StyleDamage::is_reconstruct) — box generation changed,
    /// so the parent must re-collect its children). Repaint / stacking-context
    /// / overflow-only damage touches no layout cache. An embedder therefore
    /// never invalidates layout for a change stylo can see; see
    /// [`Document::invalidate_layout`] for the mutations that remain its job.
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
        // asserts this engine owns the document. Consume the flush's damage
        // with the zero-alloc sink: only relayout-class damage is collected,
        // so a clean or paint-only flush allocates nothing here.
        let mut relayout_targets: Vec<(crate::NodeId, StyleDamage)> = Vec::new();
        self.flush_document_with_sink(document, Parallelism::Auto, &mut |id, damage| {
            if damage.needs_relayout() {
                relayout_targets.push((id, damage));
            }
        });
        // The flush's `&mut Document` borrow is released now, so the harvested
        // damage can drive invalidation.
        for (id, damage) in relayout_targets {
            document.invalidate_layout(id);
            if damage.is_reconstruct() {
                // A reconstruct changes this node's generated box, so its parent
                // must re-collect children; invalidate the parent too.
                let parent = document.get(id).and_then(Node::parent_id);
                if let Some(parent) = parent {
                    document.invalidate_layout(parent);
                }
            }
        }

        let viewport = self.device().viewport_size();
        let scale = self.device().device_pixel_ratio().get();
        host::run_layout(document, Size::new(viewport.width, viewport.height), scale);
        // The pass consumed every parked relayout root (`run_layout` re-ran each
        // boundary in place); forget them so they cannot fire again next pass.
        document.clear_relayout_roots();
    }
}

impl<T> Document<T> {
    /// Record that `id`'s layout inputs changed since the last layout pass:
    /// clears its measurement cache and its ancestors', **stopping at the
    /// nearest ancestor that is a relayout boundary** (`contain: strict`, or a
    /// skipped `content-visibility` box — [`is_relayout_boundary`] over the
    /// same [`StyleView`] the layout pass reads). A cached measurement encodes
    /// its children's contributions, so a size-affecting change to `id`
    /// invalidates ancestors up the chain — but a boundary's own outer size
    /// cannot change from an interior mutation, so nothing above it needs
    /// clearing. The boundary itself **is** cleared (its interior changed) and,
    /// with its last committed [`LayoutInput`](neutron_star::tree::LayoutInput)
    /// captured beforehand, parked for the next layout pass to re-run in place
    /// via [`compute_boundary_relayout`](neutron_star::compute::compute_boundary_relayout)
    /// — the engine-internal version of neutron-star's `invalidate_for_relayout`
    /// re-layout root, which needs no re-root call because this host owns real
    /// parent links. A style-less node (text / the document node) is never a
    /// boundary, so the walk runs to the document root there, exactly as before.
    ///
    /// `id` (the damaged node) is cleared and walked past regardless of its own
    /// containment: a `contain: strict` node whose **own** style changed can
    /// still resize (containment isolates it from its *contents*, not from its
    /// own box), so the boundary test applies only to strict *ancestors* — the
    /// same shape as
    /// [`invalidate_for_relayout`](neutron_star::invalidate::invalidate_for_relayout).
    ///
    /// The walk resolves each ancestor into one of four cases:
    ///
    /// 0. **Skips contents** (`content-visibility: hidden`) — the ancestor lays out none of its
    ///    contents, so an interior mutation cannot change any layout output; its own box is sized
    ///    from styles alone and never enters the measurement cache. Stop immediately, parking
    ///    nothing and clearing nothing above it. Checked **first** because a skipped box also folds
    ///    `LAYOUT | SIZE` (it is a relayout boundary) yet never records a committed input — without
    ///    this case it would fall through to case 3 and wrongly empty every warm ancestor cache up
    ///    to the root. A later `hidden → visible` flip re-lays-out the interior through the
    ///    container's own `RELAYOUT` damage.
    /// 1. **Boundary with a committed layout** — park it (with that captured input) and stop; its
    ///    own outer size cannot change from an interior mutation, so nothing above it needs
    ///    clearing.
    /// 2. **Boundary already parked earlier in this same batch** — a second damaged node under the
    ///    one boundary, or a manual invalidate inside a boundary the flush already parked. Parking
    ///    already cleared its cache, so re-reading its committed input would see `None`; without
    ///    this case the walk would fall through to case 3 and clear on to the root, silently
    ///    defeating the containment the first walk established. Stop instead, clearing neither it
    ///    nor anything above it.
    /// 3. **Non-boundary, or a boundary never laid out** (no committed input, not parked) — not yet
    ///    a valid re-layout root, so clear it and keep walking toward the document root.
    ///
    /// Style changes are consumed automatically by
    /// [`StyleEngine::layout_document`]; call this directly only for mutations
    /// the style system cannot see — character-data and child-list changes that
    /// leave computed styles identical, and external-state measurement inputs.
    /// For a removed node, invalidate its **old parent** instead (the removed
    /// node's layout state died with it).
    ///
    /// # Panics
    ///
    /// Panics when `id` is vacant (the let-it-crash mutation contract).
    pub fn invalidate_layout(&mut self, id: crate::NodeId) {
        let boundary = {
            let tree = self.tree();
            // Boundaries already parked earlier in this invalidation batch (a
            // tiny vec — a linear scan beats a set).
            let parked = self.relayout_roots();
            let start = tree
                .get(id)
                .expect("vacant NodeId passed to Document::invalidate_layout");
            start.layout_data.borrow_mut().measure_cache.clear();

            let mut boundary = None;
            let mut current = start.parent();
            while let Some(node) = current {
                // Elements are the only nodes that carry containment styles; a
                // text or document node is never a boundary and never skips
                // contents. Build the view once and reuse it for both tests.
                let style_view = node.is_element().then(|| StyleView::of(node));
                // Case (0): a **skipped-contents** ancestor (`content-visibility:
                // hidden`) lays out *none* of its contents, so an interior
                // mutation cannot affect any layout output; its own box is sized
                // from styles alone and bypasses the measurement cache entirely.
                // There is nothing to park and no ancestor to clear — stop
                // immediately, leaving the warm caches above (and the skipped
                // box's empty cache) untouched.
                //
                // Ordered **before** the boundary/committed-input cases below: a
                // skipped box folds `LAYOUT | SIZE` (so `is_relayout_boundary`
                // holds) yet never records a committed input, which would
                // otherwise fall through to case (3) ("never laid out → keep
                // clearing") and wrongly clear every warm ancestor cache up to
                // the document root. A later `hidden → visible` flip drives its
                // own invalidation through the container's `RELAYOUT` damage.
                if style_view.as_ref().is_some_and(CoreStyle::skips_contents) {
                    break;
                }
                let is_boundary = style_view.as_ref().is_some_and(is_relayout_boundary);
                // Case (2): a boundary already parked earlier in this batch has
                // an already-cleared cache — stop here rather than re-reading a
                // now-`None` committed input and clearing on to the root (which
                // would defeat the containment the first walk established).
                if is_boundary && parked.iter().any(|&(parked_id, _)| parked_id == node.id()) {
                    break;
                }
                // Capture a boundary's committed input *before* the clear below
                // wipes it; a non-boundary (or a boundary never laid out) yields
                // `None`.
                let boundary_input = is_boundary
                    .then(|| node.layout_data.borrow().measure_cache.committed_input())
                    .flatten();
                node.layout_data.borrow_mut().measure_cache.clear();
                if let Some(input) = boundary_input {
                    // Case (1): a laid-out boundary — park it and stop; its own
                    // outer size cannot change from an interior mutation, so
                    // nothing above it needs clearing.
                    boundary = Some((node.id(), input));
                    break;
                }
                // Case (3) / non-boundary: a boundary never laid out is not yet
                // a valid re-layout root, and an ordinary ancestor's cache
                // encodes this subtree's contribution — clear (done) and keep
                // walking toward the document root.
                current = node.parent();
            }
            boundary
        };
        if let Some((boundary_id, committed_input)) = boundary {
            self.record_relayout_root(boundary_id, committed_input);
        }
    }

    /// Drop every node's measurement cache (all layouts stay readable) and
    /// forget any parked relayout roots. The always-correct, never-incremental
    /// fallback.
    pub fn invalidate_layout_all(&mut self) {
        for (_, node) in self.tree_mut().iter_mut() {
            node.layout_data.get_mut().measure_cache.clear();
        }
        self.clear_relayout_roots();
    }
}
