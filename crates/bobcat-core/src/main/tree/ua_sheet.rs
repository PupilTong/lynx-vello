//! The main-thread Lynx UA cascade: page configuration, defaults, and
//! defaults every container tag shares, and the assembly of the one sheet.
//!
//! Each tag's own policy lives with that tag — [`super::scroll_container`],
//! [`super::list`], [`super::text`], [`super::raw_text`], [`super::image`] —
//! and this module only decides what they all agree on and what order they
//! land in.
//! [`super::blur_view`] is the one tag module with no rules of its own: a
//! blur view is a container and nothing more, so everything it needs is here.
//!
//! Order is mostly documentation, with one exception that is mechanism:
//! [`super::image`]'s child suppression ties on specificity with the `display`
//! rules `view`, `scroll-view`, `list`, `list-item`, `blur-view`,
//! `x-blur-view` and `wrapper` carry, so it wins only by being assembled last.
//! That module's `nothing_inside_an_image_generates_a_box` is the tripwire for
//! it.

use super::blur_view::{BLUR_VIEW_TAG, X_BLUR_VIEW_TAG};
use super::{image, list, raw_text, scroll_container, text};

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
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
            enable_js_data_processor: false,
        }
    }
}

/// The Lynx UA stylesheet: embedder cascade policy `dom` must not know.
///
/// The container tags — `page`, `view`, `scroll-view`, `list`, `list-item`,
/// `blur-view` and `x-blur-view` — share
/// `web-elements`' common block: a border box, and the display mode
/// `defaultDisplayLinear` picks — a per-tag exception to that switch would
/// have to be `!important`, so it is a recorded deviation instead
/// (`docs/tracking/deviations.md`). `text` is a text block whatever the switch
/// says, and `wrapper` generates no box — both from `web-elements`' own sheet,
/// where the linear toggle covers container tags only.
/// Every tag that generates a box of its own — the containers, `text` and
/// `image` — also gets the rest of that common block: `border-width: 0` with
/// `border-style: solid`, `position: relative`, `min-width: 0` and
/// `min-height: 0`, and `overflow: clip`. `clip` is not a scroll container,
/// so a clipped box neither scrolls nor gets its automatic minimum size
/// zeroed by the overflow — the explicit `min-width`/`min-height` are what do
/// that. `defaultOverflowVisible` releases `view` and the two blur-view tags
/// back to `visible`, the way web-core's
/// `[lynx-default-overflow-visible=true] x-view` releases `x-view` alone; a
/// scroller carries its own axes regardless, and a `list-item` stays clipped —
/// by this `clip` and by the paint containment [`super::list`] gives it.
///
/// **`page` is not among them.** Native reads the switch in `ViewElement`
/// alone (`SetDefaultOverflow(element_manager_->GetDefaultOverflowVisible())`,
/// `core/renderer/dom/fiber/view_element.cc`) and pins the page unconditionally
/// in `PageElement` — `SetDefaultOverflow(false)` under the comment "make sure
/// page's default overflow is hidden" (`.../page_element.cc`). web-core names
/// only `x-view` in its release selector, so its page clips too. A card that
/// asks for visible overflow is asking it of its views, not of the window it
/// is drawn in.
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
/// The exceptions are all the paragraph's, and all of the same shape: a fact
/// the cascade is merely *reporting* rather than a default it is *choosing*.
/// `display: -lynx-text` on `text`, on `inline-text` and on a `text`'s own
/// `inline-truncation` child is the first three — Lynx does not decide
/// inline-ness by cascade at all: `ConvertToInlineElement` runs when a child is
/// *added* to a text, and no author CSS can undo it. `padding: 0` on an
/// `image` that is inline content of a paragraph is the fourth: web-core gives
/// the authored element no box at all and rebuilds one in its shadow tree
/// without inheriting `padding`, so there is no box for an author's padding to
/// reach, and a declaration a page could override would misdescribe both
/// references. [`super::text`] carries the citation for each.
/// `the_ua_sheet_is_important_free_apart_from_the_text_block` pins the set to
/// exactly those four rules.
#[must_use]
pub(super) fn ua_stylesheet(config: PageConfig) -> String {
    let display = if config.default_display_linear {
        format!(
            "page, view, scroll-view, list, list-item, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG} \
             {{ display: linear; }}\n"
        )
    } else {
        String::new()
    };
    let overflow = if config.default_overflow_visible {
        format!("view, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG} {{ overflow: visible; }}\n")
    } else {
        String::new()
    };
    format!(
        "page, view, scroll-view, list, list-item, {BLUR_VIEW_TAG}, {X_BLUR_VIEW_TAG}, text, image \
         {{ box-sizing: border-box; border-width: 0; border-style: solid; \
         position: relative; overflow: clip; min-width: 0; min-height: 0; }}\n\
         {display}\
         {overflow}\
         page {{ width: 100%; height: 100%; font-family: sans-serif; }}\n\
         wrapper {{ display: contents; }}\n\
         {scrollers}\
         {lists}\
         {text}\
         {carriers}\
         {images}",
        scrollers = scroll_container::UA_RULES,
        lists = list::UA_RULES,
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
    use super::super::test_support::{child, document, overflow, style_of, with_config};
    use super::{BLUR_VIEW_TAG, PageConfig, X_BLUR_VIEW_TAG, ua_stylesheet};

    /// The tags that get `web-elements`' common container block.
    const CONTAINER_TAGS: [&str; 7] = [
        "page",
        "view",
        "scroll-view",
        "list",
        "list-item",
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
            assert_eq!(style.clone_box_sizing(), box_sizing::T::BorderBox, "{tag}");
            assert_eq!(style.clone_display(), Display::Linear, "{tag}");
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
        document.layout();

        for (tag, element) in CONTAINER_TAGS.iter().chain(&["text", "image"]).zip(boxes) {
            let style = style_of(&document, element);
            assert_eq!(style.clone_box_sizing(), box_sizing::T::BorderBox, "{tag}");
            let border = style.get_border();
            for (side_style, side_width) in [
                (
                    border.clone_border_top_style(),
                    border.clone_border_top_width(),
                ),
                (
                    border.clone_border_right_style(),
                    border.clone_border_right_width(),
                ),
                (
                    border.clone_border_bottom_style(),
                    border.clone_border_bottom_width(),
                ),
                (
                    border.clone_border_left_style(),
                    border.clone_border_left_width(),
                ),
            ] {
                assert_eq!(side_style, BorderStyle::Solid, "{tag}");
                assert_eq!(side_width.0.to_px(), 0, "{tag}");
            }
            assert_eq!(style.clone_position(), position::T::Relative, "{tag}");
            assert_eq!(px(&style.clone_min_width()), Some(0.0), "{tag}");
            assert_eq!(px(&style.clone_min_height()), Some(0.0), "{tag}");
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
            style.get_border().clone_border_top_style(),
            BorderStyle::Solid
        );
        assert_eq!(style.get_border().clone_border_top_width().0.to_px(), 2);
        assert_eq!(style.clone_position(), position::T::Absolute);
        assert_eq!(px(&style.clone_min_width()), Some(10.0));
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
                    &style_of(&document, node).get_font().clone_font_family(),
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
                style.clone_display(),
                Display::Flex,
                "a scroller follows `defaultDisplayLinear` the way a view does: {tag}"
            );
            assert_eq!(style.clone_box_sizing(), box_sizing::T::BorderBox, "{tag}");
        }
    }

    /// Every box clips by default (`overflow: clip`, not a scroll container);
    /// the switch releases the containers that are not scrollers — `view` and
    /// the two blur-view tags, which native treats as views (`LynxUIBlurView`
    /// extends `LynxUIView`) — and nothing else.
    ///
    /// `page` is deliberately not among them: native pins it in
    /// `PageElement::PageElement` with `SetDefaultOverflow(false)` and reads
    /// the switch in `ViewElement` alone, and web-core's release selector
    /// names only `x-view`. A card asking for visible overflow asks it of its
    /// views, not of the window it is drawn in.
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
            let leaves = ["text", "image"].map(|tag| (tag, child(&mut document, tag, "")));
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
        }
    }

    /// The whole rule `defaultOverflowVisible` gates, spelled out once.
    const OVERFLOW_RULE: &str = "view, blur-view, x-blur-view { overflow: visible; }";

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
    #[test]
    fn the_ua_sheet_is_important_free_apart_from_the_text_block() {
        const ALLOWED: [&str; 4] = [
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
            },
        ] {
            let sheet = ua_stylesheet(config);
            let important: Vec<&str> = sheet.lines().filter(|line| line.contains('!')).collect();
            assert_eq!(
                important, ALLOWED,
                "a UA-origin important declaration outranks author `!important`, \
                 so the set of them is fixed by §D.15's one recorded exception"
            );
        }
    }
}
