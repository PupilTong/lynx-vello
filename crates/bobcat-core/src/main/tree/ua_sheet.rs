//! The main-thread Lynx UA cascade: page configuration, defaults, and
//! defaults every container tag shares, and the assembly of the one sheet.
//!
//! Each tag's own policy lives with that tag — [`super::scroll_container`],
//! [`super::list`], [`super::viewpager`], [`super::swiper`],
//! [`super::refresh_view`], [`super::scroll_coordinator`], [`super::dialog`],
//! [`super::popover`], [`super::overlay`], [`super::text`], [`super::raw_text`],
//! [`super::image`] — and this module
//! only decides what they all agree on and what order they land in.
//! [`super::blur_view`] is the one tag module with no rules of its own: a
//! blur view is a container and nothing more, so everything it needs is here.
//! `svg` has no tag module either: it is the standard element, implemented
//! in `dom` (an inline SVG root, replaced content whose subtree is parsed as
//! one vector image), and its one rule here, `svg { display: flex; }` with
//! `svg` in the shared box block, makes it lay out the way `<image>` does.
//! Nothing hides its children: being replaced content already does.
//!
//! # HTML's rules under the Lynx tags'
//!
//! Under web-core a page has two sheets of defaults: the browser's UA sheet,
//! and web-elements' CSS, which is *author* origin and so outranks every UA
//! rule whatever its specificity. This sheet is one origin for both, so it
//! keeps that order by specificity: every HTML rule — [`super::dialog`]'s,
//! [`super::popover`]'s and the display `dialog` maps HTML's `block` to — has
//! its selector inside `:where()`, so it has none (a `::backdrop` rule keeps
//! the pseudo-element's own), and every Lynx rule names a tag, so it outranks
//! all of them. A `<view popover>` therefore keeps its Lynx `display` against
//! `[popover]`'s `display: none`, as it does in a browser under
//! web-elements' `x-view` rule.
//!
//! Among themselves the HTML rules then rank by source order alone, and the
//! sheet orders them so that this is HTML's own precedence: `dialog`'s
//! `display` first; then [`super::dialog`]'s rules, in HTML's order; then
//! [`super::popover`]'s, whose `[popover]` box beats `dialog`'s as its
//! higher specificity does in HTML; then `dialog:popover-open`'s `display`,
//! after the closed `dialog`'s `none`, as in HTML. The one pair whose order
//! here is the reverse of HTML's, `dialog:modal` before `[popover]`, writes
//! the same values for every property both declare (`position: fixed`, the
//! insets at 0, `overflow: scroll`), so no element can tell. HTML's one
//! `!important`, the popover `::backdrop`'s `pointer-events`, beats every
//! Lynx rule, as a UA `!important` beats an author one.
//!
//! This is a cascade layer below the Lynx rules, written by hand: an
//! `@layer` in a UA sheet trips Stylo's rule tree, whose root node carries
//! an unlayered UA rule's priority, so the first layered rule under it fails
//! its ordering assertion.
//!
//! Order within the Lynx rules is mostly documentation, with one exception
//! that is mechanism: [`super::image`]'s child suppression ties on specificity with
//! the `display` rules `view`, `scroll-view`, `list`, `list-item`, the two
//! spellings each of `viewpager` and `viewpager-item`, `x-swiper`,
//! `x-swiper-item`, `x-refresh-view`, `x-refresh-header`, `x-refresh-footer`,
//! the ten `scroll-coordinator` tags, `blur-view`, `x-blur-view`, `overlay`,
//! `x-overlay-ng` (their hosts' `display: contents`) and `wrapper` carry, so
//! it wins only by being assembled last. That module's `nothing_inside_an_image_generates_a_box` is
//! the tripwire for it.

use super::blur_view::{BLUR_VIEW_TAG, X_BLUR_VIEW_TAG};
use super::dialog::DIALOG_TAG;
use super::refresh_view::{REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG};
use super::swiper::{SWIPER_ITEM_TAG, SWIPER_TAG};
use super::viewpager::{VIEWPAGER_ITEM_TAG, VIEWPAGER_TAG, X_VIEWPAGER_ITEM_TAG, X_VIEWPAGER_TAG};
use super::{
    dialog, image, list, overlay, popover, raw_text, refresh_view, scroll_container,
    scroll_coordinator, swiper, text, viewpager,
};

/// Page configuration for the Lynx runtime and UA cascade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent Lynx page configuration switches"
)]
pub struct PageConfig {
    /// Whether elements default to `display: linear`.
    pub default_display_linear: bool,
    /// Whether elements default to visible overflow.
    pub default_overflow_visible: bool,
    /// Whether author CSS selector matching is enabled.
    pub enable_css_selector: bool,
    /// Pass data and its processor name to BTS without running the MTS processor.
    pub enable_js_data_processor: bool,
    /// Whether an element's `exposure-ui-margin-*` attributes take part in
    /// exposure detection when the element names no
    /// `enable-exposure-ui-margin` of its own: native's page-level
    /// `enableExposureUIMargin` (`LynxBaseUI.getEnableExposureUIMargin`,
    /// which falls back to the context's switch), false unless the page says
    /// otherwise. The UA cascade does not read it; the runtime's exposure
    /// detection does (`crate::main::exposure`).
    pub enable_exposure_ui_margin: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
            enable_js_data_processor: false,
            enable_exposure_ui_margin: false,
        }
    }
}

/// The Lynx UA stylesheet: embedder cascade policy `dom` must not know.
///
/// The container tags — `page`, `view`, `scroll-view`, `list`, `list-item`,
/// `viewpager`, `x-viewpager-ng`, `viewpager-item`, `x-viewpager-item-ng`,
/// `x-swiper`, `x-swiper-item`, `x-refresh-header`, `x-refresh-footer`, the five
/// `scroll-coordinator` roles under both spellings
/// (`scroll-coordinator`, `-header`, `-toolbar`, `-slot`, `-slot-drag`, and
/// web-core's `x-foldview-ng`, `x-foldview-header-ng`, …),
/// `blur-view` and `x-blur-view` — share
/// `web-elements`' common block: a border box, and the display mode
/// `defaultDisplayLinear` picks — a per-tag exception to that switch would
/// have to be `!important`, so it is a recorded deviation instead
/// (`docs/tracking/deviations.md`). `text` is a text block whatever the switch
/// says, and `wrapper` generates no box — both from `web-elements`' own sheet,
/// where the linear toggle covers container tags only.
/// `dialog` follows the switch's display too — HTML's `block` has no box in
/// this engine, and the switch is what picks a page's block-like container —
/// and nothing else of the common block: a browser gives it HTML's defaults
/// ([`super::dialog`]). That display is HTML's, so it is written with
/// HTML's other rules, at zero specificity, for `dialog` and for
/// `dialog:popover-open` ([`super::popover`]), rather than in the Lynx
/// display line.
/// `overlay` and `x-overlay-ng` are in neither: their host is `display:
/// contents`, as web-core's, and what renders is the `dialog` in their shadow
/// tree, which takes the switch's display as `dialog` ([`super::overlay`]).
/// Every tag that generates a box of its own — the containers, `text` and
/// `image` — also gets the rest of that common block: `border-width: 0` with
/// `border-style: solid`, `position: relative` (which is what makes a
/// coordinator the containing block of its header and slot,
/// [`super::scroll_coordinator`]), `min-width: 0` and
/// `min-height: 0`, and `overflow: clip`. `clip` is not a scroll container,
/// so a clipped box neither scrolls nor gets its automatic minimum size
/// zeroed by the overflow — the explicit `min-width`/`min-height` are what do
/// that. `defaultOverflowVisible` releases the non-scrolling containers —
/// `page`, `view` and the two blur-view tags — back to `visible`, the way
/// web-core's `[lynx-default-overflow-visible=true] x-view` releases `x-view`
/// alone; a scroller carries its own axes regardless, and a `list-item`, a
/// pager's page, a swiper's item or a refresh view's header or footer stays
/// clipped — by this `clip`, and the
/// first two also by the paint containment [`super::list`] and
/// [`super::viewpager`] give them.
///
/// The blur-view tags are here because native's `LynxUIBlurView` extends
/// `LynxUIView`: a blur view is a view in everything layout can see, and its
/// `blur-radius` is the only thing that makes it different
/// ([`super::blur_view`]). web-core is narrower — `x-blur-view` is in
/// `linear.css`'s common block but in neither the `--lynx-display-toggle` list
/// nor the `[lynx-default-overflow-visible=true] x-view` escape, so a browser
/// gives it a row flex box that always clips — which is the divergence
/// `scroll-view` and `list` already record.
///
/// Almost nothing here is `!important`. web-elements' defaults are author
/// origin in the browser and several of them lean on `!important`; ours are
/// user-agent origin, where an important declaration outranks author
/// `!important` — and inline style — instead of losing to them, so what
/// web-elements merely forces is written here as a plain declaration a page's
/// own CSS can still override (`docs/style-assumptions.md` §D.15).
///
/// The exceptions are all of the same shape: a fact the cascade is merely
/// *reporting* rather than a default it is *choosing*.
/// `display: -lynx-text` on `text`, on `inline-text` and on a `text`'s own
/// `inline-truncation` child is the first three — Lynx does not decide
/// inline-ness by cascade at all: `ConvertToInlineElement` runs when a child is
/// *added* to a text, and no author CSS can undo it. `padding: 0` on an
/// `image` that is inline content of a paragraph is the fourth: web-core gives
/// the authored element no box at all and rebuilds one in its shadow tree
/// without inheriting `padding`, so there is no box for an author's padding to
/// reach, and a declaration a page could override would misdescribe both
/// references. [`super::text`] carries the citation for each. The pager's
/// row is the fifth and sixth: its pages form one row in both references
/// whatever main axis an author writes on it, so its `flex-direction`,
/// `linear-direction` and `flex-wrap`, and its pages' `position`, are pinned
/// ([`super::viewpager`] carries the argument). The swiper's items are six
/// more, after the pager's: their main-axis size in each of its layouts,
/// which web-core itself pins (the swiper's own main axis is on its shadow
/// `#content`, out of every author rule's reach; [`super::swiper`] carries
/// the argument). The refresh view's are three more, after the swiper's:
/// its header's and its footer's `position: relative`, which web-core itself
/// pins, and `overscroll-behavior-y: auto` on its content children, without
/// which a content child that contains its chain would keep every pull from
/// the shadow scroller ([`super::refresh_view`] carries the argument). The
/// coordinator's structure
/// is six more, between the refresh view's and the text block's in the sheet: its
/// `overflow-y: scroll` and its column, the `overflow-y: hidden` that
/// `enable-scroll="false"` needs to beat that scroll, and the header's, the
/// toolbar's and the slot's positions, which its geometry is built from
/// ([`super::scroll_coordinator`] carries the argument). HTML's own is one
/// more, after the coordinator's, among HTML's rules: a popover's
/// `::backdrop` is `pointer-events: none !important`, as in a browser
/// ([`super::popover`]). The overlay's is one more, after it: only an
/// overlay's first child renders, which web-core itself pins and native's
/// measurement of child 0 alone agrees with ([`super::overlay`] carries the
/// argument). `the_ua_sheet_is_important_free_apart_from_the_text_block`
/// pins the set to exactly those twenty-three rules.
#[must_use]
pub(super) fn ua_stylesheet(config: PageConfig) -> String {
    let component_tags = format!(
        "{VIEWPAGER_TAG}, {X_VIEWPAGER_TAG}, {VIEWPAGER_ITEM_TAG}, {X_VIEWPAGER_ITEM_TAG}, \
         {SWIPER_TAG}, {SWIPER_ITEM_TAG}, {REFRESH_HEADER_TAG}, {REFRESH_FOOTER_TAG}, {}",
        scroll_coordinator::TAGS.join(", ")
    );
    let display = if config.default_display_linear {
        format!(
            "page, view, scroll-view, list, list-item, {component_tags}, {BLUR_VIEW_TAG}, \
             {X_BLUR_VIEW_TAG} {{ display: linear; }}\n"
        )
    } else {
        String::new()
    };
    // HTML's `display: block`, which no box here lowers to.
    let block = if config.default_display_linear {
        "linear"
    } else {
        "flex"
    };
    let overflow = if config.default_overflow_visible {
        format!("page, view, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG} {{ overflow: visible; }}\n")
    } else {
        String::new()
    };
    format!(
        "page, view, scroll-view, list, list-item, {component_tags}, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG}, \
         text, image, svg {{ box-sizing: border-box; border-width: 0; border-style: solid; \
         position: relative; overflow: clip; min-width: 0; min-height: 0; }}\n\
         {display}\
         {overflow}\
         page {{ width: 100%; height: 100%; font-family: sans-serif; }}\n\
         wrapper {{ display: contents; }}\n\
         svg {{ display: flex; }}\n\
         {scrollers}\
         {lists}\
         {pagers}\
         {swipers}\
         {refresh_views}\
         {coordinators}\
         :where({DIALOG_TAG}) {{ display: {block}; }}\n\
         {dialogs}\
         {popovers}\
         :where({DIALOG_TAG}:popover-open) {{ display: {block}; }}\n\
         {overlays}\
         {text}\
         {carriers}\
         {images}",
        scrollers = scroll_container::UA_RULES,
        lists = list::UA_RULES,
        pagers = viewpager::UA_RULES,
        swipers = swiper::UA_RULES,
        refresh_views = refresh_view::UA_RULES,
        coordinators = scroll_coordinator::UA_RULES,
        dialogs = dialog::UA_RULES,
        popovers = popover::UA_RULES,
        overlays = overlay::UA_RULES,
        text = text::UA_RULES,
        carriers = raw_text::UA_RULES,
        images = image::UA_RULES,
    )
}

#[cfg(test)]
mod tests {
    use dom::stylo::computed_values::{box_sizing, position};
    use dom::stylo::values::computed::font::{FontFamily, GenericFontFamily};
    use dom::stylo::values::computed::{BorderStyle, CSSPixelLength, Display, Overflow, Size};

    use super::super::LynxDocument;
    use super::super::overlay::{OVERLAY_TAG, X_OVERLAY_TAG};
    use super::super::scroll_coordinator::{
        SCROLL_COORDINATOR_HEADER_TAG, SCROLL_COORDINATOR_SLOT_DRAG_TAG,
        SCROLL_COORDINATOR_SLOT_TAG, SCROLL_COORDINATOR_TAG, SCROLL_COORDINATOR_TOOLBAR_TAG,
        X_FOLDVIEW_HEADER_TAG, X_FOLDVIEW_SLOT_DRAG_TAG, X_FOLDVIEW_SLOT_TAG, X_FOLDVIEW_TAG,
        X_FOLDVIEW_TOOLBAR_TAG,
    };
    use super::super::test_support::{child, document, overflow, style_of, with_config};
    use super::{
        BLUR_VIEW_TAG, DIALOG_TAG, PageConfig, REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG,
        SWIPER_ITEM_TAG, SWIPER_TAG, VIEWPAGER_ITEM_TAG, VIEWPAGER_TAG, X_BLUR_VIEW_TAG,
        X_VIEWPAGER_ITEM_TAG, X_VIEWPAGER_TAG, ua_stylesheet,
    };

    /// The tags that get `web-elements`' common container block and keep
    /// its `position: relative`. The coordinator's header, toolbar and slot
    /// get the block too, but their positions are pinned
    /// ([`super::super::scroll_coordinator`] tests them).
    const CONTAINER_TAGS: [&str; 17] = [
        "page",
        "view",
        "scroll-view",
        "list",
        "list-item",
        VIEWPAGER_TAG,
        X_VIEWPAGER_TAG,
        VIEWPAGER_ITEM_TAG,
        X_VIEWPAGER_ITEM_TAG,
        SWIPER_TAG,
        SWIPER_ITEM_TAG,
        SCROLL_COORDINATOR_TAG,
        X_FOLDVIEW_TAG,
        SCROLL_COORDINATOR_SLOT_DRAG_TAG,
        X_FOLDVIEW_SLOT_DRAG_TAG,
        BLUR_VIEW_TAG,
        X_BLUR_VIEW_TAG,
    ];

    /// Attaches one of each container tag, answering with the page itself for
    /// `page` — it is minted with the document and cannot be created again.
    fn containers(document: &mut LynxDocument) -> Vec<dom::NodeId> {
        CONTAINER_TAGS
            .iter()
            .map(|tag| {
                if *tag == "page" {
                    document.document_element().id()
                } else {
                    child(document, tag, "")
                }
            })
            .collect()
    }

    #[test]
    fn the_ua_sheet_gives_every_container_tag_lynx_defaults() {
        let mut document = document();
        let containers = containers(&mut document);
        document.layout();

        for (tag, container) in CONTAINER_TAGS.iter().zip(containers) {
            let style = style_of(&document, container);
            assert_eq!(*style.get_box_sizing(), box_sizing::T::BorderBox, "{tag}");
            assert_eq!(*style.get_display(), Display::Linear, "{tag}");
        }
    }

    /// `svg` is a flex box under either display configuration (it is outside
    /// the `defaultDisplayLinear` list), and an inline root lays out at its
    /// `width`/`height` attributes with no box for anything inside it.
    #[test]
    fn an_svg_is_a_flex_leaf_sized_by_its_attributes() {
        for linear in [true, false] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let svg = child(&mut document, "svg", "");
            document.set_attribute(svg, "viewBox", "0 0 24 24");
            document.set_attribute(svg, "width", "48");
            document.set_attribute(svg, "height", "32");
            let path = document.create_element("path", ());
            document.set_attribute(path, "d", "M0 0 H24 V24 Z");
            document.append_child(svg, path);
            document.layout();

            assert_eq!(
                *style_of(&document, svg).get_display(),
                Display::Flex,
                "linear={linear}"
            );
            let layout = document.rounded_layout(svg).expect("the svg is laid out");
            assert_eq!(
                (layout.size.width, layout.size.height),
                (48.0, 32.0),
                "linear={linear}"
            );
            assert!(
                document
                    .rounded_layout(path)
                    .is_none_or(|layout| layout.size.width == 0.0 && layout.size.height == 0.0),
                "a replaced svg's children generate no box: linear={linear}"
            );
        }
    }

    /// A definite `min-width`/`min-height` in pixels, `None` for anything else.
    fn px(size: &Size) -> Option<f32> {
        match size {
            Size::LengthPercentage(length) => length.0.to_length().map(CSSPixelLength::px),
            _ => None,
        }
    }

    /// `web-elements`' common block beyond `display`, on every tag that
    /// generates a box of its own (`common-css/linear.css`).
    #[test]
    fn every_box_tag_gets_the_web_elements_common_block() {
        let mut document = with_config(PageConfig {
            default_overflow_visible: false,
            ..PageConfig::default()
        });
        let mut boxes = containers(&mut document);
        boxes.push(child(&mut document, "text", ""));
        boxes.push(child(&mut document, "image", ""));
        boxes.push(child(&mut document, "svg", ""));
        document.layout();

        for (tag, element) in CONTAINER_TAGS
            .iter()
            .chain(&["text", "image", "svg"])
            .zip(boxes)
        {
            let style = style_of(&document, element);
            assert_eq!(*style.get_box_sizing(), box_sizing::T::BorderBox, "{tag}");
            let border = style.get_border();
            for (side_style, side_width) in [
                (
                    *border.get_border_top_style(),
                    border.get_border_top_width(),
                ),
                (
                    *border.get_border_right_style(),
                    border.get_border_right_width(),
                ),
                (
                    *border.get_border_bottom_style(),
                    border.get_border_bottom_width(),
                ),
                (
                    *border.get_border_left_style(),
                    border.get_border_left_width(),
                ),
            ] {
                assert_eq!(side_style, BorderStyle::Solid, "{tag}");
                assert_eq!(side_width.0.to_px(), 0, "{tag}");
            }
            assert_eq!(
                *style.get_box().get_position(),
                position::T::Relative,
                "{tag}"
            );
            assert_eq!(px(style.get_min_width()), Some(0.0), "{tag}");
            assert_eq!(px(style.get_min_height()), Some(0.0), "{tag}");
        }
    }

    /// `solid` is only the style a page's `border-width` reaches; the page can
    /// still restyle, reposition and re-floor every box.
    #[test]
    fn the_common_block_is_author_overridable() {
        let mut document = document();
        let view = child(
            &mut document,
            "view",
            "border-width: 2px; position: absolute; min-width: 10px; overflow: hidden",
        );
        document.layout();

        let style = style_of(&document, view);
        assert_eq!(
            *style.get_border().get_border_top_style(),
            BorderStyle::Solid
        );
        assert_eq!(style.get_border().get_border_top_width().0.to_px(), 2);
        assert_eq!(*style.get_box().get_position(), position::T::Absolute);
        assert_eq!(px(style.get_min_width()), Some(10.0));
        assert_eq!(
            overflow(&document, view),
            (Overflow::Hidden, Overflow::Hidden)
        );
    }

    #[test]
    fn the_default_font_family_is_inherited_and_author_overridable() {
        let mut document = document();
        let page = document.document_element().id();
        let view = child(&mut document, "view", "");
        let text = super::super::test_support::element_under(&mut document, view, "text", "");

        for (css, family) in [
            ("", GenericFontFamily::SansSerif),
            (
                "page { font-family: system-ui; }",
                GenericFontFamily::SystemUi,
            ),
        ] {
            document.add_stylesheet(css, dom::StylesheetOrigin::Author);
            document.layout();
            for node in [page, view, text] {
                assert_eq!(
                    style_of(&document, node).get_font().get_font_family(),
                    FontFamily::generic(family),
                );
            }
        }
    }

    #[test]
    fn the_display_page_config_switch_reaches_every_container_tag() {
        let mut document = with_config(PageConfig {
            default_display_linear: false,
            ..PageConfig::default()
        });
        let containers = containers(&mut document);
        document.layout();

        for (tag, container) in CONTAINER_TAGS.iter().zip(containers) {
            let style = style_of(&document, container);
            assert_eq!(
                *style.get_display(),
                Display::Flex,
                "a scroller follows `defaultDisplayLinear` the way a view does: {tag}"
            );
            assert_eq!(*style.get_box_sizing(), box_sizing::T::BorderBox, "{tag}");
        }
    }

    /// The top-layer component tags take nothing of the common container
    /// block: no border box, no `position: relative`, no `overflow: clip`.
    /// `dialog` takes the display `defaultDisplayLinear` picks once it
    /// shows; the overlay's two hosts are `display: contents` under either
    /// switch, since their shadow `dialog` is what renders
    /// ([`super::super::overlay`]).
    #[test]
    fn the_display_page_config_switch_reaches_the_top_layer_tags() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                default_overflow_visible: false,
                ..PageConfig::default()
            });
            let shown = [
                (DIALOG_TAG, "open", expected),
                (OVERLAY_TAG, "visible", Display::Contents),
                (X_OVERLAY_TAG, "visible", Display::Contents),
            ]
            .map(|(tag, attribute, display)| {
                let element = child(&mut document, tag, "");
                document.set_attribute(element, attribute, "");
                (tag, element, display)
            });
            document.layout();

            for (tag, element, expected) in shown {
                let style = style_of(&document, element);
                assert_eq!(*style.get_display(), expected, "{tag}: linear {linear}");
                assert_eq!(*style.get_box_sizing(), box_sizing::T::ContentBox, "{tag}");
                assert_ne!(
                    *style.get_box().get_position(),
                    position::T::Relative,
                    "{tag}"
                );
                assert_eq!(
                    overflow(&document, element),
                    (Overflow::Visible, Overflow::Visible),
                    "{tag}: not clipped, whatever `defaultOverflowVisible` says"
                );
            }
        }
    }

    /// Every box clips by default (`overflow: clip`, not a scroll container);
    /// the switch releases the containers that are not scrollers — `page`,
    /// `view` and the two blur-view tags, which native treats as views
    /// (`LynxUIBlurView` extends `LynxUIView`) — and nothing else.
    #[test]
    fn the_overflow_page_config_switch_skips_the_scrollers() {
        for (visible, expected) in [(true, Overflow::Visible), (false, Overflow::Clip)] {
            let mut document = with_config(PageConfig {
                default_overflow_visible: visible,
                ..PageConfig::default()
            });
            let views = ["view", BLUR_VIEW_TAG, X_BLUR_VIEW_TAG]
                .map(|tag| (tag, child(&mut document, tag, "")));
            let scroller = child(&mut document, "scroll-view", "");
            let list = child(&mut document, "list", "");
            let pagers = [VIEWPAGER_TAG, X_VIEWPAGER_TAG].map(|tag| child(&mut document, tag, ""));
            let coordinators =
                [SCROLL_COORDINATOR_TAG, X_FOLDVIEW_TAG].map(|tag| child(&mut document, tag, ""));
            // A swiper scrolls its shadow `#content`, so the host itself is
            // a box that clips (`super::super::swiper`).
            let leaves = [
                "text",
                "image",
                SWIPER_TAG,
                VIEWPAGER_ITEM_TAG,
                X_VIEWPAGER_ITEM_TAG,
                SWIPER_ITEM_TAG,
                REFRESH_HEADER_TAG,
                REFRESH_FOOTER_TAG,
                SCROLL_COORDINATOR_HEADER_TAG,
                X_FOLDVIEW_HEADER_TAG,
                SCROLL_COORDINATOR_TOOLBAR_TAG,
                X_FOLDVIEW_TOOLBAR_TAG,
                SCROLL_COORDINATOR_SLOT_TAG,
                X_FOLDVIEW_SLOT_TAG,
                SCROLL_COORDINATOR_SLOT_DRAG_TAG,
                X_FOLDVIEW_SLOT_DRAG_TAG,
            ]
            .map(|tag| (tag, child(&mut document, tag, "")));
            document.layout();

            assert_eq!(
                overflow(&document, document.document_element().id()),
                (expected, expected),
                "{visible}"
            );
            for (tag, view) in views {
                assert_eq!(
                    overflow(&document, view),
                    (expected, expected),
                    "{tag}: {visible}"
                );
            }
            for (tag, leaf) in leaves {
                assert_eq!(
                    overflow(&document, leaf),
                    (Overflow::Clip, Overflow::Clip),
                    "{tag} clips whatever the switch says: {visible}"
                );
            }
            for scroller in [scroller, list] {
                assert_eq!(
                    overflow(&document, scroller),
                    (Overflow::Hidden, Overflow::Scroll),
                    "a scroller keeps its own axes whatever the switch says: {visible}"
                );
            }
            for pager in pagers {
                assert_eq!(
                    overflow(&document, pager),
                    (Overflow::Scroll, Overflow::Hidden),
                    "a pager keeps its own axes whatever the switch says: {visible}"
                );
            }
            for coordinator in coordinators {
                assert_eq!(
                    overflow(&document, coordinator),
                    (Overflow::Hidden, Overflow::Scroll),
                    "a coordinator keeps its own axes whatever the switch says: {visible}"
                );
            }
        }
    }

    /// The whole rule `defaultOverflowVisible` gates, spelled out once.
    const OVERFLOW_RULE: &str = "page, view, blur-view, x-blur-view { overflow: visible; }";

    #[test]
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(sheet.contains(OVERFLOW_RULE));
        assert!(sheet.contains("box-sizing: border-box;"));
    }

    #[test]
    fn ua_switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
            enable_js_data_processor: false,
            enable_exposure_ui_margin: false,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(!sheet.contains(OVERFLOW_RULE));
    }

    /// The sheet's important declarations are exactly the text-block ones.
    ///
    /// A UA-origin `!important` outranks author `!important` *and* inline
    /// style, so every one of them removes a knob a page would otherwise
    /// have. `display: -lynx-text` is the recorded exception
    /// (`docs/style-assumptions.md` §D.15): in Lynx a `<text>`'s inline-ness
    /// is established structurally, at tree-mutation time, and no author CSS
    /// can undo it — so the cascade value naming it has to be equally
    /// unconditional. This pins the exception rather than dropping the
    /// invariant: a new important declaration fails here until someone
    /// decides it deserves the same argument.
    ///
    /// `text > inline-truncation` is the same argument a third time. The tag
    /// is `display: none` everywhere else, and a `text`'s own child is the
    /// only place Lynx treats it as the paragraph's custom truncation content
    /// — a structural role, established by where it is written rather than by
    /// what any sheet declares.
    ///
    /// The fourth, `padding: 0` on an inline `image`, is the argument reached
    /// from the other end. web-core gives the authored host element no box at
    /// all (`x-text > x-image { display: contents !important }`) and rebuilds
    /// one in the shadow tree out of an inherited property list `padding` is
    /// not on, so no author CSS there can make an inline image's padding
    /// matter. Here the authored element *is* the box, and a normal
    /// declaration would lose to the author's own `padding` — which would
    /// describe an engine neither reference has. [`super::text`] carries the
    /// full citation.
    ///
    /// The pager's row is two more, ahead of the text block in the
    /// sheet. In both references a
    /// `viewpager`'s pages form one row whatever the author writes on the
    /// pager — web-core lays them out in a shadow box no author rule reaches,
    /// native places them itself — and authors do write `display: flex;
    /// flex-direction: column` on it. Here the authored pager is the box that
    /// lays the pages out, so the row is either pinned in the cascade or
    /// lost; the same for a page's `position: relative`, which web-core
    /// itself pins with `!important`. [`super::viewpager`] carries the full
    /// argument.
    ///
    /// The swiper's eight follow the pager's, on the same argument: web-core
    /// lays its items out in a shadow box, pins `flex-wrap`, the column and
    /// each item's main-axis size itself, and here the authored swiper is the
    /// box. [`super::super::swiper`] carries the argument.
    ///
    /// The refresh view's three follow the swiper's. Its header's and its
    /// footer's `position: relative` are web-core's own `!important`, and
    /// authors write `position: absolute` on the header (web-core's demo
    /// card does): the header has to stay in the column above the content.
    /// `overscroll-behavior-y: auto` on its content children keeps a child
    /// that contains its chain from stopping the pull before the shadow
    /// scroller. [`super::super::refresh_view`] carries the argument.
    ///
    /// The coordinator's six follow the refresh view's. In every reference its
    /// header, toolbar and slot are placed by something no author rule
    /// reaches — web-core's `!important` scroll axis, header position and
    /// component code, native's own layout — and here the authored boxes
    /// are the layout: the coordinator must scroll on y in a column, the
    /// header and the slot must be absolutely positioned (the slot's
    /// `anchor-size()` resolves only there) and the toolbar sticky. The
    /// `enable-scroll="false"` line is important only to beat the pinned
    /// scroll. [`super::super::scroll_coordinator`] carries the argument.
    ///
    /// HTML's own follows the coordinator's: a popover's `::backdrop` is
    /// `pointer-events: none !important` in HTML's UA sheet, so it takes no
    /// touch whatever an author writes. [`super::super::popover`] carries it.
    ///
    /// The overlay's one follows HTML's. Only an overlay's first
    /// child renders: web-core pins every other child `display: none
    /// !important`, and native measures child 0 alone, so an author
    /// `display` on a second child must not bring it back.
    /// [`super::super::overlay`] carries the argument.
    #[test]
    fn the_ua_sheet_is_important_free_apart_from_the_text_block() {
        const ALLOWED: [&str; 23] = [
            "viewpager, x-viewpager-ng { flex-direction: row !important; \
             linear-direction: row !important; flex-wrap: nowrap !important; }",
            "viewpager-item, x-viewpager-item-ng { position: relative !important; }",
            "x-swiper > x-swiper-item { width: 100% !important; }",
            "x-swiper[vertical]:not([vertical=\"false\"]) > x-swiper-item { height: 100% !important; }",
            "x-swiper[mode=\"carousel\"]:is(:not([vertical]), [vertical=\"false\"]) > x-swiper-item \
             { width: 80% !important; }",
            "x-swiper[mode=\"carousel\"][vertical]:not([vertical=\"false\"]) > x-swiper-item \
             { height: 80% !important; }",
            "x-swiper:is([mode=\"flat-coverflow\"], [mode=\"coverflow\"]):is(:not([vertical]), \
             [vertical=\"false\"]) > x-swiper-item { width: 60% !important; }",
            "x-swiper:is([mode=\"flat-coverflow\"], [mode=\"coverflow\"])[vertical]:not([vertical=\"false\"]) \
             > x-swiper-item { height: 60% !important; }",
            "x-refresh-view:not([enable-refresh=\"false\"]) > x-refresh-header:first-of-type \
             { position: relative !important; }",
            "x-refresh-view:not([enable-loadmore=\"false\"]) > x-refresh-footer:first-of-type \
             { position: relative !important; }",
            "x-refresh-view > :not(x-refresh-header, x-refresh-footer) \
             { overscroll-behavior-y: auto !important; }",
            "scroll-coordinator, x-foldview-ng { overflow-y: scroll !important; }",
            "scroll-coordinator, x-foldview-ng { flex-direction: column !important; \
             linear-direction: column !important; }",
            "scroll-coordinator[enable-scroll=\"false\"], scroll-coordinator[scroll-enable=\"false\"], \
             x-foldview-ng[enable-scroll=\"false\"], x-foldview-ng[scroll-enable=\"false\"] \
             { overflow-y: hidden !important; }",
            "scroll-coordinator-header, x-foldview-header-ng { position: absolute !important; }",
            "scroll-coordinator-toolbar, x-foldview-toolbar-ng { position: sticky !important; }",
            "scroll-coordinator-slot, x-foldview-slot-ng { position: absolute !important; }",
            ":where(:popover-open)::backdrop { position: fixed; inset: 0; \
             pointer-events: none !important; background-color: transparent; }",
            "overlay > :not(:first-child), x-overlay-ng > :not(:first-child) \
             { display: none !important; }",
            "text { display: -lynx-text !important; color: initial; }",
            "inline-text { display: -lynx-text !important; }",
            "text > inline-truncation { display: -lynx-text !important; \
             --lynx-inline-truncation: 1; }",
            "text > image, text > wrapper > image, inline-text > image, \
             inline-text > wrapper > image, text > inline-truncation > image, \
             text > inline-truncation > wrapper > image { padding: 0 !important; }",
        ];

        for config in [
            PageConfig::default(),
            PageConfig {
                default_display_linear: false,
                default_overflow_visible: false,
                enable_css_selector: false,
                enable_js_data_processor: false,
                enable_exposure_ui_margin: false,
            },
        ] {
            let sheet = ua_stylesheet(config);
            let important: Vec<&str> = sheet.lines().filter(|line| line.contains('!')).collect();
            assert_eq!(
                important, ALLOWED,
                "a UA-origin important declaration outranks author `!important`, \
                 so the set of them is fixed by §D.15's recorded exceptions"
            );
        }
    }
}
