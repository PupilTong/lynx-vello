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
use stylo::properties::ComputedValues;
use stylo::servo_arc::Arc;

pub(crate) use self::style::{
    DisplayMode, StyleView, box_parent, display_mode, establishes_absolute_containing_block,
    establishes_fixed_containing_block, skips_contents,
};
use crate::tree::document::{Document, NodeLayoutState};

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
        host::run_layout(self, viewport, scale, full);
        self.clear_relayout_roots();
        self.mark_layout_complete(viewport, scale);
    }
}

impl<T> Document<T> {
    /// Updates intrinsic dimensions and invalidates affected layout.
    pub fn set_natural_size(&mut self, id: crate::NodeId, natural_size: NaturalSize) {
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

    #[must_use]
    pub(crate) fn natural_size(&self, id: crate::NodeId) -> NaturalSize {
        self.get(id)
            .map_or(NaturalSize::NONE, crate::Node::natural_size)
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

    #[must_use]
    pub fn rounded_layout(&self, id: crate::NodeId) -> Option<&Layout> {
        self.layout_state()
            .nodes
            .get(id)
            .map(|state| &state.slot.rounded)
    }

    #[must_use]
    pub(crate) fn text_layout(&self, id: crate::NodeId) -> Option<&TextLayout> {
        self.layout_state()
            .nodes
            .get(id)?
            .text
            .as_deref()?
            .committed()
    }

    #[must_use]
    pub(crate) fn paint_style(&self, id: crate::NodeId) -> Option<&ComputedValues> {
        self.get(id)?.layout_computed_style()
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn layout_cache_is_empty(&self, id: crate::NodeId) -> Option<bool> {
        self.layout_state()
            .nodes
            .get(id)
            .map(|state| state.slot.layout_cache_is_empty())
    }

    pub(crate) fn invalidate_layout(&mut self, id: crate::NodeId) {
        let (boundary, reached_root) = {
            let (tree, state, parked) = self.layout_parts();
            let start = tree
                .nodes
                .get(id)
                .expect("vacant NodeId passed to Document::invalidate_layout");
            state.clear_layout_cache(id);

            let mut boundary = None;
            let mut reached_root = true;
            let mut current = start.flat_parent_id();
            while let Some(node_id) = current {
                let node = tree
                    .nodes
                    .get(node_id)
                    .expect("internal tree link must resolve to a live node");
                let style_view = node.is_element().then(|| StyleView::of(node));
                if style_view.as_ref().is_some_and(CoreStyle::skips_contents) {
                    reached_root = false;
                    break;
                }
                let is_boundary = style_view.as_ref().is_some_and(is_relayout_boundary);
                if is_boundary && parked.contains(&node_id) {
                    reached_root = false;
                    break;
                }
                let boundary_input = is_boundary
                    .then(|| {
                        state
                            .nodes
                            .get(node_id)
                            .expect("live node must have layout-arena state")
                            .slot
                            .committed_input()
                    })
                    .flatten();
                state.clear_layout_cache(node_id);
                if let Some(input) = boundary_input {
                    boundary = Some((node_id, input));
                    reached_root = false;
                    break;
                }
                current = node.flat_parent_id();
            }
            (boundary, reached_root)
        };
        self.mark_layout_dirty(reached_root);
        if let Some((boundary_id, committed_input)) = boundary {
            self.record_relayout_root(boundary_id, committed_input);
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
            (if cfg!(debug_assertions) { 232 } else { 224 }, 640, 656, 16,),
            "Node, LayoutSlot, NodeLayoutState, and TextLayoutStore sizes changed",
        );
    }

    #[test]
    fn internal_natural_size_update_invalidates_the_dirty_spine() {
        let mut document = Document::new(crate::tree::document::tests::device(), "page", ());
        let root = document.document_element().id();
        let image = document.create_element("image", ());
        document.append_child(root, image);

        let input = LayoutInput::default();
        for id in [DOCUMENT_NODE_ID, root, image] {
            document
                .layout_state_mut()
                .nodes
                .get_mut(id)
                .expect("live node has aligned layout state")
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
