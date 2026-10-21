//! Restyle damage — what a style change means for downstream layout/paint.

use stylo::properties::ComputedValues;
use stylo::servo::restyle_damage::ServoRestyleDamage;

use crate::tree::node::Node;

/// What one element's primary style change moved beyond Stylo's longhand
/// damage, decided while the old style is still alive and filed in the
/// embedder bits of the element's damage ([`ServoRestyleDamage::EMBEDDER`]).
///
/// The traversal drops an element's old style as soon as it has restyled
/// it, so the comparisons that need both styles are taken there
/// ([`primary_style_verdicts`], Stylo's
/// `TElement::compute_primary_style_damage`) and the damage harvest reads
/// the answers ([`crate::tree::node::Node::take_style_refresh`]) — no
/// reference to the old style outlives the traversal.
pub(crate) mod verdict {
    use stylo::servo::restyle_damage::ServoRestyleDamage;

    /// The primary style is a different `Arc` than before. Set for every
    /// replaced primary style, so this alone is what makes an element
    /// restyled to the harvest.
    pub(crate) const REPLACED: ServoRestyleDamage = ServoRestyleDamage::from_bits_retain(1 << 8);
    /// The paragraph's effective custom-property limits changed: layout
    /// inputs outside Stylo's longhand comparison, which gives a custom
    /// property change repaint damage only.
    pub(crate) const PARAGRAPH_LIMITS: ServoRestyleDamage =
        ServoRestyleDamage::from_bits_retain(1 << 9);
    /// `display` or `content` changed, so the boxes the element generates
    /// may have changed shape.
    pub(crate) const BOX_TREE: ServoRestyleDamage = ServoRestyleDamage::from_bits_retain(1 << 10);
    /// The element started or stopped establishing the containing block of
    /// its absolute or fixed descendants.
    pub(crate) const CONTAINING_BLOCK: ServoRestyleDamage =
        ServoRestyleDamage::from_bits_retain(1 << 11);
}

/// The [`verdict`] bits for `node`'s primary style changing from `old` to a
/// different `new`. Called on the traversal's thread, with the element's own
/// style data borrowed for its restyle: it reads the two styles and node
/// facts no restyle moves, never a style through the node.
pub(crate) fn primary_style_verdicts<T>(
    node: &Node<T>,
    old: &ComputedValues,
    new: &ComputedValues,
) -> ServoRestyleDamage {
    let mut verdicts = verdict::REPLACED;
    if crate::layout::paragraph_limits_changed(old, new) {
        verdicts |= verdict::PARAGRAPH_LIMITS;
    }
    if *old.get_display() != *new.get_display()
        || old.get_counters().content != new.get_counters().content
    {
        verdicts |= verdict::BOX_TREE;
    }
    // Both roles read the element too (its root-ness, its transform
    // animation bit, its content-visibility relevance, its top-layer
    // membership), none of which a restyle moves, so evaluating each style
    // against the same node isolates what the style change did.
    let role = |style: &ComputedValues| {
        (
            crate::layout::establishes_fixed_containing_block(node, style),
            crate::layout::establishes_absolute_containing_block(node, style),
        )
    };
    if role(old) != role(new) {
        verdicts |= verdict::CONTAINING_BLOCK;
    }
    verdicts
}

/// The restyle damage produced for one node by a flush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StyleDamage(ServoRestyleDamage);

impl StyleDamage {
    /// Merge Stylo damage with the layout inputs read outside its longhands.
    /// Both normal restyles and animation ticks use the same classification.
    /// The [`verdict`] bits are not damage and are dropped.
    pub(crate) fn from_style_change(
        mut damage: ServoRestyleDamage,
        paragraph_limits_changed: bool,
    ) -> Option<Self> {
        damage.remove(ServoRestyleDamage::EMBEDDER);
        if paragraph_limits_changed {
            damage.insert(ServoRestyleDamage::RELAYOUT);
        }
        (!damage.is_empty()).then_some(Self(damage))
    }

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
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::StyleDamage;
    use crate::test_common::{Doc, rgb};
    use crate::tree::document::NodeId;

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
    fn registered_non_inherited_custom_property_change_reports_repaint_only() {
        let mut doc = Doc::with_css(
            "@property --limit { syntax: '<integer>'; inherits: false; initial-value: -1; }",
        );
        let el = doc.el(doc.root, "view");
        let child = doc.el(el, "view");
        doc.dom.set_inline_style_property(el, "--limit", "1");
        flush(&mut doc);
        assert_eq!(doc.value(el, "--limit"), "1");
        assert_eq!(doc.value(child, "--limit"), "-1");

        doc.dom.set_inline_style_property(el, "--limit", "2");
        let summary = flush(&mut doc);
        let damage = damage_of(&summary, el).expect("the registered property changed");
        assert!(damage.needs_repaint());
        assert!(!damage.needs_relayout());
        assert_eq!(doc.value(el, "--limit"), "2");
        assert_eq!(doc.value(child, "--limit"), "-1");
    }

    #[test]
    fn paragraph_limit_damage_tracks_effective_values() {
        for (property, initial) in [
            ("--lynx-text-maxline", "0"),
            ("--lynx-text-maxlength", "-1"),
        ] {
            let mut doc = Doc::with_css(&format!(
                "@property {property} {{ syntax: '<integer>'; inherits: false; initial-value: {initial}; }}"
            ));
            doc.define_lynx_text("text");
            let label = doc.el(doc.root, "text.label");
            flush(&mut doc);

            for (value, computed, relayout) in [
                ("1", "1", true),
                ("01", "1", false),
                ("2", "2", true),
                ("2px", initial, true),
                ("invalid", initial, false),
                ("calc(1 + 1)", "2", true),
                ("calc(1.5)", "2", false),
                ("1.5", initial, true),
                ("-1", "-1", false),
            ] {
                doc.dom.set_inline_style_property(label, property, value);
                let summary = flush(&mut doc);
                assert_eq!(doc.value(label, property), computed, "{property}: {value}");
                let damage = damage_of(&summary, label);
                if let Some(damage) = damage {
                    assert!(damage.needs_repaint());
                }
                assert_eq!(
                    damage.is_some_and(StyleDamage::needs_relayout),
                    relayout,
                    "{property}: changing to {value}"
                );
            }
        }
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
