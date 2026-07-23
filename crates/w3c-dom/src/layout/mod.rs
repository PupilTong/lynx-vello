//! Box layout over the document tree — the concrete [`neutron_star`] host.

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
    pub(crate) measure_cache: Cache,
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

pub(crate) static ANONYMOUS_STYLE: LazyLock<Arc<ComputedValues>> = LazyLock::new(|| {
    use stylo::properties::style_structs::Font;
    ComputedValues::initial_values_with_font_override(Font::initial_values())
});

impl<T: Sync> Document<T> {
    pub fn layout(&mut self) {
        self.flush_styles_with_damage_sink(Parallelism::Auto, &mut |_, _| {});

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

    pub fn invalidate_layout(&mut self, id: crate::NodeId) {
        let (boundary, reached_root) = {
            let tree = self.tree();
            let start = tree
                .get(id)
                .expect("vacant NodeId passed to Document::invalidate_layout");
            start.layout_data().borrow_mut().clear_measurement_cache();
            start.invalidate_text_artifacts();

            let mut boundary = None;
            let mut reached_root = true;
            let mut current = start.parent();
            while let Some(node) = current {
                let style_view = node.is_element().then(|| StyleView::of(node));
                if style_view.as_ref().is_some_and(CoreStyle::skips_contents) {
                    reached_root = false;
                    break;
                }
                let is_boundary = style_view.as_ref().is_some_and(is_relayout_boundary);
                if is_boundary && self.is_relayout_root_parked(node.id()) {
                    reached_root = false;
                    break;
                }
                let boundary_input = is_boundary
                    .then(|| node.layout_data().borrow().measure_cache.committed_input())
                    .flatten();
                node.layout_data().borrow_mut().clear_measurement_cache();
                node.invalidate_text_artifacts();
                if let Some(input) = boundary_input {
                    boundary = Some((node.id(), input));
                    reached_root = false;
                    break;
                }
                current = node.parent();
            }
            (boundary, reached_root)
        };
        self.mark_layout_dirty(reached_root);
        if let Some((boundary_id, committed_input)) = boundary {
            self.record_relayout_root(boundary_id, committed_input);
        }
    }

    pub fn invalidate_layout_all(&mut self) {
        for (_, data) in self.layout_data_mut() {
            data.get_mut().clear_measurement_cache();
        }
        for (_, node) in self.tree_mut().iter_mut() {
            node.invalidate_text_artifacts();
        }
        self.clear_relayout_roots();
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
        document.append_document_element(root);
        let image = document.create_element("image", ());
        document.append_child(root, image);

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

    #[test]
    fn only_a_root_reaching_invalidation_forces_a_full_pass() {
        let mut doc: Document<()> = Document::new(crate::document::tests::device());
        doc.add_stylesheet(
            "page { display: flex; width: 300px; height: 100px; }
             .box { display: flex; contain: strict; width: 80px; height: 40px; }
             .skip { display: flex; content-visibility: hidden;
                     contain-intrinsic-size: 40px 30px; width: 40px; height: 30px; }
             .leaf { width: 10px; height: 10px; }",
            StylesheetOrigin::Author,
        );
        let root = doc.create_element("page", ());
        doc.append_document_element(root);

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
