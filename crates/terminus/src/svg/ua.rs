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

const UA_RULES: &str = "\
@namespace url(http://www.w3.org/2000/svg);
*:not(svg), *:not(foreignObject) > svg { transform-origin: 0 0; }
";

static SVG_SHEET: LazyLock<UserAgentStylesheet> =
    LazyLock::new(|| UserAgentStylesheet::parse(UA_RULES));

impl<T> Document<T> {
    pub fn add_svg_user_agent_stylesheet(&mut self) {
        self.add_user_agent_stylesheet(&SVG_SHEET);
    }
}

const IMAGE_RULES: &str = ":root { font-family: sans-serif; }";

static IMAGE_SHEET: LazyLock<UserAgentStylesheet> =
    LazyLock::new(|| UserAgentStylesheet::parse(IMAGE_RULES));

impl<T> Document<T> {
    pub(crate) fn add_svg_image_stylesheet(&mut self) {
        self.add_user_agent_stylesheet(&IMAGE_SHEET);
    }
}
