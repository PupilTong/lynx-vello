//! Restyle damage — what a style change means for downstream layout/paint.

use stylo::servo::restyle_damage::ServoRestyleDamage;

/// The restyle damage produced for one node by a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StyleDamage(ServoRestyleDamage);

impl StyleDamage {
    #[must_use]
    pub(crate) fn needs_relayout(self) -> bool {
        self.0.contains(ServoRestyleDamage::RELAYOUT)
    }

    #[must_use]
    #[cfg(test)]
    fn needs_overflow_recalculation(self) -> bool {
        self.0.contains(ServoRestyleDamage::RECALCULATE_OVERFLOW)
    }

    #[must_use]
    #[cfg(test)]
    fn needs_stacking_context_rebuild(self) -> bool {
        self.0
            .contains(ServoRestyleDamage::REBUILD_STACKING_CONTEXT)
    }

    #[must_use]
    #[cfg(test)]
    fn needs_repaint(self) -> bool {
        self.0.contains(ServoRestyleDamage::REPAINT)
    }

    #[must_use]
    pub(crate) fn requires_reconstruction(self) -> bool {
        self.0.bits() == u16::MAX
    }
}

impl From<ServoRestyleDamage> for StyleDamage {
    fn from(damage: ServoRestyleDamage) -> Self {
        Self(damage)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::StyleDamage;
    use crate::document::NodeId;
    use crate::test_common::{Doc, rgb};

    type DamageSummary = Vec<(NodeId, StyleDamage)>;

    fn flush(doc: &mut Doc) -> DamageSummary {
        let mut summary = Vec::new();
        doc.dom
            .flush_styles_with_damage_sink(&mut |node_id, damage| {
                summary.push((node_id, damage));
            });
        summary
    }

    fn damage_of(summary: &DamageSummary, id: NodeId) -> Option<StyleDamage> {
        summary
            .iter()
            .find_map(|&(node_id, damage)| (node_id == id).then_some(damage))
    }

    #[test]
    fn class_flip_on_color_rule_reports_repaint_only() {
        let mut doc = Doc::with_css(".hot { color: rgb(255, 0, 0) }");
        let el = doc.el(doc.root, "view");
        let initial = flush(&mut doc);
        assert!(initial.is_empty(), "initial styling produces no damage");

        doc.add_class(el, "hot");
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, el).expect("the flipped node carries damage");
        assert!(damage.needs_repaint());
        assert!(!damage.needs_relayout());
        assert!(!damage.needs_overflow_recalculation());
        assert!(!damage.needs_stacking_context_rebuild());
        assert_eq!(summary.len(), 1, "only the flipped node is damaged");
    }

    #[test]
    fn class_flip_on_width_rule_reports_relayout() {
        let mut doc = Doc::with_css(".wide { width: 100px }");
        let el = doc.el(doc.root, "view");
        flush(&mut doc);

        doc.add_class(el, "wide");
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, el).expect("the flipped node carries damage");
        assert!(damage.needs_relayout());
        assert!(damage.needs_repaint());
        assert!(damage.needs_overflow_recalculation());
    }

    #[test]
    fn inline_width_change_reports_relayout() {
        let mut doc = Doc::new();
        let el = doc.el(doc.root, "view");
        doc.set_inline(el, "width: 10px");
        let initial = flush(&mut doc);
        assert!(initial.is_empty(), "initial styling produces no damage");

        doc.set_inline(el, "width: 20px");
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, el).expect("the inline width change carries damage");
        assert!(damage.needs_relayout());
    }

    #[test]
    fn empty_flip_structural_path_reports_relayout() {
        let mut doc = Doc::with_css(".box:empty { height: 50px }");
        let box_id = doc.el(doc.root, "view.box");
        flush(&mut doc);
        assert_eq!(doc.value(box_id, "height"), "50px");

        let child = doc.el(box_id, "view");
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, box_id).expect(":empty flip damages the container");
        assert!(damage.needs_relayout());
        assert_eq!(doc.value(box_id, "height"), "auto");
        assert!(damage_of(&summary, child).is_none());
    }

    #[test]
    fn edge_child_structural_path_reports_relayout() {
        let mut doc = Doc::with_css(".list > view:first-child { width: 30px }");
        let list = doc.el(doc.root, "view.list");
        let first = doc.el(list, "view");
        flush(&mut doc);
        assert_eq!(doc.value(first, "width"), "30px");

        let new_first = doc.dom.create_element("view", ());
        doc.dom.insert_before(list, new_first, Some(first));
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, first).expect("displaced first-child re-styles");
        assert!(damage.needs_relayout());
        assert_eq!(doc.value(first, "width"), "auto");
        assert_eq!(doc.value(new_first, "width"), "30px");
    }

    #[test]
    fn damage_is_cleared_after_harvest() {
        let mut doc = Doc::with_css(".wide { width: 100px }");
        let el = doc.el(doc.root, "view");
        flush(&mut doc);

        doc.add_class(el, "wide");
        let first = flush(&mut doc);
        assert!(!first.is_empty(), "the incremental flush reports damage");

        let second = flush(&mut doc);
        assert!(second.is_empty(), "damage must not survive the harvest");
    }

    #[test]
    fn display_none_flip_reports_relayout_and_leaves_no_stale_state() {
        let mut doc = Doc::with_css(".gone { display: none }");
        let parent = doc.el(doc.root, "view");
        let child = doc.el(parent, "view");
        flush(&mut doc);
        assert!(doc.dom.get(child).unwrap().computed_style().is_some());

        doc.add_class(parent, "gone");
        let summary = flush(&mut doc);
        let damage = damage_of(&summary, parent).expect("the display flip damages the node");
        assert!(damage.needs_relayout());

        let second = flush(&mut doc);
        assert!(second.is_empty(), "no stale damage from the pruned subtree");
    }

    #[test]
    fn sibling_invalidation_damage_is_harvested_and_cleared() {
        let mut doc = Doc::with_css(".a + .b { color: rgb(255, 0, 0) }");
        let a = doc.el(doc.root, "view");
        let b = doc.el(doc.root, "view.b");
        let initial = flush(&mut doc);
        assert!(initial.is_empty(), "initial styling produces no damage");
        assert_ne!(doc.color(b), rgb(255, 0, 0));

        doc.add_class(a, "a");
        let summary = flush(&mut doc);

        let damage = damage_of(&summary, b).expect("the invalidated sibling carries damage");
        assert!(damage.needs_repaint());
        assert!(!damage.needs_relayout());
        assert_eq!(doc.color(b), rgb(255, 0, 0));

        let second = flush(&mut doc);
        assert!(second.is_empty(), "no leaked damage after the flush");
    }
}
