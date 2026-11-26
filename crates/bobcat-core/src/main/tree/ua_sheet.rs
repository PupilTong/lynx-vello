//! The main-thread Lynx UA cascade: page configuration, defaults, and
//! defaults every container tag shares, and the assembly of the one sheet.
//!
//! Each tag's own policy lives with that tag — [`super::scroll_container`],
//! [`super::list`], [`super::viewpager`], [`super::swiper`],
//! [`super::refresh_view`], [`super::scroll_coordinator`], [`super::overlay`],
//! [`super::text`], [`super::raw_text`], [`super::frame`], [`super::image`],
//! [`super::lynx_svg`], and HTML's [`terminus::link::html::dialog`] and
//! [`terminus::link::html::popover`] — and this module only decides what they all
//! agree on and what order they land in.
//! [`super::blur_view`] is the one tag module with no rules of its own: a
//! blur view is a container and nothing more, so everything it needs is here.
//!
//! # HTML's rules under the Lynx tags'
//!
//! Under web-core a page has two sheets of defaults: the browser's UA sheet,
//! and web-elements' CSS, which is *author* origin and so outranks every UA
//! rule whatever its specificity. This sheet is one origin for both, so it
//! keeps that order by specificity: every HTML rule —
//! [`terminus::link::html::dialog`]'s, [`terminus::link::html::popover`]'s and the
//! display `dialog` maps HTML's `block` to — has its selector inside
//! `:where()`, so it has none (a `::backdrop` rule keeps the pseudo-element's
//! own), and every Lynx rule names a tag, so it outranks all of them. A
//! `<view popover>` therefore keeps its Lynx `display` against
//! `[popover]`'s `display: none`, as it does in a browser under
//! web-elements' `x-view` rule (`linear.css`'s common block, `x-view {
//! display: flex; position: relative; overflow: clip; border-width: 0; … }`):
//! it is never hidden by `[popover]`, and keeps its `position: relative` and
//! borderless box, while the properties web-elements leaves alone — the
//! insets, the fit-content size, the `auto` margins, the padding and the
//! colours — are the popover's.
//!
//! Among themselves the HTML rules then rank by source order alone, and the
//! sheet orders them so that this is HTML's own precedence
//! ([`terminus::link::html`], "The UA rules"): `dialog`'s `display` first; then
//! [`terminus::link::html::dialog`]'s rules, in HTML's order; then
//! [`terminus::link::html::popover`]'s, whose `[popover]` box beats `dialog`'s as
//! its higher specificity does in HTML; then `dialog:popover-open`'s `display`,
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
//! A `dialog` whose `open` is `"false"` is closed, in its rules and its
//! component alike ([`terminus::link::html::dialog`]): `open` is a boolean
//! attribute, this engine's `__SetAttribute` stringifies `false`, and
//! web-core removes an attribute whose value is `"false"` before its CSS sees
//! it (see [`super::swiper`]'s "Boolean attributes and `"false"`").
//!
//! Order within the Lynx rules is mostly documentation, with one exception
//! that is mechanism: [`super::image`]'s and [`super::lynx_svg`]'s child suppression ties on
//! specificity with the `display` rules `view`, `scroll-view`, `list`, `list-item`, the two
//! spellings each of `viewpager` and `viewpager-item`, `x-swiper`,
//! `x-swiper-item`, `x-refresh-view`, `x-refresh-header`, `x-refresh-footer`,
//! the ten `scroll-coordinator` tags, `blur-view`, `x-blur-view`, `overlay`,
//! `x-overlay-ng` (their hosts' `display: contents`) and `wrapper` carry, so
//! it wins only by being assembled last. Those modules' `nothing_inside_an_image_generates_a_box`
//! and `nothing_inside_an_svg_generates_a_box` are the tripwires for it.

use terminus::link::html::dialog::DIALOG_TAG;
use terminus::link::html::{dialog, popover};

use super::blur_view::{BLUR_VIEW_TAG, X_BLUR_VIEW_TAG};
use super::lynx_svg::SVG_TAG;
use super::markdown::{MARKDOWN_TAG, X_MARKDOWN_TAG};
use super::refresh_view::{REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG};
use super::swiper::{SWIPER_ITEM_TAG, SWIPER_TAG};
use super::viewpager::{VIEWPAGER_ITEM_TAG, VIEWPAGER_TAG, X_VIEWPAGER_ITEM_TAG, X_VIEWPAGER_TAG};
use super::{
    frame, image, list, lynx_svg, markdown, overlay, raw_text, refresh_view, scroll_container,
    scroll_coordinator, swiper, text, text_field, video, viewpager,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent Lynx page configuration switches"
)]
pub struct PageConfig {
    pub default_display_linear: bool,
    pub default_overflow_visible: bool,
    pub enable_css_selector: bool,
    pub enable_js_data_processor: bool,
    pub enable_css_inheritance: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
            enable_js_data_processor: false,
            enable_css_inheritance: false,
        }
    }
}

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
    let block = if config.default_display_linear {
        "linear"
    } else {
        "flex"
    };
    let overflow = if config.default_overflow_visible {
        format!("view, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG} {{ overflow: visible; }}\n")
    } else {
        String::new()
    };
    let inheritance = if config.enable_css_inheritance {
        text::CSS_INHERITANCE_RULES
    } else {
        ""
    };
    format!(
        "page, view, scroll-view, list, list-item, {component_tags}, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG}, \
         {MARKDOWN_TAG}, {X_MARKDOWN_TAG}, {text_fields}, text, image, {SVG_TAG}, video, frame {{ box-sizing: border-box; border-width: 0; border-style: solid; \
         position: relative; overflow: clip; min-width: 0; min-height: 0; }}\n\
         {display}\
         {overflow}\
         page {{ width: 100%; height: 100%; flex-direction: column; font-family: sans-serif; }}\n\
         wrapper {{ display: contents; }}\n\
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
         {markdowns}\
         {text_field_rules}\
         {text}\
         {EDITING_HOST_RULE}\n\
         {inheritance}\
         {carriers}\
         {frames}\
         {images}\
         {svgs}\
         {videos}",
        scrollers = scroll_container::UA_RULES,
        lists = list::UA_RULES,
        pagers = viewpager::UA_RULES,
        swipers = swiper::UA_RULES,
        refresh_views = refresh_view::UA_RULES,
        coordinators = scroll_coordinator::UA_RULES,
        dialogs = dialog::UA_RULES,
        popovers = popover::UA_RULES,
        overlays = overlay::UA_RULES,
        markdowns = markdown::UA_RULES,
        text_field_rules = text_field::UA_RULES,
        text_fields = text_field::INPUT_TAGS
            .iter()
            .chain(&text_field::TEXTAREA_TAGS)
            .copied()
            .collect::<Vec<_>>()
            .join(", "),
        text = text::UA_RULES,
        carriers = raw_text::UA_RULES.as_str(),
        frames = frame::UA_RULES,
        images = image::UA_RULES,
        svgs = lynx_svg::UA_RULES,
        videos = video::UA_RULES,
    )
}

pub(super) const EDITING_HOST_RULE: &str =
    "[contenteditable]:not([contenteditable=\"false\" i]) { white-space: pre-wrap; }";

#[cfg(test)]
mod tests {
    use terminus::link::stylo::computed_values::{box_sizing, flex_direction, position};
    use terminus::link::stylo::values::computed::font::{FontFamily, GenericFontFamily};
    use terminus::link::stylo::values::computed::{
        BorderStyle, CSSPixelLength, Display, Overflow, Size,
    };

    use super::super::LynxDocument;
    use super::super::overlay::{OVERLAY_TAG, X_OVERLAY_TAG};
    use super::super::scroll_coordinator::{
        SCROLL_COORDINATOR_HEADER_TAG, SCROLL_COORDINATOR_SLOT_DRAG_TAG,
        SCROLL_COORDINATOR_SLOT_TAG, SCROLL_COORDINATOR_TAG, SCROLL_COORDINATOR_TOOLBAR_TAG,
        X_FOLDVIEW_HEADER_TAG, X_FOLDVIEW_SLOT_DRAG_TAG, X_FOLDVIEW_SLOT_TAG, X_FOLDVIEW_TAG,
        X_FOLDVIEW_TOOLBAR_TAG,
    };
    use super::super::test_support::{
        child, document, element_under, overflow, style_of, with_config,
    };
    use super::{
        BLUR_VIEW_TAG, DIALOG_TAG, PageConfig, REFRESH_FOOTER_TAG, REFRESH_HEADER_TAG,
        SWIPER_ITEM_TAG, SWIPER_TAG, VIEWPAGER_ITEM_TAG, VIEWPAGER_TAG, X_BLUR_VIEW_TAG,
        X_VIEWPAGER_ITEM_TAG, X_VIEWPAGER_TAG, ua_stylesheet,
    };

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

    fn containers(document: &mut LynxDocument) -> Vec<terminus::link::NodeId> {
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

    fn px(size: &Size) -> Option<f32> {
        match size {
            Size::LengthPercentage(length) => length.0.to_length().map(CSSPixelLength::px),
            _ => None,
        }
    }

    #[test]
    fn every_box_tag_gets_the_web_elements_common_block() {
        let mut document = with_config(PageConfig {
            default_overflow_visible: false,
            ..PageConfig::default()
        });
        let mut boxes = containers(&mut document);
        boxes.push(child(&mut document, "text", ""));
        boxes.push(child(&mut document, "image", ""));
        boxes.push(child(&mut document, super::SVG_TAG, ""));
        boxes.push(child(&mut document, "frame", ""));
        document.layout();

        for (tag, element) in CONTAINER_TAGS
            .iter()
            .chain(&["text", "image", super::SVG_TAG, "frame"])
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
            document.add_stylesheet(css, terminus::link::StylesheetOrigin::Author);
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

    #[test]
    fn the_page_defaults_to_column_under_both_display_modes_and_allows_author_override() {
        for (linear, expected) in [(true, Display::Linear), (false, Display::Flex)] {
            let mut document = with_config(PageConfig {
                default_display_linear: linear,
                ..PageConfig::default()
            });
            let page = document.document_element().id();
            let view = child(&mut document, "view", "");
            document.layout();

            let style = style_of(&document, page);
            assert_eq!(*style.get_display(), expected);
            assert_eq!(*style.get_flex_direction(), flex_direction::T::Column);
            assert_eq!(
                *style_of(&document, view).get_flex_direction(),
                flex_direction::T::Row,
                "the root default does not change ordinary views"
            );

            document.add_stylesheet(
                "page { flex-direction: row; }",
                terminus::link::StylesheetOrigin::Author,
            );
            document.layout();
            let style = style_of(&document, page);
            assert_eq!(*style.get_display(), expected);
            assert_eq!(*style.get_flex_direction(), flex_direction::T::Row);
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn gradient_showcase_rectangles_resolve_against_the_page_width() {
        let mut document = with_config(PageConfig {
            default_display_linear: false,
            ..PageConfig::default()
        });
        let page = document.document_element().id();
        document.set_inline_style(page, "width: 400px; height: 600px");
        let scroller = child(&mut document, "scroll-view", "");
        document.set_attribute(scroller, "scroll-orientation", "vertical");
        let area = element_under(
            &mut document,
            scroller,
            "view",
            "height: 480px; flex-direction: column",
        );
        let gradients: &[&[&str]] = &[
            &["linear-gradient(red, yellow)"],
            &[
                "linear-gradient(green 40%, yellow 30%, blue 70%)",
                "linear-gradient(green 40%, yellow 40%, blue 70%)",
            ],
            &[
                "radial-gradient(red, green)",
                "radial-gradient(circle at 100%, #333, #333 50%, #eee 75%, #333 75%)",
            ],
        ];
        let mut rectangles = Vec::new();
        let mut fills = Vec::new();
        for row in gradients {
            let rectangle = element_under(
                &mut document,
                area,
                "view",
                "margin: 5px; text-align: center; border: 1px solid red; width: 50%; height: 100px",
            );
            rectangles.push(rectangle);
            for gradient in *row {
                fills.push(element_under(
                    &mut document,
                    rectangle,
                    "view",
                    &format!("width: 50%; background-image: {gradient}"),
                ));
            }
        }
        document.layout();

        for node in [scroller, area] {
            let layout = document.rounded_layout(node).expect("a laid-out container");
            assert_eq!(layout.size.width, 400.0);
        }
        for rectangle in rectangles {
            let layout = document
                .rounded_layout(rectangle)
                .expect("a laid-out rectangle");
            assert_eq!((layout.size.width, layout.size.height), (200.0, 100.0));
        }
        for fill in fills {
            let layout = document.rounded_layout(fill).expect("a laid-out gradient");
            assert_eq!((layout.size.width, layout.size.height), (99.0, 98.0));
        }
    }

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

    #[test]
    fn the_overflow_page_config_switch_skips_the_scrollers_and_the_page() {
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
            let leaves = [
                "text",
                "image",
                "frame",
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
                (Overflow::Clip, Overflow::Clip),
                "the page clips whatever the switch says: {visible}"
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

    const OVERFLOW_RULE: &str = "view, blur-view, x-blur-view { overflow: visible; }";

    const INHERITANCE_RULE: &str = "view > text, view > wrapper, view > wrapper > text \
                                    { color: inherit; text-decoration: inherit; }";

    #[test]
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);
        assert!(!config.enable_css_inheritance);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(sheet.contains(OVERFLOW_RULE));
        assert!(sheet.contains("box-sizing: border-box;"));
        assert!(!sheet.contains(INHERITANCE_RULE));
    }

    #[test]
    fn ua_switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
            enable_js_data_processor: false,
            enable_css_inheritance: false,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(!sheet.contains(OVERFLOW_RULE));
        assert!(!sheet.contains(INHERITANCE_RULE));
    }

    #[test]
    fn an_editing_host_preserves_white_space_by_default_only() {
        use terminus::link::stylo::computed_values::white_space_collapse::T as Collapse;

        let mut document = document();
        let hosts: Vec<(terminus::link::NodeId, bool)> = [
            ("", true),
            ("true", true),
            ("plaintext-only", true),
            ("FALSE", false),
            ("false", false),
        ]
        .into_iter()
        .map(|(value, editable)| {
            let view = child(&mut document, "view", "");
            document.set_attribute(view, "contenteditable", value);
            (view, editable)
        })
        .collect();
        let normal = child(&mut document, "view", "white-space: normal;");
        document.set_attribute(normal, "contenteditable", "");
        let hidden = child(&mut document, "view", "display: none;");
        document.set_attribute(hidden, "contenteditable", "");
        document.layout();
        let collapse = |view| {
            *style_of(&document, view)
                .get_inherited_text()
                .get_white_space_collapse()
        };
        let display = |view| *style_of(&document, view).get_box().get_display();
        for (view, editable) in hosts {
            assert_ne!(display(view), Display::None);
            assert_eq!(collapse(view) == Collapse::Preserve, editable);
            assert_eq!(document.is_editing_host(view), editable);
        }
        assert_eq!(collapse(normal), Collapse::Collapse);
        assert_eq!(display(hidden), Display::None);
    }

    #[test]
    fn the_css_inheritance_switch_adds_one_rule_and_nothing_else() {
        let off = ua_stylesheet(PageConfig::default());
        let on = ua_stylesheet(PageConfig {
            enable_css_inheritance: true,
            ..PageConfig::default()
        });

        assert_eq!(
            on.lines().filter(|line| *line == INHERITANCE_RULE).count(),
            1
        );
        assert_eq!(on.replacen(&format!("{INHERITANCE_RULE}\n"), "", 1), off);
    }

    #[test]
    fn the_ua_sheet_is_important_free_apart_from_the_recorded_exceptions() {
        const ALLOWED: [&str; 20] = [
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
                enable_css_inheritance: false,
            },
            PageConfig {
                enable_css_inheritance: true,
                ..PageConfig::default()
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
