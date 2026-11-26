//! [`XmlMarkup`] into DOM nodes.
//!
//! `Document::append_xml` builds the subtree under a parent in document
//! order: each element through the creation path of its recorded namespace,
//! each attribute through `set_attribute`, each text through
//! `create_text_node`, so every node behaves as one any other caller made.
//! It applies nothing an element type defines at build time; the caller
//! does that from the `built` callback (the SVG layer's `style` sheets and
//! `use` instances, `Document::append_svg_markup`).

use super::{XmlItem, XmlMarkup, XmlNamespace};
use crate::tree::document::{Document, NodeId};

impl<T> Document<T> {
    pub(crate) fn append_xml(
        &mut self,
        parent: NodeId,
        markup: &XmlMarkup,
        mut payload: impl FnMut() -> T,
        mut built: impl FnMut(usize, NodeId, u32),
    ) -> NodeId {
        let mut open: Vec<(usize, NodeId)> = Vec::new();
        let mut root = None;
        for index in 0..markup.len() {
            while open.last().is_some_and(|&(end, _)| index >= end) {
                open.pop();
            }
            let into = open.last().map_or(parent, |&(_, node)| node);
            match markup.item(index) {
                XmlItem::Text(text) => {
                    let node = self.create_text_node(text, payload());
                    self.append_child(into, node);
                }
                XmlItem::Element {
                    namespace,
                    local_name,
                    attributes,
                } => {
                    let element = match namespace {
                        XmlNamespace::Svg => {
                            self.create_svg_namespace_element(local_name, payload())
                        }
                    };
                    for &attribute in attributes {
                        let (name, value) = markup.attribute(attribute);
                        self.set_attribute(element, name, value);
                    }
                    self.append_child(into, element);
                    let depth = u32::try_from(open.len() + 1).expect("bounded by MAX_NESTING");
                    built(index, element, depth);
                    open.push((markup.subtree(index).end, element));
                    root.get_or_insert(element);
                }
            }
        }
        root.expect("a parsed markup holds its root element")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use crate::test_common::Doc;
    use crate::tree::xml::{XmlMarkup, XmlNamespace};

    #[test]
    fn builds_nodes_in_document_order() {
        let mut doc = Doc::new();
        let markup = XmlMarkup::parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"> <g id="a"><style>.x{}</style><use xlink:href="#a"/></g></svg>"##,
            Some(XmlNamespace::Svg),
        )
        .expect("parses");
        let mut made = 0;
        let mut reported = Vec::new();
        let svg = doc.dom.append_xml(
            doc.root,
            &markup,
            || made += 1,
            |index, node, depth| reported.push((index, node, depth)),
        );
        assert_eq!(made, 6, "four elements and two texts");
        let depths: Vec<(usize, u32)> = reported
            .iter()
            .map(|&(index, _, depth)| (index, depth))
            .collect();
        assert_eq!(depths, [(0, 1), (2, 2), (3, 3), (5, 3)]);
        assert_eq!(reported[0].1, svg);
        let node = |id| doc.dom.get(id).expect("live");
        let children = node(svg).child_ids().to_vec();
        assert!(node(children[0]).is_text_node());
        assert!(node(children[1]).is_svg_element());
        let use_element = reported[3].1;
        assert_eq!(
            node(use_element).attribute("xlink:href").as_deref(),
            Some("#a")
        );
        assert!(
            doc.dom.shadow_root(use_element).is_none(),
            "no `use` instance without the SVG builder"
        );
    }
}
