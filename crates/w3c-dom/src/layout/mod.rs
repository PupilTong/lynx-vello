//! Box layout over the document tree — the concrete [`neutron_star`] host.
//!
//! This module implements neutron-star's handle protocol on the crate's one
//! read handle: [`LayoutNode`](neutron_star::tree::LayoutNode) is
//! implemented **directly on `&Node<T>`** — the same one-word `Copy` value
//! the stylo `TNode`/`TElement` traits already use — so the engine traverses
//! the document itself, with no wrapper handle, no adapter objects, and no
//! engine-side tree or style copies. Flexbox, Grid, and Starlight
//! Linear/Relative containers dispatch to their neutron-star algorithms;
//! `display: none` subtrees are hidden; replaced leaves consume the
//! node-owned [`NaturalSize`]. There is no payload measurement callback.
//!
//! ```text
//!  Document<T> + ComputedValues          (immutable for the pass)
//!        │  impl LayoutNode for &Node<T> — children/style/dispatch/slots
//!        ▼
//!  neutron-star algorithms  ◀──▶  Node results + layout secondary arena
//!        │                         dispatch: flex │ grid │ linear │ relative │ leaf
//!        ▼
//!  positioned pass (fixed/hoisted absolute) → device-pixel rounding
//! ```
//!
//! # Styles are fetched when the engine asks
//!
//! [`LayoutNode::style`](neutron_star::tree::LayoutNode::style) reads the
//! node's computed style **at request time** — nothing is pre-collected. The
//! view holds Stylo's element-data read guard and lends `ComputedValues` field
//! references from the existing `Arc` target without cloning that Arc. The
//! values lent are the per-node storage the style flush wrote (materialized
//! once per *style change*, per the protocol's lending rule). Text nodes use
//! the fork's initial values for anonymous-box geometry and inherit font/text
//! values from their parent. Their character data always runs through
//! neutron-star's concrete Parley path.
//!
//! # Layout results live on the node
//!
//! Every [`Node`] carries the durable unrounded and device-snapped [`Layout`]
//! results behind an `AtomicRefCell`; read them with
//! [`Node::layout`](crate::Node::layout) /
//! [`Node::unrounded_layout`](crate::Node::unrounded_layout). Its measurement
//! cache and static-position bookkeeping live in the document's layout
//! secondary arena, indexed by the same `NodeId`; the primary node arena owns
//! lifecycle and removes that entry before an ID is reused. The positioned
//! pass for hoisted out-of-flow nodes is a fresh tree walk every pass (no
//! queue), so fixed-position geometry stays correct even when a hoisted node's
//! formatting parent answers from its measurement cache.
//!
//! # Phases: style first, then layout
//!
//! [`Document::layout`] runs the style flush itself (a no-op
//! when nothing is scheduled) and only then lays out — layout reads computed
//! styles strictly after the restyle traversal has finished, mirroring the
//! style → layout phase barrier every production engine uses. Every style
//! flush consumes relayout-class damage into cache invalidation as it is
//! harvested, so an earlier standalone flush cannot lose the invalidation
//! before this layout pass. The `&mut Document` it takes is what guarantees
//! the tree cannot change mid-pass.
//!
//! # Replaced content uses node-owned natural size
//!
//! Natural size is internal replaced-content state below the generic
//! Widget/PAPI layer. Leaf layout reads it directly from the node; the
//! lower-level update path clears the node-to-root measurement-cache path so
//! the next layout pass observes new dimensions. This is intrinsic replaced
//! content data, not a synthesized CSS `contain-intrinsic-size` value, and no
//! arbitrary embedder measurement callback exists.
//!
//! # Text always uses Parley
//!
//! The document node lazily creates and then owns one reusable
//! [`TextContext`](neutron_star::text::TextContext), while each text node retains separate probe
//! and committed Parley artifacts in its content record. Anonymous-box geometry uses initial CSS
//! values; shaping and paragraph values are read from the parent element's inherited
//! computed style. [`Document::register_fonts`] installs decoded fonts into
//! the document context and invalidates retained measurements.
//!
//! # Using it
//!
//! ```
//! use w3c_dom::Document;
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
//! let mut document = Document::new(device);
//! document.add_stylesheet_str(
//!     "page { display: flex; width: 100px; height: 50px; } view { flex-grow: 1; }",
//!     w3c_dom::StylesheetOrigin::Author,
//! );
//! let root = document.create_element("page", ());
//! document.append_child(root);
//! let child = document.create_element("view", ());
//! document.append(root, child);
//!
//! document.layout(); // flushes styles, then lays out
//! assert_eq!(document.get(child).unwrap().layout().size.width, 100.0);
//! ```
//!
//! # Style-driven relayout is automatic; content/structure stays the embedder's
//!
//! Layout only fills and reads each node's measurement cache; between passes
//! a node's **layout inputs** must be re-derived wherever they changed. The
//! style flush classifies exactly the style-visible half of that: it streams the restyle
//! [`StyleDamage`](crate::StyleDamage) and, for every node that
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
//! - **content inputs** that are not computed style. The internal natural-size update path performs
//!   its own targeted invalidation.
//!
//! When in doubt, [`Document::invalidate_layout_all`] is always correct,
//! merely slower.
//!
//! # What is deliberately *not* here (yet)
//!
//! - **Lynx text policy.** Element-backed raw text, Lynx-specific attributes, inline boxes,
//!   truncation/ellipsis, and paint lowering remain widget/render work.
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
#[cfg(feature = "layout-test-utils")]
use neutron_star::compute::LeafMetrics;
use neutron_star::compute::NaturalSize;
pub use neutron_star::geometry::{Edges, Point, Size};
use neutron_star::invalidate::is_relayout_boundary;
use neutron_star::style::CoreStyle;
pub use neutron_star::tree::Layout;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub use self::style::StyleView;
use crate::document::Document;
use crate::flush::Parallelism;

/// One node's intermediate layout state, stored in the document's layout
/// secondary arena under the node's `NodeId`.
pub(crate) struct LayoutData {
    /// The neutron-star **measurement cache** — not a copy of the final
    /// result, but memoized answers to the different constraint questions
    /// (`LayoutInput`) the parent algorithms ask during sizing. This is the
    /// engine's asymptotic mechanism: without it, nested flex/grid sizing
    /// probes recurse exponentially. Its committed-layout slot is also what
    /// lets a clean subtree answer relayout without being walked.
    pub(crate) measure_cache: Cache,
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
            static_position: Point::ZERO,
        }
    }
}

impl LayoutData {
    /// Clear every cached box-layout answer derived from content or style.
    pub(crate) fn clear_measurement_cache(&mut self) {
        self.measure_cache.clear();
    }
}

/// Durable layout outputs kept in the primary node arena. Painting consumes
/// `rounded`; incremental layout and positioned-coordinate conversion consume
/// `unrounded` so snapped values are never fed back into layout.
#[derive(Default)]
pub(crate) struct LayoutResults {
    pub(crate) unrounded: Layout,
    pub(crate) rounded: Layout,
}

/// The style lent to text nodes (and any style-less node): the fork's
/// initial computed values — exactly the box style CSS gives the anonymous
/// box wrapping a text run. Process-wide, like the engine's own lendable
/// defaults.
pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl<T: Sync> Document<T> {
    /// Flush pending styles, then lay the document out against its private
    /// viewport and device-pixel ratio. Replaced leaves use their node-owned
    /// internal `NaturalSize`.
    ///
    /// Style-driven relayout is **automatic**: every style flush consumes
    /// relayout-class damage into boundary-stopped layout invalidation while
    /// harvesting it. This includes a standalone
    /// [`flush_styles`](Self::flush_styles) performed before this method;
    /// its summary may be discarded without losing layout invalidation.
    /// Repaint / stacking-context / overflow-only damage touches no layout
    /// cache. An embedder therefore never invalidates layout for a change
    /// stylo can see; see [`Document::invalidate_layout`] for the mutations
    /// that remain its job.
    ///
    /// Results land on the nodes: read them with
    /// [`Node::layout`](crate::Node::layout).
    pub fn layout(&mut self) {
        // Phase barrier: layout reads computed styles only after the restyle
        // traversal and damage harvest have completed (no-op when nothing is
        // scheduled). Harvest itself consumes relayout-class damage into the
        // caches, so this sink need not retain anything. The style engine used
        // here is structurally private to this document.
        self.flush_styles_with_sink(Parallelism::Auto, &mut |_, _| {});

        let viewport_size = self.device().viewport_size();
        let viewport = Size::new(viewport_size.width, viewport_size.height);
        let scale = self.device().device_pixel_ratio().get();

        // Idle-frame fast path: nothing has been invalidated since the last pass
        // and the viewport/scale are unchanged, so the stored positioned +
        // rounded geometry still holds. Skip the whole pass — an unchanged frame
        // costs O(1), not an O(N) re-walk of the visible tree. (A clean flush
        // above already parked nothing and cleared no cache, so there is no
        // pending work to lose.)
        if !self.layout_needs_pass(viewport, scale) {
            return;
        }

        // A full pass re-runs positioned + rounding from the document root; an
        // incremental one re-processes only the parked containment boundaries'
        // subtrees, leaving every clean subtree's stored geometry untouched.
        let full = self.layout_requires_full_pass(viewport, scale);
        host::run_layout(self, viewport, scale, full);
        // The pass consumed every parked relayout root (`run_layout` re-ran each
        // boundary in place); forget them so they cannot fire again next pass,
        // and record this pass's inputs so the next idle frame can be skipped.
        self.clear_relayout_roots();
        self.mark_layout_complete(viewport, scale);
    }
}

impl<T> Document<T> {
    /// Updates a node's decoded natural dimensions/ratio and invalidates the
    /// affected layout-cache path when the value changed.
    ///
    /// The lower-level replaced-content implementation calls this after it
    /// knows the intrinsic size. This is ordinary content invalidation, not
    /// CSS size containment: no `contain-*` computed value is synthesized,
    /// and the operation is not surfaced through Widget/PAPI.
    ///
    /// # Panics
    ///
    /// Panics when `id` is vacant or does not identify an element (the
    /// let-it-crash mutation contract).
    #[allow(
        dead_code,
        reason = "owned by the future internal replaced-content loader"
    )]
    pub(crate) fn set_natural_size(&mut self, id: crate::NodeId, natural_size: NaturalSize) {
        let changed = {
            let node = self
                .tree_mut()
                .get_mut(id)
                .expect("vacant NodeId passed to Document::set_natural_size");
            assert!(
                node.is_element(),
                "non-element NodeId passed to Document::set_natural_size"
            );
            node.set_natural_size(natural_size)
        };
        if changed {
            self.invalidate_layout(id);
        }
    }

    /// Install synthetic content-box metrics for layout tests and
    /// production-host benchmarks.
    ///
    /// This routes through neutron-star's real leaf box-model routine; only
    /// the content engine is synthetic. The API is absent from normal builds.
    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn set_leaf_metrics_for_testing(
        &mut self,
        id: crate::NodeId,
        size: Size<f32>,
        first_baseline: Option<f32>,
    ) {
        let node = self
            .tree_mut()
            .get_mut(id)
            .expect("vacant NodeId passed to Document::set_leaf_metrics_for_testing");
        assert!(
            node.is_element(),
            "non-element NodeId passed to Document::set_leaf_metrics_for_testing"
        );
        node.set_test_leaf_metrics(
            LeafMetrics::new(size).with_first_baselines(Point::new(None, first_baseline)),
        );
        self.invalidate_layout(id);
    }

    /// Register decoded font data in this document's shared Parley context.
    ///
    /// Every readable face becomes available to subsequent text layout. A
    /// successful registration invalidates box caches and retained text
    /// artifacts because fallback selection may change anywhere in the tree.
    pub fn register_fonts(&mut self, bytes: &[u8]) -> usize {
        let registered = self
            .root_node()
            .text_context()
            .borrow_mut()
            .register_fonts(bytes);
        if registered != 0 {
            self.invalidate_layout_all();
        }
        registered
    }

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
    /// Only case 3 — a walk that exhausts the ancestor chain to the document
    /// root — marks the next pass as needing a **whole-tree** positioned +
    /// rounding walk. Cases 0, 1, and 2 each confine the change (a skipped
    /// container, a freshly parked boundary, or an already-parked one), so the
    /// pass re-processes only the parked boundaries' subtrees. Conflating them
    /// (e.g. keying off "no boundary was parked") would force a full-document
    /// pass for a second mutation under one boundary or any mutation under
    /// `content-visibility: hidden`, defeating the containment optimization.
    ///
    /// Style changes are consumed automatically by every style flush; call
    /// this directly only for mutations the style system cannot see —
    /// character-data and child-list changes that leave computed styles
    /// identical, and external-state measurement inputs. For a removed node,
    /// invalidate its **old parent** instead (the removed node's layout state
    /// died with it).
    ///
    /// # Panics
    ///
    /// Panics when `id` is vacant (the let-it-crash mutation contract).
    pub fn invalidate_layout(&mut self, id: crate::NodeId) {
        // `reached_root` is set only when the walk runs to the document root
        // through case (3) without a boundary, skip, or already-parked stop
        // confining it — the one outcome that forces the next pass's positioned +
        // rounding walks to run over the whole tree (`layout_root_dirty`).
        // `boundary.is_none()` must **not** stand in for it: cases (0) and (2)
        // also break with `boundary == None`, and treating those as
        // root-reaching would defeat containment (a second mutation under one
        // parked boundary, or any mutation under `content-visibility: hidden`,
        // would force a full-document pass).
        let (boundary, reached_root) = {
            let tree = self.tree();
            let start = tree
                .get(id)
                .expect("vacant NodeId passed to Document::invalidate_layout");
            start.layout_data().borrow_mut().clear_measurement_cache();
            start.invalidate_text_artifacts();

            let mut boundary = None;
            // Stays `true` only if the loop exits by exhausting the ancestor
            // chain (case 3 all the way to the document root); every `break`
            // below is a containment-confined stop and clears it.
            let mut reached_root = true;
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
                    reached_root = false;
                    break;
                }
                let is_boundary = style_view.as_ref().is_some_and(is_relayout_boundary);
                // Case (2): a boundary already parked earlier in this batch has
                // an already-cleared cache — stop here rather than re-reading a
                // now-`None` committed input and clearing on to the root (which
                // would defeat the containment the first walk established).
                // `is_relayout_root_parked` is an `O(1)` set query: a batch that
                // parks `B` boundaries (a dirty leaf per contained row of a long
                // list) must not pay `O(B²)` here.
                if is_boundary && self.is_relayout_root_parked(node.id()) {
                    reached_root = false;
                    break;
                }
                // Capture a boundary's committed input *before* the clear below
                // wipes it; a non-boundary (or a boundary never laid out) yields
                // `None`.
                let boundary_input = is_boundary
                    .then(|| node.layout_data().borrow().measure_cache.committed_input())
                    .flatten();
                node.layout_data().borrow_mut().clear_measurement_cache();
                node.invalidate_text_artifacts();
                if let Some(input) = boundary_input {
                    // Case (1): a laid-out boundary — park it and stop; its own
                    // outer size cannot change from an interior mutation, so
                    // nothing above it needs clearing.
                    boundary = Some((node.id(), input));
                    reached_root = false;
                    break;
                }
                // Case (3) / non-boundary: a boundary never laid out is not yet
                // a valid re-layout root, and an ordinary ancestor's cache
                // encodes this subtree's contribution — clear (done) and keep
                // walking toward the document root.
                current = node.parent();
            }
            (boundary, reached_root)
        };
        // Only a walk that reached the document root forces the next pass to run
        // positioned + rounding over the whole tree; a boundary/skip/already-parked
        // stop keeps it scoped to the parked boundaries.
        self.mark_layout_dirty(reached_root);
        if let Some((boundary_id, committed_input)) = boundary {
            self.record_relayout_root(boundary_id, committed_input);
        }
    }

    /// Drop every node's measurement cache (all layouts stay readable) and
    /// forget any parked relayout roots. The always-correct, never-incremental
    /// fallback.
    pub fn invalidate_layout_all(&mut self) {
        for (_, data) in self.layout_data_mut() {
            data.get_mut().clear_measurement_cache();
        }
        for (_, node) in self.tree_mut().iter_mut() {
            node.invalidate_text_artifacts();
        }
        self.clear_relayout_roots();
        // A blanket invalidation reaches the root by definition: the next pass
        // must re-run positioned + rounding over the whole tree.
        self.mark_layout_dirty(true);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use neutron_star::tree::{LayoutInput, LayoutOutput};

    use super::*;
    use crate::{DOCUMENT_NODE_ID, StylesheetOrigin};

    #[test]
    fn internal_natural_size_update_invalidates_the_dirty_spine() {
        let mut document = Document::new(crate::document::tests::device());
        let root = document.create_element("page", ());
        document.append_child(root);
        let image = document.create_element("image", ());
        document.append(root, image);

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            document
                .get(id)
                .unwrap()
                .layout_data()
                .borrow_mut()
                .measure_cache
                .store(input, LayoutOutput::default());
        }

        let natural_size = NaturalSize::from_size(Size::new(40.0, 20.0));
        document.set_natural_size(image, natural_size);

        assert_eq!(document.get(image).unwrap().natural_size(), natural_size);
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert!(
                document
                    .get(id)
                    .unwrap()
                    .layout_data()
                    .borrow()
                    .measure_cache
                    .is_empty()
            );
        }
    }

    /// Only a walk that reaches the document root may force the next pass to be a
    /// whole-tree positioned/rounding walk. The confined stops — a
    /// `content-visibility: hidden` skip (case 0), a freshly parked boundary
    /// (case 1), and an already-parked one (case 2) — must all keep
    /// `layout_root_dirty` clear, or a second mutation under one boundary (or any
    /// mutation under skipped contents) would defeat the containment scoping.
    #[test]
    fn only_a_root_reaching_invalidation_forces_a_full_pass() {
        // Reuse the shared 800×600 test device; the assertions compare against
        // that viewport (the document's own device), which is what a completed
        // pass records.
        let mut doc: Document<()> = Document::new(crate::document::tests::device());
        doc.add_stylesheet_str(
            "page { display: flex; width: 300px; height: 100px; }
             .box { display: flex; contain: strict; width: 80px; height: 40px; }
             .skip { display: flex; content-visibility: hidden;
                     contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.create_element("page", ());
        doc.append_child(root);

        let boundary = doc.create_element("view", ());
        doc.add_class(boundary, "box");
        doc.append(root, boundary);
        let c1 = doc.create_element("view", ());
        doc.add_class(c1, "leaf");
        doc.append(boundary, c1);
        let c2 = doc.create_element("view", ());
        doc.add_class(c2, "leaf");
        doc.append(boundary, c2);

        let plain = doc.create_element("view", ());
        doc.add_class(plain, "leaf");
        doc.append(root, plain);

        let skip = doc.create_element("view", ());
        doc.add_class(skip, "skip");
        doc.append(root, skip);
        let hidden_child = doc.create_element("view", ());
        doc.add_class(hidden_child, "leaf");
        doc.append(skip, hidden_child);

        doc.layout();

        // Read the same inputs the pass recorded (the document's device
        // viewport), so `layout_requires_full_pass` isolates `layout_root_dirty`.
        let viewport = Size::new(800.0, 600.0);
        let scale = 1.0;
        assert!(
            !doc.layout_needs_pass(viewport, scale),
            "an unchanged frame after layout needs no pass at all",
        );

        // Case 0 — a mutation under `content-visibility: hidden` is confined.
        doc.invalidate_layout(hidden_child);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a skipped-contents mutation must not force a whole-tree pass",
        );

        // Cases 1 + 2 — park the boundary, then a second mutation under it hits
        // the already-parked stop; both stay incremental.
        doc.invalidate_layout(c1);
        doc.invalidate_layout(c2);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a second mutation under one parked boundary must stay incremental",
        );

        // Case 3 — a mutation whose walk reaches the document root forces it.
        doc.invalidate_layout(plain);
        assert!(
            doc.layout_requires_full_pass(viewport, scale),
            "a root-reaching mutation forces a whole-tree pass",
        );
    }
}
