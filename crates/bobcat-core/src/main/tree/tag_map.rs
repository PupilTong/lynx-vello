//! The Lynx tags whose document element has another name. There are three:
//!
//! - `svg`, which is the [`super::lynx_svg`] component `lynx-svg` in the document, because the tag
//!   `svg` is left for the SVG element itself (ruled 2026-11-23);
//! - `input` and `textarea`, which are the [`super::text_field`] fields `x-input` and `x-textarea`,
//!   web-core's element names (user ruling 2026-11-26).
//!
//! web-core maps every Lynx tag to its own custom element the same way
//! (`svg` → `x-svg`, `input` → `x-input`, `textarea` → `x-textarea`), from
//! one table its element creation and its `StyleInfo` decoder share
//! (`lynx-stack/packages/web-platform/web-core/src/constants.rs:25-55`,
//! `ts/constants.ts:93-130`). This table is that one table here, and is read
//! in the same places:
//!
//! - creation: the host member `createElement`, which every `__Create*` of the Element PAPI
//!   reaches, creates [`document_tag`];
//! - the tag read: the host member `tagName` — `__GetTag` and a `SelectorQuery`'s `tag` field —
//!   answers [`lynx_tag`];
//! - a `StyleInfo` type selector: `bobcat-source`'s lowering (`lower_style.rs`) writes a `Type`
//!   component through [`document_tag`], as web-core's decoder does
//!   (`web-core/src/template/template_sections/style_info/style_info_decoder.rs:148-156`).
//!
//! Nothing else is mapped, as in web-core: a selector query's selector goes
//! to the document unchanged (`web-core/ts/client/mainthread/elementAPIs/
//! pureElementPAPIs.ts:242`), so the type selector `svg` finds no element
//! and `lynx-svg` does; nor is a stylesheet that arrives as text, or selector
//! text inside a pseudo-class argument (`:not(svg)`).
//!
//! The table is read both ways, so a page that creates `lynx-svg`,
//! `x-input` or `x-textarea` itself gets the same element, whose `__GetTag`
//! then answers `svg`, `input` or `textarea`. For `x-textarea` web-core
//! answers the same; for `x-input` it answers `x-input-ng`
//! (`ts/constants.ts:125-136`, `HTML_TAG_TO_LYNX_TAG_MAP`), and native Lynx
//! answers the tag the element was created under
//! (`docs/tracking/deviations.md`). An element created as `x-input-ng` or
//! `x-textarea-ng` keeps that document tag and reads it.

use super::{lynx_svg, text_field};

const TAGS: [(&str, &str); 3] = [
    ("svg", lynx_svg::SVG_TAG),
    ("input", text_field::INPUT_TAGS[0]),
    ("textarea", text_field::TEXTAREA_TAGS[0]),
];

#[must_use]
pub fn document_tag(lynx_tag: &str) -> &str {
    TAGS.iter()
        .find(|(lynx, _)| *lynx == lynx_tag)
        .map_or(lynx_tag, |&(_, document)| document)
}

pub(crate) fn lynx_tag(document_tag: &str) -> &str {
    TAGS.iter()
        .find(|(_, document)| *document == document_tag)
        .map_or(document_tag, |&(lynx, _)| lynx)
}

#[cfg(test)]
mod tests {
    use super::{TAGS, document_tag, lynx_tag};

    #[test]
    fn svg_input_and_textarea_are_the_mapped_tags() {
        assert_eq!(TAGS.len(), 3);
        assert_eq!(document_tag("svg"), "lynx-svg");
        assert_eq!(lynx_tag("lynx-svg"), "svg");
        assert_eq!(document_tag("input"), "x-input");
        assert_eq!(lynx_tag("x-input"), "input");
        assert_eq!(document_tag("textarea"), "x-textarea");
        assert_eq!(lynx_tag("x-textarea"), "textarea");
        for tag in [
            "view",
            "image",
            "x-svg",
            "SVG",
            "lynx-svg",
            "x-input",
            "x-input-ng",
            "x-textarea",
            "x-textarea-ng",
            "INPUT",
        ] {
            assert_eq!(document_tag(tag), tag);
        }
        for tag in [
            "view",
            "svg",
            "x-svg",
            "input",
            "textarea",
            "x-input-ng",
            "x-textarea-ng",
        ] {
            assert_eq!(lynx_tag(tag), tag);
        }
    }
}
