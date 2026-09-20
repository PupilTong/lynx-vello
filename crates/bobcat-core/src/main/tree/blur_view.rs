//! The `blur-view` component: the container whose `blur-radius` attribute
//! blurs what is behind it.
//!
//! The whole of it is one reflection. `backdrop-filter` is already
//! implemented, W3C-style, from the cascade down to the painter's prefix bake,
//! so nothing here touches paint or layout: the component turns the attribute
//! into a `backdrop-filter: blur(…)` presentational hint and the rest of the
//! engine does what it would have done for author CSS.
//!
//! # Two tag names, one component
//!
//! Native registers `blur-view` — `LYNX_LAZY_REGISTER_UI("blur-view")` on iOS,
//! `@LynxBehavior(tagName = ["blur-view"])` on Android,
//! `map["blur-view"] = {UIBlurView::Make}` in `lynx_xelement/registry.cc` on
//! Harmony. web-core registers `x-blur-view`
//! (`@Component('x-blur-view', [CommonEventsAndMethods, BlurRadius], …)` in
//! `web-elements/src/elements/XView/XBlurView.ts`) and nothing else: the tag is
//! absent from `LYNX_TAG_TO_HTML_TAG_MAP`, so `__CreateElement` passes the JSX
//! tag through verbatim and a compiled `.web.bundle` writes `x-blur-view` into
//! the tree. Both names therefore reach this engine, and both are defined.
//!
//! # The radius is a CSS length, not a number
//!
//! The three references disagree about units, and only one of them is a
//! styling engine:
//!
//! - web-core drops the unit. `BlurRadius.ts` writes `:host { backdrop-filter:
//!   blur(${parseFloat(newVal)}px) }` into a per-instance shadow `<style>`, so `20rpx` becomes
//!   `20px`.
//! - iOS drops it too, by a different route: `blur-radius` is a `LYNX_PROP_SETTER(…, CGFloat)`, and
//!   `LynxConverter`'s `toCGFloat` is `[value doubleValue]` — `NSString`'s own numeric prefix scan.
//! - Android honors it. `LynxUIBlurView.setBlurRadius` runs the string through
//!   `UnitUtils.toPxWithDisplayMetrics`, which knows `rpx`, `ppx`, `px`, `%`, `rem`, `em`, `vw` and
//!   `vh`.
//! - Harmony honors it as CSS: `UIBlurView::OnPropUpdate` parses with
//!   `CSSStringParser::ParseLengthTo` and hands the result to `NODE_BACKDROP_BLUR` — the same
//!   property the platform's own toolkit spells the same way.
//!
//! **Ruled (user, 2026-09-20): unit conversion is the styling engine's job**,
//! so the attribute's text goes into the cascade as a CSS length rather than
//! through a `parseFloat`. `20rpx` resolves against the viewport, `1.5em`
//! against the element's own font size, `2vw` against the viewport — one
//! resolution path, the one every other length in this engine already takes.
//! A bare number is the one value that needs rewriting: `25` is not a CSS
//! length, and both references read it as pixels, so it is reflected as
//! `25px`. Recorded in `docs/tracking/deviations.md`.
//!
//! # A rejected value has to clear the hint
//!
//! [`dom::Document::set_presentational_hint`] has `setProperty` semantics: an
//! empty value removes the hint, and **an invalid value is a no-op**. A no-op
//! is the wrong answer here — `blur-radius="abc"` after `blur-radius="25"`
//! would leave the 25px blur standing — and the grammar that decides valid is
//! the fork's, not this module's, so the reflection cannot pre-judge it. So
//! every write clears first and then sets. The clear costs nothing when there
//! is nothing to clear (the setter returns before it touches the element when
//! the property is not in the block), and when there is, the element was
//! already being restyled.
//!
//! # What the cascade gives the tag
//!
//! Both tags are in [`super::ua_sheet`]'s container list — a border box and
//! the display mode `defaultDisplayLinear` picks — and in its
//! `defaultOverflowVisible` rule beside `page` and `view`. That follows
//! native, where `LynxUIBlurView` extends `LynxUIView` and a blur view is a
//! view in every respect layout can see. web-core is not quite that:
//! `x-blur-view` is in `linear.css`'s common block (`display: flex`,
//! `box-sizing: border-box`, `position: relative`, `overflow: clip`,
//! `border-width: 0`, `min-width`/`min-height: 0`) and in the linear *item*
//! rules, but in neither the `--lynx-display-toggle` list that
//! `defaultDisplayLinear` drives nor the `[lynx-default-overflow-visible=true]
//! x-view` escape — so in a browser a blur view is a row flex container that
//! always clips, whichever way the two switches are set. The divergence is the
//! one `scroll-view` and `list` already record in
//! `docs/tracking/deviations.md`: a per-tag exception to either switch would
//! have to be `!important`, which `docs/style-assumptions.md` §D.15 forbids in
//! this sheet.
//!
//! The author's own `backdrop-filter` outranks the attribute, because a
//! presentational hint loses to author rules and to inline style. That is
//! web-core's ordering too: its `:host` rule is the lowest-priority author
//! rule an element can carry.
//!
//! # What is deliberately absent
//!
//! Every platform-only property: `blur-effect` (the iOS
//! `light`/`dark`/`extra-light`/`glass`/`glass-container` system tint),
//! `blur-sampling`, `spacing`, `android-capture-target`, `enable-auto-blur`,
//! `experimental-update-blur-radius`, `ios-user-interface-style`,
//! `glass-interactive`, `glass-tint-color` and `glass-style`. web-core ignores
//! all ten as well — `BlurRadius` observes `blur-radius` alone — so ignoring
//! them is the compat target rather than a gap.
//!
//! Backgrounds and borders on the element paint normally, which is web-core's
//! behavior (the host element is an ordinary box under its shadow `<slot>`)
//! and not iOS's, where `LynxUIBlurView` overrides every `background-*` setter
//! with an empty body because the view *is* a `UIVisualEffectView`. Recorded
//! in `docs/tracking/deviations.md`.

use std::borrow::Cow;

use dom::{CustomElement, NodeId};

use super::LynxDocument;

/// Native's tag, and the tag a compiled `.web.bundle` writes.
pub(super) const BLUR_VIEW_TAG: &str = "blur-view";
pub(super) const X_BLUR_VIEW_TAG: &str = "x-blur-view";

const BLUR_RADIUS_ATTRIBUTE: &str = "blur-radius";
const BACKDROP_FILTER: &str = "backdrop-filter";

/// Installs the component under both tag names. Must run before any element
/// could carry either, which is [`Document::define`](dom::Document::define)'s
/// own precondition.
pub(super) fn define(document: &mut LynxDocument) {
    for tag in [BLUR_VIEW_TAG, X_BLUR_VIEW_TAG] {
        document.define(tag, Box::new(BlurView));
    }
}

/// Reflects `blur-radius` into the element's `backdrop-filter` hint.
///
/// `attribute_changed_callback` is the whole component, for the same reason it
/// is the whole of [`super::image`]'s: `__CreateElement` mints the element
/// before `__SetAttribute` writes on it, so `constructed` could observe
/// nothing, and a removal frees nothing that a disconnect would have to undo.
struct BlurView;

impl CustomElement<()> for BlurView {
    fn observed_attributes(&self) -> Vec<String> {
        vec![BLUR_RADIUS_ATTRIBUTE.to_owned()]
    }

    fn attribute_changed_callback(
        &self,
        document: &mut LynxDocument,
        element: NodeId,
        name: &str,
        _old: Option<&str>,
        new: Option<&str>,
    ) {
        debug_assert_eq!(
            name, BLUR_RADIUS_ATTRIBUTE,
            "`blur-view` observes `blur-radius` alone"
        );
        // Clear before setting: a value the grammar rejects leaves the hint
        // alone rather than removing it, so the previous radius would survive
        // `blur-radius="abc"`. See the module docs.
        document.set_presentational_hint(element, BACKDROP_FILTER, "");
        let Some(radius) = new.map(str::trim).filter(|value| !value.is_empty()) else {
            return;
        };
        document.set_presentational_hint(
            element,
            BACKDROP_FILTER,
            &format!("blur({})", blur_length(radius)),
        );
    }
}

/// The CSS length the attribute's text names.
///
/// A finite, non-negative bare number is the only value rewritten, and it is
/// rewritten to pixels: web-core's `parseFloat(v) + 'px'`, iOS's `doubleValue`
/// read as points and Android's `UnitUtils` fallthrough (`Float.parseFloat` of
/// the whole string) all agree that an unsuffixed `blur-radius` is pixels,
/// while CSS has no unitless non-zero length. Everything else passes through
/// for the cascade to accept or reject — a unit this engine knows resolves, and
/// `abc`, `-4px` or `10qq` reach no hint at all.
///
/// Reflecting the *parsed* number rather than appending `px` to the text is
/// what keeps the number spellings CSS does not share honest: `5.` is a number
/// to `parseFloat` and to Rust but not a CSS one, so glued text would have
/// produced a declaration the cascade throws away where both references blur
/// by 5. It also keeps `inf` and `NaN` — which `f32::from_str` accepts and
/// `parseFloat` does not — out of the declaration entirely.
fn blur_length(value: &str) -> Cow<'_, str> {
    match value.parse::<f32>() {
        Ok(number) if number.is_finite() && number >= 0.0 => Cow::Owned(format!("{number}px")),
        _ => Cow::Borrowed(value),
    }
}

#[cfg(test)]
mod tests {
    use dom::NodeId;
    use dom::stylo::values::computed::Filter;

    use super::super::LynxDocument;
    use super::super::test_support::{child, document, element_under, style_of};
    use super::{BLUR_RADIUS_ATTRIBUTE, BLUR_VIEW_TAG, X_BLUR_VIEW_TAG};

    /// Every tag this module defines.
    const TAGS: [&str; 2] = [BLUR_VIEW_TAG, X_BLUR_VIEW_TAG];

    /// `1rpx` is the viewport width over 750, and every viewport-derived
    /// length truncates onto Stylo's 1/60 px `Au` grid:
    /// `(393 * 60 * 20 / 750).trunc() / 60`. That truncation is why this is
    /// not `20 * 393 / 750 = 10.48`.
    const TWENTY_RPX: f32 = 628.0 / 60.0;
    /// The same arithmetic with `vw`'s denominator: `(393 * 60 * 2 / 100)`
    /// truncates to 471 au, not the 471.6 that `393 * 0.02` would give.
    const TWO_VW: f32 = 471.0 / 60.0;

    /// The one `blur()` radius the element's computed `backdrop-filter`
    /// carries, in CSS pixels, or `None` for the initial `none`.
    fn blur_radius(document: &LynxDocument, element: NodeId) -> Option<f32> {
        let style = style_of(document, element);
        let filters = &style.get_effects().backdrop_filter.0;
        assert!(
            filters.len() <= 1,
            "the reflection writes one filter function, not {filters:?}"
        );
        filters.first().map(|filter| match filter {
            Filter::Blur(radius) => radius.0.px(),
            other => panic!("the reflection writes `blur()`, not {other:?}"),
        })
    }

    /// A blur view with `blur-radius` written the way script writes it, laid
    /// out so its computed style is available.
    fn blurred(document: &mut LynxDocument, tag: &str, style: &str, radius: &str) -> NodeId {
        let element = child(document, tag, style);
        document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, radius);
        document.layout();
        element
    }

    #[test]
    fn a_bare_radius_becomes_one_blur_in_pixels() {
        for tag in TAGS {
            let mut document = document();
            let element = blurred(&mut document, tag, "", "25");
            assert_eq!(
                blur_radius(&document, element),
                Some(25.0),
                "web-core's `parseFloat` and every native platform read an \
                 unsuffixed radius as pixels: {tag}"
            );
        }
    }

    /// The bare-number path reflects the parsed *number*, not the text, so
    /// however the page spelled it the radius is the same. `5.` is the one in
    /// this list CSS itself would have rejected with `px` glued on — the
    /// others pin that nothing else about the spelling survives either.
    #[test]
    fn a_number_is_a_radius_however_it_is_spelled() {
        for (written, expected) in [
            ("5.", 5.0),
            ("+5", 5.0),
            (".5", 0.5),
            ("5e0", 5.0),
            ("0", 0.0),
        ] {
            let mut document = document();
            let element = blurred(&mut document, BLUR_VIEW_TAG, "", written);
            assert_eq!(
                blur_radius(&document, element),
                Some(expected),
                "{written:?} is a number both references read as pixels"
            );
        }
    }

    /// The ruling this module exists to implement: the value is a CSS length,
    /// so the cascade resolves its unit instead of a `parseFloat` dropping it.
    #[test]
    fn a_unit_resolves_through_the_cascade() {
        let mut document = document();
        let relative = blurred(&mut document, BLUR_VIEW_TAG, "font-size: 20px", "1.5em");
        let viewport = blurred(&mut document, BLUR_VIEW_TAG, "", "20rpx");
        let percent = blurred(&mut document, BLUR_VIEW_TAG, "", "2vw");
        document.layout();

        assert_eq!(
            blur_radius(&document, relative),
            Some(30.0),
            "`em` resolves against the element's own font size"
        );
        let rpx = blur_radius(&document, viewport).expect("`rpx` is a length the fork knows");
        assert!(
            (rpx - TWENTY_RPX).abs() < 1e-4,
            "1rpx = viewport width / 750: {rpx} is not {TWENTY_RPX}"
        );
        let vw = blur_radius(&document, percent).expect("`vw` is a length");
        assert!(
            (vw - TWO_VW).abs() < 1e-4,
            "`vw` resolves against the viewport: {vw} is not {TWO_VW}"
        );
    }

    /// A removal, an empty value and a value the grammar rejects all mean no
    /// blur — and they have to mean it *after* a valid radius, which is the
    /// case a plain `set_presentational_hint` would get wrong.
    #[test]
    fn a_missing_or_rejected_radius_leaves_no_backdrop_filter() {
        for tag in TAGS {
            let mut document = document();
            let element = child(&mut document, tag, "");
            document.layout();
            assert_eq!(
                blur_radius(&document, element),
                None,
                "a blur view with no attribute filters nothing: {tag}"
            );

            document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, "25");
            document.remove_attribute(element, BLUR_RADIUS_ATTRIBUTE);
            document.layout();
            assert_eq!(
                blur_radius(&document, element),
                None,
                "removing the attribute removes the hint: {tag}"
            );

            // `none` is in the list on purpose: the attribute names a radius,
            // never a whole `backdrop-filter` value, so the CSS keyword that
            // would have parsed as one is text like any other.
            for rejected in [
                "", " ", "abc", "-4px", "-4", "10qq", "inf", "NaN", "none", "25px)",
            ] {
                document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, "25");
                document.layout();
                assert_eq!(blur_radius(&document, element), Some(25.0), "{tag}");

                document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, rejected);
                document.layout();
                assert_eq!(
                    blur_radius(&document, element),
                    None,
                    "a rejected radius clears the hint rather than keeping the \
                     one before it: {tag} {rejected:?}"
                );
            }
        }
    }

    /// The value reaches the cascade as one declaration's value, never as
    /// declaration *text*, so a radius carrying `;` or `)` cannot open a
    /// second declaration: the whole value fails to parse and the hint stays
    /// empty.
    #[test]
    fn a_radius_cannot_smuggle_in_a_second_declaration() {
        let mut document = document();
        let element = blurred(
            &mut document,
            BLUR_VIEW_TAG,
            "",
            "10px); background-color: rgb(255 0 0",
        );

        assert_eq!(blur_radius(&document, element), None);
        assert_eq!(
            style_of(&document, element)
                .get_background()
                .background_color,
            dom::stylo::values::computed::Color::TRANSPARENT_BLACK,
            "nothing but `backdrop-filter` is ever written"
        );
    }

    /// Whitespace around the value is the page's formatting, not part of the
    /// length — and `trim` is the only normalization the reflection does.
    #[test]
    fn a_padded_radius_is_the_same_radius() {
        let mut document = document();
        let element = blurred(&mut document, BLUR_VIEW_TAG, "", "  7px\n");
        assert_eq!(blur_radius(&document, element), Some(7.0));
    }

    /// The hint is at [`CascadeOrigin::PresHints`], so the author wins — which
    /// is also what web-core's per-instance `:host` rule does.
    #[test]
    fn author_style_outranks_the_attribute() {
        let mut document = document();
        let element = blurred(
            &mut document,
            BLUR_VIEW_TAG,
            "backdrop-filter: blur(2px)",
            "25",
        );
        assert_eq!(
            blur_radius(&document, element),
            Some(2.0),
            "inline style outranks a presentational hint"
        );

        let sheeted = blurred(&mut document, X_BLUR_VIEW_TAG, "", "25");
        document.set_attribute(sheeted, "class", "frosted");
        document.add_stylesheet(
            ".frosted { backdrop-filter: blur(3px); }",
            dom::StylesheetOrigin::Author,
        );
        document.layout();
        assert_eq!(
            blur_radius(&document, sheeted),
            Some(3.0),
            "an author rule outranks a presentational hint too"
        );
    }

    /// The attribute reflection is the tag's, which is why it is a
    /// `CustomElement` and not an attribute hook keyed on the name: a
    /// `blur-radius` written on a `view` — or on an `image`, where web-core
    /// spends the very same attribute name on a `filter: blur()` of the
    /// picture itself through `--blur-radius` (`XImage/ImageSrc.ts:40-43`,
    /// `x-image.css:20-22`) — must not blur a backdrop.
    #[test]
    fn a_blur_radius_on_another_tag_is_not_a_backdrop_filter() {
        let mut document = document();
        for tag in ["view", "image", "text", "scroll-view"] {
            let element = child(&mut document, tag, "");
            document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, "25");
            document.layout();
            assert_eq!(blur_radius(&document, element), None, "{tag}");
        }
    }

    /// A blurred box is a stacking context and an absolute/fixed containing
    /// block, filter-effects-2 §2.1, and it has to become one from the
    /// *attribute* just as it does from author CSS. The fixed child is the
    /// observable half of that: `rounded_layout` reports a parent-relative
    /// location, so a child resolving against the viewport instead would land
    /// at `10 - 60 = -50`.
    #[test]
    fn a_blurred_box_captures_its_fixed_descendants() {
        for (radius, expected) in [("25", 10.0), ("", -50.0)] {
            let mut document = document();
            let host = blurred(
                &mut document,
                BLUR_VIEW_TAG,
                "width: 200px; height: 120px; margin-left: 60px",
                radius,
            );
            let fixed = element_under(
                &mut document,
                host,
                "view",
                "position: fixed; left: 10px; top: 20px; width: 30px; height: 40px",
            );
            document.layout();

            let layout = document
                .rounded_layout(fixed)
                .expect("the child is laid out");
            assert!(
                (layout.location.x - expected).abs() < f32::EPSILON,
                "radius {radius:?}: {} is not {expected}",
                layout.location.x
            );
        }
    }

    /// Children are ordinary content: web-core's template is
    /// `<style id="dynamic-style"></style><slot></slot>`, so everything
    /// written inside a blur view renders, and native's `insertChild` puts
    /// each child view into the effect view's `contentView`.
    #[test]
    fn a_blur_view_lays_its_children_out() {
        for tag in TAGS {
            let mut document = document();
            let host = blurred(&mut document, tag, "width: 200px; height: 120px", "25");
            let inner = element_under(&mut document, host, "view", "width: 40px; height: 30px");
            document.layout();

            let layout = document.rounded_layout(inner).expect("laid out");
            assert!(
                (layout.size.width - 40.0).abs() < f32::EPSILON
                    && (layout.size.height - 30.0).abs() < f32::EPSILON,
                "a blur view is a container, not a leaf: {tag}"
            );
        }
    }

    /// Both tags are one component, so a page can write either and a bundle
    /// mixing them is not a special case.
    #[test]
    fn the_two_tag_names_behave_identically() {
        let mut document = document();
        let native = child(&mut document, BLUR_VIEW_TAG, "");
        let web = child(&mut document, X_BLUR_VIEW_TAG, "");
        for element in [native, web] {
            document.set_attribute(element, BLUR_RADIUS_ATTRIBUTE, "12px");
        }
        document.layout();

        assert_eq!(blur_radius(&document, native), Some(12.0));
        assert_eq!(
            blur_radius(&document, native),
            blur_radius(&document, web),
            "`blur-view` and `x-blur-view` are the same component"
        );
        assert_eq!(
            style_of(&document, native).clone_display(),
            style_of(&document, web).clone_display(),
            "and carry the same UA rules"
        );
    }
}
