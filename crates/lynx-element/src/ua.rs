//! The Lynx UA stylesheet.
//!
//! `AGENTS.md` keeps Lynx's computed defaults out of the DOM core explicitly:
//! "Lynx computed defaults (border-box, `overflow: hidden`, `display: linear`
//! on every element, …) stay embedder cascade policy (UA sheet)." This module
//! is that policy.

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
    format!(
        "page, view {{ box-sizing: border-box; {display} {overflow} }}\n\
         page {{ width: 100%; height: 100%; }}\n"
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
