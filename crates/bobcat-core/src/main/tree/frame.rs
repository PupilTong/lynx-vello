//! The `frame` tag: a box another card renders into.
//!
//! Native's `FrameElement` loads a second Lynx bundle by its `src` and
//! renders it in a pipeline of its own (`lynx/core/renderer/dom/fiber/
//! frame_element.cc`); web-core maps `frame` to a nested `lynx-view`
//! (`web-core/ts/constants.ts:121`), whose shadow root holds the nested
//! card's `[part="page"]` and its sheets, and whose MTS runs in a realm of
//! its own. This engine follows web-core's structure over one document: the
//! element is this component's, its shadow root is where the nested page
//! goes, and the realm that renders it is a `ShadowRealm` child of the MTS
//! realm bound to it (`main/runtime/frames.rs`).
//!
//! # The component
//!
//! [`Frame`] attaches an open shadow root when the element is constructed
//! and does nothing else: no shadow sheet and no slot. The nested page is
//! the binding's to create, because only a realm that binds the frame has
//! one. With no slot, the frame's light children are assigned nowhere and
//! never render, which is native's "a frame element cannot adopt any child"
//! (`frame_element.cc`, `OnNodeAdded`) without refusing the insertion.
//! `src` is an ordinary attribute: what loads it is JavaScript's, so no
//! attribute is observed. A frame that leaves the document keeps its
//! binding and its realm (`docs/tracking/deviations.md`).
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

use super::LynxDocument;

/// Native's tag, and the JSX tag a compiled card writes.
pub(super) const FRAME_TAG: &str = "frame";

/// The frame's own rules; see the module documentation.
pub(super) const UA_RULES: &str = "frame { display: flex; contain: strict; }\n";

/// Installs the component. Must run before any element could carry the
/// tag, which is [`Document::define`](dom::Document::define)'s own
/// precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(FRAME_TAG, Box::new(Frame));
}

/// The frame component: an open shadow root, attached when the element is
/// constructed, so the element is new and detached when it gains one.
struct Frame;

impl CustomElement<()> for Frame {
    fn constructed(&self, document: &mut LynxDocument, element: NodeId) {
        document.attach_shadow(element, ShadowRootMode::Open);
    }
}

/// Whether `node` is a live `frame` element.
pub(crate) fn is_frame(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(dom::Node::tag_name)
        .is_some_and(|tag| tag == FRAME_TAG)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Authored pixel sizes lay out exactly.

    use dom::ShadowRootMode;
    use dom::stylo::values::computed::Display;

    use super::super::test_support::{child, display, document, element_under, with_config};
    use super::super::{PAGE_TAG, PageConfig};
    use super::{FRAME_TAG, is_frame};

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
            assert!(
                document
                    .get(shadow)
                    .is_some_and(|root| root.child_ids().is_empty()),
                "nothing in the shadow tree until a realm binds the frame"
            );
            document.layout();
            assert_eq!(display(&document, frame), Display::Flex, "linear {linear}");
            let size = document.rounded_layout(frame).expect("laid out").size;
            assert_eq!((size.width, size.height), (100.0, 50.0));
        }
    }

    /// Strict containment: what renders inside sizes nothing. The page's
    /// column still stretches the frame across it.
    #[test]
    fn a_frames_content_does_not_size_it() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "");
        let shadow = document.shadow_root(frame).expect("a shadow root");
        let page = document.create_element(PAGE_TAG, ());
        document.append_child(shadow, page);
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
    fn a_page_in_the_shadow_tree_fills_the_frame() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "width: 100px; height: 50px");
        let shadow = document.shadow_root(frame).expect("a shadow root");
        let page = document.create_element(PAGE_TAG, ());
        document.append_child(shadow, page);
        document.layout();
        let size = document.rounded_layout(page).expect("laid out").size;
        assert_eq!((size.width, size.height), (100.0, 50.0));
    }

    #[test]
    fn is_frame_names_frames_alone() {
        let mut document = document();
        let frame = child(&mut document, FRAME_TAG, "");
        let view = child(&mut document, "view", "");
        assert!(is_frame(&document, frame));
        assert!(!is_frame(&document, view));
        assert!(!is_frame(&document, document.document_element().id()));
        document.remove_element(frame);
        document.drop_element(frame);
        assert!(!is_frame(&document, frame), "a freed frame is no frame");
    }
}
