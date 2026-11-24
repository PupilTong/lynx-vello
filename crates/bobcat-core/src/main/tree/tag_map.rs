//! The Lynx tags whose document element has another name: one, `svg`, which
//! is the [`super::lynx_svg`] component `lynx-svg` in the document, because
//! the tag `svg` is left for the SVG element itself (ruled 2026-11-23).
//!
//! web-core maps every Lynx tag to its own custom element the same way
//! (`svg` → `x-svg`), from one table its element creation and its
//! `StyleInfo` decoder share (`lynx-stack/packages/web-platform/web-core/
//! src/constants.rs:25-55`, `ts/constants.ts:93-130`). This table is that one
//! table here, and is read in the same places:
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
//! A page that creates `lynx-svg` itself gets the same element, whose
//! `__GetTag` then answers `svg`.

use super::lynx_svg;

/// Each Lynx tag that is mapped, with its document tag.
const TAGS: [(&str, &str); 1] = [("svg", lynx_svg::SVG_TAG)];

/// The document tag a page's tag creates, and a `StyleInfo` type selector
/// names.
#[must_use]
pub fn document_tag(lynx_tag: &str) -> &str {
    TAGS.iter()
        .find(|(lynx, _)| *lynx == lynx_tag)
        .map_or(lynx_tag, |&(_, document)| document)
}

/// The tag a page reads for a document element's tag.
pub(crate) fn lynx_tag(document_tag: &str) -> &str {
    TAGS.iter()
        .find(|(_, document)| *document == document_tag)
        .map_or(document_tag, |&(lynx, _)| lynx)
}

#[cfg(test)]
mod tests {
    use super::{document_tag, lynx_tag};

    #[test]
    fn svg_is_the_one_mapped_tag() {
        assert_eq!(document_tag("svg"), "lynx-svg");
        assert_eq!(lynx_tag("lynx-svg"), "svg");
        for tag in ["view", "image", "x-svg", "SVG", "lynx-svg"] {
            assert_eq!(document_tag(tag), tag);
        }
        for tag in ["view", "svg", "x-svg"] {
            assert_eq!(lynx_tag(tag), tag);
        }
    }
}
