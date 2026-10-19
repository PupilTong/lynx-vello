//! `raw-text` is a CSS text carrier. Its attribute supplies generated content
//! to the box that holds it; no custom element or DOM text child is needed.
//!
//! # Where a carrier renders
//!
//! - **Inside Lynx text** (`text`, `inline-text`, directly or through one `wrapper`): its text is a
//!   run of the paragraph, as in both references.
//! - **Inside any other Lynx tag** (`view`, `scroll-view`, `list`, …): nothing. Native Lynx drops a
//!   raw-text there (a virtual node never laid out or painted,
//!   `lynx/core/renderer/dom/fiber/fiber_element.cc:4099-4131`), and web-core hides every carrier
//!   outside `x-text` (`raw-text { display: none }`,
//!   `lynx-stack/packages/web-platform/web-elements/src/elements/ XText/x-text.css:261-265`).
//! - **Inside a tag that is not a Lynx tag** — an HTML element such as `div` or `span`, or any
//!   other name — directly or through one `wrapper`: its text is loose text of that element, laid
//!   out the way CSS lays out the text children of a flex, grid or grid-lanes container: each
//!   contiguous sequence in one anonymous block box, which is one item (`dom`'s `tree::anonymous`).
//!   A ruled deviation from web-core, which hides it there too (`docs/tracking/deviations.md`);
//!   native has no such tags.
//!
//! A tag is a Lynx tag when [`LYNX_TAGS`] lists it. `wrapper` is not one: it
//! generates no box, so the tag that matters is the one holding the wrapper —
//! one wrapper level, as inside Lynx text.

use std::sync::LazyLock;

/// Every Lynx tag: Lynx's JSX intrinsic elements
/// (`lynx/js_libraries/types/types/common/element/element.d.ts:56-90`),
/// web-core's tag-map keys (`lynx-stack/packages/web-platform/web-core/ts/
/// constants.ts:93-123`), and web-core's element names, which compiled cards
/// write verbatim and this engine defines (`x-swiper`, `x-refresh-view`, …;
/// `lynx-stack/packages/web-platform/web-elements/src/elements/`). Except
/// `input` and `textarea`: HTML element names, which follow HTML here (user
/// ruling 2026-10-19), so the text fields are `x-input` and `x-textarea`
/// only (`super::text_field`).
const LYNX_TAGS: &[&str] = &[
    "blur-view",
    "component",
    "filter-image",
    "frame",
    "image",
    "inline-image",
    "inline-text",
    "inline-truncation",
    "list",
    "list-item",
    "list-row",
    "lynx-wrapper",
    "markdown",
    "overlay",
    "page",
    "raw-text",
    "refresh",
    "refresh-header",
    "scroll-coordinator",
    "scroll-coordinator-header",
    "scroll-coordinator-slot",
    "scroll-coordinator-slot-drag",
    "scroll-coordinator-toolbar",
    "scroll-view",
    "svg",
    "text",
    "title-bar-view",
    "video",
    "view",
    "viewpager",
    "viewpager-item",
    "webview",
    "x-audio-tt",
    "x-blur-view",
    "x-canvas",
    "x-foldview-header-ng",
    "x-foldview-ng",
    "x-foldview-slot-drag-ng",
    "x-foldview-slot-ng",
    "x-foldview-toolbar-ng",
    "x-image",
    "x-input",
    "x-input-ng",
    "x-list",
    "x-markdown",
    "x-overlay-ng",
    "x-refresh-footer",
    "x-refresh-header",
    "x-refresh-view",
    "x-svg",
    "x-swiper",
    "x-swiper-item",
    "x-text",
    "x-textarea",
    "x-textarea-ng",
    "x-view",
    "x-viewpager-item-ng",
    "x-viewpager-ng",
    "x-webview",
];

/// Carriers are hidden unless the box holding them renders their text (module
/// doc), and author CSS may override or suppress their content. A Lynx
/// paragraph keeps every run verbatim (`text.rs`), so a
/// carrier's own `preserve-breaks` decides its newlines only as loose text in
/// an HTML element.
pub(super) static UA_RULES: LazyLock<String> = LazyLock::new(|| {
    let lynx = LYNX_TAGS.join(", ");
    format!(
        "raw-text {{ display: none; white-space-collapse: preserve-breaks; }}
text > raw-text, text > wrapper > raw-text,
inline-text > raw-text, inline-text > wrapper > raw-text,
:not({lynx}, wrapper) > raw-text,
:not({lynx}, wrapper) > wrapper > raw-text {{ display: contents; }}
raw-text {{ content: attr(text); }}
"
    )
});

#[cfg(test)]
mod tests {
    use dom::NodeId;

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document};

    const RAW_TEXT_TAG: &str = "raw-text";
    const TEXT_ATTRIBUTE: &str = "text";
    const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");

    fn text_element(document: &mut LynxDocument) -> NodeId {
        child(document, "text", "font-family: Ahem; font-size: 20px")
    }

    fn raw_text(document: &mut LynxDocument, parent: NodeId, text: &str) -> NodeId {
        let element = document.create_element(RAW_TEXT_TAG, ());
        document.set_attribute(element, TEXT_ATTRIBUTE, text);
        document.append_child(parent, element);
        element
    }

    #[test]
    fn generated_text_updates_without_mutating_the_carrier_children() {
        let mut document = document();
        document.register_fonts(dom::FontBlob::from_static(AHEM));
        let text = text_element(&mut document);
        let wrapper = document.create_element("wrapper", ());
        document.append_child(text, wrapper);
        let raw = raw_text(&mut document, wrapper, "hello");
        for (value, width) in [
            (Some("hello"), 100.0),
            (Some("hi"), 40.0),
            (Some(""), 0.0),
            (Some("again"), 100.0),
            (None, 0.0),
        ] {
            match value {
                Some(value) => document.set_attribute(raw, "text", value),
                None => document.remove_attribute(raw, "text"),
            }
            document.layout();
            let measured = document.text_block_size(text).expect("paragraph");
            assert!(
                (measured.width - width).abs() < f32::EPSILON,
                "{value:?}: {measured:?}"
            );
            assert!(document.get(raw).unwrap().child_ids().is_empty());
        }
        document.add_stylesheet(
            "raw-text { content: 'CSS'; }",
            dom::StylesheetOrigin::Author,
        );
        document.layout();
        assert!((document.text_block_size(text).unwrap().width - 60.0).abs() < f32::EPSILON);
        document.add_stylesheet("raw-text { content: none; }", dom::StylesheetOrigin::Author);
        document.layout();
        assert!(document.text_block_size(text).unwrap().width.abs() < f32::EPSILON);
    }

    #[test]
    fn text_attribute_replaces_children_and_removal_restores_them() {
        let mut document = document();
        document.register_fonts(dom::FontBlob::from_static(AHEM));
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "child");
        for (value, width) in [
            (None, 100.0),
            (Some("AB"), 40.0),
            (Some(""), 0.0),
            (None, 100.0),
        ] {
            match value {
                Some(value) => document.set_attribute(text, "text", value),
                None => document.remove_attribute(text, "text"),
            }
            document.layout();
            assert!((document.text_block_size(text).unwrap().width - width).abs() < f32::EPSILON);
            assert_eq!(document.get(text).unwrap().child_ids(), [raw]);
        }
    }

    #[test]
    fn the_ua_sheet_dissolves_a_raw_text_only_inside_the_text_it_is_written_in() {
        use dom::stylo::values::computed::Display;

        let mut document = document();
        let page = document.document_element().id();
        let text = text_element(&mut document);
        let direct = raw_text(&mut document, text, "direct");
        let wrapper = document.create_element("wrapper", ());
        document.append_child(text, wrapper);
        let inside = raw_text(&mut document, wrapper, "wrapped");
        let view = document.create_element("view", ());
        document.append_child(page, view);
        let stray = raw_text(&mut document, view, "stray");
        document.layout();

        assert_eq!(display(&document, text), Display::Flex);
        assert!(
            document.text_block_size(text).is_some(),
            "the text lays out as a Lynx paragraph by its kind"
        );
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, direct), Display::Contents);
        assert_eq!(display(&document, inside), Display::Contents);
        assert_eq!(
            display(&document, stray),
            Display::None,
            "a carrier written under a `view` renders nothing, as in both references"
        );
    }

    #[test]
    fn a_carrier_renders_under_a_tag_that_is_not_a_lynx_tag() {
        use dom::stylo::values::computed::Display;

        let mut document = document();
        let page = document.document_element().id();
        let under = |document: &mut LynxDocument, parents: &[&str]| {
            let mut parent = page;
            for tag in parents {
                let element = document.create_element(tag, ());
                document.append_child(parent, element);
                parent = element;
            }
            raw_text(document, parent, "text")
        };
        let cases = [
            (under(&mut document, &["div"]), Display::Contents, "div"),
            (under(&mut document, &["span"]), Display::Contents, "span"),
            (
                under(&mut document, &["my-box"]),
                Display::Contents,
                "any other name",
            ),
            (
                under(&mut document, &["div", "wrapper"]),
                Display::Contents,
                "one wrapper",
            ),
            (
                under(&mut document, &["div", "wrapper", "wrapper"]),
                Display::None,
                "two wrappers",
            ),
            (under(&mut document, &["view"]), Display::None, "view"),
            (
                under(&mut document, &["view", "wrapper"]),
                Display::None,
                "view > wrapper",
            ),
            (
                under(&mut document, &["x-swiper"]),
                Display::None,
                "a web-core element name",
            ),
            (
                under(&mut document, &["view", "div"]),
                Display::Contents,
                "a div inside a view",
            ),
            (
                under(&mut document, &["div", "view"]),
                Display::None,
                "a view inside a div",
            ),
        ];
        document.layout();
        for (carrier, expected, case) in cases {
            assert_eq!(display(&document, carrier), expected, "{case}");
        }
    }

    #[test]
    fn carriers_in_a_div_are_one_item_across_a_wrapper() {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let div = child(
            &mut document,
            "div",
            "font-family: Ahem; font-size: 20px; display: flex; align-self: flex-start",
        );
        raw_text(&mut document, div, "Hello ");
        let wrapper = document.create_element("wrapper", ());
        document.append_child(div, wrapper);
        raw_text(&mut document, wrapper, "Bob");
        let view = child(
            &mut document,
            "view",
            "font-family: Ahem; font-size: 20px; align-self: flex-start",
        );
        raw_text(&mut document, view, "hidden");
        document.layout();

        // One anonymous item holding "Hello Bob": nine 20px em squares on
        // one 20px line.
        let div_box = document.rounded_layout(div).expect("laid out");
        assert_eq!(
            (div_box.size.width, div_box.size.height),
            (180.0, 20.0),
            "{div_box:?}"
        );
        let view_box = document.rounded_layout(view).expect("laid out");
        assert!(
            view_box.size.width.abs() < f32::EPSILON,
            "a view renders no carrier: {view_box:?}"
        );
    }

    #[test]
    fn a_text_element_is_sized_by_the_run_its_raw_text_carries() {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let text = text_element(&mut document);
        let _raw = raw_text(&mut document, text, "hello");
        document.layout();

        // A text node generates no box: the paragraph belongs to the element
        // that establishes it, and its content size is what the run measures.
        let run = document
            .text_block_size(text)
            .expect("the text element established a paragraph");
        assert!(
            (run.width - 100.0).abs() < f32::EPSILON && (run.height - 20.0).abs() < f32::EPSILON,
            "five Ahem em squares at 20px, got {run:?}"
        );

        let box_ = document.rounded_layout(text).expect("the text is laid out");
        assert!(
            (box_.size.height - 20.0).abs() < f32::EPSILON,
            "the text element takes the run's height, got {:?}",
            box_.size
        );
    }

    #[test]
    fn a_literal_newline_in_the_attribute_breaks_the_line() {
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let text = text_element(&mut document);
        let raw = raw_text(&mut document, text, "ab\ncd");
        document.layout();

        let _ = raw;
        let run = document
            .text_block_size(text)
            .expect("the text element established a paragraph");
        assert!(
            (run.width - 40.0).abs() < f32::EPSILON && (run.height - 40.0).abs() < f32::EPSILON,
            "two lines of two em squares, got {run:?}"
        );
    }
}
