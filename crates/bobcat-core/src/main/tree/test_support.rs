//! Shared support for the main-thread tree policy tests:
//! a document, a way to hang an element in it, and the computed style that
//! comes back out.

use dom::NodeId;
use dom::stylo::properties::ComputedValues;
use dom::stylo::servo_arc::Arc;
use dom::stylo::values::computed::{Display, Overflow};

use super::{LynxDocument, PageConfig, Viewport, new_document};

/// A document on a phone-shaped viewport with the default page config.
pub(super) fn document() -> LynxDocument {
    with_config(PageConfig::default())
}

pub(super) fn with_config(config: PageConfig) -> LynxDocument {
    new_document(Viewport::new(393.0, 727.0), config)
}

/// Attaches `tag` under the page, with `style` as its inline style.
pub(super) fn child(document: &mut LynxDocument, tag: &str, style: &str) -> NodeId {
    let page = document.document_element().id();
    element_under(document, page, tag, style)
}

pub(super) fn element_under(
    document: &mut LynxDocument,
    parent: NodeId,
    tag: &str,
    style: &str,
) -> NodeId {
    let element = document.create_element(tag, ());
    if !style.is_empty() {
        document.set_inline_style(element, style);
    }
    document.append_child(parent, element);
    element
}

pub(super) fn style_of(document: &LynxDocument, element: NodeId) -> Arc<ComputedValues> {
    document
        .get(element)
        .expect("a live element")
        .computed_style()
        .expect("a flushed element has computed style")
}

pub(super) fn display(document: &LynxDocument, element: NodeId) -> Display {
    style_of(document, element).clone_display()
}

pub(super) fn overflow(document: &LynxDocument, element: NodeId) -> (Overflow, Overflow) {
    let style = style_of(document, element);
    (style.clone_overflow_x(), style.clone_overflow_y())
}
