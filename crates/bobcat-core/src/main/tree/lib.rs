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
//! A Lynx tag is the document element's tag, with three exceptions: the
//! Lynx `svg` is the document element `lynx-svg`, and the Lynx `input` and
//! `textarea` are `x-input` and `x-textarea`, mapped at creation, at the tag
//! read and in a `StyleInfo` type selector ([`tag_map`]).
//!
//! Each tag owns its UA rules and tests. Numeric text and list attributes
//! flow through `attr()`; boolean flags use attribute selectors. Only `image`,
//! `lynx-svg`, `video`, `blur_view`, `swiper`, `refresh_view`, `viewpager`,
//! `scroll_container`, `list`, `scroll_coordinator`, `overlay`,
//! `markdown`, `frame` and `text_field` need components: for image resources (a `lynx-svg`'s
//! `content` as a document handed to the host as its bytes), the video element's picture, its
//! `play` and `pause` UI methods and its `firstframe`, `ended` and `error` events ([`video`]), blur
//! hints, the swiper's UA shadow tree, item count, `scrollTo` UI method and events, the
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
mod image_sources;
mod list;
mod lynx_svg;
mod markdown;
mod overlay;
#[cfg(test)]
#[path = "popover_tests.rs"]
mod popover_tests;
pub(crate) mod raw_text;
pub(crate) mod refresh_view;
mod scroll_container;
mod scroll_coordinator;
pub(crate) mod swiper;
mod tag_map;
#[cfg(test)]
pub(super) mod test_support;
mod text;
pub(crate) mod text_field;
mod touch_count;
mod ua_sheet;
mod video;
mod viewpager;
#[cfg(test)]
mod web_text_replication;

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Mutex;

use smallvec::SmallVec;
use terminus::link::event::QueuedEventSink;
use terminus::link::{Document, NodeId, UserAgentStylesheet};

pub(crate) use self::base_methods::invoke_base_method;
pub(crate) use self::frame::{MAX_FRAME_DEPTH, frame_depth, nested_page, reset_frame};
pub(crate) use self::image_sources::{
    ImageOutcome, ImageSources, START_PLAY_EVENT, apply_image_events,
};
pub(crate) use self::lynx_svg::is_svg;
pub(crate) use self::scroll_container::relevance_span;
pub(crate) use self::swiper::Autoplays;
pub use self::tag_map::document_tag;
pub(crate) use self::tag_map::lynx_tag;
pub(crate) use self::text::{SelectionReports, is_text, report_selection_changes};
pub(crate) use self::text_field::focus_delegate;
pub use self::ua_sheet::PageConfig;
pub(crate) use self::video::{Videos, apply_reports as apply_video_reports};
pub(crate) use crate::view::Viewport;

pub(crate) fn layout_change_target(document: &LynxDocument, element: NodeId) -> NodeId {
    overlay::dialog_of(document, element).unwrap_or(element)
}

pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

pub(super) const SLOT_CHANGE: &str = "slotchange";

pub(crate) const SELECTION_CHANGE: &str = "selectionchange";

#[must_use]
pub(crate) fn new_document(
    viewport: Viewport,
    config: PageConfig,
    events: ComponentEvents,
    autoplays: Autoplays,
    images: ImageSources,
    videos: Videos,
) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    document.set_queued_event_sink(Rc::new(events.clone()));
    blur_view::define(&mut document);
    image::define(&mut document, events.clone(), images.clone());
    lynx_svg::define(&mut document, events.clone(), images);
    video::define(&mut document, events.clone(), videos);
    swiper::define(&mut document, autoplays, events.clone());
    refresh_view::define(&mut document, events.clone());
    viewpager::define(&mut document, events.clone());
    scroll_container::define(&mut document, events.clone());
    list::define(&mut document, events.clone());
    scroll_coordinator::define(&mut document);
    text::define(&mut document);
    markdown::define(&mut document, events.clone());
    document.define_html_elements();
    text_field::define(&mut document, events.clone());
    overlay::define(&mut document, events.clone());
    frame::define(&mut document, events);
    document.add_user_agent_stylesheet(&ua_stylesheet(config));
    document
}

fn ua_stylesheet(config: PageConfig) -> UserAgentStylesheet {
    static PARSED: Mutex<Vec<(PageConfig, UserAgentStylesheet)>> = Mutex::new(Vec::new());
    let mut parsed = PARSED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, sheet)) = parsed.iter().find(|(known, _)| *known == config) {
        return sheet.clone();
    }
    let sheet = UserAgentStylesheet::parse(&ua_sheet::ua_stylesheet(config));
    parsed.push((config, sheet.clone()));
    sheet
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct InvalidParams;

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

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ComponentEvent {
    Image(ImageOutcome),
    Plain {
        node: NodeId,
        name: &'static str,
    },
    Scroll {
        node: NodeId,
        name: &'static str,
        delta: terminus::link::Vector2D<f32>,
    },
    Detailed {
        node: NodeId,
        name: &'static str,
        detail: ComponentDetail,
    },
    Frame {
        node: NodeId,
        connected: bool,
    },
    TextLayout {
        node: NodeId,
        lines: Vec<terminus::link::TextLineReport>,
        width: f32,
        height: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ComponentDetail {
    Id {
        id: Option<String>,
    },
    UrlContent {
        url: String,
        content: String,
        content_id: Option<String>,
    },
    Url {
        url: String,
        content_id: Option<String>,
    },
    Value {
        value: String,
    },
    Input {
        value: String,
        selection: Range<u32>,
        is_composing: bool,
    },
    Selection {
        selection: Range<u32>,
    },
    Error {
        code: i32,
        message: String,
    },
    TextSelection {
        range: Option<Range<u32>>,
        forward: bool,
    },
    Fields(SmallVec<[(&'static str, Field); 4]>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Field {
    Number(f64),
    Bool(bool),
}

#[derive(Clone, Default)]
pub(crate) struct ComponentEvents(Rc<RefCell<Vec<ComponentEvent>>>);

impl ComponentEvents {
    pub(crate) fn queue_image(&self, outcomes: impl IntoIterator<Item = ImageOutcome>) {
        self.0
            .borrow_mut()
            .extend(outcomes.into_iter().map(ComponentEvent::Image));
    }

    pub(crate) fn extend(&self, events: impl IntoIterator<Item = ComponentEvent>) {
        self.0.borrow_mut().extend(events);
    }

    pub(crate) fn extend_images(&self, outcomes: Vec<ImageOutcome>) {
        self.0
            .borrow_mut()
            .extend(outcomes.into_iter().map(ComponentEvent::Image));
    }

    pub(crate) fn queue(&self, node: NodeId, name: &'static str) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Plain { node, name });
    }

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

    pub(crate) fn queue_detailed(&self, node: NodeId, name: &'static str, detail: ComponentDetail) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Detailed { node, name, detail });
    }

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

    pub(crate) fn queue_frame(&self, node: NodeId, connected: bool) {
        self.0
            .borrow_mut()
            .push(ComponentEvent::Frame { node, connected });
    }

    pub(crate) fn take(&self) -> Vec<ComponentEvent> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

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
