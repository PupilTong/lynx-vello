//! Box layout over the document tree — the concrete [`hughie`] host.

mod host;
mod style;

use std::sync::LazyLock;

#[cfg(feature = "layout-test-utils")]
use hughie::compute::LeafMetrics;
pub use hughie::compute::NaturalSize;
pub(crate) use hughie::geometry::Edges;
#[cfg(feature = "layout-test-utils")]
use hughie::geometry::Point;
pub use hughie::geometry::Size;
use hughie::invalidate::is_relayout_boundary;
use hughie::style::CoreStyle;
pub(crate) use hughie::text::TextLayout;
use hughie::text::{FontBlob, TextContext};
pub use hughie::tree::Layout;
use hughie::tree::LayoutSlot;
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub(crate) use self::style::{
    DisplayMode, StyleView, box_parent, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, shaping_inputs_changed, skips_contents,
};
use crate::tree::document::{Document, NodeLayoutState, RelayoutKind};

pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl<T: Sync> Document<T> {
    pub fn layout(&mut self) {
        self.flush_styles_with_damage_sink(&mut |_, _| {});

        let viewport_size = self.device().viewport_size();
        let viewport = Size::new(viewport_size.width, viewport_size.height);
        let scale = self.device().device_pixel_ratio().get();

        if !self.layout_needs_pass(viewport, scale) {
            return;
        }

        let full = self.layout_requires_full_pass(viewport, scale);
        let rescale = self.layout_inputs_changed(viewport, scale);
        let bound = self.arenas().slot_bound();
        self.layout_state_mut().ensure_covers(bound);
        host::run_layout(self, viewport, scale, full, rescale);
        self.clear_relayout_roots();
        self.mark_layout_complete(viewport, scale);
    }
}

/// The intrinsic size a completed image load reports, as layout wants it.
///
/// One conversion for the two places a load can reach layout — the report
/// itself, and a source set on a node that had already loaded.
#[must_use]
pub(crate) fn natural_size(width: u32, height: u32) -> NaturalSize {
    #[expect(
        clippy::cast_precision_loss,
        reason = "an intrinsic size large enough to lose precision here is far past anything \
                  layout can present, and rounds harmlessly. It is deliberately not bounded: \
                  the atlas limit applies to the decoded bitmap, not to this."
    )]
    NaturalSize::from_size(hughie::geometry::Size::new(width as f32, height as f32))
}

impl<T> Document<T> {
    /// Updates intrinsic dimensions and invalidates affected layout.
    pub fn set_natural_size(&mut self, id: crate::NodeId, natural_size: NaturalSize) {
        let changed = {
            let node = self
                .arenas_mut()
                .get_mut(id)
                .expect("stale NodeId passed to Document::set_natural_size");
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

    #[must_use]
    pub(crate) fn natural_size(&self, id: crate::NodeId) -> NaturalSize {
        self.get(id)
            .map_or(NaturalSize::NONE, crate::Node::natural_size)
    }

    /// Sets the source the paint walk presents to the host's resource system
    /// through [`FrameImages`](crate::FrameImages) for this replaced element.
    ///
    /// A replaced element's geometry comes from [`Self::set_natural_size`],
    /// which the caller sets separately once the store reports the image's
    /// own dimensions — the two halves arrive independently and in either
    /// order. Changing the source of an element that is already replaced
    /// therefore invalidates only the scene, but the call that *makes* an
    /// element replaced also invalidates layout: being replaced forces
    /// `DisplayMode::Leaf`, which sizes the box from its natural size and
    /// hides every child, so a source arriving before any natural size is a
    /// layout change on its own.
    pub fn set_image_source(&mut self, id: crate::NodeId, source: Option<&str>) {
        let (changed, became_replaced, previous) = {
            let node = self
                .arenas_mut()
                .get_mut(id)
                .expect("stale NodeId passed to Document::set_image_source");
            assert!(
                node.is_element(),
                "non-element NodeId passed to Document::set_image_source"
            );
            let was_replaced = node.is_replaced();
            let previous = node.image_source().map(str::to_owned);
            (node.set_image_source(source), !was_replaced, previous)
        };
        if !changed {
            return;
        }
        // The registry has to know which node presents which source, or a
        // completed load has nobody to hand its intrinsic size to.
        if let Some(previous) = previous {
            self.images.unbind_node(&previous, id);
        }
        if let Some(source) = source {
            self.images.bind_node(source, id);
            // A source already loaded sizes the element in this same call,
            // so it lays out correctly in the commit that first draws it
            // rather than a frame later.
            if let Some((width, height)) = self.images.dimensions_of(source) {
                self.set_natural_size(id, natural_size(width, height));
            }
        }
        if became_replaced {
            self.invalidate_layout(id);
        } else {
            self.note_visual_mutation();
        }
    }

    #[must_use]
    pub(crate) fn image_source(&self, id: crate::NodeId) -> Option<&str> {
        self.get(id).and_then(crate::Node::image_source)
    }

    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn set_leaf_metrics_for_testing(
        &mut self,
        id: crate::NodeId,
        size: Size<f32>,
        first_baseline: Option<f32>,
    ) {
        let node = self
            .arenas_mut()
            .get_mut(id)
            .expect("stale NodeId passed to Document::set_leaf_metrics_for_testing");
        assert!(
            node.is_element(),
            "non-element NodeId passed to Document::set_leaf_metrics_for_testing"
        );
        node.set_test_leaf_metrics(
            LeafMetrics::new(size).with_first_baselines(Point::new(None, first_baseline)),
        );
        self.invalidate_layout(id);
    }

    /// Registers an owned font resource without copying its byte payload.
    pub fn register_fonts(&mut self, data: FontBlob) -> usize {
        let context = self
            .layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()));
        let registered = context.register_fonts(data);
        if registered != 0 {
            self.invalidate_layout_all();
        }
        registered
    }

    /// Selects a registered family as the embedder-provided platform default.
    ///
    /// This maps CSS `system-ui`, `sans-serif`, and `serif` to `family` ahead
    /// of any platform fallbacks. Returns `false` when the family is unknown.
    pub fn set_default_font_family(&mut self, family: &str) -> bool {
        let context = self
            .layout_state_mut()
            .text_context
            .get_or_insert_with(|| Box::new(TextContext::new()));
        let configured = context.set_default_font_family(family);
        if configured {
            self.invalidate_layout_all();
        }
        configured
    }

    #[must_use]
    pub fn rounded_layout(&self, id: crate::NodeId) -> Option<&Layout> {
        let slot = self.slot(id)?;
        Some(&self.layout_state().get(slot)?.slot.rounded)
    }

    #[must_use]
    pub(crate) fn text_layout(&self, id: crate::NodeId) -> Option<&TextLayout> {
        let slot = self.slot(id)?;
        self.layout_state().get(slot)?.text.as_deref()?.committed()
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn text_store(&self, id: crate::NodeId) -> Option<&hughie::text::TextLayoutStore> {
        let slot = self.slot(id)?;
        self.layout_state().get(slot)?.text.as_deref()
    }

    #[must_use]
    pub(crate) fn paint_style(&self, id: crate::NodeId) -> Option<&ComputedValues> {
        self.get(id)?.layout_computed_style()
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn layout_cache_is_empty(&self, id: crate::NodeId) -> Option<bool> {
        let slot = self.slot(id)?;
        Some(
            self.layout_state()
                .get(slot)
                .is_none_or(|state| state.slot.layout_cache_is_empty()),
        )
    }

    /// Drops the cached box measurements of `id` and of every ancestor whose
    /// geometry could move because of it.
    ///
    /// **Box caches only.** A shaped paragraph is not geometry: it dies when
    /// the text or the style it was shaped from changes, which is a different
    /// question and has its own entry points
    /// ([`Document::invalidate_text_children`] and
    /// [`TreeArenas::clear_text_artifact`]). Clearing it here would be free
    /// today — a text node is a leaf, and no element holds an artifact — but
    /// it stops being free the moment a paragraph is owned by the element
    /// that establishes it: every descendant mutation and every ancestor this
    /// walk touches would re-shape a paragraph whose text never changed.
    pub(crate) fn invalidate_layout(&mut self, id: crate::NodeId) {
        let (pending, reached_root) = {
            let (tree, state, _) = self.layout_parts();
            let slot = tree
                .slot(id)
                .expect("stale NodeId passed to Document::invalidate_layout");
            let start = tree.at(slot);
            state.clear_box_cache(slot);

            let mut pending = None;
            let mut reached_root = true;
            let mut current = start.flat_parent_id();
            while let Some(node_id) = current {
                let node_slot = tree.live_slot(node_id);
                let node = tree.at(node_slot);
                let node_state = state.get(node_slot).map(|state| &state.slot);
                if node_state.is_none_or(LayoutSlot::layout_cache_is_empty)
                    && node.flat_parent_id().is_some()
                {
                    // Nothing above needs clearing: either an earlier
                    // invalidation already walked past here (whatever it
                    // recorded still stands), the node sits parked for a
                    // committed-input relayout (recording clears its cache),
                    // or it was never laid out (detached or hidden). The
                    // parentless document node is exempt — it never holds a
                    // cache, and stopping there must still count as reaching
                    // the root.
                    reached_root = false;
                    break;
                }
                let style_view = node.is_element().then(|| StyleView::of(node));
                if style_view.as_ref().is_some_and(CoreStyle::skips_contents) {
                    reached_root = false;
                    break;
                }
                let scheduled = if style_view.as_ref().is_some_and(is_relayout_boundary) {
                    node_state
                        .and_then(LayoutSlot::committed_input)
                        .map(|input| (node_id, input, RelayoutKind::Boundary))
                } else if node.is_element()
                    && node_id != crate::tree::document::DOCUMENT_ELEMENT_NODE_ID
                {
                    // A committed input the parent imposed independently of
                    // this subtree's content survives the mutation, so the
                    // subtree can relayout in place under it; run_layout
                    // verifies the output stayed identical before trusting it.
                    node_state.and_then(LayoutSlot::committed_independent).map(
                        |(input, previous)| (node_id, input, RelayoutKind::InPlace { previous }),
                    )
                } else {
                    None
                };
                state.clear_box_cache(node_slot);
                if let Some(entry) = scheduled {
                    pending = Some(entry);
                    reached_root = false;
                    break;
                }
                current = node.flat_parent_id();
            }
            (pending, reached_root)
        };
        self.mark_layout_dirty(reached_root);
        if let Some((root_id, committed_input, kind)) = pending {
            self.record_relayout_root(root_id, committed_input, kind);
        }
    }

    pub(crate) fn invalidate_layout_all(&mut self) {
        for (
            _,
            NodeLayoutState {
                slot,
                text,
                scroll_offset: _,
            },
        ) in self.layout_data_mut()
        {
            slot.clear_layout_cache();
            if let Some(artifacts) = text.as_deref_mut() {
                artifacts.invalidate();
            }
        }
        self.clear_relayout_roots();
        self.mark_layout_dirty(true);
    }

    /// Test/benchmark-only cache invalidation hook for protocol fixtures that
    /// mutate synthetic leaf data outside the production DOM mutation API.
    #[cfg(feature = "layout-test-utils")]
    #[doc(hidden)]
    pub fn invalidate_layout_for_testing(&mut self, id: crate::NodeId) {
        self.invalidate_layout(id);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::mem::size_of;

    use hughie::text::TextLayoutStore;
    use hughie::tree::{LayoutInput, LayoutOutput, LayoutSlot};

    use super::*;
    use crate::StylesheetOrigin;
    use crate::tree::document::DOCUMENT_NODE_ID;

    #[test]
    fn layout_state_size_probe() {
        const PRE_SPLIT_NODE_SIZE: usize = 368;
        const PRE_SPLIT_ATOMIC_LAYOUT_DATA_SIZE: usize = 456;
        const PRE_SPLIT_ATOMIC_LAYOUT_RESULTS_SIZE: usize = 160;
        let current = (
            size_of::<crate::Node<()>>(),
            size_of::<LayoutSlot>(),
            size_of::<NodeLayoutState>(),
            size_of::<TextLayoutStore>(),
        );
        eprintln!(
            "current: node={} layout_slot={} node_layout_state={} text_store={}; \
             pre-static-split baseline: node={} atomic_layout_data={} \
             atomic_layout_results={}",
            current.0,
            current.1,
            current.2,
            current.3,
            PRE_SPLIT_NODE_SIZE,
            PRE_SPLIT_ATOMIC_LAYOUT_DATA_SIZE,
            PRE_SPLIT_ATOMIC_LAYOUT_RESULTS_SIZE,
        );
        #[cfg(target_pointer_width = "64")]
        assert_eq!(
            current,
            (if cfg!(debug_assertions) { 224 } else { 216 }, 336, 352, 8,),
            "Node, LayoutSlot, NodeLayoutState, and TextLayoutStore sizes changed",
        );
    }

    /// Builds the shape a Lynx label actually has: an auto-sized `text`
    /// element holding one text node, inside a definite-width flex row.
    fn label_document(text: &str, width: f32) -> (Document<()>, crate::NodeId) {
        let (document, _, run) = label_document_parts(text, width);
        (document, run)
    }

    fn label_document_parts(
        text: &str,
        width: f32,
    ) -> (Document<()>, crate::NodeId, crate::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../hughie/tests/fixtures/Ahem.ttf");

        let mut document: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        document.add_stylesheet(
            &format!(
                "page {{ display: flex; width: {width}px; height: 100px;
                         align-items: flex-start; font-family: Ahem; font-size: 16px; }}
                 .label {{ display: flex; }}"
            ),
            StylesheetOrigin::Author,
        );
        assert_eq!(document.register_fonts(FontBlob::from_static(AHEM)), 1);
        let root = document.document_element().id();
        let label = document.create_element("text", ());
        document.add_class(label, "label");
        document.append_child(root, label);
        let run = document.create_text_node(text, ());
        document.append_child(label, run);
        (document, label, run)
    }

    /// The number this whole path exists to hold down.
    ///
    /// Nine measurements reach a flex text child in one pass — max-content,
    /// min-content, and its used width, each asked once per enclosing flex
    /// level, plus the commit — because the box cache keys on the available
    /// height a text node's answer does not depend on. What survives is one
    /// line break per *distinct* constraint, which is the floor. Before the
    /// break and constraint memos all nine broke, and the first probe also
    /// deep-cloned the shaped layout.
    #[test]
    fn one_pass_breaks_a_text_node_once_per_distinct_constraint() {
        for (case, text, width, lines, breaks) in [
            // min-content == max-content == used width: two distinct widths.
            ("one word", "hello", 200.0, 1, 2),
            // min-content (one word) < used width == max-content: three.
            ("one line, two words", "hello world", 200.0, 1, 3),
            // min-content, max-content as a width, the used width, and
            // unconstrained: four, one break each.
            ("wrapped", "hello world", 100.0, 2, 4),
        ] {
            let (mut document, run) = label_document(text, width);
            document.layout();

            let store = document
                .text_store(run)
                .expect("the text node retains a layout");
            assert!(!store.is_probe_dirty(), "{case}");
            let committed = store.committed().expect("the pass committed a text layout");
            assert_eq!(committed.line_count(), lines, "{case}: lines");
            assert_eq!(
                committed.break_count(),
                breaks,
                "{case}: line breaks per pass"
            );
        }
    }

    /// The case the restore queue exists for: a pass that measures a text node
    /// and never commits it.
    ///
    /// It is reachable whenever the box cache answers a node's `Commit` but
    /// not the `Measure` that preceded it — a single-axis probe can never be
    /// served from the committed slot, while the commit input the parent
    /// re-imposes always is. Driving the host directly is the deterministic
    /// way to produce it; what matters is that the layout does not end the
    /// pass painting the probe's line breaks.
    #[test]
    fn a_probe_that_never_commits_is_handed_back_to_its_committed_break() {
        use hughie::tree::{AvailableSpace, LayoutInput, LayoutTree, RequestedAxis};

        let (mut document, run) = label_document("hello world", 200.0);
        document.layout();
        let committed = document.text_layout(run).expect("committed").max_advance();
        let lines = document.text_layout(run).expect("committed").line_count();

        let slot = document.live_slot(run);
        let (tree, state, _) = document.layout_parts();
        tree.compute_layout(
            state,
            slot,
            LayoutInput::measure(
                Size::NONE,
                Size::NONE,
                Size::new(AvailableSpace::Definite(37.0), AvailableSpace::MaxContent),
                RequestedAxis::Horizontal,
            ),
        );

        let store = document.text_store(run).expect("retained");
        assert!(
            store.is_probe_dirty(),
            "a probe at an unseen constraint moves the retained line breaks",
        );
        assert_eq!(
            store.retained().expect("retained").max_advance(),
            Some(37.0)
        );

        document.layout_state_mut().restore_probed_text();

        let store = document.text_store(run).expect("retained");
        assert!(!store.is_probe_dirty());
        let restored = store.committed().expect("committed");
        assert_eq!(restored.max_advance(), committed);
        assert_eq!(restored.line_count(), lines);
    }

    /// The two-level eviction, from the outside: a relayout-damaged element
    /// keeps its text children's shaped glyphs unless the restyle moved
    /// something Parley shapes from.
    #[test]
    fn a_relayout_keeps_shaped_text_unless_the_shaping_inputs_moved() {
        for (case, declaration, survives) in [
            // Geometry: a different `Font`/`InheritedText` is never even
            // allocated, so level one answers.
            ("width", "width: 150px", true),
            ("padding", "padding: 4px", true),
            // Same struct as `letter-spacing`, but not a shaping input — only
            // the field comparison can tell these apart.
            ("text-align", "text-align: center", true),
            ("text-indent", "text-indent: 8px", true),
            // Shaping inputs.
            ("font-size", "font-size: 24px", false),
            ("font-weight", "font-weight: 700", false),
            ("letter-spacing", "letter-spacing: 2px", false),
            ("word-break", "word-break: break-all", false),
        ] {
            let (mut document, label, run) = label_document_parts("hello world", 200.0);
            document.layout();
            assert!(
                document
                    .text_store(run)
                    .expect("retained")
                    .committed()
                    .is_some()
            );

            document.set_inline_style(label, declaration);
            document.flush_styles_with_damage_sink(&mut |_, _| {});

            assert_eq!(
                document
                    .text_store(run)
                    .expect("the store outlives its artifact")
                    .retained()
                    .is_some(),
                survives,
                "{case}: shaped text should {} the restyle",
                if survives { "survive" } else { "not survive" },
            );
            assert!(
                document.layout_cache_is_empty(run).expect("live text node"),
                "{case}: a relayout always drops the text child's box cache",
            );
        }
    }

    #[test]
    fn a_repaint_only_restyle_never_reaches_the_text_eviction_path() {
        let (mut document, label, run) = label_document_parts("hello world", 200.0);
        document.layout();

        document.set_inline_style(label, "color: rgb(255, 0, 0)");
        document.flush_styles_with_damage_sink(&mut |_, _| {});

        assert!(
            document
                .text_store(run)
                .expect("retained")
                .retained()
                .is_some()
        );
        assert!(
            !document.layout_cache_is_empty(run).expect("live text node"),
            "a repaint-only change leaves even the box cache alone",
        );
    }

    #[test]
    fn a_re_measured_text_node_ends_the_pass_on_its_committed_break() {
        let (mut document, run) = label_document("hello world", 200.0);
        document.layout();
        let first = document.text_layout(run).expect("committed").max_advance();

        // Force the whole spine to re-measure, then check the retained layout
        // did not end the pass holding an intrinsic-sizing probe's break.
        document.invalidate_layout(run);
        document.layout();

        let store = document.text_store(run).expect("retained");
        assert!(!store.is_probe_dirty());
        let committed = store.committed().expect("committed after re-measure");
        assert_eq!(committed.max_advance(), first);
        assert_eq!(committed.line_count(), 1);
    }

    #[test]
    fn internal_natural_size_update_invalidates_the_dirty_spine() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            let slot = document.live_slot(id);
            document
                .layout_state_mut()
                .at_mut(slot)
                .slot
                .store_cached_layout(input, LayoutOutput::default());
        }

        let natural_size = NaturalSize::from_size(Size::new(40.0, 20.0));
        document.set_natural_size(image, natural_size);

        assert_eq!(document.get(image).unwrap().natural_size(), natural_size);
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(document.layout_cache_is_empty(id), Some(true));
        }
    }

    /// Becoming replaced is a layout change, not just a paint change:
    /// `is_replaced` forces `DisplayMode::Leaf`, which sizes the box from its
    /// natural size and hides every child. A source that arrives before any
    /// natural size — the ordinary order, because the source comes from an
    /// attribute and the size only from a completed load — must therefore
    /// invalidate layout, or the document keeps laying the node out as a
    /// container while painting it as an image.
    #[test]
    fn the_source_that_makes_an_element_replaced_invalidates_layout() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);

        let prime = |document: &mut Document<()>| {
            let input = LayoutInput::default();
            for id in [DOCUMENT_NODE_ID, root, image] {
                let slot = document.live_slot(id);
                document
                    .layout_state_mut()
                    .at_mut(slot)
                    .slot
                    .store_cached_layout(input, LayoutOutput::default());
            }
        };

        prime(&mut document);
        document.set_image_source(image, Some("app:///a.png"));
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(true),
                "the first source flips the node to replaced, which layout reads",
            );
        }

        // A later source only changes which pixels the same replaced box
        // draws, so the retained boxes survive and only the scene is rebuilt.
        prime(&mut document);
        document.render();
        document.set_image_source(image, Some("app:///b.png"));
        assert!(
            document.needs_render(),
            "a new source invalidates the retained frame"
        );
        for id in [DOCUMENT_NODE_ID, root, image] {
            assert_eq!(
                document.layout_cache_is_empty(id),
                Some(false),
                "swapping one image for another changes no layout input",
            );
        }
    }

    /// Clearing a source an element never had must stay a no-op. Writing
    /// `Replaced` for it would turn an ordinary container into a childless
    /// zero-sized leaf, which is not what "there is no image here" means.
    #[test]
    fn clearing_a_source_never_set_leaves_the_element_alone() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let view = document.create_element("view", ());
        document.append_child(root, view);
        document.layout();
        document.render();

        document.set_image_source(view, None);

        assert!(!document.get(view).expect("live element").is_replaced());
        assert!(!document.needs_render(), "nothing changed");
    }

    /// Whether the node's committing parent proved its input survives any
    /// change confined to the node's own subtree — the license the in-place
    /// relayout path parks on.
    fn is_content_independent(doc: &Document<()>, id: crate::NodeId) -> bool {
        let slot = doc.live_slot(id);
        doc.layout_state()
            .get(slot)
            .is_some_and(|state| state.slot.committed_independent().is_some())
    }

    fn linear_page() -> Document<()> {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            // `overflow: hidden` is what the Lynx UA sheet gives every element,
            // and it is load-bearing here: it is what frees the flex list's
            // automatic minimum size from its content, without which the list
            // has no independent main axis for percentages to chain off.
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 100%; height: 100%; }
             .list { display: linear; linear-direction: column;
                     flex-grow: 1; flex-basis: 0px; }
             .row { display: linear; linear-direction: row; height: 56px; }
             .weighted { display: linear; linear-weight: 1; }
             .measured { display: linear; }
             .ratio { display: linear; width: 40px; aspect-ratio: 2; }
             .percent-row { display: linear; linear-direction: row; height: 50%; }",
            StylesheetOrigin::Author,
        );
        doc
    }

    fn child_of(doc: &mut Document<()>, parent: crate::NodeId, class: &str) -> crate::NodeId {
        let child = doc.create_element("view", ());
        doc.add_class(child, class);
        doc.append_child(parent, child);
        child
    }

    #[test]
    fn linear_imposes_content_independent_inputs_on_pinned_and_weighted_children() {
        let mut doc = linear_page();
        let root = doc.document_element().id();
        let list = child_of(&mut doc, root, "list");
        // A pinned main size and a stretched cross axis: both imposed. Linear
        // has no content-based automatic minimum, so `overflow` plays no part
        // here the way it does under flexbox.
        let row = child_of(&mut doc, list, "row");
        // Main size distributed from the row's own definite main size by
        // weight, cross size stretched to it.
        let weighted = child_of(&mut doc, row, "weighted");
        // Its main size is whatever its content measures.
        let measured = child_of(&mut doc, row, "measured");
        // A ratio ties the axes together: a definite width fixes the height.
        let ratio = child_of(&mut doc, row, "ratio");
        // A percentage main size resolves against the list's own main axis,
        // which the page imposes.
        let percent = child_of(&mut doc, list, "percent-row");

        doc.layout();

        assert!(is_content_independent(&doc, row));
        assert!(is_content_independent(&doc, weighted));
        assert!(is_content_independent(&doc, ratio));
        assert!(is_content_independent(&doc, percent));
        assert!(
            !is_content_independent(&doc, measured),
            "a content-measured main size cannot license an in-place relayout",
        );
    }

    #[test]
    fn grid_relative_and_out_of_flow_children_carry_their_own_independence() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            // Lynx's UA `overflow: hidden` is again load-bearing: it is what
            // frees a grid item's automatic minimum from its content, which is
            // what lets a flexible track stay put under it.
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .grid { display: grid; grid-template-columns: 100px 1fr;
                     grid-template-rows: 60px; width: 300px; height: 60px; }
             .pinned { width: 40px; }
             .relative { display: relative; width: 200px; height: 100px; }
             .sized { width: 30px; height: 30px; }
             .abs { position: absolute; left: 4px; top: 4px; width: 20px; height: 20px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let grid = child_of(&mut doc, root, "grid");
        // Fixed column, fixed row: the area cannot move at all.
        let fixed_cell = child_of(&mut doc, grid, "");
        // Flexible column, but a pinned width keeps this item's contribution
        // out of the track's sizing.
        let pinned_cell = child_of(&mut doc, grid, "pinned");
        let relative = child_of(&mut doc, root, "relative");
        let anchored = child_of(&mut doc, relative, "sized");
        let measured = child_of(&mut doc, relative, "");
        let out_of_flow = child_of(&mut doc, relative, "abs");

        doc.layout();

        assert!(is_content_independent(&doc, fixed_cell));
        assert!(is_content_independent(&doc, pinned_cell));
        assert!(is_content_independent(&doc, anchored));
        assert!(
            is_content_independent(&doc, out_of_flow),
            "an out-of-flow box is sized from its containing block and its own \
             style, never from a measurement of itself",
        );
        assert!(
            !is_content_independent(&doc, measured),
            "a relative child with no imposed size is whatever it measures",
        );
    }

    #[test]
    fn a_grid_item_in_a_flexible_track_follows_its_own_content_without_a_pinned_size() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page, view { overflow: hidden; box-sizing: border-box; }
             page { display: flex; flex-direction: column; width: 300px; height: 300px; }
             .grid { display: grid; grid-template-columns: 1fr;
                     grid-template-rows: auto; width: 300px; height: 60px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();
        let grid = child_of(&mut doc, root, "grid");
        let cell = child_of(&mut doc, grid, "");
        doc.layout();

        assert!(
            !is_content_independent(&doc, cell),
            "an auto row sizes itself from this item, so the item's own area \
             moves with its content",
        );
    }

    #[test]
    fn a_linear_subtree_mutation_parks_instead_of_reaching_the_root() {
        let mut doc = linear_page();
        let root = doc.document_element().id();
        let list = child_of(&mut doc, root, "list");
        let row = child_of(&mut doc, list, "row");
        let cell = child_of(&mut doc, row, "weighted");
        let leaf = child_of(&mut doc, cell, "measured");
        doc.layout();

        let viewport = Size::new(800.0, 600.0);
        doc.invalidate_layout(leaf);
        assert!(
            !doc.layout_requires_full_pass(viewport, 1.0),
            "a mutation under a content-independent linear ancestor stays incremental",
        );
    }

    #[test]
    fn only_a_root_reaching_invalidation_forces_a_full_pass() {
        let mut doc: Document<()> =
            Document::new(crate::tree::document::tests::device(), "page", ());
        doc.add_stylesheet(
            "page { display: flex; width: 300px; height: 100px; }
             .box { display: flex; contain: strict; width: 80px; height: 40px; }
             .skip { display: flex; content-visibility: hidden;
                     contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.document_element().id();

        let boundary = doc.create_element("view", ());
        doc.add_class(boundary, "box");
        doc.append_child(root, boundary);
        let c1 = doc.create_element("view", ());
        doc.add_class(c1, "leaf");
        doc.append_child(boundary, c1);
        let c2 = doc.create_element("view", ());
        doc.add_class(c2, "leaf");
        doc.append_child(boundary, c2);

        let plain = doc.create_element("view", ());
        doc.add_class(plain, "leaf");
        doc.append_child(root, plain);

        let skip = doc.create_element("view", ());
        doc.add_class(skip, "skip");
        doc.append_child(root, skip);
        let hidden_child = doc.create_element("view", ());
        doc.add_class(hidden_child, "leaf");
        doc.append_child(skip, hidden_child);

        doc.layout();

        let viewport = Size::new(800.0, 600.0);
        let scale = 1.0;
        assert!(
            !doc.layout_needs_pass(viewport, scale),
            "an unchanged frame after layout needs no pass at all",
        );

        doc.invalidate_layout(hidden_child);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a skipped-contents mutation must not force a whole-tree pass",
        );

        doc.invalidate_layout(c1);
        doc.invalidate_layout(c2);
        assert!(
            !doc.layout_requires_full_pass(viewport, scale),
            "a second mutation under one parked boundary must stay incremental",
        );

        doc.invalidate_layout(plain);
        assert!(
            doc.layout_requires_full_pass(viewport, scale),
            "a root-reaching mutation forces a whole-tree pass",
        );
    }
}
