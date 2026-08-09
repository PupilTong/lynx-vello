//! The Lynx UA stylesheet.
//!
//! `AGENTS.md` keeps Lynx's computed defaults out of the DOM core explicitly:
//! "Lynx computed defaults (border-box, `overflow: hidden`, `display: linear`
//! on every element, …) stay embedder cascade policy (UA sheet)." This module
//! is that policy.

/// The page-config switches that change the UA cascade.
///
/// These are read from the decoded `.web.bundle` `Configurations` section —
/// web-core bakes the same three booleans into its element-API closures in
/// `onPageConfigReady`, before any main-thread script runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageConfig {
    /// `defaultDisplayLinear` — every element defaults to `display: linear`
    /// (vertical), Lynx's default box model, rather than a W3C block/flow one.
    pub default_display_linear: bool,
    /// `defaultOverflowVisible` — elements default to `overflow: visible`
    /// instead of Lynx's `overflow: hidden`.
    pub default_overflow_visible: bool,
    /// `enableCSSSelector` — whether author CSS is matched as real selectors.
    ///
    /// Recorded as configuration, but nothing reads it yet: this crate has no
    /// `__SetCSSId`, so there is no per-component CSS-scope path to switch.
    pub enable_css_selector: bool,
}

impl Default for PageConfig {
    /// The defaults a `.web.bundle` built by today's toolchain carries:
    /// `defaultDisplayLinear` and `defaultOverflowVisible` both `"true"`, CSS
    /// selectors enabled.
    fn default() -> Self {
        Self {
            default_display_linear: true,
            default_overflow_visible: true,
            enable_css_selector: true,
        }
    }
}

/// The UA stylesheet for `config`.
///
/// The compatibility target is web-core, so the tag defaults below are the ones
/// `web-elements` authors — not the native engine's, which resolves every
/// element's `display: auto` through the page config and then measures `<text>`
/// with a platform paragraph function instead of a box algorithm.
///
/// `linear-direction` already computes to `column` initially in the fork's
/// grammar, which is Lynx's vertical default, so `display: linear` alone
/// reproduces "linear/vertical on a container".
///
/// The `display` values are load-bearing rather than cosmetic: the layout
/// engine routes everything that is not flex/grid/linear/relative to a leaf,
/// and a leaf hides its children, so a tag left at the CSS initial
/// `display: inline` would silently drop its content.
#[must_use]
pub(crate) fn ua_stylesheet(config: PageConfig) -> String {
    // `defaultDisplayLinear` toggles the *container* elements only. web-core
    // installs that toggle on a narrower selector list than its box defaults
    // (`web-elements`' `linear.css`) — `x-text` and `x-image` are deliberately
    // absent from it and are row flex containers whatever the switch says.
    // `page` and `view` are the container tags this runtime creates.
    let container_display = if config.default_display_linear {
        "display: linear;"
    } else {
        ""
    };
    let overflow = if config.default_overflow_visible {
        ""
    } else {
        "overflow: hidden;"
    };
    // `page` is sized to the viewport: it is the containing block every other
    // element resolves percentages against, and the element `position: fixed`
    // anchors to.
    //
    // `image` takes `contain: strict`, which is what keeps a Lynx image sized
    // purely by CSS: unlike a W3C `<img>`, it never takes its bitmap's natural
    // size unless `auto-size` is set, and size containment is how web-core
    // spells that.
    //
    // `raw-text` generates no box of its own — it is a virtual node the parent
    // text consumes — so it is `display: none` outside a text and
    // `display: contents` inside one, which splices its content into the text's
    // own item list.
    format!(
        "page, view {{ box-sizing: border-box; {container_display} {overflow} }}\n\
         page {{ width: 100%; height: 100%; }}\n\
         text, image {{ display: flex; box-sizing: border-box; position: relative; \
         border-width: 0; border-style: solid; overflow: clip; \
         min-width: 0; min-height: 0; }}\n\
         text {{ align-items: stretch; overflow-wrap: break-word; color: initial; }}\n\
         text > text {{ color: inherit; }}\n\
         image {{ contain: strict; object-fit: fill; flex-direction: row; \
         align-items: center; justify-content: center; }}\n\
         raw-text {{ display: none; white-space-collapse: preserve-breaks; }}\n\
         text > raw-text {{ display: contents; }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{PageConfig, ua_stylesheet};

    #[test]
    fn default_config_is_linear_and_overflow_visible() {
        let config = PageConfig::default();
        assert!(config.default_display_linear);
        assert!(config.default_overflow_visible);

        let sheet = ua_stylesheet(config);
        assert!(sheet.contains("display: linear;"));
        assert!(!sheet.contains("overflow: hidden;"));
        assert!(sheet.contains("box-sizing: border-box;"));
    }

    #[test]
    fn switches_drop_the_declarations_they_gate() {
        let sheet = ua_stylesheet(PageConfig {
            default_display_linear: false,
            default_overflow_visible: false,
            enable_css_selector: true,
        });
        assert!(!sheet.contains("display: linear;"));
        assert!(sheet.contains("overflow: hidden;"));
    }
}
