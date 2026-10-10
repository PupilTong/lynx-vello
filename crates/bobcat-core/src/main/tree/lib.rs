//! Main-thread Lynx page policy: the `page` root tag, the UA
//! cascade defaults, the components the engine defines, and view metrics.
//! Everything else the runtime does goes
//! straight to [`dom::Document`] — element identity is the DOM [`NodeId`],
//! which is also the element's Lynx `unique_id`: one number, issued by the
//! DOM, never reissued after the element is freed. Script therefore cannot
//! name a stranger by holding an id too long, only something that no longer
//! exists. The private host boundary still validates script-provided IDs and
//! mutation preconditions before entering `dom`, returning misuse as a
//! JavaScript error.
//!
//! Each tag owns its UA rules and tests. Numeric text and list attributes
//! flow through `attr()`; boolean flags use attribute selectors. Only `image`,
//! `blur_view`, `swiper`, `refresh_view`, `viewpager`, `scroll_container`,
//! `dialog` and `overlay` need components: for image resources, blur hints,
//! the swiper's UA shadow tree and item count, the refresh view's UA shadow
//! tree and its header and footer slot assignment, the viewpager's
//! `selectTab` UI method, the `scroll-view`'s `scrollTo`, `scrollBy` and
//! `getScrollInfo` UI methods, the dialog's
//! `:open`/`:modal` state, top-layer membership and four UI methods
//! ([`dialog`]), and the overlay's UA shadow tree, the top-layer membership of
//! the `dialog` in it, and its `showoverlay` and `dismissoverlay` events
//! ([`overlay`]). A component's UI methods are its
//! [`dom::CustomElement::invoke`], which the runtime's `callElementMethod`
//! reaches through [`dom::Document::invoke_element_method`]. The methods
//! every element has whatever its kind, which `callElementMethod` runs when
//! the kind has no method of the name, are [`base_methods`]' (only
//! `scrollIntoView` today).
//! `scroll_coordinator` needs no component, and has no UI method: its ten tags
//! are UA rules over anchor-sized absolute boxes, a sticky toolbar and
//! `scroll-capture-y`. `swiper`, `refresh_view` and `overlay` have no UI
//! method.
//!
//! [`NodeId`]: dom::NodeId

mod base_methods;
mod blur_view;
pub(crate) mod dialog;
mod image;
mod list;
mod overlay;
mod popover;
pub(crate) mod raw_text;
mod refresh_view;
mod scroll_container;
mod scroll_coordinator;
mod swiper;
/// Reached from `main` as well as from the tags here: the exposure tests
/// build their documents the same way.
#[cfg(test)]
pub(super) mod test_support;
mod text;
mod ua_sheet;
mod viewpager;
#[cfg(test)]
mod web_text_replication;

use std::cell::RefCell;
use std::rc::Rc;

use dom::{Document, ImageOutcome, NodeId, StylesheetOrigin};

pub(crate) use self::base_methods::invoke_base_method;
pub use self::ua_sheet::PageConfig;
pub(crate) use crate::view::Viewport;

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
///
/// `events` is the queue the `image` component leaves a `src` that settled at
/// its bind in, the `dialog` component the `close` and `cancel` its methods
/// queue, and the `overlay` component its `showoverlay` and
/// `dismissoverlay`, for the runtime to dispatch once it is out of the
/// JavaScript call that caused them.
#[must_use]
pub(crate) fn new_document(
    viewport: Viewport,
    config: PageConfig,
    events: ComponentEvents,
) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    blur_view::define(&mut document);
    image::define(&mut document, events.clone());
    swiper::define(&mut document);
    refresh_view::define(&mut document);
    viewpager::define(&mut document);
    scroll_container::define(&mut document);
    dialog::define(&mut document, events.clone());
    // After `dialog`: every overlay builds one in its shadow tree.
    overlay::define(&mut document, events);
    document.add_stylesheet(
        &ua_sheet::ua_stylesheet(config),
        StylesheetOrigin::UserAgent,
    );
    document
}

/// JavaScript's `ToBoolean` over the values JSON can carry: how a component
/// method reads a boolean param, such as `selectTab`'s and `scrollTo`'s
/// `smooth`, from the JSON text the realm serialized.
pub(super) fn is_truthy(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// One event a component owes script: always non-bubbling, at the element
/// the component belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ComponentEvent {
    /// An `<image>`'s own `src` settled: `load`, which carries the bitmap's
    /// intrinsic size, or `error`.
    Image(ImageOutcome),
    /// An event whose detail is `{}`, named by the component that queued it:
    /// a `<dialog>`'s `close` and `cancel` ([`dialog`]), an `<overlay>`'s
    /// `showoverlay` and `dismissoverlay` ([`overlay`]).
    Plain { node: NodeId, name: &'static str },
}

/// The events this document's components have produced and not delivered
/// yet, in the order they formed.
///
/// A handle rather than a field, because the producers are on both sides of
/// the document: the `image` component, which is inside it and reaches nothing
/// else; the runtime's own image report path, which is outside it; the
/// `dialog` component's UI methods; and the `overlay` component, inside the
/// document like `image`.
/// Each holds a clone of this one queue, and the runtime drains it in an entry
/// of its own, posted by the epilogue of the entry that filled it
/// (`docs/runtime-architecture.md` has the entry boundary,
/// [`crate::realm::owner`] the epilogue's order).
///
/// Every producer runs inside a JavaScript call or with the document
/// borrowed, where nothing may dispatch, and queueing is the only thing any of
/// them can do with an event — which is what keeps one from being dropped.
#[derive(Clone, Default)]
pub(crate) struct ComponentEvents(Rc<RefCell<Vec<ComponentEvent>>>);

impl ComponentEvents {
    /// Queues what an image source bind settled, if it settled anything.
    /// `None` is the ordinary case — a source still loading, or a write that
    /// changed nothing.
    pub(crate) fn queue_image(&self, outcome: Option<ImageOutcome>) {
        if let Some(outcome) = outcome {
            self.0.borrow_mut().push(ComponentEvent::Image(outcome));
        }
    }

    /// Queues a whole image report batch's outcomes, in the order `dom`
    /// returned them.
    pub(crate) fn extend_images(&self, outcomes: Vec<ImageOutcome>) {
        self.0
            .borrow_mut()
            .extend(outcomes.into_iter().map(ComponentEvent::Image));
    }

    /// Queues an event with a `{}` detail at `node`.
    pub(crate) fn queue(&self, node: NodeId, name: &'static str) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Plain { node, name });
    }

    /// Takes everything queued since the last drain.
    pub(crate) fn take(&self) -> Vec<ComponentEvent> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    /// Whether anything is waiting for a turn to be delivered on.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::document;

    #[test]
    fn a_layout_pass_sizes_the_page_to_the_viewport() {
        let mut document = document();
        let page = document.document_element().id();
        document.layout();
        let layout = document
            .rounded_layout(page)
            .expect("the page is laid out after the pass");
        assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 727.0).abs() < f32::EPSILON);
    }
}
