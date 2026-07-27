//! Test-only `<div>` fragment → `w3c-dom` adapter.

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, QualName, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use w3c_dom::{Document, NodeId};

use crate::common::{Doc, device};

/// Parses one root `<div>` and its nested inline-styled `<div>` children.
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
    assert_eq!(roots.len(), 1, "fragment must contain one root <div>");

    let mut dom = Document::new(device(width, height));
    let root = import_div(&mut dom, &roots[0]);
    dom.append_document_element(root);
    Doc { dom, root }
}

fn import_div(dom: &mut Document<()>, source: &Handle) -> NodeId {
    let NodeData::Element { name, attrs, .. } = &source.data else {
        panic!("fragment importer expected a <div>");
    };
    assert_eq!(name.local.as_ref(), "div", "only <div> is supported");
    let id = dom.create_element("div", ());
    if let Some(attr) = attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == "style")
    {
        dom.set_inline_style(id, attr.value.as_ref());
    }
    for child in source.children.borrow().iter() {
        if matches!(&child.data, NodeData::Element { .. }) {
            let child_id = import_div(dom, child);
            dom.append_child(id, child_id);
        }
    }
    id
}
