//! Main-thread Lynx page policy: the `page` root tag, the UA
//! cascade defaults, the components the engine defines, and view metrics.
//! Everything else the runtime does goes
//! straight to [`dom::Document`] — element identity is the DOM [`NodeId`],
//! which is also the element's Lynx `unique_id`: one number, issued by the
//! DOM, never reissued after the element is freed. Script therefore cannot
//! name a stranger by holding an id too long, only something that no longer
//! exists. The private host boundary still validates script-provided IDs and
//! mutation preconditions before entering `dom`, returning misuse as a
//! JavaScript error.
//!
//! One module per tag, each owning that tag's UA rules and its tests —
//! [`raw_text`] additionally owns the `raw-text` component, because a run
//! reaches the engine as an attribute and has to become a text node. What the
//! tags share, and the order they cascade in, is [`ua_sheet`]; this file only
//! mints the document they all describe.
//!
//! [`NodeId`]: dom::NodeId

pub(crate) mod raw_text;
mod scroll_container;
#[cfg(test)]
mod test_support;
mod text;
mod ua_sheet;

use dom::{Document, StylesheetOrigin};

pub use self::ua_sheet::PageConfig;
pub(crate) use crate::view::Viewport;

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
#[must_use]
pub(crate) fn new_document(viewport: Viewport, config: PageConfig) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    raw_text::define(&mut document);
    document.add_stylesheet(
        &ua_sheet::ua_stylesheet(config),
        StylesheetOrigin::UserAgent,
    );
    document
}

#[cfg(test)]
mod tests {
    use super::test_support::document;

    #[test]
    fn a_layout_pass_sizes_the_page_to_the_viewport() {
        let mut document = document();
        let page = document.document_element().id();
        document.layout();
        let layout = document
            .rounded_layout(page)
            .expect("the page is laid out after the pass");
        assert!((layout.size.width - 393.0).abs() < f32::EPSILON);
        assert!((layout.size.height - 727.0).abs() < f32::EPSILON);
    }
}
