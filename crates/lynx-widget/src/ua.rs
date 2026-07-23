//! The user-agent default stylesheet for Lynx built-in elements, and the
//! page-config knobs that parameterize it.

use std::fmt::Write;

/// The style-relevant page configuration, decoded from a bundle's `pageConfig`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageConfig {
    pub default_display_linear: bool,
    pub default_overflow_visible: bool,
}

impl Default for PageConfig {
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: false,
        }
    }
}

#[must_use]
pub(crate) fn ua_stylesheet(config: PageConfig) -> String {
    let container_display = if config.default_display_linear {
        "linear"
    } else {
        "flex"
    };

    let mut css = String::with_capacity(1024);

    let _ = write!(
        css,
        "page, view, text, image, scroll-view, list, list-item, wrapper {{\
           display: {container_display};\
           box-sizing: border-box;\
           border-width: 0;\
           border-style: solid;\
           position: relative;\
           min-width: 0;\
           min-height: 0;\
           overflow: hidden;\
         }}"
    );

    css.push_str("text { display: flex; }");

    css.push_str("image { justify-content: center; align-items: center; }");

    css.push_str("raw-text { display: none; }");

    css.push_str("scroll-view { display: flex; flex-direction: row; flex-wrap: nowrap; }");
    css.push_str(
        "scroll-view[scroll-y], scroll-view[scroll-orientation=\"vertical\"] {\
           flex-direction: column;\
           overflow-y: scroll;\
           overflow-x: hidden;\
         }",
    );
    css.push_str(
        "scroll-view[scroll-x], scroll-view[scroll-orientation=\"horizontal\"] {\
           flex-direction: row;\
           overflow-x: scroll;\
           overflow-y: hidden;\
         }",
    );

    if config.default_overflow_visible {
        css.push_str("view { overflow: visible; }");
    }

    css
}
