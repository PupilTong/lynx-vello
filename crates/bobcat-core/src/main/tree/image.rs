//! The `image` component joins how Lynx *writes* a picture and what the
//! engine can lay out and paint.
//!
//! Script names a picture with an attribute — `__CreateImage()` builds an
//! `image` element and `__SetAttribute(element, 'src', …)` writes the source
//! on it — while everything downstream speaks replaced content: the source
//! reaches the host's resource system, the completed load reports an intrinsic
//! size back, and the paint walk draws the bitmap into the content box. So
//! this module owns both halves of the join: a [`dom::CustomElement`]
//! reflecting the attribute into [`dom::Document::set_image_source`], and the
//! UA rules that decide what shape the box it draws into has ([`UA_RULES`]).
//!
//! # An `<image>` box is CSS-sized, never bitmap-sized
//!
//! This is the one non-obvious rule, and `contain: size` is the whole of its
//! implementation. `<image>` is not `<img>`: it is a Lynx tag, so `AGENTS.md`'s
//! standards policy puts it in the second bucket, where the engine implements
//! what Lynx does rather than what the nearest W3C feature would do.
//!
//! What Lynx does is give the tag no measurement at all. Native refuses to
//! build a platform layout node for `image` unless it carries `auto-size`
//! (`LayoutContext::NoNeedPlatformLayoutNode`,
//! `core/renderer/ui_wrapper/layout/layout_context.cc`, whose table is exactly
//! `{"image": {"auto-size"}}`), which leaves starlight measuring it as a
//! childless leaf: `LayoutObject::UpdateMeasureWithLeafNode`
//! (`core/renderer/starlight/layout/layout_object.cc`) writes the constraint on
//! a definite axis and **zero** on every other one. No intrinsic ratio softens
//! that — starlight's `SL_DEFAULT_ASPECT_RATIO` is `-1.0f`, i.e. absent. The
//! Android and iOS image views agree, and so does web-core, which buys the same
//! rule from the browser with `contain: strict` on `x-image`
//! (`web-elements`' `x-image.css`). Both references therefore render an
//! unsized `<image src>` as nothing.
//!
//! Our replaced-content path would otherwise do the opposite for free — it
//! sizes a leaf from its natural size the way an `<img>` does — and shipping
//! that would be precisely the "improvement" of a Lynx-only feature the
//! standards policy forbids. `contain: size` is the CSS spelling of Lynx's
//! rule, and it is a *normal* declaration on purpose: the deferred
//! `image[auto-size] { contain: none; }` has to be able to outrank it, and a
//! UA-origin `!important` here could not be outranked by another UA rule.
//!
//! The natural size still arrives and still reaches the node. Containment
//! removes its effect on layout, not the data: `object-fit: contain`/`cover` —
//! what the deferred `mode` attribute maps to — reads it at paint time, in
//! `dom`'s `paint_replaced_content`.
//!
//! # Two cascade facts worth knowing before editing [`UA_RULES`]
//!
//! **These rules have to stay last in the sheet.** `image > *` is specificity
//! (0,0,1) and merely *ties* with the (0,0,1) `display` rules `view`,
//! `scroll-view`, `list` and `wrapper` carry in [`super::ua_sheet`], so source
//! order is what decides them, and moving this block earlier would let an
//! image's children generate boxes again. Writing those four tags out a second
//! time at (0,0,2) would make the block order-free, but only against today's
//! tag list — a tag module added after this one would tie all the same — so
//! the guard is `nothing_inside_an_image_generates_a_box`, which fails the
//! moment the order stops holding, rather than a selector list that can only
//! ever be a snapshot.
//!
//! **A `text` child is the one this sheet cannot suppress.** `text` and
//! `inline-text` carry `display: -lynx-text !important` ([`super::text`]), and
//! a UA-origin important declaration outranks every normal one whatever its
//! specificity — so a `text` written inside an `image` keeps its text block.
//! Matching it would need a second `!important`, which `§D.15` grants to the
//! text block alone. It is masked wherever it could matter: an element with a
//! `src` is replaced, and `dom` hides every child of a replaced box outright,
//! whatever the cascade said.
//!
//! `text > image` is [`super::text`]'s rule, not this module's — an image
//! written *inside* a text is content of that paragraph. Note that
//! `contain: size` applies to it there too, which follows native (an inline
//! image is sized from its own style) rather than web-core (which erases the
//! host element with `display: contents !important` and promotes the shadow
//! `<img>` in its place).
//!
//! # Recorded traps
//!
//! - `Document::set_image_source(id, None)` leaves the node replaced with no source rather than
//!   turning it back into an ordinary element. Unobservable for this tag — under `contain: size`
//!   and the child rules a sourceless replaced `image` and a never-replaced one lay out identically
//!   — but it stops being so the day `auto-size` lands.
//! - `Document::set_natural_size` makes *any* element replaced, including one that never carried a
//!   source. Nothing here calls it; the host's load reports do.
//! - Swapping `src` from one URL to another leaves the departed image's natural size on the node
//!   until the new one reports. Also unobservable under `contain: size`, and also not once
//!   `auto-size` lands. Native blanks the view first.

use dom::{CustomElement, NodeId};

use super::LynxDocument;

/// Lynx's picture tag, and the attribute naming what it draws.
const IMAGE_TAG: &str = "image";
const SRC_ATTRIBUTE: &str = "src";

/// What shape an `image` draws into, from `web-elements`' own sheet and from
/// native's layout defaults.
///
/// `box-sizing: border-box` closes a real gap: `x-image` sits in the common
/// block `web-elements` opens with `display: flex; box-sizing: border-box`
/// (`common-css/linear.css`), and native defaults every element to a border
/// box, while this sheet hands border-box to the container tags and `text`
/// alone. `display: flex` computes nothing today — the fork's initial `display`
/// is already `flex` — and is written to pin the fact that `web-elements`
/// deliberately keeps `x-image` out of the `--lynx-display-toggle` list that
/// `defaultDisplayLinear` drives, a parity native does not share (its display
/// is tag-independent) and that no childless leaf can observe either way.
/// `contain: size` is the load-bearing one; the module docs carry its argument.
///
/// The child rule is `web-elements`' `x-image > * { display: none; }` and
/// native's refusal to let an image take children at all, which makes
/// leaf-ness a property of the tag rather than of whether a `src` happens to
/// be set — `dom` hides a replaced element's children, but only once it is
/// replaced. It ties on specificity with `view`'s, `scroll-view`'s, `list`'s
/// and `wrapper`'s own `display` rules and wins on source order, which is why
/// this block is assembled last; see the module docs.
///
/// Nothing here is `!important`: see
/// `the_ua_sheet_is_important_free_apart_from_the_text_block`.
///
/// What `x-image.css` carries that this deliberately does not: `object-fit:
/// fill` (already both the CSS initial value and Lynx's default `mode`),
/// `align-items`/`justify-content`/`--justify-content`/`flex-direction: row
/// !important` (scaffolding for the shadow `<img>` this engine does not have),
/// and `overflow`/`position`/`border-*`/`min-*`/`scrollbar-width` (whole-sheet
/// container policy this sheet withholds from every tag — and `min-*: 0` is
/// moot here anyway, since containment already makes the automatic minimum
/// size zero).
pub(super) const UA_RULES: &str = "\
image { box-sizing: border-box; display: flex; contain: size; }
image > * { display: none; }
";

/// Installs the component. Must run before any element could carry the tag,
/// which is [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    document.define(IMAGE_TAG, Box::new(Image));
}

/// Points the element at whatever its `src` currently names.
///
/// Only `attribute_changed_callback` is implemented, and that is the whole
/// component. `constructed` could observe nothing: `__CreateImage` mints the
/// element before `__SetAttribute` writes on it, so the reaction that carries
/// the source always comes later. Nothing runs on disconnect either — a
/// removal frees nothing, so a detached image keeps its source and its request,
/// and the path that does free it raises no reaction at all. The registry
/// unbind that a free owes belongs where the free is, in `dom`.
struct Image;

impl CustomElement<()> for Image {
    fn observed_attributes(&self) -> Vec<String> {
        vec![SRC_ATTRIBUTE.to_owned()]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        debug_assert_eq!(name, SRC_ATTRIBUTE, "`image` observes `src` alone");
        // An empty value names nothing, and is the same case as no attribute
        // at all: web-core relays `newval || placeholder` to the inner image,
        // which with no placeholder present makes `''` and a removal identical,
        // and native refuses to build a request for an empty URL.
        //
        // Nothing is trimmed, resolved, or validated. The registry keys on the
        // raw string the page wrote, and turning that into bytes — resolving it
        // against a base URL included — is the embedder's resource system's
        // job, not this engine's.
        //
        // No `old == new` guard: `Document::set_image_source` already returns
        // before it binds, asks, or invalidates anything when the source it is
        // handed is the one already there.
        document.set_image_source(element, new.filter(|source| !source.is_empty()));
    }
}

#[cfg(test)]
mod tests {
    // Every size asserted here is an authored pixel count, a viewport edge, or
    // the exact zero Lynx's rule produces — no measurement, so no tolerance.
    #![allow(clippy::float_cmp)]

    use dom::NodeId;
    use dom::stylo::computed_values::box_sizing;
    use dom::stylo::values::computed::{Contain, Display};

    use super::super::test_support::{
        child, display, document, element_under, style_of, with_config,
    };
    use super::super::{LynxDocument, PageConfig};
    use super::{IMAGE_TAG, SRC_ATTRIBUTE};

    const SOURCE: &str = "app:///a.png";
    /// Enough of a bitmap to derive a ratio from, if anything ever did.
    const NATURAL: (f32, f32) = (40.0, 20.0);

    fn image(document: &mut LynxDocument, style: &str) -> NodeId {
        child(document, IMAGE_TAG, style)
    }

    /// An image with a source and a reported intrinsic size — the state in
    /// which an `<img>` would size itself from its pixels.
    fn loaded_image(document: &mut LynxDocument, style: &str) -> NodeId {
        let element = image(document, style);
        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.set_natural_size(
            element,
            dom::layout::NaturalSize::from_size(dom::layout::Size::new(NATURAL.0, NATURAL.1)),
        );
        element
    }

    fn size_of(document: &mut LynxDocument, element: NodeId) -> (f32, f32) {
        document.layout();
        let layout = document
            .rounded_layout(element)
            .expect("the image is laid out");
        (layout.size.width, layout.size.height)
    }

    #[test]
    fn the_ua_sheet_gives_an_image_a_contained_border_box() {
        for linear in [true, false] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let element = image(&mut document, "");
            document.layout();

            let style = style_of(&document, element);
            assert_eq!(
                style.clone_box_sizing(),
                box_sizing::T::BorderBox,
                "an image is a border box like every other Lynx element: linear={linear}"
            );
            assert_eq!(
                style.clone_display(),
                Display::Flex,
                "`defaultDisplayLinear` reaches the container tags, and \
                 web-elements keeps `x-image` out of that list: linear={linear}"
            );
            assert_eq!(
                style.clone_contain(),
                Contain::SIZE | Contain::INLINE_SIZE | Contain::BLOCK_SIZE,
                "size containment, and nothing else: linear={linear}"
            );
        }
    }

    #[test]
    fn writing_src_makes_an_image_replaced_and_asks_the_host_for_it() {
        let mut document = document();
        let element = image(&mut document, "");
        assert!(
            document.take_wanted_images().is_empty(),
            "an image with no source asks for nothing"
        );

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);

        assert_eq!(
            document.take_wanted_images(),
            vec![std::sync::Arc::<str>::from(SOURCE)],
            "the reflection binds the source, which is what asks the host for it"
        );
        assert!(
            document.take_wanted_images().is_empty(),
            "and asks exactly once"
        );

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        assert!(
            document.take_wanted_images().is_empty(),
            "rewriting the value that was already there asks for nothing"
        );
    }

    /// The rule this module exists for: Lynx gives `<image>` no measurement, so
    /// a box with no definite axis is zero on that axis however large the
    /// bitmap is. An `<img>` would be 393x150 here, deriving the height from
    /// the 2:1 ratio.
    #[test]
    fn an_unsized_image_is_not_sized_by_its_bitmap() {
        let mut document = document();
        let element = loaded_image(&mut document, "");

        assert_eq!(
            size_of(&mut document, element),
            (393.0, 0.0),
            "the cross axis still stretches to the linear container; the main axis is zero"
        );
    }

    #[test]
    fn a_width_alone_does_not_derive_a_height_from_the_bitmap() {
        let mut document = document();
        let element = loaded_image(&mut document, "width: 100px");

        assert_eq!(
            size_of(&mut document, element),
            (100.0, 0.0),
            "the natural ratio is contents, and a size-contained box has none"
        );
    }

    #[test]
    fn an_authored_box_is_the_whole_box() {
        let mut document = document();
        let element = loaded_image(&mut document, "width: 100px; height: 60px; padding: 10px");

        assert_eq!(
            size_of(&mut document, element),
            (100.0, 60.0),
            "border-box sizing puts the padding inside the authored size"
        );
    }

    #[test]
    fn an_empty_or_removed_src_names_no_source() {
        let bare = {
            let mut document = document();
            let element = loaded_image(&mut document, "width: 100px");
            document.remove_attribute(element, SRC_ATTRIBUTE);
            let _ = document.take_wanted_images();
            size_of(&mut document, element)
        };

        for value in ["", " "] {
            let mut document = document();
            let element = image(&mut document, "width: 100px");
            document.set_attribute(element, SRC_ATTRIBUTE, value);

            let wanted = document.take_wanted_images();
            if value.is_empty() {
                assert!(
                    wanted.is_empty(),
                    "an empty source names nothing, as in web-core and native"
                );
                assert_eq!(
                    size_of(&mut document, element),
                    bare,
                    "and lays out exactly like an image whose source was removed"
                );
            } else {
                assert_eq!(
                    wanted,
                    vec![std::sync::Arc::<str>::from(value)],
                    "nothing else is trimmed or normalized: the registry keys on \
                     the raw string the page wrote"
                );
            }
        }
    }

    /// The reflection is the tag's, which is the whole reason it is a
    /// `CustomElement` rather than an attribute hook keyed on the name alone:
    /// a `src` written on a `view` must not turn it into replaced content and
    /// swallow its children.
    #[test]
    fn a_src_on_another_tag_is_not_an_image_source() {
        let mut document = document();
        let view = child(&mut document, "view", "width: 100px; height: 60px");
        let inner = element_under(&mut document, view, "view", "width: 10px; height: 10px");
        document.set_attribute(view, SRC_ATTRIBUTE, SOURCE);

        assert!(document.take_wanted_images().is_empty());
        document.layout();
        assert_eq!(
            document.rounded_layout(inner).expect("laid out").size.width,
            10.0,
            "the view keeps its children"
        );
    }

    /// The tripwire for [`UA_RULES`]' one ordering constraint.
    ///
    /// `image > *` only *ties* with `view`'s, `scroll-view`'s, `list`'s and
    /// `wrapper`'s own `display` rules, so it wins on source order alone. This
    /// fails the moment the block stops being assembled last — which is the
    /// whole reason it is safe to keep the selector short.
    #[test]
    fn nothing_inside_an_image_generates_a_box() {
        for sourced in [false, true] {
            let mut document = document();
            let element = image(&mut document, "width: 100px; height: 60px");
            if sourced {
                document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            }
            let children = [
                "view",
                "scroll-view",
                "list",
                "wrapper",
                "raw-text",
                "x-foreign",
            ]
            .map(|tag| (tag, element_under(&mut document, element, tag, "")));
            document.layout();

            for (tag, child) in children {
                assert_eq!(
                    display(&document, child),
                    Display::None,
                    "an image renders its bitmap and nothing else: {tag}, sourced={sourced}"
                );
            }
        }
    }

    /// The documented exception, asserted rather than wished away: `text`
    /// carries `display: -lynx-text !important`, and a UA-origin important
    /// declaration outranks every normal one. A `src` masks it, because a
    /// replaced element's children are hidden by `dom` and not by the cascade.
    #[test]
    fn a_text_inside_an_image_is_the_one_child_the_sheet_cannot_suppress() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        let text = element_under(&mut document, element, "text", "");
        document.layout();

        assert_eq!(display(&document, text), Display::LynxText);

        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
        document.layout();
        assert_eq!(
            document
                .rounded_layout(text)
                .map(|layout| layout.size.height),
            Some(0.0),
            "being replaced hides every child whatever the cascade decided"
        );
    }

    #[test]
    fn an_image_inside_a_text_is_still_content() {
        let mut document = document();
        let text = child(&mut document, "text", "");
        let element = element_under(&mut document, text, IMAGE_TAG, "");
        document.layout();

        assert_eq!(
            display(&document, element),
            Display::Flex,
            "`text > image` is more specific than this module's bare tag rule"
        );
    }
}
