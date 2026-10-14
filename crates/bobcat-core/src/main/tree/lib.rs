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
//! `svg`, `blur_view`, `swiper`, `refresh_view`, `viewpager`,
//! `scroll_container`, `list`, `scroll_coordinator`, `dialog` and `overlay`
//! need components: for image resources (an `svg`'s `content` as a document
//! handed to the host as its bytes), blur hints,
//! the swiper's UA shadow tree, item count and `scrollTo` UI method, the
//! refresh view's UA shadow tree, its header and footer slot assignment and
//! its `finishRefresh`, `finishLoadMore` and `autoStartRefresh` UI methods
//! ([`refresh_view`]), the viewpager's `selectTab` UI method, the
//! `scroll-view`'s `scrollTo`, `scrollBy` and `getScrollInfo` UI methods, the `list`'s
//! `scrollToPosition`, `scrollBy`, `getScrollContainerInfo` and
//! `getVisibleCells` UI methods ([`list`]), the scroll coordinator's
//! `setFoldExpanded` UI method ([`scroll_coordinator`]), the dialog's
//! `:open`/`:modal` state, top-layer membership and four UI methods
//! ([`dialog`]), and the overlay's UA shadow tree, the top-layer membership of
//! the `dialog` in it, and its `showoverlay` and `dismissoverlay` events
//! ([`overlay`]). A component's UI methods are its
//! [`dom::CustomElement::invoke`], which the runtime's `callElementMethod`
//! reaches through [`dom::Document::invoke_element_method`]. The methods
//! every element has whatever its kind, which `callElementMethod` runs when
//! the kind has no method of the name, are [`base_methods`]'
//! (`boundingClientRect`, `scrollIntoView`, `scrollTo` and `scrollBy`).
//! `scroll_coordinator`'s layout needs no component: its ten tags are UA
//! rules over anchor-sized absolute boxes, a sticky toolbar and
//! `scroll-capture-y`, and the component on its two coordinator tags exists
//! for `setFoldExpanded` alone. `overlay` has no UI method.
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
pub(crate) mod refresh_view;
mod scroll_container;
mod scroll_coordinator;
mod svg;
pub(crate) mod swiper;
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
pub(crate) use self::svg::is_svg;
pub use self::ua_sheet::PageConfig;
pub(crate) use crate::view::Viewport;

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// DOM's `slotchange`, which a component with a UA shadow tree observes
/// ([`dom::CustomElement::observed_events`]) to learn that its assigned
/// children changed.
pub(super) const SLOT_CHANGE: &str = "slotchange";

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
///
/// `events` is the queue the `image` and `svg` components leave a source that
/// settled at its bind in, the `dialog` component the `close` and `cancel` its
/// methods queue, and the `overlay` component its `showoverlay` and
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
    svg::define(&mut document, events.clone());
    swiper::define(&mut document);
    refresh_view::define(&mut document);
    viewpager::define(&mut document);
    scroll_container::define(&mut document);
    list::define(&mut document);
    scroll_coordinator::define(&mut document);
    text::define(&mut document);
    dialog::define(&mut document, events.clone());
    // After `dialog`: every overlay builds one in its shadow tree.
    overlay::define(&mut document, events);
    document.add_stylesheet(
        &ua_sheet::ua_stylesheet(config),
        StylesheetOrigin::UserAgent,
    );
    document
}

/// A UI method's params do not have the shape it reads, or name nothing it
/// can act on: the UI-method status table's code 4, `PARAM_INVALID`. Nothing
/// moved. Shared by every component method and the base set.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct InvalidParams;

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

/// `parent`'s element child number `index`, counting element children only
/// (a text node between two elements is skipped), or `None` when `index` is
/// negative or names no child. `index` is a whole number: the caller has
/// truncated it, as `scroll-view`'s `scrollTo({index})` and the pager's
/// `selectTab({index})` do.
pub(super) fn element_child(document: &LynxDocument, parent: NodeId, index: f64) -> Option<NodeId> {
    if index < 0.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative integer; one past any child count saturates and names none"
    )]
    let index = index as usize;
    element_children(document, parent).nth(index)
}

/// `parent`'s element children in tree order, a text node between two
/// elements skipped; none when `parent` is not in the document. What
/// [`element_child`] counts, and what `list`'s `scrollToPosition` searches
/// for an `item-key` and its `getVisibleCells` filters for cells.
pub(super) fn element_children(
    document: &LynxDocument,
    parent: NodeId,
) -> impl Iterator<Item = NodeId> + '_ {
    document
        .get(parent)
        .into_iter()
        .flat_map(|node| node.child_ids().iter().copied())
        .filter(|child| document.get(*child).is_some_and(dom::Node::is_element))
}

/// One event a component owes script, at the element the component belongs
/// to: non-bubbling, except a [`ComponentEvent::Detailed`] that says
/// otherwise.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ComponentEvent {
    /// An `<image>`'s or `<svg>`'s own source settled: `load`, which carries
    /// an `<image>`'s intrinsic size and an `<svg>`'s layout size, or `error`,
    /// which an `<svg>` does not fire.
    Image(ImageOutcome),
    /// An event whose detail is `{}`, named by the component that queued it:
    /// a `<dialog>`'s `close` and `cancel` ([`dialog`]), an `<overlay>`'s
    /// `showoverlay` and `dismissoverlay` ([`overlay`]).
    Plain { node: NodeId, name: &'static str },
    /// An event whose detail is made of strings the component read when it
    /// queued the event, named by that component. [`ComponentDetail`] is the
    /// detail's shape.
    ///
    /// `bubbles` is the event's own, and a bubbling one is composed too: the
    /// two flags web-core's components set together on the events of this
    /// kind they fire (`XMarkdownAttributes.ts`: `bindlink`, `bindimageTap`
    /// and `bindparseEnd` are each `bubbles: true, composed: true`). Bubbling,
    /// it reaches every `bindEvent` on the path and the `global-bindEvent`
    /// pass; composed, it leaves a UA shadow tree its host sits in.
    Detailed {
        node: NodeId,
        name: &'static str,
        bubbles: bool,
        detail: ComponentDetail,
    },
}

/// The `detail` of a [`ComponentEvent::Detailed`]: one variant per *shape*
/// the realm builds, never per event name, so two events whose details have
/// the same keys share one.
///
/// Owned, because the event outlives the call that queued it: the component
/// reads its attributes or its content at that moment, and the runtime
/// delivers from an entry of its own. Each string crosses to the realm as one
/// primitive argument, and the realm builds the object (`element-papi.ts`,
/// `detailOf`); no JSON is written or parsed. A `None` is a key the realm
/// leaves out of the object, neither `undefined`-valued nor `""`: web-core's
/// `{id: undefined}` loses that key when it is serialized, and the transport
/// here would carry an `undefined`-valued key as one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "queued by `<x-markdown>`, which is not built yet")
)]
pub(crate) enum ComponentDetail {
    /// `{id}`: a content id, such as the `content-id` attribute an
    /// `<x-markdown>`'s `parseEnd` reports.
    Id { id: Option<String> },
    /// `{url, content, contentId}`: a link and its text, such as the `<a>` an
    /// `<x-markdown>`'s `link` reports.
    UrlContent {
        url: String,
        content: String,
        content_id: Option<String>,
    },
    /// `{url, contentId}`: a resource, such as the `<img>` an
    /// `<x-markdown>`'s `imageTap` reports.
    Url {
        url: String,
        content_id: Option<String>,
    },
}

/// The events this document's components have produced and not delivered
/// yet, in the order they formed.
///
/// A handle rather than a field, because the producers are on both sides of
/// the document: the `image` component, which is inside it and reaches nothing
/// else; the runtime's own image report path, which is outside it; the
/// `dialog` component's UI methods; and the `overlay` component, inside the
/// document like `image`, as is any component that queues a
/// [`ComponentEvent::Detailed`].
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

    /// Queues an event at `node` whose detail is `detail`'s strings, bubbling
    /// and composed when `bubbles` is set, neither otherwise.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "queued by `<x-markdown>`, which is not built yet")
    )]
    pub(crate) fn queue_detailed(
        &self,
        node: NodeId,
        name: &'static str,
        bubbles: bool,
        detail: ComponentDetail,
    ) {
        self.0.borrow_mut().push(ComponentEvent::Detailed {
            node,
            name,
            bubbles,
            detail,
        });
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
