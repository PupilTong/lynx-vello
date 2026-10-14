//! The `frame` tag: a box another card renders into.
//!
//! Native's `FrameElement` loads a second Lynx bundle by its `src` and
//! renders it in a pipeline of its own (`lynx/core/renderer/dom/fiber/
//! frame_element.cc`); web-core maps `frame` to a nested `lynx-view`
//! (`web-core/ts/constants.ts:121`), whose shadow root holds the nested
//! card's `[part="page"]` and its sheets, whose MTS runs in a realm of its
//! own, and which renders its card on `connectedCallback` and disposes of it
//! on `disconnectedCallback` (`web-core/ts/client/mainthread/LynxView.ts`).
//! This engine follows web-core's structure over one document, as a custom
//! element ability (user ruling): the element, its shadow root and the
//! nested page in it are this component's, and the realm that renders the
//! card is a `ShadowRealm` child of the MTS realm, which `bobcat:frame`
//! creates and disposes of.
//!
//! # The component
//!
//! [`Frame`] attaches an open shadow root when the element is constructed
//! and appends the nested page to it: a `page` element with web-core's
//! `part="page"`, which lives as long as the frame does. A card the frame
//! loads renders under it ([`nested_page`] is how the host's `framePage`
//! finds it), and a card it lets go of takes its children and its sheets
//! with it, never the page. There is no shadow sheet and no slot. With no
//! slot, the frame's light children are assigned nowhere and never render,
//! which is native's "a frame element cannot adopt any child"
//! (`frame_element.cc`, `OnNodeAdded`) without refusing the insertion.
//! `src` is an ordinary attribute: what loads it is JavaScript's, so no
//! attribute is observed.
//!
//! What the component does observe is its connection: each
//! `connectedCallback` and `disconnectedCallback` queues a
//! [`ComponentEvent::Frame`](super::ComponentEvent::Frame), which the
//! runtime turns into one call of `bobcat:frame`'s `__BobcatFrameConnected`
//! or `__BobcatFrameDisconnected` in the page realm, out of the mutation
//! that caused it. A reaction queues and never acts, because the callback
//! holds the document.
//!
//! # The UA rules
//!
//! The common box block of [`super::ua_sheet`] — a border box,
//! `position: relative`, `overflow: clip`, zero minimums — and
//! [`UA_RULES`]: `display: flex`, whatever `defaultDisplayLinear` says,
//! and `contain: strict`, web-core's `lynx-view` containment
//! (`web-core/css/index.css`). Strict containment makes the frame's size
//! independent of what renders in it: it is what its author and its
//! container give it — stretched across the page's column, and 0 tall with
//! no author height, as a native frame without `preset-height` or
//! `auto-height` is before its card reports one. The nested page is a `page`, so the UA sheet sizes
//! it 100% of the frame, which is web-core's `[part="page"] { width: 100%; height: 100% }`.

use dom::{CustomElement, NodeId, ShadowRootMode};

use super::{ComponentEvents, LynxDocument, PAGE_TAG};

/// Native's tag, and the JSX tag a compiled card writes.
pub(super) const FRAME_TAG: &str = "frame";

/// The part name web-core's nested page carries (`web-core/css/index.css`,
/// `[part="page"]`).
const PAGE_PART: &str = "page";

/// How many frames deep a nested page may be read, counting the frame
/// itself: what lets a card that embeds itself stop instead of recursing
/// without bound.
pub(crate) const MAX_FRAME_DEPTH: usize = 8;

/// The frame's own rules; see the module documentation.
pub(super) const UA_RULES: &str = "frame { display: flex; contain: strict; }\n";

/// Installs the component, over the queue its connections go on. Must run
/// before any element could carry the tag, which is
/// [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument, events: ComponentEvents) {
    document.define(FRAME_TAG, Box::new(Frame { events }));
}

/// The frame component: an open shadow root with the nested page in it,
/// built when the element is constructed, so the element is new and
/// detached when it gains them; and its connections, queued.
struct Frame {
    events: ComponentEvents,
}

impl CustomElement<()> for Frame {
    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        let shadow = document.attach_shadow(element, ShadowRootMode::Open);
        let page = document.create_element(PAGE_TAG, ());
        document.set_attribute(page, "part", PAGE_PART);
        document.append_child(shadow, page);
    }

    fn connected_callback(&self, _document: &mut LynxDocument, element: NodeId) {
        self.events.queue_frame(element, true);
    }

    fn disconnected_callback(&self, _document: &LynxDocument, element: NodeId) {
        self.events.queue_frame(element, false);
    }
}

/// Whether `node` is a live `frame` element.
pub(crate) fn is_frame(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(dom::Node::tag_name)
        .is_some_and(|tag| tag == FRAME_TAG)
}

/// The nested page of the frame `frame`: the first element child of its
/// shadow root. `None` for a node that is not a live frame.
pub(crate) fn nested_page(document: &LynxDocument, frame: NodeId) -> Option<NodeId> {
    if !is_frame(document, frame) {
        return None;
    }
    let shadow = document.shadow_root(frame)?;
    document
        .get(shadow)?
        .children()
        .find(|child| child.is_element())
        .map(dom::Node::id)
}

/// How many frames `node` sits in, itself included: every frame among its
/// shadow-including inclusive ancestors. A shadow root's parent is its host
/// here, so the parent chain crosses every shadow boundary.
pub(crate) fn frame_depth(document: &LynxDocument, node: NodeId) -> usize {
    let mut depth = 0;
    let mut current = Some(node);
    while let Some(id) = current {
        if is_frame(document, id) {
            depth += 1;
        }
        current = document.get(id).and_then(dom::Node::parent_id);
    }
    depth
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

    use dom::ShadowRootMode;
    use dom::stylo::values::computed::Display;

    use super::super::test_support::{
        child, display, document, element_under, with_component_events, with_config,
    };
    use super::super::{ComponentEvent, ComponentEvents, PAGE_TAG, PageConfig};
    use super::{FRAME_TAG, frame_depth, is_frame, nested_page};

    #[test]
    fn a_frame_has_an_open_shadow_root_and_is_a_flex_box_under_either_switch() {
        for linear in [true, false] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let frame = child(&mut document, FRAME_TAG, "width: 100px; height: 50px");
            let shadow = document
                .shadow_root(frame)
                .expect("a frame hosts a shadow root");
            assert_eq!(
                document.shadow_root_mode(shadow),
                Some(ShadowRootMode::Open)
            );
            document.layout();
            assert_eq!(display(&document, frame), Display::Flex, "linear {linear}");
            let size = document.rounded_layout(frame).expect("laid out").size;
            assert_eq!((size.width, size.height), (100.0, 50.0));
        }
    }

    /// The nested page is built with the element, detached or not, and is
    /// the shadow root's one child: a `page` with web-core's `part`.
    #[test]
    fn a_frame_is_constructed_with_its_nested_page() {
        let mut document = document();
        let frame = document.create_element(FRAME_TAG, ());
        let shadow = document.shadow_root(frame).expect("a shadow root");
        let page = nested_page(&document, frame).expect("a nested page");
        let node = document.get(page).expect("live");
        assert_eq!(node.tag_name(), Some(PAGE_TAG));
        assert_eq!(node.attribute("part"), Some("page"));
        assert_eq!(document.get(shadow).unwrap().child_ids(), [page]);
        let view = child(&mut document, "view", "");
        assert_eq!(nested_page(&document, view), None, "a view is no frame");
    }

    /// Strict containment: what renders inside sizes nothing. The page's
    /// column still stretches the frame across it.
    #[test]
    fn a_frames_content_does_not_size_it() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "");
        let page = nested_page(&document, frame).expect("a nested page");
        element_under(&mut document, page, "view", "width: 80px; height: 80px");
        document.layout();
        let size = document.rounded_layout(frame).expect("laid out").size;
        assert_eq!((size.width, size.height), (393.0, 0.0));
    }

    /// The shadow root has no slot, so a light child is assigned nowhere and
    /// generates no box.
    #[test]
    fn a_frames_light_children_do_not_render() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "width: 100px; height: 50px");
        let light = element_under(&mut document, frame, "view", "width: 40px; height: 40px");
        document.layout();
        assert!(
            document.bounding_client_rect(light).is_none(),
            "a light child of a frame has no box"
        );
    }

    /// The nested page fills the frame: the UA sheet's `page` rule is
    /// web-core's `[part="page"]`.
    #[test]
    fn the_nested_page_fills_the_frame() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "width: 100px; height: 50px");
        let page = nested_page(&document, frame).expect("a nested page");
        document.layout();
        let size = document.rounded_layout(page).expect("laid out").size;
        assert_eq!((size.width, size.height), (100.0, 50.0));
    }

    /// What the component queued since the last call, as `(node,
    /// connected)`.
    fn queued(events: &ComponentEvents) -> Vec<(dom::NodeId, bool)> {
        events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Frame { node, connected } => (node, connected),
                other => panic!("no such event here: {other:?}"),
            })
            .collect()
    }

    /// Each connection and each disconnection is one queued event, a frame
    /// inside another frame's nested page included; a detached frame queues
    /// nothing.
    #[test]
    fn connecting_and_disconnecting_a_frame_queues_one_event_each() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let frame = document.create_element(FRAME_TAG, ());
        let inner = document.create_element(FRAME_TAG, ());
        let nested = nested_page(&document, frame).expect("a nested page");
        document.append_child(nested, inner);
        assert_eq!(queued(&events), [], "nothing is connected yet");
        let page = document.document_element().id();
        document.append_child(page, frame);
        assert_eq!(queued(&events), [(frame, true), (inner, true)]);
        document.remove_element(frame);
        assert_eq!(queued(&events), [(frame, false), (inner, false)]);
        document.append_child(page, frame);
        assert_eq!(queued(&events), [(frame, true), (inner, true)]);
    }

    #[test]
    fn frame_depth_counts_the_frames_a_node_sits_in() {
        let mut document = document();
        let outer = child(&mut document, FRAME_TAG, "");
        let inner = document.create_element(FRAME_TAG, ());
        let page = nested_page(&document, outer).expect("a nested page");
        document.append_child(page, inner);
        let deep = nested_page(&document, inner).expect("a nested page");
        assert_eq!(frame_depth(&document, document.document_element().id()), 0);
        assert_eq!(frame_depth(&document, outer), 1);
        assert_eq!(frame_depth(&document, page), 1);
        assert_eq!(frame_depth(&document, inner), 2);
        assert_eq!(frame_depth(&document, deep), 2);
    }

    #[test]
    fn is_frame_names_frames_alone() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "");
        let view = child(&mut document, "view", "");
        assert!(is_frame(&document, frame));
        assert!(!is_frame(&document, view));
        assert!(!is_frame(&document, document.document_element().id()));
        let page = nested_page(&document, frame).expect("a nested page");
        document.remove_element(frame);
        document.drop_element(frame);
        assert!(!is_frame(&document, frame), "a freed frame is no frame");
        assert!(document.get(page).is_none(), "its nested page went with it");
    }
}
