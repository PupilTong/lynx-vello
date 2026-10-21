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
//! `part="page"`. The page belongs to the card, not to the frame: a card the
//! frame loads renders under the page the frame holds then ([`nested_page`]
//! is how the host's `framePage` finds it), and a card it lets go of takes
//! its page with it, everything under the page and the sheets on the shadow
//! root included; [`reset_frame`] frees them and builds a fresh page for the next
//! card, as web-core's nested `lynx-view` renders each card into a page of
//! its own. A card's realm therefore never meets the next card's nodes, and
//! a node of the old page names nothing once it is freed. There is no
//! shadow sheet and no slot. With no
//! slot, the frame's light children are assigned nowhere and never render,
//! which is native's "a frame element cannot adopt any child"
//! (`frame_element.cc`, `OnNodeAdded`) without refusing the insertion.
//! No attribute is observed here: `src`, `data` and `global-props` are
//! `bobcat:frame`'s, in JavaScript, which `bobcat:element` calls after
//! writing them, and `auto-width` and `auto-height` act through the UA
//! rules alone.
//!
//! What the component does observe is its connection: each
//! `connectedCallback` and `disconnectedCallback` queues a
//! [`ComponentEvent::Frame`](super::ComponentEvent::Frame), which the
//! runtime dispatches at the frame as the internal event
//! `bobcat:frameconnected` or `bobcat:framedisconnected`, out of the mutation
//! that caused it, so the realm that holds the frame's handle hears it. A
//! reaction queues and never acts, because the callback holds the document.
//!
//! # The UA rules
//!
//! The common box block of [`super::ua_sheet`] — a border box,
//! `position: relative`, `overflow: clip`, zero minimums — and
//! [`UA_RULES`]: `display: flex`, whatever `defaultDisplayLinear` says,
//! and `contain: strict`, web-core's `lynx-view` containment
//! (`web-core/css/index.css:7-18`). Strict containment makes the frame's
//! size independent of what renders in it: it is what its author and its
//! container give it. Under the default linear page, that is stretched
//! across the page's width and 0 tall with no author height and no
//! `auto-height`; a row stretches its height instead. The nested page is a
//! `page`, so the UA sheet sizes it 100% of the frame, which is web-core's
//! `[part="page"] { width: 100%; height: 100% }`
//! (`web-core/css/index.css:47-55`).
//!
//! Two boolean attributes let the nested card's content size the frame.
//! Each is a selector over the attribute and `:not([…="false"])`, as
//! `image[auto-size]` is ([`super::image`]), because `__SetAttribute`
//! writes `false` as the string `"false"`.
//!
//! - `auto-height`, on which native and web-core agree, relaxes the containment to `inline-size
//!   layout paint style`. That is web-core's `lynx-view[auto-height] { contain: content }` over the
//!   `container-type: inline-size` every `lynx-view` has (`web-core/css/index.css:7-45`), which
//!   together contain the width alone. Native lays the nested card out with an unspecified height
//!   (`LynxFrameView.java:199-208`) and gives the frame the height the card reports
//!   (`FrameShadowNode.java:57-83`).
//! - `auto-width` is native's alone, and native is what this follows (user ruling). Native lays the
//!   nested card out with an unspecified width (`LynxFrameView.java:188-197`), and
//!   `FrameShadowNode.measure` gives the frame the width the card reports wherever its own width is
//!   not `EXACTLY`: an author width or a stretch still wins, and min and max still clamp. Here that
//!   is `contain: layout paint style`, no size containment at all, so the frame's width is its
//!   content's on an axis nothing makes definite, as any flex box's is. The rule comes after
//!   `auto-height`'s at the same specificity, so a frame with both attributes takes it. The gap,
//!   accepted by the same ruling: CSS has no containment of the height alone, so an `auto-width`
//!   frame with no author height takes its content's height too, where native leaves it 0 tall.
//!
//! The nested page needs no rule of its own for either. Its `height: 100%`
//! of an indefinite height behaves as `auto`, so its content's height is
//! an `auto-height` frame's; its `width: 100%` is cyclic in the frame's
//! intrinsic size contribution, which takes it as `auto` (css-sizing-3
//! §5.2.1), so its content's width is an `auto-width` frame's. Either way
//! the page then fills what the frame became.
//!
//! `preset-width` and `preset-height` are accepted and do nothing. Native
//! hands them to the nested card as its viewport until the frame has a
//! content rect of its own (`LynxFrameView.java:106-110`, `149-156`) and
//! never sizes the frame's box by them; web-core reads neither. Here the
//! nested page lays out in the same pass as the frame, so there is no
//! moment before the frame has a size. `enable-multi-async-thread` is
//! native's choice of threads for the nested card
//! (`LynxFrameView.java:103-105`), which web-core does not read either: it
//! is accepted and does nothing.

use terminus::link::{CustomElement, NodeId, ShadowRootMode};

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

/// The frame's own rules; see the module documentation. The `auto-width`
/// rule comes last at the same specificity, so a frame with both attributes
/// takes its `contain: layout paint style`.
pub(super) const UA_RULES: &str = r#"frame { display: flex; contain: strict; }
frame[auto-height]:not([auto-height="false"]) { contain: inline-size layout paint style; }
frame[auto-width]:not([auto-width="false"]) { contain: layout paint style; }
"#;

/// Installs the component, over the queue its connections go on. Must run
/// before any element could carry the tag, which is
/// [`Document::define`](terminus::link::Document::define)'s own precondition.
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
        append_nested_page(document, shadow);
    }

    fn connected_callback(&self, _document: &mut LynxDocument, element: NodeId) {
        self.events.queue_frame(element, true);
    }

    fn disconnected_callback(&self, _document: &LynxDocument, element: NodeId) {
        self.events.queue_frame(element, false);
    }
}

/// Appends a fresh nested page to the frame's shadow root `shadow` and
/// answers it: the one the first card renders under, built with the element,
/// and the one each later card does, built by [`reset_frame`].
fn append_nested_page(document: &mut LynxDocument, shadow: NodeId) -> NodeId {
    let page = document.create_element(PAGE_TAG, ());
    document.set_attribute(page, "part", PAGE_PART);
    document.append_child(shadow, page);
    page
}

/// Lets go of the card the frame `frame` shows: frees its nested page with
/// everything under it — the page is inside a shadow tree, so
/// [`terminus::link::Document::drop_element`] frees the whole subtree outright, and
/// each freed node takes its listener rows and its intersection
/// registrations with it through the document's free path — and every sheet
/// mounted on the frame's shadow root, so the next card is styled by its
/// own alone; then builds a fresh nested page for that card. Answers the new
/// page, or `None`, changing nothing, for a node that is not a live frame.
pub(crate) fn reset_frame(document: &mut LynxDocument, frame: NodeId) -> Option<NodeId> {
    let page = nested_page(document, frame)?;
    let shadow = document
        .shadow_root(frame)
        .expect("a nested page sits in its frame's shadow root");
    document.drop_element(page);
    document.clear_shadow_stylesheets(shadow);
    Some(append_nested_page(document, shadow))
}

/// Whether `node` is a live `frame` element.
pub(crate) fn is_frame(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(terminus::link::Node::tag_name)
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
        .map(terminus::link::Node::id)
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
        current = document.get(id).and_then(terminus::link::Node::parent_id);
    }
    depth
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

    use terminus::link::stylo::values::computed::Display;
    use terminus::link::{NodeId, ShadowRootMode};

    use super::super::test_support::{
        child, display, document, element_under, with_component_events, with_config,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PAGE_TAG, PageConfig};
    use super::{FRAME_TAG, frame_depth, is_frame, nested_page, reset_frame};

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

    /// A reset frees the nested page with everything under it, empties the
    /// shadow root's sheets, and builds the next card a page of its own: a
    /// different node, the shadow root's one child again, with the same tag
    /// and `part`. A node that is no frame is left alone.
    #[test]
    fn a_reset_frees_the_nested_page_and_builds_the_next() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "width: 100px; height: 50px");
        let shadow = document.shadow_root(frame).expect("a shadow root");
        let old = nested_page(&document, frame).expect("a nested page");
        let content = element_under(&mut document, old, "view", "width: 80px; height: 80px");
        let deeper = element_under(&mut document, content, "view", "");
        document.add_shadow_stylesheet(shadow, "view { color: red; }");
        document.layout();

        let page = reset_frame(&mut document, frame).expect("a frame");
        assert_ne!(page, old, "a page of its own for the next card");
        assert_eq!(nested_page(&document, frame), Some(page));
        assert_eq!(document.get(shadow).unwrap().child_ids(), [page]);
        let node = document.get(page).expect("live");
        assert_eq!(node.tag_name(), Some(PAGE_TAG));
        assert_eq!(node.attribute("part"), Some("page"));
        for freed in [old, content, deeper] {
            assert!(document.get(freed).is_none(), "{freed:?} was freed");
        }
        document.layout();
        let size = document.rounded_layout(page).expect("laid out").size;
        assert_eq!(
            (size.width, size.height),
            (100.0, 50.0),
            "it fills the frame"
        );

        let view = child(&mut document, "view", "");
        assert_eq!(reset_frame(&mut document, view), None, "a view is no frame");
        assert!(document.get(view).is_some());
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

    /// Both values of `defaultDisplayLinear`, which decides the nested page's
    /// own display and so how its content measures.
    fn under_either_switch() -> impl Iterator<Item = (bool, LynxDocument)> {
        [true, false].into_iter().map(|linear| {
            (
                linear,
                with_config(PageConfig {
                    default_display_linear: linear,
                    ..PageConfig::default()
                }),
            )
        })
    }

    /// A frame under `parent` with `style` and `attributes`, and one view of
    /// `content` in its nested page; the frame and its nested page.
    fn frame_holding(
        document: &mut LynxDocument,
        parent: NodeId,
        style: &str,
        attributes: &[(&str, &str)],
        content: &str,
    ) -> (NodeId, NodeId) {
        let frame = element_under(document, parent, FRAME_TAG, style);
        for (name, value) in attributes {
            document.set_attribute(frame, name, value);
        }
        let page = nested_page(document, frame).expect("a nested page");
        element_under(document, page, "view", content);
        (frame, page)
    }

    /// A flex container under the page with `style`. The page's own display
    /// follows `defaultDisplayLinear` (a vertical linear box, or a row flex
    /// container that stretches a child's height), so the tests that run
    /// under either switch place the frame in a container of their own and
    /// let the switch decide only the nested page's display.
    fn container(document: &mut LynxDocument, style: &str) -> NodeId {
        child(document, "view", &format!("display: flex; {style}"))
    }

    /// A row that does not stretch: a frame in it is sized by its own
    /// contribution on both axes.
    fn row(document: &mut LynxDocument) -> NodeId {
        container(document, "flex-direction: row; align-items: flex-start")
    }

    fn size_of(document: &LynxDocument, node: NodeId) -> (f32, f32) {
        let size = document.rounded_layout(node).expect("laid out").size;
        (size.width, size.height)
    }

    /// `auto-height`: the nested card's content sizes the frame's height,
    /// and the page, `height: 100%` of an indefinite height, takes it too.
    #[test]
    fn an_auto_height_frame_takes_its_contents_height() {
        for (linear, mut document) in under_either_switch() {
            let column = container(&mut document, "flex-direction: column");
            let (frame, nested) = frame_holding(
                &mut document,
                column,
                "width: 300px",
                &[("auto-height", "")],
                "width: 80px; height: 80px",
            );
            document.layout();
            assert_eq!(size_of(&document, frame), (300.0, 80.0), "linear {linear}");
            assert_eq!(size_of(&document, nested), (300.0, 80.0), "linear {linear}");
        }
    }

    /// `__SetAttribute` writes `false` as the string `"false"`, which is off.
    #[test]
    fn auto_height_false_is_off() {
        let mut document = document();
        let page = document.document_element().id();
        let (frame, _) = frame_holding(
            &mut document,
            page,
            "width: 300px",
            &[("auto-height", "false")],
            "width: 80px; height: 80px",
        );
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 0.0));
    }

    /// An author height is a definite height, which content does not move.
    #[test]
    fn an_author_height_wins_over_auto_height() {
        let mut document = document();
        let page = document.document_element().id();
        let (frame, nested) = frame_holding(
            &mut document,
            page,
            "width: 300px; height: 50px",
            &[("auto-height", "")],
            "width: 80px; height: 80px",
        );
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 50.0));
        assert_eq!(size_of(&document, nested), (300.0, 50.0));
    }

    /// `auto-width`: where nothing stretches it, the nested card's content
    /// width is the frame's, and the page's `width: 100%` follows it.
    #[test]
    fn an_auto_width_frame_takes_its_contents_width() {
        for (linear, mut document) in under_either_switch() {
            let row = row(&mut document);
            let (frame, nested) = frame_holding(
                &mut document,
                row,
                "height: 120px",
                &[("auto-width", "")],
                "width: 70px; height: 80px",
            );
            document.layout();
            assert_eq!(size_of(&document, frame), (70.0, 120.0), "linear {linear}");
            assert_eq!(size_of(&document, nested), (70.0, 120.0), "linear {linear}");
        }
    }

    /// Both attributes: the content's size on both axes.
    #[test]
    fn a_frame_with_both_attributes_takes_its_contents_size() {
        for (linear, mut document) in under_either_switch() {
            let row = row(&mut document);
            let (frame, nested) = frame_holding(
                &mut document,
                row,
                "",
                &[("auto-width", ""), ("auto-height", "")],
                "width: 70px; height: 80px",
            );
            document.layout();
            assert_eq!(size_of(&document, frame), (70.0, 80.0), "linear {linear}");
            assert_eq!(size_of(&document, nested), (70.0, 80.0), "linear {linear}");
        }
    }

    /// A stretched width is a definite one, which wins over `auto-width` as
    /// native's `EXACTLY` measure mode does. With no author height, the
    /// frame also takes its content's height: CSS has no containment of the
    /// height alone, where native would leave the frame 0 tall.
    #[test]
    fn a_stretched_auto_width_frame_keeps_the_stretched_width() {
        for (linear, mut document) in under_either_switch() {
            let column = container(&mut document, "flex-direction: column; width: 200px");
            let (frame, nested) = frame_holding(
                &mut document,
                column,
                "",
                &[("auto-width", "")],
                "width: 70px; height: 80px",
            );
            document.layout();
            assert_eq!(size_of(&document, frame), (200.0, 80.0), "linear {linear}");
            assert_eq!(size_of(&document, nested), (200.0, 80.0), "linear {linear}");
        }
    }

    /// The same on the other axis: a row stretches an `auto-height` frame's
    /// height to its own, and the content does not move it.
    #[test]
    fn a_stretched_auto_height_frame_keeps_the_stretched_height() {
        for (linear, mut document) in under_either_switch() {
            let row = container(&mut document, "flex-direction: row; height: 150px");
            let (frame, nested) = frame_holding(
                &mut document,
                row,
                "width: 300px",
                &[("auto-height", "")],
                "width: 80px; height: 80px",
            );
            document.layout();
            assert_eq!(size_of(&document, frame), (300.0, 150.0), "linear {linear}");
            assert_eq!(
                size_of(&document, nested),
                (300.0, 150.0),
                "linear {linear}"
            );
        }
    }

    /// `preset-height` is native's nested viewport before the first content
    /// rect, a window this engine does not have: it sizes nothing.
    #[test]
    fn preset_height_has_no_effect() {
        let mut document = document();
        let page = document.document_element().id();
        let (frame, _) = frame_holding(
            &mut document,
            page,
            "",
            &[("preset-height", "200px"), ("preset-width", "100px")],
            "width: 80px; height: 80px",
        );
        document.layout();
        assert_eq!(size_of(&document, frame), (393.0, 0.0));
    }

    /// The attribute rules restyle: adding `auto-height` sizes the frame by
    /// its content and removing it again puts it back at 0.
    #[test]
    fn toggling_auto_height_relayouts_the_frame() {
        let mut document = document();
        let page = document.document_element().id();
        let (frame, _) = frame_holding(
            &mut document,
            page,
            "width: 300px",
            &[],
            "width: 80px; height: 80px",
        );
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 0.0));
        document.set_attribute(frame, "auto-height", "");
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 80.0), "on");
        document.remove_attribute(frame, "auto-height");
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 0.0), "off again");
        document.set_attribute(frame, "auto-height", "false");
        document.layout();
        assert_eq!(size_of(&document, frame), (300.0, 0.0), "\"false\"");
    }

    /// What the component queued since the last call, as `(node,
    /// connected)`.
    fn queued(events: &ComponentEvents) -> Vec<(terminus::link::NodeId, bool)> {
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
