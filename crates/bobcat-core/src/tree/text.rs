//! The `text` tag's own defaults: what color a run wears, and what may
//! generate a box inside one.
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
/// `view` and `image` stay boxes — flex ones, not the `inline-flex` and
/// `contents` web-elements can hand a real inline formatting context, which
/// this engine does not have. `raw-text` opts back in from
/// [`super::raw_text::UA_RULES`], where the rest of a carrier's policy already
/// lives.
pub(super) const UA_RULES: &str = "\
text { box-sizing: border-box; display: flex; color: initial; }
text > * { display: none; }
text > wrapper { display: contents; }
text > view, text > image, text > text { display: flex; }
text > text, text > wrapper > text { color: inherit; }
";

#[cfg(test)]
mod tests {
    use dom::stylo::color::AbsoluteColor;
    use dom::stylo::values::computed::{ColorPropertyValue, Display};

    use crate::tree::test_support::{child, display, document, element_under, style_of};

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
        let boxes =
            ["view", "image", "text"].map(|tag| (tag, element_under(&mut document, text, tag, "")));
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "view", "");
        let foreign = element_under(&mut document, text, "x-foreign", "");
        let outside = child(&mut document, "x-foreign", "");
        document.layout();

        for (tag, content) in boxes {
            assert_eq!(display(&document, content), Display::Flex, "{tag}");
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
