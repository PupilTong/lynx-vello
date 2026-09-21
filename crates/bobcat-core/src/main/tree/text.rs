//! The `text` tag's attribute-to-CSS limits and defaults: what color a run wears,
//! and what may generate a box inside one.
//!
//! The other half of Lynx text — how a run reaches the engine and where it
//! lays out — is [`super::raw_text`], which owns the `raw-text` generated-content rule and
//! the rules that dissolve a carrier into the `text` it is written inside.

use dom::{CustomElement, NodeId};

use super::{LynxDocument, parse_count};

/// The tag a paragraph limit can be written on, and the only one.
///
/// web-core mixes `XTextTruncation` — the sole reader of these three
/// attributes — into `x-text` alone (`XText.ts:16-24`); `inline-text` and
/// `inline-truncation` are declared without it
/// (`XText/InlineText.ts:12`, `XText/InlineTruncation.ts`), and this engine
/// agrees for a reason of its own: a limit is a property of the block that
/// establishes the paragraph, so a nested run's limit is ignored whatever
/// tag the run is written as
/// (`a_maxline_on_a_nested_inline_run_neither_clips_nor_re_boxes_it`, in
/// `web_text_replication.rs`).
const TEXT_TAG: &str = "text";

const MAX_LINE_ATTRIBUTE: &str = "text-maxline";
const MAX_LENGTH_ATTRIBUTE: &str = "text-maxlength";
const TAIL_COLOR_CONVERT_ATTRIBUTE: &str = "tail-color-convert";

const MAX_LINE_PROPERTY: &str = "--lynx-text-maxline";
const MAX_LENGTH_PROPERTY: &str = "--lynx-text-maxlength";
const TAIL_COLOR_CONVERT_PROPERTY: &str = "--lynx-tail-color-convert";

/// Installs the component. Must run before any element could carry the tag,
/// which is [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(TEXT_TAG, Box::new(Text));
}

/// Reflects a paragraph's three limit attributes into the registered custom
/// properties [`UA_RULES`] declares and the text block reads.
///
/// `attribute_changed_callback` is the whole component, for the same reason it
/// is the whole of [`super::image`]'s and [`super::blur_view`]'s:
/// `__CreateElement` mints the element before `__SetAttribute` writes on it,
/// so `constructed` could observe nothing, and a removal frees nothing a
/// disconnect would have to undo.
///
/// A hint lands at `CascadeOrigin::PresHints`, below author CSS and above
/// [`UA_RULES`], so an author declaration overrides the attribute without
/// replacing the attribute's own value.
struct Text;

impl CustomElement<()> for Text {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            MAX_LINE_ATTRIBUTE.to_owned(),
            MAX_LENGTH_ATTRIBUTE.to_owned(),
            TAIL_COLOR_CONVERT_ATTRIBUTE.to_owned(),
        ]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        // Every arm names a value, and the empty string is how one says "no
        // usable value": `set_presentational_hint` has `setProperty`
        // semantics, so it clears the hint rather than leaving a stale one
        // behind. No arm can produce a declaration the cascade rejects — the
        // two counts reflect as bare integers and the boolean as `1` — so
        // none of them needs [`super::blur_view`]'s clear-then-set.
        let (property, css) = match name {
            MAX_LINE_ATTRIBUTE => (
                MAX_LINE_PROPERTY,
                count_css(parse_count(new).filter(|count| *count > 0.0 && count.fract() == 0.0)),
            ),
            MAX_LENGTH_ATTRIBUTE => (MAX_LENGTH_PROPERTY, count_css(parse_count(new))),
            // Native reads this one as a BOOL rather than a number
            // (`LynxTextRenderer.m overrideTruncatedAttrIfNeed`, Android
            // `TextRenderer.convertTailColor`), and its default is off, so
            // only the literal `true` turns it on — which is what ReactLynx's
            // `tail-color-convert={true}` reaches the DOM as. Any other value,
            // including a removal, writes the empty string and resets the
            // hint.
            TAIL_COLOR_CONVERT_ATTRIBUTE => (
                TAIL_COLOR_CONVERT_PROPERTY,
                if new == Some("true") {
                    "1".to_owned()
                } else {
                    String::new()
                },
            ),
            other => {
                debug_assert!(false, "`text` does not observe `{other}`");
                return;
            }
        };
        document.set_presentational_hint(element, property, &css);
    }
}

/// The canonical integer a parsed count reflects as; an absent count resets
/// the hint.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "parse_count bounds values to u32; character offsets truncate like DOM Range"
)]
fn count_css(count: Option<f64>) -> String {
    count.map_or_else(String::new, |count| (count as u32).to_string())
}

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
/// Attribute parsing supplies canonical counts; the registered integer syntax
/// validates CSS overrides, and each paragraph receives its own limits.
/// `--lynx-tail-color-convert` carries the `tail-color-convert` boolean the
/// same way: zero, its initial value, leaves the truncation marker in the
/// colour of the run the cut landed in, which is native Lynx's default.
///
/// `text-overflow` is an attribute as well as a property on native Lynx —
/// `TextElement::ProcessAttributeForNormalLayoutMode` caches the attribute onto
/// `kPropertyIDTextOverflow` (`core/renderer/dom/fiber/text_element.cc:176-182`)
/// — and web-core observes it nowhere, so a 2026-09-16 ruling follows native
/// here (`docs/tracking/deviations.md`). It is a selector rule rather than a
/// presentational hint because it needs no parsing of its own: native's enum
/// reader accepts the two literals and nothing else, so two UA rules keyed on
/// those literals say the whole thing. Plain declarations, so author CSS still
/// outranks the attribute the way it outranks every other UA rule.
///
/// An `inline-truncation` is content wherever it is written *directly* inside
/// a `text`, and generates no box anywhere else — the same scope web-core's
/// `:scope > inline-truncation` query honours. It carries
/// `--lynx-inline-truncation` because `crates/dom` names no Lynx tag: which
/// subtree a paragraph takes its custom truncation content from is a
/// computed-style fact there, exactly as the paragraph limits are.
///
/// # Why an inline `image` has no padding
///
/// `padding: 0 !important` on an `image` that is inline content of a paragraph
/// is the one `!important` this sheet spends outside `display`, and it buys
/// web-core's geometry rather than a default. There, the authored host element
/// has no box
/// at all: `x-text > x-image` — and the `inline-text >`, `inline-truncation >`
/// and `lynx-wrapper` variants of it — is `display: contents !important`
/// (`x-text.css:69-82`), so the box on the line is the shadow `::part(img)`,
/// which is built by inheriting a fixed list: `width`, `height`, `border`,
/// `border-radius`, `background-color`, `vertical-align`, `object-fit`,
/// `flex`, `align-self` and `margin` (`x-text.css:120-135`). `padding` is not
/// on that list, and the one `padding: inherit` in web-elements belongs to
/// `x-image[auto-size]::part(img)`, an attribute this engine does not
/// implement. An author's `padding` on an inline image therefore reaches no
/// box in the reference and must reach none here.
///
/// Native is split. `TextLayoutTextra::HandleInlineImageProps`
/// (`core/renderer/ui_wrapper/layout/textra/text_layout_textra.cc:448-537`)
/// fills the placeholder's `ImageProps` from the specified width and height,
/// the four margins, the border radius and `vertical-align`, and never reads
/// `padding`; Android's `InlineImageSpan` and iOS's default shadow-node path
/// read the same values and no padding either. Harmony, and iOS's
/// layout-in-element path, instead measure the image as a starlight leaf, whose
/// `ClampExactWidth` floors the border box at padding plus border
/// (`core/renderer/starlight/layout/layout_object.cc:515-519`). The rule here
/// follows web-core, which the larger part of native matches.
///
/// Here the authored `image` *is* the box, so nothing removes its padding for
/// free: under the Lynx `box-sizing: border-box` default a
/// `width: 22px; padding-left: 50px` image would floor its border box at 50 and
/// advance the line by 50 where web-core advances by 22. A normal UA
/// declaration cannot say so — it loses to the author's own `padding-left` —
/// which is what makes this the same argument the `display` exceptions carry:
/// web-core's erasure of the host box is itself `!important` and no author CSS
/// can undo it. `margin` is a different case and is *not* touched: the shadow
/// part inherits it, so it reaches the line in both engines. Nor is a `view`,
/// which web-core leaves as a real `inline-flex` box with its padding intact.
pub(super) const UA_RULES: &str = r#"
@property --lynx-text-maxline { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-text-maxlength { syntax: "<integer>"; inherits: false; initial-value: -1; }
@property --lynx-tail-color-convert { syntax: "<integer>"; inherits: false; initial-value: 0; }
@property --lynx-inline-truncation { syntax: "<integer>"; inherits: false; initial-value: 0; }
text { display: -lynx-text !important; color: initial; }
text[text] { content: attr(text); }
text[text-overflow="ellipsis"] { text-overflow: ellipsis; }
text[text-overflow="clip"] { text-overflow: clip; }
inline-text { display: -lynx-text !important; }
inline-image, inline-truncation { display: none; }
text > * { display: none; }
text > wrapper { display: contents; }
text > view, text > image { display: flex; }
text > inline-truncation { display: -lynx-text !important; --lynx-inline-truncation: 1; }
text > text, text > wrapper > text { color: inherit; }
text > image, text > wrapper > image, inline-text > image, inline-text > wrapper > image, text > inline-truncation > image, text > inline-truncation > wrapper > image { padding: 0 !important; }
"#;

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // Ahem and explicit line heights have exact metrics.

    use dom::CustomElement;
    use dom::stylo::color::AbsoluteColor;
    use dom::stylo::values::computed::{ColorPropertyValue, Display};

    use super::super::LynxDocument;
    use super::super::test_support::{child, display, document, element_under, style_of};
    use super::Text;

    const MAX_LINES: &str = "text-maxline";
    const MAX_CHARS: &str = "text-maxlength";
    const TAIL_COLOR_CONVERT: &str = "tail-color-convert";
    const TEXT_OVERFLOW: &str = "text-overflow";

    /// Writes or removes an attribute the way the runtime does — the DOM
    /// mutation and nothing else. An observed name raises the component's
    /// reaction from inside the mutation, which is what writes the hint.
    fn set_limit(
        document: &mut LynxDocument,
        element: dom::NodeId,
        name: &str,
        value: Option<&str>,
    ) {
        if let Some(value) = value {
            document.set_attribute(element, name, value);
        } else {
            document.remove_attribute(element, name);
        }
    }

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
                set_limit(&mut document, text, MAX_LINES, value);
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
            set_limit(&mut document, text, MAX_LINES, Some(value));
            assert_height(&mut document, text, height);
        }
        // Both attributes use the same numeric reader, but maxlength permits
        // zero and truncates fractional character offsets like DOM Range.
        for (value, width) in [("1.9", 20.0), ("0", 0.0), ("-1", 60.0)] {
            set_limit(&mut document, text, MAX_CHARS, Some(value));
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
        set_limit(&mut document, nested, MAX_CHARS, Some("0"));
        set_limit(&mut document, nested, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 42.0);

        set_limit(&mut document, text, MAX_LINES, Some("1"));
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, Some("2"));
        document.layout();
        assert_eq!(
            document.text_block_size(text).expect("paragraph").width,
            40.0
        );

        set_limit(&mut document, text, MAX_LINES, None);
        assert_height(&mut document, text, 21.0);
        set_limit(&mut document, text, MAX_CHARS, None);
        assert_height(&mut document, text, 42.0);
    }

    /// Two independent reasons a limit written above a paragraph leaves it
    /// alone: the attribute reaches no component on a tag that is not a
    /// `text`, and the property it *would* have written is registered
    /// `inherits: false`, so even a declared one stops at the element that
    /// declared it.
    #[test]
    fn registered_text_limits_do_not_inherit_from_the_parent() {
        let (mut document, text) = paragraph("abc def");
        let page = document.document_element().id();
        set_limit(&mut document, page, MAX_LINES, Some("1"));
        set_limit(&mut document, page, MAX_CHARS, Some("0"));
        assert_height(&mut document, text, 42.0);
        assert_eq!(document.text_block_size(text).unwrap().width, 60.0);
        assert_eq!(
            hint(&document, page, "lynx-text-maxline", "0"),
            "0",
            "`page` is not a `text`, so the attribute names no hint at all",
        );

        document.set_inline_style_property(page, "--lynx-text-maxline", "1");
        document.set_inline_style_property(page, "--lynx-text-maxlength", "0");
        assert_height(&mut document, text, 42.0);
        assert_eq!(
            document.text_block_size(text).unwrap().width,
            60.0,
            "and a declared limit is non-inheriting, so it stops at the page",
        );
    }

    /// The component's contract, which the tests above exercise through
    /// layout: these three names and no others, on this tag and no other.
    #[test]
    fn a_text_observes_its_three_limit_attributes_and_nothing_else() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        assert_eq!(
            CustomElement::<()>::observed_attributes(&Text),
            vec![
                MAX_LINES.to_owned(),
                MAX_CHARS.to_owned(),
                TAIL_COLOR_CONVERT.to_owned(),
            ],
        );

        // `text-overflow` is a UA selector rule rather than a hint, and
        // `ellipsize-mode` is inert: neither may reach a declaration.
        for name in [TEXT_OVERFLOW, "ellipsize-mode", "text-maxline-x"] {
            set_limit(&mut document, text, name, Some("1"));
        }
        // And the same three names on a tag that does not observe them.
        let view = child(&mut document, "view", "");
        for name in [MAX_LINES, MAX_CHARS, TAIL_COLOR_CONVERT] {
            set_limit(&mut document, view, name, Some("1"));
        }
        document.layout();

        for (property, initial) in [
            ("lynx-text-maxline", "0"),
            ("lynx-text-maxlength", "-1"),
            ("lynx-tail-color-convert", "0"),
        ] {
            assert_eq!(
                hint(&document, text, property, initial),
                initial,
                "an unobserved attribute writes no hint: {property}",
            );
            assert_eq!(
                hint(&document, view, property, initial),
                initial,
                "a paragraph limit means nothing on a tag that is not a \
                 `text`: {property}",
            );
        }
    }

    #[test]
    fn inline_css_can_override_a_reflected_text_limit() {
        for (attribute, property, value, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", "2", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "calc(2 - 1)", 60.0, 21.0),
            (MAX_LINES, "--lynx-text-maxline", "1.5", 60.0, 42.0),
            (MAX_LINES, "--lynx-text-maxline", "1px", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "2", 40.0, 21.0),
            (
                MAX_CHARS,
                "--lynx-text-maxlength",
                "calc(1 + 1)",
                40.0,
                21.0,
            ),
            (MAX_CHARS, "--lynx-text-maxlength", "1.5", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", "1px", 60.0, 42.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.set_inline_style_property(text, property, value);
            assert_height(&mut document, text, height);
            assert_eq!(document.text_block_size(text).unwrap().width, width);
            assert_eq!(document.get(text).unwrap().attribute(attribute), Some("1"));
        }
    }

    #[test]
    fn author_css_overrides_text_attributes_and_removing_it_restores_the_hint() {
        for (attribute, property, width, height) in [
            (MAX_LINES, "--lynx-text-maxline", 60.0, 42.0),
            (MAX_CHARS, "--lynx-text-maxlength", 40.0, 21.0),
        ] {
            let (mut document, text) = paragraph("abc def");
            set_limit(&mut document, text, attribute, Some("1"));
            document.add_class(text, "override");
            document.add_stylesheet(
                &format!("@layer limits {{ .override {{ {property}: 2; }} }}"),
                dom::StylesheetOrigin::Author,
            );
            for value in [Some("1"), Some("3"), None, Some("1")] {
                set_limit(&mut document, text, attribute, value);
                assert_height(&mut document, text, height);
                assert_eq!(document.text_block_size(text).unwrap().width, width);
            }
            document.remove_class(text, "override");
            assert_height(&mut document, text, 21.0);
            assert_eq!(
                document.text_block_size(text).unwrap().width,
                if attribute == MAX_LINES { 60.0 } else { 20.0 }
            );
        }
    }

    #[test]
    fn reflected_limits_preserve_attribute_selector_values() {
        for (attribute, first, equivalent) in [(MAX_LINES, "1", "1.0"), (MAX_CHARS, "2", "2.9")] {
            let (mut document, text) = paragraph("abc def");
            document.add_stylesheet(
                &format!("[{attribute}=\"{equivalent}\"] {{ padding-top: 7px; }}"),
                dom::StylesheetOrigin::Author,
            );
            set_limit(&mut document, text, attribute, Some(first));
            document.layout();
            let before = document.rounded_layout(text).unwrap().size.height;
            set_limit(&mut document, text, attribute, Some(equivalent));
            document.layout();
            assert_eq!(
                document.rounded_layout(text).unwrap().size.height,
                before + 7.0
            );
            assert_eq!(
                document.get(text).unwrap().attribute(attribute),
                Some(equivalent)
            );
        }
    }

    /// The computed value of the registered property `name`, as CSS text. An
    /// unset property reports its registered initial value, which is what an
    /// empty hint value leaves the paragraph reading.
    fn hint(document: &LynxDocument, element: dom::NodeId, name: &str, initial: &str) -> String {
        use dom::stylo::custom_properties::Name;

        style_of(document, element)
            .custom_properties()
            .non_inherited
            .get(&Name::from(name))
            .map_or_else(|| initial.to_owned(), |value| value.to_variable_value().css)
    }

    /// `tail-color-convert` is a native BOOL whose default is off, so only the
    /// literal `true` — what `ReactLynx`'s `tail-color-convert={true}` reaches
    /// the DOM as — turns the hint on. Anything else, a removal included,
    /// writes the empty value that resets it to the registered initial 0.
    #[test]
    fn tail_color_convert_reflects_only_the_literal_true() {
        let (mut document, text) = paragraph("abc def");
        for (value, expected) in [
            (Some("true"), "1"),
            (Some("false"), "0"),
            (Some("TRUE"), "0"),
            (Some("1"), "0"),
            (Some(""), "0"),
            (Some("true"), "1"),
            (None, "0"),
        ] {
            set_limit(&mut document, text, TAIL_COLOR_CONVERT, value);
            document.layout();
            assert_eq!(
                hint(&document, text, "lynx-tail-color-convert", "0"),
                expected,
                "tail-color-convert={value:?}",
            );
            assert_eq!(
                document.get(text).unwrap().attribute(TAIL_COLOR_CONVERT),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    #[test]
    fn text_overflow_selects_the_existing_ellipsis_path() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (overflow, width) in [("clip", 20.0), ("ellipsis", 80.0), ("clip", 20.0)] {
            document.set_inline_style_property(text, "text-overflow", overflow);
            document.layout();
            assert_eq!(
                document.text_block_size(text).expect("paragraph").width,
                width
            );
        }
    }

    /// The width of the paragraph `text` establishes, after a layout.
    fn paragraph_width(document: &mut LynxDocument, text: dom::NodeId) -> f32 {
        document.layout();
        document.text_block_size(text).expect("paragraph").width
    }

    /// Native Lynx reads `text-overflow` off the element as well as out of
    /// CSS (`text_element.cc:176-182`), and the UA sheet's two attribute rules
    /// are how this engine honours that. Native's enum reader accepts the two
    /// literals only, so every other value — a removal, a different case, an
    /// empty string — leaves the initial `clip`.
    #[test]
    fn text_overflow_attribute_selects_the_ellipsis_path_through_the_ua_sheet() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        for (value, width) in [
            (Some("ellipsis"), 80.0),
            (Some("clip"), 20.0),
            (Some("ellipsis"), 80.0),
            (None, 20.0),
            (Some("ELLIPSIS"), 20.0),
            (Some("garbage"), 20.0),
            (Some(""), 20.0),
            (Some("ellipsis"), 80.0),
        ] {
            set_limit(&mut document, text, TEXT_OVERFLOW, value);
            assert_eq!(
                paragraph_width(&mut document, text),
                width,
                "text-overflow={value:?}",
            );
            assert_eq!(
                document.get(text).unwrap().attribute(TEXT_OVERFLOW),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    /// The attribute's rules are UA origin and plain, so a page's own CSS
    /// overrides them exactly as it overrides the rest of the UA sheet.
    #[test]
    fn author_css_outranks_the_text_overflow_attribute() {
        let (mut document, text) = paragraph("abc def");
        set_limit(&mut document, text, MAX_CHARS, Some("1"));
        set_limit(&mut document, text, TEXT_OVERFLOW, Some("ellipsis"));
        document.add_class(text, "override");
        document.add_stylesheet(
            "@layer limits { .override { text-overflow: clip; } }",
            dom::StylesheetOrigin::Author,
        );
        assert_eq!(paragraph_width(&mut document, text), 20.0);
        assert_eq!(
            document.get(text).unwrap().attribute(TEXT_OVERFLOW),
            Some("ellipsis"),
            "the author declaration wins the cascade without touching the attribute",
        );

        document.remove_class(text, "override");
        assert_eq!(paragraph_width(&mut document, text), 80.0);
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

    /// Custom truncation content is content only where web-core looks for it:
    /// `XTextTruncation.ts` queries `:scope > inline-truncation`, so a marker
    /// written directly inside a `text` becomes a text scope and one written
    /// anywhere else — through a wrapper, or outside a paragraph entirely —
    /// keeps the `display: none` the tag carries by default.
    #[test]
    fn an_inline_truncation_is_content_only_as_a_text_s_own_child() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let marker = element_under(&mut document, text, "inline-truncation", "");
        let second = element_under(&mut document, text, "inline-truncation", "");
        let wrapper = element_under(&mut document, text, "wrapper", "");
        let through_wrapper = element_under(&mut document, wrapper, "inline-truncation", "");
        let outside = child(&mut document, "inline-truncation", "");
        document.layout();

        for (label, scope) in [("first", marker), ("second", second)] {
            assert_eq!(
                display(&document, scope),
                Display::LynxText,
                "a text's own inline-truncation child is a text scope: {label}",
            );
            assert_eq!(
                hint(&document, scope, "lynx-inline-truncation", "0"),
                "1",
                "and says so in computed style, which is how the layout host \
                 finds it without naming the tag: {label}",
            );
        }
        for (label, elsewhere) in [("under a wrapper", through_wrapper), ("outside", outside)] {
            assert_eq!(
                display(&document, elsewhere),
                Display::None,
                "and nowhere else: {label}",
            );
        }
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
