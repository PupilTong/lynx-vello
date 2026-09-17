//! Computed- and resolved-value readback: the CSSOM
//! `getComputedStyle(el).getPropertyValue(name)` and Typed OM
//! `el.computedStyleMap()` answers, served straight off the style the last
//! flush left on the node.
//!
//! Neither entry point runs style, layout or paint. They read the styles the
//! last completed pass produced and — for the resolved set — the box that pass
//! measured, so an element that has never been flushed answers `None`/empty and
//! a mutation made after the last flush is invisible here until the caller
//! flushes. That is deliberate: the readback is a pure query, and the embedder
//! decides when a pipeline pass runs.
//!
//! Two name spaces meet here and only one of them is stylo's full longhand
//! table. The fork compiles a set of non-authorable longhands
//! (`LYNX_INTERNAL_LONGHANDS` in `properties/data.py` — `writing-mode`,
//! `float`, `mix-blend-mode`, `rotate`, `zoom`, `column-count`, …) that the
//! cascade needs as storage but that never enter the author-facing name map.
//! They pass [`NonCustomPropertyId`]'s `enabled_for_all_content` check and have
//! a `name()`, so enumeration has to filter them back out by asking the author
//! parser whether it would accept the name — which is what
//! [`PropertyId::parse_enabled_for_all_content`] answers, and why the
//! enumeration below round-trips every name through it.
//!
//! Logical longhands (`margin-inline-start` and friends) stay in the
//! enumeration under their logical names; stylo physicalizes them against the
//! element's writing mode as it serializes
//! (`ComputedValues::computed_or_resolved_value`), so `margin-inline-start` and
//! `margin-left` report the same text in a horizontal-tb ltr element.

use std::collections::HashSet;

use stylo::computed_values::box_sizing;
use stylo::properties::{
    ComputedValues, LonghandId, NonCustomPropertyId, PropertyDeclarationId, PropertyId,
};
use stylo::values::computed::CSSPixelLength;
use stylo_traits::ToCss;

use crate::tree::document::{Document, NodeId};
use crate::tree::node::Node;

/// The box edge a resolved-value substitution reads off the last layout.
///
/// Only these longhands differ between the computed and the resolved value
/// here; every other property resolves to its computed value, which is what
/// CSSOM asks for and what both references report.
#[derive(Clone, Copy)]
enum UsedBoxValue {
    MarginTop,
    MarginRight,
    MarginBottom,
    MarginLeft,
    PaddingTop,
    PaddingRight,
    PaddingBottom,
    PaddingLeft,
    Width,
    Height,
}

impl UsedBoxValue {
    /// The physical longhand this substitution answers for, or `None` when the
    /// property has no used value distinct from its computed one.
    fn of(physical: LonghandId) -> Option<Self> {
        Some(match physical {
            LonghandId::MarginTop => Self::MarginTop,
            LonghandId::MarginRight => Self::MarginRight,
            LonghandId::MarginBottom => Self::MarginBottom,
            LonghandId::MarginLeft => Self::MarginLeft,
            LonghandId::PaddingTop => Self::PaddingTop,
            LonghandId::PaddingRight => Self::PaddingRight,
            LonghandId::PaddingBottom => Self::PaddingBottom,
            LonghandId::PaddingLeft => Self::PaddingLeft,
            LonghandId::Width => Self::Width,
            LonghandId::Height => Self::Height,
            _ => return None,
        })
    }
}

impl<T> Document<T> {
    /// One property's computed — or, with `resolved`, resolved — value, as
    /// CSSOM serializes it.
    ///
    /// `None` for a name the author-facing parser does not accept (an unknown
    /// property, or one of the fork's internal non-authorable longhands), for a
    /// shorthand, for a stale or non-element id, and for an element the style
    /// pass has not reached yet. A custom property name that is simply unset
    /// answers `Some("")`, the way `getPropertyValue` does.
    ///
    /// The name is matched as given. Stylo's own lookup is ASCII
    /// case-insensitive for non-custom names and exact for `--*` names; nothing
    /// is folded or camel-case-converted on the way in.
    ///
    /// With `resolved`, `margin-*`, `padding-*`, `width` and `height` report
    /// the used values of the last layout pass instead of their computed ones.
    /// Those numbers come from the *rounded* layout, so they are snapped to
    /// device pixels; every other property, insets included, reports its
    /// computed value.
    #[must_use]
    pub fn computed_style_text(
        &self,
        id: NodeId,
        property: &str,
        resolved: bool,
    ) -> Option<String> {
        let style = self.get(id)?.computed_style()?;
        let parsed = PropertyId::parse_enabled_for_all_content(property).ok()?;
        match parsed.as_shorthand().err()? {
            PropertyDeclarationId::Longhand(longhand) => {
                Some(self.longhand_text(id, &style, longhand, resolved))
            }
            custom @ PropertyDeclarationId::Custom(_) => {
                Some(style.computed_value_to_string(custom))
            }
        }
    }

    /// Every author-facing longhand, then every custom property that has a
    /// value on the element, as `(name, value)` pairs.
    ///
    /// An empty `filter` asks for all of them, both groups ordered by code
    /// point, longhands first — the order a Typed OM `computedStyleMap()`
    /// iterates in. A non-empty `filter` answers each listed name through
    /// [`Document::computed_style_text`], in the filter's own order and under
    /// the name as given, so it matches the way that entry point does and a
    /// name it would refuse is simply absent. Shorthands are absent either
    /// way.
    ///
    /// Empty for a stale or non-element id and for an element the style pass
    /// has not reached yet. `resolved` carries the same meaning as in
    /// [`Document::computed_style_text`], including the device-pixel snapping.
    #[must_use]
    pub fn computed_style_entries(
        &self,
        id: NodeId,
        filter: &[&str],
        resolved: bool,
    ) -> Vec<(String, String)> {
        if !filter.is_empty() {
            return filter
                .iter()
                .filter_map(|name| {
                    self.computed_style_text(id, name, resolved)
                        .map(|value| ((*name).to_owned(), value))
                })
                .collect();
        }
        let Some(style) = self.get(id).and_then(Node::computed_style) else {
            return Vec::new();
        };

        let mut longhands: Vec<(String, String)> = NonCustomPropertyId::iter()
            .filter_map(NonCustomPropertyId::as_longhand)
            .filter(|longhand| {
                let name = longhand.name();
                // The author parser is the authority on what is author-facing:
                // the fork's internal longhands have a name here but no entry
                // in the name map.
                PropertyId::parse_enabled_for_all_content(name).is_ok()
            })
            .map(|longhand| {
                (
                    longhand.name().to_owned(),
                    self.longhand_text(id, &style, longhand, resolved),
                )
            })
            .collect();
        longhands.sort_by(|left, right| left.0.cmp(&right.0));

        // The upstream iterator walks the copy-on-write chain leaf-first and
        // means to skip an ancestor entry a descendant overrode, but its
        // `continue` continues the inner `for` instead of the outer `loop`
        // (`custom_properties_map.rs`), so the overridden entry is yielded a
        // second time. Dedupe by name with the first — the descendant's —
        // occurrence winning.
        let properties = style.custom_properties();
        let mut seen: HashSet<String> = HashSet::new();
        let mut customs: Vec<(String, String)> = Vec::new();
        for (name, value) in properties
            .inherited
            .iter()
            .chain(properties.non_inherited.iter())
        {
            if value.is_none() {
                // `CustomPropertiesMap::remove` stores a `None`: the name is in
                // the map precisely to shadow an ancestor's value.
                continue;
            }
            let display_name = format!("--{name}");
            if !seen.insert(display_name.clone()) {
                continue;
            }
            customs.push((
                display_name,
                style.computed_value_to_string(PropertyDeclarationId::Custom(name)),
            ));
        }
        customs.sort_by(|left, right| left.0.cmp(&right.0));

        longhands.extend(customs);
        longhands
    }

    fn longhand_text(
        &self,
        id: NodeId,
        style: &ComputedValues,
        longhand: LonghandId,
        resolved: bool,
    ) -> String {
        if resolved
            && let Some(used) =
                self.used_box_value(id, style, longhand.to_physical(style.writing_mode))
        {
            return used;
        }
        style.computed_value_to_string(PropertyDeclarationId::Longhand(longhand))
    }

    /// The used value of a box-edge longhand, or `None` when the element has no
    /// box in the last layout pass (and so nothing to resolve against).
    fn used_box_value(
        &self,
        id: NodeId,
        style: &ComputedValues,
        physical: LonghandId,
    ) -> Option<String> {
        let wanted = UsedBoxValue::of(physical)?;
        let display = style.clone_display();
        if display.is_none() || display.is_contents() {
            return None;
        }
        let layout = self.rounded_layout(id)?;
        let px = match wanted {
            UsedBoxValue::MarginTop => layout.margin.top,
            UsedBoxValue::MarginRight => layout.margin.right,
            UsedBoxValue::MarginBottom => layout.margin.bottom,
            UsedBoxValue::MarginLeft => layout.margin.left,
            UsedBoxValue::PaddingTop => layout.padding.top,
            UsedBoxValue::PaddingRight => layout.padding.right,
            UsedBoxValue::PaddingBottom => layout.padding.bottom,
            UsedBoxValue::PaddingLeft => layout.padding.left,
            UsedBoxValue::Width | UsedBoxValue::Height => {
                let vertical = matches!(wanted, UsedBoxValue::Height);
                let border_box = if vertical {
                    layout.size.height
                } else {
                    layout.size.width
                };
                if style.clone_box_sizing() == box_sizing::T::BorderBox {
                    border_box
                } else {
                    let edges = if vertical {
                        layout.border.top
                            + layout.border.bottom
                            + layout.padding.top
                            + layout.padding.bottom
                    } else {
                        layout.border.left
                            + layout.border.right
                            + layout.padding.left
                            + layout.padding.right
                    };
                    (border_box - edges).max(0.0)
                }
            }
        };
        Some(CSSPixelLength::new(px).to_css_string())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::test_common::Doc;

    fn names(entries: &[(String, String)]) -> Vec<&str> {
        entries.iter().map(|(name, _)| name.as_str()).collect()
    }

    fn lookup<'a>(entries: &'a [(String, String)], name: &str) -> Option<&'a str> {
        entries
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn a_property_answers_none_until_the_first_flush() {
        let mut doc = Doc::with_css("page { color: red }");

        assert_eq!(doc.dom.computed_style_text(doc.root, "color", false), None);
        assert!(
            doc.dom
                .computed_style_entries(doc.root, &[], false)
                .is_empty()
        );

        doc.flush();

        assert_eq!(
            doc.dom
                .computed_style_text(doc.root, "color", false)
                .as_deref(),
            Some("rgb(255, 0, 0)"),
        );
    }

    /// The fork's internal longhands pass `enabled_for_all_content` and have a
    /// `name()`, but no author can write them; enumeration has to drop them.
    #[test]
    fn enumeration_lists_author_facing_longhands_in_code_point_order() {
        let mut doc = Doc::new();
        doc.flush();

        let entries = doc.dom.computed_style_entries(doc.root, &[], false);
        let listed = names(&entries);

        assert!(listed.contains(&"margin-inline-start"));
        assert!(listed.contains(&"direction"));
        for internal in [
            "writing-mode",
            "float",
            "mix-blend-mode",
            "rotate",
            "scale",
            "translate",
            "zoom",
            "column-count",
        ] {
            assert!(
                !listed.contains(&internal),
                "`{internal}` is not author-facing and must not be enumerated",
            );
        }

        let mut sorted = listed.clone();
        sorted.sort_unstable();
        assert_eq!(listed, sorted, "longhands are ordered by code point");
    }

    #[test]
    fn a_shorthand_name_has_no_computed_value() {
        let mut doc = Doc::new();
        doc.flush();

        assert_eq!(doc.dom.computed_style_text(doc.root, "margin", false), None);
        assert_eq!(doc.dom.computed_style_text(doc.root, "all", false), None);
        assert_eq!(
            doc.dom.computed_style_text(doc.root, "writing-mode", false),
            None,
        );
        assert_eq!(
            doc.dom.computed_style_text(doc.root, "nonesuch", false),
            None
        );
    }

    #[test]
    fn custom_properties_are_deduped_inherited_and_removable() {
        let mut doc =
            Doc::with_css("page { --a: 1; --b: 2; --c: 4 } .child { --a: 3; --c: initial }");
        let child = doc.el(doc.root, "view.child");
        doc.flush();

        let entries = doc.dom.computed_style_entries(child, &[], false);
        let listed = names(&entries);

        assert_eq!(
            listed.iter().filter(|name| **name == "--a").count(),
            1,
            "an overridden ancestor entry is yielded twice upstream",
        );
        assert_eq!(lookup(&entries, "--a"), Some("3"));
        assert_eq!(lookup(&entries, "--b"), Some("2"));
        assert_eq!(lookup(&entries, "--c"), None);

        let first_custom = listed
            .iter()
            .position(|name| name.starts_with("--"))
            .expect("a custom property");
        assert!(
            listed[first_custom..]
                .iter()
                .all(|name| name.starts_with("--")),
            "custom properties come after every longhand",
        );

        assert_eq!(
            doc.dom.computed_style_text(child, "--a", false).as_deref(),
            Some("3"),
        );
        assert_eq!(
            doc.dom.computed_style_text(child, "--c", false).as_deref(),
            Some(""),
        );
    }

    /// `width: 50%` of a 200px containing block is a 100px *border* box under
    /// `border-box` and a 100px *content* box under `content-box`, so the two
    /// report the same text while the boxes they name differ by the padding:
    /// each box-sizing is read in its own frame of reference. Where the two
    /// part company is a used width the author did not write — the stretched
    /// item below.
    #[test]
    fn width_resolves_against_the_last_layout_per_box_sizing() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 400px; height: 400px }
             .parent { display: flex; flex-direction: column;
                       width: 200px; height: 100px }
             .child { width: 50%; height: 20px; padding: 10px }
             .stretchy { height: 20px; padding: 10px }
             .border-box { box-sizing: border-box }
             .content-box { box-sizing: content-box }",
        );
        let parent = doc.el(doc.root, "view.parent");
        let border_box = doc.el(parent, "view.child.border-box");
        let content_box = doc.el(parent, "view.child.content-box");
        let stretched_border = doc.el(parent, "view.stretchy.border-box");
        let stretched_content = doc.el(parent, "view.stretchy.content-box");
        doc.flush();

        for child in [border_box, content_box] {
            assert_eq!(
                doc.dom
                    .computed_style_text(child, "width", false)
                    .as_deref(),
                Some("50%"),
            );
        }
        assert_eq!(
            doc.dom
                .computed_style_text(border_box, "width", true)
                .as_deref(),
            Some("100px"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(content_box, "width", true)
                .as_deref(),
            Some("100px"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(stretched_border, "width", true)
                .as_deref(),
            Some("200px"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(stretched_content, "width", true)
                .as_deref(),
            Some("180px"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(border_box, "padding-left", true)
                .as_deref(),
            Some("10px"),
        );
    }

    #[test]
    fn an_auto_margin_resolves_to_the_used_length() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 400px; height: 400px }
             .parent { display: flex; width: 200px; height: 100px }
             .child { width: 100px; height: 20px; margin: auto }",
        );
        let parent = doc.el(doc.root, "view.parent");
        let child = doc.el(parent, "view.child");
        doc.flush();

        assert_eq!(
            doc.dom
                .computed_style_text(child, "margin-left", false)
                .as_deref(),
            Some("auto"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(child, "margin-left", true)
                .as_deref(),
            Some("50px"),
        );
        assert_eq!(
            doc.dom
                .computed_style_text(child, "margin-top", true)
                .as_deref(),
            Some("40px"),
        );
    }

    #[test]
    fn a_boxless_element_resolves_to_its_computed_value() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 400px; height: 400px }
             .gone { display: none; width: 50px }",
        );
        let gone = doc.el(doc.root, "view.gone");
        doc.flush();

        assert_eq!(
            doc.dom.computed_style_text(gone, "width", true).as_deref(),
            Some("50px"),
        );
        assert_eq!(
            doc.dom.computed_style_text(gone, "height", true).as_deref(),
            Some("auto"),
        );
    }

    #[test]
    fn a_logical_longhand_reports_the_physical_resolved_value() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 400px; height: 400px }
             .child { width: 100px; height: 20px; margin-left: 7px }",
        );
        let child = doc.el(doc.root, "view.child");
        doc.flush();

        let physical = doc.dom.computed_style_text(child, "margin-left", true);
        assert_eq!(physical.as_deref(), Some("7px"));
        assert_eq!(
            doc.dom
                .computed_style_text(child, "margin-inline-start", true),
            physical,
        );
    }

    #[test]
    fn a_filter_reports_exactly_the_named_properties() {
        let mut doc = Doc::with_css(
            "page { display: flex; width: 400px; height: 400px }
             .child { color: red; width: 100px; height: 20px }",
        );
        let child = doc.el(doc.root, "view.child");
        doc.flush();

        // The filter's own order, one entry per name that resolves: a
        // shorthand and an unknown name are absent, and a non-custom name
        // matches the way stylo's parser does, ASCII-case-insensitively.
        let entries = doc.dom.computed_style_entries(
            child,
            &["width", "margin", "bogus", "COLOR", "color"],
            true,
        );
        assert_eq!(
            entries,
            vec![
                ("width".to_owned(), "100px".to_owned()),
                ("COLOR".to_owned(), "rgb(255, 0, 0)".to_owned()),
                ("color".to_owned(), "rgb(255, 0, 0)".to_owned()),
            ],
        );
    }
}
