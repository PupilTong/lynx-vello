//! Shared support for the main-thread tree policy tests:
//! a document, a way to hang an element in it, and the computed style that
//! comes back out.

use dom::NodeId;
use dom::stylo::properties::ComputedValues;
use dom::stylo::servo_arc::Arc;
use dom::stylo::values::computed::{Display, Overflow};

use super::{ComponentEvents, LynxDocument, PageConfig, Viewport, new_document};

/// A document on a phone-shaped viewport with the default page config.
pub(in crate::main) fn document() -> LynxDocument {
    with_config(PageConfig::default())
}

pub(in crate::main) fn with_config(config: PageConfig) -> LynxDocument {
    with_component_events(config).0
}

/// The same document, plus the queue its components leave the events they
/// owe script in — what the runtime holds the other end of.
pub(in crate::main) fn with_component_events(
    config: PageConfig,
) -> (LynxDocument, ComponentEvents) {
    let events = ComponentEvents::default();
    (
        new_document(Viewport::new(393.0, 727.0), config, events.clone()),
        events,
    )
}

/// Attaches `tag` under the page, with `style` as its inline style.
pub(in crate::main) fn child(document: &mut LynxDocument, tag: &str, style: &str) -> NodeId {
    let page = document.document_element().id();
    element_under(document, page, tag, style)
}

pub(in crate::main) fn element_under(
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

pub(in crate::main) fn style_of(document: &LynxDocument, element: NodeId) -> Arc<ComputedValues> {
    document
        .get(element)
        .expect("a live element")
        .computed_style()
        .expect("a flushed element has computed style")
}

pub(in crate::main) fn display(document: &LynxDocument, element: NodeId) -> Display {
    *style_of(document, element).get_display()
}

pub(in crate::main) fn overflow(document: &LynxDocument, element: NodeId) -> (Overflow, Overflow) {
    let style = style_of(document, element);
    (*style.get_overflow_x(), *style.get_overflow_y())
}
