//! The main-thread Lynx UA cascade: page configuration, defaults, and
//! defaults every container tag shares, and the assembly of the one sheet.
//!
//! Each tag's own policy lives with that tag — [`super::scroll_container`],
//! [`super::text`], [`super::raw_text`] — and this module only decides what
//! they all agree on and what order they land in.

use super::{raw_text, scroll_container, text};

/// Page configuration that controls the Lynx UA cascade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageConfig {
    /// Whether elements default to `display: linear`.
    pub default_display_linear: bool,
    /// Whether elements default to visible overflow.
    pub default_overflow_visible: bool,
    /// Whether author CSS selector matching is enabled.
    pub enable_css_selector: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
        }
    }
}

/// The Lynx UA stylesheet: embedder cascade policy `dom` must not know.
///
/// The container tags — `page`, `view`, `scroll-view`, `list` — share
/// `web-elements`' common block: a border box, and the display mode
/// `defaultDisplayLinear` picks — a per-tag exception to that switch would
/// have to be `!important`, so it is a recorded deviation instead
/// (`docs/tracking/deviations.md`). `text` is a flex container whatever the
/// switch says, and `wrapper` generates no box — both from `web-elements`'
/// own sheet, where the linear toggle covers container tags only.
/// `defaultOverflowVisible` reaches `page` and `view` alone, the way web-core
/// spends it on `x-view` alone; a scroller carries its own axes regardless.
///
/// Nothing here is `!important`. web-elements' defaults are author origin in
/// the browser and several of them lean on `!important`; ours are user-agent
/// origin, where an important declaration outranks author `!important` instead
/// of losing to it, so the sheet stays important-free and what web-elements
/// forces is written as a plain declaration a page's own CSS can still
/// override (`docs/style-assumptions.md` §D.15).
#[must_use]
pub(super) fn ua_stylesheet(config: PageConfig) -> String {
    let display = if config.default_display_linear {
        "display: linear;"
    } else {
        ""
    };
    let overflow = if config.default_overflow_visible {
        ""
    } else {
        "page, view { overflow: hidden; }\n"
    };
    format!(
        "page, view, scroll-view, list {{ box-sizing: border-box; {display} }}\n\
         {overflow}\
         page {{ width: 100%; height: 100%; }}\n\
         wrapper {{ display: contents; }}\n\
         {scrollers}\
         {text}\
         {carriers}",
        scrollers = scroll_container::UA_RULES,
        text = text::UA_RULES,
        carriers = raw_text::UA_RULES,
    )
}

#[cfg(test)]
mod tests {
    use dom::stylo::computed_values::box_sizing;
    use dom::stylo::values::computed::{Display, Overflow};

    use super::super::LynxDocument;
    use super::super::test_support::{child, document, overflow, style_of, with_config};
    use super::{PageConfig, ua_stylesheet};

    /// The tags that get `web-elements`' common container block.
    const CONTAINER_TAGS: [&str; 4] = ["page", "view", "scroll-view", "list"];

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

    #[test]
    fn the_overflow_page_config_switch_reaches_page_and_view_alone() {
        for (visible, expected) in [(true, Overflow::Visible), (false, Overflow::Hidden)] {
            let mut document = with_config(PageConfig {
                default_overflow_visible: visible,
                ..PageConfig::default()
            });
            let view = child(&mut document, "view", "");
            let scroller = child(&mut document, "scroll-view", "");
            let list = child(&mut document, "list", "");
            document.layout();

            assert_eq!(overflow(&document, view), (expected, expected), "{visible}");
            for scroller in [scroller, list] {
                assert_eq!(
                    overflow(&document, scroller),
                    (Overflow::Hidden, Overflow::Scroll),
                    "a scroller keeps its own axes whatever the switch says: {visible}"
                );
            }
        }
    }

    #[test]
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(!sheet.contains("page, view { overflow: hidden; }"));
        assert!(sheet.contains("box-sizing: border-box;"));
    }

    #[test]
    fn ua_switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(sheet.contains("page, view { overflow: hidden; }"));
    }

    #[test]
    fn the_ua_sheet_carries_no_important_declaration() {
        for config in [
            PageConfig::default(),
            PageConfig {
                default_display_linear: false,
                default_overflow_visible: false,
                enable_css_selector: false,
            },
        ] {
            assert!(
                !ua_stylesheet(config).contains('!'),
                "a UA-origin important declaration would outrank author `!important`"
            );
        }
    }
}
