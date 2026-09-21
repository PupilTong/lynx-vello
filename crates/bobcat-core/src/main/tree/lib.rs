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
//! One module per tag, each owning that tag's UA rules and its tests — four
//! of them additionally own a component, because what those tags carry arrives
//! as an attribute and has to become something else: [`image`]'s `src` becomes
//! replaced content, [`blur_view`]'s `blur-radius` becomes a
//! `backdrop-filter` presentational hint, [`text`]'s paragraph limits become
//! the registered custom properties the text block reads, and [`list`]'s lane
//! count, sticky offset and cell estimate become the custom properties its UA
//! rules resolve against. ([`raw_text`]'s run becomes a text node without a
//! component at all: `content: attr(text)` is the whole of it.)
//!
//! **An attribute-to-CSS mapping lives in its own tag's component, never in a
//! shared name-keyed dispatcher** (user ruling, 2026-09-21): a hint is written
//! where that tag's policy is written, by the `attribute_changed_callback`
//! `dom` already raises on the tags that declare the name observed, so an
//! attribute means what the tag it was written on says it means and nothing
//! anywhere else has to be consulted to find out. What the tags do share is a
//! value reader — [`parse_count`], the numeric grammar every Lynx attribute is
//! written in — and the cascade order [`ua_sheet`] gives them; this file mints
//! the document they all describe.
//!
//! [`NodeId`]: dom::NodeId

mod blur_view;
mod image;
mod list;
pub(crate) mod raw_text;
mod scroll_container;
#[cfg(test)]
mod test_support;
mod text;
mod ua_sheet;
#[cfg(test)]
mod web_text_replication;

use dom::{Document, StylesheetOrigin};

pub(crate) use self::image::ImageOutcomes;
pub use self::ua_sheet::PageConfig;
pub(crate) use crate::view::Viewport;

/// The number a Lynx attribute's text names, read the way every reference
/// reads one.
///
/// Shared by the tag components because it is a *parser*, not a policy: every
/// number arrives as a string, since `__SetAttribute` stringifies whatever the
/// compiled card passed, and web-core reads each of them with JavaScript's
/// `parseFloat` (`XTextTruncation.ts:356`, `XListAttributes.ts:29-38`,
/// `ListItemAttributes.ts:22-27`). What a parsed number *means* — a line
/// count, a lane count, a pixel length — stays with the component that
/// observes the attribute.
///
/// `parseFloat` semantics: decimal prefixes and exponents work, empty and
/// negative values do not. Counts beyond a paragraph's u32 source space are
/// effectively unlimited, so the reader stops there.
fn parse_count(value: Option<&str>) -> Option<f64> {
    let value = value?
        .trim_start_matches(|c: char| (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}');
    let bytes = value.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        exponent += usize::from(matches!(bytes.get(exponent), Some(b'+' | b'-')));
        let digits = exponent;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
        }
        if exponent > digits {
            end = exponent;
        }
    }
    value[..end]
        .parse::<f64>()
        .ok()
        .filter(|count| (0.0..=f64::from(u32::MAX)).contains(count))
}

/// The one document shape the runtime speaks.
pub(crate) type LynxDocument = Document<()>;

pub(crate) const PAGE_TAG: &str = "page";

/// Creates the document with its permanent `page` element, the components the
/// engine defines, and the UA cascade.
///
/// `outcomes` is the queue the `image` component leaves a `src` that settled at
/// its bind in, for the runtime to dispatch once it is out of the JavaScript
/// call that wrote it.
#[must_use]
pub(crate) fn new_document(
    viewport: Viewport,
    config: PageConfig,
    outcomes: ImageOutcomes,
) -> LynxDocument {
    let mut document = Document::new(viewport.device(), PAGE_TAG, ());
    blur_view::define(&mut document);
    image::define(&mut document, outcomes);
    list::define(&mut document);
    text::define(&mut document);
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
