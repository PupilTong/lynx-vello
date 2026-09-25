//! The `image` component joins how Lynx *writes* a picture and what the
//! engine can lay out and paint.
//!
//! Script names a picture with an attribute — `__CreateImage()` builds an
//! `image` element and `__SetAttribute(element, 'src', …)` writes the source
//! on it — while everything downstream speaks replaced content: the source
//! reaches the host's resource system, the completed load reports an intrinsic
//! size back, and the paint walk draws the bitmap into the content box. So
//! this module owns both halves of the join: a [`dom::CustomElement`]
//! reflecting the attributes into [`dom`], and the UA rules that decide what
//! shape the box it draws into has ([`UA_RULES`]).
//!
//! # Which attribute becomes what
//!
//! Four of the tag's attributes are implemented, and they reach the engine
//! three different ways.
//!
//! `src` and `placeholder` are the two *sources*, reflected into the one
//! [`dom::Document::set_image_source`] under [`dom::ImageRole::Source`] and
//! [`dom::ImageRole::Placeholder`] respectively. They are concurrent, not a
//! fallback chain — `dom` owns that model and `docs/tracking/media-resources.md`
//! carries the 2026-09-17 ruling behind it; this module only relays the strings,
//! identically for both, with an empty value meaning no source at all.
//!
//! `mode`, `auto-size` and the rest of what a picture's *shape* depends on are
//! attribute selectors in [`UA_RULES`], not node state. That is web-core's own
//! spelling (`x-image.css`), it costs `dom` no per-node field, and it leaves
//! author CSS outranking the attribute the way PR #261 left `text-overflow`
//! outranking its own.
//!
//! `blur-radius` is a presentational hint, because its value is a length this
//! sheet cannot spell as a finite set of literals. It reflects into
//! `filter: blur(…)`.
//!
//! Writing `src` can also *settle* it in the same call: binding is what asks
//! the host for a URL, so a URL this document has already seen settle answers
//! at the bind rather than through a later report. That answer is a `load` or
//! an `error` this element owes, and it arrives in the middle of a JavaScript
//! call — inside the `__SetAttribute` that wrote the attribute — where nothing
//! may dispatch. So it is queued in [`ImageOutcomes`], which the runtime drains
//! in the epilogue of the entry that produced it; `docs/runtime-architecture.md`
//! has the entry boundary, and [`crate::realm::owner`] the epilogue's order.
//!
//! What is deliberately still missing: `cap-insets` (a 9-slice composite the
//! paint layer has no primitive for) and the animated-image events.
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
//! rule, and it is a *normal* declaration on purpose: `image[auto-size]`'s
//! `contain: none` has to be able to outrank it, and a UA-origin `!important`
//! here could not be outranked by another UA rule.
//!
//! The natural size still arrives and still reaches the node. Containment
//! removes its effect on layout, not the data: `object-fit: contain`/`cover` —
//! what `mode` maps to — reads it at paint time, in `dom`'s
//! `paint_replaced_content`.
//!
//! # `auto-size` is the opt-out, and it is exactly a `<img>` again
//!
//! `image[auto-size]` is the one case in which Lynx *does* build a platform
//! layout node for the tag, and `AutoSizeImage.measure`
//! (`platform/android/.../image/AutoSizeImage.java:57-162`) is what it measures
//! with: both axes exact means take them; one exact axis derives the other
//! through the bitmap's ratio, capped by the available space on the free one;
//! neither exact means the natural size where it fits and a ratio-preserving
//! shrink where it does not. Every clause of that is what CSS already does for
//! a replaced element sized `width: auto; height: auto; max-width: 100%;
//! max-height: 100%` — which is also, near-literally, what web-core writes on
//! its shadow `<img>` under `x-image[auto-size]` (`x-image.css:55-81`). So the
//! rule is `contain: none` plus those two maxima, and the replaced-content
//! path `contain: size` was suppressing does the rest.
//!
//! The maxima are what stops the box overflowing its parent: a stretched cross
//! size transfers through the ratio to a main size that can be larger than the
//! space there is, and `flex-shrink` cannot pull it back, because the transfer
//! *is* the item's flex base size. `auto_size_sizes_the_box_from_its_bitmap`'s
//! 500x400 parent is the case — 400 of height through a 2:1 ratio asks for 800
//! of width, and both references give 500. They clamp *without* re-deriving the
//! other axis through the ratio, which is again both references' behaviour:
//! web-core's own e2e fixture has a 40px-wide, 200px-tall parent whose
//! auto-size image comes out 40x200 with the bitmap stretched to fill it.
//!
//! The rule depends on `hughie` handing a stretched item's cross size to the
//! measurement that produces its flex base size (css-flexbox-1 §9.8 with §9.2
//! step B, PR #287), without which an auto-size image in a parent that
//! stretches it reported its own pixels' main size and came out 40x400 where
//! every browser renders 500x400.
//!
//! Under `auto-size` web-core makes `mode` and `blur-radius` inert — a side
//! effect of the `::part(img)` rules its host-erasing `display: contents`
//! leaves unmatched, not a decision. Native keeps both live, and so does this
//! engine, because nothing here couples them.
//!
//! # Two cascade facts worth knowing before editing [`UA_RULES`]
//!
//! **These rules have to stay last in the sheet.** `image > *` is specificity
//! (0,0,1) and merely *ties* with the (0,0,1) `display` rules `view`,
//! `scroll-view`, `list`, `blur-view`, `x-blur-view` and `wrapper` carry in
//! [`super::ua_sheet`], so source
//! order is what decides them, and moving this block earlier would let an
//! image's children generate boxes again. Writing those six tags out a second
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
//! paragraph's own rules alone. It is masked wherever it could matter: an
//! element with a `src` is replaced, and `dom` hides every child of a replaced
//! box outright, whatever the cascade said.
//!
//! `text > image` is [`super::text`]'s rule, not this module's — an image
//! written *inside* a text is content of that paragraph, and so is the
//! `padding: 0 !important` that module gives it. Note that `contain: size`
//! applies to it there too, which follows native (an inline image is sized
//! from its own style) rather than web-core (which erases the host element
//! with `display: contents !important` and promotes the shadow `<img>` in its
//! place).
//!
//! # Recorded traps
//!
//! - `Document::set_natural_size` makes *any* element replaced, including one that never carried a
//!   source. Nothing here calls it; the document sets it from the load reports. For an element that
//!   *has* a source it is the document's to maintain — it recomputes it from whichever of the two
//!   sources the element draws — so a size written by hand there survives only until the next
//!   report or source change.
//! - Under `auto-size` that recomputation is a *layout* input, and the bitmap it names is whichever
//!   one the element draws: the placeholder's size until `src` has pixels, `src`'s afterwards. So a
//!   placeholder can size the box, which is iOS's and web-core's behaviour; Android sizes from
//!   `src` alone (`AutoSizeImage.justSize` is fed by the source load).
//! - `Document::set_presentational_hint` leaves an unparsable value as a *no-op*, so `blur-radius`
//!   removes its hint before offering a new one. Setting it in one call would let
//!   `blur-radius="garbage"` keep the previous `blur-radius="10px"`.
//! - `filter: blur(…)` paints since PR #273, as an offscreen bake of the *whole element*: a
//!   `blur-radius` here blurs the background and border along with the bitmap, and its ink
//!   overflows the box, where native and web-core blur only the bitmap and keep it inside the box.
//!   Ruled 2026-09-20 to stay so in this change; bitmap-only blur is a separate piece of paint
//!   work, and needs a declaration other than `filter` for this module to write.
//! - A `load` or an `error` is the *element's own source* settling. The placeholder produces
//!   neither, which is `dom`'s rule rather than this module's: [`dom::ImageOutcome`] has no variant
//!   naming a placeholder.

use std::cell::RefCell;
use std::rc::Rc;

use dom::{CustomElement, ImageOutcome, ImageRole, NodeId};

use super::LynxDocument;

/// Lynx's picture tag, and the attributes this module reflects off it.
///
/// `mode` and `auto-size` are absent on purpose: they are selectors in
/// [`UA_RULES`], so nothing observes them here.
const IMAGE_TAG: &str = "image";
const SRC_ATTRIBUTE: &str = "src";
const PLACEHOLDER_ATTRIBUTE: &str = "placeholder";
const BLUR_RADIUS_ATTRIBUTE: &str = "blur-radius";

/// The property `blur-radius` reflects into, as a presentational hint.
const FILTER_PROPERTY: &str = "filter";

/// What shape an `image` draws into, from `web-elements`' own sheet and from
/// native's layout defaults.
///
/// The border box and the rest of `web-elements`' common block
/// (`common-css/linear.css`) come from [`super::ua_sheet`], which hands them to
/// `image` beside the container tags and `text`. `display: flex` computes nothing today — the
/// fork's initial `display` is already `flex` — and is written to pin the fact that `web-elements`
/// deliberately keeps `x-image` out of the `--lynx-display-toggle` list that
/// `defaultDisplayLinear` drives, a parity native does not share (its display
/// is tag-independent) and that no childless leaf can observe either way.
/// `contain: size` is the load-bearing one; the module docs carry its argument.
///
/// The child rule is `web-elements`' `x-image > * { display: none; }` and
/// native's refusal to let an image take children at all, which makes
/// leaf-ness a property of the tag rather than of whether a `src` happens to
/// be set — `dom` hides a replaced element's children, but only once it is
/// replaced. It ties on specificity with the `display` rules `view`,
/// `scroll-view`, `list`, `blur-view`, `x-blur-view` and `wrapper` carry, and
/// wins on source order, which is why
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
///
/// # `mode` and `auto-size`
///
/// Both are attribute selectors rather than node state, which is web-core's
/// own spelling of them (`x-image.css:38-57`). Three literals cover `mode`
/// whole: `scaleToFill`, an absent attribute and anything unrecognised all
/// leave the initial `fill`, and the three that are not `fill` name the CSS
/// keyword each reference already maps them to — Android's
/// `FIT_CENTER`/`CENTER_CROP`/`CENTER` (`LynxImageManager.getMode`), Harmony's
/// `ARKUI_OBJECT_FIT_CONTAIN`/`COVER`/`NONE` (`ui_new_image.cc:220-231`), and
/// web-core's `contain`/`cover` plus, for `center`, an absolutely positioned
/// unscaled `<img>` centred in the host — which is `object-fit: none` with the
/// initial `object-position: 50% 50%`, one source pixel per CSS pixel.
///
/// The literals are case-sensitive, matching web-core's selectors and native's
/// `equals`/`isEqualToString` readers; this is not an HTML document, so the
/// fork matches attribute values case-sensitively by default.
///
/// `auto-size` is a boolean, and `:not([auto-size="false"])` is how a boolean
/// survives the two stacks' opposite spellings of "off": `__SetAttribute`
/// stringifies `false` into the literal `"false"`
/// (`packages/bobcat-element/src/element-papi.ts:1377-1385`), while web-core's
/// component framework strips any attribute whose value is `"false"` outright
/// (`web-elements/src/element-reactive/component.ts:158-205`) and native parses
/// a bool. Matching the string covers the first and is vacuous for the second.
///
/// Nothing pairs `mode` or `blur-radius` with `auto-size`: see the module docs.
pub(super) const UA_RULES: &str = r#"image { display: flex; contain: size; }
image[mode="aspectFit"] { object-fit: contain; }
image[mode="aspectFill"] { object-fit: cover; }
image[mode="center"] { object-fit: none; }
image[auto-size]:not([auto-size="false"]) { contain: none; max-width: 100%; max-height: 100%; }
image > * { display: none; }
"#;

/// The `load`s and `error`s this document has produced and not delivered yet.
///
/// A handle rather than a field, because the two producers are on opposite
/// sides of the document: the component below, which is inside it and reaches
/// nothing else, and the runtime's own report path, which is outside it. Both
/// hold a clone of this one queue, and the runtime drains it in the epilogue
/// of every entry.
///
/// Queueing is the only thing either producer can do with an outcome, which is
/// what keeps one from being dropped — there is no call that both settles a
/// source and answers somewhere else.
#[derive(Clone, Default)]
pub(crate) struct ImageOutcomes(Rc<RefCell<Vec<ImageOutcome>>>);

impl ImageOutcomes {
    /// Queues what a source bind settled, if it settled anything. `None` is
    /// the ordinary case — a source still loading, or a write that changed
    /// nothing.
    pub(crate) fn queue(&self, outcome: Option<ImageOutcome>) {
        if let Some(outcome) = outcome {
            self.0.borrow_mut().push(outcome);
        }
    }

    /// Queues a whole report batch's outcomes, in the order `dom` returned
    /// them.
    pub(crate) fn extend(&self, outcomes: Vec<ImageOutcome>) {
        self.0.borrow_mut().extend(outcomes);
    }

    /// Takes everything queued since the last drain.
    pub(crate) fn take(&self) -> Vec<ImageOutcome> {
        std::mem::take(&mut *self.0.borrow_mut())
    }

    /// Whether anything is waiting for a turn to be delivered on.
    pub(crate) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

/// Installs the component, over the queue its bind-time outcomes go into. Must
/// run before any element could carry the tag, which is
/// [`Document::define`](dom::Document::define)'s own precondition.
pub(super) fn define(document: &mut LynxDocument, outcomes: ImageOutcomes) {
    document.define(IMAGE_TAG, Box::new(Image { outcomes }));
}

/// Points the element at whatever its attributes currently name.
///
/// Only `attribute_changed_callback` is implemented, and that is the whole
/// component. `constructed` could observe nothing: `__CreateImage` mints the
/// element before `__SetAttribute` writes on it, so the reaction that carries
/// the source always comes later. Nothing runs on disconnect either — a
/// removal frees nothing, so a detached image keeps its source and its request,
/// and the path that does free it raises no reaction at all. The registry
/// unbind that a free owes belongs where the free is, in `dom`.
struct Image {
    /// Where a `src` that settles at the bind leaves its outcome. The
    /// reaction runs inside a JavaScript call, so it cannot dispatch.
    outcomes: ImageOutcomes,
}

impl CustomElement<()> for Image {
    fn observed_attributes(&self) -> Vec<String> {
        vec![
            SRC_ATTRIBUTE.to_owned(),
            PLACEHOLDER_ATTRIBUTE.to_owned(),
            BLUR_RADIUS_ATTRIBUTE.to_owned(),
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
        match name {
            // An empty value names nothing, and is the same case as no
            // attribute at all: web-core relays `newval || placeholder` to the
            // inner image, which with no placeholder present makes `''` and a
            // removal identical, and native refuses to build a request for an
            // empty URL.
            //
            // Nothing is trimmed, resolved, or validated. The registry keys on
            // the raw string the page wrote, and turning that into bytes —
            // resolving it against a base URL included — is the embedder's
            // resource system's job, not this engine's.
            //
            // No `old == new` guard: the setter already returns before it
            // binds, asks, or invalidates anything when handed the value that
            // role already holds.
            //
            // A source the registry has already settled answers here, at the
            // bind, because no report will ever arrive for it again — the
            // second mount of a URL this document has seen. It is queued
            // rather than dispatched: this runs inside `__SetAttribute`, and
            // an event delivered from there would re-enter the realm in the
            // middle of the call that wrote the attribute. web-core's
            // equivalent is asynchronous for the same reason — an `<img>`
            // load event is a task, even for a cached URL.
            SRC_ATTRIBUTE => self.outcomes.queue(document.set_image_source(
                element,
                ImageRole::Source,
                source(new),
            )),
            // The placeholder is the same string relayed the same way, into
            // the same setter, because in native it is the same kind of thing:
            // a URL requested in its own right, concurrently with `src`,
            // rather than a fallback the element reaches for when `src` fails.
            // Only the role differs — and with it the outcome, which
            // `ImageRole::Placeholder` never produces, so there is nothing to
            // queue here.
            PLACEHOLDER_ATTRIBUTE => {
                document.set_image_source(element, ImageRole::Placeholder, source(new));
            }
            // Removing first is load-bearing: a hint keeps its old declaration
            // when handed a value that does not parse, so offering
            // `blur(garbage)` on its own would leave a previous `blur(10px)`
            // standing. The raw attribute value goes into `blur(…)` unchanged,
            // which is web-core's grammar — it writes the value into
            // `--blur-radius` and lets the browser's `blur()` judge it — and
            // costs a unitless number, which Android reads as physical pixels
            // and CSS rejects.
            BLUR_RADIUS_ATTRIBUTE => {
                document.set_presentational_hint(element, FILTER_PROPERTY, "");
                if let Some(radius) = source(new) {
                    document.set_presentational_hint(
                        element,
                        FILTER_PROPERTY,
                        &format!("blur({radius})"),
                    );
                }
            }
            other => debug_assert!(false, "`image` does not observe `{other}`"),
        }
    }
}

/// An attribute value that names something, with the empty string and a
/// removal being the same nothing.
fn source(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    // Every size asserted here is an authored pixel count, a viewport edge, or
    // the exact zero Lynx's rule produces — no measurement, so no tolerance.
    #![allow(clippy::float_cmp)]

    use dom::NodeId;
    use dom::stylo::computed_values::box_sizing;
    use dom::stylo::computed_values::object_fit::T as ObjectFit;
    use dom::stylo::values::computed::{Contain, Display};

    use super::super::test_support::{
        child, display, document, element_under, style_of, with_config,
    };
    use super::super::{LynxDocument, PageConfig};
    use super::{BLUR_RADIUS_ATTRIBUTE, IMAGE_TAG, PLACEHOLDER_ATTRIBUTE, SRC_ATTRIBUTE};

    const SOURCE: &str = "app:///a.png";
    const PLACEHOLDER: &str = "app:///holding.png";
    const AUTO_SIZE_ATTRIBUTE: &str = "auto-size";
    const MODE_ATTRIBUTE: &str = "mode";
    /// Enough of a bitmap to derive a ratio from, if anything ever did — as
    /// layout wants it, and as the host reports it.
    const NATURAL: (f32, f32) = (40.0, 20.0);
    const NATURAL_PIXELS: (u32, u32) = (40, 20);

    fn image(document: &mut LynxDocument, style: &str) -> NodeId {
        child(document, IMAGE_TAG, style)
    }

    /// Writes `value`, or removes the attribute when it is `None`.
    fn set(document: &mut LynxDocument, element: NodeId, name: &str, value: Option<&str>) {
        if let Some(value) = value {
            document.set_attribute(element, name, value);
        } else {
            document.remove_attribute(element, name);
        }
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
                "blur-view",
                "x-blur-view",
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

    /// `placeholder` is a second source, not a fallback, so it is relayed
    /// exactly as `src` is — same request, same "nothing is normalized" rule.
    #[test]
    fn a_placeholder_is_asked_for_exactly_like_a_source() {
        for (attribute, url, other) in [
            (SRC_ATTRIBUTE, SOURCE, "app:///b.png"),
            (PLACEHOLDER_ATTRIBUTE, PLACEHOLDER, "app:///holding-2.png"),
        ] {
            let mut document = document();
            let element = image(&mut document, "width: 100px; height: 60px");
            assert!(document.take_wanted_images().is_empty());

            set(&mut document, element, attribute, Some(url));
            assert_eq!(
                document.take_wanted_images(),
                vec![std::sync::Arc::<str>::from(url)],
                "{attribute} binds on write, which is what asks the host for it",
            );

            set(&mut document, element, attribute, Some(url));
            assert!(
                document.take_wanted_images().is_empty(),
                "{attribute}: rewriting the value already there asks for nothing",
            );

            for cleared in [Some(""), None] {
                set(&mut document, element, attribute, cleared);
                assert!(
                    document.take_wanted_images().is_empty(),
                    "{attribute}={cleared:?} names nothing",
                );
            }

            set(&mut document, element, attribute, Some(other));
            assert_eq!(
                document.take_wanted_images(),
                vec![std::sync::Arc::<str>::from(other)],
                "{attribute}: and nothing is trimmed or resolved on the way",
            );
        }
    }

    /// An empty `placeholder` is the same nothing a removed one is, and the
    /// place that shows is the box: under `auto-size` the natural size is a
    /// layout input, and `dom` takes it from whichever bitmap the element
    /// draws — the placeholder's while `src` has no pixels.
    ///
    /// That a placeholder can size the box at all follows iOS and web-core;
    /// Android's `AutoSizeImage` is fed by the source load alone
    /// (`AutoSizeImage.justSize`), so there a placeholder never sizes anything.
    #[test]
    fn an_empty_or_removed_placeholder_leaves_the_element_nothing_to_draw() {
        let mut document = document();
        let parent = child(&mut document, "view", "display: flex; flex-direction: row");
        let element = element_under(&mut document, parent, IMAGE_TAG, "");
        document.set_attribute(element, AUTO_SIZE_ATTRIBUTE, "");
        document.set_attribute(element, PLACEHOLDER_ATTRIBUTE, PLACEHOLDER);
        let _ = document.apply_image_events(&[dom::ImageEvent::Loaded {
            source: std::sync::Arc::from(PLACEHOLDER),
            width: NATURAL_PIXELS.0,
            height: NATURAL_PIXELS.1,
        }]);
        assert_eq!(size_of(&mut document, element), NATURAL);

        for cleared in [Some(""), None] {
            set(&mut document, element, PLACEHOLDER_ATTRIBUTE, cleared);
            assert_eq!(
                size_of(&mut document, element),
                (0.0, 0.0),
                "placeholder={cleared:?} leaves no bitmap and so no size",
            );
            set(
                &mut document,
                element,
                PLACEHOLDER_ATTRIBUTE,
                Some(PLACEHOLDER),
            );
            assert_eq!(size_of(&mut document, element), NATURAL);
        }
    }

    /// Both sources at once, which is the state native's model exists for: the
    /// element asks for each of them once, and neither request waits on the
    /// other.
    #[test]
    fn a_source_and_a_placeholder_are_two_concurrent_requests() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        document.set_attribute(element, PLACEHOLDER_ATTRIBUTE, PLACEHOLDER);
        document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);

        let mut wanted = document.take_wanted_images();
        wanted.sort();
        let mut expected = vec![
            std::sync::Arc::<str>::from(PLACEHOLDER),
            std::sync::Arc::<str>::from(SOURCE),
        ];
        expected.sort();
        assert_eq!(wanted, expected);
    }

    fn object_fit(document: &LynxDocument, element: NodeId) -> ObjectFit {
        style_of(document, element).clone_object_fit()
    }

    /// `mode` reaches the engine as three UA attribute rules and nothing else.
    /// The literals are case-sensitive, exactly as web-core's selectors and
    /// native's string comparisons are, and everything they do not name —
    /// `scaleToFill`, a typo, an empty value, no attribute at all — is the CSS
    /// initial `fill`, which is also Android's and Harmony's fallback.
    #[test]
    fn the_mode_attribute_selects_an_object_fit() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        for (mode, fit) in [
            (Some("aspectFit"), ObjectFit::Contain),
            (Some("aspectFill"), ObjectFit::Cover),
            (Some("center"), ObjectFit::None),
            (Some("scaleToFill"), ObjectFit::Fill),
            (Some("aspectfit"), ObjectFit::Fill),
            (Some("ASPECTFIT"), ObjectFit::Fill),
            (Some("contain"), ObjectFit::Fill),
            (Some(""), ObjectFit::Fill),
            (Some("aspectFit"), ObjectFit::Contain),
            (None, ObjectFit::Fill),
        ] {
            set(&mut document, element, MODE_ATTRIBUTE, mode);
            // A whole layout pass between writes: the flip has to restyle
            // through stylo's attribute dependencies, not through a first
            // cascade that had not happened yet.
            document.layout();
            assert_eq!(object_fit(&document, element), fit, "mode={mode:?}");
            assert_eq!(
                document.get(element).unwrap().attribute(MODE_ATTRIBUTE),
                mode,
                "and the attribute itself is never rewritten",
            );
        }
    }

    /// The `mode` rules are UA origin and plain, so a page's own CSS outranks
    /// them — the standing the `text-overflow` attribute rules have. web-core
    /// differs here and knows it: its `x-image[mode=…]` rule is (0,1,1) in an
    /// author-level sheet, so it beats a page's class rule.
    #[test]
    fn author_css_outranks_the_mode_attribute() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        document.set_attribute(element, MODE_ATTRIBUTE, "aspectFit");
        document.add_class(element, "override");
        document.add_stylesheet(
            "@layer fits { .override { object-fit: cover; } }",
            dom::StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(object_fit(&document, element), ObjectFit::Cover);
        assert_eq!(
            document.get(element).unwrap().attribute(MODE_ATTRIBUTE),
            Some("aspectFit"),
            "the author declaration wins the cascade without touching the attribute",
        );

        document.remove_class(element, "override");
        document.layout();
        assert_eq!(object_fit(&document, element), ObjectFit::Contain);
    }

    /// `auto-size` is the one attribute that lifts `contain: size`, and it is a
    /// boolean: present is on, and the literal `"false"` — what
    /// `__SetAttribute` writes for a JavaScript `false` — is off.
    #[test]
    fn auto_size_lifts_size_containment() {
        const CONTAINED: Contain = Contain::from_bits_retain(
            Contain::SIZE.bits() | Contain::INLINE_SIZE.bits() | Contain::BLOCK_SIZE.bits(),
        );

        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        for (value, contain) in [
            (None, CONTAINED),
            (Some(""), Contain::empty()),
            (Some("false"), CONTAINED),
            (Some("true"), Contain::empty()),
            (Some("auto-size"), Contain::empty()),
            (Some("false"), CONTAINED),
            (None, CONTAINED),
            (Some(""), Contain::empty()),
        ] {
            set(&mut document, element, AUTO_SIZE_ATTRIBUTE, value);
            document.layout();
            assert_eq!(
                style_of(&document, element).clone_contain(),
                contain,
                "auto-size={value:?}",
            );
        }
    }

    /// web-core's own `basic-element-image-auto-size` fixture, as sizes.
    ///
    /// The fixture nests an outer view laid out in one direction, an inner view
    /// constraining both axes, only the width, or only the height, and the
    /// auto-size image inside that. Every expected pair here is what Chrome
    /// renders for the element web-core actually lays out under those parents —
    /// its shadow `<img>`, which `x-image[auto-size]`'s `display: contents`
    /// promotes into the inner view and `::part(img)` sizes `max-width: 100%;
    /// max-height: 100%` (`x-image.css:55-81`).
    ///
    /// Native reaches the same six sizes. Its measure functions never grow a
    /// bitmap on an at-most axis (`AutoSizeImage.java:144-146`,
    /// `LynxUIImage.mm:1992-2043`), but starlight measures a stretched item with
    /// an exact cross constraint (`flex_layout_algorithm.cc:132-146`,
    /// `:335-345`), and an exact axis derives the other through the ratio,
    /// capped by the at-most constraint on it — which is the stretch, the ratio
    /// transfer and `max-*: 100%` of the CSS path. See
    /// `docs/tracking/deviations.md`.
    #[test]
    fn auto_size_sizes_the_box_from_its_bitmap() {
        for (direction, inner, expected) in [
            ("column", "width: 500px; height: 400px", (500.0, 400.0)),
            ("column", "width: 40px", (40.0, 20.0)),
            ("column", "height: 50px", (100.0, 50.0)),
            ("row", "width: 500px; height: 400px", (500.0, 400.0)),
            ("row", "width: 40px", (40.0, 1000.0)),
            ("row", "height: 50px", (100.0, 50.0)),
        ] {
            let mut document = document();
            let outer = child(
                &mut document,
                "view",
                &format!(
                    "display: flex; flex-direction: {direction}; width: 1000px; height: 1000px"
                ),
            );
            let inner = element_under(
                &mut document,
                outer,
                "view",
                &format!("display: flex; {inner}"),
            );
            let element = element_under(&mut document, inner, IMAGE_TAG, "");
            document.set_attribute(element, AUTO_SIZE_ATTRIBUTE, "");
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            document.set_natural_size(
                element,
                dom::layout::NaturalSize::from_size(dom::layout::Size::new(NATURAL.0, NATURAL.1)),
            );

            assert_eq!(
                size_of(&mut document, element),
                expected,
                "{direction} / {inner}",
            );
        }
    }

    /// An authored axis still wins under `auto-size`: it is the "exact" branch
    /// of `AutoSizeImage.measure` and the used value of `width`/`height` in
    /// CSS. The free axis comes from the ratio, and `max-height: 100%` caps it.
    #[test]
    fn an_authored_axis_outranks_the_bitmap_under_auto_size() {
        for (own, expected) in [
            ("width: 100px; height: 60px", (100.0, 60.0)),
            ("width: 100px", (100.0, 50.0)),
            ("height: 60px", (120.0, 60.0)),
            ("", NATURAL),
        ] {
            let mut document = document();
            // A row parent, so the cross axis the container stretches is the
            // one the authored height already pins: what is asserted here is
            // the ratio, not the stretch that
            // `auto_size_sizes_the_box_from_its_bitmap` covers.
            let parent = child(&mut document, "view", "display: flex; flex-direction: row");
            let element = element_under(&mut document, parent, IMAGE_TAG, own);
            document.set_attribute(element, AUTO_SIZE_ATTRIBUTE, "");
            document.set_attribute(element, SRC_ATTRIBUTE, SOURCE);
            document.set_natural_size(
                element,
                dom::layout::NaturalSize::from_size(dom::layout::Size::new(NATURAL.0, NATURAL.1)),
            );

            assert_eq!(size_of(&mut document, element), expected, "{own:?}");
        }
    }

    /// Flipping `auto-size` after a layout pass has to move the box, which is
    /// the attribute-dependency half of the UA rule working.
    #[test]
    fn flipping_auto_size_after_a_layout_resizes_the_box() {
        let mut document = document();
        let element = loaded_image(&mut document, "width: 100px");
        assert_eq!(size_of(&mut document, element), (100.0, 0.0));

        for (value, expected) in [
            (Some(""), (100.0, 50.0)),
            (Some("false"), (100.0, 0.0)),
            (Some(""), (100.0, 50.0)),
            (None, (100.0, 0.0)),
        ] {
            set(&mut document, element, AUTO_SIZE_ATTRIBUTE, value);
            assert_eq!(size_of(&mut document, element), expected, "{value:?}");
        }
    }

    /// The blur radius in the element's computed `filter`, in CSS pixels;
    /// `None` when the list holds no blur at all, the initial value included.
    fn blur(document: &LynxDocument, element: NodeId) -> Option<f32> {
        use dom::stylo::values::computed::effects::Filter;

        match style_of(document, element).get_effects().filter.0.first() {
            Some(Filter::Blur(radius)) => Some(radius.0.px()),
            _ => None,
        }
    }

    /// `blur-radius` is the one `<image>` attribute that becomes a
    /// presentational hint, because its value is a length rather than one of a
    /// handful of literals. The raw string goes into `blur(…)`, so CSS judges
    /// it — which is web-core's grammar, since it writes the value into
    /// `--blur-radius` and lets `blur(var(--blur-radius))` parse it.
    #[test]
    fn blur_radius_reflects_into_a_filter() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        for (value, expected) in [
            (Some("10px"), Some(10.0)),
            // A relative length computes against this element's own font size,
            // which the page's `font-family` block leaves at the initial 16px.
            (Some("2em"), Some(32.0)),
            // An invalid value must not leave the previous one standing.
            (Some("garbage"), None),
            (Some("10px"), Some(10.0)),
            // Android reads a unitless number as physical pixels
            // (`UnitUtils.toPxWithDisplayMetrics`'s final `Float.parseFloat`);
            // CSS `blur()` needs a length, so web-core drops it, and so do we.
            (Some("10"), None),
            (Some("10px"), Some(10.0)),
            // Not a second declaration: one property's value is parsed on its
            // own, so a `;` is simply invalid.
            (Some("10px); color: red"), None),
            (Some("10px"), Some(10.0)),
            (Some(""), None),
            (Some("10px"), Some(10.0)),
            (None, None),
        ] {
            set(&mut document, element, BLUR_RADIUS_ATTRIBUTE, value);
            document.layout();
            assert_eq!(blur(&document, element), expected, "blur-radius={value:?}");
            assert_eq!(
                document
                    .get(element)
                    .unwrap()
                    .attribute(BLUR_RADIUS_ATTRIBUTE),
                value,
                "and the attribute itself is never rewritten",
            );
        }
    }

    /// The hint origin loses to author CSS and to inline style, so a page can
    /// still take the blur off or replace it.
    #[test]
    fn author_css_outranks_the_blur_radius_attribute() {
        let mut document = document();
        let element = image(&mut document, "width: 100px; height: 60px");
        document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, "10px");
        document.add_class(element, "override");
        document.add_stylesheet(
            "@layer blurs { .override { filter: none; } }",
            dom::StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(blur(&document, element), None);
        assert_eq!(
            document
                .get(element)
                .unwrap()
                .attribute(BLUR_RADIUS_ATTRIBUTE),
            Some("10px"),
            "the author declaration wins the cascade without touching the attribute",
        );

        document.remove_class(element, "override");
        document.layout();
        assert_eq!(blur(&document, element), Some(10.0));
    }
}
