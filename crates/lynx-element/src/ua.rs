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
    /// Recorded as configuration, but nothing reads it yet: `__SetCSSId`
    /// records a CSS fragment id without any per-fragment stylesheet to scope
    /// against, so there is no alternative matching path to switch to.
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
/// `linear-direction` already computes to `column` initially in the fork's
/// grammar, which is Lynx's vertical default, so `display: linear` alone
/// reproduces "linear/vertical on every element".
///
/// The selector really is `*`: the switch's name is `defaultDisplayLinear` and
/// Lynx applies it to every element, and `__CreateElement` accepts an
/// arbitrary tag, so a tag list could never be complete.
///
/// `raw-text` is the one exception, and it is not a Lynx quirk — a `raw-text`
/// element generates no box in Lynx either, its content belongs to the
/// enclosing `<text>`'s formatting context. `display: contents` is the W3C
/// spelling of exactly that: the element is spliced out of every item
/// collection while its text child still inherits through it, so the text is
/// measured as an item of the `<text>` box rather than inside a nested one.
#[must_use]
pub(crate) fn ua_stylesheet(config: PageConfig) -> String {
    let display = if config.default_display_linear {
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
    format!(
        "* {{ box-sizing: border-box; {display} {overflow} }}\n\
         page {{ width: 100%; height: 100%; }}\n\
         raw-text {{ display: contents; }}\n"
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
