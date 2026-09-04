//! Test-only HTML fragment → `dom` adapter.
//!
//! Handles the subset the screenshot fixtures are written in: nested
//! inline-styled elements plus text. Element names carry no meaning here —
//! `<div>` and `<span>` differ only in how the fixture reads.
//!
//! One class does carry meaning: `text-block`, which [`TEXT_BLOCK_SHEET`]
//! turns into `display: -lynx-text`. A fixture whose element directly
//! contains text wears it, because in this engine only a `-lynx-text` box
//! lays text out — `crates/bobcat-core` gives `<text>` that value through the
//! Lynx UA sheet, and these fixtures have no UA sheet of their own.
//!
//! The class is deliberately inert in a browser: the reference captures see
//! an unknown class and an unchanged `display`, so a fixture stays a valid
//! Chromium fragment and its golden keeps meaning what it meant. Putting
//! `display: -lynx-text` in the fragment itself would not — Chromium drops it
//! as invalid and falls back to `display: inline`, which is a different box.

use dom::{Document, NodeId};
use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, QualName, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::paint_common::{Doc, device};

/// The one rule this harness stands in for the Lynx UA sheet with.
///
/// `!important` for the same reason the real sheet uses it: a fixture's own
/// `display` is inline style, and text layout is not a preference an element
/// gets to express.
const TEXT_BLOCK_SHEET: &str = ".text-block { display: -lynx-text !important; }";

#[must_use]
pub(super) fn parse(fragment: &str, width: f32, height: f32) -> Doc {
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

    let NodeData::Element { name, .. } = &roots[0].data else {
        panic!("fragment importer expected an element root");
    };
    let mut dom = Document::new(device(width, height), name.local.as_ref(), ());
    dom.add_stylesheet(TEXT_BLOCK_SHEET, dom::StylesheetOrigin::UserAgent);
    let root = dom.document_element().id();
    import_onto(&mut dom, root, &roots[0]);
    Doc { dom, root }
}

fn import_element(dom: &mut Document<()>, source: &Handle) -> NodeId {
    let NodeData::Element { name, .. } = &source.data else {
        panic!("fragment importer expected an element");
    };
    let id = dom.create_element(name.local.as_ref(), ());
    import_onto(dom, id, source);
    id
}

fn import_onto(dom: &mut Document<()>, id: NodeId, source: &Handle) {
    let NodeData::Element { attrs, .. } = &source.data else {
        panic!("fragment importer expected an element");
    };
    if let Some(attr) = attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == "style")
    {
        dom.set_inline_style(id, attr.value.as_ref());
    }
    if let Some(attr) = attrs
        .borrow()
        .iter()
        .find(|attr| attr.name.local.as_ref() == "class")
    {
        dom.set_classes(id, attr.value.as_ref());
    }
    for child in source.children.borrow().iter() {
        match &child.data {
            NodeData::Element { .. } => {
                let child_id = import_element(dom, child);
                dom.append_child(id, child_id);
            }
            NodeData::Text { contents } => {
                let text = contents.borrow();
                if text.trim().is_empty() {
                    continue;
                }
                let child_id = dom.create_text_node(text.as_ref(), ());
                dom.append_child(id, child_id);
            }
            _ => {}
        }
    }
}
