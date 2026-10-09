//! The `svg` component: Lynx's vector picture tag, drawn as one image.
//!
//! Lynx never treats an SVG document as a subtree. Native's `<svg>` takes a
//! URL in `src` or raw markup in `content` and renders the result at the
//! element's layout size; web-core's `x-svg` is a shadow `<img>` whose `src`
//! is the URL or a `Blob` URL of the markup, and it observes no other
//! attribute and renders no light-DOM child. Under the standards policy the
//! tag is a Lynx-only component that shares its name with the HTML element
//! (`docs/svg-lynx-component-design.md`). This module reflects both
//! attributes into the element's [`dom::ImageRole::Source`], so the element
//! is replaced content and the image pipeline — the parse, the vector image,
//! the natural size, the painter's raster cache — is shared with `<image>`.
//!
//! # `src` and `content`
//!
//! `src` is relayed as written to [`dom::Document::set_image_source`]: the
//! host fetches it and reports the document's bytes, as it does for an
//! `<image src="x.svg">`.
//!
//! `content` goes to [`dom::Document::set_image_document`] as its bytes. No
//! URL is made of it and the host never sees it: `dom` files the markup
//! under a synthetic source named by a 128-bit hash of it
//! (`svg-content:<32 hex digits>`) and queues the bytes, and the runtime
//! parses them as it parses a host's document — natively on the engine
//! thread's blocking pool, on wasm32 inline (`crate::main::page`'s
//! epilogue). Identical markup on any number of elements is one entry, one
//! parse, one scene and one raster texture.
//!
//! The last attribute written wins: both callbacks write the one source, as
//! in web-core, where both end up assigning the shadow `<img>`'s `src`. An
//! empty value or a removal of `src` means no source, as in
//! [`super::image`]. An empty value or a removal of `content` leaves the
//! element's current source as it is, whichever attribute set it: this is a
//! web-core-vs-native difference, ruled for web-core, whose `_handleContent`
//! (`lynx-stack/packages/web-platform/web-elements/src/elements/XSvg/XSvg.ts`)
//! revokes its `Blob` URL on a `null` or empty `content` and does not touch
//! the shadow `<img>`'s `src`.
//!
//! Cost: a `content` document stays in the registry while some element
//! presents it, and is forgotten with the last one (a new `content` or `src`
//! on it, or its release). A `src` URL stays for the document's life, as
//! every host source does.
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
//! keeps its CSS size with the picture drawn into it by the document's own
//! `preserveAspectRatio`. web-core's `contain: content` is not adopted: the
//! element is a replaced leaf here and has nothing to contain.
//!
//! `svg > * { display: none; }` has the same ordering constraint as
//! `image > *` ([`super::image`]'s module docs): it ties on specificity with
//! the container tags' `display` rules and wins on source order, so
//! [`super::ua_sheet`] assembles it after them. Children are never read
//! either: the markup is the attribute, not the subtree.
//! `nothing_inside_an_svg_generates_a_box` is the check.
//!
//! # Events
//!
//! `load` only: the Lynx `<svg>` typing and web-core's `x-svg` both expose
//! `bindload` alone, so a source that fails dispatches nothing. A source that
//! settled before the bind is queued on [`ComponentEvents`] exactly as
//! [`super::image`] queues it. The detail differs from `<image>`'s and is the
//! runtime's to build: native reports the element's layout size, not the
//! document's intrinsic size (ruled 2026-10-08), so the runtime reads the
//! border box at delivery ([`is_svg`] is how it tells the two apart).

use dom::{CustomElement, DocumentKind, ImageRole, NodeId};

use super::{ComponentEvents, LynxDocument};

/// Lynx's vector picture tag, and the two attributes this module reflects.
const SVG_TAG: &str = "svg";
const SRC_ATTRIBUTE: &str = "src";
const CONTENT_ATTRIBUTE: &str = "content";

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
/// always follows the element's creation, and a release unbinds the source
/// in `dom` itself.
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
                document.set_image_document(
                    element,
                    ImageRole::Source,
                    content.as_bytes(),
                    DocumentKind::Svg,
                )
            }),
            other => {
                debug_assert!(false, "`svg` does not observe `{other}`");
                None
            }
        };
        self.outcomes.queue_image(outcome);
    }
}

#[cfg(test)]
mod tests {
    // Every size asserted here is an authored pixel count or a natural size
    // set by hand, so no tolerance.
    #![allow(clippy::float_cmp)]

    use std::sync::Arc;

    use dom::stylo::computed_values::box_sizing;
    use dom::stylo::values::computed::{Contain, Display};
    use dom::{ImageEvent, ImageOutcome, ImageRole, NodeId};

    use super::super::test_support::{
        child, display, document, element_under, style_of, with_component_events, with_config,
    };
    use super::super::{ComponentEvent, ComponentEvents, LynxDocument, PageConfig};
    use super::{CONTENT_ATTRIBUTE, SRC_ATTRIBUTE, SVG_TAG, is_svg};

    const SOURCE: &str = "app:///a.svg";
    const OTHER_SOURCE: &str = "app:///b.svg";
    /// Markup whose natural size, 10x5, tells its load from a `src`'s 30x15.
    const MARKUP: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="5"/>"#;
    /// Other markup, 20x20.
    const OTHER_MARKUP: &str =
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"/>"#;

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

    fn outcome(node: NodeId, width: u32, height: u32) -> ImageOutcome {
        ImageOutcome::Loaded {
            node,
            width,
            height,
        }
    }

    /// The source `element` presents.
    fn source_of(document: &LynxDocument, element: NodeId) -> Option<String> {
        document
            .image_source(element, ImageRole::Source)
            .map(str::to_owned)
    }

    /// How many documents are queued for a parse, draining them.
    fn parses(document: &mut LynxDocument) -> usize {
        document.take_pending_documents().len()
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
    /// size, and a sized one keeps its CSS size, for both attributes.
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
        let inline = element_under(&mut document, holder, SVG_TAG, "");
        for element in [natural, sized] {
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        }
        document.set_attribute(inline, CONTENT_ATTRIBUTE, MARKUP);
        let _ = document.apply_image_events(&[loaded(SOURCE)]);
        let _ = document.apply_pending_documents();

        assert_eq!(size_of(&mut document, natural), (30.0, 15.0));
        assert_eq!(size_of(&mut document, sized), (120.0, 80.0));
        assert_eq!(size_of(&mut document, inline), (10.0, 5.0));
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
        assert_eq!(parses(&mut document), 0, "a URL is the host's to fetch");
    }

    /// `content` is the engine's to parse: nothing is asked of the host, the
    /// markup's bytes are queued as they were written, and the element
    /// presents the synthetic source they are filed under.
    #[test]
    fn content_is_queued_for_the_engine_to_parse() {
        let mut document = document();
        let element = svg(&mut document, "");
        let markup = r##"<svg fill="#f00">50% é</svg>"##;
        document.set_attribute(element, CONTENT_ATTRIBUTE, markup);

        assert!(document.take_wanted_images().is_empty());
        let pending = document.take_pending_documents();
        assert_eq!(pending.len(), 1);
        let (source, bytes, kind) = &pending[0];
        assert_eq!(&**bytes, markup.as_bytes(), "the bytes as written");
        assert_eq!(*kind, dom::DocumentKind::Svg);
        assert!(source.starts_with("svg-content:"), "{source}");
        assert_eq!(source_of(&document, element).as_deref(), Some(&**source));
    }

    /// Both attributes write the one source, so whichever was written last
    /// is what the element draws and what its `load` answers for.
    #[test]
    fn the_last_attribute_written_wins() {
        let mut document = document();
        let element = svg(&mut document, "");

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
        assert_eq!(
            document.apply_image_events(&[loaded(SOURCE)]),
            Vec::new(),
            "`content` replaced `src`, so `src`'s load is nobody's"
        );
        assert_eq!(
            document.apply_pending_documents(),
            vec![outcome(element, 10, 5)]
        );

        document.set_attribute(element, SRC_ATTRIBUTE, OTHER_SOURCE);
        assert_eq!(
            document.apply_image_events(&[loaded(OTHER_SOURCE)]),
            vec![outcome(element, 30, 15)],
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
    /// after it, or the `content`'s own document.
    #[test]
    fn an_empty_or_removed_content_keeps_the_current_source() {
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
                document.take_wanted_images().is_empty() && parses(&mut document) == 0,
                "an empty `content` names nothing: {clear:?}"
            );

            // `src` written after `content` stays when `content` goes.
            document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            let _ = document.take_wanted_images();
            clear_content(&mut document, element);
            assert!(document.take_wanted_images().is_empty(), "{clear:?}");
            assert_eq!(
                document.apply_image_events(&[loaded(SOURCE)]),
                vec![outcome(element, 30, 15)],
                "`src` is still the source: {clear:?}"
            );

            // So does the `content`'s own document when it was the last
            // written.
            let mut document = super::super::test_support::document();
            let element = svg(&mut document, "");
            document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
            clear_content(&mut document, element);
            assert_eq!(
                document.apply_pending_documents(),
                vec![outcome(element, 10, 5)],
                "the markup is still the source: {clear:?}"
            );
        }
    }

    /// A source this document has already settled answers at the bind, and is
    /// queued rather than dispatched, for both attributes.
    #[test]
    fn a_settled_source_is_queued_at_the_bind() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let _ = document.apply_image_events(&[loaded(SOURCE)]);
        // Another element settles the markup first and keeps it bound.
        let first = svg(&mut document, "");
        document.set_attribute(first, CONTENT_ATTRIBUTE, MARKUP);
        let _ = document.apply_pending_documents();
        assert!(queued(&events).is_empty(), "the first bind was pending");

        let element = svg(&mut document, "");
        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
        assert_eq!(
            queued(&events),
            vec![outcome(element, 30, 15), outcome(element, 10, 5)]
        );
        assert_eq!(parses(&mut document), 0, "the markup was already parsed");
    }

    /// Two elements with the same `content` present one source, parsed once,
    /// and both settle when it applies.
    #[test]
    fn two_elements_with_the_same_content_share_one_source() {
        let mut document = document();
        let first = svg(&mut document, "");
        let second = svg(&mut document, "");
        for element in [first, second] {
            document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
        }
        assert_eq!(source_of(&document, first), source_of(&document, second));
        assert_eq!(
            document.apply_pending_documents(),
            vec![outcome(first, 10, 5), outcome(second, 10, 5)],
            "one parse settles both"
        );
    }

    /// A `content` change parses once per distinct markup: writing the
    /// current markup again parses nothing, another element's settled markup
    /// parses nothing, and markup no element presents any more is forgotten,
    /// so writing it later parses it afresh.
    #[test]
    fn a_content_change_reparses_once_per_distinct_markup() {
        let (mut document, events) = with_component_events(PageConfig::default());
        let element = svg(&mut document, "");

        document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
        assert_eq!(document.apply_pending_documents().len(), 1);
        let first = source_of(&document, element).expect("a source");

        document.set_attribute(element, CONTENT_ATTRIBUTE, OTHER_MARKUP);
        assert_eq!(
            document.apply_pending_documents(),
            vec![outcome(element, 20, 20)],
            "new markup is one parse"
        );
        assert!(
            !document.knows_image_source(&first),
            "the markup nobody presents is forgotten"
        );

        document.set_attribute(element, CONTENT_ATTRIBUTE, OTHER_MARKUP);
        assert_eq!(parses(&mut document), 0, "the same markup again");

        let sibling = svg(&mut document, "");
        let _ = queued(&events);
        document.set_attribute(sibling, CONTENT_ATTRIBUTE, OTHER_MARKUP);
        assert_eq!(parses(&mut document), 0, "a sibling's settled markup");
        assert_eq!(queued(&events), vec![outcome(sibling, 20, 20)]);

        document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP);
        assert_eq!(
            document.apply_pending_documents(),
            vec![outcome(element, 10, 5)],
            "forgotten markup is parsed afresh"
        );
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
    /// `nothing_inside_an_image_generates_a_box` is for `image`. Without a
    /// source the element is not replaced, so the cascade alone has to hide
    /// its children.
    #[test]
    fn nothing_inside_an_svg_generates_a_box() {
        for sourced in [None, Some(SRC_ATTRIBUTE), Some(CONTENT_ATTRIBUTE)] {
            let mut document = document();
            let element = svg(&mut document, "width: 100px; height: 60px");
            match sourced {
                Some(SRC_ATTRIBUTE) => document.set_attribute(element, SRC_ATTRIBUTE, SOURCE),
                Some(_) => document.set_attribute(element, CONTENT_ATTRIBUTE, MARKUP),
                None => {}
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
                "rect",
            ]
            .map(|tag| (tag, element_under(&mut document, element, tag, "")));
            document.layout();

            for (tag, child) in children {
                assert_eq!(
                    display(&document, child),
                    Display::None,
                    "an svg draws its picture and nothing else: {tag}, sourced={sourced:?}"
                );
            }
        }
    }
}
