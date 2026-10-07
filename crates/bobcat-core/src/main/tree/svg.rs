//! The `svg` component: Lynx's vector picture tag, drawn as one image.
//!
//! Lynx never treats an SVG document as a subtree. Native's `<svg>` takes a
//! URL in `src` or raw markup in `content` and renders the result at the
//! element's layout size; web-core's `x-svg` is a shadow `<img>` whose `src`
//! is the URL or a `Blob` URL of the markup. This module reflects both
//! attributes into the one [`dom::Document::set_image_source`] that
//! [`super::image`] uses, under [`dom::ImageRole::Source`], so the element is
//! replaced content and the image pipeline — fetch, `data:` parsing, the
//! vector decode, the natural size — is shared with `<image>`.
//!
//! # `src` and `content`
//!
//! `src` is relayed as written. `content` becomes
//! `data:image/svg+xml;charset=utf-8,` followed by the markup with every byte
//! outside RFC 3986's unreserved set (`A-Z a-z 0-9 - . _ ~`) percent-encoded:
//! the `data:` parser stops at the first `#`, and a bare `%` would be read as
//! an escape, so leaving any reserved byte as it is would change what the
//! URL names. The data URL is the equivalent of web-core's `Blob` URL.
//!
//! The last attribute written wins: both callbacks write the same source, as
//! in web-core, where both end up assigning the shadow `<img>`'s `src`. An
//! empty value or a removal of `src` means no source, as in
//! [`super::image`]. An empty value or a removal of `content` leaves the
//! element's current source as it is, whichever attribute set it: this is a
//! web-core-vs-native difference, ruled for web-core, whose `_handleContent`
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XSvg/XSvg.ts`)
//! revokes its `Blob` URL on a `null` or empty `content` and does not touch
//! the shadow `<img>`'s `src`.
//!
//! Cost: the full data URL is the key in `dom`'s image registry and in the
//! host's resource entries, and neither evicts, so every distinct `content`
//! an element is given stays resident for the document's life.
//!
//! No `placeholder`, `mode` or `blur-radius`: neither reference has them on
//! `<svg>`.
//!
//! # The box
//!
//! [`UA_RULES`] mirror `web-elements`' `x-svg.css` (`x-svg { contain:
//! content; display: flex; }` around a shadow `img` that inherits the host's
//! width and height). Unlike `<image>`, there is no `contain: size`: an
//! `<svg>` with no CSS size lays out at its natural size, and a sized one
//! keeps its CSS size with the picture stretched to it (`object-fit: fill`,
//! the initial value). web-core's `contain: content` is not adopted: the
//! element is a replaced leaf here and has nothing to contain.
//!
//! `svg > * { display: none; }` has the same ordering constraint as
//! `image > *` ([`super::image`]'s module docs): it ties on specificity with
//! the container tags' `display` rules and wins on source order, so
//! [`super::ua_sheet`] assembles it after them.
//! `nothing_inside_an_svg_generates_a_box` is the check.
//!
//! # Events
//!
//! `load` only: the Lynx `<svg>` typing and web-core's `x-svg` both expose
//! `bindload` alone, so a source that fails dispatches nothing. A settled
//! `src` at the bind is queued on [`ComponentEvents`] exactly as
//! [`super::image`] queues it. The detail differs from `<image>`'s and is the
//! runtime's to build: native reports the element's layout size, not the
//! document's intrinsic size, so the runtime reads the border box at
//! delivery ([`is_svg`] is how it tells the two apart).

use dom::{CustomElement, ImageRole, NodeId};

use super::{ComponentEvents, LynxDocument};

/// Lynx's vector picture tag, and the two attributes this module reflects.
const SVG_TAG: &str = "svg";
const SRC_ATTRIBUTE: &str = "src";
const CONTENT_ATTRIBUTE: &str = "content";

/// What `content` is prefixed with to become a source.
const CONTENT_URL_PREFIX: &str = "data:image/svg+xml;charset=utf-8,";

/// The box an `svg` draws into, and the suppression of its children.
///
/// The border box and the rest of `web-elements`' common block come from
/// [`super::ua_sheet`], which lists `svg` beside `image`. `display: flex`
/// is `x-svg.css`'s, and keeps the tag out of the `defaultDisplayLinear`
/// list as `image` is. Nothing here is `!important`: see
/// `the_ua_sheet_is_important_free_apart_from_the_text_block`.
pub(super) const UA_RULES: &str = r"svg { display: flex; }
svg > * { display: none; }
";

/// Installs the component, over the queue its bind-time outcomes go into.
/// Must run before any element could carry the tag.
pub(super) fn define(document: &mut LynxDocument, outcomes: ComponentEvents) {
    document.define(SVG_TAG, Box::new(Svg { outcomes }));
}

/// Whether `node` is a live `svg` element: the runtime builds an `svg`
/// node's `load` detail from its layout box and dispatches no `error` for it.
pub(crate) fn is_svg(document: &LynxDocument, node: NodeId) -> bool {
    document
        .get(node)
        .and_then(dom::Node::tag_name)
        .is_some_and(|tag| tag == SVG_TAG)
}

/// Points the element at whatever its last written attribute names.
///
/// Only `attribute_changed_callback` is implemented, for the reasons
/// [`super::image`]'s component gives: the reaction that carries a source
/// always follows the element's creation, and a removal frees nothing.
struct Svg {
    /// Where a source that settles at the bind leaves its outcome. The
    /// reaction runs inside a JavaScript call, so it cannot dispatch.
    outcomes: ComponentEvents,
}

impl CustomElement<()> for Svg {
    fn observed_attributes(&self) -> Vec<String> {
        vec![SRC_ATTRIBUTE.to_owned(), CONTENT_ATTRIBUTE.to_owned()]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        let new = new.filter(|value| !value.is_empty());
        let outcome = match name {
            SRC_ATTRIBUTE => document.set_image_source(element, ImageRole::Source, new),
            // No `content` keeps the current source (web-core's
            // `_handleContent`; see the module docs).
            CONTENT_ATTRIBUTE => new.and_then(|content| {
                document.set_image_source(element, ImageRole::Source, Some(&content_url(content)))
            }),
            other => {
                debug_assert!(false, "`svg` does not observe `{other}`");
                None
            }
        };
        self.outcomes.queue_image(outcome);
    }
}

/// The `data:` URL that names `content` as an SVG document.
fn content_url(content: &str) -> String {
    let mut url = String::with_capacity(CONTENT_URL_PREFIX.len() + content.len() * 3);
    url.push_str(CONTENT_URL_PREFIX);
    for byte in content.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            url.push(char::from(byte));
        } else {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            url.push('%');
            url.push(char::from(HEX[usize::from(byte >> 4)]));
            url.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    // Every size asserted here is an authored pixel count or a natural size
    // set by hand, so no tolerance.
    #![allow(clippy::float_cmp)]

    use std::sync::Arc;

    use dom::stylo::computed_values::box_sizing;
    use dom::stylo::values::computed::{Contain, Display};
    use dom::{ImageEvent, ImageOutcome, NodeId};

    use super::super::test_support::{
        child, display, document, element_under, style_of, with_component_events, with_config,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{CONTENT_ATTRIBUTE, SRC_ATTRIBUTE, SVG_TAG, content_url, is_svg};

    const SOURCE: &str = "app:///a.svg";
    const OTHER_SOURCE: &str = "app:///b.svg";

    fn svg(document: &mut LynxDocument, style: &str) -> NodeId {
        child(document, SVG_TAG, style)
    }

    fn loaded(source: &str) -> ImageEvent {
        ImageEvent::Loaded {
            source: Arc::from(source),
            width: 30,
            height: 15,
        }
    }

    /// The outcomes queued at a bind, as the runtime would drain them.
    fn queued(events: &ComponentEvents) -> Vec<ImageOutcome> {
        events
            .take()
            .into_iter()
            .map(|event| match event {
                ComponentEvent::Image(outcome) => outcome,
                ComponentEvent::Plain { name, .. } => panic!("no {name} here"),
            })
            .collect()
    }

    fn size_of(document: &mut LynxDocument, element: NodeId) -> (f32, f32) {
        document.layout();
        let layout = document
            .rounded_layout(element)
            .expect("the svg is laid out");
        (layout.size.width, layout.size.height)
    }

    #[test]
    fn the_ua_sheet_gives_an_svg_an_uncontained_flex_border_box() {
        for linear in [true, false] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let element = svg(&mut document, "");
            document.layout();

            let style = style_of(&document, element);
            assert_eq!(*style.get_box_sizing(), box_sizing::T::BorderBox);
            assert_eq!(
                *style.get_display(),
                Display::Flex,
                "x-svg.css's display, outside the defaultDisplayLinear list: linear={linear}"
            );
            assert_eq!(
                *style.get_contain(),
                Contain::empty(),
                "no size containment: an unsized svg takes its natural size"
            );
        }
    }

    /// The reverse of `<image>`'s rule: an unsized `<svg>` takes its natural
    /// size, and a sized one keeps its CSS size.
    #[test]
    fn an_unsized_svg_lays_out_at_its_natural_size_and_a_sized_one_at_its_css_size() {
        let mut document = document();
        let holder = child(
            &mut document,
            "view",
            "display: flex; flex-direction: row; align-items: flex-start",
        );
        let natural = element_under(&mut document, holder, SVG_TAG, "");
        let sized = element_under(&mut document, holder, SVG_TAG, "width: 120px; height: 80px");
        for element in [natural, sized] {
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        }
        let _ = document.apply_image_events(&[loaded(SOURCE)]);

        assert_eq!(size_of(&mut document, natural), (30.0, 15.0));
        assert_eq!(size_of(&mut document, sized), (120.0, 80.0));
    }

    #[test]
    fn writing_src_asks_the_host_for_it() {
        let mut document = document();
        let element = svg(&mut document, "");
        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        assert_eq!(
            document.take_wanted_images(),
            vec![Arc::<str>::from(SOURCE)]
        );
    }

    /// Every byte outside RFC 3986's unreserved set is escaped: `#` would end
    /// the URL's data at the fragment, `%` would start an escape, and the
    /// space, the quote and the UTF-8 bytes of `é` are not URL code points.
    #[test]
    fn content_becomes_a_percent_encoded_data_url() {
        let mut document = document();
        let element = svg(&mut document, "");
        document.set_attribute(
            element,
            CONTENT_ATTRIBUTE,
            r##"<svg fill="#f00">50% é</svg>"##,
        );

        assert_eq!(
            document.take_wanted_images(),
            vec![Arc::<str>::from(
                "data:image/svg+xml;charset=utf-8,\
                 %3Csvg%20fill%3D%22%23f00%22%3E50%25%20%C3%A9%3C%2Fsvg%3E"
            )]
        );
        assert_eq!(
            content_url("AZaz09-._~"),
            "data:image/svg+xml;charset=utf-8,AZaz09-._~",
            "the unreserved set is written as it is"
        );
    }

    /// Both attributes write the one source, so whichever was written last
    /// is what the element draws and what its `load` answers for.
    #[test]
    fn the_last_attribute_written_wins() {
        let mut document = document();
        let element = svg(&mut document, "");
        let markup = "<svg/>";
        let inline = content_url(markup);

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.set_attribute(element, CONTENT_ATTRIBUTE, markup);
        assert_eq!(
            document.apply_image_events(&[loaded(SOURCE)]),
            Vec::new(),
            "`content` replaced `src`, so `src`'s load is nobody's"
        );
        assert_eq!(
            document.apply_image_events(&[loaded(&inline)]),
            vec![ImageOutcome::Loaded {
                node: element,
                width: 30,
                height: 15,
            }]
        );

        document.set_attribute(element, SRC_ATTRIBUTE, OTHER_SOURCE);
        assert_eq!(
            document.apply_image_events(&[loaded(OTHER_SOURCE)]),
            vec![ImageOutcome::Loaded {
                node: element,
                width: 30,
                height: 15,
            }],
            "and `src` written after `content` replaces it again"
        );
    }

    /// An empty value or a removal of `src` names nothing, as in `<image>`.
    #[test]
    fn an_empty_or_removed_src_names_no_source() {
        let mut document = document();
        let element = svg(&mut document, "");
        document.set_attribute(element, SRC_ATTRIBUTE, "");
        assert!(document.take_wanted_images().is_empty());

        for clear in [Some(""), None] {
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            if let Some(value) = clear {
                document.set_attribute(element, SRC_ATTRIBUTE, value);
            } else {
                document.remove_attribute(element, SRC_ATTRIBUTE);
            }
            document.layout();
            assert!(
                document.rounded_layout(element).is_some(),
                "the element still has a box: {clear:?}"
            );
            assert_eq!(
                document.apply_image_events(&[loaded(SOURCE)]),
                Vec::new(),
                "a cleared `src` answers for nobody: {clear:?}"
            );
        }
    }

    /// An empty value or a removal of `content` keeps whatever source the
    /// element has, as web-core's `_handleContent` does: the `src` written
    /// after it, or the `content`'s own `data:` URL.
    #[test]
    fn an_empty_or_removed_content_keeps_the_current_source() {
        let outcome = |element| {
            vec![ImageOutcome::Loaded {
                node: element,
                width: 30,
                height: 15,
            }]
        };
        for clear in [Some(""), None] {
            let clear_content = |document: &mut LynxDocument, element| {
                if let Some(value) = clear {
                    document.set_attribute(element, CONTENT_ATTRIBUTE, value);
                } else {
                    document.remove_attribute(element, CONTENT_ATTRIBUTE);
                }
            };

            let mut document = document();
            let element = svg(&mut document, "");
            document.set_attribute(element, CONTENT_ATTRIBUTE, "");
            assert!(
                document.take_wanted_images().is_empty(),
                "an empty `content` names nothing: {clear:?}"
            );

            // `src` written after `content` stays when `content` goes.
            document.set_attribute(element, CONTENT_ATTRIBUTE, "<svg/>");
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            let _ = document.take_wanted_images();
            clear_content(&mut document, element);
            assert!(document.take_wanted_images().is_empty(), "{clear:?}");
            assert_eq!(
                document.apply_image_events(&[loaded(SOURCE)]),
                outcome(element),
                "`src` is still the source: {clear:?}"
            );

            // So does the `content`'s own source when it was the last written.
            let mut document = super::super::test_support::document();
            let element = svg(&mut document, "");
            document.set_attribute(element, CONTENT_ATTRIBUTE, "<svg/>");
            clear_content(&mut document, element);
            assert_eq!(
                document.apply_image_events(&[loaded(&content_url("<svg/>"))]),
                outcome(element),
                "the `data:` URL is still the source: {clear:?}"
            );
        }
    }

    /// A source this document has already settled answers at the bind, and is
    /// queued rather than dispatched, for both attributes.
    #[test]
    fn a_settled_source_is_queued_at_the_bind() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let inline = content_url("<svg/>");
        let _ = document.apply_image_events(&[loaded(SOURCE), loaded(&inline)]);
        let element = svg(&mut document, "");

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.set_attribute(element, CONTENT_ATTRIBUTE, "<svg/>");
        let outcome = ImageOutcome::Loaded {
            node: element,
            width: 30,
            height: 15,
        };
        assert_eq!(queued(&events), vec![outcome, outcome]);
    }

    #[test]
    fn is_svg_names_the_tag_alone() {
        let mut document = document();
        let element = svg(&mut document, "");
        let image = child(&mut document, "image", "");
        assert!(is_svg(&document, element));
        assert!(!is_svg(&document, image));
    }

    /// The check for [`super::UA_RULES`]' ordering constraint, as
    /// `nothing_inside_an_image_generates_a_box` is for `image`.
    #[test]
    fn nothing_inside_an_svg_generates_a_box() {
        for sourced in [false, true] {
            let mut document = document();
            let element = svg(&mut document, "width: 100px; height: 60px");
            if sourced {
                document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            }
            let children = [
                "view",
                "scroll-view",
                "list",
                "blur-view",
                "x-blur-view",
                "wrapper",
                "raw-text",
                "path",
            ]
            .map(|tag| (tag, element_under(&mut document, element, tag, "")));
            document.layout();

            for (tag, child) in children {
                assert_eq!(
                    display(&document, child),
                    Display::None,
                    "an svg draws its picture and nothing else: {tag}, sourced={sourced}"
                );
            }
        }
    }
}
