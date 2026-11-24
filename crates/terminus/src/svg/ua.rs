//! The user-agent rules SVG content needs.
//!
//! One sheet, parsed once per process and shared by every document that
//! mounts it ([`UserAgentStylesheet`]). Its default namespace is SVG's, so
//! its rules match SVG-namespace elements only. `Document::new` does not
//! mount it; a document that holds SVG elements does, through
//! `Document::add_svg_user_agent_stylesheet`.

use std::sync::LazyLock;

use crate::style::engine::UserAgentStylesheet;
use crate::tree::document::Document;

/// The rules, each with the browser rule it comes from.
///
/// - `transform-origin: 0 0` on every SVG element but an outermost `svg` (SVG 2 §8.5: the initial
///   origin of an SVG element's transform is its user space's origin, where CSS's is the centre of
///   the border box): Chromium's `core/html/resources/svg.css` and Gecko's `layout/svg/svg.css`
///   have this selector and declaration.
const UA_RULES: &str = "\
@namespace url(http://www.w3.org/2000/svg);
*:not(svg), *:not(foreignObject) > svg { transform-origin: 0 0; }
";

static SVG_SHEET: LazyLock<UserAgentStylesheet> =
    LazyLock::new(|| UserAgentStylesheet::parse(UA_RULES));

impl<T> Document<T> {
    /// Mounts the SVG user-agent sheet, the counterpart of the HTML
    /// elements' `UA_RULES` an embedder adds: a document that holds SVG
    /// elements mounts it once.
    pub fn add_svg_user_agent_stylesheet(&mut self) {
        self.add_user_agent_stylesheet(&SVG_SHEET);
    }
}
