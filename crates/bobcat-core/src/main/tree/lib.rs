//! Main-thread Lynx page policy: the `page` root tag, the UA
//! cascade defaults, the components the engine defines, and view metrics.
//! Everything else the runtime does goes
//! straight to [`terminus::link::Document`] — element identity is the DOM [`NodeId`],
//! which is also the element's Lynx `unique_id`: one number, issued by the
//! DOM, never reissued after the element is freed. Script therefore cannot
//! name a stranger by holding an id too long, only something that no longer
//! exists. The private host boundary still validates script-provided IDs and
//! mutation preconditions before entering `terminus`, returning misuse as a
//! JavaScript error.
//!
//! Each tag owns its UA rules and tests. Numeric text and list attributes
//! flow through `attr()`; boolean flags use attribute selectors. Only `image`,
//! `svg`, `blur_view`, `swiper`, `refresh_view`, `viewpager`,
//! `scroll_container`, `list`, `scroll_coordinator`, `overlay`,
//! `markdown`, `frame` and `text_field` need components: for image resources (an `svg`'s `content`
//! as a document handed to the host as its bytes), blur hints,
//! the swiper's UA shadow tree, item count, `scrollTo` UI method and events, the
//! refresh view's UA shadow tree, its header and footer slot assignment and
//! its `finishRefresh`, `finishLoadMore` and `autoStartRefresh` UI methods
//! and its events ([`refresh_view`]), the viewpager's `selectTab` UI method and events, the
//! `scroll-view`'s `scrollTo`, `scrollBy` and `getScrollInfo` UI methods, the `list`'s
//! `scrollToPosition`, `scrollBy`, `getScrollContainerInfo` and
//! `getVisibleCells` UI methods ([`list`]), both scrollers'
//! `scrolltoupper` and `scrolltolower` events, decided from the `scroll`
//! they observe ([`scroll_container`]), the scroll coordinator's
//! `setFoldExpanded` UI method ([`scroll_coordinator`]), the overlay's UA
//! shadow tree, the top-layer membership of
//! the `dialog` in it, and its `showoverlay` and `dismissoverlay` events
//! ([`overlay`]), the markdown element's UA shadow tree rendered from its
//! `content`, its `parseEnd`, `link` and `imageTap` events and its
//! `getContent`, `getImages`, `getParseResult`, `getTextBoundingRect` and
//! `getCharIndexByPoint` UI methods ([`markdown`]), and the frame's open
//! shadow root with the nested page in it, and the
//! `connected`/`disconnected` signals its card's lifecycle follows
//! ([`frame`]), and the text fields' UA shadow tree around one editing
//! host, their attributes, their `input`, `confirm`, `selection`, `focus`
//! and `blur` events, queued like the markdown element's as
//! [`ComponentEvent::Detailed`], and their UI methods ([`text_field`]). A
//! component's UI methods are its
//! [`terminus::link::CustomElement::invoke`], which the runtime's `callElementMethod`
//! reaches through [`terminus::link::Document::invoke_element_method`]. The methods
//! every element has whatever its kind, which `callElementMethod` runs when
//! the kind has no method of the name, are [`base_methods`]'
//! (`boundingClientRect`, `scrollIntoView`, `scrollTo` and `scrollBy`).
//! `scroll_coordinator`'s layout needs no component: its ten tags are UA
//! rules over anchor-sized absolute boxes, a sticky toolbar and
//! `scroll-capture-y`, and the component on its two coordinator tags exists
//! for `setFoldExpanded` alone. `overlay` has no UI method.
//!
//! # HTML's elements
//!
//! HTML's `<dialog>` and the `popover` attribute's UA rules are
//! `terminus`'s ([`terminus::link::html`]): [`new_document`] defines the `dialog`
//! component with [`terminus::link::Document::define_html_elements`], and the UA
//! sheet writes HTML's rules among the Lynx ones (`ua_sheet`, "HTML's rules
//! under the Lynx tags'"). Both are W3C features, so `AGENTS.md`'s
//! standards policy puts them in the first bucket: web-core runs in a
//! browser, where a card that writes `<dialog>` gets the browser's
//! `HTMLDialogElement` — web-elements' `x-overlay-ng` carries one in its
//! shadow tree, opened with `showModal()` (`htmlTemplates.ts:136-172`), and
//! so does this engine's ([`overlay`]) — and an element with a `popover`
//! attribute is the browser's popover. Native Lynx has neither, and no top
//! layer: its `<overlay>` is a zero-sized box whose content is reparented
//! into a platform window.
//!
//! The dialog's `show`, `showModal`, `close` and `requestClose` are UI
//! methods, reached through `invoke` / `__InvokeUIMethod`:
//! `callElementMethod` in `main/runtime` asks
//! [`terminus::link::Document::invoke_element_method`], which runs the component's
//! [`terminus::link::CustomElement::invoke`]. Its `InvalidStateError`
//! ([`terminus::link::html::dialog::InvalidState`]) is
//! [`terminus::link::MethodError::InvalidState`], which the runtime answers as the
//! UI-method status 7, native's `INVALID_STATE_ERROR`
//! (`lynx/core/renderer/dom/lynx_get_ui_result.h:53-61`). web-core reports
//! every method that throws as 4, `PARAM_INVALID`
//! (`web-core/ts/client/mainthread/elementAPIs/createInvokeUIMethod.ts:12-44`);
//! native's code is a ruled deviation from it (`docs/tracking/deviations.md`).
//! The methods' `params` are not read: `returnValue` has no reader in Lynx
//! JS, so `close(returnValue)` drops it. The dialog's `close` and `cancel`
//! reach the document's [`ComponentEvents`], beside an image's `load` and
//! `error`, through the queued-event sink [`new_document`] registers
//! ([`terminus::link::event::QueuedEventSink`]), and the page delivers them from
//! an entry of its own, non-bubbling, with a `{}` detail. The overlay's
//! shadow dialog is driven through the same functions, as web-core drives
//! its own through `showModal()` and `close()`.
//!
//! [`NodeId`]: terminus::link::NodeId

mod base_methods;
mod blur_view;
#[cfg(test)]
#[path = "dialog_tests.rs"]
mod dialog_tests;
mod frame;
mod image;
mod list;
mod markdown;
mod overlay;
#[cfg(test)]
#[path = "popover_tests.rs"]
mod popover_tests;
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
pub(crate) mod text_field;
mod touch_count;
mod ua_sheet;
mod viewpager;
#[cfg(test)]
mod web_text_replication;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use smallvec::SmallVec;
use terminus::link::event::QueuedEventSink;
use terminus::link::{Document, ImageOutcome, NodeId, StylesheetOrigin};

pub(crate) use self::base_methods::invoke_base_method;
pub(crate) use self::frame::{MAX_FRAME_DEPTH, frame_depth, nested_page, reset_frame};
pub(crate) use self::svg::is_svg;
pub(crate) use self::swiper::Autoplays;
pub(crate) use self::text::{SelectionReports, is_text, report_selection_changes};
pub(crate) use self::text_field::focus_delegate;
pub use self::ua_sheet::PageConfig;
pub(crate) use crate::view::Viewport;

/// The node whose border box a `layoutchange` registration on `element`
/// observes (`crate::main::layout_change`): web-core's `layoutChangeTarget`.
/// For an `overlay` or `x-overlay-ng` host, which generates no box, its
/// shadow `#dialog` (`XOverlayNg.ts:21-23`); for every other element, the
/// element itself.
pub(crate) fn layout_change_target(document: &LynxDocument, element: NodeId) -> NodeId {
    overlay::dialog_of(document, element).unwrap_or(element)
}

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// DOM's `slotchange`, which a component with a UA shadow tree observes
/// ([`terminus::link::CustomElement::observed_events`]) to learn that its assigned
/// children changed.
pub(super) const SLOT_CHANGE: &str = "slotchange";

/// `selectionchange`, the one W3C type name fired at three places, each
/// with its own audience:
///
/// - the Selection API's event at the document node, which the runtime fires from the
///   component-events entry when [`terminus::link::Document::take_selectionchange`] says the
///   document's selection changed, and which a realm hears through the boot module's `document`;
/// - the engine-only event the runtime fires at an editing host after every command it applied
///   (`runtime/editing.rs`), which no realm hears and from which the text field component reads its
///   `selection` ([`text_field`]);
/// - the Lynx event at a `text` or `inline-text`, `{start, end, direction}`, which the `text`
///   module derives from the document's selection after the W3C event fired
///   ([`text::report_selection_changes`]).
pub(crate) const SELECTION_CHANGE: &str = "selectionchange";

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
///
/// `events` is the queue the `image` and `svg` components leave a source that
/// settled at its bind in, HTML's `dialog` component the `close` and `cancel`
/// its methods queue (the document's queued-event sink,
/// [`terminus::link::event::QueuedEventSink`]), the `overlay` component its `showoverlay` and
/// `dismissoverlay`, the `markdown` component its `parseEnd`, `link` and
/// `imageTap`, the `scroll-view` and `list` components their
/// `scrolltoupper` and `scrolltolower`, the `x-swiper` component its
/// `scrollstart`, `change`, `scrollend` and `transition`, the `viewpager`
/// component its `offsetchange`, `change` and `willchange`, the
/// `x-refresh-view` component its `headershow`, `headeroffset`,
/// `footeroffset`, `headerreleased`, `footerreleased`, `startrefresh` and
/// `startloadmore`, and the `frame` component its
/// connections and disconnections, for the runtime to dispatch once it is out of the
/// JavaScript call that caused them. `autoplays` is where the `x-swiper`
/// component keeps its autoplay timers, for the runtime to wait out and run.
#[must_use]
pub(crate) fn new_document(
    viewport: Viewport,
    config: PageConfig,
    events: ComponentEvents,
    autoplays: Autoplays,
) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    document.set_queued_event_sink(Rc::new(events.clone()));
    blur_view::define(&mut document);
    image::define(&mut document, events.clone());
    svg::define(&mut document, events.clone());
    swiper::define(&mut document, autoplays, events.clone());
    refresh_view::define(&mut document, events.clone());
    viewpager::define(&mut document, events.clone());
    scroll_container::define(&mut document, events.clone());
    list::define(&mut document, events.clone());
    scroll_coordinator::define(&mut document);
    text::define(&mut document);
    // After `image`: every image a markdown element renders is one.
    markdown::define(&mut document, events.clone());
    document.define_html_elements();
    text_field::define(&mut document, events.clone());
    // After HTML's elements: every overlay builds a `dialog` in its shadow
    // tree.
    overlay::define(&mut document, events.clone());
    frame::define(&mut document, events);
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
/// for an `item-key` and its `getVisibleCells` filters for cells. Walked from
/// the end too: `list`'s lower item-count zone takes its `count`th child from
/// the end.
pub(super) fn element_children(
    document: &LynxDocument,
    parent: NodeId,
) -> impl DoubleEndedIterator<Item = NodeId> + '_ {
    document
        .get(parent)
        .into_iter()
        .flat_map(|node| node.child_ids().iter().copied())
        .filter(|child| {
            document
                .get(*child)
                .is_some_and(terminus::link::Node::is_element)
        })
}

/// Which of `parent`'s element children holds `node` — `node` itself or an
/// ancestor of it — as its index among them ([`element_children`]), or
/// `None` when `node` is not inside `parent`. Web-core's index into
/// `children`: how the `x-swiper` and the `viewpager` name the item or page
/// a snap position belongs to.
pub(super) fn element_child_index(
    document: &LynxDocument,
    parent: NodeId,
    node: NodeId,
) -> Option<usize> {
    let mut child = node;
    loop {
        let up = document.get(child)?.parent_id()?;
        if up == parent {
            break;
        }
        child = up;
    }
    element_children(document, parent).position(|element| element == child)
}

/// One event a component owes script, at the element the component belongs
/// to. None of them bubbles; a [`ComponentEvent::Detailed`] alone is
/// composed. A [`ComponentEvent::Frame`] is dispatched under an internal
/// name, which only `bobcat:element`'s own listener on a `frame` hears.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ComponentEvent {
    /// An `<image>`'s or `<svg>`'s own source settled: `load`, which carries
    /// an `<image>`'s intrinsic size and an `<svg>`'s layout size, or `error`,
    /// which an `<svg>` does not fire.
    Image(ImageOutcome),
    /// An event whose detail is `{}`, named by the component that queued
    /// it: a `<dialog>`'s `close` and `cancel` ([`terminus::link::html::dialog`],
    /// through the document's queued-event sink), an
    /// `<overlay>`'s `showoverlay` and `dismissoverlay` ([`overlay`]), an
    /// `x-refresh-view`'s `headerreleased`, `footerreleased` and
    /// `startloadmore` ([`refresh_view`]).
    Plain { node: NodeId, name: &'static str },
    /// A scroll container's `scroll` or `scrollend`, which the runtime
    /// queues, with the change in its offset the event reports (zero for a
    /// `scrollend`); the rest of Lynx's scroll metrics are read at
    /// delivery. A `scroll-view`'s or `list`'s `scrolltoupper` and
    /// `scrolltolower` take the same shape, queued by its component
    /// ([`scroll_container`]) with the change since its previous `scroll`.
    Scroll {
        node: NodeId,
        name: &'static str,
        delta: terminus::link::Vector2D<f32>,
    },
    /// An event whose detail the component read when it queued the event,
    /// named by that component: `<x-markdown>`'s `parseEnd`, `link` and
    /// `imageTap` ([`markdown`]), and an `x-input`'s or `x-textarea`'s
    /// `input`, `confirm`, `selection`, `focus` and `blur`
    /// ([`text_field`]). [`ComponentDetail`] is the detail's shape.
    ///
    /// Dispatched not bubbling and composed (ruled 2026-10-14). Not bubbling
    /// is native's: Lynx fires `<markdown>`'s `link`, `imageTap` and
    /// `parseEnd` as custom events that do not bubble
    /// (`lynx/core/renderer/events/touch_event_handler.cc:388-403`,
    /// `lynx/core/event/custom_event.h:17-23`), so only a `bindEvent` on the
    /// element itself hears one, never an ancestor's nor the
    /// `global-bindEvent` pass; a text field's events do not bubble in
    /// web-core either (`XInputEvents.ts`). Composed is so that an event
    /// queued at a node of a UA shadow tree retargets to the host for
    /// script, whose `target` is then the component's element; a text
    /// field queues its events at the field element itself, outside its
    /// shadow tree, where composed changes nothing.
    Detailed {
        node: NodeId,
        name: &'static str,
        detail: ComponentDetail,
    },
    /// A `frame` was connected to the document or disconnected from it
    /// ([`frame`]): not an event script hears, but a call into
    /// `bobcat:frame`, which loads the frame's card on a connection and
    /// disposes of it on a disconnection.
    Frame { node: NodeId, connected: bool },
    /// A reporting `text`'s paragraph was recomputed by a committing layout
    /// run: its `layout` event (`crate::main::layout_change`), with what the
    /// commit left — the lines it shows ([`terminus::link::Document::text_lines`]) and
    /// its content box (the border box less padding and border, in CSS px,
    /// native's `GetContentBoxWidth`/`GetContentBoxHeight`,
    /// `text_fragment_behavior.cc:86-87`) — read when the epilogue queued it.
    ///
    /// Not bubbling and not composed: native fires it as a custom event,
    /// which does not bubble (`text_fragment_behavior.cc:65-124`,
    /// `lynx/core/event/custom_event.h:17-23`), at the `text` itself, which is
    /// outside every UA shadow tree.
    TextLayout {
        node: NodeId,
        lines: Vec<terminus::link::TextLineReport>,
        width: f32,
        height: f32,
    },
}

/// The `detail` of a [`ComponentEvent::Detailed`]: one variant per *shape*
/// the realm builds, never per event name, so two events whose details have
/// the same keys share one.
///
/// Owned, because the event outlives the call that queued it: the component
/// reads its attributes, its content or its value at that moment, and the
/// runtime delivers from an entry of its own. The event itself is
/// `EventDetail::None` on the kernel's side; each member crosses as one entry
/// of the extras channel (`"id"`, `"url"`, `"value"`, `"selectionStart"`,
/// ...), from which the realm's Lynx layer builds the `detail` object, so no
/// JSON is written or parsed. A `None` pushes no entry, and the key is absent from the object —
/// neither `undefined`-valued nor `""`: web-core's `{id: undefined}` loses
/// that key when it is serialized, and the transport here would carry an
/// `undefined`-valued key as one.
#[derive(Debug, Clone, PartialEq)]
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
    /// `{value}`: a text field's value, such as an `x-input`'s or
    /// `x-textarea`'s `focus`, `blur` and `confirm` report.
    Value { value: String },
    /// `{value, textLength, cursor, isComposing, selectionStart,
    /// selectionEnd}`: a text field's value after an edit, its selection
    /// and whether a composition is in progress, such as an `x-input`'s or
    /// `x-textarea`'s `input` reports. Offsets in UTF-16 units; `textLength`
    /// (the value's UTF-16 length) and the deprecated `cursor` (the
    /// selection's start) are computed when the runtime expands the detail.
    Input {
        value: String,
        selection: Range<u32>,
        is_composing: bool,
    },
    /// `{selectionStart, selectionEnd}`: a text field's selection in UTF-16
    /// units, such as an `x-input`'s or `x-textarea`'s `selection` reports.
    Selection { selection: Range<u32> },
    /// `{start, end, direction}`: a Lynx text element's selection as offsets
    /// of its text content in UTF-16 units and the direction from anchor to
    /// focus (`"forward"` or `"backward"`), such as a `text`'s or
    /// `inline-text`'s `selectionchange` reports ([`text`]); `None` is the
    /// empty record `-1`, `-1`, `"forward"`, and `forward` is then `true`.
    TextSelection {
        range: Option<Range<u32>>,
        forward: bool,
    },
    /// Numbers and booleans under the keys the component names, in its
    /// order: the shape of a detail with no string in it, such as a
    /// `{current, isDragged}`. Each pair is one key of the `detail`, so two
    /// pairs under one key are a component's mistake the realm resolves as
    /// an object literal would, the later one kept.
    Fields(SmallVec<[(&'static str, Field); 4]>),
}

/// One value of a [`ComponentDetail::Fields`] pair: a JavaScript number or
/// boolean.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Field {
    Number(f64),
    Bool(bool),
}

/// The events this document's components have produced and not delivered
/// yet, in the order they formed.
///
/// A handle rather than a field, because the producers are on both sides of
/// the document: the `image` component, which is inside it and reaches nothing
/// else; the runtime's own image report path and its drain of the text
/// layout reports, which are outside it; HTML's
/// `dialog` component's UI methods ([`terminus::link::html::dialog`]), through the
/// document's queued-event sink ([`QueuedEventSink`]); and the `overlay` and
/// `frame` components, inside the document like `image`, as is any component that
/// queues a [`ComponentEvent::Detailed`].
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

    /// Queues a whole image report batch's outcomes, in the order `terminus`
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

    /// Queues a `scroll` or `scrollend` at `node`, with the offset change it
    /// reports.
    pub(crate) fn queue_scroll(
        &self,
        node: NodeId,
        name: &'static str,
        delta: terminus::link::Vector2D<f32>,
    ) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Scroll { node, name, delta });
    }

    /// Queues an event at `node` whose detail is `detail`'s strings: not
    /// bubbling, composed ([`ComponentEvent::Detailed`]).
    pub(crate) fn queue_detailed(&self, node: NodeId, name: &'static str, detail: ComponentDetail) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Detailed { node, name, detail });
    }

    /// Queues a `text`'s `layout` with the lines and content box its last
    /// commit left ([`ComponentEvent::TextLayout`]).
    pub(crate) fn queue_text_layout(
        &self,
        node: NodeId,
        lines: Vec<terminus::link::TextLineReport>,
        width: f32,
        height: f32,
    ) {
        self.0.borrow_mut().push(ComponentEvent::TextLayout {
            node,
            lines,
            width,
            height,
        });
    }

    /// Queues a `frame`'s connection (`connected`) or disconnection.
    pub(crate) fn queue_frame(&self, node: NodeId, connected: bool) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Frame { node, connected });
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

/// The document's queued-event sink, which [`new_document`] registers: an
/// event one of `terminus`'s own elements queues ([`terminus::link::html`]) is a
/// [`ComponentEvent::Plain`], pushed at the moment it is queued, in order
/// with what this document's components queue.
impl QueuedEventSink for ComponentEvents {
    fn queue(&self, target: NodeId, name: &'static str) {
        ComponentEvents::queue(self, target, name);
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
