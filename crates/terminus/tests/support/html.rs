//! Test-only HTML fragment → `terminus` adapter.
//!
//! Handles the subset the screenshot fixtures are written in: nested
//! inline-styled elements plus text. Element names carry no meaning here, with
//! one exception: `<div>` and `<span>` differ only in how the fixture reads.
//! The fragments carry no stylesheet, the sheets a test adds after parsing
//! select by class or by a Lynx text tag, and `crates/terminus` reads no tag name
//! but `slot`'s.
//!
//! The exception is the Lynx text tags, `text`, `inline-text` and
//! `inline-truncation`. [`parse`] defines them as Lynx text elements
//! ([`terminus::link::CustomElement::is_lynx_text`]), as `crates/bobcat-core` does, so a
//! fixture's `<inline-truncation>` is the element it is in a Lynx page.
//!
//! One class carries meaning too: `text-block`. A fixture whose element
//! directly contains text wears it, because in this engine a Lynx text element
//! is what lays text out as a Lynx paragraph, and these fixtures have no Lynx
//! page around them. The importer creates a `text-block` element as a
//! [`TEXT_TAG`], whatever tag the fixture wrote; its inline style and classes
//! come along unchanged, and its `display` stays the fixture's own, which a
//! Lynx text element lays out under as a paragraph all the same.
//!
//! The class is deliberately inert in a browser: the reference captures see
//! an unknown class on an unchanged element, so a fixture stays a valid
//! Chromium fragment and its golden keeps meaning what it meant. Writing
//! `<text>` in the fragment itself would not: Chromium knows no such HTML
//! element and gives it an unknown element's inline box wherever the fixture's
//! inline style names no `display`.
//!
//! Until 2026-10-17, when the value was removed, the harness made a
//! `text-block` a Lynx paragraph with a user-agent rule instead:
//! `.text-block { display: -lynx-text !important; }`.

use html5ever::tendril::TendrilSink;
use html5ever::{ParseOpts, QualName, local_name, ns, parse_fragment};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use terminus::link::{Document, NodeId};

use crate::paint_common::{Doc, define_lynx_text, device};

/// The class that marks a fixture element holding text.
const TEXT_BLOCK_CLASS: &str = "text-block";

/// The tag a `text-block` element is created under.
const TEXT_TAG: &str = "text";

/// The tags `crates/bobcat-core` defines as Lynx text elements.
const LYNX_TEXT_TAGS: [&str; 3] = [TEXT_TAG, "inline-text", "inline-truncation"];

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

    // A definition upgrades the document element when the root is a
    // `text-block` too, so the root needs no case of its own.
    let mut dom = Document::new(device(width, height), tag_of(&roots[0]), ());
    for tag in LYNX_TEXT_TAGS {
        define_lynx_text(&mut dom, tag);
    }
    let root = dom.document_element().id();
    import_onto(&mut dom, root, &roots[0]);
    Doc { dom, root }
}

/// The tag `source` is created under: [`TEXT_TAG`] for a `text-block`
/// element, the fixture's own name otherwise.
fn tag_of(source: &Handle) -> &str {
    let NodeData::Element { name, attrs, .. } = &source.data else {
        panic!("fragment importer expected an element");
    };
    let text_block = attrs.borrow().iter().any(|attr| {
        attr.name.local.as_ref() == "class"
            && attr
                .value
                .split_whitespace()
                .any(|class| class == TEXT_BLOCK_CLASS)
    });
    if text_block {
        TEXT_TAG
    } else {
        name.local.as_ref()
    }
}

fn import_element(dom: &mut Document<()>, source: &Handle) -> NodeId {
    let id = dom.create_element(tag_of(source), ());
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
