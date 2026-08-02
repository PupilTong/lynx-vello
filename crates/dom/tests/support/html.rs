//! Test-only HTML fragment → `dom` adapter.
//!
//! Handles the subset the screenshot fixtures are written in: nested
//! inline-styled elements plus text. Element names carry no meaning here —
//! there is no UA stylesheet, so `<div>` and `<span>` differ only in how the
//! fixture reads.

use dom::{Document, NodeId};
use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, QualName, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::paint_common::{Doc, device};

/// Parses one root element and its nested inline-styled children.
#[must_use]
pub fn parse(fragment: &str, width: f32, height: f32) -> Doc {
    let context = QualName::new(None, ns!(html), local_name!("div"));
    let parsed = parse_fragment(
        RcDom::default(),
        ParseOpts::default(),
        context,
        Vec::new(),
        false,
    )
    .one(fragment);
    assert!(
        parsed.errors.borrow().is_empty(),
        "screenshot fragment must be valid HTML: {:?}",
        parsed.errors.borrow()
    );

    let parser_root = parsed
        .document
        .children
        .borrow()
        .first()
        .cloned()
        .expect("fragment parser must create its synthetic root");
    let roots: Vec<_> = parser_root
        .children
        .borrow()
        .iter()
        .filter(|node| matches!(&node.data, NodeData::Element { .. }))
        .cloned()
        .collect();
    assert_eq!(roots.len(), 1, "fragment must contain one root element");

    let mut dom = Document::new(device(width, height));
    let root = import_element(&mut dom, &roots[0]);
    dom.append_document_element(root);
    Doc { dom, root }
}

fn import_element(dom: &mut Document<()>, source: &Handle) -> NodeId {
    let NodeData::Element { name, attrs, .. } = &source.data else {
        panic!("fragment importer expected an element");
    };
    let id = dom.create_element(name.local.as_ref(), ());
    if let Some(attr) = attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == "style")
    {
        dom.set_inline_style(id, attr.value.as_ref());
    }
    for child in source.children.borrow().iter() {
        match &child.data {
            NodeData::Element { .. } => {
                let child_id = import_element(dom, child);
                dom.append_child(id, child_id);
            }
            NodeData::Text { contents } => {
                let text = contents.borrow();
                // Inter-element indentation is a whitespace-only text run, and
                // css-flexbox-1 §4 does not render one as a flex item. Keeping
                // them would turn every newline in a fixture into a stray text
                // leaf, so the fixtures could not be indented at all.
                if text.trim().is_empty() {
                    continue;
                }
                let child_id = dom.create_text_node(text.as_ref(), ());
                dom.append_child(id, child_id);
            }
            _ => {}
        }
    }
    id
}
