//! `raw-text` is a CSS text carrier. Its attribute supplies generated content
//! to the enclosing paragraph; no custom element or DOM text child is needed.

/// Carriers dissolve only inside Lynx text. Their own style supplies the
/// newline policy, and author CSS may override or suppress their content.
pub(super) const UA_RULES: &str = "\
raw-text { display: none; white-space-collapse: preserve-breaks; }
text > raw-text, text > wrapper > raw-text,
inline-text > raw-text, inline-text > wrapper > raw-text { display: contents; }
raw-text { content: attr(text); }
";

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

        assert_eq!(display(&document, text), Display::LynxText);
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, direct), Display::Contents);
        assert_eq!(display(&document, inside), Display::Contents);
        assert_eq!(
            display(&document, stray),
            Display::None,
            "a carrier written outside a `text` renders nothing, as in web-core"
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
