//! [`XmlMarkup`] into an SVG subtree.
//!
//! `Document::append_svg_markup` builds the markup under a parent with the
//! tree's XML builder (`Document::append_xml`): every element through the
//! SVG-namespace creation path and every attribute through `set_attribute`,
//! so presentation attributes become hints and `id`, `class` and `style`
//! behave as for any other caller. In the same walk it collects the ids, the
//! `use` elements and the `style` sheets. Then, in this order:
//!
//! 1. each `style` element whose `type` is absent, empty or `text/css` (ASCII case-insensitive)
//!    adds its text as an author stylesheet of the document, in document order. The builder is
//!    meant for a document whose content is this one SVG, so its sheets are the document's; a
//!    parent inside a shadow tree is refused, because the sheets would still style the whole
//!    document;
//! 2. each `use` element is instantiated ([`super::use_element`]).
//!
//! Neither is kept in step with later mutations: changing a `style`
//! element's text or a `use` element's `href` afterwards changes nothing.
//! No caller mutates this tree: its production caller is the isolated
//! document of an SVG image (`super::image`).

use rustc_hash::FxHashMap;
use stylo::stylesheets::Origin;

use super::USE_TAG;
use crate::tree::document::{Document, NodeId};
use crate::tree::xml::{XmlItem, XmlMarkup, XmlNamespace};

impl<T> Document<T> {
    /// Builds `markup` under `parent` and answers its root element.
    /// `payload` is called once per node made, in document order, and once
    /// per node of every `use` instance after that.
    ///
    /// # Panics
    ///
    /// When `parent` is inside a shadow tree: the markup's `style` sheets
    /// are document sheets.
    pub(crate) fn append_svg_markup(
        &mut self,
        parent: NodeId,
        markup: &XmlMarkup,
        mut payload: impl FnMut() -> T,
    ) -> NodeId {
        assert!(
            self.shadow_root_of(parent).is_none(),
            "Document::append_svg_markup: the markup's style elements are document sheets, so \
             it cannot be built inside a shadow tree"
        );
        // The first element with an id wins, as `getElementById` says.
        let mut ids: FxHashMap<&str, NodeId> = FxHashMap::default();
        let mut uses: Vec<(NodeId, u32)> = Vec::new();
        let mut sheets: Vec<String> = Vec::new();
        let root = self.append_xml(parent, markup, &mut payload, |index, element, depth| {
            let XmlItem::Element {
                namespace: XmlNamespace::Svg,
                local_name,
                attributes,
            } = markup.item(index)
            else {
                return;
            };
            if let Some((_, id)) = attributes
                .iter()
                .map(|&attribute| markup.attribute(attribute))
                .find(|(name, _)| *name == "id")
            {
                ids.entry(id).or_insert(element);
            }
            match local_name {
                USE_TAG => uses.push((element, depth)),
                "style" => {
                    if let Some(css) = style_sheet(markup, index) {
                        sheets.push(css);
                    }
                }
                _ => {}
            }
        });
        for css in &sheets {
            self.add_stylesheet(css, Origin::Author);
        }
        self.instantiate_uses(&uses, &ids, &mut payload);
        root
    }
}

/// The CSS a `style` element at `index` contributes: its text, when its
/// `type` is absent, empty or `text/css` (SVG 2 §6.3, HTML's `style`
/// processing model).
fn style_sheet(markup: &XmlMarkup, index: usize) -> Option<String> {
    let XmlItem::Element { attributes, .. } = markup.item(index) else {
        return None;
    };
    let css = attributes
        .iter()
        .map(|&attribute| markup.attribute(attribute))
        .find(|(name, _)| *name == "type")
        .is_none_or(|(_, value)| value.is_empty() || value.eq_ignore_ascii_case("text/css"));
    css.then(|| {
        markup
            .children(index)
            .filter_map(|child| match markup.item(child) {
                XmlItem::Text(text) => Some(text),
                XmlItem::Element { .. } => None,
            })
            .collect()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use stylo_atoms::Atom;

    use crate::test_common::Doc;
    use crate::tree::xml::{XmlMarkup, XmlNamespace};

    #[test]
    fn keyframes_in_a_style_element_are_rules_of_the_document() {
        let mut doc = Doc::new();
        let markup = XmlMarkup::parse(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><style>
                @keyframes spin { from { opacity: 0 } to { opacity: 1 } }
            </style><path/></svg>"#,
            Some(XmlNamespace::Svg),
        )
        .expect("parses");
        let svg = doc.dom.append_svg_markup(doc.root, &markup, || ());
        doc.flush();
        let element = doc.dom.get(svg).expect("live");
        assert!(
            doc.dom
                .style_engine()
                .stylist()
                .lookup_keyframes(&Atom::from("spin"), element)
                .is_some()
        );
    }
}
