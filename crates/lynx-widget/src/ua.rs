//! The user-agent default stylesheet for Lynx built-in elements, and the
//! page-config knobs that parameterize it.
//!
//! Per `docs/style-assumptions.md` §D.14/§D.15: built-in component defaults
//! live in **one UA-origin stylesheet** in the stylist (correct cascade-origin
//! semantics for free — author styles override naturally), and pageConfig
//! flags are honored **by generating different UA styles**, never by branches
//! inside the styling engine. This mirrors how web-core works (host styles in
//! `web-elements` + attribute-keyed static rules), with the web-DOM tricks
//! (`--lynx-*` custom-property toggles, `@container style()` linear
//! emulation, `x-` tag names) dropped: the lynx-patched stylo parses
//! `display: linear` and friends natively.
//!
//! Divergences from CSS initial values are Lynx defaults, recorded in
//! `docs/tracking/deviations.md`: `box-sizing: border-box`,
//! `overflow: hidden`, `display: linear` (when `defaultDisplayLinear`, the
//! default), `position: relative`, and zero-width solid borders.

use std::fmt::Write;

/// The style-relevant page configuration, decoded from a bundle's `pageConfig`.
///
/// Only knobs the **style layer** consumes live here; they become generated
/// UA styles (see [`ua_stylesheet`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PageConfig {
    /// `defaultDisplayLinear`: unstyled containers default to
    /// `display: linear` (`true`, Lynx's default) or `display: flex`.
    pub default_display_linear: bool,
    /// `defaultOverflowVisible`: `<view>` defaults to `overflow: visible`
    /// instead of Lynx's default `hidden`.
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

/// Generate the UA stylesheet for the given page configuration.
#[must_use]
pub(crate) fn ua_stylesheet(config: PageConfig) -> String {
    let container_display = if config.default_display_linear {
        "linear"
    } else {
        "flex"
    };

    let mut css = String::with_capacity(1024);

    // The shared element base (web-elements' `linear.css` base rule, with
    // native values: `overflow: hidden` — Lynx's actual default — instead of
    // the web target's `clip`, which the lynx stylo build does not parse).
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

    // Text is always a flex-like text container, never linear (web-elements'
    // x-text host style wins over the base rule the same way).
    css.push_str("text { display: flex; }");

    // Images center their content box (web-elements x-image parity).
    css.push_str("image { justify-content: center; align-items: center; }");

    // raw-text carries character data for the text engine; it never
    // generates its own box (web-core parity: `raw-text { display: none }`).
    css.push_str("raw-text { display: none; }");

    // scroll-view: scrolling axis from the Lynx attributes.
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
        // web-core parity: `[lynx-default-overflow-visible="true"] x-view`
        // targets only views.
        css.push_str("view { overflow: visible; }");
    }

    css
}
