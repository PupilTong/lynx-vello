//! Shared support for the main-thread tree policy tests:
//! a document, a way to hang an element in it, and the computed style that
//! comes back out.

use dom::NodeId;
use dom::event::EventDetail;
use dom::stylo::properties::ComputedValues;
use dom::stylo::servo_arc::Arc;
use dom::stylo::values::computed::{Display, Overflow};

use super::{
    Autoplays, ComponentDetail, ComponentEvent, ComponentEvents, Field, LynxDocument, PageConfig,
    Viewport, new_document,
};

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
        new_document(
            Viewport::new(393.0, 727.0),
            config,
            events.clone(),
            Autoplays::default(),
        ),
        events,
    )
}

/// The default document, plus the autoplay timers its swipers keep — what
/// the runtime holds the other end of.
pub(in crate::main) fn with_autoplays() -> (LynxDocument, Autoplays) {
    let autoplays = Autoplays::default();
    (
        new_document(
            Viewport::new(393.0, 727.0),
            PageConfig::default(),
            ComponentEvents::default(),
            autoplays.clone(),
        ),
        autoplays,
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

/// What a component queued on `events` since the last drain, each a
/// [`ComponentDetail::Fields`] event at `node`, as `(name, pairs)`.
pub(in crate::main) fn fields_heard(
    events: &ComponentEvents,
    node: NodeId,
) -> Vec<(&'static str, Vec<(&'static str, Field)>)> {
    events
        .take()
        .into_iter()
        .map(|event| match event {
            ComponentEvent::Detailed {
                node: at,
                name,
                detail: ComponentDetail::Fields(fields),
            } => {
                assert_eq!(at, node, "{name} is queued at the component's element");
                (name, fields.into_vec())
            }
            other => panic!("only events with a fields detail are queued: {other:?}"),
        })
        .collect()
}

/// Dispatches the touch event `name` at `target` as the runtime dispatches
/// a routed one: bubbling and composed.
pub(in crate::main) fn touched(document: &mut LynxDocument, target: NodeId, name: &'static str) {
    document.dispatch_element_event(target, name, &EventDetail::None, true, true, 0.0);
}
