//! The `text` tag's defaults: what color a run wears,
//! and what may generate a box inside one.
//!
//! The other half of Lynx text — how a run reaches the engine and where it
//! lays out — is [`super::raw_text`], which owns the `raw-text` component and
//! the rules that dissolve a carrier into the `text` it is written inside.

/// `text`'s own defaults, from `web-elements`' `x-text.css`.
///
/// `color: initial` is why Lynx text does not wear an ancestor's color. Native
/// Lynx runs with inheritance off; web-core runs on the browser's always-on
/// inheritance and buys the same result by resetting `color` at every text
/// root, which is the parity this engine committed to
/// (`docs/tracking/deviations.md`). A nested run opts back in with `inherit`,
/// and that carries a Lynx text gradient exactly as it carries a solid color —
/// the fork's `color` holds either, and the glyph painter reads whichever it
/// finds. The reset therefore also stops an ancestor's gradient at the text
/// root, where web-core stops it with the same declaration.
///
/// A `text` renders text: anything else written directly inside one generates
/// no box, and the tags that are content opt back in. `wrapper` dissolves,
/// `view` and `image` stay atomic boxes in the flattened paragraph.
/// `raw-text` opts back in from
/// [`super::raw_text::UA_RULES`], where the rest of a carrier's policy already
/// lives.
pub(super) const UA_RULES: &str = "\
text { box-sizing: border-box; display: -lynx-text !important; color: initial; }
inline-text { display: -lynx-text !important; }
inline-image, inline-truncation { display: none; }
text > * { display: none; }
text > wrapper { display: contents; }
text > view, text > image { display: flex; }
text > text, text > wrapper > text { color: inherit; }
";

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem and explicit line heights have exact metrics.

    use dom::stylo::color::AbsoluteColor;
    use dom::stylo::values::computed::{ColorPropertyValue, Display};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, style_of};

    const MAX_LINES: &str = "text-maxline";
    const MAX_CHARS: &str = "text-maxlength";

    fn append_run(document: &mut LynxDocument, parent: dom::NodeId, content: &str) {
        let raw = element_under(document, parent, "raw-text", "");
        document.set_attribute(raw, "text", content);
    }

    fn paragraph(content: &str) -> (LynxDocument, dom::NodeId) {
        const AHEM: &[u8] = include_bytes!("../../../../hughie/tests/fixtures/Ahem.ttf");
        let mut document = document();
        assert_eq!(document.register_fonts(dom::FontBlob::from_static(AHEM)), 1);
        let text = child(
            &mut document,
            "text",
            "width:100px;font-family:Ahem;font-size:20px;line-height:21px",
        );
        append_run(&mut document, text, content);
        (document, text)
    }

    fn assert_height(document: &mut LynxDocument, text: dom::NodeId, height: f32) {
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").height,
            height
        );
        assert_eq!(
            document.rounded_layout(text).expect("text box").size.height,
            height
        );
    }

    #[test]
    fn text_maxline_updates_and_removal_resize_text_and_following_content() {
        for content in ["abc def", "abc\ndef"] {
            let (mut document, text) = paragraph(content);
            let sibling = child(&mut document, "view", "width:10px;height:10px");
            assert_height(&mut document, text, 42.0);

            for (value, height) in [
                (Some("1"), 21.0),
                (Some("2"), 42.0),
                (Some("1"), 21.0),
                (None, 42.0),
            ] {
                if let Some(value) = value {
                    document.set_attribute(text, MAX_LINES, value);
                } else {
                    document.remove_attribute(text, MAX_LINES);
                }
                assert_height(&mut document, text, height);
                assert_eq!(
                    document
                        .rounded_layout(sibling)
                        .expect("following box")
                        .location
                        .y,
                    height,
                    "content after the paragraph follows its new height"
                );
            }
        }
    }

    #[test]
    fn text_limits_use_web_attribute_numbers_and_reset_invalid_values() {
        let (mut document, text) = paragraph("abc def");
        for (value, height) in [
            ("1", 21.0),
            ("0", 42.0),
            ("1.0", 21.0),
            ("-1", 42.0),
            ("1e0", 21.0),
            ("", 42.0),
            ("  +1px", 21.0),
            ("garbage", 42.0),
            ("1e+", 21.0),
            ("1.5", 42.0),
            ("\u{feff}1", 21.0),
            ("Infinity", 42.0),
            ("1", 21.0),
            ("4294967296", 42.0),
        ] {
            document.set_attribute(text, MAX_LINES, value);
            assert_height(&mut document, text, height);
        }
        // Both attributes use the same numeric reader, but maxlength permits
        // zero and truncates fractional character offsets like DOM Range.
        for (value, width) in [("1.9", 20.0), ("0", 0.0), ("-1", 60.0)] {
            document.set_attribute(text, MAX_CHARS, value);
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    #[test]
    fn text_limits_belong_to_the_whole_paragraph_across_nested_runs() {
        let (mut document, text) = paragraph("abc ");
        let nested = element_under(&mut document, text, "text", "");
        append_run(&mut document, nested, "def");
        document.set_attribute(nested, MAX_CHARS, "0");
        document.set_attribute(nested, MAX_LINES, "1");
        assert_height(&mut document, text, 42.0);

        document.set_attribute(text, MAX_LINES, "1");
        assert_height(&mut document, text, 21.0);
        document.set_attribute(text, MAX_CHARS, "2");
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").width,
            40.0
        );

        document.remove_attribute(text, MAX_LINES);
        assert_height(&mut document, text, 21.0);
        document.remove_attribute(text, MAX_CHARS);
        assert_height(&mut document, text, 42.0);
    }

    #[test]
    fn text_overflow_selects_the_existing_ellipsis_path() {
        let (mut document, text) = paragraph("abc def");
        document.set_attribute(text, MAX_CHARS, "1");
        for (overflow, width) in [("clip", 20.0), ("ellipsis", 80.0), ("clip", 20.0)] {
            document.set_inline_style_property(text, "text-overflow", overflow);
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    #[test]
    fn a_text_does_not_wear_an_ancestor_s_color() {
        let mut document = document();
        let view = child(&mut document, "view", "color: rgb(1, 2, 3)");
        let text = element_under(&mut document, view, "text", "");
        let nested = element_under(&mut document, view, "view", "");
        document.layout();

        assert_eq!(
            style_of(&document, nested).clone_color(),
            style_of(&document, view).clone_color(),
            "a view still inherits color: only text is reset"
        );
        assert_eq!(
            style_of(&document, text).clone_color(),
            AbsoluteColor::BLACK,
            "`color: initial` stops the cascade at the text root"
        );
    }

    #[test]
    fn a_nested_text_wears_its_parent_text_s_color() {
        let mut document = document();
        let text = child(&mut document, "text", "color: rgb(1, 2, 3)");
        let direct = element_under(&mut document, text, "text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "text", "");
        document.layout();

        let parent = style_of(&document, text).clone_color();
        assert_ne!(parent, AbsoluteColor::BLACK);
        for nested in [direct, through_wrapper] {
            assert_eq!(style_of(&document, nested).clone_color(), parent);
        }
    }

    /// A Lynx `color` holds a gradient as readily as a solid, and the glyph
    /// painter reads that whole value — so the reset and the opt-in have to
    /// carry the gradient too, not just the solid it collapses to.
    #[test]
    fn the_color_reset_and_its_opt_in_carry_a_text_gradient() {
        const GRADIENT: &str = "color: linear-gradient(90deg, rgb(1, 2, 3), rgb(4, 5, 6))";

        let mut document = document();
        let painted = child(&mut document, "text", GRADIENT);
        let nested = element_under(&mut document, painted, "text", "");
        let view = child(&mut document, "view", GRADIENT);
        let under_view = element_under(&mut document, view, "text", "");
        document.layout();

        for gradient in [painted, nested] {
            assert!(
                matches!(
                    style_of(&document, gradient).clone_color_value(),
                    ColorPropertyValue::Gradient(_)
                ),
                "`inherit` hands a nested run the gradient, not the black it collapses to"
            );
        }
        assert!(
            matches!(
                style_of(&document, under_view).clone_color_value(),
                ColorPropertyValue::Color(_)
            ),
            "`color: initial` stops an ancestor's gradient at the text root"
        );
    }

    #[test]
    fn a_text_renders_text_and_the_tags_that_are_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let boxes = ["view", "image"].map(|tag| (tag, element_under(&mut document, text, tag, "")));
        let nested = element_under(&mut document, text, "text", "");
        let nested_inline = element_under(&mut document, text, "inline-text", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "view", "");
        let foreign = element_under(&mut document, text, "x-foreign", "");
        let outside = child(&mut document, "x-foreign", "");
        document.layout();

        for (tag, content) in boxes {
            assert_eq!(display(&document, content), Display::Flex, "{tag}");
        }
        for (tag, scope) in [("text", nested), ("inline-text", nested_inline)] {
            assert_eq!(
                display(&document, scope),
                Display::LynxText,
                "a nested text scope is part of the paragraph, not a box in it: {tag}"
            );
        }
        assert_eq!(display(&document, wrapper), Display::Contents);
        assert_eq!(display(&document, through_wrapper), Display::Linear);
        assert_eq!(
            display(&document, foreign),
            Display::None,
            "anything else written inside a text generates no box"
        );
        assert_eq!(
            display(&document, outside),
            Display::Flex,
            "and the suppression reaches no further than a text's own children"
        );
    }
}
